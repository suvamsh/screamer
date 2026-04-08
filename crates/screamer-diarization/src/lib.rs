mod assets;

pub use assets::{
    path_file_name, write_manifest, AmbientDiarizationAssetFile,
    AmbientDiarizationAssetManifest, AmbientDiarizationAssetSet,
    AmbientDiarizationModelSpec, AmbientDiarizationPipelineManifest,
    AmbientModelInputLayout, AmbientModelOutputLayout, AMBIENT_DIARIZATION_DIR_ENV,
    ASSET_MANIFEST_NAME, BUILTIN_ASSET_VERSION,
};

use screamer_core::ambient::{merge_segment, CanonicalSegment, SpeakerLabel, TranscriptSegment};
use screamer_core::speaker::SpeakerEmbedding;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Instant;

const SHORT_REGION_MS: u64 = 375;
const SHORT_REGION_ATTACH_GAP_MS: u64 = 240;
#[cfg_attr(not(feature = "ort-coreml"), allow(dead_code))]
const DEFAULT_SPEECH_SNAP_COLLAR_MS: u64 = 80;
#[cfg_attr(not(feature = "ort-coreml"), allow(dead_code))]
const DEFAULT_NEAREST_SPEECH_ATTACH_MS: u64 = 320;
const DEFAULT_CLUSTERING_SIMILARITY_THRESHOLD: f32 = 0.90;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NativeFinalPassDiagnostics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(default)]
    pub detected_speakers: usize,
    #[serde(default)]
    pub transcription_ms: u64,
    #[serde(default)]
    pub alignment_ms: u64,
    #[serde(default)]
    pub diarization_ms: u64,
    #[serde(default)]
    pub assignment_ms: u64,
    #[serde(default)]
    pub segmentation_ms: u64,
    #[serde(default)]
    pub embedding_ms: u64,
    #[serde(default)]
    pub clustering_ms: u64,
    #[serde(default)]
    pub total_ms: u64,
    #[serde(default)]
    pub real_time_factor: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeFinalPassResult {
    pub engine: String,
    pub transcript_text: String,
    pub segments: Vec<CanonicalSegment>,
    pub diagnostics: NativeFinalPassDiagnostics,
}

#[derive(Clone, Debug)]
pub struct NativeFinalPassRequest<'a> {
    pub sample_rate_hz: usize,
    pub samples: &'a [f32],
    pub transcript_segments: &'a [TranscriptSegment],
    pub transcript_text: &'a str,
}

#[derive(Clone, Debug)]
struct CandidateRegion {
    start_ms: u64,
    end_ms: u64,
    segment_indices: Vec<usize>,
    embedding: Option<RegionEmbedding>,
    cluster_id: Option<usize>,
}

#[derive(Clone, Debug)]
struct Cluster {
    region_indices: Vec<usize>,
    centroid: RegionEmbedding,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "ort-coreml"), allow(dead_code))]
struct SpeechRegion {
    start_ms: u64,
    end_ms: u64,
}

#[derive(Clone, Debug)]
struct ModelPipelineResult {
    regions: Vec<CandidateRegion>,
    segmentation_ms: u64,
    embedding_ms: u64,
    runtime_backend: String,
}

#[derive(Clone, Debug)]
struct RegionEmbedding {
    values: Vec<f32>,
    mfcc: Option<SpeakerEmbedding>,
}

impl RegionEmbedding {
    fn from_mfcc(embedding: SpeakerEmbedding) -> Self {
        let mut values = Vec::with_capacity(embedding.mfcc_mean.len() + embedding.mfcc_std.len());
        values.extend_from_slice(&embedding.mfcc_mean);
        values.extend_from_slice(&embedding.mfcc_std);
        Self {
            values,
            mfcc: Some(embedding),
        }
        .normalized()
    }

    fn from_raw(values: Vec<f32>) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        Some(
            Self {
                values,
                mfcc: None,
            }
            .normalized(),
        )
    }

    fn normalized(mut self) -> Self {
        let norm = self
            .values
            .iter()
            .map(|value| (*value as f64) * (*value as f64))
            .sum::<f64>()
            .sqrt();
        if norm > 0.0 {
            for value in &mut self.values {
                *value /= norm as f32;
            }
        }
        self
    }

    fn cosine_similarity(&self, other: &Self) -> f32 {
        if let (Some(left), Some(right)) = (&self.mfcc, &other.mfcc) {
            return left.similarity(right);
        }

        let len = self.values.len().min(other.values.len());
        if len == 0 {
            return -1.0;
        }

        let mut dot = 0.0f32;
        let mut left_norm = 0.0f32;
        let mut right_norm = 0.0f32;
        for index in 0..len {
            let left = self.values[index];
            let right = other.values[index];
            dot += left * right;
            left_norm += left * left;
            right_norm += right * right;
        }

        let denom = (left_norm.sqrt() * right_norm.sqrt()).max(1e-6);
        dot / denom
    }
}

pub fn discover_asset_version() -> Result<String, String> {
    Ok(AmbientDiarizationAssetSet::discover()?
        .map(|assets| assets.manifest.asset_version)
        .unwrap_or_else(|| BUILTIN_ASSET_VERSION.to_string()))
}

pub fn run_native_final_pass(
    request: NativeFinalPassRequest<'_>,
) -> Result<NativeFinalPassResult, String> {
    if request.transcript_segments.is_empty() {
        return Err("Native ambient final pass received no transcript segments.".to_string());
    }
    if request.samples.is_empty() {
        return Err("Native ambient final pass received no audio samples.".to_string());
    }

    let total_t0 = Instant::now();
    let mut asset_version = BUILTIN_ASSET_VERSION.to_string();
    let mut warning = None;
    let mut runtime_backend = "builtin_mfcc_v1".to_string();
    let mut clustering_similarity_threshold = DEFAULT_CLUSTERING_SIMILARITY_THRESHOLD;

    let maybe_assets = match AmbientDiarizationAssetSet::discover() {
        Ok(Some(asset_set)) => {
            asset_version = asset_set.manifest.asset_version.clone();
            if let Some(pipeline) = &asset_set.manifest.pipeline {
                clustering_similarity_threshold = pipeline.clustering_similarity_threshold;
                Some(asset_set)
            } else {
                warning = Some(
                    "Ambient diarization assets are installed without pipeline metadata; using built-in native diarization."
                        .to_string(),
                );
                None
            }
        }
        Ok(None) => {
            warning = Some(
                "Ambient diarization assets are not installed; using built-in native diarization."
                    .to_string(),
            );
            None
        }
        Err(err) => {
            warning = Some(format!(
                "Ambient diarization assets are unavailable; using built-in native diarization. {err}"
            ));
            None
        }
    };

    let (mut regions, segmentation_ms, embedding_ms, engine) = match maybe_assets.as_ref() {
        Some(asset_set) => match build_model_regions(&request, asset_set) {
            Ok(model) => {
                runtime_backend = model.runtime_backend;
                (
                    model.regions,
                    model.segmentation_ms,
                    model.embedding_ms,
                    "native_diarization_ort_coreml_v1".to_string(),
                )
            }
            Err(err) => {
                warning = Some(format!(
                    "Ambient diarization model runtime failed; using built-in native diarization. {err}"
                ));
                let (regions, segmentation_ms, embedding_ms) = build_builtin_regions(&request);
                (
                    regions,
                    segmentation_ms,
                    embedding_ms,
                    "native_diarization_builtin_v1".to_string(),
                )
            }
        },
        None => {
            let (regions, segmentation_ms, embedding_ms) = build_builtin_regions(&request);
            (
                regions,
                segmentation_ms,
                embedding_ms,
                "native_diarization_builtin_v1".to_string(),
            )
        }
    };

    let clustering_t0 = Instant::now();
    assign_clusters(&mut regions, clustering_similarity_threshold);
    let clustering_ms = clustering_t0.elapsed().as_millis() as u64;

    let assignment_t0 = Instant::now();
    let labeled_segments = assign_segment_labels(request.transcript_segments, &regions);
    let output_segments = build_canonical_segments(request.transcript_segments, &labeled_segments);
    let assignment_ms = assignment_t0.elapsed().as_millis() as u64;

    let detected_speakers = if output_segments.is_empty() {
        1
    } else {
        unique_speaker_count(&output_segments)
    };

    let total_ms = total_t0.elapsed().as_millis() as u64;
    let audio_duration_ms =
        ((request.samples.len() as u128 * 1_000) / request.sample_rate_hz.max(1) as u128) as f64;

    Ok(NativeFinalPassResult {
        engine,
        transcript_text: plain_transcript(request.transcript_segments, request.transcript_text),
        segments: output_segments,
        diagnostics: NativeFinalPassDiagnostics {
            asset_version: Some(asset_version),
            runtime_backend: Some(runtime_backend),
            warning,
            detected_speakers,
            diarization_ms: segmentation_ms + embedding_ms + clustering_ms,
            assignment_ms,
            segmentation_ms,
            embedding_ms,
            clustering_ms,
            total_ms,
            real_time_factor: if audio_duration_ms > 0.0 {
                total_ms as f64 / audio_duration_ms
            } else {
                0.0
            },
            ..NativeFinalPassDiagnostics::default()
        },
    })
}

fn build_builtin_regions(request: &NativeFinalPassRequest<'_>) -> (Vec<CandidateRegion>, u64, u64) {
    let segmentation_t0 = Instant::now();
    let mut regions = build_candidate_regions(request.transcript_segments);
    smooth_regions(&mut regions);
    let segmentation_ms = segmentation_t0.elapsed().as_millis() as u64;

    let embedding_t0 = Instant::now();
    for region in &mut regions {
        region.embedding = extract_builtin_region_embedding(
            request.samples,
            request.sample_rate_hz,
            region.start_ms,
            region.end_ms,
        );
    }
    let embedding_ms = embedding_t0.elapsed().as_millis() as u64;

    (regions, segmentation_ms, embedding_ms)
}

fn build_candidate_regions(transcript_segments: &[TranscriptSegment]) -> Vec<CandidateRegion> {
    let mut regions: Vec<CandidateRegion> = Vec::new();

    for (index, segment) in transcript_segments.iter().enumerate() {
        let text = segment.text.trim();
        if text.is_empty() {
            continue;
        }

        regions.push(CandidateRegion {
            start_ms: segment.start_ms,
            end_ms: segment.end_ms.max(segment.start_ms + 1),
            segment_indices: vec![index],
            embedding: None,
            cluster_id: None,
        });
    }

    regions
}

fn smooth_regions(regions: &mut Vec<CandidateRegion>) {
    let mut index = 0usize;
    while index < regions.len() {
        let duration_ms = regions[index].end_ms.saturating_sub(regions[index].start_ms);
        if duration_ms >= SHORT_REGION_MS || regions.len() <= 1 {
            index += 1;
            continue;
        }

        let prev_gap = index
            .checked_sub(1)
            .and_then(|prev| regions.get(prev).map(|region| regions[index].start_ms.saturating_sub(region.end_ms)));
        let next_gap = regions
            .get(index + 1)
            .map(|next| next.start_ms.saturating_sub(regions[index].end_ms));

        let attach_prev = prev_gap
            .filter(|gap| *gap <= SHORT_REGION_ATTACH_GAP_MS)
            .zip(index.checked_sub(1));
        let attach_next = next_gap
            .filter(|gap| *gap <= SHORT_REGION_ATTACH_GAP_MS)
            .map(|gap| (gap, index + 1));

        match (attach_prev, attach_next) {
            (Some((prev_gap, prev)), Some((next_gap, _next))) if prev_gap <= next_gap => {
                merge_region_into_previous(regions, prev, index);
            }
            (Some((_gap, prev)), _) => {
                merge_region_into_previous(regions, prev, index);
            }
            (_, Some((_gap, next))) => {
                merge_region_into_next(regions, index, next);
            }
            _ => {
                index += 1;
            }
        }
    }
}

fn merge_region_into_previous(regions: &mut Vec<CandidateRegion>, prev: usize, current: usize) {
    let current_region = regions.remove(current);
    if let Some(previous) = regions.get_mut(prev) {
        previous.end_ms = previous.end_ms.max(current_region.end_ms);
        previous.segment_indices.extend(current_region.segment_indices);
    }
}

fn merge_region_into_next(regions: &mut Vec<CandidateRegion>, current: usize, next: usize) {
    let current_region = regions.remove(current);
    if let Some(next_region) = regions.get_mut(next - 1) {
        next_region.start_ms = next_region.start_ms.min(current_region.start_ms);
        let mut segment_indices = current_region.segment_indices;
        segment_indices.extend(next_region.segment_indices.iter().copied());
        next_region.segment_indices = segment_indices;
    }
}

fn extract_builtin_region_embedding(
    samples: &[f32],
    sample_rate_hz: usize,
    start_ms: u64,
    end_ms: u64,
) -> Option<RegionEmbedding> {
    let start_idx = ((start_ms as usize * sample_rate_hz) / 1_000).min(samples.len());
    let end_idx = ((end_ms as usize * sample_rate_hz) / 1_000)
        .max(start_idx + 1)
        .min(samples.len());
    SpeakerEmbedding::from_samples(&samples[start_idx..end_idx]).map(RegionEmbedding::from_mfcc)
}

fn assign_clusters(regions: &mut [CandidateRegion], similarity_threshold: f32) {
    let mut clusters = regions
        .iter()
        .enumerate()
        .filter_map(|(index, region)| {
            region.embedding.as_ref().map(|embedding| Cluster {
                region_indices: vec![index],
                centroid: embedding.clone(),
            })
        })
        .collect::<Vec<_>>();

    loop {
        let mut best_pair = None;
        let mut best_similarity = similarity_threshold;

        for left in 0..clusters.len() {
            for right in (left + 1)..clusters.len() {
                let similarity = clusters[left].centroid.cosine_similarity(&clusters[right].centroid);
                if similarity > best_similarity {
                    best_similarity = similarity;
                    best_pair = Some((left, right));
                }
            }
        }

        let Some((left, right)) = best_pair else {
            break;
        };
        let right_cluster = clusters.remove(right);
        clusters[left].region_indices.extend(right_cluster.region_indices);
        clusters[left].centroid = average_centroid(&clusters[left], regions);
    }

    let mut clusters_with_start = clusters
        .into_iter()
        .map(|cluster| {
            let first_start = cluster
                .region_indices
                .iter()
                .filter_map(|index| regions.get(*index))
                .map(|region| region.start_ms)
                .min()
                .unwrap_or(u64::MAX);
            (first_start, cluster)
        })
        .collect::<Vec<_>>();
    clusters_with_start.sort_by_key(|(first_start, _)| *first_start);

    for (cluster_id, (_first_start, cluster)) in clusters_with_start.into_iter().enumerate() {
        for region_index in cluster.region_indices {
            if let Some(region) = regions.get_mut(region_index) {
                region.cluster_id = Some(cluster_id);
            }
        }
    }

    assign_missing_cluster_ids(regions);
}

fn average_centroid(cluster: &Cluster, regions: &[CandidateRegion]) -> RegionEmbedding {
    let mut total_weight = 0.0f32;
    let mut centroid = Vec::<f32>::new();
    let mut mfcc_mean = [0.0f32; 13];
    let mut mfcc_std = [0.0f32; 13];
    let mut all_mfcc = true;

    for index in &cluster.region_indices {
        let Some(region) = regions.get(*index) else {
            continue;
        };
        let Some(embedding) = region.embedding.as_ref() else {
            continue;
        };
        if centroid.is_empty() {
            centroid.resize(embedding.values.len(), 0.0);
        }

        let duration_weight = region.end_ms.saturating_sub(region.start_ms).max(1) as f32;
        total_weight += duration_weight;
        for (slot, value) in centroid.iter_mut().zip(embedding.values.iter()) {
            *slot += *value * duration_weight;
        }
        if let Some(mfcc) = &embedding.mfcc {
            for coeff in 0..mfcc_mean.len() {
                mfcc_mean[coeff] += mfcc.mfcc_mean[coeff] * duration_weight;
                mfcc_std[coeff] += mfcc.mfcc_std[coeff] * duration_weight;
            }
        } else {
            all_mfcc = false;
        }
    }

    let denom = total_weight.max(1.0);
    for value in &mut centroid {
        *value /= denom;
    }

    if all_mfcc && total_weight > 0.0 {
        for coeff in 0..mfcc_mean.len() {
            mfcc_mean[coeff] /= denom;
            mfcc_std[coeff] /= denom;
        }
        return RegionEmbedding::from_mfcc(SpeakerEmbedding {
            mfcc_mean,
            mfcc_std,
        });
    }

    RegionEmbedding::from_raw(centroid).unwrap_or_else(|| RegionEmbedding {
        values: vec![0.0],
        mfcc: None,
    })
}

fn assign_missing_cluster_ids(regions: &mut [CandidateRegion]) {
    let labeled_regions = regions
        .iter()
        .enumerate()
        .filter_map(|(index, region)| region.cluster_id.map(|cluster_id| (index, cluster_id)))
        .collect::<Vec<_>>();

    if labeled_regions.is_empty() {
        for region in regions {
            region.cluster_id = Some(0);
        }
        return;
    }

    for index in 0..regions.len() {
        if regions[index].cluster_id.is_some() {
            continue;
        }

        let midpoint = (regions[index].start_ms + regions[index].end_ms) / 2;
        let nearest = labeled_regions
            .iter()
            .min_by_key(|(candidate_index, _cluster_id)| {
                let candidate = &regions[*candidate_index];
                let candidate_midpoint = (candidate.start_ms + candidate.end_ms) / 2;
                midpoint.abs_diff(candidate_midpoint)
            })
            .map(|(_, cluster_id)| *cluster_id)
            .unwrap_or(0);

        regions[index].cluster_id = Some(nearest);
    }
}

fn assign_segment_labels(
    transcript_segments: &[TranscriptSegment],
    regions: &[CandidateRegion],
) -> Vec<SpeakerLabel> {
    let cluster_to_label = build_cluster_label_map(regions);
    let mut labels = Vec::with_capacity(transcript_segments.len());

    for segment in transcript_segments {
        let best_region = regions
            .iter()
            .filter_map(|region| {
                let cluster_id = region.cluster_id?;
                let overlap = overlap_ms(segment.start_ms, segment.end_ms, region.start_ms, region.end_ms);
                Some((region, cluster_id, overlap))
            })
            .max_by_key(|(region, _cluster_id, overlap)| (*overlap, std::cmp::Reverse(region.start_ms)));

        if let Some((_region, cluster_id, overlap)) = best_region {
            if overlap > 0 {
                labels.push(
                    cluster_to_label
                        .get(&cluster_id)
                        .copied()
                        .unwrap_or(SpeakerLabel::S1),
                );
                continue;
            }
        }

        let segment_midpoint = (segment.start_ms + segment.end_ms) / 2;
        let nearest_cluster = regions
            .iter()
            .filter_map(|region| {
                region.cluster_id.map(|cluster_id| {
                    let region_midpoint = (region.start_ms + region.end_ms) / 2;
                    (cluster_id, segment_midpoint.abs_diff(region_midpoint))
                })
            })
            .min_by_key(|(_, distance)| *distance)
            .map(|(cluster_id, _)| cluster_id)
            .unwrap_or(0);

        labels.push(
            cluster_to_label
                .get(&nearest_cluster)
                .copied()
                .unwrap_or(SpeakerLabel::S1),
        );
    }

    labels
}

fn build_cluster_label_map(regions: &[CandidateRegion]) -> BTreeMap<usize, SpeakerLabel> {
    let mut cluster_starts = BTreeMap::<usize, u64>::new();
    for region in regions {
        if let Some(cluster_id) = region.cluster_id {
            cluster_starts
                .entry(cluster_id)
                .and_modify(|start| *start = (*start).min(region.start_ms))
                .or_insert(region.start_ms);
        }
    }

    let mut ordered = cluster_starts.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(_, start_ms)| *start_ms);

    ordered
        .into_iter()
        .enumerate()
        .map(|(index, (cluster_id, _))| (cluster_id, speaker_label_for_index(index)))
        .collect()
}

fn build_canonical_segments(
    transcript_segments: &[TranscriptSegment],
    labels: &[SpeakerLabel],
) -> Vec<CanonicalSegment> {
    let mut segments = Vec::new();
    let mut next_segment_id = 1u64;

    for (segment, speaker) in transcript_segments.iter().zip(labels.iter().copied()) {
        let candidate = CanonicalSegment {
            id: next_segment_id,
            lane: segment.lane,
            speaker,
            start_ms: segment.start_ms,
            end_ms: segment.end_ms.max(segment.start_ms + 1),
            text: segment.text.trim().to_string(),
        };

        let len_before = segments.len();
        if merge_segment(&mut segments, candidate, segment.speaker_turn_next).is_some()
            && segments.len() > len_before
        {
            next_segment_id += 1;
        }
    }

    segments
}

fn overlap_ms(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> u64 {
    a_end.min(b_end).saturating_sub(a_start.max(b_start))
}

fn speaker_label_for_index(index: usize) -> SpeakerLabel {
    match index {
        0 => SpeakerLabel::S1,
        1 => SpeakerLabel::S2,
        2 => SpeakerLabel::S3,
        3 => SpeakerLabel::S4,
        4 => SpeakerLabel::S5,
        _ => SpeakerLabel::S6,
    }
}

fn unique_speaker_count(segments: &[CanonicalSegment]) -> usize {
    let mut speakers = segments.iter().map(|segment| segment.speaker).collect::<Vec<_>>();
    speakers.sort_by_key(|speaker| speaker.index());
    speakers.dedup();
    speakers.len()
}

fn plain_transcript(transcript_segments: &[TranscriptSegment], transcript_text: &str) -> String {
    let provided = transcript_text.trim();
    if !provided.is_empty() {
        return provided.to_string();
    }

    transcript_segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg_attr(not(feature = "ort-coreml"), allow(dead_code))]
fn snap_regions_to_speech(regions: &mut [CandidateRegion], speech_regions: &[SpeechRegion]) {
    if speech_regions.is_empty() {
        return;
    }

    for region in regions {
        if let Some(best_overlap_region) = speech_regions
            .iter()
            .filter_map(|speech| {
                let overlap = overlap_ms(region.start_ms, region.end_ms, speech.start_ms, speech.end_ms);
                (overlap > 0).then_some((overlap, speech))
            })
            .max_by_key(|(overlap, speech)| (*overlap, std::cmp::Reverse(speech.start_ms)))
            .map(|(_, speech)| speech)
        {
            region.start_ms = region
                .start_ms
                .max(best_overlap_region.start_ms.saturating_sub(DEFAULT_SPEECH_SNAP_COLLAR_MS));
            region.end_ms = region
                .end_ms
                .min(best_overlap_region.end_ms.saturating_add(DEFAULT_SPEECH_SNAP_COLLAR_MS));
        } else if let Some(nearest_speech) = speech_regions
            .iter()
            .min_by_key(|speech| {
                let midpoint = (region.start_ms + region.end_ms) / 2;
                let speech_midpoint = (speech.start_ms + speech.end_ms) / 2;
                midpoint.abs_diff(speech_midpoint)
            })
        {
            let midpoint = (region.start_ms + region.end_ms) / 2;
            let speech_midpoint = (nearest_speech.start_ms + nearest_speech.end_ms) / 2;
            if midpoint.abs_diff(speech_midpoint) <= DEFAULT_NEAREST_SPEECH_ATTACH_MS {
                region.start_ms = region.start_ms.min(nearest_speech.start_ms);
                region.end_ms = region.end_ms.max(nearest_speech.end_ms);
            }
        }

        if region.end_ms <= region.start_ms {
            region.end_ms = region.start_ms + 1;
        }
    }
}

#[cfg_attr(not(feature = "ort-coreml"), allow(dead_code))]
fn build_speech_regions_from_scores(
    scores: &[f32],
    frame_hop_ms: u64,
    threshold: f32,
    min_speech_ms: u64,
    min_silence_ms: u64,
) -> Vec<SpeechRegion> {
    if scores.is_empty() || frame_hop_ms == 0 {
        return Vec::new();
    }

    let mut active = scores.iter().map(|score| *score >= threshold).collect::<Vec<_>>();
    fill_short_silence_gaps(
        &mut active,
        ((min_silence_ms + frame_hop_ms.saturating_sub(1)) / frame_hop_ms) as usize,
    );

    let mut regions = Vec::new();
    let mut current_start = None;
    for (frame_index, is_active) in active.iter().copied().enumerate() {
        match (current_start, is_active) {
            (None, true) => current_start = Some(frame_index),
            (Some(start_frame), false) => {
                regions.push(SpeechRegion {
                    start_ms: start_frame as u64 * frame_hop_ms,
                    end_ms: frame_index as u64 * frame_hop_ms,
                });
                current_start = None;
            }
            _ => {}
        }
    }

    if let Some(start_frame) = current_start {
        regions.push(SpeechRegion {
            start_ms: start_frame as u64 * frame_hop_ms,
            end_ms: active.len() as u64 * frame_hop_ms,
        });
    }

    regions.retain(|region| region.end_ms.saturating_sub(region.start_ms) >= min_speech_ms);
    regions
}

#[cfg_attr(not(feature = "ort-coreml"), allow(dead_code))]
fn fill_short_silence_gaps(active: &mut [bool], max_gap_frames: usize) {
    if max_gap_frames == 0 || active.len() < 3 {
        return;
    }

    let mut index = 0usize;
    while index < active.len() {
        if active[index] {
            index += 1;
            continue;
        }

        let gap_start = index;
        while index < active.len() && !active[index] {
            index += 1;
        }
        let gap_end = index;
        let gap_len = gap_end.saturating_sub(gap_start);
        if gap_len == 0 || gap_len > max_gap_frames {
            continue;
        }

        let has_active_before = gap_start > 0 && active[gap_start - 1];
        let has_active_after = gap_end < active.len() && active[gap_end];
        if has_active_before && has_active_after {
            for flag in &mut active[gap_start..gap_end] {
                *flag = true;
            }
        }
    }
}

#[cfg_attr(not(feature = "ort-coreml"), allow(dead_code))]
fn sliding_window_starts(audio_duration_ms: u64, window_ms: u64, hop_ms: u64) -> Vec<u64> {
    if audio_duration_ms <= window_ms || hop_ms == 0 {
        return vec![0];
    }

    let last_start = audio_duration_ms.saturating_sub(window_ms);
    let mut starts = Vec::new();
    let mut start = 0u64;
    while start < last_start {
        starts.push(start);
        start = start.saturating_add(hop_ms);
    }
    if starts.last().copied() != Some(last_start) {
        starts.push(last_start);
    }
    starts
}

#[cfg(feature = "ort-coreml")]
fn build_model_regions(
    request: &NativeFinalPassRequest<'_>,
    asset_set: &AmbientDiarizationAssetSet,
) -> Result<ModelPipelineResult, String> {
    use ort::ep::{self, CoreML};
    use ort::session::Session;
    use ort::value::Tensor;

    struct ModelRuntime {
        segmentation: Session,
        embedding: Session,
    }

    impl ModelRuntime {
        fn load(
            asset_set: &AmbientDiarizationAssetSet,
            pipeline: &AmbientDiarizationPipelineManifest,
        ) -> Result<Self, String> {
            Ok(Self {
                segmentation: load_onnx_session(
                    asset_set,
                    &pipeline.segmentation,
                    "segmentation-coreml-cache",
                )?,
                embedding: load_onnx_session(
                    asset_set,
                    &pipeline.embedding,
                    "embedding-coreml-cache",
                )?,
            })
        }
    }

    fn load_onnx_session(
        asset_set: &AmbientDiarizationAssetSet,
        model: &AmbientDiarizationModelSpec,
        fallback_cache_name: &str,
    ) -> Result<Session, String> {
        let model_path = asset_set.resolve_relative_path(&model.relative_path);
        let cache_dir = asset_set.model_cache_dir(model.model_cache_subdir.as_deref(), fallback_cache_name);
        std::fs::create_dir_all(&cache_dir).map_err(|err| {
            format!(
                "Failed to create ambient diarization CoreML cache directory {}: {err}",
                cache_dir.display()
            )
        })?;

        let execution_provider = CoreML::default()
            .with_compute_units(ep::coreml::ComputeUnits::CPUAndGPU)
            .with_specialization_strategy(ep::coreml::SpecializationStrategy::FastPrediction)
            .with_model_cache_dir(cache_dir.display().to_string())
            .build();

        let mut builder = Session::builder()
            .map_err(|err| format!("Failed to create ONNX Runtime session builder: {err}"))?;
        builder = builder
            .with_execution_providers([execution_provider])
            .map_err(|err| format!("Failed to configure CoreML execution provider: {err}"))?;
        builder
            .commit_from_file(&model_path)
            .map_err(|err| format!("Failed to load ambient diarization model {}: {err}", model_path.display()))
    }

    fn prepare_window_samples(
        samples: &[f32],
        sample_rate_hz: usize,
        start_ms: u64,
        length_samples: usize,
    ) -> Vec<f32> {
        let start_idx = ((start_ms as usize * sample_rate_hz) / 1_000).min(samples.len());
        let end_idx = (start_idx + length_samples).min(samples.len());
        let mut window = samples[start_idx..end_idx].to_vec();
        if window.len() < length_samples {
            window.resize(length_samples, 0.0);
        }
        window
    }

    fn prepare_embedding_samples(
        samples: &[f32],
        sample_rate_hz: usize,
        start_ms: u64,
        end_ms: u64,
        target_samples: Option<usize>,
    ) -> Vec<f32> {
        let start_idx = ((start_ms as usize * sample_rate_hz) / 1_000).min(samples.len());
        let end_idx = ((end_ms as usize * sample_rate_hz) / 1_000)
            .max(start_idx + 1)
            .min(samples.len());
        let mut clip = samples[start_idx..end_idx].to_vec();

        if let Some(target_samples) = target_samples {
            if clip.len() > target_samples {
                let offset = (clip.len() - target_samples) / 2;
                clip = clip[offset..offset + target_samples].to_vec();
            } else if clip.len() < target_samples {
                let mut padded = vec![0.0f32; target_samples];
                let offset = (target_samples - clip.len()) / 2;
                padded[offset..offset + clip.len()].copy_from_slice(&clip);
                clip = padded;
            }
        }

        clip
    }

    fn build_audio_input(
        model: &AmbientDiarizationModelSpec,
        samples: Vec<f32>,
    ) -> Result<Tensor<f32>, String> {
        match model.input_layout {
            AmbientModelInputLayout::BatchSamples => Tensor::from_array((
                [1usize, samples.len()],
                samples.into_boxed_slice(),
            ))
            .map_err(|err| format!("Failed to build ONNX input tensor: {err}")),
            AmbientModelInputLayout::BatchChannelSamples => Tensor::from_array((
                [1usize, 1usize, samples.len()],
                samples.into_boxed_slice(),
            ))
            .map_err(|err| format!("Failed to build ONNX input tensor: {err}")),
        }
    }

    fn run_audio_model(
        session: &mut Session,
        model: &AmbientDiarizationModelSpec,
        input_samples: Vec<f32>,
    ) -> Result<(Vec<usize>, Vec<f32>), String> {
        let input_name = model
            .input_name
            .as_deref()
            .or_else(|| session.inputs().first().map(|input| input.name()))
            .ok_or_else(|| "Model exposes no inputs.".to_string())?
            .to_string();
        let input_tensor = build_audio_input(model, input_samples)?;
        let outputs = session
            .run(ort::inputs! { input_name => input_tensor })
            .map_err(|err| format!("ONNX Runtime inference failed: {err}"))?;
        let output = if let Some(name) = model.output_name.as_deref() {
            outputs
                .get(name)
                .ok_or_else(|| format!("Model output `{name}` was not present in ONNX Runtime outputs."))?
                .view()
        } else {
            outputs
                .iter()
                .next()
                .map(|(_name, value)| value)
                .ok_or_else(|| "Model produced no ONNX Runtime outputs.".to_string())?
        };
        let (shape, data) = output
            .try_extract_tensor::<f32>()
            .map_err(|err| format!("Failed to extract ONNX tensor output: {err}"))?;
        let dims = shape.iter().map(|dim| *dim as usize).collect::<Vec<_>>();
        Ok((dims, data.to_vec()))
    }

    fn decode_segmentation_output(
        model: &AmbientDiarizationModelSpec,
        shape: &[usize],
        data: &[f32],
    ) -> Result<(usize, usize, Vec<f32>), String> {
        match model.output_layout {
            AmbientModelOutputLayout::FramesSpeakers => {
                if shape.len() != 2 {
                    return Err(format!(
                        "Segmentation output expected rank 2 but received shape {:?}.",
                        shape
                    ));
                }
                let frames = shape[0];
                let speakers = shape[1];
                Ok((frames, speakers, data.to_vec()))
            }
            AmbientModelOutputLayout::BatchFramesSpeakers => {
                if shape.len() == 2 {
                    return Ok((shape[0], shape[1], data.to_vec()));
                }
                if shape.len() != 3 || shape[0] != 1 {
                    return Err(format!(
                        "Segmentation output expected shape [1, frames, speakers] but received {:?}.",
                        shape
                    ));
                }
                Ok((shape[1], shape[2], data.to_vec()))
            }
            AmbientModelOutputLayout::BatchSpeakersFrames => {
                if shape.len() != 3 || shape[0] != 1 {
                    return Err(format!(
                        "Segmentation output expected shape [1, speakers, frames] but received {:?}.",
                        shape
                    ));
                }
                let speakers = shape[1];
                let frames = shape[2];
                let mut reordered = vec![0.0f32; frames * speakers];
                for speaker in 0..speakers {
                    for frame in 0..frames {
                        reordered[frame * speakers + speaker] = data[speaker * frames + frame];
                    }
                }
                Ok((frames, speakers, reordered))
            }
            _ => Err("Segmentation model output layout must be a speaker-activity tensor.".to_string()),
        }
    }

    fn infer_speech_regions(
        runtime: &mut ModelRuntime,
        request: &NativeFinalPassRequest<'_>,
        segmentation: &AmbientDiarizationModelSpec,
    ) -> Result<Vec<SpeechRegion>, String> {
        if request.sample_rate_hz != segmentation.sample_rate_hz {
            return Err(format!(
                "Segmentation model requires {} Hz audio but received {} Hz.",
                segmentation.sample_rate_hz, request.sample_rate_hz
            ));
        }

        let audio_duration_ms =
            ((request.samples.len() as u128 * 1_000) / request.sample_rate_hz.max(1) as u128) as u64;
        let window_starts = sliding_window_starts(audio_duration_ms, segmentation.window_ms, segmentation.hop_ms);
        let global_frame_hop_ms = segmentation.frame_hop_ms.max(1);
        let global_frame_count = ((audio_duration_ms + global_frame_hop_ms - 1) / global_frame_hop_ms)
            .max(1) as usize;
        let mut score_sum = vec![0.0f32; global_frame_count];
        let mut score_count = vec![0u32; global_frame_count];

        let window_samples =
            ((segmentation.window_ms as usize * segmentation.sample_rate_hz) / 1_000).max(1);
        for start_ms in window_starts {
            let input = prepare_window_samples(
                request.samples,
                request.sample_rate_hz,
                start_ms,
                window_samples,
            );
            let (shape, data) = run_audio_model(&mut runtime.segmentation, segmentation, input)?;
            let (frame_count, speaker_count, scores) =
                decode_segmentation_output(segmentation, &shape, &data)?;
            if speaker_count == 0 {
                return Err("Segmentation model returned zero speaker channels.".to_string());
            }

            for frame_index in 0..frame_count {
                let frame_scores =
                    &scores[frame_index * speaker_count..(frame_index + 1) * speaker_count];
                let speech_score = frame_scores
                    .iter()
                    .copied()
                    .fold(f32::MIN, f32::max)
                    .max(0.0);
                let global_frame = ((start_ms / global_frame_hop_ms) as usize).saturating_add(frame_index);
                if global_frame >= global_frame_count {
                    break;
                }
                score_sum[global_frame] += speech_score;
                score_count[global_frame] = score_count[global_frame].saturating_add(1);
            }
        }

        let averaged_scores = score_sum
            .into_iter()
            .zip(score_count.into_iter())
            .map(|(sum, count)| {
                if count == 0 {
                    0.0
                } else {
                    sum / count as f32
                }
            })
            .collect::<Vec<_>>();

        Ok(build_speech_regions_from_scores(
            &averaged_scores,
            global_frame_hop_ms,
            segmentation.activation_threshold,
            segmentation.min_speech_ms,
            segmentation.min_silence_ms,
        ))
    }

    fn infer_region_embedding(
        runtime: &mut ModelRuntime,
        request: &NativeFinalPassRequest<'_>,
        embedding_model: &AmbientDiarizationModelSpec,
        region: &CandidateRegion,
    ) -> Result<Option<RegionEmbedding>, String> {
        if request.sample_rate_hz != embedding_model.sample_rate_hz {
            return Err(format!(
                "Embedding model requires {} Hz audio but received {} Hz.",
                embedding_model.sample_rate_hz, request.sample_rate_hz
            ));
        }

        let clip = prepare_embedding_samples(
            request.samples,
            request.sample_rate_hz,
            region.start_ms,
            region.end_ms,
            embedding_model.target_samples,
        );
        if clip.is_empty() {
            return Ok(None);
        }

        let (_shape, values) = run_audio_model(&mut runtime.embedding, embedding_model, clip)?;
        Ok(RegionEmbedding::from_raw(values))
    }

    let pipeline = asset_set
        .manifest
        .pipeline
        .as_ref()
        .ok_or_else(|| "Ambient diarization pipeline metadata is missing.".to_string())?;
    let mut runtime = ModelRuntime::load(asset_set, pipeline)?;

    let segmentation_t0 = Instant::now();
    let speech_regions = infer_speech_regions(&mut runtime, request, &pipeline.segmentation)?;
    let mut regions = build_candidate_regions(request.transcript_segments);
    snap_regions_to_speech(&mut regions, &speech_regions);
    smooth_regions(&mut regions);
    let segmentation_ms = segmentation_t0.elapsed().as_millis() as u64;

    let embedding_t0 = Instant::now();
    for region in &mut regions {
        region.embedding =
            infer_region_embedding(&mut runtime, request, &pipeline.embedding, region)?;
    }
    let embedding_ms = embedding_t0.elapsed().as_millis() as u64;

    Ok(ModelPipelineResult {
        regions,
        segmentation_ms,
        embedding_ms,
        runtime_backend: "onnx_runtime_coreml_v1".to_string(),
    })
}

#[cfg(not(feature = "ort-coreml"))]
fn build_model_regions(
    _request: &NativeFinalPassRequest<'_>,
    _asset_set: &AmbientDiarizationAssetSet,
) -> Result<ModelPipelineResult, String> {
    Err(
        "This Screamer build does not include the ONNX Runtime CoreML backend (`ort-coreml`)."
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use screamer_core::ambient::AudioLane;
    use std::f32::consts::PI;

    const SR: usize = 16_000;

    fn sine_wave(freq: f32, duration_secs: f32) -> Vec<f32> {
        let sample_count = (SR as f32 * duration_secs) as usize;
        (0..sample_count)
            .map(|index| (2.0 * PI * freq * index as f32 / SR as f32).sin() * 0.4)
            .collect()
    }

    fn segment(start_ms: u64, end_ms: u64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            lane: AudioLane::Microphone,
            start_ms,
            end_ms,
            speaker_turn_next: false,
            text: text.to_string(),
        }
    }

    #[test]
    fn native_final_pass_assigns_consistent_labels_for_repeated_voice() {
        let voice_a = sine_wave(220.0, 0.8);
        let voice_b = sine_wave(1_800.0, 0.8);
        let mut samples = voice_a.clone();
        samples.extend_from_slice(&voice_b);
        samples.extend_from_slice(&voice_a);

        let segments = vec![
            segment(0, 800, "hello there"),
            segment(800, 1_600, "good morning"),
            segment(1_600, 2_400, "welcome back"),
        ];

        let result = run_native_final_pass(NativeFinalPassRequest {
            sample_rate_hz: SR,
            samples: &samples,
            transcript_segments: &segments,
            transcript_text: "",
        })
        .unwrap();

        assert_eq!(result.segments.len(), 3);
        assert_eq!(result.segments[0].speaker, result.segments[2].speaker);
        assert_ne!(result.segments[0].speaker, result.segments[1].speaker);
    }

    #[test]
    fn native_final_pass_uses_nearest_cluster_when_overlap_is_missing() {
        let voice_a = sine_wave(250.0, 1.2);
        let segments = vec![
            segment(0, 400, "one"),
            segment(450, 700, "two"),
            segment(900, 1_150, "three"),
        ];

        let result = run_native_final_pass(NativeFinalPassRequest {
            sample_rate_hz: SR,
            samples: &voice_a,
            transcript_segments: &segments,
            transcript_text: "",
        })
        .unwrap();

        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].speaker, SpeakerLabel::S1);
        assert!(result
            .diagnostics
            .warning
            .as_deref()
            .unwrap_or_default()
            .contains("built-in native diarization"));
    }

    #[test]
    fn smoothing_absorbs_short_regions_between_neighbors() {
        let mut regions = vec![
            CandidateRegion {
                start_ms: 0,
                end_ms: 600,
                segment_indices: vec![0],
                embedding: None,
                cluster_id: None,
            },
            CandidateRegion {
                start_ms: 620,
                end_ms: 760,
                segment_indices: vec![1],
                embedding: None,
                cluster_id: None,
            },
            CandidateRegion {
                start_ms: 770,
                end_ms: 1_300,
                segment_indices: vec![2],
                embedding: None,
                cluster_id: None,
            },
        ];

        smooth_regions(&mut regions);

        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].segment_indices, vec![0, 1]);
    }

    #[test]
    fn short_silence_gaps_are_filled_in_speech_regions() {
        let scores = vec![0.8, 0.9, 0.1, 0.85, 0.9];
        let regions = build_speech_regions_from_scores(&scores, 20, 0.5, 40, 40);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].start_ms, 0);
        assert_eq!(regions[0].end_ms, 100);
    }

    #[test]
    fn speech_regions_snap_transcript_boundaries() {
        let mut regions = vec![CandidateRegion {
            start_ms: 100,
            end_ms: 420,
            segment_indices: vec![0],
            embedding: None,
            cluster_id: None,
        }];
        let speech = vec![SpeechRegion {
            start_ms: 150,
            end_ms: 350,
        }];

        snap_regions_to_speech(&mut regions, &speech);

        assert_eq!(regions[0].start_ms, 100.max(150 - DEFAULT_SPEECH_SNAP_COLLAR_MS));
        assert_eq!(regions[0].end_ms, 420.min(350 + DEFAULT_SPEECH_SNAP_COLLAR_MS));
    }
}

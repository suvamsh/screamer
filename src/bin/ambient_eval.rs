#[path = "../ambient_final_pass.rs"]
mod ambient_final_pass;
#[path = "../ambient_pipeline.rs"]
mod ambient_pipeline;
#[path = "../ambient_whisperx.rs"]
mod ambient_whisperx;
#[path = "../diarization.rs"]
mod diarization;

use ambient_final_pass::{
    run_native_diarization_final_pass, run_whisperx_hybrid_final_pass, AmbientFinalPassResult,
};
use serde::{Deserialize, Serialize};
use screamer_core::ambient::{segments_to_transcript, AmbientSessionConfig, CanonicalSegment};
use screamer_core::audio::{resample_to_target, TARGET_SAMPLE_RATE};
use screamer_whisper::{MachineProfile, Transcriber};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Deserialize)]
struct EvalManifest {
    #[serde(default = "default_model")]
    model: String,
    cases: Vec<EvalCase>,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    #[serde(default)]
    required: bool,
    audio_path: PathBuf,
    #[serde(default)]
    reference_transcript_path: Option<PathBuf>,
    #[serde(default)]
    reference_turns_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReferenceTurn {
    start_ms: u64,
    end_ms: u64,
    speaker: String,
}

#[derive(Debug)]
struct EvalOptions {
    manifest_path: PathBuf,
    baseline_out: Option<PathBuf>,
    enable_legacy_python: bool,
}

#[derive(Debug, Serialize)]
struct EvalReport {
    model: String,
    runtime: RuntimeMetadata,
    cases: Vec<EvalCaseReport>,
}

#[derive(Debug, Serialize)]
struct RuntimeMetadata {
    machine_summary: String,
    chip: String,
    architecture: String,
}

#[derive(Debug, Serialize)]
struct EvalCaseReport {
    id: String,
    required: bool,
    audio_path: String,
    audio_duration_ms: u64,
    reference: EvalReferenceSummary,
    native_live_current: BackendReport,
    legacy_python_final: BackendReport,
    native_final_v1: BackendReport,
}

#[derive(Debug, Default, Serialize)]
struct EvalReferenceSummary {
    transcript_path: Option<String>,
    turns_path: Option<String>,
    reference_word_count: Option<usize>,
    reference_turn_count: Option<usize>,
    reference_speaker_count: Option<usize>,
}

#[derive(Debug, Serialize)]
struct BackendReport {
    status: String,
    backend: String,
    engine: Option<String>,
    transcript: Option<String>,
    segment_count: Option<usize>,
    turn_count: Option<usize>,
    speaker_count: Option<usize>,
    speaker_count_delta: Option<isize>,
    turn_count_delta: Option<isize>,
    word_error_rate: Option<f64>,
    turn_boundary_f1_500ms: Option<f64>,
    speaker_time_accuracy: Option<f64>,
    total_ms: Option<u64>,
    real_time_factor: Option<f64>,
    peak_rss_mb: Option<f64>,
    asset_version: Option<String>,
    error: Option<String>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = parse_options(std::env::args().skip(1))?;
    let manifest: EvalManifest = serde_json::from_str(
        &fs::read_to_string(&options.manifest_path).map_err(|err| {
            format!(
                "Failed to read manifest {}: {err}",
                options.manifest_path.display()
            )
        })?,
    )
    .map_err(|err| {
        format!(
            "Failed to parse manifest {}: {err}",
            options.manifest_path.display()
        )
    })?;

    let model_path = Transcriber::find_model(&manifest.model).ok_or_else(|| {
        format!(
            "Could not find Screamer whisper model `{}`. Run ./download_model.sh {} first.",
            manifest.model, manifest.model
        )
    })?;
    let transcriber = Transcriber::new(&model_path)?;
    let ambient_config = AmbientSessionConfig::default();
    let diarization_engine = diarization::default_diarization_engine();
    let machine = MachineProfile::detect();

    let mut reports = Vec::with_capacity(manifest.cases.len());
    for case in manifest.cases {
        let samples = read_audio_file(&case.audio_path)?;
        let audio_duration_ms =
            ((samples.len() as u128 * 1_000) / TARGET_SAMPLE_RATE as u128) as u64;

        let reference_transcript = case
            .reference_transcript_path
            .as_ref()
            .map(fs::read_to_string)
            .transpose()
            .map_err(|err| format!("Failed to read reference transcript for {}: {err}", case.id))?;
        let reference_turns = case
            .reference_turns_path
            .as_ref()
            .map(|path| load_reference_turns(path.as_path()))
            .transpose()?;

        let native_live_start = Instant::now();
        let native_live_segments = ambient_pipeline::run_native_ambient_pass(
            &ambient_config,
            &transcriber,
            &*diarization_engine,
            &samples,
        )?;
        let native_live_ms = native_live_start.elapsed().as_millis() as u64;

        let native_final_start = Instant::now();
        let native_final_result = run_native_diarization_final_pass(&samples, &transcriber);
        let native_final_ms = native_final_start.elapsed().as_millis() as u64;

        let legacy_python_result = if options.enable_legacy_python {
            let audio_wav = ambient_whisperx::write_temp_wav("ambient-eval", &samples)?;
            let result = run_whisperx_hybrid_final_pass(&audio_wav, &manifest.model);
            let _ = fs::remove_file(&audio_wav);
            result
        } else {
            Err("Legacy Python benchmark disabled. Pass --enable-legacy-python to run it.".to_string())
        };

        let reference_summary = EvalReferenceSummary {
            transcript_path: case
                .reference_transcript_path
                .as_ref()
                .map(|path| path.display().to_string()),
            turns_path: case
                .reference_turns_path
                .as_ref()
                .map(|path| path.display().to_string()),
            reference_word_count: reference_transcript
                .as_deref()
                .map(normalized_words)
                .map(|words| words.len()),
            reference_turn_count: reference_turns.as_ref().map(|turns| turns.len()),
            reference_speaker_count: reference_turns
                .as_ref()
                .map(|turns| unique_reference_speaker_count(turns)),
        };

        let native_live_report = build_segment_report(
            "native_live_current",
            "native_live_current",
            native_live_segments,
            reference_transcript.as_deref(),
            reference_turns.as_deref(),
            native_live_ms,
            audio_duration_ms,
            None,
        );
        let legacy_python_report = build_final_pass_report(
            "legacy_python_final",
            legacy_python_result,
            reference_transcript.as_deref(),
            reference_turns.as_deref(),
            audio_duration_ms,
            options.enable_legacy_python,
        );
        let native_final_report = build_final_pass_report_with_elapsed(
            "native_final_v1",
            native_final_result,
            reference_transcript.as_deref(),
            reference_turns.as_deref(),
            audio_duration_ms,
            native_final_ms,
        );

        reports.push(EvalCaseReport {
            id: case.id,
            required: case.required,
            audio_path: case.audio_path.display().to_string(),
            audio_duration_ms,
            reference: reference_summary,
            native_live_current: native_live_report,
            legacy_python_final: legacy_python_report,
            native_final_v1: native_final_report,
        });
    }

    let report = EvalReport {
        model: manifest.model,
        runtime: RuntimeMetadata {
            machine_summary: machine.summary(),
            chip: chip_label(&machine),
            architecture: format!("{:?}", machine.architecture),
        },
        cases: reports,
    };

    let report_json = serde_json::to_string_pretty(&report)
        .map_err(|err| format!("Failed to serialize eval report: {err}"))?;
    if let Some(path) = options.baseline_out {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "Failed to create ambient eval baseline directory {}: {err}",
                    parent.display()
                )
            })?;
        }
        fs::write(&path, report_json.as_bytes())
            .map_err(|err| format!("Failed to write eval baseline {}: {err}", path.display()))?;
    }

    println!("{report_json}");
    Ok(())
}

fn parse_options(mut args: impl Iterator<Item = String>) -> Result<EvalOptions, String> {
    let mut manifest_path = None;
    let mut baseline_out = None;
    let mut enable_legacy_python = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest" => {
                manifest_path = args.next().map(PathBuf::from);
            }
            "--baseline-out" => {
                baseline_out = args.next().map(PathBuf::from);
            }
            "--enable-legacy-python" => {
                enable_legacy_python = true;
            }
            _ => {}
        }
    }

    let manifest_path = manifest_path.ok_or_else(|| {
        "Usage: cargo run --bin ambient_eval -- --manifest path/to/manifest.json [--baseline-out path] [--enable-legacy-python]".to_string()
    })?;

    Ok(EvalOptions {
        manifest_path,
        baseline_out,
        enable_legacy_python,
    })
}

fn default_model() -> String {
    "base".to_string()
}

fn load_reference_turns(path: &Path) -> Result<Vec<ReferenceTurn>, String> {
    serde_json::from_str(
        &fs::read_to_string(path)
            .map_err(|err| format!("Failed to read reference turns {}: {err}", path.display()))?,
    )
    .map_err(|err| format!("Failed to parse reference turns {}: {err}", path.display()))
}

fn read_audio_file(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|err| format!("Failed to open WAV {}: {err}", path.display()))?;
    let spec = reader.spec();

    let mut raw_samples = Vec::new();
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for sample in reader.samples::<f32>() {
                raw_samples.push(sample.map_err(|err| {
                    format!("Failed to read float WAV sample from {}: {err}", path.display())
                })?);
            }
        }
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample.saturating_sub(1) as u32)) as f32;
            for sample in reader.samples::<i32>() {
                raw_samples.push(
                    sample.map_err(|err| {
                        format!("Failed to read PCM WAV sample from {}: {err}", path.display())
                    })? as f32
                        / scale.max(1.0),
                );
            }
        }
    }

    let channels = spec.channels.max(1) as usize;
    let mono = if channels == 1 {
        raw_samples
    } else {
        raw_samples
            .chunks(channels)
            .map(|frame| frame[0])
            .collect::<Vec<_>>()
    };

    if spec.sample_rate == TARGET_SAMPLE_RATE {
        Ok(mono)
    } else {
        Ok(resample_to_target(&mono, spec.sample_rate))
    }
}

fn build_segment_report(
    backend: &str,
    engine: &str,
    segments: Vec<CanonicalSegment>,
    reference_transcript: Option<&str>,
    reference_turns: Option<&[ReferenceTurn]>,
    total_ms: u64,
    audio_duration_ms: u64,
    asset_version: Option<String>,
) -> BackendReport {
    let transcript = segments_to_transcript(&segments);
    let plain_text = plain_transcript(&segments);
    BackendReport {
        status: "ok".to_string(),
        backend: backend.to_string(),
        engine: Some(engine.to_string()),
        transcript: Some(transcript),
        segment_count: Some(segments.len()),
        turn_count: Some(segments.len()),
        speaker_count: Some(unique_speaker_count(&segments)),
        speaker_count_delta: reference_turns.map(|turns| {
            unique_speaker_count(&segments) as isize - unique_reference_speaker_count(turns) as isize
        }),
        turn_count_delta: reference_turns.map(|turns| segments.len() as isize - turns.len() as isize),
        word_error_rate: reference_transcript.map(|reference| word_error_rate(reference, &plain_text)),
        turn_boundary_f1_500ms: reference_turns.map(|turns| turn_boundary_f1(turns, &segments, 500)),
        speaker_time_accuracy: reference_turns.map(|turns| speaker_time_accuracy(turns, &segments)),
        total_ms: Some(total_ms),
        real_time_factor: Some(real_time_factor(total_ms, audio_duration_ms)),
        peak_rss_mb: peak_rss_mb(),
        asset_version,
        error: None,
    }
}

fn build_final_pass_report(
    backend: &str,
    result: Result<AmbientFinalPassResult, String>,
    reference_transcript: Option<&str>,
    reference_turns: Option<&[ReferenceTurn]>,
    audio_duration_ms: u64,
    enabled: bool,
) -> BackendReport {
    if !enabled {
        return BackendReport {
            status: "disabled".to_string(),
            backend: backend.to_string(),
            engine: None,
            transcript: None,
            segment_count: None,
            turn_count: None,
            speaker_count: None,
            speaker_count_delta: None,
            turn_count_delta: None,
            word_error_rate: None,
            turn_boundary_f1_500ms: None,
            speaker_time_accuracy: None,
            total_ms: None,
            real_time_factor: None,
            peak_rss_mb: None,
            asset_version: None,
            error: Some("Legacy Python benchmark disabled.".to_string()),
        };
    }

    build_final_pass_report_with_elapsed(
        backend,
        result,
        reference_transcript,
        reference_turns,
        audio_duration_ms,
        0,
    )
}

fn build_final_pass_report_with_elapsed(
    backend: &str,
    result: Result<AmbientFinalPassResult, String>,
    reference_transcript: Option<&str>,
    reference_turns: Option<&[ReferenceTurn]>,
    audio_duration_ms: u64,
    fallback_elapsed_ms: u64,
) -> BackendReport {
    match result {
        Ok(result) => build_segment_report(
            backend,
            &result.engine,
            result.segments,
            reference_transcript,
            reference_turns,
            if result.diagnostics.total_ms == 0 {
                fallback_elapsed_ms
            } else {
                result.diagnostics.total_ms
            },
            audio_duration_ms,
            result.diagnostics.asset_version.clone(),
        ),
        Err(err) => BackendReport {
            status: "error".to_string(),
            backend: backend.to_string(),
            engine: None,
            transcript: None,
            segment_count: None,
            turn_count: None,
            speaker_count: None,
            speaker_count_delta: None,
            turn_count_delta: None,
            word_error_rate: None,
            turn_boundary_f1_500ms: None,
            speaker_time_accuracy: None,
            total_ms: None,
            real_time_factor: None,
            peak_rss_mb: peak_rss_mb(),
            asset_version: None,
            error: Some(err),
        },
    }
}

fn unique_speaker_count(segments: &[CanonicalSegment]) -> usize {
    let mut speakers = segments.iter().map(|segment| segment.speaker).collect::<Vec<_>>();
    speakers.sort_by_key(|speaker| speaker.index());
    speakers.dedup();
    speakers.len()
}

fn unique_reference_speaker_count(turns: &[ReferenceTurn]) -> usize {
    turns
        .iter()
        .map(|turn| turn.speaker.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

fn plain_transcript(segments: &[CanonicalSegment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalized_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split_whitespace()
        .map(|token| token.trim_matches(|c: char| !c.is_alphanumeric() && c != '\''))
        .filter(|token| !token.is_empty())
        .map(|token| token.to_string())
        .collect()
}

fn word_error_rate(reference: &str, hypothesis: &str) -> f64 {
    let reference_words = normalized_words(reference);
    let hypothesis_words = normalized_words(hypothesis);
    if reference_words.is_empty() {
        return 0.0;
    }

    let mut dp = vec![vec![0usize; hypothesis_words.len() + 1]; reference_words.len() + 1];
    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=hypothesis_words.len() {
        dp[0][j] = j;
    }

    for i in 1..=reference_words.len() {
        for j in 1..=hypothesis_words.len() {
            let substitution_cost = usize::from(reference_words[i - 1] != hypothesis_words[j - 1]);
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + substitution_cost);
        }
    }

    dp[reference_words.len()][hypothesis_words.len()] as f64 / reference_words.len() as f64
}

fn turn_boundary_f1(reference_turns: &[ReferenceTurn], segments: &[CanonicalSegment], tolerance_ms: u64) -> f64 {
    let reference_boundaries = reference_turns
        .iter()
        .skip(1)
        .map(|turn| turn.start_ms)
        .collect::<Vec<_>>();
    let predicted_boundaries = segments
        .iter()
        .skip(1)
        .map(|segment| segment.start_ms)
        .collect::<Vec<_>>();

    if reference_boundaries.is_empty() && predicted_boundaries.is_empty() {
        return 1.0;
    }

    let mut matched_reference = vec![false; reference_boundaries.len()];
    let mut matches = 0usize;

    for boundary in predicted_boundaries {
        let mut best_index = None;
        let mut best_delta = tolerance_ms + 1;

        for (index, candidate) in reference_boundaries.iter().enumerate() {
            if matched_reference[index] {
                continue;
            }
            let delta = boundary.abs_diff(*candidate);
            if delta <= tolerance_ms && delta < best_delta {
                best_delta = delta;
                best_index = Some(index);
            }
        }

        if let Some(index) = best_index {
            matched_reference[index] = true;
            matches += 1;
        }
    }

    let precision = if segments.len() > 1 {
        matches as f64 / (segments.len() - 1) as f64
    } else {
        0.0
    };
    let recall = if reference_boundaries.is_empty() {
        1.0
    } else {
        matches as f64 / reference_boundaries.len() as f64
    };

    if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    }
}

fn speaker_time_accuracy(reference_turns: &[ReferenceTurn], segments: &[CanonicalSegment]) -> f64 {
    if reference_turns.is_empty() || segments.is_empty() {
        return 0.0;
    }

    let reference_speakers = reference_turns
        .iter()
        .map(|turn| turn.speaker.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let predicted_speakers = segments
        .iter()
        .map(|segment| segment.speaker)
        .collect::<Vec<_>>();
    let mut predicted_speakers = predicted_speakers;
    predicted_speakers.sort_by_key(|speaker| speaker.index());
    predicted_speakers.dedup();

    let mut overlaps = vec![vec![0u64; predicted_speakers.len()]; reference_speakers.len()];
    for (reference_index, reference_speaker) in reference_speakers.iter().enumerate() {
        for (predicted_index, predicted_speaker) in predicted_speakers.iter().enumerate() {
            let overlap = reference_turns
                .iter()
                .filter(|turn| &turn.speaker == reference_speaker)
                .flat_map(|turn| {
                    segments.iter().filter(move |segment| &segment.speaker == predicted_speaker).map(
                        move |segment| {
                            overlap_ms(turn.start_ms, turn.end_ms, segment.start_ms, segment.end_ms)
                        },
                    )
                })
                .sum::<u64>();
            overlaps[reference_index][predicted_index] = overlap;
        }
    }

    let best_overlap = best_overlap_assignment(&overlaps, 0, 0, &mut vec![false; predicted_speakers.len()]);
    let total_reference_ms = reference_turns
        .iter()
        .map(|turn| turn.end_ms.saturating_sub(turn.start_ms))
        .sum::<u64>()
        .max(1);

    best_overlap as f64 / total_reference_ms as f64
}

fn best_overlap_assignment(
    overlaps: &[Vec<u64>],
    reference_index: usize,
    current_total: u64,
    used_predicted: &mut [bool],
) -> u64 {
    if reference_index >= overlaps.len() {
        return current_total;
    }

    let mut best = best_overlap_assignment(overlaps, reference_index + 1, current_total, used_predicted);
    for predicted_index in 0..used_predicted.len() {
        if used_predicted[predicted_index] {
            continue;
        }
        used_predicted[predicted_index] = true;
        best = best.max(best_overlap_assignment(
            overlaps,
            reference_index + 1,
            current_total + overlaps[reference_index][predicted_index],
            used_predicted,
        ));
        used_predicted[predicted_index] = false;
    }

    best
}

fn overlap_ms(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> u64 {
    a_end.min(b_end).saturating_sub(a_start.max(b_start))
}

fn real_time_factor(total_ms: u64, audio_duration_ms: u64) -> f64 {
    if audio_duration_ms == 0 {
        0.0
    } else {
        total_ms as f64 / audio_duration_ms as f64
    }
}

fn peak_rss_mb() -> Option<f64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    Some(usage.ru_maxrss as f64 / 1024.0 / 1024.0)
}

fn chip_label(machine: &MachineProfile) -> String {
    match machine.family {
        screamer_whisper::MachineFamily::AppleSilicon(chip) => match chip.generation {
            Some(generation) => format!("Apple M{} {:?}", generation, chip.tier),
            None => format!("Apple {:?}", chip.tier),
        },
        screamer_whisper::MachineFamily::Intel => "Intel".to_string(),
        screamer_whisper::MachineFamily::OtherArm => "Other ARM".to_string(),
        screamer_whisper::MachineFamily::Other => "Other".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screamer_core::ambient::{AudioLane, SpeakerLabel};

    fn segment(start_ms: u64, end_ms: u64, speaker: SpeakerLabel) -> CanonicalSegment {
        CanonicalSegment {
            id: start_ms + 1,
            lane: AudioLane::Microphone,
            speaker,
            start_ms,
            end_ms,
            text: format!("{}-{}", start_ms, end_ms),
        }
    }

    #[test]
    fn turn_boundary_f1_matches_identical_turns() {
        let reference = vec![
            ReferenceTurn {
                start_ms: 0,
                end_ms: 1_000,
                speaker: "A".to_string(),
            },
            ReferenceTurn {
                start_ms: 1_000,
                end_ms: 2_000,
                speaker: "B".to_string(),
            },
        ];
        let predicted = vec![segment(0, 1_000, SpeakerLabel::S1), segment(1_000, 2_000, SpeakerLabel::S2)];

        assert_eq!(turn_boundary_f1(&reference, &predicted, 500), 1.0);
    }

    #[test]
    fn speaker_time_accuracy_allows_label_permutation() {
        let reference = vec![
            ReferenceTurn {
                start_ms: 0,
                end_ms: 1_000,
                speaker: "Alice".to_string(),
            },
            ReferenceTurn {
                start_ms: 1_000,
                end_ms: 2_000,
                speaker: "Bob".to_string(),
            },
        ];
        let predicted = vec![segment(0, 1_000, SpeakerLabel::S2), segment(1_000, 2_000, SpeakerLabel::S1)];

        assert_eq!(speaker_time_accuracy(&reference, &predicted), 1.0);
    }
}

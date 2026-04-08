use screamer_core::ambient::{
    chunk_len_samples, chunk_step_samples, merge_segment, AmbientSessionConfig, AudioLane,
    CanonicalSegment, DiarizationEngine, DiarizationRequest, DiarizedSegment, TranscriptSegment,
};
use screamer_core::audio::TARGET_SAMPLE_RATE;
use screamer_core::session::{prepare_final_transcription, FinalTranscriptionAction};
use screamer_whisper::{DetailedTranscriptionOutput, Transcriber};

pub const MIN_CHUNK_PROCESS_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 2;

#[derive(Clone, Debug)]
pub struct ProcessState {
    pub processed_until: usize,
    pub next_segment_id: u64,
}

impl Default for ProcessState {
    fn default() -> Self {
        Self {
            processed_until: 0,
            next_segment_id: 1,
        }
    }
}

pub fn process_audio_snapshot(
    ambient_config: &AmbientSessionConfig,
    transcriber: &Transcriber,
    diarization_engine: &dyn DiarizationEngine,
    samples: &[f32],
    process_state: &mut ProcessState,
    segments: &mut Vec<CanonicalSegment>,
) -> Result<bool, String> {
    if samples.len().saturating_sub(process_state.processed_until) < MIN_CHUNK_PROCESS_SAMPLES {
        return Ok(false);
    }

    let chunk_len = chunk_len_samples(ambient_config.chunk_seconds, TARGET_SAMPLE_RATE as usize);
    let step = chunk_step_samples(
        ambient_config.chunk_seconds,
        ambient_config.overlap_seconds,
        TARGET_SAMPLE_RATE as usize,
    )
    .max(MIN_CHUNK_PROCESS_SAMPLES);
    let mut changed = false;

    while samples.len().saturating_sub(process_state.processed_until) >= MIN_CHUNK_PROCESS_SAMPLES {
        let end = (process_state.processed_until + step).min(samples.len());
        let start = end.saturating_sub(chunk_len);
        let window = &samples[start..end];

        match prepare_final_transcription(window) {
            FinalTranscriptionAction::Ready(window_bounds) => {
                let transcribe_range = window_bounds.range;
                let chunk = &window[transcribe_range.clone()];
                let detailed = transcriber.transcribe_detailed_profiled(chunk)?;
                let chunk_start_ms = samples_to_ms(start + transcribe_range.start);
                let chunk_end_ms = samples_to_ms(start + transcribe_range.end);
                let transcript_segments =
                    transcript_segments_from_decode(AudioLane::Microphone, chunk, &detailed);
                if !transcript_segments.is_empty() {
                    let diarized = diarization_engine.diarize(DiarizationRequest {
                        lane: AudioLane::Microphone,
                        sample_rate_hz: TARGET_SAMPLE_RATE as usize,
                        chunk_start_ms,
                        chunk_end_ms,
                        samples: chunk,
                        transcript_segments: &transcript_segments,
                        previous_segments: segments,
                    });
                    changed |= integrate_diarized_segments(
                        segments,
                        &diarized,
                        &mut process_state.next_segment_id,
                    );
                }
            }
            FinalTranscriptionAction::SkipSilence
            | FinalTranscriptionAction::SkipTooShort { .. } => {}
        }

        process_state.processed_until = end;
        if end == samples.len() {
            break;
        }
    }

    Ok(changed)
}

pub fn run_native_ambient_pass(
    ambient_config: &AmbientSessionConfig,
    transcriber: &Transcriber,
    diarization_engine: &dyn DiarizationEngine,
    samples: &[f32],
) -> Result<Vec<CanonicalSegment>, String> {
    let mut process_state = ProcessState::default();
    let mut segments = Vec::new();
    let _ = process_audio_snapshot(
        ambient_config,
        transcriber,
        diarization_engine,
        samples,
        &mut process_state,
        &mut segments,
    )?;
    Ok(segments)
}

pub fn transcript_segments_from_decode(
    lane: AudioLane,
    samples: &[f32],
    detailed: &DetailedTranscriptionOutput,
) -> Vec<TranscriptSegment> {
    let mut segments = detailed
        .segments
        .iter()
        .map(|segment| TranscriptSegment {
            lane,
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            speaker_turn_next: segment.speaker_turn_next,
            text: segment.text.clone(),
        })
        .collect::<Vec<_>>();

    if segments.is_empty() && !detailed.text.trim().is_empty() {
        segments.push(TranscriptSegment {
            lane,
            start_ms: 0,
            end_ms: samples_to_ms(samples.len()).max(1),
            speaker_turn_next: false,
            text: detailed.text.trim().to_string(),
        });
    }

    segments
}

pub fn integrate_diarized_segments(
    segments: &mut Vec<CanonicalSegment>,
    diarized: &[DiarizedSegment],
    next_segment_id: &mut u64,
) -> bool {
    let mut changed = false;

    for segment in diarized {
        let candidate = CanonicalSegment {
            id: *next_segment_id,
            lane: segment.lane,
            speaker: segment.speaker,
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            text: segment.text.trim().to_string(),
        };

        let len_before = segments.len();
        let tail_before = segments.last().cloned();
        if merge_segment(segments, candidate, segment.force_new).is_some() {
            if segments.len() > len_before {
                *next_segment_id += 1;
                changed = true;
            } else if segments.last() != tail_before.as_ref() {
                changed = true;
            }
        }
    }

    changed
}

fn samples_to_ms(samples: usize) -> u64 {
    (samples as u64 * 1_000) / TARGET_SAMPLE_RATE as u64
}

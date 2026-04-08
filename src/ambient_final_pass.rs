use crate::ambient_pipeline::transcript_segments_from_decode;
use crate::ambient_whisperx::{
    run_whisperx_helper, WhisperxHelperInputSegment, WhisperxHelperMode, WhisperxHelperRequest,
    whisperx_model_for_screamer_model,
};
pub use screamer_diarization::NativeFinalPassDiagnostics as AmbientFinalPassDiagnostics;
use screamer_core::ambient::{AudioLane, CanonicalSegment, SpeakerLabel};
use screamer_whisper::Transcriber;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct AmbientFinalPassResult {
    pub engine: String,
    pub transcript_text: String,
    pub segments: Vec<CanonicalSegment>,
    pub diagnostics: AmbientFinalPassDiagnostics,
}

pub fn run_native_diarization_final_pass(
    samples: &[f32],
    transcriber: &Transcriber,
) -> Result<AmbientFinalPassResult, String> {
    let detailed = transcriber.transcribe_detailed_profiled(samples)?;
    let transcript_segments =
        transcript_segments_from_decode(AudioLane::Microphone, samples, &detailed);
    let mut result = screamer_diarization::run_native_final_pass(
        screamer_diarization::NativeFinalPassRequest {
            sample_rate_hz: screamer_core::audio::TARGET_SAMPLE_RATE as usize,
            samples,
            transcript_segments: &transcript_segments,
            transcript_text: &detailed.text,
        },
    )?;
    let transcription_ms = detailed.profile.total.as_millis() as u64;
    result.diagnostics.transcription_ms = transcription_ms;
    result.diagnostics.total_ms = result.diagnostics.total_ms.saturating_add(transcription_ms);
    let audio_duration_ms =
        ((samples.len() as u128 * 1_000) / screamer_core::audio::TARGET_SAMPLE_RATE as u128) as f64;
    result.diagnostics.real_time_factor = if audio_duration_ms > 0.0 {
        result.diagnostics.total_ms as f64 / audio_duration_ms
    } else {
        0.0
    };

    Ok(AmbientFinalPassResult {
        engine: result.engine,
        transcript_text: result.transcript_text,
        segments: result.segments,
        diagnostics: result.diagnostics,
    })
}

pub fn run_whisperx_hybrid_final_pass(
    audio_path: &Path,
    screamer_model: &str,
) -> Result<AmbientFinalPassResult, String> {
    let request = WhisperxHelperRequest {
        mode: WhisperxHelperMode::WhisperxHybrid,
        audio_path: audio_path.to_path_buf(),
        model: whisperx_model_for_screamer_model(screamer_model).to_string(),
        device: "cpu".to_string(),
        compute_type: "int8".to_string(),
        language: "en".to_string(),
        segments: Vec::new(),
    };

    helper_response_to_result(run_whisperx_helper(&request)?)
}

pub fn run_pyannote_reassign_pass(
    audio_path: &Path,
    screamer_model: &str,
    segments: &[CanonicalSegment],
) -> Result<AmbientFinalPassResult, String> {
    let request = WhisperxHelperRequest {
        mode: WhisperxHelperMode::PyannoteReassign,
        audio_path: audio_path.to_path_buf(),
        model: whisperx_model_for_screamer_model(screamer_model).to_string(),
        device: "cpu".to_string(),
        compute_type: "int8".to_string(),
        language: "en".to_string(),
        segments: segments
            .iter()
            .map(|segment| WhisperxHelperInputSegment {
                start_ms: segment.start_ms,
                end_ms: segment.end_ms,
                text: segment.text.clone(),
            })
            .collect(),
    };

    helper_response_to_result(run_whisperx_helper(&request)?)
}

fn helper_response_to_result(
    response: crate::ambient_whisperx::WhisperxHelperResponse,
) -> Result<AmbientFinalPassResult, String> {
    if response.segments.is_empty() {
        return Err("WhisperX helper returned no segments.".to_string());
    }

    let mut speaker_map = HashMap::<String, SpeakerLabel>::new();
    let mut next_label_index = 0usize;
    let mut last_label = SpeakerLabel::S1;
    let mut canonical_segments = Vec::with_capacity(response.segments.len());

    for (index, segment) in response.segments.iter().enumerate() {
        let speaker = match segment.speaker.as_deref() {
            Some(raw) if !raw.trim().is_empty() => {
                if let Some(existing) = speaker_map.get(raw) {
                    *existing
                } else {
                    let label = speaker_label_for_index(next_label_index);
                    speaker_map.insert(raw.to_string(), label);
                    next_label_index = next_label_index.saturating_add(1);
                    label
                }
            }
            _ => last_label,
        };
        last_label = speaker;

        canonical_segments.push(CanonicalSegment {
            id: index as u64 + 1,
            lane: screamer_core::ambient::AudioLane::Microphone,
            speaker,
            start_ms: segment.start_ms,
            end_ms: segment.end_ms.max(segment.start_ms + 1),
            text: segment.text.trim().to_string(),
        });
    }

    Ok(AmbientFinalPassResult {
        engine: response.engine,
        transcript_text: response.transcript_text.trim().to_string(),
        segments: canonical_segments,
        diagnostics: AmbientFinalPassDiagnostics {
            asset_version: None,
            detected_speakers: response.diagnostics.detected_speakers,
            transcription_ms: response.diagnostics.transcription_ms,
            alignment_ms: response.diagnostics.alignment_ms,
            diarization_ms: response.diagnostics.diarization_ms,
            assignment_ms: response.diagnostics.assignment_ms,
            total_ms: response.diagnostics.total_ms,
            ..AmbientFinalPassDiagnostics::default()
        },
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_response_maps_legacy_diagnostics() {
        let result = helper_response_to_result(crate::ambient_whisperx::WhisperxHelperResponse {
            engine: "legacy".to_string(),
            transcript_text: "hello".to_string(),
            segments: vec![crate::ambient_whisperx::WhisperxHelperSegment {
                start_ms: 0,
                end_ms: 1000,
                speaker: Some("SPEAKER_00".to_string()),
                text: "hello".to_string(),
                words: Vec::new(),
            }],
            diagnostics: crate::ambient_whisperx::WhisperxHelperDiagnostics {
                detected_speakers: 1,
                transcription_ms: 12,
                alignment_ms: 3,
                diarization_ms: 4,
                assignment_ms: 5,
                total_ms: 24,
            },
        })
        .unwrap();

        assert_eq!(result.diagnostics.transcription_ms, 12);
        assert_eq!(result.diagnostics.alignment_ms, 3);
        assert_eq!(result.diagnostics.total_ms, 24);
    }
}

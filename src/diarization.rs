use screamer_core::ambient::{
    DiarizationEngine, DiarizationRequest, DiarizedSegment, SpeakerLabel,
};
use screamer_core::speaker::SpeakerIdentifier;
use std::sync::{Arc, Mutex};

pub fn default_diarization_engine() -> Arc<dyn DiarizationEngine> {
    Arc::new(VoiceDiarizationEngine::new())
}

/// Diarization engine that identifies speakers by the sound of their voice
/// using MFCC-based speaker embeddings.
pub struct VoiceDiarizationEngine {
    identifier: Mutex<SpeakerIdentifier>,
}

impl VoiceDiarizationEngine {
    pub fn new() -> Self {
        Self {
            identifier: Mutex::new(SpeakerIdentifier::new()),
        }
    }

    /// Extract the audio slice for a transcript segment within a chunk.
    /// `chunk_samples` is the full audio chunk; `segment_start_ms` and
    /// `segment_end_ms` are relative to the chunk start.
    fn segment_audio<'a>(
        chunk_samples: &'a [f32],
        sample_rate_hz: usize,
        segment_start_ms: u64,
        segment_end_ms: u64,
    ) -> &'a [f32] {
        let start_idx = (segment_start_ms as usize * sample_rate_hz / 1000)
            .min(chunk_samples.len());
        let end_idx = (segment_end_ms as usize * sample_rate_hz / 1000)
            .min(chunk_samples.len());
        &chunk_samples[start_idx..end_idx]
    }
}

impl DiarizationEngine for VoiceDiarizationEngine {
    fn label(&self) -> &'static str {
        "voice_mfcc_v1"
    }

    fn diarize(&self, request: DiarizationRequest<'_>) -> Vec<DiarizedSegment> {
        if request.transcript_segments.is_empty() {
            return Vec::new();
        }

        let mut identifier = self.identifier.lock().unwrap_or_else(|p| p.into_inner());

        let mut turns = Vec::with_capacity(request.transcript_segments.len());
        let mut last_speaker = request
            .previous_segments
            .last()
            .map(|s| s.speaker)
            .unwrap_or(SpeakerLabel::S1);
        let mut prev_speaker_turn_next = false;

        for segment in request.transcript_segments {
            let text = segment.text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.is_empty() {
                continue;
            }

            let audio = Self::segment_audio(
                request.samples,
                request.sample_rate_hz,
                segment.start_ms,
                segment.end_ms,
            );

            // Try voice identification; fall back to previous speaker if audio too short.
            let speaker = identifier.identify(audio).unwrap_or(last_speaker);
            last_speaker = speaker;

            let start_ms = request.chunk_start_ms.saturating_add(segment.start_ms);
            let end_ms = request
                .chunk_start_ms
                .saturating_add(segment.end_ms)
                .max(start_ms + 1);

            turns.push(DiarizedSegment {
                lane: segment.lane,
                speaker,
                start_ms,
                end_ms,
                text,
                force_new: prev_speaker_turn_next,
            });
            prev_speaker_turn_next = segment.speaker_turn_next;
        }

        turns
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screamer_core::ambient::{AudioLane, TranscriptSegment};
    use std::f32::consts::PI;

    const SR: usize = 16_000;

    fn sine_wave(freq: f32, duration_secs: f32, amplitude: f32) -> Vec<f32> {
        let n = (SR as f32 * duration_secs) as usize;
        (0..n)
            .map(|i| amplitude * (2.0 * PI * freq * i as f32 / SR as f32).sin())
            .collect()
    }

    fn transcript(
        start_ms: u64,
        end_ms: u64,
        text: &str,
    ) -> TranscriptSegment {
        TranscriptSegment {
            lane: AudioLane::Microphone,
            start_ms,
            end_ms,
            speaker_turn_next: false,
            text: text.to_string(),
        }
    }

    #[test]
    fn same_voice_gets_same_label() {
        let engine = VoiceDiarizationEngine::new();
        // 2 seconds of 300 Hz tone
        let audio = sine_wave(300.0, 2.0, 0.5);

        let turns = engine.diarize(DiarizationRequest {
            lane: AudioLane::Microphone,
            sample_rate_hz: SR,
            chunk_start_ms: 0,
            chunk_end_ms: 2_000,
            samples: &audio,
            transcript_segments: &[
                transcript(0, 1_000, "hello there"),
                transcript(1_000, 2_000, "how are you"),
            ],
            previous_segments: &[],
        });

        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].speaker, turns[1].speaker);
    }

    #[test]
    fn different_voices_get_different_labels() {
        let engine = VoiceDiarizationEngine::new();
        // Construct audio with two very different "voices"
        let voice_a = sine_wave(200.0, 1.0, 0.5);
        let voice_b = sine_wave(3000.0, 1.0, 0.5);
        let mut audio = voice_a;
        audio.extend_from_slice(&voice_b);

        let turns = engine.diarize(DiarizationRequest {
            lane: AudioLane::Microphone,
            sample_rate_hz: SR,
            chunk_start_ms: 0,
            chunk_end_ms: 2_000,
            samples: &audio,
            transcript_segments: &[
                transcript(0, 1_000, "I think we should proceed"),
                transcript(1_000, 2_000, "I disagree completely"),
            ],
            previous_segments: &[],
        });

        assert_eq!(turns.len(), 2);
        assert_ne!(
            turns[0].speaker, turns[1].speaker,
            "different voices should be different speakers"
        );
    }
}

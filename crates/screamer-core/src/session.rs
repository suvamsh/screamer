use std::ops::Range;
use std::time::Duration;

pub const LIVE_TRANSCRIPTION_INTERVAL: Duration = Duration::from_millis(350);

const LIVE_TRANSCRIPTION_MIN_SAMPLES: usize = 9600;
const LIVE_TRANSCRIPTION_MIN_DELTA: usize = 2400;
const LIVE_TRANSCRIPTION_MAX_SAMPLES: usize = 192_000;
const LIVE_TRANSCRIPTION_PADDING_SAMPLES: usize = 8000;
const LIVE_TRANSCRIPT_MAX_CHARS: usize = 180;
const SPEECH_DETECTION_LOOKBACK_SAMPLES: usize = 16_000;
const SPEECH_DETECTION_FRAME_SAMPLES: usize = 320;
const SPEECH_DETECTION_FRAME_RMS_GATE: f32 = 0.006;
const SPEECH_DETECTION_MIN_ACTIVE_FRAMES: usize = 3;
const SPEECH_TRIM_PADDING_SAMPLES: usize = 1600;
const FINAL_TRANSCRIPTION_MIN_SAMPLES: usize = 1600;
const SHORT_UTTERANCE_FINAL_MIN_SAMPLES: usize =
    SPEECH_DETECTION_FRAME_SAMPLES * SHORT_UTTERANCE_MIN_ACTIVE_FRAMES;
const SHORT_UTTERANCE_MAX_SAMPLES: usize = 12_800;
const SHORT_UTTERANCE_FRAME_RMS_GATE: f32 = 0.004;
const SHORT_UTTERANCE_MIN_ACTIVE_FRAMES: usize = 2;
const SHORT_UTTERANCE_MIN_PEAK: f32 = 0.02;

#[derive(Clone, Copy)]
struct SpeechDetectionConfig {
    frame_rms_gate: f32,
    min_active_frames: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinalSpeechWindowKind {
    Standard,
    ShortUtterance,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalSpeechWindow {
    pub range: Range<usize>,
    pub kind: FinalSpeechWindowKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FinalTranscriptionAction {
    SkipSilence,
    SkipTooShort { trimmed_len: usize },
    Ready(FinalSpeechWindow),
}

#[derive(Clone, Debug, PartialEq)]
pub enum LivePreviewAction {
    Skip,
    Clear,
    Transcribe {
        padded_samples: Vec<f32>,
        observed_samples_len: usize,
    },
}

#[derive(Default)]
pub struct LivePreviewState {
    last_transcribed_samples: usize,
    last_text: String,
}

impl LivePreviewState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_action(&self, samples: &[f32]) -> LivePreviewAction {
        if samples.len() < LIVE_TRANSCRIPTION_MIN_SAMPLES {
            return LivePreviewAction::Skip;
        }

        if samples.len().saturating_sub(self.last_transcribed_samples)
            < LIVE_TRANSCRIPTION_MIN_DELTA
        {
            return LivePreviewAction::Skip;
        }

        if !samples_contain_speech(recent_speech_window(samples)) {
            return LivePreviewAction::Clear;
        }

        LivePreviewAction::Transcribe {
            padded_samples: padded_live_samples(samples),
            observed_samples_len: samples.len(),
        }
    }

    pub fn register_transcription(
        &mut self,
        observed_samples_len: usize,
        text: &str,
    ) -> Option<String> {
        self.last_transcribed_samples = observed_samples_len;

        let display_text = format_live_transcript(text);
        if display_text.is_empty() || display_text == self.last_text {
            return None;
        }

        self.last_text.clear();
        self.last_text.push_str(&display_text);
        Some(display_text)
    }

    pub fn clear(&mut self) {
        self.last_transcribed_samples = 0;
        self.last_text.clear();
    }
}

pub fn prepare_final_transcription(samples: &[f32]) -> FinalTranscriptionAction {
    let Some(window) = final_transcription_window(samples) else {
        return FinalTranscriptionAction::SkipSilence;
    };

    let trimmed_len = window.range.end - window.range.start;
    if trimmed_len < minimum_final_transcription_samples(window.kind) {
        return FinalTranscriptionAction::SkipTooShort { trimmed_len };
    }

    FinalTranscriptionAction::Ready(window)
}

pub fn final_transcription_window(samples: &[f32]) -> Option<FinalSpeechWindow> {
    if let Some((start, end)) = speech_activity_bounds(samples) {
        let range = padded_speech_range(samples.len(), start, end);
        if range.end.saturating_sub(range.start)
            >= minimum_final_transcription_samples(FinalSpeechWindowKind::Standard)
        {
            return Some(FinalSpeechWindow {
                range,
                kind: FinalSpeechWindowKind::Standard,
            });
        }
    }

    if samples.len() > SHORT_UTTERANCE_MAX_SAMPLES {
        return None;
    }

    let short_config = SpeechDetectionConfig {
        frame_rms_gate: SHORT_UTTERANCE_FRAME_RMS_GATE,
        min_active_frames: SHORT_UTTERANCE_MIN_ACTIVE_FRAMES,
    };
    let (start, end) = speech_activity_bounds_with_config(samples, short_config)?;
    let range = padded_speech_range(samples.len(), start, end);
    if range.end.saturating_sub(range.start)
        < minimum_final_transcription_samples(FinalSpeechWindowKind::ShortUtterance)
    {
        return None;
    }
    if max_abs_sample(&samples[range.clone()]) < SHORT_UTTERANCE_MIN_PEAK {
        return None;
    }

    Some(FinalSpeechWindow {
        range,
        kind: FinalSpeechWindowKind::ShortUtterance,
    })
}

pub fn minimum_final_transcription_samples(kind: FinalSpeechWindowKind) -> usize {
    match kind {
        FinalSpeechWindowKind::Standard => FINAL_TRANSCRIPTION_MIN_SAMPLES,
        FinalSpeechWindowKind::ShortUtterance => SHORT_UTTERANCE_FINAL_MIN_SAMPLES,
    }
}

pub fn samples_contain_speech(samples: &[f32]) -> bool {
    speech_activity_bounds(samples).is_some()
}

pub fn recent_speech_window(samples: &[f32]) -> &[f32] {
    let start = samples
        .len()
        .saturating_sub(SPEECH_DETECTION_LOOKBACK_SAMPLES);
    &samples[start..]
}

pub fn padded_live_samples(samples: &[f32]) -> Vec<f32> {
    let samples = live_preview_window(samples);
    let mut padded = Vec::with_capacity(samples.len() + LIVE_TRANSCRIPTION_PADDING_SAMPLES);
    padded.extend_from_slice(samples);
    padded.resize(samples.len() + LIVE_TRANSCRIPTION_PADDING_SAMPLES, 0.0);
    padded
}

pub fn live_preview_window(samples: &[f32]) -> &[f32] {
    let start = samples.len().saturating_sub(LIVE_TRANSCRIPTION_MAX_SAMPLES);
    &samples[start..]
}

pub fn format_live_transcript(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let total_chars = trimmed.chars().count();
    if total_chars <= LIVE_TRANSCRIPT_MAX_CHARS {
        return trimmed.to_string();
    }

    let tail_start = trimmed
        .char_indices()
        .nth(total_chars - LIVE_TRANSCRIPT_MAX_CHARS)
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let tail = trimmed[tail_start..].trim_start();
    format!("...{}", tail)
}

fn speech_activity_bounds(samples: &[f32]) -> Option<(usize, usize)> {
    speech_activity_bounds_with_config(
        samples,
        SpeechDetectionConfig {
            frame_rms_gate: SPEECH_DETECTION_FRAME_RMS_GATE,
            min_active_frames: SPEECH_DETECTION_MIN_ACTIVE_FRAMES,
        },
    )
}

fn speech_activity_bounds_with_config(
    samples: &[f32],
    config: SpeechDetectionConfig,
) -> Option<(usize, usize)> {
    let mut first_active = None;
    let mut last_active_end = 0usize;
    let mut active_frames = 0usize;

    for (frame_idx, frame) in samples.chunks(SPEECH_DETECTION_FRAME_SAMPLES).enumerate() {
        if frame_rms(frame) < config.frame_rms_gate {
            continue;
        }

        active_frames += 1;
        let frame_start = frame_idx * SPEECH_DETECTION_FRAME_SAMPLES;
        first_active.get_or_insert(frame_start);
        last_active_end = frame_start + frame.len();
    }

    if active_frames < config.min_active_frames {
        return None;
    }

    Some((first_active.unwrap_or(0), last_active_end))
}

fn padded_speech_range(total_len: usize, start: usize, end: usize) -> Range<usize> {
    start.saturating_sub(SPEECH_TRIM_PADDING_SAMPLES)
        ..(end + SPEECH_TRIM_PADDING_SAMPLES).min(total_len)
}

fn max_abs_sample(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
}

fn frame_rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }

    (frame.iter().map(|sample| sample * sample).sum::<f32>() / frame.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_preview_window_caps_to_recent_audio() {
        let samples: Vec<f32> = (0..(LIVE_TRANSCRIPTION_MAX_SAMPLES + 10))
            .map(|v| v as f32)
            .collect();
        let window = live_preview_window(&samples);

        assert_eq!(window.len(), LIVE_TRANSCRIPTION_MAX_SAMPLES);
        assert_eq!(window[0], 10.0);
        assert_eq!(window[window.len() - 1], samples[samples.len() - 1]);
    }

    #[test]
    fn live_transcript_formatting_keeps_recent_suffix() {
        let input = "alpha ".repeat(64);
        let formatted = format_live_transcript(&input);

        assert!(formatted.starts_with("..."));
        assert!(formatted.ends_with("alpha"));
        assert!(formatted.chars().count() <= LIVE_TRANSCRIPT_MAX_CHARS + 3);
    }

    #[test]
    fn silence_is_not_detected_as_speech() {
        let samples = vec![0.0; SPEECH_DETECTION_LOOKBACK_SAMPLES];
        assert!(!samples_contain_speech(&samples));
    }

    #[test]
    fn low_noise_is_not_detected_as_speech() {
        let samples = vec![0.002; SPEECH_DETECTION_LOOKBACK_SAMPLES];
        assert!(!samples_contain_speech(&samples));
    }

    #[test]
    fn sustained_voice_is_detected_as_speech() {
        let samples =
            vec![0.02; SPEECH_DETECTION_FRAME_SAMPLES * SPEECH_DETECTION_MIN_ACTIVE_FRAMES];
        assert!(samples_contain_speech(&samples));
    }

    #[test]
    fn trimmed_speech_range_drops_outer_silence_with_padding() {
        let mut samples = vec![0.0; SPEECH_DETECTION_FRAME_SAMPLES * 10];
        samples.extend(vec![0.02; SPEECH_DETECTION_FRAME_SAMPLES * 6]);
        samples.extend(vec![0.0; SPEECH_DETECTION_FRAME_SAMPLES * 8]);

        let range = final_transcription_window(&samples)
            .expect("speech should be detected")
            .range;

        assert_eq!(range.start, 1600);
        assert_eq!(range.end, 6720);
    }

    #[test]
    fn final_transcription_window_salvages_brief_phrase() {
        let samples = vec![0.02; SPEECH_DETECTION_FRAME_SAMPLES * 2];

        let window = final_transcription_window(&samples).expect("brief speech should survive");
        assert_eq!(window.range.start, 0);
        assert_eq!(window.range.end, samples.len());
        assert!(matches!(window.kind, FinalSpeechWindowKind::ShortUtterance));
    }

    #[test]
    fn final_transcription_window_ignores_single_short_spike() {
        let samples = vec![0.03; SPEECH_DETECTION_FRAME_SAMPLES];

        assert!(final_transcription_window(&samples).is_none());
    }

    #[test]
    fn live_preview_state_requests_transcription_after_enough_new_audio() {
        let mut samples = vec![0.0; 6400];
        samples.extend(vec![0.02; 6400]);
        let state = LivePreviewState::new();

        let action = state.next_action(&samples);

        assert!(matches!(action, LivePreviewAction::Transcribe { .. }));
    }

    #[test]
    fn live_preview_state_clears_when_recent_window_has_no_speech() {
        let state = LivePreviewState::new();
        let samples = vec![0.0; LIVE_TRANSCRIPTION_MIN_SAMPLES + 200];

        assert_eq!(state.next_action(&samples), LivePreviewAction::Clear);
    }

    #[test]
    fn prepare_final_transcription_reports_silence() {
        let samples = vec![0.0; 8000];

        assert_eq!(
            prepare_final_transcription(&samples),
            FinalTranscriptionAction::SkipSilence
        );
    }
}

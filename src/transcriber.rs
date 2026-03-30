use crate::hardware::{ComputeBackendPreference, MachineProfile, RuntimeTuning};
use crate::model_paths;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

const AUDIO_CTX_SAMPLES_PER_UNIT: usize = 320;
const AUDIO_CTX_GRANULARITY: i32 = 64;
const WARMUP_SAMPLES: usize = 16_000;
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub enum AudioContextStrategy {
    Adaptive,
    Fixed(i32),
    ModelDefault,
}

#[derive(Clone, Copy, Debug)]
pub struct TranscriberConfig {
    pub n_threads: i32,
    pub no_timestamps: bool,
    pub audio_ctx: AudioContextStrategy,
    pub adaptive_audio_ctx_min: i32,
    pub reuse_state: bool,
    pub compute_backend: ComputeBackendPreference,
    pub flash_attn: bool,
    pub gpu_device: i32,
}

impl Default for TranscriberConfig {
    fn default() -> Self {
        let tuning = MachineProfile::detect().recommended_tuning();
        Self {
            n_threads: tuning.n_threads,
            no_timestamps: true,
            audio_ctx: AudioContextStrategy::Adaptive,
            adaptive_audio_ctx_min: tuning.adaptive_audio_ctx_min,
            reuse_state: true,
            compute_backend: tuning.compute_backend,
            flash_attn: tuning.flash_attn,
            gpu_device: tuning.gpu_device,
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TranscriptionProfile {
    pub state_acquire: Duration,
    pub inference: Duration,
    pub extract: Duration,
    pub total: Duration,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct TranscriptionOutput {
    pub text: String,
    pub profile: TranscriptionProfile,
}

enum StateAccess<'a> {
    Borrowed(MutexGuard<'a, WhisperState>),
    Owned(WhisperState),
}

impl StateAccess<'_> {
    fn as_mut(&mut self) -> &mut WhisperState {
        match self {
            Self::Borrowed(guard) => guard,
            Self::Owned(state) => state,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecodePreset {
    Fast,
    Conservative,
}

pub struct Transcriber {
    ctx: WhisperContext,
    final_state: Option<Mutex<WhisperState>>,
    live_state: Option<Mutex<WhisperState>>,
    config: TranscriberConfig,
    machine_profile: MachineProfile,
    runtime_tuning: RuntimeTuning,
    selected_backend: SelectedBackend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectedBackend {
    Cpu,
    Gpu,
}

impl Transcriber {
    pub fn new(model_path: &Path) -> Result<Self, String> {
        Self::with_config(model_path, TranscriberConfig::default())
    }

    pub fn with_config(model_path: &Path, config: TranscriberConfig) -> Result<Self, String> {
        let machine_profile = MachineProfile::detect();
        let runtime_tuning = RuntimeTuning {
            compute_backend: config.compute_backend,
            flash_attn: config.flash_attn,
            gpu_device: config.gpu_device,
            n_threads: config.n_threads,
            adaptive_audio_ctx_min: config.adaptive_audio_ctx_min,
        };

        let (ctx, selected_backend) = Self::create_context(model_path, &config, &machine_profile)?;

        let (final_state, live_state) = if config.reuse_state {
            let final_state = ctx
                .create_state()
                .map_err(|e| format!("Failed to create whisper state: {}", e))?;
            let live_state = ctx
                .create_state()
                .map_err(|e| format!("Failed to create whisper state: {}", e))?;
            (Some(Mutex::new(final_state)), Some(Mutex::new(live_state)))
        } else {
            (None, None)
        };

        Ok(Self {
            ctx,
            final_state,
            live_state,
            config,
            machine_profile,
            runtime_tuning,
            selected_backend,
        })
    }

    pub fn runtime_summary(&self) -> String {
        format!(
            "{} | backend={} | flash_attn={} | threads={} | min_audio_ctx={}",
            self.machine_profile.summary(),
            match self.selected_backend {
                SelectedBackend::Cpu => "cpu",
                SelectedBackend::Gpu => "gpu",
            },
            yes_no(self.runtime_tuning.flash_attn && self.selected_backend == SelectedBackend::Gpu),
            self.runtime_tuning.n_threads,
            self.runtime_tuning.adaptive_audio_ctx_min
        )
    }

    #[allow(dead_code)]
    pub fn transcribe(&self, samples: &[f32]) -> Result<String, String> {
        self.transcribe_profiled(samples).map(|result| result.text)
    }

    pub fn warm_up(&self, include_live_preview: bool) -> Result<Duration, String> {
        let samples = vec![0.0; WARMUP_SAMPLES];
        let warmup_t0 = Instant::now();

        {
            let mut state = self.acquire_final_state()?;
            let _ = self.run_with_state(state.as_mut(), &samples, DecodePreset::Fast)?;
        }

        if include_live_preview {
            if let Some(state) = &self.live_state {
                let mut state = state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let _ = self.run_with_state(&mut state, &samples, DecodePreset::Fast)?;
            } else {
                let mut state = self.create_state()?;
                let _ = self.run_with_state(&mut state, &samples, DecodePreset::Fast)?;
            }
        }

        Ok(warmup_t0.elapsed())
    }

    pub fn transcribe_profiled(&self, samples: &[f32]) -> Result<TranscriptionOutput, String> {
        let total_t0 = Instant::now();
        let state_t0 = Instant::now();
        let mut state = self.acquire_final_state()?;
        let mut state_acquire = state_t0.elapsed();

        let (mut text, mut inference, mut extract) =
            self.run_with_state(state.as_mut(), samples, DecodePreset::Fast)?;

        if let Some(reason) = conservative_retry_reason(samples, &text) {
            eprintln!("[screamer] Retrying final decode with conservative beam search ({reason})");

            let retry_state_t0 = Instant::now();
            let mut retry_state = self.create_state()?;
            state_acquire += retry_state_t0.elapsed();

            let (retry_text, retry_inference, retry_extract) =
                self.run_with_state(&mut retry_state, samples, DecodePreset::Conservative)?;

            inference += retry_inference;
            extract += retry_extract;

            if !retry_text.trim().is_empty() && !transcript_looks_repetitive(&retry_text) {
                text = retry_text;
            }
        }

        Ok(TranscriptionOutput {
            text,
            profile: TranscriptionProfile {
                state_acquire,
                inference,
                extract,
                total: total_t0.elapsed(),
            },
        })
    }

    pub fn try_transcribe(&self, samples: &[f32]) -> Result<Option<String>, String> {
        let Some(mut state) = self.try_acquire_live_state()? else {
            return Ok(None);
        };

        let (text, _, _) = self.run_with_state(state.as_mut(), samples, DecodePreset::Fast)?;
        Ok(Some(text))
    }

    /// Find the model file, checking bundle Resources first, then local models/ dir
    pub fn find_model(model_name: &str) -> Option<PathBuf> {
        model_paths::find_model(model_name)
    }

    fn acquire_final_state(&self) -> Result<StateAccess<'_>, String> {
        if let Some(state) = &self.final_state {
            Ok(StateAccess::Borrowed(
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            ))
        } else {
            Ok(StateAccess::Owned(self.create_state()?))
        }
    }

    fn try_acquire_live_state(&self) -> Result<Option<StateAccess<'_>>, String> {
        if let Some(state) = &self.live_state {
            match state.try_lock() {
                Ok(guard) => Ok(Some(StateAccess::Borrowed(guard))),
                Err(TryLockError::WouldBlock) => Ok(None),
                Err(TryLockError::Poisoned(poisoned)) => {
                    Ok(Some(StateAccess::Borrowed(poisoned.into_inner())))
                }
            }
        } else {
            Ok(Some(StateAccess::Owned(self.create_state()?)))
        }
    }

    fn create_state(&self) -> Result<WhisperState, String> {
        self.ctx
            .create_state()
            .map_err(|e| format!("Failed to create whisper state: {}", e))
    }

    fn create_context(
        model_path: &Path,
        config: &TranscriberConfig,
        machine_profile: &MachineProfile,
    ) -> Result<(WhisperContext, SelectedBackend), String> {
        let attempts = match config.compute_backend {
            ComputeBackendPreference::CpuOnly => vec![(false, SelectedBackend::Cpu)],
            ComputeBackendPreference::GpuOnly => vec![(true, SelectedBackend::Gpu)],
            ComputeBackendPreference::PreferGpu => {
                vec![(true, SelectedBackend::Gpu), (false, SelectedBackend::Cpu)]
            }
        };

        let model_path = model_path.to_str().ok_or("Invalid model path")?;
        let mut last_error = None;

        for (use_gpu, backend) in attempts {
            let mut params = WhisperContextParameters::default();
            params.use_gpu(use_gpu);
            params.flash_attn(use_gpu && config.flash_attn);
            params.gpu_device(config.gpu_device);

            match WhisperContext::new_with_params(model_path, params) {
                Ok(ctx) => return Ok((ctx, backend)),
                Err(err) => {
                    last_error = Some(format!(
                        "Failed to load whisper model with {} backend on {}: {}",
                        match backend {
                            SelectedBackend::Cpu => "cpu",
                            SelectedBackend::Gpu => "gpu",
                        },
                        machine_profile.summary(),
                        err
                    ));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| "Failed to load whisper model".to_string()))
    }

    fn run_with_state(
        &self,
        state: &mut WhisperState,
        samples: &[f32],
        preset: DecodePreset,
    ) -> Result<(String, Duration, Duration), String> {
        let mut params = FullParams::new(match preset {
            DecodePreset::Fast => SamplingStrategy::Greedy { best_of: 1 },
            DecodePreset::Conservative => SamplingStrategy::BeamSearch {
                beam_size: 2,
                patience: 1.0,
            },
        });
        params.set_n_threads(self.config.n_threads);
        params.set_language(Some("en"));
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_no_timestamps(self.config.no_timestamps);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_no_context(matches!(preset, DecodePreset::Fast));
        params.set_single_segment(matches!(preset, DecodePreset::Fast));

        if let Some(audio_ctx) = self.selected_audio_ctx(samples, preset) {
            params.set_audio_ctx(audio_ctx);
        }

        let inference_t0 = Instant::now();
        state
            .full(params, samples)
            .map_err(|e| format!("Transcription failed: {}", e))?;
        let inference = inference_t0.elapsed();

        let extract_t0 = Instant::now();
        let num_segments = state
            .full_n_segments()
            .map_err(|e| format!("Failed to get segments: {}", e))?;

        let mut text = String::with_capacity(256);
        for i in 0..num_segments {
            if let Ok(segment) = state.full_get_segment_text(i) {
                text.push_str(&segment);
            }
        }

        Ok((text.trim().to_string(), inference, extract_t0.elapsed()))
    }

    fn selected_audio_ctx(&self, samples: &[f32], preset: DecodePreset) -> Option<i32> {
        if matches!(preset, DecodePreset::Conservative) {
            return None;
        }

        match self.config.audio_ctx {
            AudioContextStrategy::Adaptive => Some(self.recommended_audio_ctx(samples)),
            AudioContextStrategy::Fixed(audio_ctx) => Some(audio_ctx),
            AudioContextStrategy::ModelDefault => None,
        }
    }

    fn recommended_audio_ctx(&self, samples: &[f32]) -> i32 {
        let required = ceil_div(samples.len(), AUDIO_CTX_SAMPLES_PER_UNIT) as i32;
        round_up_to_multiple(
            required.max(self.config.adaptive_audio_ctx_min),
            AUDIO_CTX_GRANULARITY,
        )
        .min(self.ctx.n_audio_ctx())
    }
}

fn ceil_div(value: usize, divisor: usize) -> usize {
    value.div_ceil(divisor)
}

fn round_up_to_multiple(value: i32, multiple: i32) -> i32 {
    ((value + multiple - 1) / multiple) * multiple
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn transcript_looks_repetitive(text: &str) -> bool {
    let tokens = normalized_tokens(text);
    if tokens.len() < 8 {
        return false;
    }

    has_adjacent_repeated_window(&tokens, 3)
}

fn conservative_retry_reason(samples: &[f32], text: &str) -> Option<&'static str> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some("empty transcript");
    }
    if transcript_looks_repetitive(trimmed) {
        return Some("repetitive transcript");
    }
    if samples.len() >= 32_000 && normalized_tokens(trimmed).len() <= 1 {
        return Some("low-information transcript for long audio");
    }

    None
}

fn normalized_tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|token| {
            let normalized = token
                .trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '\'')
                .to_ascii_lowercase();

            if normalized.is_empty() {
                None
            } else {
                Some(normalized)
            }
        })
        .collect()
}

fn has_adjacent_repeated_window(tokens: &[String], min_window: usize) -> bool {
    for window in min_window..=tokens.len() / 2 {
        for start in 0..=tokens.len() - window * 2 {
            if tokens[start..start + window] == tokens[start + window..start + window * 2] {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repetition_detector_flags_repeated_phrase() {
        let text =
            "Okay, it also seems like there's a bug with the transcription. Okay, it also seems like there's a bug with the transcription.";

        assert!(transcript_looks_repetitive(text));
    }

    #[test]
    fn repetition_detector_flags_short_clause_repeat() {
        let text = "This is broken this is broken right now.";

        assert!(transcript_looks_repetitive(text));
    }

    #[test]
    fn repetition_detector_ignores_normal_sentence() {
        let text = "Okay, it also seems like there's a bug with the transcription.";

        assert!(!transcript_looks_repetitive(text));
    }

    #[test]
    fn repetition_detector_ignores_short_stutter() {
        let text = "I think I think this is okay.";

        assert!(!transcript_looks_repetitive(text));
    }

    #[test]
    fn conservative_retry_reason_flags_empty_output() {
        assert_eq!(
            conservative_retry_reason(&vec![0.02; 8_000], "   "),
            Some("empty transcript")
        );
    }

    #[test]
    fn conservative_retry_reason_flags_repetition() {
        let text = "This is broken this is broken right now.";

        assert_eq!(
            conservative_retry_reason(&vec![0.02; 8_000], text),
            Some("repetitive transcript")
        );
    }

    #[test]
    fn conservative_retry_reason_flags_low_information_long_clip() {
        assert_eq!(
            conservative_retry_reason(&vec![0.02; 32_000], "yes"),
            Some("low-information transcript for long audio")
        );
    }

    #[test]
    fn conservative_retry_reason_allows_short_commands() {
        assert_eq!(conservative_retry_reason(&vec![0.02; 8_000], "yes"), None);
    }
}

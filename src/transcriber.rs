use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

const AUDIO_CTX_SAMPLES_PER_UNIT: usize = 320;
const AUDIO_CTX_GRANULARITY: i32 = 64;
const MIN_AUDIO_CTX: i32 = 256;

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
    pub reuse_state: bool,
}

impl Default for TranscriberConfig {
    fn default() -> Self {
        Self {
            n_threads: recommended_n_threads(),
            no_timestamps: true,
            audio_ctx: AudioContextStrategy::Adaptive,
            reuse_state: true,
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

pub struct Transcriber {
    ctx: WhisperContext,
    final_state: Option<Mutex<WhisperState>>,
    live_state: Option<Mutex<WhisperState>>,
    config: TranscriberConfig,
}

impl Transcriber {
    pub fn new(model_path: &Path) -> Result<Self, String> {
        Self::with_config(model_path, TranscriberConfig::default())
    }

    pub fn with_config(model_path: &Path, config: TranscriberConfig) -> Result<Self, String> {
        let mut params = WhisperContextParameters::default();
        params.flash_attn(true);

        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or("Invalid model path")?,
            params,
        )
        .map_err(|e| format!("Failed to load whisper model: {}", e))?;

        let (final_state, live_state) = if config.reuse_state {
            let final_state = ctx
                .create_state()
                .map_err(|e| format!("Failed to create whisper state: {}", e))?;
            let live_state = ctx
                .create_state()
                .map_err(|e| format!("Failed to create whisper state: {}", e))?;
            (
                Some(Mutex::new(final_state)),
                Some(Mutex::new(live_state)),
            )
        } else {
            (None, None)
        };

        Ok(Self {
            ctx,
            final_state,
            live_state,
            config,
        })
    }

    #[allow(dead_code)]
    pub fn transcribe(&self, samples: &[f32]) -> Result<String, String> {
        self.transcribe_profiled(samples).map(|result| result.text)
    }

    pub fn transcribe_profiled(&self, samples: &[f32]) -> Result<TranscriptionOutput, String> {
        let total_t0 = Instant::now();
        let state_t0 = Instant::now();
        let mut state = self.acquire_final_state()?;
        let state_acquire = state_t0.elapsed();

        let (text, inference, extract) = self.run_with_state(state.as_mut(), samples)?;

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

        let (text, _, _) = self.run_with_state(state.as_mut(), samples)?;
        Ok(Some(text))
    }

    /// Find the model file, checking bundle Resources first, then local models/ dir
    pub fn find_model(model_name: &str) -> Option<PathBuf> {
        let candidates = [
            format!("ggml-{}.en.bin", model_name),
            format!("ggml-{}.bin", model_name),
            format!("ggml-{}-v3.bin", model_name),
        ];

        // Cache exe parent path once
        let bundle_models_dir = std::env::current_exe().ok().and_then(|exe| {
            exe.parent()
                .and_then(|p| p.parent())
                .map(|p| p.join("Resources").join("models"))
        });

        for filename in &candidates {
            if let Some(ref dir) = bundle_models_dir {
                let path = dir.join(filename);
                if path.exists() {
                    return Some(path);
                }
            }

            let local = PathBuf::from("models").join(filename);
            if local.exists() {
                return Some(local);
            }
        }

        None
    }

    fn acquire_final_state(&self) -> Result<StateAccess<'_>, String> {
        if let Some(state) = &self.final_state {
            Ok(StateAccess::Borrowed(
                state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()),
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

    fn run_with_state(
        &self,
        state: &mut WhisperState,
        samples: &[f32],
    ) -> Result<(String, Duration, Duration), String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.config.n_threads);
        params.set_language(Some("en"));
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_no_timestamps(self.config.no_timestamps);
        params.set_suppress_blank(true);
        params.set_no_context(true);
        params.set_single_segment(true);

        if let Some(audio_ctx) = self.selected_audio_ctx(samples) {
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

    fn selected_audio_ctx(&self, samples: &[f32]) -> Option<i32> {
        match self.config.audio_ctx {
            AudioContextStrategy::Adaptive => Some(self.recommended_audio_ctx(samples)),
            AudioContextStrategy::Fixed(audio_ctx) => Some(audio_ctx),
            AudioContextStrategy::ModelDefault => None,
        }
    }

    fn recommended_audio_ctx(&self, samples: &[f32]) -> i32 {
        let required = ceil_div(samples.len(), AUDIO_CTX_SAMPLES_PER_UNIT) as i32;
        round_up_to_multiple(required.max(MIN_AUDIO_CTX), AUDIO_CTX_GRANULARITY)
            .min(self.ctx.n_audio_ctx())
    }
}

fn recommended_n_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().min(4) as i32)
        .unwrap_or(4)
}

fn ceil_div(value: usize, divisor: usize) -> usize {
    value.div_ceil(divisor)
}

fn round_up_to_multiple(value: i32, multiple: i32) -> i32 {
    ((value + multiple - 1) / multiple) * multiple
}

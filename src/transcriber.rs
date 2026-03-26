use std::path::{Path, PathBuf};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct Transcriber {
    ctx: WhisperContext,
    n_threads: i32,
}

impl Transcriber {
    pub fn new(model_path: &Path) -> Result<Self, String> {
        let mut params = WhisperContextParameters::default();
        params.flash_attn(true);

        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or("Invalid model path")?,
            params,
        )
        .map_err(|e| format!("Failed to load whisper model: {}", e))?;

        let n_threads = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4);

        Ok(Self { ctx, n_threads })
    }

    pub fn transcribe(&self, samples: &[f32]) -> Result<String, String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.n_threads);
        params.set_language(Some("en"));
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_no_context(true);
        params.set_single_segment(true);

        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| format!("Failed to create whisper state: {}", e))?;

        state
            .full(params, samples)
            .map_err(|e| format!("Transcription failed: {}", e))?;

        let num_segments = state
            .full_n_segments()
            .map_err(|e| format!("Failed to get segments: {}", e))?;

        let mut text = String::with_capacity(256);
        for i in 0..num_segments {
            if let Ok(segment) = state.full_get_segment_text(i) {
                text.push_str(&segment);
            }
        }

        Ok(text.trim().to_string())
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
}

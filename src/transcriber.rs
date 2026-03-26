use std::path::{Path, PathBuf};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct Transcriber {
    ctx: WhisperContext,
}

impl Transcriber {
    pub fn new(model_path: &Path) -> Result<Self, String> {
        let mut params = WhisperContextParameters::default();
        // use_gpu is already true when metal feature is enabled
        params.flash_attn(true); // faster attention computation on GPU

        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or("Invalid model path")?,
            params,
        )
        .map_err(|e| format!("Failed to load whisper model: {}", e))?;

        Ok(Self { ctx })
    }

    pub fn transcribe(&self, samples: &[f32]) -> Result<String, String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        let n_threads = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4);
        params.set_n_threads(n_threads);
        params.set_language(Some("en"));
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_no_context(true); // don't use prior context, faster for independent utterances
        params.set_single_segment(true); // treat as single segment, skip segmentation overhead

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

        let mut text = String::new();
        for i in 0..num_segments {
            if let Ok(segment) = state.full_get_segment_text(i) {
                text.push_str(&segment);
            }
        }

        let result = text.trim().to_string();
        Ok(result)
    }

    /// Find the model file, checking bundle Resources first, then local models/ dir
    pub fn find_model(model_name: &str) -> Option<PathBuf> {
        // Try multiple filename patterns (some models don't have .en variant)
        let candidates = vec![
            format!("ggml-{}.en.bin", model_name),
            format!("ggml-{}.bin", model_name),
            format!("ggml-{}-v3.bin", model_name),
        ];

        for filename in &candidates {
            // Check inside .app bundle: ../Resources/models/
            if let Ok(exe) = std::env::current_exe() {
                let bundle_models = exe
                    .parent() // MacOS/
                    .and_then(|p| p.parent()) // Contents/
                    .map(|p| p.join("Resources").join("models").join(filename));
                if let Some(path) = bundle_models {
                    if path.exists() {
                        return Some(path);
                    }
                }
            }

            // Check local models/ directory (for development)
            let local = PathBuf::from("models").join(filename);
            if local.exists() {
                return Some(local);
            }
        }

        None
    }
}

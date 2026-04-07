use std::path::{Path, PathBuf};

pub const DEFAULT_BUNDLED_SUMMARY_MODEL_ID: &str = "gemma-3-1b-it-q4_k_m";
pub const DEFAULT_BUNDLED_SUMMARY_MODEL_FILENAME: &str = "gemma-3-1b-it-q4_k_m.gguf";

pub const VISION_MODEL_FILENAME: &str = "gemma-3-4b-it-q4.gguf";
pub const VISION_MMPROJ_FILENAME: &str = "mmproj-gemma-3-4b-it-f16.gguf";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub filename: &'static str,
}

pub const BUNDLED_SUMMARY_MODELS: &[SummaryModelInfo] = &[SummaryModelInfo {
    id: DEFAULT_BUNDLED_SUMMARY_MODEL_ID,
    label: "Gemma 3 1B Instruct",
    filename: DEFAULT_BUNDLED_SUMMARY_MODEL_FILENAME,
}];

pub fn bundled_model_candidates(model_name: &str) -> [String; 3] {
    [
        format!("ggml-{}.en.bin", model_name),
        format!("ggml-{}.bin", model_name),
        format!("ggml-{}-v3.bin", model_name),
    ]
}

pub fn find_model(model_name: &str) -> Option<PathBuf> {
    let candidates = bundled_model_candidates(model_name);
    let bundle_models_dir = current_bundle_models_dir();

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

pub fn bundled_summary_model() -> Option<&'static SummaryModelInfo> {
    BUNDLED_SUMMARY_MODELS.first()
}

pub fn find_summary_model(model_id: &str) -> Option<PathBuf> {
    let info = BUNDLED_SUMMARY_MODELS
        .iter()
        .find(|info| info.id == model_id)?;
    let bundle_models_dir = current_bundle_summary_models_dir();
    let local_models_dir = PathBuf::from("models").join("summary");

    resolve_existing_path(bundle_models_dir.as_deref(), info.filename)
        .or_else(|| resolve_existing_path(Some(&local_models_dir), info.filename))
}

pub fn summary_model_exists(model_id: &str) -> bool {
    find_summary_model(model_id).is_some()
}

pub fn current_bundle_models_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|exe| {
        exe.parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("Resources").join("models"))
    })
}

pub fn current_bundle_summary_models_dir() -> Option<PathBuf> {
    current_bundle_models_dir().map(|dir| dir.join("summary"))
}

pub fn find_vision_model() -> Option<(PathBuf, PathBuf)> {
    let bundle_dir = current_bundle_summary_models_dir();
    let local_dir = PathBuf::from("models").join("summary");

    let model_path = resolve_existing_path(bundle_dir.as_deref(), VISION_MODEL_FILENAME)
        .or_else(|| resolve_existing_path(Some(&local_dir), VISION_MODEL_FILENAME))?;
    let mmproj_path = resolve_existing_path(bundle_dir.as_deref(), VISION_MMPROJ_FILENAME)
        .or_else(|| resolve_existing_path(Some(&local_dir), VISION_MMPROJ_FILENAME))?;

    Some((model_path, mmproj_path))
}

fn resolve_existing_path(base: Option<&Path>, filename: &str) -> Option<PathBuf> {
    let base = base?;
    let path = base.join(filename);
    path.exists().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_candidates_cover_current_bundle_conventions() {
        let candidates = bundled_model_candidates("base");

        assert_eq!(candidates[0], "ggml-base.en.bin");
        assert_eq!(candidates[1], "ggml-base.bin");
        assert_eq!(candidates[2], "ggml-base-v3.bin");
    }

    #[test]
    fn bundled_summary_model_metadata_is_present() {
        let model = bundled_summary_model().unwrap();
        assert_eq!(model.id, DEFAULT_BUNDLED_SUMMARY_MODEL_ID);
        assert_eq!(model.filename, DEFAULT_BUNDLED_SUMMARY_MODEL_FILENAME);
    }
}

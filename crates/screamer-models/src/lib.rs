use std::path::PathBuf;

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

fn current_bundle_models_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|exe| {
        exe.parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("Resources").join("models"))
    })
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
}

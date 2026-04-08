use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

pub const ASSET_MANIFEST_NAME: &str = "manifest.json";
pub const BUILTIN_ASSET_VERSION: &str = "builtin-mfcc-v1";
pub const DEFAULT_ASSET_ROOT_SUFFIX: &[&str] = &["Screamer", "models", "ambient-diarization"];
pub const AMBIENT_DIARIZATION_DIR_ENV: &str = "SCREAMER_AMBIENT_DIARIZATION_DIR";

fn default_sample_rate_hz() -> usize {
    16_000
}

fn default_segmentation_window_ms() -> u64 {
    5_000
}

fn default_segmentation_hop_ms() -> u64 {
    2_500
}

fn default_frame_hop_ms() -> u64 {
    20
}

fn default_activation_threshold() -> f32 {
    0.4
}

fn default_min_speech_ms() -> u64 {
    200
}

fn default_min_silence_ms() -> u64 {
    160
}

fn default_clustering_similarity_threshold() -> f32 {
    0.90
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbientModelInputLayout {
    #[default]
    BatchSamples,
    BatchChannelSamples,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbientModelOutputLayout {
    FramesSpeakers,
    BatchFramesSpeakers,
    BatchSpeakersFrames,
    EmbeddingVector,
    BatchEmbeddingVector,
}

impl Default for AmbientModelOutputLayout {
    fn default() -> Self {
        Self::BatchFramesSpeakers
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmbientDiarizationAssetFile {
    pub relative_path: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AmbientDiarizationModelSpec {
    pub relative_path: String,
    #[serde(default)]
    pub input_name: Option<String>,
    #[serde(default)]
    pub output_name: Option<String>,
    #[serde(default = "default_sample_rate_hz")]
    pub sample_rate_hz: usize,
    #[serde(default)]
    pub input_layout: AmbientModelInputLayout,
    #[serde(default)]
    pub output_layout: AmbientModelOutputLayout,
    #[serde(default)]
    pub target_samples: Option<usize>,
    #[serde(default)]
    pub model_cache_subdir: Option<String>,
    #[serde(default = "default_segmentation_window_ms")]
    pub window_ms: u64,
    #[serde(default = "default_segmentation_hop_ms")]
    pub hop_ms: u64,
    #[serde(default = "default_frame_hop_ms")]
    pub frame_hop_ms: u64,
    #[serde(default = "default_activation_threshold")]
    pub activation_threshold: f32,
    #[serde(default = "default_min_speech_ms")]
    pub min_speech_ms: u64,
    #[serde(default = "default_min_silence_ms")]
    pub min_silence_ms: u64,
}

impl AmbientDiarizationModelSpec {
    pub fn resolved_path(&self, root_dir: &Path) -> PathBuf {
        root_dir.join(&self.relative_path)
    }

    fn validate(&self, root_dir: &Path, label: &str) -> Result<(), String> {
        if self.relative_path.trim().is_empty() {
            return Err(format!(
                "Ambient diarization {label} model is missing `relative_path`."
            ));
        }
        let model_path = self.resolved_path(root_dir);
        if !model_path.is_file() {
            return Err(format!(
                "Ambient diarization {label} model is missing: {}",
                model_path.display()
            ));
        }
        if self.sample_rate_hz == 0 {
            return Err(format!(
                "Ambient diarization {label} model has an invalid `sample_rate_hz`."
            ));
        }
        if self.window_ms == 0 {
            return Err(format!(
                "Ambient diarization {label} model has an invalid `window_ms`."
            ));
        }
        if self.hop_ms == 0 {
            return Err(format!(
                "Ambient diarization {label} model has an invalid `hop_ms`."
            ));
        }
        if self.frame_hop_ms == 0 {
            return Err(format!(
                "Ambient diarization {label} model has an invalid `frame_hop_ms`."
            ));
        }
        if !(0.0..=1.0).contains(&self.activation_threshold) {
            return Err(format!(
                "Ambient diarization {label} model has an invalid `activation_threshold`."
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AmbientDiarizationPipelineManifest {
    pub segmentation: AmbientDiarizationModelSpec,
    pub embedding: AmbientDiarizationModelSpec,
    #[serde(default = "default_clustering_similarity_threshold")]
    pub clustering_similarity_threshold: f32,
}

impl AmbientDiarizationPipelineManifest {
    fn validate(&self, root_dir: &Path) -> Result<(), String> {
        self.segmentation.validate(root_dir, "segmentation")?;
        self.embedding.validate(root_dir, "embedding")?;
        if !(0.0..=1.0).contains(&self.clustering_similarity_threshold)
            || self.clustering_similarity_threshold <= 0.0
        {
            return Err(
                "Ambient diarization pipeline has an invalid `clustering_similarity_threshold`."
                    .to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AmbientDiarizationAssetManifest {
    pub format_version: u32,
    pub asset_version: String,
    #[serde(default)]
    pub backend_kind: String,
    #[serde(default)]
    pub files: Vec<AmbientDiarizationAssetFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<AmbientDiarizationPipelineManifest>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AmbientDiarizationAssetSet {
    pub root_dir: PathBuf,
    pub manifest: AmbientDiarizationAssetManifest,
}

impl AmbientDiarizationAssetManifest {
    pub fn validate(&self, root_dir: &Path) -> Result<(), String> {
        if self.format_version == 0 {
            return Err("Ambient diarization asset manifest is missing `format_version`.".to_string());
        }
        if self.asset_version.trim().is_empty() {
            return Err("Ambient diarization asset manifest is missing `asset_version`.".to_string());
        }

        for file in &self.files {
            let path = root_dir.join(&file.relative_path);
            if !path.is_file() {
                if file.required {
                    return Err(format!(
                        "Ambient diarization asset file is missing: {}",
                        path.display()
                    ));
                }
                continue;
            }

            if let Some(expected_sha) = &file.sha256 {
                let actual_sha = sha256_file(&path)?;
                if actual_sha != expected_sha.to_ascii_lowercase() {
                    return Err(format!(
                        "Ambient diarization asset checksum mismatch for {}",
                        path.display()
                    ));
                }
            }
        }

        if let Some(pipeline) = &self.pipeline {
            pipeline.validate(root_dir)?;
        }

        Ok(())
    }
}

impl AmbientDiarizationAssetSet {
    pub fn discover() -> Result<Option<Self>, String> {
        if let Some(explicit) = std::env::var_os(AMBIENT_DIARIZATION_DIR_ENV) {
            let explicit = PathBuf::from(explicit);
            return resolve_root(&explicit);
        }

        let root = default_asset_root();
        resolve_root(&root)
    }

    pub fn resolve_relative_path(&self, relative_path: &str) -> PathBuf {
        self.root_dir.join(relative_path)
    }

    pub fn model_cache_dir(&self, subdir: Option<&str>, fallback_name: &str) -> PathBuf {
        self.root_dir.join(
            subdir
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(fallback_name),
        )
    }
}

pub fn default_asset_root() -> PathBuf {
    let mut root = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    for component in DEFAULT_ASSET_ROOT_SUFFIX {
        root.push(component);
    }
    root
}

pub fn write_manifest(
    target_dir: &Path,
    manifest: &AmbientDiarizationAssetManifest,
) -> Result<PathBuf, String> {
    fs::create_dir_all(target_dir).map_err(|err| {
        format!(
            "Failed to create ambient diarization asset directory {}: {err}",
            target_dir.display()
        )
    })?;
    let path = target_dir.join(ASSET_MANIFEST_NAME);
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|err| format!("Failed to serialize ambient diarization asset manifest: {err}"))?;
    fs::write(&path, bytes)
        .map_err(|err| format!("Failed to write ambient diarization manifest {}: {err}", path.display()))?;
    Ok(path)
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path)
        .map_err(|err| format!("Failed to read {} for checksum: {err}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hex::encode(hasher.finalize()))
}

fn load_from_root(root_dir: &Path) -> Result<AmbientDiarizationAssetSet, String> {
    let manifest_path = root_dir.join(ASSET_MANIFEST_NAME);
    if !manifest_path.is_file() {
        return Err(format!(
            "Ambient diarization manifest not found at {}.",
            manifest_path.display()
        ));
    }

    let manifest: AmbientDiarizationAssetManifest = serde_json::from_slice(
        &fs::read(&manifest_path)
            .map_err(|err| format!("Failed to read ambient diarization manifest {}: {err}", manifest_path.display()))?,
    )
    .map_err(|err| format!("Failed to parse ambient diarization manifest {}: {err}", manifest_path.display()))?;
    manifest.validate(root_dir)?;

    Ok(AmbientDiarizationAssetSet {
        root_dir: root_dir.to_path_buf(),
        manifest,
    })
}

fn resolve_root(root: &Path) -> Result<Option<AmbientDiarizationAssetSet>, String> {
    if !root.exists() {
        return Ok(None);
    }

    if root.join(ASSET_MANIFEST_NAME).is_file() {
        return Ok(Some(load_from_root(root)?));
    }

    let mut version_dirs = fs::read_dir(root)
        .map_err(|err| format!("Failed to list ambient diarization assets in {}: {err}", root.display()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .filter(|entry| entry.path().join(ASSET_MANIFEST_NAME).is_file())
        .collect::<Vec<_>>();

    version_dirs.sort_by(|left, right| left.file_name().cmp(&right.file_name()));

    if let Some(entry) = version_dirs.pop() {
        Ok(Some(load_from_root(&entry.path())?))
    } else {
        Ok(None)
    }
}

pub fn path_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn manifest_validation_checks_required_files() {
        let dir = tempdir().unwrap();
        let manifest = AmbientDiarizationAssetManifest {
            format_version: 1,
            asset_version: "test-v1".to_string(),
            backend_kind: "onnx".to_string(),
            files: vec![AmbientDiarizationAssetFile {
                relative_path: "segmentation.onnx".to_string(),
                sha256: None,
                required: true,
            }],
            pipeline: None,
        };

        let err = manifest.validate(dir.path()).unwrap_err();
        assert!(err.contains("segmentation.onnx"));
    }

    #[test]
    fn manifest_validation_checks_sha256() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("embedding.onnx");
        fs::write(&path, b"hello").unwrap();
        let manifest = AmbientDiarizationAssetManifest {
            format_version: 1,
            asset_version: "test-v1".to_string(),
            backend_kind: "onnx".to_string(),
            files: vec![AmbientDiarizationAssetFile {
                relative_path: "embedding.onnx".to_string(),
                sha256: Some(sha256_file(&path).unwrap()),
                required: true,
            }],
            pipeline: None,
        };

        manifest.validate(dir.path()).unwrap();
    }

    #[test]
    fn pipeline_validation_checks_model_paths() {
        let dir = tempdir().unwrap();
        let manifest = AmbientDiarizationAssetManifest {
            format_version: 1,
            asset_version: "test-v2".to_string(),
            backend_kind: "onnx_runtime_v1".to_string(),
            files: Vec::new(),
            pipeline: Some(AmbientDiarizationPipelineManifest {
                segmentation: AmbientDiarizationModelSpec {
                    relative_path: "segmentation.onnx".to_string(),
                    input_name: None,
                    output_name: None,
                    sample_rate_hz: 16_000,
                    input_layout: AmbientModelInputLayout::BatchSamples,
                    output_layout: AmbientModelOutputLayout::BatchFramesSpeakers,
                    target_samples: None,
                    model_cache_subdir: None,
                    window_ms: 5_000,
                    hop_ms: 2_500,
                    frame_hop_ms: 20,
                    activation_threshold: 0.4,
                    min_speech_ms: 200,
                    min_silence_ms: 160,
                },
                embedding: AmbientDiarizationModelSpec {
                    relative_path: "embedding.onnx".to_string(),
                    input_name: None,
                    output_name: None,
                    sample_rate_hz: 16_000,
                    input_layout: AmbientModelInputLayout::BatchSamples,
                    output_layout: AmbientModelOutputLayout::BatchEmbeddingVector,
                    target_samples: None,
                    model_cache_subdir: None,
                    window_ms: 3_000,
                    hop_ms: 3_000,
                    frame_hop_ms: 20,
                    activation_threshold: 0.4,
                    min_speech_ms: 200,
                    min_silence_ms: 160,
                },
                clustering_similarity_threshold: 0.9,
            }),
        };

        let err = manifest.validate(dir.path()).unwrap_err();
        assert!(err.contains("segmentation model is missing"));
    }

    #[test]
    fn discover_picks_latest_version_directory() {
        let dir = tempdir().unwrap();
        let older = dir.path().join("2026-01-01");
        let newer = dir.path().join("2026-02-01");
        let manifest = AmbientDiarizationAssetManifest {
            format_version: 1,
            asset_version: "v1".to_string(),
            backend_kind: "builtin".to_string(),
            files: Vec::new(),
            pipeline: None,
        };
        write_manifest(&older, &manifest).unwrap();
        write_manifest(
            &newer,
            &AmbientDiarizationAssetManifest {
                asset_version: "v2".to_string(),
                ..manifest.clone()
            },
        )
        .unwrap();

        std::env::set_var(AMBIENT_DIARIZATION_DIR_ENV, dir.path());
        let asset_set = AmbientDiarizationAssetSet::discover().unwrap().unwrap();
        std::env::remove_var(AMBIENT_DIARIZATION_DIR_ENV);

        assert_eq!(path_file_name(&asset_set.root_dir), "2026-02-01");
        assert_eq!(asset_set.manifest.asset_version, "v2");
    }
}

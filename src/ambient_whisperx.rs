use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const HELPER_SCRIPT_NAME: &str = "ambient_whisperx_helper.py";
const TARGET_SAMPLE_RATE_HZ: u32 = 16_000;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhisperxHelperMode {
    WhisperxHybrid,
    PyannoteReassign,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhisperxHelperInputSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhisperxHelperRequest {
    pub mode: WhisperxHelperMode,
    pub audio_path: PathBuf,
    pub model: String,
    pub device: String,
    pub compute_type: String,
    pub language: String,
    #[serde(default)]
    pub segments: Vec<WhisperxHelperInputSegment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WhisperxHelperWord {
    pub start_ms: Option<u64>,
    pub end_ms: Option<u64>,
    pub speaker: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WhisperxHelperSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: Option<String>,
    pub text: String,
    #[serde(default)]
    pub words: Vec<WhisperxHelperWord>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WhisperxHelperDiagnostics {
    #[serde(default)]
    pub detected_speakers: usize,
    #[serde(default)]
    pub transcription_ms: u64,
    #[serde(default)]
    pub alignment_ms: u64,
    #[serde(default)]
    pub diarization_ms: u64,
    #[serde(default)]
    pub assignment_ms: u64,
    #[serde(default)]
    pub total_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WhisperxHelperResponse {
    pub engine: String,
    pub transcript_text: String,
    #[serde(default)]
    pub segments: Vec<WhisperxHelperSegment>,
    #[serde(default)]
    pub diagnostics: WhisperxHelperDiagnostics,
}

pub fn whisperx_model_for_screamer_model(model: &str) -> &'static str {
    match model {
        "tiny" => "tiny.en",
        "small" => "small.en",
        _ => "base.en",
    }
}

pub fn write_temp_wav(prefix: &str, samples: &[f32]) -> Result<PathBuf, String> {
    let temp_id = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "screamer-{prefix}-{}-{timestamp}-{temp_id}.wav",
        std::process::id()
    ));

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE_HZ,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec)
        .map_err(|err| format!("Failed to create temporary WAV at {}: {err}", path.display()))?;

    for sample in samples {
        let scaled = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        writer
            .write_sample(scaled)
            .map_err(|err| format!("Failed to write temporary WAV at {}: {err}", path.display()))?;
    }

    writer
        .finalize()
        .map_err(|err| format!("Failed to finalize temporary WAV at {}: {err}", path.display()))?;
    Ok(path)
}

pub fn run_whisperx_helper(
    request: &WhisperxHelperRequest,
) -> Result<WhisperxHelperResponse, String> {
    let helper_path = helper_script_path().ok_or_else(|| {
        format!(
            "WhisperX helper script not found. Expected {} in the app bundle or repository scripts directory.",
            HELPER_SCRIPT_NAME
        )
    })?;
    let payload = serde_json::to_vec(request)
        .map_err(|err| format!("Failed to serialize WhisperX helper request: {err}"))?;

    let mut last_spawn_error = None;

    for python in python_candidates() {
        let mut child = match Command::new(&python)
            .arg(&helper_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("PYANNOTE_METRICS_ENABLED", "0")
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("TOKENIZERS_PARALLELISM", "false")
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                last_spawn_error = Some(format!(
                    "Failed to launch `{}` with helper {}: {err}",
                    Path::new(&python).display(),
                    helper_path.display()
                ));
                continue;
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(&payload)
                .map_err(|err| format!("Failed to send request to WhisperX helper: {err}"))?;
        }

        let output = child
            .wait_with_output()
            .map_err(|err| format!("Failed to wait for WhisperX helper: {err}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                format!(
                    "WhisperX helper exited with status {}",
                    output
                        .status
                        .code()
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                )
            } else {
                stderr
            });
        }

        return serde_json::from_slice::<WhisperxHelperResponse>(&output.stdout)
            .map_err(|err| format!("Failed to parse WhisperX helper response: {err}"));
    }

    Err(last_spawn_error.unwrap_or_else(|| {
        "Unable to find a usable Python runtime. Set SCREAMER_AMBIENT_PYTHON or install python3."
            .to_string()
    }))
}

fn python_candidates() -> Vec<OsString> {
    let mut candidates = Vec::new();
    if let Some(explicit) = std::env::var_os("SCREAMER_AMBIENT_PYTHON") {
        candidates.push(explicit);
    }
    candidates.push(OsString::from("python3"));
    candidates.push(OsString::from("python"));
    candidates
}

fn helper_script_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("SCREAMER_WHISPERX_HELPER") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }

    let exe = std::env::current_exe().ok()?;
    let mut candidates = Vec::new();

    if let Some(parent) = exe.parent() {
        candidates.push(parent.join(HELPER_SCRIPT_NAME));

        if parent.ends_with("deps") {
            if let Some(build_dir) = parent.parent() {
                candidates.push(build_dir.join(HELPER_SCRIPT_NAME));
                if let Some(repo_root) = build_dir.parent().and_then(|target_dir| target_dir.parent())
                {
                    candidates.push(repo_root.join("scripts").join(HELPER_SCRIPT_NAME));
                }
            }
        } else if let Some(repo_root) = parent.parent().and_then(|target_dir| target_dir.parent())
        {
            candidates.push(repo_root.join("scripts").join(HELPER_SCRIPT_NAME));
        }

        if parent.ends_with("MacOS") {
            if let Some(contents) = parent.parent() {
                candidates.push(contents.join("Resources").join(HELPER_SCRIPT_NAME));
                candidates.push(contents.join("Resources").join("scripts").join(HELPER_SCRIPT_NAME));
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("scripts").join(HELPER_SCRIPT_NAME));
    }

    candidates.into_iter().find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::whisperx_model_for_screamer_model;

    #[test]
    fn maps_screamer_models_to_whisperx_models() {
        assert_eq!(whisperx_model_for_screamer_model("tiny"), "tiny.en");
        assert_eq!(whisperx_model_for_screamer_model("base"), "base.en");
        assert_eq!(whisperx_model_for_screamer_model("small"), "small.en");
        assert_eq!(whisperx_model_for_screamer_model("unknown"), "base.en");
    }
}

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const HELPER_BINARY_NAME: &str = "screamer_vision_helper";

/// Run a vision query: pass the user's transcribed question + screenshot to the
/// multimodal Gemma 3 4B model and return the response.
pub fn ask_about_screen(prompt: &str, screenshot_path: &Path) -> Result<String, String> {
    let helper_path = find_helper_path()
        .ok_or_else(|| format!("Vision helper not found: {HELPER_BINARY_NAME}"))?;

    let screenshot_str = screenshot_path
        .to_str()
        .ok_or("Screenshot path contains invalid UTF-8")?;

    let mut child = Command::new(&helper_path)
        .arg("--image")
        .arg(screenshot_str)
        .arg("--max-tokens")
        .arg("512")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            format!(
                "Failed to launch vision helper at {}: {err}",
                helper_path.display()
            )
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|err| format!("Failed to send prompt to vision helper: {err}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("Failed to wait for vision helper: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!(
                "Vision helper exited with status {}",
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

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn find_helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut candidates = Vec::new();

    if let Some(parent) = exe.parent() {
        candidates.push(parent.join(HELPER_BINARY_NAME));
        if parent.ends_with("deps") {
            if let Some(debug_or_release_dir) = parent.parent() {
                candidates.push(debug_or_release_dir.join(HELPER_BINARY_NAME));
            }
        }
    }

    // Check inside .app bundle
    if let Some(parent) = exe.parent() {
        if parent.file_name().map(|n| n == "MacOS").unwrap_or(false) {
            candidates.push(parent.join(HELPER_BINARY_NAME));
        }
    }

    candidates.into_iter().find(|candidate| candidate.is_file())
}

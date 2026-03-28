use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::IntoRawFd;
use std::path::PathBuf;
use std::sync::OnceLock;

const DEFAULT_LOG_DIR: &str = "Library/Logs/Screamer";
const DEFAULT_LOG_FILE: &str = "screamer.log";
const LOG_FILE_ENV: &str = "SCREAMER_LOG_FILE";
const TRANSCRIPT_LOG_ENV: &str = "SCREAMER_LOG_TRANSCRIPTS";

pub fn init_stderr_log() {
    let Some(path) = log_path() else {
        return;
    };

    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return;
        }
    }

    let Ok(file) = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
    else {
        return;
    };

    let _ = fs::set_permissions(&path, Permissions::from_mode(0o600));

    let fd = file.into_raw_fd();
    unsafe {
        libc::dup2(fd, 2); // redirect stderr
        libc::close(fd);
    }
}

pub fn log_transcript(label: &str, text: &str) {
    if transcript_logging_enabled() {
        eprintln!("[screamer] {label}: {text}");
    }
}

fn transcript_logging_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_flag(TRANSCRIPT_LOG_ENV))
}

fn log_path() -> Option<PathBuf> {
    if let Ok(value) = std::env::var(LOG_FILE_ENV) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }

    dirs::home_dir().map(|home| home.join(DEFAULT_LOG_DIR).join(DEFAULT_LOG_FILE))
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

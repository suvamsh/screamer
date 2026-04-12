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
        .append(true)
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

pub fn active_log_path() -> Option<PathBuf> {
    log_path()
}

pub fn log_transcript(label: &str, text: &str) {
    if transcript_logging_enabled() {
        eprintln!("[screamer] {label}: {text}");
    }
}

pub fn log_flow_event(flow: &str, stage: &str, message: &str) {
    eprintln!("[screamer][{flow}][{stage}] {message}");
}

pub fn log_flow_block(flow: &str, stage: &str, text: &str) {
    eprintln!("[screamer][{flow}][{stage}] BEGIN");
    write_block(text);
    eprintln!("[screamer][{flow}][{stage}] END");
}

pub fn log_ambient_session_report(
    source: &str,
    session_id: i64,
    title: &str,
    state: &str,
    started_at_ms: i64,
    ended_at_ms: i64,
    summary_backend: &str,
    summary_template: &str,
    warning: Option<&str>,
    transcript_markdown: &str,
    structured_notes: &str,
) {
    eprintln!("[screamer][ambient-report] BEGIN");
    eprintln!("source: {source}");
    eprintln!("session_id: {session_id}");
    eprintln!("state: {state}");
    eprintln!("title: {}", sanitize_log_header_value(title));
    eprintln!("started_at_ms: {started_at_ms}");
    eprintln!("ended_at_ms: {ended_at_ms}");
    eprintln!("duration_ms: {}", ended_at_ms.saturating_sub(started_at_ms));
    eprintln!(
        "summary_backend: {}",
        sanitize_log_header_value(summary_backend)
    );
    eprintln!(
        "summary_template: {}",
        sanitize_log_header_value(summary_template)
    );
    eprintln!(
        "warning: {}",
        warning
            .map(sanitize_log_header_value)
            .unwrap_or_else(|| "none".to_string())
    );
    eprintln!("transcript_chars: {}", transcript_markdown.chars().count());
    eprintln!("summary_chars: {}", structured_notes.chars().count());
    eprintln!("transcript:");
    write_block(transcript_markdown);
    eprintln!("summary:");
    write_block(structured_notes);
    eprintln!("[screamer][ambient-report] END");
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

fn sanitize_log_header_value(value: &str) -> String {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn write_block(text: &str) {
    if text.trim().is_empty() {
        eprintln!("  (empty)");
        return;
    }

    for line in text.lines() {
        eprintln!("  {line}");
    }
}

use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::IntoRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_LOG_DIR: &str = "Library/Logs/Screamer";
const DEFAULT_LOG_FILE: &str = "screamer.log";
const LOG_FILE_ENV: &str = "SCREAMER_LOG_FILE";
const TRANSCRIPT_LOG_ENV: &str = "SCREAMER_LOG_TRANSCRIPTS";
const SAVE_VISION_SCREENSHOTS_ENV: &str = "SCREAMER_SAVE_VISION_SCREENSHOTS";
const VISION_VERBOSE_ENV: &str = "SCREAMER_VISION_VERBOSE";

static VISION_DEBUG_SEQ: AtomicU64 = AtomicU64::new(0);

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

/// When `SCREAMER_VISION_VERBOSE` is set, vision emits detailed traces (multiline blocks, pipeline
/// events, and extra `eprintln` lines). Off by default for lighter `screamer.log` / stderr.
pub fn vision_verbose_detailed() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_flag(VISION_VERBOSE_ENV))
}

/// Multiline vision debug (prompts, raw model text). Off by default to reduce log I/O and noise.
pub fn log_vision_block(stage: &str, text: &str) {
    if vision_verbose_detailed() {
        log_flow_block("vision", stage, text);
    }
}

/// Single-line `[vision][stage]` events; skipped unless [`vision_verbose_detailed`] is on.
pub fn log_vision_event(stage: &str, message: &str) {
    if vision_verbose_detailed() {
        log_flow_event("vision", stage, message);
    }
}

pub fn eprint_vision_verbose_line(message: &str) {
    if vision_verbose_detailed() {
        eprintln!("{message}");
    }
}

/// Arrow overlay geometry; same gate as vision verbose (`SCREAMER_VISION_VERBOSE`).
pub fn log_highlight_event(stage: &str, message: &str) {
    if vision_verbose_detailed() {
        log_flow_event("highlight", stage, message);
    }
}

fn slugify_token(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// When `SCREAMER_SAVE_VISION_SCREENSHOTS` is `1`/`true`/`yes`/`on`, copies the PNG about to be
/// sent to a cloud vision API into `~/Library/Logs/Screamer/vision-cloud/` (after any downscale
/// for large Retina captures) and returns the path (also logged). `provider` is a short tag
/// (e.g. `openai`, `gemini`) embedded in the filename.
pub fn save_cloud_vision_debug_copy(
    source: &Path,
    pipeline_stage: &str,
    provider: &str,
) -> Option<PathBuf> {
    if !env_flag(SAVE_VISION_SCREENSHOTS_ENV) {
        return None;
    }
    if !source.is_file() {
        return None;
    }
    let home = dirs::home_dir()?;
    let dir = home.join(DEFAULT_LOG_DIR).join("vision-cloud");
    fs::create_dir_all(&dir).ok()?;
    let safe_stage = slugify_token(pipeline_stage);
    let safe_provider = slugify_token(provider);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis();
    let seq = VISION_DEBUG_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dest = dir.join(format!(
        "vision-{ms}-{pid}-{seq}-{safe_provider}-{safe_stage}.png"
    ));
    fs::copy(source, &dest).ok()?;
    eprintln!(
        "[screamer][vision][cloud_debug_png] provider={provider} pipeline_stage={pipeline_stage} saved={}",
        dest.display()
    );
    Some(dest)
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

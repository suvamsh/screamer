use screamer_models::find_tts_model;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const TTS_HELPER_BINARY_NAME: &str = "screamer_tts_helper";
const ONNX_RUNTIME_DYLIB_NAME: &str = "libonnxruntime.dylib";
const AFPLAY_COMMAND: &str = "/usr/bin/afplay";
const KOKORO_VOICE: &str = "af_sky";
const KOKORO_SPEED: f32 = 1.08;
/// Screen-help answers can be longer after higher vision token limits; keep TTS in sync so audio
/// is not cut off while the overlay shows the full text.
const MAX_SPOKEN_CHARS: usize = 1200;
const MAX_SPOKEN_SENTENCES: usize = 8;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(40);

static SPEECH_STATE: OnceLock<Mutex<SpeechState>> = OnceLock::new();
static TTS_HELPER: OnceLock<Mutex<Option<TtsHelperProcess>>> = OnceLock::new();
static NEXT_SPEECH_JOB_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_TTS_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub fn warm_up() {
    let _ = std::thread::Builder::new()
        .name("screamer-kokoro-warmup".to_string())
        .spawn(|| {
            if let Err(err) = ensure_tts_helper_ready() {
                eprintln!("[screamer] Kokoro TTS warmup failed: {err}");
            }
        });
}

pub fn speak(text: &str) -> Result<(), String> {
    speak_with_completion(text, || ())
}

/// Like [`speak`], but runs `on_done` after playback finishes (or immediately if there is nothing to speak).
/// `on_done` also runs if synthesis or thread startup fails, after optional cleanup.
pub fn speak_with_completion<F>(text: &str, on_done: F) -> Result<(), String>
where
    F: FnOnce() + Send + 'static,
{
    let spoken = prepare_spoken_text(text);
    let mut on_done = Some(on_done);
    if spoken.is_empty() {
        if let Some(f) = on_done.take() {
            f();
        }
        return Ok(());
    }

    if let Err(err) = ensure_tts_runtime_available() {
        return Err(err);
    }
    let job_id = NEXT_SPEECH_JOB_ID.fetch_add(1, Ordering::SeqCst);
    let audio_path = speech_audio_path(job_id);

    {
        let mut state = speech_state()
            .lock()
            .map_err(|_| "Speech process lock poisoned".to_string())?;
        stop_locked(&mut state);
        state.current_job = Some(job_id);
        state.audio_path = Some(audio_path.clone());
    }

    match std::thread::Builder::new()
        .name("screamer-kokoro-tts".to_string())
        .spawn(move || {
            let result = monitor_speech_job(job_id, spoken, audio_path);
            if let Err(ref err) = result {
                finish_job(job_id);
                eprintln!("[screamer] Speech synthesis error: {err}");
            }
            if let Some(f) = on_done.take() {
                f();
            }
        }) {
        Ok(_) => Ok(()),
        Err(err) => {
            finish_job(job_id);
            Err(format!("Failed to start speech monitor thread: {err}"))
        }
    }
}

pub fn stop() {
    let Ok(mut state) = speech_state().lock() else {
        return;
    };
    stop_locked(&mut state);
}

fn monitor_speech_job(job_id: u64, spoken: String, audio_path: PathBuf) -> Result<(), String> {
    synthesize_to_file(&spoken, &audio_path)?;
    if !is_current_job(job_id) {
        cleanup_audio_file(&audio_path);
        return Ok(());
    }

    let player = Command::new(AFPLAY_COMMAND)
        .arg(&audio_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|err| format!("Failed to start audio playback: {err}"))?;

    {
        let mut state = speech_state()
            .lock()
            .map_err(|_| "Speech process lock poisoned".to_string())?;
        if state.current_job != Some(job_id) {
            stop_detached_child(player, "stale speech player");
            cleanup_audio_file(&audio_path);
            return Ok(());
        }
        state.player = Some(player);
    }

    let _ = wait_for_player(job_id, "speech player")?;
    finish_job(job_id);
    Ok(())
}

fn speech_state() -> &'static Mutex<SpeechState> {
    SPEECH_STATE.get_or_init(|| Mutex::new(SpeechState::default()))
}

fn tts_helper() -> &'static Mutex<Option<TtsHelperProcess>> {
    TTS_HELPER.get_or_init(|| Mutex::new(None))
}

#[derive(Default)]
struct SpeechState {
    current_job: Option<u64>,
    player: Option<Child>,
    audio_path: Option<PathBuf>,
}

struct TtsHelperProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

#[derive(serde::Serialize)]
struct TtsRequest<'a> {
    id: u64,
    text: &'a str,
    output: &'a str,
    voice: &'a str,
    speed: f32,
    gain: f32,
    lang: &'a str,
}

#[derive(serde::Deserialize)]
struct TtsResponse {
    id: u64,
    ok: bool,
    error: Option<String>,
}

fn wait_for_player(job_id: u64, label: &str) -> Result<bool, String> {
    loop {
        let mut state = speech_state()
            .lock()
            .map_err(|_| "Speech process lock poisoned".to_string())?;
        if state.current_job != Some(job_id) {
            return Ok(false);
        }

        let Some(child) = state.player.as_mut() else {
            return Ok(false);
        };

        match child.try_wait() {
            Ok(Some(status)) => {
                state.player = None;
                if status.success() {
                    return Ok(true);
                }
                return Err(format!("{label} exited with status {status}"));
            }
            Ok(None) => {}
            Err(err) => {
                state.player = None;
                return Err(format!("Failed to inspect {label}: {err}"));
            }
        }

        drop(state);
        std::thread::sleep(PROCESS_POLL_INTERVAL);
    }
}

fn stop_locked(state: &mut SpeechState) {
    stop_child(&mut state.player, "speech player");
    if let Some(path) = state.audio_path.take() {
        cleanup_audio_file(&path);
    }
    state.current_job = None;
}

fn stop_child(child_slot: &mut Option<Child>, label: &str) {
    let Some(child) = child_slot.as_mut() else {
        return;
    };

    match child.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) => {
            if let Err(err) = child.kill() {
                eprintln!("[screamer] Failed to stop {label}: {err}");
            }
            let _ = child.wait();
        }
        Err(err) => eprintln!("[screamer] Failed to inspect {label}: {err}"),
    }
    *child_slot = None;
}

fn stop_detached_child(mut child: Child, label: &str) {
    match child.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) => {
            if let Err(err) = child.kill() {
                eprintln!("[screamer] Failed to stop {label}: {err}");
            }
            let _ = child.wait();
        }
        Err(err) => eprintln!("[screamer] Failed to inspect {label}: {err}"),
    }
}

fn finish_job(job_id: u64) {
    let Ok(mut state) = speech_state().lock() else {
        return;
    };
    if state.current_job != Some(job_id) {
        return;
    }
    state.player = None;
    if let Some(path) = state.audio_path.take() {
        cleanup_audio_file(&path);
    }
    state.current_job = None;
}

fn is_current_job(job_id: u64) -> bool {
    speech_state()
        .lock()
        .map(|state| state.current_job == Some(job_id))
        .unwrap_or(false)
}

fn cleanup_audio_file(path: &PathBuf) {
    if let Err(err) = fs::remove_file(path) {
        if err.kind() != std::io::ErrorKind::NotFound {
            eprintln!(
                "[screamer] Failed to remove temporary speech audio {}: {err}",
                path.display()
            );
        }
    }
}

fn ensure_tts_runtime_available() -> Result<(), String> {
    let _ = find_tts_helper()?;
    find_tts_model().ok_or_else(|| {
        "Kokoro TTS model not found. Expected models/tts/0.onnx and models/tts/0.bin. Run ./download_model.sh tts.".to_string()
    })?;
    find_onnx_runtime_dylib().ok_or_else(|| {
        "ONNX Runtime for Kokoro TTS not found. Expected models/tts/onnxruntime/libonnxruntime.dylib or libonnxruntime.dylib next to the app binary. Run ./download_model.sh tts.".to_string()
    })?;
    Ok(())
}

fn ensure_tts_helper_ready() -> Result<(), String> {
    ensure_tts_runtime_available()?;
    let mut helper = tts_helper()
        .lock()
        .map_err(|_| "TTS helper lock poisoned".to_string())?;
    let _ = ensure_tts_helper_locked(&mut helper)?;
    Ok(())
}

fn synthesize_to_file(text: &str, audio_path: &PathBuf) -> Result<(), String> {
    for attempt in 0..2 {
        match synthesize_to_file_once(text, audio_path) {
            Ok(()) => return Ok(()),
            Err(err) if attempt == 0 => {
                eprintln!("[screamer] Restarting Kokoro TTS helper after error: {err}");
                reset_tts_helper();
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("retry loop always returns by the second attempt")
}

fn synthesize_to_file_once(text: &str, audio_path: &PathBuf) -> Result<(), String> {
    let output = audio_path.to_str().ok_or_else(|| {
        format!(
            "Speech audio path is not valid UTF-8: {}",
            audio_path.display()
        )
    })?;
    let request = TtsRequest {
        id: NEXT_TTS_REQUEST_ID.fetch_add(1, Ordering::SeqCst),
        text,
        output,
        voice: KOKORO_VOICE,
        speed: KOKORO_SPEED,
        gain: 1.0,
        lang: "en",
    };

    let mut helper = tts_helper()
        .lock()
        .map_err(|_| "TTS helper lock poisoned".to_string())?;
    let helper = ensure_tts_helper_locked(&mut helper)?;

    serde_json::to_writer(&mut helper.stdin, &request)
        .map_err(|err| format!("Failed to encode TTS request: {err}"))?;
    helper
        .stdin
        .write_all(b"\n")
        .and_then(|_| helper.stdin.flush())
        .map_err(|err| format!("Failed to send TTS request: {err}"))?;

    let mut response_line = String::new();
    loop {
        response_line.clear();
        let bytes = helper
            .stdout
            .read_line(&mut response_line)
            .map_err(|err| format!("Failed to read TTS response: {err}"))?;
        if bytes == 0 {
            return Err("Kokoro TTS helper exited before responding".to_string());
        }

        let response: TtsResponse = serde_json::from_str(response_line.trim())
            .map_err(|err| format!("Invalid TTS helper response: {err}"))?;
        if response.id != request.id {
            eprintln!(
                "[screamer] Ignoring stale TTS response id {}, expected {}",
                response.id, request.id
            );
            continue;
        }

        return if response.ok {
            Ok(())
        } else {
            Err(response
                .error
                .unwrap_or_else(|| "Kokoro TTS helper failed without an error message".to_string()))
        };
    }
}

fn ensure_tts_helper_locked(
    helper_slot: &mut Option<TtsHelperProcess>,
) -> Result<&mut TtsHelperProcess, String> {
    let should_restart = if let Some(helper) = helper_slot.as_mut() {
        match helper.child.try_wait() {
            Ok(Some(status)) => {
                eprintln!("[screamer] Kokoro TTS helper exited with status {status}; restarting");
                true
            }
            Ok(None) => false,
            Err(err) => {
                eprintln!("[screamer] Failed to inspect Kokoro TTS helper: {err}; restarting");
                true
            }
        }
    } else {
        true
    };

    if !should_restart {
        return helper_slot
            .as_mut()
            .ok_or_else(|| "Kokoro TTS helper disappeared unexpectedly".to_string());
    }

    if let Some(mut helper) = helper_slot.take() {
        let _ = helper.child.kill();
        let _ = helper.child.wait();
    }

    let helper_path = find_tts_helper()?;
    let (model_path, voices_path) = find_tts_model().ok_or_else(|| {
        "Kokoro TTS model not found. Expected models/tts/0.onnx and models/tts/0.bin. Run ./download_model.sh tts.".to_string()
    })?;
    *helper_slot = Some(start_tts_server(&helper_path, &model_path, &voices_path)?);
    helper_slot
        .as_mut()
        .ok_or_else(|| "Failed to retain Kokoro TTS helper process".to_string())
}

fn start_tts_server(
    helper_path: &PathBuf,
    model_path: &PathBuf,
    voices_path: &PathBuf,
) -> Result<TtsHelperProcess, String> {
    let mut child = Command::new(helper_path)
        .arg("--server")
        .arg("--model")
        .arg(model_path)
        .arg("--voices")
        .arg(voices_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|err| format!("Failed to start Kokoro TTS helper: {err}"))?;

    let stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Failed to open Kokoro TTS helper stdin".to_string());
        }
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Failed to open Kokoro TTS helper stdout".to_string());
        }
    };

    Ok(TtsHelperProcess {
        child,
        stdin,
        stdout: BufReader::new(stdout),
    })
}

fn reset_tts_helper() {
    let Ok(mut helper_slot) = tts_helper().lock() else {
        return;
    };
    if let Some(mut helper) = helper_slot.take() {
        if let Err(err) = helper.child.kill() {
            eprintln!("[screamer] Failed to stop Kokoro TTS helper: {err}");
        }
        let _ = helper.child.wait();
    }
}

fn find_tts_helper() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|err| format!("Failed to resolve current executable path: {err}"))?;
    if let Some(dir) = exe.parent() {
        let sibling = dir.join(TTS_HELPER_BINARY_NAME);
        if sibling.exists() {
            return Ok(sibling);
        }
    }

    let build_dir = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let local = PathBuf::from("target")
        .join(build_dir)
        .join(TTS_HELPER_BINARY_NAME);
    if local.exists() {
        return Ok(local);
    }

    Err(format!(
        "Kokoro TTS helper not found. Expected {TTS_HELPER_BINARY_NAME} next to the app binary or at {}.",
        local.display()
    ))
}

fn find_onnx_runtime_dylib() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ORT_DYLIB_PATH").map(PathBuf::from) {
        if path.exists() {
            return Some(path);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(ONNX_RUNTIME_DYLIB_NAME);
            if sibling.exists() {
                return Some(sibling);
            }
        }
    }

    let local = PathBuf::from("models")
        .join("tts")
        .join("onnxruntime")
        .join(ONNX_RUNTIME_DYLIB_NAME);
    local.exists().then_some(local)
}

fn speech_audio_path(job_id: u64) -> PathBuf {
    std::env::temp_dir().join(format!(
        "screamer-kokoro-{}-{job_id}.wav",
        std::process::id()
    ))
}

fn prepare_spoken_text(text: &str) -> String {
    let mut normalized = text
        .lines()
        .filter_map(clean_spoken_line)
        .collect::<Vec<_>>()
        .join(" ");

    while normalized.contains("  ") {
        normalized = normalized.replace("  ", " ");
    }

    if let Some(index) = sentence_limit_index(&normalized, MAX_SPOKEN_SENTENCES) {
        normalized.truncate(index);
        return normalized.trim().to_string();
    }

    if normalized.chars().count() <= MAX_SPOKEN_CHARS {
        return normalized;
    }

    let mut truncated = normalized
        .chars()
        .take(MAX_SPOKEN_CHARS)
        .collect::<String>();
    if let Some(index) = truncated.rfind(|ch| matches!(ch, '.' | '!' | '?')) {
        truncated.truncate(index + 1);
    } else {
        truncated.push_str("...");
    }
    truncated
}

fn sentence_limit_index(text: &str, max_sentences: usize) -> Option<usize> {
    let mut count = 0;
    for (index, ch) in text.char_indices() {
        if matches!(ch, '.' | '!' | '?') {
            count += 1;
            if count == max_sentences {
                return Some(index + ch.len_utf8());
            }
        }
    }
    None
}

fn clean_spoken_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let cleaned = trimmed
        .trim_start_matches('#')
        .trim()
        .trim_start_matches(|ch: char| matches!(ch, '-' | '*' | '+'))
        .trim()
        .replace(['`', '*', '_'], "");

    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

#[cfg(test)]
mod tests {
    use super::{prepare_spoken_text, MAX_SPOKEN_CHARS};

    #[test]
    fn prepares_markdownish_text_for_speech() {
        let text = "## Try this\n- Click **Settings**, then `Billing`.";

        assert_eq!(
            prepare_spoken_text(text),
            "Try this Click Settings, then Billing."
        );
    }

    #[test]
    fn caps_unexpectedly_long_text() {
        let text = "a".repeat(MAX_SPOKEN_CHARS + 25);

        assert!(prepare_spoken_text(&text).chars().count() <= MAX_SPOKEN_CHARS + 3);
    }

    #[test]
    fn limits_spoken_text_to_max_sentences() {
        let text = "One. Two! Three? Four. Five. Six. Seven. Eight. Nine.";

        assert_eq!(
            prepare_spoken_text(text),
            "One. Two! Three? Four. Five. Six. Seven. Eight."
        );
    }
}

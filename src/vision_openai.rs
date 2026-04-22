//! OpenAI Chat Completions API for multimodal (vision) screen-help requests.
//! API key: `OPENAI_API_KEY` env var, or one line in `~/Library/Application Support/Screamer/openai_api_key`.
//!
//! With `SCREAMER_SAVE_VISION_SCREENSHOTS=1`, `vision.rs` copies each PNG to
//! `~/Library/Logs/Screamer/vision-cloud/` before the API call (even if the API key is missing).

use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::time::Duration;
use ureq::Error as UreqError;

const OPENAI_CHAT_URL: &str = "https://api.openai.com/v1/chat/completions";
/// Enough for a few short spoken sentences on non-reasoning models.
const VISION_MAX_OUTPUT_TOKENS_DEFAULT: u32 = 1024;

/// GPT-5 / o-series use completion budget for internal reasoning first; a low cap truncates the
/// user-visible answer mid-sentence. Keep this high enough that the spoken reply can finish.
const VISION_MAX_OUTPUT_TOKENS_REASONING: u32 = 8192;

/// GPT-5 / o-series Chat Completions reject `max_tokens`; they require `max_completion_tokens`.
fn model_uses_max_completion_tokens(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    m.starts_with("gpt-5") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4")
}

fn vision_max_output_tokens_openai(model: &str) -> u32 {
    if model_uses_max_completion_tokens(model) {
        VISION_MAX_OUTPUT_TOKENS_REASONING
    } else {
        VISION_MAX_OUTPUT_TOKENS_DEFAULT
    }
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Option<Vec<Choice>>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Choice {
    message: Option<MessageBody>,
}

#[derive(Deserialize)]
struct MessageBody {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

fn image_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

/// Run one vision+text completion (same contract as the local helper stdout).
pub fn complete_vision_multimodal(
    api_key: &str,
    model: &str,
    user_prompt: &str,
    image_path: &Path,
) -> Result<String, String> {
    let bytes = std::fs::read(image_path)
        .map_err(|e| format!("Failed to read screenshot {}: {e}", image_path.display()))?;
    let b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &bytes,
    );
    let mime = image_mime(image_path);
    let data_url = format!("data:{mime};base64,{b64}");

    let max_out = vision_max_output_tokens_openai(model);
    let mut body = Map::new();
    body.insert("model".to_string(), json!(model));
    if model_uses_max_completion_tokens(model) {
        body.insert(
            "max_completion_tokens".to_string(),
            json!(max_out),
        );
    } else {
        body.insert("max_tokens".to_string(), json!(max_out));
    }
    body.insert(
        "messages".to_string(),
        json!([{
            "role": "user",
            "content": [
                {"type": "text", "text": user_prompt},
                {"type": "image_url", "image_url": {"url": data_url}}
            ]
        }]),
    );
    let body = Value::Object(body);

    let response_result = ureq::post(OPENAI_CHAT_URL)
        .set("Authorization", &format!("Bearer {api_key}"))
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(120))
        .send_json(body);

    let (status, text) = match response_result {
        Ok(resp) => {
            let status = resp.status();
            let text = resp
                .into_string()
                .map_err(|e| format!("OpenAI response body: {e}"))?;
            (status, text)
        }
        Err(UreqError::Status(code, resp)) => {
            let text = resp
                .into_string()
                .unwrap_or_else(|_| String::from("(could not read body)"));
            (code, text)
        }
        Err(e) => return Err(format!("OpenAI request failed: {e}")),
    };

    if status >= 400 {
        if let Ok(parsed) = serde_json::from_str::<ChatCompletionResponse>(&text) {
            if let Some(err) = parsed.error {
                return Err(format!("OpenAI API error ({status}): {}", err.message));
            }
        }
        let preview: String = text.chars().take(600).collect();
        return Err(format!("OpenAI API HTTP {status}: {preview}"));
    }

    let parsed: ChatCompletionResponse = serde_json::from_str(&text)
        .map_err(|e| format!("OpenAI invalid JSON: {e}"))?;

    if let Some(err) = parsed.error {
        return Err(format!("OpenAI: {}", err.message));
    }

    let content = parsed
        .choices
        .and_then(|choices| choices.into_iter().next())
        .and_then(|c| c.message)
        .and_then(|m| m.content)
        .ok_or_else(|| "OpenAI response missing message content".to_string())?;

    Ok(content.trim().to_string())
}

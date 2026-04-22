//! Google Gemini `generateContent` API for multimodal screen-help (see <https://ai.google.dev/gemini-api/docs>).
//! API key: `GEMINI_API_KEY` env var, or one line in `~/Library/Application Support/Screamer/gemini_api_key`.
//!
//! Debug PNG copies use `logging::save_cloud_vision_debug_copy` when `SCREAMER_SAVE_VISION_SCREENSHOTS=1`.

use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use std::time::Duration;
use ureq::Error as UreqError;

const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
/// Room for several sentences; avoids cutting off mid-phrase on longer Flash/Pro replies.
const VISION_MAX_OUTPUT_TOKENS: u32 = 2048;

#[derive(Deserialize)]
struct GenerateContentResponse {
    candidates: Option<Vec<Candidate>>,
    error: Option<GeminiApiError>,
}

#[derive(Deserialize)]
struct Candidate {
    content: Option<ContentBody>,
}

#[derive(Deserialize)]
struct ContentBody {
    parts: Option<Vec<Part>>,
}

#[derive(Deserialize)]
struct Part {
    text: Option<String>,
}

#[derive(Deserialize)]
struct GeminiApiError {
    message: String,
    #[allow(dead_code)]
    code: Option<i64>,
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
        Some("gif") => "image/gif",
        _ => "image/png",
    }
}

fn generate_content_url(model: &str) -> String {
    let m = model.trim();
    format!("{GEMINI_API_BASE}/{m}:generateContent")
}

/// One image + text user turn; returns assistant text (trimmed).
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

    let body = json!({
        "contents": [{
            "role": "user",
            "parts": [
                { "text": user_prompt },
                {
                    "inline_data": {
                        "mime_type": mime,
                        "data": b64
                    }
                }
            ]
        }],
        "generationConfig": {
            "maxOutputTokens": VISION_MAX_OUTPUT_TOKENS
        }
    });

    let url = generate_content_url(model);
    let response_result = ureq::post(&url)
        .set("x-goog-api-key", api_key)
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(120))
        .send_json(body);

    let (status, text) = match response_result {
        Ok(resp) => {
            let status = resp.status();
            let text = resp
                .into_string()
                .map_err(|e| format!("Gemini response body: {e}"))?;
            (status, text)
        }
        Err(UreqError::Status(code, resp)) => {
            let text = resp
                .into_string()
                .unwrap_or_else(|_| String::from("(could not read body)"));
            (code, text)
        }
        Err(e) => return Err(format!("Gemini request failed: {e}")),
    };

    if status >= 400 {
        if let Ok(parsed) = serde_json::from_str::<GenerateContentResponse>(&text) {
            if let Some(err) = parsed.error {
                return Err(format!("Gemini API error ({status}): {}", err.message));
            }
        }
        let preview: String = text.chars().take(600).collect();
        return Err(format!("Gemini API HTTP {status}: {preview}"));
    }

    let parsed: GenerateContentResponse = serde_json::from_str(&text)
        .map_err(|e| format!("Gemini invalid JSON: {e}"))?;

    if let Some(err) = parsed.error {
        return Err(format!("Gemini: {}", err.message));
    }

    let mut out = String::new();
    if let Some(candidates) = parsed.candidates {
        for c in candidates {
            if let Some(content) = c.content {
                if let Some(parts) = content.parts {
                    for p in parts {
                        if let Some(t) = p.text {
                            if !out.is_empty() {
                                out.push('\n');
                            }
                            out.push_str(&t);
                        }
                    }
                }
            }
        }
    }

    let trimmed = out.trim().to_string();
    if trimmed.is_empty() {
        return Err("Gemini response had no text parts".to_string());
    }
    Ok(trimmed)
}

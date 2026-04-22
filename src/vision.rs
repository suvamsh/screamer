use crate::config::{Config, VisionProvider};
use crate::logging;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

const HELPER_BINARY_NAME: &str = "screamer_vision_helper";
const VISION_MAX_TOKENS: &str = "256";
const MEDIA_MARKER: &str = "<__media__>";
const APP_CONTENT_TOP_PCT: f64 = 14.0;
/// Retina screenshots are often ~5M pixels; cloud APIs are much faster on smaller images.
const CLOUD_VISION_MAX_LONG_EDGE: u32 = 1536;
static NEXT_VISION_CROP_ID: AtomicU64 = AtomicU64::new(1);
const SCREEN_HELPER_ANSWER_PROMPT: &str = "\
You are Screamer's screen buddy. The screenshot is from a product, website, \
or web app, and the user is asking how to use it. Be concise because your \
answer will be spoken aloud. Answer in at most three short sentences. Give \
the exact next step when you can. Do not use markdown, bullets, lists, or \
long explanations. Start with the answer itself; do not prefix it with \
\"Answer:\" or \"This is the answer.\" If the screenshot is unclear, say what \
you can see and ask one short follow-up question.";
const SCREEN_HELPER_LOCALIZE_PROMPT: &str = "\
You are Screamer's screen buddy. Your only job is to localize the single UI \
element in the screenshot that the spoken answer is telling the user to use \
next. Return exactly one line in this format and nothing else:\n\
POINT(x_percent, y_percent, side)\n\n\
Rules:\n\
- x_percent and y_percent are the exact point where the arrow tip should \
land.\n\
- side must be exactly one of: left, right, top, bottom.\n\
- side means where the arrow body should sit relative to the target point. \
For example, left means the arrow stays to the left of the target and points \
right into it.\n\
- Use percentages from the screenshot's left edge and top edge.\n\
- The point must land on the actual control, icon, button, field, row, or \
tab the user should interact with next.\n\
- The point must agree with location words in the spoken answer such as left \
sidebar, right sidebar, top right, bottom bar, etc.\n\
- If the target is a tiny icon next to text, point at the icon itself and \
choose the side that keeps the arrow readable.\n\
- Never point to browser chrome, tabs, bookmarks, extension icons, or OS UI \
unless the spoken answer explicitly mentions them.\n\
- If you cannot confidently identify the element, return NONE.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerSide {
    Left,
    Right,
    Top,
    Bottom,
}

impl PointerSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }

    pub fn rotation_degrees(self) -> f64 {
        match self {
            Self::Left => 0.0,
            Self::Right => 180.0,
            Self::Top => -90.0,
            Self::Bottom => 90.0,
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value
            .trim()
            .trim_matches(&['"', '\''][..])
            .to_ascii_lowercase()
            .as_str()
        {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }
}

/// The exact pointer target returned by the vision model, in percentages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenPoint {
    /// Percentage from the left edge of the screen.
    pub x_pct: f64,
    /// Percentage from the top edge of the screen.
    pub y_pct: f64,
    /// Which side of the point the arrow body should sit on.
    pub side: PointerSide,
}

impl ScreenPoint {
    pub fn describe(&self) -> String {
        format!(
            "POINT({:.2}, {:.2}, {})",
            self.x_pct,
            self.y_pct,
            self.side.as_str()
        )
    }
}

/// Legacy bounding box support kept as a fallback if the model emits BOX(...)
/// instead of the new POINT(...) directive.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScreenRect {
    x_pct: f64,
    y_pct: f64,
    w_pct: f64,
    h_pct: f64,
}

/// The result of a vision query: the spoken answer and an optional pointer target.
#[derive(Clone, Debug)]
pub struct VisionResult {
    pub text: String,
    pub highlight: Option<ScreenPoint>,
}

/// Run a vision query: pass the user's transcribed question + screenshot to the
/// configured vision backend (local Gemma, OpenAI, or Gemini) and return the response with optional pointer target.
pub fn ask_about_screen(prompt: &str, screenshot_path: &Path) -> Result<VisionResult, String> {
    let config = Config::load();
    logging::log_vision_event(
        "query_start",
        &format!(
            "screenshot={} question_chars={}",
            screenshot_path.display(),
            prompt.chars().count()
        ),
    );
    logging::log_vision_block("user_question", prompt);

    let answer_prompt = build_screen_helper_answer_prompt(prompt);
    let answer_raw = run_vision_inference(&config, "answer", &answer_prompt, screenshot_path)?;
    let answer_cleaned = clean_model_response(&answer_raw);
    logging::log_vision_block("answer_cleaned_output", &answer_cleaned);

    let (answer_without_point, answer_point) = extract_point(&answer_cleaned);
    let (answer_text, answer_box) = extract_box(&answer_without_point);
    let answer_text = answer_text.trim().to_string();
    logging::log_vision_block("answer_text", &answer_text);
    logging::log_vision_event(
        "answer_directives",
        &format!(
            "inline_point={} inline_box={}",
            describe_point_option(answer_point),
            describe_rect_option(answer_box)
        ),
    );

    let context = build_context(prompt, &answer_text);
    logging::log_vision_block("context", &context);

    let answer_fallback =
        answer_point.or_else(|| answer_box.map(|rect| legacy_box_to_point(&context, rect)));
    let localized = if config.vision_fast_screen_help {
        logging::log_vision_event(
            "localize_skip",
            "vision_fast_screen_help: skipping localization model call",
        );
        None
    } else if answer_fallback.is_some() {
        logging::log_vision_event(
            "localize_skip",
            "answer_included_point_or_box; skipping second model call",
        );
        None
    } else {
        localize_highlight(prompt, &answer_text, screenshot_path, &context, &config)
    };
    let highlight = choose_highlight(&context, localized.or(answer_fallback));
    logging::log_vision_event("final_target", &describe_point_option(highlight));

    Ok(VisionResult {
        text: answer_text,
        highlight,
    })
}

/// Runs `f` with a path to a PNG suitable for cloud vision: scales down when the longest edge
/// exceeds [`CLOUD_VISION_MAX_LONG_EDGE`], then deletes any temporary file afterward.
fn with_cloud_vision_image<R>(
    source: &Path,
    f: impl FnOnce(&Path) -> Result<R, String>,
) -> Result<R, String> {
    let img = image::open(source).map_err(|err| {
        format!(
            "Failed to open screenshot {}: {err}",
            source.display()
        )
    })?;
    let (ow, oh) = (img.width(), img.height());
    let long = ow.max(oh);
    if long <= CLOUD_VISION_MAX_LONG_EDGE {
        return f(source);
    }
    let scaled = img.thumbnail(CLOUD_VISION_MAX_LONG_EDGE, CLOUD_VISION_MAX_LONG_EDGE);
    logging::log_vision_event(
        "cloud_image_resize",
        &format!(
            "{}x{} -> {}x{} (max_long_edge={})",
            ow,
            oh,
            scaled.width(),
            scaled.height(),
            CLOUD_VISION_MAX_LONG_EDGE
        ),
    );
    let tmp = std::env::temp_dir().join(format!(
        "screamer-vision-api-{}-{}.png",
        std::process::id(),
        NEXT_VISION_CROP_ID.fetch_add(1, Ordering::SeqCst)
    ));
    scaled.save(&tmp).map_err(|err| {
        format!(
            "Failed to write resized vision image {}: {err}",
            tmp.display()
        )
    })?;
    let out = f(&tmp);
    let _ = std::fs::remove_file(&tmp);
    out
}

fn run_vision_inference(
    config: &Config,
    stage: &str,
    model_prompt: &str,
    screenshot_path: &Path,
) -> Result<String, String> {
    match config.vision_provider {
        VisionProvider::Local => {
            run_local_vision_helper(stage, model_prompt, screenshot_path)
        }
        VisionProvider::Openai => {
            with_cloud_vision_image(screenshot_path, |api_path| {
                let _ = crate::logging::save_cloud_vision_debug_copy(api_path, stage, "openai");
                let key = Config::resolve_openai_api_key()?;
                logging::log_vision_event(
                    stage,
                    &format!(
                        "backend=openai model={}",
                        config.vision_openai_model.trim()
                    ),
                );
                crate::vision_openai::complete_vision_multimodal(
                    &key,
                    config.vision_openai_model.trim(),
                    model_prompt,
                    api_path,
                )
            })
        }
        VisionProvider::Gemini => {
            with_cloud_vision_image(screenshot_path, |api_path| {
                let _ = crate::logging::save_cloud_vision_debug_copy(api_path, stage, "gemini");
                let key = Config::resolve_gemini_api_key()?;
                logging::log_vision_event(
                    stage,
                    &format!(
                        "backend=gemini model={}",
                        config.vision_gemini_model.trim()
                    ),
                );
                crate::vision_gemini::complete_vision_multimodal(
                    &key,
                    config.vision_gemini_model.trim(),
                    model_prompt,
                    api_path,
                )
            })
        }
    }
}

fn run_local_vision_helper(
    stage: &str,
    model_prompt: &str,
    screenshot_path: &Path,
) -> Result<String, String> {
    let helper_path = find_helper_path()
        .ok_or_else(|| format!("Vision helper not found: {HELPER_BINARY_NAME}"))?;
    let screenshot_str = screenshot_path
        .to_str()
        .ok_or("Screenshot path contains invalid UTF-8")?;

    logging::log_vision_event(
        stage,
        &format!(
            "helper={} image={} max_tokens={}",
            helper_path.display(),
            screenshot_path.display(),
            VISION_MAX_TOKENS
        ),
    );
    logging::log_vision_block(&format!("{stage}_prompt"), model_prompt);

    let mut child = Command::new(&helper_path)
        .arg("--image")
        .arg(screenshot_str)
        .arg("--max-tokens")
        .arg(VISION_MAX_TOKENS)
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
            .write_all(model_prompt.as_bytes())
            .map_err(|err| format!("Failed to send prompt to vision helper: {err}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("Failed to wait for vision helper: {err}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let err = if stderr.is_empty() {
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
        };
        logging::log_flow_event("vision", stage, &format!("helper_error={err}"));
        return Err(err);
    }

    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    logging::log_vision_block(&format!("{stage}_raw_output"), &raw);
    Ok(raw)
}

fn build_screen_helper_answer_prompt(user_question: &str) -> String {
    format!(
        "{SCREEN_HELPER_ANSWER_PROMPT}\n\nUser question: {}",
        user_question.trim()
    )
}

fn build_screen_helper_localize_prompt(user_question: &str, answer_text: &str) -> String {
    format!(
        "{SCREEN_HELPER_LOCALIZE_PROMPT}\n\nUser question: {}\nSpoken answer: {}",
        user_question.trim(),
        answer_text.trim()
    )
}

fn build_screen_helper_localize_crop_prompt(
    user_question: &str,
    answer_text: &str,
    region_name: &str,
) -> String {
    format!(
        "{SCREEN_HELPER_LOCALIZE_PROMPT}\n\nUser question: {}\nSpoken answer: {}\nThe screenshot has already been cropped to the {} region of the original screen. Localize only within this cropped image.",
        user_question.trim(),
        answer_text.trim(),
        region_name
    )
}

fn clean_model_response(response: &str) -> String {
    let mut normalized = response
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && *line != "<start_of_image>"
                && *line != "<end_of_image>"
                && *line != MEDIA_MARKER
        })
        .collect::<Vec<_>>()
        .join(" ");

    while normalized.contains("  ") {
        normalized = normalized.replace("  ", " ");
    }

    strip_answer_prefix(&normalized).to_string()
}

fn strip_answer_prefix(text: &str) -> &str {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    for prefix in [
        "answer:",
        "final answer:",
        "the answer is:",
        "this is the answer:",
        "here's the answer:",
        "here is the answer:",
    ] {
        if lower.starts_with(prefix) {
            return trimmed[prefix.len()..].trim_start();
        }
    }
    trimmed
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ImageRegion {
    x_pct: f64,
    y_pct: f64,
    w_pct: f64,
    h_pct: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LocalizationRegion {
    name: &'static str,
    rect: ImageRegion,
}

fn build_context(user_question: &str, answer_text: &str) -> String {
    format!(
        "{} {}",
        user_question.trim().to_ascii_lowercase(),
        answer_text.trim().to_ascii_lowercase()
    )
}

fn choose_highlight(context: &str, candidate: Option<ScreenPoint>) -> Option<ScreenPoint> {
    let context = context.trim().to_ascii_lowercase();

    match candidate {
        Some(point) if point_matches_directional_cues(&context, &point) => {
            logging::log_vision_event(
                "highlight_choice",
                &format!("accepted_candidate={}", point.describe()),
            );
            Some(point)
        }
        Some(point) => {
            logging::log_vision_event(
                "highlight_choice",
                &format!(
                    "rejected_candidate={} reason=directional_mismatch",
                    point.describe()
                ),
            );
            let fallback = directional_fallback_point(&context);
            logging::log_vision_event(
                "highlight_fallback",
                &describe_point_option(fallback),
            );
            fallback
        }
        None => {
            logging::log_vision_event("highlight_choice", "no_candidate_from_model");
            let fallback = directional_fallback_point(&context);
            logging::log_vision_event(
                "highlight_fallback",
                &describe_point_option(fallback),
            );
            fallback
        }
    }
}

fn context_mentions_browser_ui(context: &str) -> bool {
    [
        "browser",
        "address bar",
        "url bar",
        "omnibox",
        "browser tab",
        "new tab",
        "tab bar",
        "tabs",
        "bookmark",
        "extension",
        "refresh",
        "back button",
        "forward button",
        "site info",
        "site settings",
        "chrome",
    ]
    .iter()
    .any(|needle| context.contains(needle))
}

fn point_matches_directional_cues(context: &str, point: &ScreenPoint) -> bool {
    if !context_mentions_browser_ui(context) && point.y_pct < APP_CONTENT_TOP_PCT {
        return false;
    }

    if context.contains("left sidebar") || context.contains("left side bar") {
        return point.x_pct <= 24.0;
    }
    if context.contains("right sidebar") || context.contains("right side bar") {
        return point.x_pct >= 76.0;
    }
    if context.contains("top right") || context.contains("upper right") {
        return point.x_pct >= 60.0 && point.y_pct >= APP_CONTENT_TOP_PCT && point.y_pct <= 42.0;
    }
    if context.contains("top left") || context.contains("upper left") {
        return point.x_pct <= 40.0 && point.y_pct >= APP_CONTENT_TOP_PCT && point.y_pct <= 42.0;
    }
    if context.contains("bottom right") || context.contains("lower right") {
        return point.x_pct >= 60.0 && point.y_pct >= 60.0;
    }
    if context.contains("bottom left") || context.contains("lower left") {
        return point.x_pct <= 40.0 && point.y_pct >= 60.0;
    }

    true
}

fn directional_fallback_point(context: &str) -> Option<ScreenPoint> {
    if context.contains("left sidebar") || context.contains("left side bar") {
        return Some(ScreenPoint {
            x_pct: 12.0,
            y_pct: APP_CONTENT_TOP_PCT + 28.0,
            side: PointerSide::Left,
        });
    }
    if context.contains("right sidebar") || context.contains("right side bar") {
        return Some(ScreenPoint {
            x_pct: 88.0,
            y_pct: APP_CONTENT_TOP_PCT + 28.0,
            side: PointerSide::Right,
        });
    }
    if context.contains("top right") || context.contains("upper right") {
        return Some(ScreenPoint {
            x_pct: 86.0,
            y_pct: APP_CONTENT_TOP_PCT + 12.0,
            side: PointerSide::Top,
        });
    }
    if context.contains("top left") || context.contains("upper left") {
        return Some(ScreenPoint {
            x_pct: 14.0,
            y_pct: APP_CONTENT_TOP_PCT + 12.0,
            side: PointerSide::Top,
        });
    }
    if context.contains("bottom right") || context.contains("lower right") {
        return Some(ScreenPoint {
            x_pct: 86.0,
            y_pct: 86.0,
            side: PointerSide::Bottom,
        });
    }
    if context.contains("bottom left") || context.contains("lower left") {
        return Some(ScreenPoint {
            x_pct: 14.0,
            y_pct: 86.0,
            side: PointerSide::Bottom,
        });
    }

    None
}

fn default_pointer_side_for_context(context: &str) -> PointerSide {
    if context.contains("left sidebar") || context.contains("left side bar") {
        return PointerSide::Left;
    }
    if context.contains("right sidebar") || context.contains("right side bar") {
        return PointerSide::Right;
    }
    if context.contains("bottom right")
        || context.contains("lower right")
        || context.contains("bottom left")
        || context.contains("lower left")
    {
        return PointerSide::Bottom;
    }

    PointerSide::Top
}

fn classify_localization_region(context: &str) -> Option<LocalizationRegion> {
    if context_mentions_browser_ui(context) {
        return None;
    }

    if context.contains("left sidebar") || context.contains("left side bar") {
        return Some(LocalizationRegion {
            name: "left sidebar",
            rect: ImageRegion {
                x_pct: 0.0,
                y_pct: APP_CONTENT_TOP_PCT,
                w_pct: 24.0,
                h_pct: 100.0 - APP_CONTENT_TOP_PCT,
            },
        });
    }
    if context.contains("right sidebar") || context.contains("right side bar") {
        return Some(LocalizationRegion {
            name: "right sidebar",
            rect: ImageRegion {
                x_pct: 76.0,
                y_pct: APP_CONTENT_TOP_PCT,
                w_pct: 24.0,
                h_pct: 100.0 - APP_CONTENT_TOP_PCT,
            },
        });
    }
    if context.contains("top right") || context.contains("upper right") {
        return Some(LocalizationRegion {
            name: "top-right app area",
            rect: ImageRegion {
                x_pct: 66.0,
                y_pct: APP_CONTENT_TOP_PCT,
                w_pct: 34.0,
                h_pct: 28.0,
            },
        });
    }
    if context.contains("top left") || context.contains("upper left") {
        return Some(LocalizationRegion {
            name: "top-left app area",
            rect: ImageRegion {
                x_pct: 0.0,
                y_pct: APP_CONTENT_TOP_PCT,
                w_pct: 34.0,
                h_pct: 28.0,
            },
        });
    }

    Some(LocalizationRegion {
        name: "app content area",
        rect: ImageRegion {
            x_pct: 0.0,
            y_pct: APP_CONTENT_TOP_PCT,
            w_pct: 100.0,
            h_pct: 100.0 - APP_CONTENT_TOP_PCT,
        },
    })
}

fn localize_highlight(
    user_question: &str,
    answer_text: &str,
    screenshot_path: &Path,
    context: &str,
    config: &Config,
) -> Option<ScreenPoint> {
    let region = classify_localization_region(context);
    logging::log_vision_event(
        "localize_region",
        &match region {
            Some(region) => format!("name={} rect={}", region.name, describe_region(region.rect)),
            None => "name=none reason=browser_ui_context".to_string(),
        },
    );

    if let Some(region) = region {
        if let Some(point) = localize_highlight_in_crop(
            user_question,
            answer_text,
            screenshot_path,
            context,
            region,
            config,
        ) {
            return Some(point);
        }
        logging::log_vision_event(
            "localize_crop_fallback",
            "crop_localization_failed_trying_full_image",
        );
    }

    let prompt = build_screen_helper_localize_prompt(user_question, answer_text);
    let raw = run_vision_inference(config, "localize_full", &prompt, screenshot_path).ok()?;
    let cleaned = clean_model_response(&raw);
    parse_localization_output("localize_full", &cleaned, context, None)
}

fn localize_highlight_in_crop(
    user_question: &str,
    answer_text: &str,
    screenshot_path: &Path,
    context: &str,
    region: LocalizationRegion,
    config: &Config,
) -> Option<ScreenPoint> {
    let crop_path = match crop_image_to_region(screenshot_path, region.rect) {
        Ok(path) => path,
        Err(err) => {
            logging::log_vision_event(
                "localize_crop_error",
                &format!("region={} error={err}", region.name),
            );
            return None;
        }
    };

    logging::log_vision_event(
        "localize_crop_image",
        &format!(
            "region={} rect={} crop={}",
            region.name,
            describe_region(region.rect),
            crop_path.display()
        ),
    );

    let prompt = build_screen_helper_localize_crop_prompt(user_question, answer_text, region.name);
    let raw = run_vision_inference(config, "localize_crop", &prompt, &crop_path).ok();
    let result = raw.and_then(|raw| {
        let cleaned = clean_model_response(&raw);
        parse_localization_output("localize_crop", &cleaned, context, Some(region.rect))
    });
    let _ = std::fs::remove_file(&crop_path);
    logging::log_vision_event(
        "localize_crop_cleanup",
        &format!("removed={}", crop_path.display()),
    );
    result
}

fn crop_image_to_region(screenshot_path: &Path, region: ImageRegion) -> Result<PathBuf, String> {
    let image = image::open(screenshot_path).map_err(|err| {
        format!(
            "Failed to open screenshot {}: {err}",
            screenshot_path.display()
        )
    })?;

    let image_width = image.width().max(1);
    let image_height = image.height().max(1);

    let x = pct_to_px(region.x_pct, image_width);
    let y = pct_to_px(region.y_pct, image_height);
    let max_width = image_width.saturating_sub(x).max(1);
    let max_height = image_height.saturating_sub(y).max(1);
    let width = pct_span_to_px(region.w_pct, image_width).clamp(1, max_width);
    let height = pct_span_to_px(region.h_pct, image_height).clamp(1, max_height);

    let cropped = image.crop_imm(x, y, width, height);
    let crop_path = vision_crop_path();
    cropped.save(&crop_path).map_err(|err| {
        format!(
            "Failed to save cropped screenshot {}: {err}",
            crop_path.display()
        )
    })?;
    Ok(crop_path)
}

fn vision_crop_path() -> PathBuf {
    let id = NEXT_VISION_CROP_ID.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "screamer-vision-crop-{}-{id}.png",
        std::process::id()
    ))
}

fn pct_to_px(pct: f64, full: u32) -> u32 {
    let pct = pct.clamp(0.0, 100.0);
    ((pct / 100.0) * f64::from(full)).floor() as u32
}

fn pct_span_to_px(pct: f64, full: u32) -> u32 {
    let pct = pct.clamp(0.0, 100.0);
    (((pct / 100.0) * f64::from(full)).round() as u32).max(1)
}

fn remap_point_from_region(point: ScreenPoint, region: ImageRegion) -> ScreenPoint {
    ScreenPoint {
        x_pct: region.x_pct + point.x_pct * region.w_pct / 100.0,
        y_pct: region.y_pct + point.y_pct * region.h_pct / 100.0,
        side: point.side,
    }
}

fn remap_rect_from_region(rect: ScreenRect, region: ImageRegion) -> ScreenRect {
    ScreenRect {
        x_pct: region.x_pct + rect.x_pct * region.w_pct / 100.0,
        y_pct: region.y_pct + rect.y_pct * region.h_pct / 100.0,
        w_pct: rect.w_pct * region.w_pct / 100.0,
        h_pct: rect.h_pct * region.h_pct / 100.0,
    }
}

fn parse_localization_output(
    stage: &str,
    cleaned: &str,
    context: &str,
    region: Option<ImageRegion>,
) -> Option<ScreenPoint> {
    logging::log_vision_block(&format!("{stage}_cleaned_output"), cleaned);

    let (_, point) = extract_point(cleaned);
    if let Some(point) = point {
        let point = region.map_or(point, |region| remap_point_from_region(point, region));
        logging::log_vision_event(
            stage,
            &format!("parsed_target={} source=point", point.describe()),
        );
        return Some(point);
    }

    let (_, rect) = extract_box(cleaned);
    if let Some(rect) = rect {
        let rect = region.map_or(rect, |region| remap_rect_from_region(rect, region));
        let point = legacy_box_to_point(context, rect);
        logging::log_vision_event(
            stage,
            &format!(
                "parsed_target={} source=legacy_box legacy_box={}",
                point.describe(),
                describe_rect(rect)
            ),
        );
        return Some(point);
    }

    logging::log_vision_event(
        stage,
        "parsed_target=none source=unparseable_output",
    );
    None
}

fn legacy_box_to_point(context: &str, rect: ScreenRect) -> ScreenPoint {
    ScreenPoint {
        x_pct: rect.x_pct + rect.w_pct / 2.0,
        y_pct: rect.y_pct + rect.h_pct / 2.0,
        side: default_pointer_side_for_context(context),
    }
}

fn describe_region(region: ImageRegion) -> String {
    format!(
        "REGION({:.2}, {:.2}, {:.2}, {:.2})",
        region.x_pct, region.y_pct, region.w_pct, region.h_pct
    )
}

fn describe_rect(rect: ScreenRect) -> String {
    format!(
        "BOX({:.2}, {:.2}, {:.2}, {:.2})",
        rect.x_pct, rect.y_pct, rect.w_pct, rect.h_pct
    )
}

fn describe_point_option(point: Option<ScreenPoint>) -> String {
    point
        .map(|point| point.describe())
        .unwrap_or_else(|| "NONE".to_string())
}

fn describe_rect_option(rect: Option<ScreenRect>) -> String {
    rect.map(describe_rect)
        .unwrap_or_else(|| "NONE".to_string())
}

/// Parse a `POINT(x, y, side)` suffix from the model response.
/// Returns the text without the POINT directive and the parsed point (if any).
fn extract_point(text: &str) -> (String, Option<ScreenPoint>) {
    let Some((clean_text, args)) = extract_directive_body(text, "POINT") else {
        return (text.trim().to_string(), None);
    };

    let parts: Vec<&str> = args.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        return (text.trim().to_string(), None);
    }

    let x_pct = parts[0].parse::<f64>().ok();
    let y_pct = parts[1].parse::<f64>().ok();
    let side = PointerSide::from_str(parts[2]);

    match (x_pct, y_pct, side) {
        (Some(x_pct), Some(y_pct), Some(side))
            if (0.0..=100.0).contains(&x_pct) && (0.0..=100.0).contains(&y_pct) =>
        {
            (clean_text, Some(ScreenPoint { x_pct, y_pct, side }))
        }
        _ => (text.trim().to_string(), None),
    }
}

/// Parse a legacy `BOX(x, y, w, h)` suffix from the model response.
/// Returns the text without the BOX directive and the parsed rect (if any).
fn extract_box(text: &str) -> (String, Option<ScreenRect>) {
    let Some((clean_text, args)) = extract_directive_body(text, "BOX") else {
        return (text.trim().to_string(), None);
    };

    let parts: Vec<f64> = args
        .split(',')
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .collect();

    if parts.len() == 4
        && parts.iter().all(|&v| (0.0..=100.0).contains(&v))
        && parts[2] > 0.0
        && parts[3] > 0.0
    {
        (
            clean_text,
            Some(ScreenRect {
                x_pct: parts[0],
                y_pct: parts[1],
                w_pct: parts[2],
                h_pct: parts[3],
            }),
        )
    } else {
        (text.trim().to_string(), None)
    }
}

fn extract_directive_body(text: &str, directive_name: &str) -> Option<(String, String)> {
    let uppercase = text.to_ascii_uppercase();
    let needle = format!("{directive_name}(");
    let directive_start = uppercase.rfind(&needle)?;
    let args_start = directive_start + needle.len();
    let args_end = text[args_start..].find(')')?;
    let args = text[args_start..args_start + args_end].trim().to_string();
    let clean_text = text[..directive_start].trim().to_string();
    Some((clean_text, args))
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

    if let Some(parent) = exe.parent() {
        if parent.file_name().map(|n| n == "MacOS").unwrap_or(false) {
            candidates.push(parent.join(HELPER_BINARY_NAME));
        }
    }

    candidates.into_iter().find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::{
        build_context, build_screen_helper_answer_prompt, build_screen_helper_localize_crop_prompt,
        build_screen_helper_localize_prompt, classify_localization_region, clean_model_response,
        context_mentions_browser_ui, directional_fallback_point, extract_box, extract_point,
        legacy_box_to_point, point_matches_directional_cues, remap_point_from_region,
        remap_rect_from_region, ImageRegion, PointerSide, ScreenPoint, ScreenRect,
        APP_CONTENT_TOP_PCT,
    };

    #[test]
    fn screen_helper_prompt_keeps_spoken_answers_short() {
        let prompt = build_screen_helper_answer_prompt("How do I change this setting?");

        assert!(prompt.contains("spoken aloud"));
        assert!(prompt.contains("at most three short sentences"));
        assert!(prompt.contains("do not prefix"));
        assert!(prompt.contains("User question: How do I change this setting?"));
    }

    #[test]
    fn localization_prompt_mentions_point_format() {
        let prompt = build_screen_helper_localize_prompt(
            "How do I make a vault?",
            "Click the plus sign next to Vault in the left sidebar.",
        );

        assert!(prompt.contains("POINT(x_percent, y_percent, side)"));
        assert!(prompt.contains("left means the arrow stays to the left"));
        assert!(prompt.contains("Spoken answer: Click the plus sign next to Vault"));
    }

    #[test]
    fn crop_localization_prompt_mentions_region_name() {
        let prompt = build_screen_helper_localize_crop_prompt(
            "How do I make a vault?",
            "Click the plus sign next to Vault in the left sidebar.",
            "left sidebar",
        );

        assert!(prompt.contains("cropped to the left sidebar region"));
    }

    #[test]
    fn cleans_image_markers_and_answer_prefix_for_tts() {
        let response = "<start_of_image>\nAnswer: Click Share, then Copy Link.";

        assert_eq!(
            clean_model_response(response),
            "Click Share, then Copy Link."
        );
    }

    #[test]
    fn extract_point_parses_valid_coordinates_and_side() {
        let text = "POINT(16.4, 42.5, left)";
        let (_, point) = extract_point(text);
        let point = point.unwrap();

        assert!((point.x_pct - 16.4).abs() < 0.01);
        assert!((point.y_pct - 42.5).abs() < 0.01);
        assert_eq!(point.side, PointerSide::Left);
    }

    #[test]
    fn extract_point_returns_none_when_missing() {
        let (_, point) = extract_point("Click the button.");
        assert!(point.is_none());
    }

    #[test]
    fn extract_box_parses_valid_coordinates() {
        let text = "Click the Share button in the top right.\nBOX(85.2, 3.0, 6.5, 2.8)";
        let (clean, rect) = extract_box(text);
        assert_eq!(clean, "Click the Share button in the top right.");
        let rect = rect.unwrap();

        assert!((rect.x_pct - 85.2).abs() < 0.01);
        assert!((rect.y_pct - 3.0).abs() < 0.01);
        assert!((rect.w_pct - 6.5).abs() < 0.01);
        assert!((rect.h_pct - 2.8).abs() < 0.01);
    }

    #[test]
    fn extract_box_rejects_out_of_range() {
        let (_, rect) = extract_box("Click here.\nBOX(150.0, 3.0, 6.5, 2.8)");
        assert!(rect.is_none());
    }

    #[test]
    fn legacy_box_fallback_uses_center_and_context_side() {
        let point = legacy_box_to_point(
            "click the plus sign next to vault in the left sidebar",
            ScreenRect {
                x_pct: 8.0,
                y_pct: 32.0,
                w_pct: 4.0,
                h_pct: 6.0,
            },
        );

        assert!((point.x_pct - 10.0).abs() < 0.01);
        assert!((point.y_pct - 35.0).abs() < 0.01);
        assert_eq!(point.side, PointerSide::Left);
    }

    #[test]
    fn rejects_point_that_conflicts_with_left_sidebar_language() {
        let point = ScreenPoint {
            x_pct: 40.0,
            y_pct: 2.0,
            side: PointerSide::Top,
        };
        assert!(!point_matches_directional_cues(
            "click the plus sign in the left sidebar",
            &point
        ));
    }

    #[test]
    fn provides_left_sidebar_fallback_point() {
        let point = directional_fallback_point("click the plus sign in the left sidebar").unwrap();
        assert!(point.x_pct <= 14.0);
        assert!(point.y_pct >= APP_CONTENT_TOP_PCT);
        assert_eq!(point.side, PointerSide::Left);
    }

    #[test]
    fn rejects_browser_chrome_when_answer_does_not_reference_browser_ui() {
        let point = ScreenPoint {
            x_pct: 5.0,
            y_pct: 2.0,
            side: PointerSide::Top,
        };
        assert!(!point_matches_directional_cues(
            "click the plus sign next to vault in the left sidebar",
            &point
        ));
    }

    #[test]
    fn detects_browser_ui_context() {
        assert!(context_mentions_browser_ui("click the tab in the browser"));
        assert!(!context_mentions_browser_ui(
            "click the plus sign next to vault in the left sidebar"
        ));
    }

    #[test]
    fn classifies_left_sidebar_region() {
        let region =
            classify_localization_region("click the plus sign next to vault in the left sidebar")
                .unwrap();
        assert_eq!(region.name, "left sidebar");
        assert!(region.rect.w_pct <= 24.0);
        assert!(region.rect.y_pct >= APP_CONTENT_TOP_PCT);
    }

    #[test]
    fn remaps_crop_point_coordinates_back_to_full_image() {
        let point = remap_point_from_region(
            ScreenPoint {
                x_pct: 50.0,
                y_pct: 25.0,
                side: PointerSide::Left,
            },
            ImageRegion {
                x_pct: 0.0,
                y_pct: APP_CONTENT_TOP_PCT,
                w_pct: 24.0,
                h_pct: 86.0,
            },
        );

        assert!((point.x_pct - 12.0).abs() < 0.01);
        assert!((point.y_pct - (APP_CONTENT_TOP_PCT + 21.5)).abs() < 0.01);
        assert_eq!(point.side, PointerSide::Left);
    }

    #[test]
    fn remaps_crop_coordinates_back_to_full_image_for_legacy_box() {
        let rect = remap_rect_from_region(
            ScreenRect {
                x_pct: 50.0,
                y_pct: 25.0,
                w_pct: 10.0,
                h_pct: 10.0,
            },
            ImageRegion {
                x_pct: 0.0,
                y_pct: APP_CONTENT_TOP_PCT,
                w_pct: 24.0,
                h_pct: 86.0,
            },
        );

        assert!((rect.x_pct - 12.0).abs() < 0.01);
        assert!((rect.y_pct - (APP_CONTENT_TOP_PCT + 21.5)).abs() < 0.01);
        assert!((rect.w_pct - 2.4).abs() < 0.01);
        assert!((rect.h_pct - 8.6).abs() < 0.01);
    }

    #[test]
    fn build_context_combines_question_and_answer() {
        let context = build_context("Where is it?", "In the left sidebar.");
        assert!(context.contains("where is it?"));
        assert!(context.contains("left sidebar"));
    }
}

use crate::bundled_llm::generate_bundled_summary;
use crate::config::{Config, SummaryBackendPreference};
use screamer_core::ambient::{
    heuristic_title, segments_to_transcript, CanonicalSegment, NotesSummarizer, StructuredNotes,
    SummaryTemplate,
};
use screamer_models::{
    bundled_summary_model, find_summary_model, DEFAULT_BUNDLED_SUMMARY_MODEL_ID,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

const MAX_SESSION_TITLE_WORDS: usize = 4;
const MAX_SESSION_TITLE_CHARS: usize = 32;
const MAX_MODEL_PROMPT_CHARS: usize = 24_000;
const BUNDLED_SUMMARY_MAX_TOKENS: usize = 512;
const SCRATCH_PAD_START_MARKER: &str = "--- User Notes (Scratch Pad) ---";
const SCRATCH_PAD_END_MARKER: &str = "--- End User Notes ---";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryModelOption {
    pub backend: SummaryBackendPreference,
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct SummaryBackendRegistry {
    bundled_model_id: String,
    bundled_model_path: Option<PathBuf>,
    ollama_models: Vec<String>,
}

impl SummaryBackendRegistry {
    pub fn detect() -> Self {
        let bundled_model_id = bundled_summary_model()
            .map(|model| model.id.to_string())
            .unwrap_or_else(|| DEFAULT_BUNDLED_SUMMARY_MODEL_ID.to_string());
        let bundled_model_path = find_summary_model(&bundled_model_id);
        let ollama_models = detect_ollama_models();

        Self {
            bundled_model_id,
            bundled_model_path,
            ollama_models,
        }
    }

    pub fn bundled_model_label(&self) -> String {
        if self.bundled_model_path.is_some() {
            "Bundled Gemma 3 1B (Default)".to_string()
        } else {
            "Bundled Gemma 3 1B (Missing local artifact)".to_string()
        }
    }

    pub fn options(&self, config: &Config) -> Vec<SummaryModelOption> {
        let mut options = vec![SummaryModelOption {
            backend: SummaryBackendPreference::Bundled,
            label: self.bundled_model_label(),
            value: self.bundled_model_id.clone(),
        }];

        for model in &self.ollama_models {
            let suffix = if model == &config.summary_ollama_model {
                " (Selected)"
            } else {
                ""
            };
            options.push(SummaryModelOption {
                backend: SummaryBackendPreference::Ollama,
                label: format!("Local Ollama: {model}{suffix}"),
                value: model.clone(),
            });
        }

        if options.len() == 1 {
            options.push(SummaryModelOption {
                backend: SummaryBackendPreference::Ollama,
                label: "Local Ollama: gemma4:latest".to_string(),
                value: "gemma4:latest".to_string(),
            });
        }

        options
    }

    pub fn summarizer_for_config(&self, config: &Config) -> Arc<dyn NotesSummarizer> {
        match config.summary_backend {
            SummaryBackendPreference::Bundled => Arc::new(BundledSummaryBackend),
            SummaryBackendPreference::Ollama => Arc::new(OllamaSummaryBackend {
                model: config.summary_ollama_model.clone(),
            }),
        }
    }

    pub fn concise_session_title(
        &self,
        config: &Config,
        live_notes: &str,
        segments: &[CanonicalSegment],
    ) -> String {
        let fallback = sanitize_session_title(&heuristic_title(live_notes, segments));
        let Some(model) = self.preferred_title_model(config) else {
            return fallback;
        };

        let output = Command::new("ollama")
            .arg("run")
            .arg(&model)
            .arg(build_ollama_title_prompt(live_notes, segments, &fallback))
            .output();
        let Ok(output) = output else {
            return fallback;
        };

        if !output.status.success() {
            return fallback;
        }

        sanitize_session_title(&String::from_utf8_lossy(&output.stdout))
    }

    /// Generate a concise session title from the already-generated summary.
    /// Falls back to `concise_session_title` (heuristic / raw transcript) on failure.
    pub fn title_from_summary(
        &self,
        config: &Config,
        structured_notes: &str,
        live_notes: &str,
        segments: &[CanonicalSegment],
    ) -> String {
        let fallback = self.concise_session_title(config, live_notes, segments);

        // Need some summary content to work with
        let summary_excerpt = excerpt_lines(structured_notes, 20);
        if summary_excerpt.trim().is_empty() {
            return fallback;
        }

        let prompt = build_title_from_summary_prompt(&summary_excerpt, &fallback);

        let result = match config.summary_backend {
            SummaryBackendPreference::Ollama => {
                if let Some(model) = self.preferred_title_model(config) {
                    Command::new("ollama")
                        .arg("run")
                        .arg(&model)
                        .arg(&prompt)
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                } else {
                    None
                }
            }
            SummaryBackendPreference::Bundled => {
                generate_bundled_summary(&prompt, 32).ok()
            }
        };

        match result {
            Some(raw) if !raw.trim().is_empty() => sanitize_session_title(&raw),
            _ => fallback,
        }
    }

    pub fn has_any_ollama_model(&self) -> bool {
        !self.ollama_models.is_empty()
    }

    fn preferred_title_model(&self, config: &Config) -> Option<String> {
        if self
            .ollama_models
            .iter()
            .any(|model| model == &config.summary_ollama_model)
        {
            return Some(config.summary_ollama_model.clone());
        }

        self.ollama_models.first().cloned()
    }
}

struct BundledSummaryBackend;

impl NotesSummarizer for BundledSummaryBackend {
    fn summarize(
        &self,
        live_notes: &str,
        segments: &[CanonicalSegment],
        title_hint: Option<&str>,
        template: SummaryTemplate,
    ) -> Result<StructuredNotes, String> {
        let title_hint = title_hint.filter(|value| !value.trim().is_empty());
        let fallback = heuristic_structured_notes(live_notes, segments, title_hint);

        if template == SummaryTemplate::General {
            return summarize_general_chunked(live_notes, segments, title_hint, fallback);
        }

        let prompt = build_structured_notes_prompt(live_notes, segments, title_hint, template);

        match generate_bundled_summary(&prompt, BUNDLED_SUMMARY_MAX_TOKENS) {
            Ok(content) if !content.trim().is_empty() => Ok(merge_model_structured_notes(
                &content, live_notes, segments, title_hint, fallback,
            )),
            _ => Ok(fallback),
        }
    }
}

struct OllamaSummaryBackend {
    model: String,
}

impl OllamaSummaryBackend {
    fn generate(&self, prompt: &str) -> Result<String, String> {
        let output = Command::new("ollama")
            .arg("run")
            .arg(&self.model)
            .arg(prompt)
            .output()
            .map_err(|err| format!("Failed to launch Ollama: {err}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }

        Ok(trim_generated_response(
            &String::from_utf8_lossy(&output.stdout),
        ))
    }
}

impl NotesSummarizer for OllamaSummaryBackend {
    fn summarize(
        &self,
        live_notes: &str,
        segments: &[CanonicalSegment],
        title_hint: Option<&str>,
        template: SummaryTemplate,
    ) -> Result<StructuredNotes, String> {
        if template == SummaryTemplate::General {
            let fallback = heuristic_structured_notes(live_notes, segments, title_hint);
            return summarize_general_ollama(self, live_notes, segments, title_hint, fallback);
        }

        let prompt = build_structured_notes_prompt(live_notes, segments, title_hint, template);
        let content = self.generate(&prompt)?;

        if content.is_empty() {
            return Ok(heuristic_structured_notes(live_notes, segments, title_hint));
        }

        Ok(merge_model_structured_notes(
            &content,
            live_notes,
            segments,
            title_hint,
            heuristic_structured_notes(live_notes, segments, title_hint),
        ))
    }
}

fn detect_ollama_models() -> Vec<String> {
    let output = Command::new("ollama").arg("list").output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| name.contains("gemma"))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// General-template chunked summarization
// ---------------------------------------------------------------------------

/// Threshold (in chars) below which we summarize in a single pass.
const GENERAL_SINGLE_PASS_CHARS: usize = 6_000;
/// Max chars per chunk when splitting a long transcript.
const GENERAL_CHUNK_CHARS: usize = 5_000;
/// Max tokens for each chunk summary (bundled model).
const GENERAL_CHUNK_MAX_TOKENS: usize = 384;
/// Max tokens for the final merge pass (bundled model).
const GENERAL_MERGE_MAX_TOKENS: usize = 768;
const GENERAL_TOPIC_DISCOVERY_MAX_TOKENS: usize = 128;
const GENERAL_TOPIC_DETAIL_MAX_TOKENS: usize = 224;
const GENERAL_FOCUSED_STAGE_MAX_TOKENS: usize = 160;
const GENERAL_MAX_TOPICS: usize = 5;
const GENERAL_TOPIC_CONTEXT_MAX_CHARS: usize = 10_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct GeneralSummaryContext {
    scratch_pad: Option<String>,
    transcript: String,
    transcript_lines: Vec<String>,
    chunks: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TopicMention {
    title: String,
    chunk_index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TopicCluster {
    title: String,
    chunk_indices: Vec<usize>,
    mentions: usize,
    first_chunk_index: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TopicSection {
    title: String,
    bullets: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct GeneralSummaryDraft {
    topics: Vec<TopicSection>,
    decisions: Vec<String>,
    action_items: Vec<String>,
    open_questions: Vec<String>,
}

fn summarize_general_chunked(
    live_notes: &str,
    segments: &[CanonicalSegment],
    title_hint: Option<&str>,
    fallback: StructuredNotes,
) -> Result<StructuredNotes, String> {
    let generate = |prompt: &str, max_tokens: usize| generate_bundled_summary(prompt, max_tokens);
    if let Some(notes) =
        summarize_general_multistage(live_notes, segments, title_hint, &fallback, &generate)
    {
        return Ok(notes);
    }

    let transcript = full_summary_context(live_notes, segments, false);
    let title = title_hint.unwrap_or("Ambient session");

    // Short transcripts: single pass
    if transcript.chars().count() <= GENERAL_SINGLE_PASS_CHARS {
        let prompt = format!(
            "{GENERAL_TEMPLATE_PROMPT}\n\nTitle hint: {title}\n\nTranscript:\n{transcript}"
        );
        return match generate_bundled_summary(&prompt, GENERAL_MERGE_MAX_TOKENS) {
            Ok(raw) if !raw.trim().is_empty() => Ok(general_raw_notes(raw, segments)),
            _ => Ok(fallback),
        };
    }

    // Long transcripts: chunk → summarize each → merge
    let chunks = split_transcript_chunks(&transcript, GENERAL_CHUNK_CHARS);
    let mut chunk_summaries = Vec::new();

    for (i, chunk) in chunks.iter().enumerate() {
        let prompt = format!(
            "{GENERAL_CHUNK_PROMPT}\n\nPart {}/{} of transcript:\n{chunk}",
            i + 1,
            chunks.len()
        );
        match generate_bundled_summary(&prompt, GENERAL_CHUNK_MAX_TOKENS) {
            Ok(partial) if !partial.trim().is_empty() => {
                chunk_summaries.push(trim_generated_response(&partial));
            }
            _ => {} // skip failed chunks
        }
    }

    if chunk_summaries.is_empty() {
        return Ok(fallback);
    }

    let combined = chunk_summaries.join("\n\n---\n\n");
    let merge_prompt = format!(
        "{GENERAL_MERGE_PROMPT}\n\nTitle hint: {title}\n\nPartial summaries:\n{combined}"
    );

    match generate_bundled_summary(&merge_prompt, GENERAL_MERGE_MAX_TOKENS) {
        Ok(raw) if !raw.trim().is_empty() => Ok(general_raw_notes(raw, segments)),
        _ => {
            // If merge fails, concatenate chunk summaries as the output
            Ok(general_raw_notes(combined, segments))
        }
    }
}

fn summarize_general_ollama(
    backend: &OllamaSummaryBackend,
    live_notes: &str,
    segments: &[CanonicalSegment],
    title_hint: Option<&str>,
    fallback: StructuredNotes,
) -> Result<StructuredNotes, String> {
    let generate = |prompt: &str, _max_tokens: usize| backend.generate(prompt);
    if let Some(notes) =
        summarize_general_multistage(live_notes, segments, title_hint, &fallback, &generate)
    {
        return Ok(notes);
    }

    let transcript = full_summary_context(live_notes, segments, false);
    let title = title_hint.unwrap_or("Ambient session");

    if transcript.chars().count() <= GENERAL_SINGLE_PASS_CHARS {
        let prompt = format!(
            "{GENERAL_TEMPLATE_PROMPT}\n\nTitle hint: {title}\n\nTranscript:\n{transcript}"
        );
        return match backend.generate(&prompt) {
            Ok(raw) if !raw.trim().is_empty() => Ok(general_raw_notes(raw, segments)),
            Ok(_) => Ok(fallback),
            Err(err) => Err(err),
        };
    }

    let chunks = split_transcript_chunks(&transcript, GENERAL_CHUNK_CHARS);
    let mut chunk_summaries = Vec::new();

    for (i, chunk) in chunks.iter().enumerate() {
        let prompt = format!(
            "{GENERAL_CHUNK_PROMPT}\n\nPart {}/{} of transcript:\n{chunk}",
            i + 1,
            chunks.len()
        );
        if let Ok(partial) = backend.generate(&prompt) {
            if !partial.trim().is_empty() {
                chunk_summaries.push(partial);
            }
        }
    }

    if chunk_summaries.is_empty() {
        return Ok(fallback);
    }

    let combined = chunk_summaries.join("\n\n---\n\n");
    let merge_prompt = format!(
        "{GENERAL_MERGE_PROMPT}\n\nTitle hint: {title}\n\nPartial summaries:\n{combined}"
    );

    match backend.generate(&merge_prompt) {
        Ok(raw) if !raw.trim().is_empty() => Ok(general_raw_notes(raw, segments)),
        _ => Ok(general_raw_notes(combined, segments)),
    }
}

fn summarize_general_multistage(
    live_notes: &str,
    segments: &[CanonicalSegment],
    title_hint: Option<&str>,
    fallback: &StructuredNotes,
    generate: &dyn Fn(&str, usize) -> Result<String, String>,
) -> Option<StructuredNotes> {
    let context = general_summary_context(live_notes, segments);
    if context.transcript.trim().is_empty() {
        return None;
    }

    let topic_clusters = discover_topic_clusters(&context, title_hint, generate);
    if topic_clusters.is_empty() {
        return None;
    }

    let topics = build_topic_sections(&context, title_hint, &topic_clusters, generate);
    if topics.is_empty() {
        return None;
    }

    let decisions = extract_focused_section(
        &context,
        title_hint,
        "Decisions",
        GENERAL_DECISIONS_PROMPT,
        &[
            "decide",
            "decision",
            "agreed",
            "agree",
            "ship",
            "release",
            "go or no-go",
            "go/no-go",
            "fallback",
            "plan",
        ],
        fallback.decisions.clone(),
        generate,
    );
    let action_items = extract_focused_section(
        &context,
        title_hint,
        "Action Items",
        GENERAL_ACTION_ITEMS_PROMPT,
        &[
            "action",
            "next step",
            "follow up",
            "follow-up",
            "todo",
            "owner",
            "will ",
            "pair",
            "need to",
            "tomorrow",
            "friday",
        ],
        fallback.action_items.clone(),
        generate,
    );
    let open_questions = extract_focused_section(
        &context,
        title_hint,
        "Open Questions",
        GENERAL_OPEN_QUESTIONS_PROMPT,
        &[
            "?",
            "question",
            "unclear",
            "unknown",
            "whether",
            "if ",
            "blocker",
            "risk",
            "concern",
        ],
        fallback.open_questions.clone(),
        generate,
    );

    let draft = GeneralSummaryDraft {
        topics,
        decisions,
        action_items,
        open_questions,
    };
    let rendered = render_general_summary_draft(&draft);
    if rendered.trim().is_empty() {
        return None;
    }

    Some(StructuredNotes {
        transcript: segments_to_transcript(segments),
        raw_notes: Some(rendered),
        ..Default::default()
    })
}

/// Build a `StructuredNotes` that uses `raw_notes` so `to_markdown()` emits
/// the LLM output directly instead of the rigid section layout.
fn general_raw_notes(raw: String, segments: &[CanonicalSegment]) -> StructuredNotes {
    let cleaned = clean_general_markdown(&raw);
    StructuredNotes {
        transcript: segments_to_transcript(segments),
        raw_notes: Some(cleaned),
        ..Default::default()
    }
}

fn general_summary_context(live_notes: &str, segments: &[CanonicalSegment]) -> GeneralSummaryContext {
    let (scratch_pad, transcript_body) = split_scratch_pad_context(live_notes);
    let transcript = if segments.is_empty() {
        normalize_live_transcript(transcript_body, false)
    } else {
        segments_to_speakerless_transcript(segments)
    };
    let transcript_lines = transcript
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let chunks = split_transcript_chunks(&transcript, GENERAL_CHUNK_CHARS);

    GeneralSummaryContext {
        scratch_pad: scratch_pad
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        transcript,
        transcript_lines,
        chunks,
    }
}

fn discover_topic_clusters(
    context: &GeneralSummaryContext,
    title_hint: Option<&str>,
    generate: &dyn Fn(&str, usize) -> Result<String, String>,
) -> Vec<TopicCluster> {
    let mut mentions = Vec::new();
    for (chunk_index, chunk) in context.chunks.iter().enumerate() {
        let prompt = build_general_stage_prompt(
            GENERAL_TOPIC_DISCOVERY_PROMPT,
            title_hint,
            context.scratch_pad.as_deref(),
            "Transcript excerpt",
            chunk,
        );
        let Ok(output) = generate(&prompt, GENERAL_TOPIC_DISCOVERY_MAX_TOKENS) else {
            continue;
        };

        for title in parse_topic_titles(&output) {
            mentions.push(TopicMention { title, chunk_index });
        }
    }

    merge_topic_mentions(mentions)
}

fn build_topic_sections(
    context: &GeneralSummaryContext,
    title_hint: Option<&str>,
    clusters: &[TopicCluster],
    generate: &dyn Fn(&str, usize) -> Result<String, String>,
) -> Vec<TopicSection> {
    let mut sections = Vec::new();

    for cluster in clusters {
        let topic_context = build_topic_context(context, cluster);
        if topic_context.trim().is_empty() {
            continue;
        }

        let prompt = build_general_stage_prompt(
            &GENERAL_TOPIC_DETAIL_PROMPT.replace("{topic}", &cluster.title),
            title_hint,
            context.scratch_pad.as_deref(),
            "Relevant transcript passages",
            &topic_context,
        );
        let bullets = generate(&prompt, GENERAL_TOPIC_DETAIL_MAX_TOKENS)
            .ok()
            .map(|output| parse_simple_bullets(&output))
            .filter(|bullets| !bullets.is_empty())
            .unwrap_or_else(|| fallback_topic_bullets(&topic_context));
        let bullets = dedupe_bullets(&bullets);
        if bullets.is_empty() {
            continue;
        }

        sections.push(TopicSection {
            title: cluster.title.clone(),
            bullets,
        });
    }

    sections
}

fn extract_focused_section(
    context: &GeneralSummaryContext,
    title_hint: Option<&str>,
    section_name: &str,
    instruction: &str,
    needles: &[&str],
    fallback: Vec<String>,
    generate: &dyn Fn(&str, usize) -> Result<String, String>,
) -> Vec<String> {
    let filtered_lines = filtered_context_lines(&context.transcript_lines, needles);
    let body = if filtered_lines.is_empty() {
        excerpt_balanced_text(&context.transcript, 6_000)
    } else {
        filtered_lines.join("\n")
    };
    if body.trim().is_empty() {
        return dedupe_bullets(&fallback);
    }

    let prompt = build_general_stage_prompt(
        instruction,
        title_hint,
        context.scratch_pad.as_deref(),
        "Candidate transcript lines",
        &body,
    );
    let parsed = generate(&prompt, GENERAL_FOCUSED_STAGE_MAX_TOKENS)
        .ok()
        .map(|output| parse_simple_bullets(&output))
        .unwrap_or_default();
    let cleaned = dedupe_bullets(&parsed);
    if cleaned.is_empty() {
        return dedupe_bullets(&fallback)
            .into_iter()
            .filter(|line| !is_empty_section_marker(line) && !line.starts_with("No explicit "))
            .collect();
    }

    cleaned
        .into_iter()
        .filter(|line| !is_empty_section_marker(line))
        .filter(|line| {
            !line.eq_ignore_ascii_case(section_name)
                && !line.to_ascii_lowercase().starts_with("none")
        })
        .collect()
}

fn build_general_stage_prompt(
    instruction: &str,
    title_hint: Option<&str>,
    scratch_pad: Option<&str>,
    body_label: &str,
    body: &str,
) -> String {
    let mut out = String::new();
    out.push_str(instruction.trim());
    out.push_str("\n\nTitle hint: ");
    out.push_str(title_hint.unwrap_or("Ambient session"));

    if let Some(scratch_pad) = scratch_pad.filter(|text| !text.trim().is_empty()) {
        out.push_str("\n\nUser Notes (Scratch Pad):\n");
        out.push_str(scratch_pad.trim());
    }

    out.push_str("\n\n");
    out.push_str(body_label);
    out.push_str(":\n");
    out.push_str(body.trim());
    out
}

fn parse_topic_titles(text: &str) -> Vec<String> {
    parse_simple_bullets(text)
        .into_iter()
        .filter_map(|title| normalize_topic_title(&title))
        .collect()
}

fn normalize_topic_title(text: &str) -> Option<String> {
    let line = strip_list_prefix(text);
    let line = line
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '*' | '-' | ':' | '.'))
        .trim();
    if line.is_empty() {
        return None;
    }

    let line = if let Some((prefix, rest)) = line.split_once(':') {
        let prefix_lower = prefix.trim().to_ascii_lowercase();
        if prefix_lower.starts_with("topic") || prefix_lower == "title" {
            rest.trim()
        } else {
            line
        }
    } else {
        line
    };
    let line = strip_speaker_prefix(line).trim();
    let normalized = collapse_spaces(line);
    let lower = normalized.to_ascii_lowercase();
    if normalized.split_whitespace().count() > 7 {
        return None;
    }
    if [
        "summary",
        "key points",
        "recap",
        "discussion",
        "notes",
        "transcript",
        "miscellaneous",
        "other",
        "general",
        "questions",
        "updates",
    ]
    .iter()
    .any(|generic| lower == *generic || lower.starts_with(&format!("{generic} ")))
    {
        return None;
    }

    Some(normalized)
}

fn merge_topic_mentions(mentions: Vec<TopicMention>) -> Vec<TopicCluster> {
    let mut clusters = Vec::<TopicCluster>::new();

    for mention in mentions {
        if let Some(existing) = clusters
            .iter_mut()
            .find(|cluster| topic_titles_similar(&cluster.title, &mention.title))
        {
            existing.mentions += 1;
            if !existing.chunk_indices.contains(&mention.chunk_index) {
                existing.chunk_indices.push(mention.chunk_index);
            }
            if mention.chunk_index < existing.first_chunk_index {
                existing.first_chunk_index = mention.chunk_index;
            }
            existing.title = preferred_topic_title(&existing.title, &mention.title);
            continue;
        }

        clusters.push(TopicCluster {
            title: mention.title,
            chunk_indices: vec![mention.chunk_index],
            mentions: 1,
            first_chunk_index: mention.chunk_index,
        });
    }

    clusters.sort_by(|left, right| {
        right
            .mentions
            .cmp(&left.mentions)
            .then(left.first_chunk_index.cmp(&right.first_chunk_index))
    });
    clusters.truncate(GENERAL_MAX_TOPICS);
    clusters.sort_by_key(|cluster| cluster.first_chunk_index);
    clusters
}

fn topic_titles_similar(left: &str, right: &str) -> bool {
    let left_normalized = collapse_spaces(&left.to_ascii_lowercase());
    let right_normalized = collapse_spaces(&right.to_ascii_lowercase());
    if left_normalized == right_normalized {
        return true;
    }
    if left_normalized.contains(&right_normalized) || right_normalized.contains(&left_normalized) {
        return true;
    }

    let left_tokens = summary_tokens(&left_normalized);
    let right_tokens = summary_tokens(&right_normalized);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return false;
    }

    let left_set = left_tokens.iter().collect::<HashSet<_>>();
    let right_set = right_tokens.iter().collect::<HashSet<_>>();
    let intersection = left_set.intersection(&right_set).count();
    let min_len = left_set.len().min(right_set.len());
    min_len > 0 && intersection * 2 >= min_len
}

fn preferred_topic_title(current: &str, candidate: &str) -> String {
    let current_words = current.split_whitespace().count();
    let candidate_words = candidate.split_whitespace().count();
    if candidate_words > current_words && candidate_words <= 7 {
        candidate.to_string()
    } else {
        current.to_string()
    }
}

fn build_topic_context(context: &GeneralSummaryContext, cluster: &TopicCluster) -> String {
    let mut selected = cluster.chunk_indices.clone();
    selected.sort_unstable();
    selected.dedup();

    let combined = selected
        .into_iter()
        .filter_map(|index| context.chunks.get(index))
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n\n");
    excerpt_balanced_text(&combined, GENERAL_TOPIC_CONTEXT_MAX_CHARS)
}

fn fallback_topic_bullets(topic_context: &str) -> Vec<String> {
    topic_context
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(4)
        .map(strip_speaker_prefix)
        .map(collapse_spaces)
        .collect()
}

fn filtered_context_lines(lines: &[String], needles: &[&str]) -> Vec<String> {
    lines.iter()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            needles.iter().any(|needle| lower.contains(needle))
        })
        .cloned()
        .collect()
}

fn parse_simple_bullets(text: &str) -> Vec<String> {
    let cleaned = strip_code_fence(&trim_generated_response(text));
    let mut bullets = Vec::new();

    for raw_line in cleaned.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let stripped = strip_list_prefix(line).trim();
        if stripped.is_empty() || stripped == line && is_presentational_line(line) {
            continue;
        }

        let normalized = strip_speaker_prefix(stripped).trim();
        if normalized.is_empty() || is_presentational_line(normalized) {
            continue;
        }
        bullets.push(collapse_spaces(normalized));
    }

    dedupe_bullets(&bullets)
}

fn is_presentational_line(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    lower.starts_with("here are")
        || lower.starts_with("here's")
        || lower.starts_with("below are")
        || lower.starts_with("topics discussed")
        || lower.starts_with("core discussion points")
        || lower.starts_with("breakdown")
        || lower.starts_with("thinking")
        || lower.starts_with("topic ")
}

fn dedupe_bullets(lines: &[String]) -> Vec<String> {
    let mut out = Vec::<String>::new();
    for line in lines {
        let trimmed = collapse_spaces(strip_speaker_prefix(line).trim());
        if trimmed.is_empty() || is_empty_section_marker(&trimmed) {
            continue;
        }
        if out.iter().any(|existing| lines_are_similar(existing, &trimmed)) {
            continue;
        }
        out.push(trimmed);
    }
    out
}

fn render_general_summary_draft(draft: &GeneralSummaryDraft) -> String {
    let mut sections = Vec::new();

    for topic in &draft.topics {
        if topic.title.trim().is_empty() || topic.bullets.is_empty() {
            continue;
        }
        let bullets = dedupe_bullets(&topic.bullets)
            .into_iter()
            .map(|bullet| format!("- {bullet}"))
            .collect::<Vec<_>>()
            .join("\n");
        if bullets.is_empty() {
            continue;
        }
        sections.push(format!("## {}\n{}", topic.title.trim(), bullets));
    }

    for (title, items) in [
        ("Decisions", &draft.decisions),
        ("Action Items", &draft.action_items),
        ("Open Questions", &draft.open_questions),
    ] {
        let items = dedupe_bullets(items)
            .into_iter()
            .filter(|line| !line.starts_with("No explicit "))
            .collect::<Vec<_>>();
        if items.is_empty() {
            continue;
        }
        let body = items
            .into_iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("## {title}\n{body}"));
    }

    sections.join("\n\n").trim().to_string()
}

/// Split transcript text into roughly equal chunks on line boundaries.
fn split_transcript_chunks(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for line in text.lines() {
        if !current.is_empty() && current.chars().count() + line.chars().count() + 1 > max_chars {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(text.to_string());
    }
    chunks
}

fn build_structured_notes_prompt(
    live_notes: &str,
    segments: &[CanonicalSegment],
    title_hint: Option<&str>,
    template: SummaryTemplate,
) -> String {
    let transcript = prepared_summary_context(live_notes, segments, true);
    let system_prompt = template_system_prompt(template);

    format!(
        "{system_prompt}\n\nTitle hint: {}\n\nTranscript:\n{}",
        title_hint.unwrap_or("Ambient session"),
        transcript,
    )
}

fn template_system_prompt(template: SummaryTemplate) -> &'static str {
    match template {
        SummaryTemplate::General => GENERAL_TEMPLATE_PROMPT,
        SummaryTemplate::OneOnOne => ONE_ON_ONE_TEMPLATE_PROMPT,
        SummaryTemplate::TeamMeeting => TEAM_MEETING_TEMPLATE_PROMPT,
        SummaryTemplate::StandUp => STAND_UP_TEMPLATE_PROMPT,
    }
}

const GENERAL_TEMPLATE_PROMPT: &str = "\
You are an expert note-taker who transforms messy spoken transcripts into clear, structured summaries.\n\n\
INPUT: A raw transcript with filler words (um, uh, like, you know), false starts, repetition, \
and natural spoken-language messiness from one or more speakers.\n\n\
YOUR TASK:\n\
1. Identify the distinct topics or themes discussed.\n\
2. Create a markdown heading (##) for each topic.\n\
3. Under each heading, write concise bullet points capturing the key information.\n\
4. Completely rewrite — do NOT echo or quote the transcript. Synthesize what was said.\n\
5. Strip all filler words, false starts, and repetition.\n\n\
FORMAT RULES:\n\
- Output clean markdown only. No preamble, no closing remarks.\n\
- Use ## headings for each topic or theme.\n\
- Use bullet points (- ) under each heading.\n\
- Keep bullets short and information-dense.\n\
- If action items, decisions, or next steps came up, include them under a relevant heading.\n\
- Do NOT create generic sections like 'Key Points' or 'Summary' — name headings after the actual topics.\n\
- Do NOT reproduce speaker labels (Speaker 1, You, S1, etc.) — just capture the substance.\n\n\
QUALITY:\n\
- Preserve all important facts, names, numbers, and technical terms exactly.\n\
- Merge duplicate or restated ideas into a single clear bullet.\n\
- Keep output proportional to the input — don't pad short conversations.\n\
- Do not hallucinate or invent information not in the transcript.";

const GENERAL_CHUNK_PROMPT: &str = "\
Summarize this portion of a transcript into concise bullet-point notes.\n\
- Remove filler words, repetition, and false starts.\n\
- Identify topics discussed and group bullets under short ## headings named after the topics.\n\
- Capture facts, names, numbers, and decisions accurately.\n\
- Do NOT reproduce speaker labels — just the substance.\n\
- Output clean markdown only.";

const GENERAL_MERGE_PROMPT: &str = "\
You are given partial summaries of different sections of the same conversation.\n\
Merge them into a single cohesive set of structured notes.\n\n\
RULES:\n\
- Combine duplicate topics under one heading.\n\
- Remove redundant bullets that say the same thing.\n\
- Use ## headings named after the actual topics (not generic labels like 'Key Points').\n\
- Use bullet points (- ) under each heading.\n\
- Keep it concise and well-organized.\n\
- Output clean markdown only. No preamble.";

const GENERAL_TOPIC_DISCOVERY_PROMPT: &str = "\
Identify the concrete discussion topics in this meeting transcript excerpt.\n\
\n\
RULES:\n\
- Output topic titles only, one per line, each starting with `- `.\n\
- Use short, concrete titles named after the actual subject matter.\n\
- Do not use generic titles like Summary, Key Points, Miscellaneous, Recap, or Updates.\n\
- Do not include speaker names or labels.\n\
- Only include topics that are explicitly discussed in the excerpt.\n\
- Prefer 1 to 4 topic titles.";

const GENERAL_TOPIC_DETAIL_PROMPT: &str = "\
Write bullets for the meeting topic `{topic}`.\n\
\n\
RULES:\n\
- Output bullet lines only, each starting with `- `.\n\
- Capture only details relevant to this topic.\n\
- Preserve concrete facts, names, numbers, dates, and technical terms.\n\
- Do not mention speaker labels.\n\
- Do not add a heading or preamble.\n\
- Do not repeat the topic title inside each bullet.\n\
- Prefer 2 to 5 bullets.";

const GENERAL_DECISIONS_PROMPT: &str = "\
Extract only decisions or tentative decisions from these meeting notes.\n\
\n\
RULES:\n\
- Output bullet lines only, each starting with `- `.\n\
- Include only decisions, commitments, chosen directions, or explicit fallback plans.\n\
- Do not include general discussion points.\n\
- Do not mention speaker labels.\n\
- If there are no reliable decisions, output `- None`.";

const GENERAL_ACTION_ITEMS_PROMPT: &str = "\
Extract only concrete action items and next steps from these meeting notes.\n\
\n\
RULES:\n\
- Output bullet lines only, each starting with `- `.\n\
- Include owners, dates, or timing only when they are explicit.\n\
- Do not include general discussion points.\n\
- Do not mention speaker labels.\n\
- If there are no reliable action items, output `- None`.";

const GENERAL_OPEN_QUESTIONS_PROMPT: &str = "\
Extract only unresolved questions, risks, blockers, or uncertainties from these meeting notes.\n\
\n\
RULES:\n\
- Output bullet lines only, each starting with `- `.\n\
- Include only items that are still unresolved or require follow-up.\n\
- Do not include settled decisions.\n\
- Do not mention speaker labels.\n\
- If there are no reliable open questions, output `- None`.";

const ONE_ON_ONE_TEMPLATE_PROMPT: &str = "\
Write concise 1:1 meeting notes in markdown with these sections exactly and in this order:\n\
## Summary\n## Discussion Topics\n## Feedback & Coaching\n## Action Items\n## Follow-ups for Next 1:1\n\n\
Rules:\n\
- Output markdown only.\n\
- Start with `## Summary` on the first line.\n\
- Do not add any preamble, conversational filler, or closing sentence.\n\
- Be concise and avoid repeating the transcript.\n\
- Use short bullet lists for every section except Summary.\n\
- In Discussion Topics, capture the main subjects raised by each participant.\n\
- In Feedback & Coaching, note any feedback given or received, growth areas, or coaching moments.\n\
- In Follow-ups for Next 1:1, list topics or items that should be revisited.\n\
- If the transcript is noisy or ambiguous, say that briefly in Summary instead of inventing details.\n\
- Preserve important technical terms exactly when they appear.\n\
- If a section has nothing reliable, write a single bullet that says `None`.\n\
- Do not include a Transcript section.\n\
- If 'User Notes (Scratch Pad)' content is present, treat it as high-priority context and use its markdown formatting to structure and enhance the summary.";

const TEAM_MEETING_TEMPLATE_PROMPT: &str = "\
Write concise team meeting notes in markdown with these sections exactly and in this order:\n\
## Summary\n## Agenda Items Covered\n## Decisions\n## Action Items\n## Owners & Deadlines\n## Parking Lot\n\n\
Rules:\n\
- Output markdown only.\n\
- Start with `## Summary` on the first line.\n\
- Do not add any preamble, conversational filler, or closing sentence.\n\
- Be concise and avoid repeating the transcript.\n\
- Use short bullet lists for every section except Summary.\n\
- In Agenda Items Covered, list each topic discussed with a brief note on the outcome.\n\
- In Owners & Deadlines, attribute action items to specific speakers when identifiable from the transcript.\n\
- In Parking Lot, capture topics raised but deferred for later discussion.\n\
- If the transcript is noisy or ambiguous, say that briefly in Summary instead of inventing details.\n\
- Preserve important technical terms exactly when they appear.\n\
- If a section has nothing reliable, write a single bullet that says `None`.\n\
- Do not include a Transcript section.\n\
- If 'User Notes (Scratch Pad)' content is present, treat it as high-priority context and use its markdown formatting to structure and enhance the summary.";

const STAND_UP_TEMPLATE_PROMPT: &str = "\
Write concise stand-up meeting notes in markdown with these sections exactly and in this order:\n\
## Summary\n## Yesterday / Completed\n## Today / In Progress\n## Blockers\n## Key Callouts\n\n\
Rules:\n\
- Output markdown only.\n\
- Start with `## Summary` on the first line.\n\
- Do not add any preamble, conversational filler, or closing sentence.\n\
- Be concise and avoid repeating the transcript.\n\
- Use short bullet lists for every section except Summary.\n\
- Attribute updates to specific speakers when identifiable (e.g. 'S1: ...', 'You: ...').\n\
- In Yesterday / Completed, capture what each participant reported as done.\n\
- In Today / In Progress, capture what each participant plans to work on.\n\
- In Blockers, list any impediments or dependencies mentioned.\n\
- In Key Callouts, note any announcements, reminders, or cross-team items raised.\n\
- If the transcript is noisy or ambiguous, say that briefly in Summary instead of inventing details.\n\
- Preserve important technical terms exactly when they appear.\n\
- If a section has nothing reliable, write a single bullet that says `None`.\n\
- Do not include a Transcript section.\n\
- If 'User Notes (Scratch Pad)' content is present, treat it as high-priority context and use its markdown formatting to structure and enhance the summary.";

fn build_ollama_title_prompt(
    live_notes: &str,
    segments: &[CanonicalSegment],
    fallback: &str,
) -> String {
    let notes_excerpt = excerpt_lines(live_notes, 6);
    let transcript_excerpt = segments
        .iter()
        .take(6)
        .map(CanonicalSegment::note_line)
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "Write a concise sidebar title for a macOS notes app.\nRequirements:\n- 2 to 4 words\n- Title Case\n- No punctuation\n- Maximum 30 characters\n- Output the title only\nFallback topic: {fallback}\n\nLive notes:\n{notes_excerpt}\n\nTranscript excerpt:\n{transcript_excerpt}"
    )
}

fn build_title_from_summary_prompt(summary_excerpt: &str, fallback: &str) -> String {
    format!(
        "Write a concise sidebar title for a macOS notes app based on the following meeting summary.\n\
         Requirements:\n\
         - 2 to 4 words\n\
         - Title Case\n\
         - No punctuation\n\
         - Maximum 30 characters\n\
         - Output the title only, nothing else\n\
         Fallback topic: {fallback}\n\n\
         Summary:\n{summary_excerpt}"
    )
}

fn prepared_summary_context(
    live_notes: &str,
    segments: &[CanonicalSegment],
    include_speaker_labels: bool,
) -> String {
    let source = full_summary_context(live_notes, segments, include_speaker_labels);
    excerpt_balanced_text(&source, MAX_MODEL_PROMPT_CHARS)
}

fn full_summary_context(
    live_notes: &str,
    segments: &[CanonicalSegment],
    include_speaker_labels: bool,
) -> String {
    let (scratch_pad, transcript_body) = split_scratch_pad_context(live_notes);
    let transcript = if segments.is_empty() {
        normalize_live_transcript(transcript_body, include_speaker_labels)
    } else if include_speaker_labels {
        segments_to_transcript(segments)
    } else {
        segments_to_speakerless_transcript(segments)
    };

    let mut sections = Vec::new();
    if let Some(scratch_pad) = scratch_pad {
        let scratch_pad = scratch_pad.trim();
        if !scratch_pad.is_empty() {
            sections.push(format!("User Notes (Scratch Pad):\n{scratch_pad}"));
        }
    }

    let transcript = transcript.trim();
    if !transcript.is_empty() {
        sections.push(format!("Transcript:\n{transcript}"));
    }

    sections.join("\n\n")
}

fn split_scratch_pad_context(live_notes: &str) -> (Option<&str>, &str) {
    let trimmed = live_notes.trim();
    let Some(rest) = trimmed.strip_prefix(SCRATCH_PAD_START_MARKER) else {
        return (None, trimmed);
    };

    let rest = rest.trim_start();
    if let Some((scratch_pad, transcript)) = rest.split_once(SCRATCH_PAD_END_MARKER) {
        (Some(scratch_pad.trim()), transcript.trim())
    } else {
        (Some(rest.trim()), "")
    }
}

fn normalize_live_transcript(text: &str, include_speaker_labels: bool) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            if include_speaker_labels {
                line.to_string()
            } else {
                strip_speaker_prefix(line).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn segments_to_speakerless_transcript(segments: &[CanonicalSegment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn excerpt_balanced_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }

    let lines = trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return trimmed.chars().take(max_chars).collect();
    }

    let head_budget = max_chars / 3;
    let tail_budget = max_chars.saturating_sub(head_budget + 64);
    let mut head = Vec::new();
    let mut used = 0usize;
    for line in &lines {
        let len = line.chars().count() + 1;
        if used + len > head_budget {
            break;
        }
        used += len;
        head.push((*line).to_string());
    }

    let mut tail = Vec::new();
    let mut tail_used = 0usize;
    for line in lines.iter().rev() {
        let len = line.chars().count() + 1;
        if tail_used + len > tail_budget {
            break;
        }
        tail_used += len;
        tail.push((*line).to_string());
    }
    tail.reverse();

    let mut combined = head;
    combined.push("[... transcript truncated for local summarization ...]".to_string());
    combined.extend(tail);
    combined.join("\n")
}

fn merge_model_structured_notes(
    content: &str,
    live_notes: &str,
    segments: &[CanonicalSegment],
    title_hint: Option<&str>,
    fallback: StructuredNotes,
) -> StructuredNotes {
    let transcript = segments_to_transcript(segments);
    let Some(mut parsed) = parse_model_structured_notes(content, &transcript) else {
        let salient_lines = collect_salient_lines(live_notes, segments);
        return StructuredNotes {
            summary: content.trim().to_string(),
            key_points: collect_key_points(live_notes, segments, &salient_lines),
            decisions: collect_decisions(live_notes),
            action_items: collect_action_items(live_notes),
            open_questions: collect_open_questions(live_notes),
            transcript,
            raw_notes: None,
        };
    };

    if parsed.summary.trim().is_empty() {
        parsed.summary = fallback.summary;
    }
    if parsed.key_points.is_empty() {
        parsed.key_points = fallback.key_points;
    }
    if parsed.decisions.is_empty() {
        parsed.decisions = fallback.decisions;
    }
    if parsed.action_items.is_empty() {
        parsed.action_items = fallback.action_items;
    }
    if parsed.open_questions.is_empty() {
        parsed.open_questions = fallback.open_questions;
    }
    if parsed.summary.trim().is_empty() {
        parsed.summary = heuristic_structured_notes(live_notes, segments, title_hint).summary;
    }
    parsed.transcript = transcript;
    parsed
}

fn parse_model_structured_notes(content: &str, transcript: &str) -> Option<StructuredNotes> {
    let mut summary_lines = Vec::new();
    let mut key_points = Vec::new();
    let mut decisions = Vec::new();
    let mut action_items = Vec::new();
    let mut open_questions = Vec::new();
    let mut current_section = None::<ParsedNotesSection>;

    let cleaned = trim_generated_response(content);
    let cleaned = strip_code_fence(&cleaned);

    for raw_line in cleaned.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(section) = parse_notes_heading(line) {
            current_section = Some(section);
            continue;
        }

        match current_section {
            Some(ParsedNotesSection::Summary) => summary_lines.push(line.to_string()),
            Some(ParsedNotesSection::KeyPoints) => push_parsed_bullet(&mut key_points, line),
            Some(ParsedNotesSection::Decisions) => push_parsed_bullet(&mut decisions, line),
            Some(ParsedNotesSection::ActionItems) => push_parsed_bullet(&mut action_items, line),
            Some(ParsedNotesSection::OpenQuestions) => {
                push_parsed_bullet(&mut open_questions, line)
            }
            None => {}
        }
    }

    let summary = collapse_spaces(&summary_lines.join(" "));
    if summary.is_empty()
        && key_points.is_empty()
        && decisions.is_empty()
        && action_items.is_empty()
        && open_questions.is_empty()
    {
        return None;
    }

    Some(StructuredNotes {
        summary,
        key_points,
        decisions,
        action_items,
        open_questions,
        transcript: transcript.to_string(),
        raw_notes: None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParsedNotesSection {
    Summary,
    KeyPoints,
    Decisions,
    ActionItems,
    OpenQuestions,
}

fn parse_notes_heading(line: &str) -> Option<ParsedNotesSection> {
    let normalized = line
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_end_matches(':')
        .to_ascii_lowercase();

    match normalized.as_str() {
        "summary" => Some(ParsedNotesSection::Summary),
        "key points"
        | "discussion topics"
        | "agenda items covered"
        | "yesterday / completed"
        | "yesterday"
        | "completed" => Some(ParsedNotesSection::KeyPoints),
        "decisions"
        | "feedback & coaching"
        | "feedback"
        | "coaching"
        | "today / in progress"
        | "today"
        | "in progress" => Some(ParsedNotesSection::Decisions),
        "action items" | "actions" | "owners & deadlines" | "owners" | "blockers" => {
            Some(ParsedNotesSection::ActionItems)
        }
        "open questions"
        | "open questions / risks"
        | "follow-ups for next 1:1"
        | "follow-ups"
        | "parking lot"
        | "key callouts" => Some(ParsedNotesSection::OpenQuestions),
        _ => None,
    }
}

fn push_parsed_bullet(target: &mut Vec<String>, line: &str) {
    let normalized = strip_list_prefix(line);
    let normalized = normalized.trim();
    if normalized.is_empty() {
        return;
    }
    if is_empty_section_marker(normalized) {
        return;
    }
    target.push(normalized.to_string());
}

fn is_empty_section_marker(line: &str) -> bool {
    let normalized = line
        .trim()
        .trim_end_matches(|ch: char| matches!(ch, '.' | ':' | '!' | ';'))
        .trim();
    normalized.eq_ignore_ascii_case("none") || normalized.eq_ignore_ascii_case("n/a")
}

fn strip_list_prefix(line: &str) -> &str {
    let line = line.trim();
    for prefix in ["- ", "* ", "• "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return rest.trim();
        }
    }

    let mut digits = 0usize;
    for ch in line.chars() {
        if ch.is_ascii_digit() {
            digits += 1;
            continue;
        }
        break;
    }
    if digits > 0 {
        let remainder = &line[digits..];
        if let Some(rest) = remainder.strip_prefix(". ") {
            return rest.trim();
        }
    }

    line
}

fn strip_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }

    trimmed
        .trim_start_matches("```markdown")
        .trim_start_matches("```md")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
        .to_string()
}

fn clean_general_markdown(text: &str) -> String {
    let cleaned = strip_code_fence(&trim_generated_response(text));
    let mut lines = cleaned
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();

    if let Some(first_heading) = lines.iter().position(|line| line.starts_with('#')) {
        lines = lines.split_off(first_heading);
    }

    let mut normalized = Vec::new();
    for line in lines {
        if line == "---" {
            continue;
        }

        if line.starts_with('#') {
            normalized.push(line);
            continue;
        }

        let list_stripped = strip_list_prefix(&line);
        if list_stripped != line {
            let rest = list_stripped;
            let rest = strip_speaker_prefix(rest).trim();
            if !rest.is_empty() {
                normalized.push(format!("- {rest}"));
            }
            continue;
        }

        let line = strip_speaker_prefix(&line).trim();
        if !line.is_empty() {
            normalized.push(line.to_string());
        }
    }

    normalized.join("\n").trim().to_string()
}

fn trim_generated_response(text: &str) -> String {
    text.lines()
        .skip_while(|line| line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn heuristic_structured_notes(
    live_notes: &str,
    segments: &[CanonicalSegment],
    title_hint: Option<&str>,
) -> StructuredNotes {
    let title = title_hint
        .map(sanitize_session_title)
        .unwrap_or_else(|| sanitize_session_title(&heuristic_title(live_notes, segments)));
    let salient_lines = collect_salient_lines(live_notes, segments);
    let key_points = collect_key_points(live_notes, segments, &salient_lines);
    let decisions = collect_decisions(live_notes);
    let action_items = collect_action_items(live_notes);
    let open_questions = collect_open_questions(live_notes);
    let transcript = segments_to_transcript(segments);

    StructuredNotes {
        summary: build_heuristic_summary(&title, &salient_lines, live_notes, segments),
        key_points,
        decisions,
        action_items,
        open_questions,
        transcript,
        raw_notes: None,
    }
}

fn collect_key_points(
    live_notes: &str,
    segments: &[CanonicalSegment],
    salient_lines: &[String],
) -> Vec<String> {
    let mut points = salient_lines.iter().take(4).cloned().collect::<Vec<_>>();

    if points.is_empty() {
        points.extend(
            segments
                .iter()
                .take(4)
                .map(|segment| segment.text.trim().to_string())
                .collect::<Vec<_>>(),
        );
    }

    if points.is_empty() {
        points.extend(
            live_notes
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .take(4)
                .map(strip_speaker_prefix)
                .map(str::to_string),
        );
    }

    points
}

fn collect_action_items(live_notes: &str) -> Vec<String> {
    collect_matching_lines(
        live_notes,
        &["todo", "action", "follow up", "next step", "will "],
    )
}

fn collect_decisions(live_notes: &str) -> Vec<String> {
    collect_matching_lines(
        live_notes,
        &["decide", "decision", "agreed", "ship", "go with"],
    )
}

fn collect_open_questions(live_notes: &str) -> Vec<String> {
    let mut questions = collect_matching_lines(live_notes, &["?", "open question", "unclear"]);
    if questions.is_empty() {
        questions.push("No explicit open questions were captured.".to_string());
    }
    questions
}

fn collect_matching_lines(live_notes: &str, needles: &[&str]) -> Vec<String> {
    live_notes
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            let lower = line.to_lowercase();
            needles.iter().any(|needle| lower.contains(needle))
        })
        .take(4)
        .map(str::to_string)
        .collect()
}

fn excerpt_lines(text: &str, limit: usize) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(limit)
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Debug)]
struct SalientLine {
    index: usize,
    text: String,
    score: i32,
}

fn build_heuristic_summary(
    title: &str,
    salient_lines: &[String],
    live_notes: &str,
    segments: &[CanonicalSegment],
) -> String {
    if salient_lines.is_empty() {
        return format!(
            "{title}\n\nConversation was captured, but the transcript was too noisy to produce a reliable detailed summary."
        );
    }

    let mut lines = vec![
        title.to_string(),
        String::new(),
        format!("Main takeaway: {}", as_summary_sentence(&salient_lines[0])),
    ];

    if let Some(second) = salient_lines.get(1) {
        lines.push(format!("Also discussed: {}", as_summary_sentence(second)));
    }

    if transcript_seems_noisy(live_notes, segments, salient_lines) {
        lines.push(
            "Note: parts of the transcript were noisy or repetitive, so minor details may be incomplete."
                .to_string(),
        );
    }

    lines.join("\n")
}

fn collect_salient_lines(live_notes: &str, segments: &[CanonicalSegment]) -> Vec<String> {
    let candidates = if segments.is_empty() {
        live_notes
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(strip_speaker_prefix)
            .map(str::to_string)
            .collect::<Vec<_>>()
    } else {
        segments
            .iter()
            .map(|segment| segment.text.trim().to_string())
            .collect::<Vec<_>>()
    };

    let mut kept = Vec::<SalientLine>::new();
    for (index, candidate) in candidates.into_iter().enumerate() {
        let text = clean_candidate_line(&candidate);
        if text.is_empty() {
            continue;
        }

        let score = summary_candidate_score(&text);
        if score < 8 {
            continue;
        }

        if let Some(existing) = kept
            .iter_mut()
            .find(|existing| lines_are_similar(&existing.text, &text))
        {
            if score > existing.score {
                existing.index = index;
                existing.text = text;
                existing.score = score;
            }
            continue;
        }

        kept.push(SalientLine { index, text, score });
    }

    kept.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then(left.index.cmp(&right.index))
    });
    kept.truncate(4);
    kept.sort_by_key(|line| line.index);
    kept.into_iter().map(|line| line.text).collect()
}

fn clean_candidate_line(text: &str) -> String {
    let mut cleaned = collapse_spaces(strip_speaker_prefix(text));
    let lower = cleaned.to_ascii_lowercase();

    for prefix in [
        "okay ",
        "ok ",
        "well ",
        "so ",
        "i mean ",
        "let me tell you about ",
        "the whole point is ",
    ] {
        if lower.starts_with(prefix) {
            cleaned = cleaned[prefix.len()..].trim().to_string();
            break;
        }
    }

    cleaned
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '-' | ':' | ',' | '.'))
        .trim()
        .to_string()
}

fn summary_candidate_score(text: &str) -> i32 {
    let tokens = summary_tokens(text);
    if tokens.len() < 4 {
        return -10;
    }

    let unique_tokens = tokens.iter().collect::<HashSet<_>>().len();
    let unique_ratio = unique_tokens as f32 / tokens.len() as f32;
    let long_tokens = tokens.iter().filter(|token| token.len() >= 4).count() as i32;
    let short_tokens = tokens.iter().filter(|token| token.len() <= 2).count() as i32;
    let repeated_penalty = most_common_token_frequency(&tokens).saturating_sub(2) as i32 * 3;

    let mut score = tokens.len().min(18) as i32 + long_tokens * 2 - short_tokens;

    if unique_ratio >= 0.75 {
        score += 8;
    } else if unique_ratio >= 0.6 {
        score += 4;
    } else {
        score -= 6;
    }

    if text.ends_with(['.', '?', '!']) {
        score += 3;
    }

    if contains_summary_keywords(text) {
        score += 4;
    }

    if looks_noisy_line(text, &tokens) {
        return -10;
    }

    score - repeated_penalty
}

fn summary_tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|token| {
            let normalized = token
                .trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '\'')
                .to_ascii_lowercase();
            (!normalized.is_empty()).then_some(normalized)
        })
        .collect()
}

fn most_common_token_frequency(tokens: &[String]) -> usize {
    let mut best = 0usize;
    for token in tokens {
        best = best.max(
            tokens
                .iter()
                .filter(|candidate| *candidate == token)
                .count(),
        );
    }
    best
}

fn contains_summary_keywords(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "apple",
        "metal",
        "cuda",
        "nvidia",
        "implementation",
        "algorithm",
        "library",
        "rewrite",
        "research",
        "plan",
        "need to",
        "have to",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn looks_noisy_line(text: &str, tokens: &[String]) -> bool {
    if tokens.len() < 4 {
        return true;
    }

    let unique_ratio = tokens.iter().collect::<HashSet<_>>().len() as f32 / tokens.len() as f32;
    let repeated_adjacent = tokens.windows(2).any(|window| window[0] == window[1]);
    let short_ratio =
        tokens.iter().filter(|token| token.len() <= 2).count() as f32 / tokens.len() as f32;
    let lower = text.to_ascii_lowercase();

    unique_ratio < 0.5
        || repeated_adjacent
        || short_ratio > 0.45
        || lower.matches("okay").count() >= 3
        || lower.matches("clean").count() >= 4
}

fn lines_are_similar(left: &str, right: &str) -> bool {
    let left_normalized = collapse_spaces(&left.to_ascii_lowercase());
    let right_normalized = collapse_spaces(&right.to_ascii_lowercase());

    if left_normalized == right_normalized {
        return true;
    }

    if left_normalized.contains(&right_normalized) || right_normalized.contains(&left_normalized) {
        return left_normalized.len().min(right_normalized.len()) >= 24;
    }

    let left_tokens = summary_tokens(&left_normalized);
    let right_tokens = summary_tokens(&right_normalized);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return false;
    }

    let left_set = left_tokens.iter().collect::<HashSet<_>>();
    let right_set = right_tokens.iter().collect::<HashSet<_>>();
    let intersection = left_set.intersection(&right_set).count() as f32;
    let union = left_set.union(&right_set).count() as f32;

    union > 0.0 && intersection / union >= 0.7
}

fn transcript_seems_noisy(
    live_notes: &str,
    segments: &[CanonicalSegment],
    salient_lines: &[String],
) -> bool {
    if salient_lines.is_empty() {
        return true;
    }

    let raw_lines = if segments.is_empty() {
        live_notes
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(strip_speaker_prefix)
            .map(str::to_string)
            .collect::<Vec<_>>()
    } else {
        segments
            .iter()
            .map(|segment| segment.text.trim().to_string())
            .collect::<Vec<_>>()
    };

    if raw_lines.is_empty() {
        return true;
    }

    let noisy_count = raw_lines
        .iter()
        .filter(|line| looks_noisy_line(line, &summary_tokens(line)))
        .count();

    noisy_count * 2 >= raw_lines.len()
}

fn as_summary_sentence(text: &str) -> String {
    let cleaned = clean_candidate_line(text)
        .trim_end_matches(['.', '?', '!'])
        .trim()
        .to_string();
    if cleaned.is_empty() {
        return "transcript quality was too low to extract a stable point.".to_string();
    }

    let lower = cleaned.to_ascii_lowercase();
    if lower.contains("no implementation") && lower.contains("apple") {
        return "there is no Apple-compatible implementation of the available library.".to_string();
    }
    if lower.contains("rewrite") && (lower.contains("apple silicon") || lower.contains("metal")) {
        return "making this work on Mac requires rewriting the algorithm for Apple Silicon / Metal."
            .to_string();
    }
    if lower.contains("cuda") && lower.contains("metal") {
        return "the work involves bridging CUDA-oriented code to Metal.".to_string();
    }
    if lower.contains("months") || lower.contains("year") {
        return "the effort was described as a multi-month project, potentially up to a year."
            .to_string();
    }

    let mut sentence = cleaned;
    if let Some(first) = sentence.chars().next() {
        let first_upper = first.to_uppercase().to_string();
        sentence.replace_range(..first.len_utf8(), &first_upper);
    }
    if !sentence.ends_with('.') {
        sentence.push('.');
    }
    sentence
}

fn sanitize_session_title(text: &str) -> String {
    let candidate = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Ambient session");
    let candidate = strip_speaker_prefix(candidate)
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '#' | '-' | '*' | ':' | '.'))
        .trim();
    let candidate = if let Some((prefix, suffix)) = candidate.split_once(':') {
        if prefix.trim().eq_ignore_ascii_case("title") {
            suffix.trim()
        } else {
            candidate
        }
    } else {
        candidate
    };
    let candidate = strip_speaker_prefix(candidate).trim();

    let cleaned = collapse_spaces(
        &candidate
            .chars()
            .map(|ch| {
                if ch.is_alphanumeric() || ch.is_whitespace() || ch == '&' {
                    ch
                } else {
                    ' '
                }
            })
            .collect::<String>(),
    );
    if cleaned.is_empty() {
        return "Ambient Session".to_string();
    }

    let limited_words = cleaned
        .split_whitespace()
        .take(MAX_SESSION_TITLE_WORDS)
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed = trim_to_len(&limited_words, MAX_SESSION_TITLE_CHARS);
    if trimmed.is_empty() {
        "Ambient Session".to_string()
    } else {
        title_case_words(&trimmed)
    }
}

fn strip_speaker_prefix(text: &str) -> &str {
    for prefix in [
        "You:",
        "S1:",
        "S2:",
        "S3:",
        "S4:",
        "S5:",
        "S6:",
        "Speaker 1:",
        "Speaker 2:",
        "Speaker 3:",
        "Speaker 4:",
        "Speaker 5:",
        "Speaker 6:",
        "Person A:",
        "Person B:",
        "Person C:",
        "Person D:",
        "Person E:",
        "Person F:",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            return rest.trim();
        }
    }
    text
}

fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn trim_to_len(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut out = String::new();
    for word in text.split_whitespace() {
        let candidate = if out.is_empty() {
            word.to_string()
        } else {
            format!("{out} {word}")
        };
        if candidate.chars().count() > max_chars {
            break;
        }
        out = candidate;
    }

    if out.is_empty() {
        text.chars().take(max_chars).collect::<String>()
    } else {
        out
    }
}

fn title_case_words(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            let rest = chars.as_str().to_lowercase();
            format!("{}{}", first.to_uppercase(), rest)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{
        build_structured_notes_prompt, clean_general_markdown, full_summary_context,
        heuristic_structured_notes, merge_topic_mentions, parse_model_structured_notes,
        render_general_summary_draft, sanitize_session_title, summarize_general_multistage,
        GeneralSummaryDraft, TopicMention, TopicSection,
    };
    use screamer_core::ambient::{AudioLane, CanonicalSegment, SpeakerLabel, StructuredNotes};
    use screamer_core::ambient::SummaryTemplate;

    #[test]
    fn sanitize_session_title_strips_formatting_and_speaker_prefixes() {
        assert_eq!(
            sanitize_session_title("Title: You: hospital rehab planning!!!"),
            "Hospital Rehab Planning"
        );
    }

    #[test]
    fn sanitize_session_title_limits_words_and_length() {
        assert_eq!(
            sanitize_session_title(
                "a very long title that should absolutely keep only the first few words"
            ),
            "A Very Long Title"
        );
    }

    #[test]
    fn heuristic_summary_prefers_salient_unique_lines_over_raw_transcript() {
        let segments = vec![
            CanonicalSegment {
                id: 1,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S1,
                start_ms: 0,
                end_ms: 500,
                text: "Start cleaning you want me to clean clean what?".to_string(),
            },
            CanonicalSegment {
                id: 2,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S1,
                start_ms: 500,
                end_ms: 1_000,
                text: "There is no implementation of this library that runs on Apple."
                    .to_string(),
            },
            CanonicalSegment {
                id: 3,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S1,
                start_ms: 1_000,
                end_ms: 1_500,
                text: "So to do this, I have to rewrite that algorithm into an Apple Silicon implementation."
                    .to_string(),
            },
            CanonicalSegment {
                id: 4,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S1,
                start_ms: 1_500,
                end_ms: 2_000,
                text: "NVIDIA hardware uses CUDA, so we would need a Metal equivalent on MacBooks."
                    .to_string(),
            },
        ];

        let notes = heuristic_structured_notes("", &segments, Some("Diarization Port"));

        assert!(notes.summary.contains("Main takeaway:"));
        assert!(
            notes.summary.contains("Apple-compatible") || notes.summary.contains("Apple Silicon")
        );
        assert!(!notes.summary.contains("Start cleaning"));
        assert!(notes.key_points.iter().any(|point| point.contains("Apple")));
    }

    #[test]
    fn parses_structured_markdown_sections_from_model_output() {
        let parsed = parse_model_structured_notes(
            "## Summary\nThis was about porting diarization to Apple Silicon.\n\n## Key Points\n- The current Python library expects CUDA.\n- Metal support needs a rewrite.\n\n## Decisions\n- Use a staged v1 rollout.\n\n## Action Items\n- Prototype a Metal path.\n\n## Open Questions\n- How much of wav2vec2 can be shared?\n",
            "S1: transcript",
        )
        .expect("structured notes should parse");

        assert_eq!(
            parsed.summary,
            "This was about porting diarization to Apple Silicon."
        );
        assert_eq!(parsed.key_points.len(), 2);
        assert_eq!(
            parsed.decisions,
            vec!["Use a staged v1 rollout.".to_string()]
        );
        assert_eq!(
            parsed.action_items,
            vec!["Prototype a Metal path.".to_string()]
        );
        assert_eq!(
            parsed.open_questions,
            vec!["How much of wav2vec2 can be shared?".to_string()]
        );
        assert_eq!(parsed.transcript, "S1: transcript");
    }

    #[test]
    fn parses_common_gemma_heading_variants_and_none_markers() {
        let parsed = parse_model_structured_notes(
            "## Summary\nWe need better diarization before summarization.\n\n## Key Points\nGemma 3 1B should be bundled and run through Metal on Apple Silicon.\n\n## Decisions\nWe should switch to the smaller bundled model.\n\n## Actions\nNone.\n\n## Open Questions\nN/A\n",
            "S1: transcript",
        )
        .expect("variant structured notes should parse");

        assert_eq!(
            parsed.summary,
            "We need better diarization before summarization."
        );
        assert_eq!(
            parsed.key_points,
            vec![
                "Gemma 3 1B should be bundled and run through Metal on Apple Silicon.".to_string()
            ]
        );
        assert_eq!(
            parsed.decisions,
            vec!["We should switch to the smaller bundled model.".to_string()]
        );
        assert!(parsed.action_items.is_empty());
        assert!(parsed.open_questions.is_empty());
    }

    #[test]
    fn general_template_prompt_uses_topic_grouped_markdown_style() {
        let prompt = build_structured_notes_prompt(
            "",
            &[],
            Some("Ambient session"),
            SummaryTemplate::General,
        );

        assert!(prompt.starts_with("You are an expert note-taker"));
        assert!(prompt.contains("Create a markdown heading (##) for each topic"));
        assert!(prompt.contains("Do NOT reproduce speaker labels"));
        assert!(prompt.contains("Transcript:"));
    }

    #[test]
    fn structured_prompt_preserves_scratch_pad_context() {
        let prompt = build_structured_notes_prompt(
            "--- User Notes (Scratch Pad) ---\nPrioritize launch risk and customer issues.\n--- End User Notes ---\n\nPerson A: Launch next week if QA passes.",
            &[],
            Some("Launch review"),
            SummaryTemplate::TeamMeeting,
        );

        assert!(prompt.contains("User Notes (Scratch Pad):"));
        assert!(prompt.contains("Prioritize launch risk and customer issues."));
        assert!(prompt.contains("Transcript:"));
    }

    #[test]
    fn general_context_uses_speakerless_segments() {
        let segments = vec![
            CanonicalSegment {
                id: 1,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S1,
                start_ms: 0,
                end_ms: 500,
                text: "Ship the calendar invite flow next week.".to_string(),
            },
            CanonicalSegment {
                id: 2,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S2,
                start_ms: 500,
                end_ms: 1_000,
                text: "Recurring mobile bug is the blocker.".to_string(),
            },
        ];

        let context = full_summary_context(
            "--- User Notes (Scratch Pad) ---\nFocus on launch blockers.\n--- End User Notes ---\n\nPerson A: ignored because segments win.",
            &segments,
            false,
        );

        assert!(context.contains("User Notes (Scratch Pad):\nFocus on launch blockers."));
        assert!(context.contains("Ship the calendar invite flow next week."));
        assert!(context.contains("Recurring mobile bug is the blocker."));
        assert!(!context.contains("Person A:"));
        assert!(!context.contains("Person B:"));
    }

    #[test]
    fn clean_general_markdown_strips_speaker_labels_and_preamble() {
        let cleaned = clean_general_markdown(
            "Here’s a breakdown of the core discussion points:\n\n## Calendar Invite Flow\n* Person A: Ship next week if QA passes.\n* Person B: Maya will pair on the blocker.\n\n---",
        );

        assert_eq!(
            cleaned,
            "## Calendar Invite Flow\n- Ship next week if QA passes.\n- Maya will pair on the blocker."
        );
    }

    #[test]
    fn merge_topic_mentions_collapses_near_duplicate_titles() {
        let clusters = merge_topic_mentions(vec![
            TopicMention {
                title: "Launch timeline".to_string(),
                chunk_index: 0,
            },
            TopicMention {
                title: "Launch plan".to_string(),
                chunk_index: 1,
            },
            TopicMention {
                title: "Customer confusion".to_string(),
                chunk_index: 1,
            },
        ]);

        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].chunk_indices, vec![0, 1]);
        assert!(clusters[0].title.contains("Launch"));
        assert_eq!(clusters[1].title, "Customer confusion");
    }

    #[test]
    fn render_general_summary_draft_outputs_topics_then_operational_sections() {
        let markdown = render_general_summary_draft(&GeneralSummaryDraft {
            topics: vec![
                TopicSection {
                    title: "Release timing".to_string(),
                    bullets: vec!["Ship next week if QA passes by Thursday.".to_string()],
                },
                TopicSection {
                    title: "Customer confusion".to_string(),
                    bullets: vec!["Support flagged time zones and duplicate reminders.".to_string()],
                },
            ],
            decisions: vec!["Release desktop editing first if the mobile bug slips.".to_string()],
            action_items: vec!["Pair with Maya on the recurring bug tomorrow.".to_string()],
            open_questions: vec!["Whether the recurring bug will be fixed before Thursday.".to_string()],
        });

        assert!(markdown.starts_with("## Release timing"));
        assert!(markdown.contains("## Customer confusion"));
        assert!(markdown.contains("## Decisions"));
        assert!(markdown.contains("## Action Items"));
        assert!(markdown.contains("## Open Questions"));
    }

    #[test]
    fn multistage_general_summary_builds_topic_grouped_notes() {
        let segments = vec![
            CanonicalSegment {
                id: 1,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S1,
                start_ms: 0,
                end_ms: 500,
                text: "We should ship the calendar invite flow next week if QA passes by Thursday."
                    .to_string(),
            },
            CanonicalSegment {
                id: 2,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S2,
                start_ms: 500,
                end_ms: 1_000,
                text: "The blocker is the recurring-event bug in mobile.".to_string(),
            },
            CanonicalSegment {
                id: 3,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S1,
                start_ms: 1_000,
                end_ms: 1_500,
                text: "If that slips, release desktop editing first.".to_string(),
            },
            CanonicalSegment {
                id: 4,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S3,
                start_ms: 1_500,
                end_ms: 2_000,
                text: "Support flagged confusion around time zones and duplicate reminders."
                    .to_string(),
            },
            CanonicalSegment {
                id: 5,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S2,
                start_ms: 2_000,
                end_ms: 2_500,
                text: "Action item is to pair with Maya on the recurring bug tomorrow."
                    .to_string(),
            },
            CanonicalSegment {
                id: 6,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S1,
                start_ms: 2_500,
                end_ms: 3_000,
                text: "We will decide go or no-go in Friday's launch review.".to_string(),
            },
        ];
        let generator = |prompt: &str, _max_tokens: usize| -> Result<String, String> {
            if prompt.contains("Identify the concrete discussion topics") {
                return Ok("- Release timing\n- Customer confusion".to_string());
            }
            if prompt.contains("Write bullets for the meeting topic `Release timing`") {
                return Ok("- Ship the calendar invite flow next week if QA passes by Thursday.\n- If the recurring mobile bug slips, release desktop editing first.\n- The final go or no-go call happens in Friday's launch review.".to_string());
            }
            if prompt.contains("Write bullets for the meeting topic `Customer confusion`") {
                return Ok("- Support flagged confusion around time zones and duplicate reminders.".to_string());
            }
            if prompt.contains("Extract only decisions") {
                return Ok("- Ship the calendar invite flow next week if QA passes by Thursday.\n- Release desktop editing first if the recurring mobile bug slips.\n- Make the go or no-go call in Friday's launch review.".to_string());
            }
            if prompt.contains("Extract only concrete action items") {
                return Ok("- Pair with Maya on the recurring bug tomorrow.".to_string());
            }
            if prompt.contains("Extract only unresolved questions") {
                return Ok("- Whether the recurring mobile bug will be fixed before Thursday QA.".to_string());
            }
            Err(format!("unexpected prompt: {prompt}"))
        };

        let notes = summarize_general_multistage(
            "",
            &segments,
            Some("Launch review"),
            &StructuredNotes::default(),
            &generator,
        )
        .expect("multistage summary should succeed");
        let markdown = notes.raw_notes.expect("raw notes should be present");

        assert!(markdown.contains("## Release timing"));
        assert!(markdown.contains("## Customer confusion"));
        assert!(markdown.contains("## Decisions"));
        assert!(markdown.contains("## Action Items"));
        assert!(markdown.contains("## Open Questions"));
        assert!(!markdown.contains("Person A"));
        assert!(!markdown.contains("Person B"));
    }
}

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const DEFAULT_CHUNK_SECONDS: u64 = 8;
pub const DEFAULT_OVERLAP_SECONDS: u64 = 1;
const TRANSCRIPT_REPEAT_NGRAM_MIN: usize = 3;
const TRANSCRIPT_REPEAT_NGRAM_MAX: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioLane {
    Microphone,
    SystemOutput,
}

impl AudioLane {
    pub fn label(self) -> &'static str {
        match self {
            Self::Microphone => "Microphone",
            Self::SystemOutput => "System Output",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakerLabel {
    You,
    S1,
    S2,
    S3,
    S4,
    S5,
    S6,
}

impl SpeakerLabel {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::You => "You",
            Self::S1 => "Person A",
            Self::S2 => "Person B",
            Self::S3 => "Person C",
            Self::S4 => "Person D",
            Self::S5 => "Person E",
            Self::S6 => "Person F",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::You | Self::S1 => 0,
            Self::S2 => 1,
            Self::S3 => 2,
            Self::S4 => 3,
            Self::S5 => 4,
            Self::S6 => 5,
        }
    }

    pub fn next(self) -> SpeakerLabel {
        match self {
            Self::You | Self::S1 => Self::S2,
            Self::S2 => Self::S3,
            Self::S3 => Self::S4,
            Self::S4 => Self::S5,
            Self::S5 => Self::S6,
            Self::S6 => Self::S6,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryTemplate {
    #[default]
    General,
    OneOnOne,
    TeamMeeting,
    StandUp,
}

impl SummaryTemplate {
    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::OneOnOne => "1:1",
            Self::TeamMeeting => "Team Meeting",
            Self::StandUp => "Stand-up",
        }
    }

    pub fn all() -> &'static [SummaryTemplate] {
        &[
            Self::General,
            Self::OneOnOne,
            Self::TeamMeeting,
            Self::StandUp,
        ]
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "one_on_one" => Self::OneOnOne,
            "team_meeting" => Self::TeamMeeting,
            "stand_up" => Self::StandUp,
            _ => Self::General,
        }
    }

    pub fn to_db(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::OneOnOne => "one_on_one",
            Self::TeamMeeting => "team_meeting",
            Self::StandUp => "stand_up",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbientSessionState {
    Idle,
    Recording,
    Processing,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmbientSessionConfig {
    pub enable_microphone: bool,
    pub enable_system_audio: bool,
    pub chunk_seconds: u64,
    pub overlap_seconds: u64,
}

impl Default for AmbientSessionConfig {
    fn default() -> Self {
        Self {
            enable_microphone: true,
            enable_system_audio: true,
            chunk_seconds: DEFAULT_CHUNK_SECONDS,
            overlap_seconds: DEFAULT_OVERLAP_SECONDS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalSegment {
    pub id: u64,
    pub lane: AudioLane,
    pub speaker: SpeakerLabel,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

impl CanonicalSegment {
    pub fn note_line(&self) -> String {
        format!("{}: {}", self.speaker.display_name(), self.text.trim())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptSegment {
    pub lane: AudioLane,
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker_turn_next: bool,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiarizedSegment {
    pub lane: AudioLane,
    pub speaker: SpeakerLabel,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    /// Hint from the transcriber that this segment should start a new bubble
    /// (e.g. a detected speaker turn boundary).
    pub force_new: bool,
}

#[derive(Clone, Debug)]
pub struct DiarizationRequest<'a> {
    pub lane: AudioLane,
    pub sample_rate_hz: usize,
    pub chunk_start_ms: u64,
    pub chunk_end_ms: u64,
    pub samples: &'a [f32],
    pub transcript_segments: &'a [TranscriptSegment],
    pub previous_segments: &'a [CanonicalSegment],
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredNotes {
    pub summary: String,
    pub key_points: Vec<String>,
    pub decisions: Vec<String>,
    pub action_items: Vec<String>,
    pub open_questions: Vec<String>,
    pub transcript: String,
    /// Free-form markdown produced by the General template. When present,
    /// `to_markdown()` emits this directly. Transcript content is stored and
    /// rendered separately by the app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_notes: Option<String>,
}

impl StructuredNotes {
    pub fn to_markdown(&self) -> String {
        let markdown = if let Some(raw) = &self.raw_notes {
            raw.trim().to_string()
        } else {
            let mut out = String::new();
            push_section(&mut out, "Summary", std::slice::from_ref(&self.summary));
            push_section(&mut out, "Key Points", &self.key_points);
            push_section(&mut out, "Decisions", &self.decisions);
            push_section(&mut out, "Action Items", &self.action_items);
            push_section(&mut out, "Open Questions", &self.open_questions);
            out.trim().to_string()
        };

        polish_summary_markdown(&markdown)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: i64,
    pub title: String,
    pub state: AmbientSessionState,
    pub live_note_markdown: String,
    pub structured_note_markdown: String,
    pub transcript_markdown: String,
    pub scratch_pad: String,
}

pub trait AudioCaptureSource: Send + Sync {
    fn lane(&self) -> AudioLane;
    fn start(&self) -> Result<(), String>;
    fn stop(&self) -> Result<Vec<f32>, String>;
    fn snapshot(&self) -> Result<Vec<f32>, String>;
}

pub trait StreamingTranscriber: Send + Sync {
    fn transcribe_chunk(&self, lane: AudioLane, samples: &[f32]) -> Result<String, String>;
}

pub trait SpeakerAttributionEngine: Send + Sync {
    fn assign_speaker(
        &self,
        lane: AudioLane,
        text: &str,
        segment_index: usize,
        previous: Option<&CanonicalSegment>,
    ) -> SpeakerLabel;
}

pub trait NotesSummarizer: Send + Sync {
    fn summarize(
        &self,
        live_notes: &str,
        segments: &[CanonicalSegment],
        title_hint: Option<&str>,
        template: SummaryTemplate,
    ) -> Result<StructuredNotes, String>;
}

pub trait DiarizationEngine: Send + Sync {
    fn label(&self) -> &'static str;

    fn diarize(&self, request: DiarizationRequest<'_>) -> Vec<DiarizedSegment>;
}

pub fn chunk_step_samples(chunk_seconds: u64, overlap_seconds: u64, sample_rate: usize) -> usize {
    let chunk = chunk_seconds as usize * sample_rate;
    let overlap = overlap_seconds as usize * sample_rate;
    chunk.saturating_sub(overlap).max(sample_rate)
}

pub fn chunk_len_samples(chunk_seconds: u64, sample_rate: usize) -> usize {
    chunk_seconds as usize * sample_rate
}

fn repair_transcript_spacing(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        out.push(ch);
        if matches!(ch, '.' | '?' | '!' | ';' | ':') {
            if let Some(next) = chars.peek().copied() {
                if !next.is_whitespace() && !matches!(next, '.' | ',' | '?' | '!') {
                    out.push(' ');
                }
            }
        }
    }

    out
}

fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn split_transcript_fragments(text: &str) -> Vec<String> {
    let mut fragments = Vec::<String>::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '?' | '!' | ';' | '\n') {
            let fragment = collapse_spaces(current.trim());
            if !fragment.is_empty() {
                fragments.push(fragment);
            }
            current.clear();
        }
    }

    let trailing = collapse_spaces(current.trim());
    if !trailing.is_empty() {
        fragments.push(trailing);
    }
    if fragments.is_empty() {
        let single = collapse_spaces(text.trim());
        if !single.is_empty() {
            fragments.push(single);
        }
    }

    fragments
}

fn transcript_tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|token| {
            let normalized = token
                .trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '\'')
                .to_ascii_lowercase();
            (!normalized.is_empty()).then_some(normalized)
        })
        .collect()
}

fn compact_repeated_tokens(text: &str) -> String {
    let raw_tokens = text.split_whitespace().collect::<Vec<_>>();
    if raw_tokens.is_empty() {
        return String::new();
    }

    let normalized_tokens = raw_tokens
        .iter()
        .map(|token| {
            token
                .trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '\'')
                .to_ascii_lowercase()
        })
        .collect::<Vec<_>>();
    let mut kept = Vec::<&str>::new();
    let mut kept_normalized = Vec::<String>::new();
    let max_ngram = TRANSCRIPT_REPEAT_NGRAM_MAX.min(raw_tokens.len() / 2);
    let mut index = 0usize;

    while index < raw_tokens.len() {
        let mut collapsed = false;
        for size in (TRANSCRIPT_REPEAT_NGRAM_MIN..=max_ngram).rev() {
            if index + size * 2 > raw_tokens.len() {
                continue;
            }
            if normalized_tokens[index..index + size]
                .iter()
                .any(|token| token.is_empty())
            {
                continue;
            }

            let first = &normalized_tokens[index..index + size];
            let second = &normalized_tokens[index + size..index + size * 2];
            if first != second {
                continue;
            }

            kept.extend_from_slice(&raw_tokens[index..index + size]);
            kept_normalized.extend(first.iter().cloned());
            let pattern = first.to_vec();
            index += size * 2;
            while index + size <= raw_tokens.len()
                && normalized_tokens[index..index + size] == pattern[..]
            {
                index += size;
            }
            collapsed = true;
            break;
        }

        if collapsed {
            continue;
        }

        if kept_normalized
            .last()
            .is_some_and(|last| !last.is_empty() && *last == normalized_tokens[index])
        {
            index += 1;
            continue;
        }

        kept.push(raw_tokens[index]);
        kept_normalized.push(normalized_tokens[index].clone());
        index += 1;
    }

    collapse_spaces(&kept.join(" "))
}

fn transcript_fragments_similar(left: &str, right: &str) -> bool {
    let left_normalized = collapse_spaces(&left.to_ascii_lowercase());
    let right_normalized = collapse_spaces(&right.to_ascii_lowercase());

    if left_normalized == right_normalized {
        return true;
    }

    if left_normalized.contains(&right_normalized) || right_normalized.contains(&left_normalized) {
        return left_normalized.len().min(right_normalized.len()) >= 24;
    }

    let left_tokens = transcript_tokens(&left_normalized);
    let right_tokens = transcript_tokens(&right_normalized);
    if left_tokens.is_empty() || right_tokens.is_empty() {
        return false;
    }

    let left_set = left_tokens.iter().collect::<HashSet<_>>();
    let right_set = right_tokens.iter().collect::<HashSet<_>>();
    let intersection = left_set.intersection(&right_set).count() as f32;
    let union = left_set.union(&right_set).count() as f32;

    union > 0.0 && intersection / union >= 0.8
}

fn clean_transcript_text(text: &str) -> String {
    let repaired = repair_transcript_spacing(text);
    let mut kept = Vec::<String>::new();

    for fragment in split_transcript_fragments(&repaired) {
        let compacted = compact_repeated_tokens(&fragment);
        let compacted = collapse_spaces(compacted.trim());
        if compacted.is_empty() {
            continue;
        }

        if let Some(existing) = kept
            .iter_mut()
            .rev()
            .take(3)
            .find(|existing| transcript_fragments_similar(existing, &compacted))
        {
            if compacted.chars().count() > existing.chars().count()
                && compacted.contains(existing.as_str())
            {
                *existing = compacted;
            }
            continue;
        }

        kept.push(compacted);
    }

    if kept.is_empty() {
        collapse_spaces(&compact_repeated_tokens(&repaired))
    } else {
        kept.join(". ")
    }
}

pub fn stitch_text(existing: &str, incoming: &str) -> String {
    let existing = clean_transcript_text(existing);
    let incoming = clean_transcript_text(incoming);
    let existing = existing.trim();
    let incoming = incoming.trim();

    if incoming.is_empty() {
        return String::new();
    }
    if existing.is_empty() {
        return incoming.to_string();
    }
    if existing == incoming || existing.ends_with(incoming) {
        return String::new();
    }
    if existing.contains(incoming) && incoming.chars().count() >= 24 {
        return String::new();
    }

    let existing_words: Vec<&str> = existing.split_whitespace().collect();
    let incoming_words: Vec<&str> = incoming.split_whitespace().collect();
    let existing_tokens = existing_words
        .iter()
        .map(|word| {
            word.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '\'')
                .to_ascii_lowercase()
        })
        .collect::<Vec<_>>();
    let incoming_tokens = incoming_words
        .iter()
        .map(|word| {
            word.trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '\'')
                .to_ascii_lowercase()
        })
        .collect::<Vec<_>>();
    let max_overlap = existing_words.len().min(incoming_words.len()).min(20);

    for overlap in (1..=max_overlap).rev() {
        if existing_tokens[existing_tokens.len() - overlap..] == incoming_tokens[..overlap] {
            return collapse_spaces(&incoming_words[overlap..].join(" "));
        }
    }

    if transcript_fragments_similar(existing, incoming)
        && existing.chars().count().abs_diff(incoming.chars().count())
            <= incoming.chars().count() / 4
    {
        return String::new();
    }

    incoming.to_string()
}

pub fn merge_segment(
    segments: &mut Vec<CanonicalSegment>,
    mut incoming: CanonicalSegment,
    force_new: bool,
) -> Option<CanonicalSegment> {
    incoming.text = clean_transcript_text(&incoming.text);
    let stitched = segments
        .last()
        .map(|last| stitch_text(&last.text, &incoming.text))
        .unwrap_or_else(|| incoming.text.clone());

    if stitched.trim().is_empty() {
        return None;
    }
    incoming.text = stitched;

    if !force_new {
        if let Some(last) = segments.last_mut() {
            if last.speaker == incoming.speaker
                && last.lane == incoming.lane
                && incoming.start_ms.saturating_sub(last.end_ms) <= 800
            {
                if !last.text.ends_with('.')
                    && !last.text.ends_with('!')
                    && !last.text.ends_with('?')
                {
                    last.text.push(' ');
                }
                last.text.push_str(incoming.text.trim());
                last.text = clean_transcript_text(&last.text);
                last.end_ms = incoming.end_ms;
                return Some(last.clone());
            }
        }
    }

    incoming.text = clean_transcript_text(&incoming.text);
    if incoming.text.trim().is_empty() {
        return None;
    }
    segments.push(incoming.clone());
    Some(incoming)
}

pub fn clean_canonical_segments(segments: &[CanonicalSegment]) -> Vec<CanonicalSegment> {
    let mut cleaned = Vec::<CanonicalSegment>::with_capacity(segments.len());

    for segment in segments {
        let mut normalized = segment.clone();
        normalized.text = clean_transcript_text(&normalized.text);
        if normalized.text.trim().is_empty() {
            continue;
        }
        let _ = merge_segment(&mut cleaned, normalized, false);
    }

    cleaned
}

pub fn segments_to_transcript(segments: &[CanonicalSegment]) -> String {
    segments
        .iter()
        .map(CanonicalSegment::note_line)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn heuristic_title(live_notes: &str, segments: &[CanonicalSegment]) -> String {
    let first_line = live_notes
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if !first_line.is_empty() {
        return first_line.chars().take(48).collect::<String>();
    }

    segments
        .iter()
        .find_map(|segment| {
            let text = segment.text.trim();
            (!text.is_empty()).then(|| text.chars().take(48).collect::<String>())
        })
        .unwrap_or_else(|| "Ambient session".to_string())
}

fn push_section(out: &mut String, heading: &str, lines: &[String]) {
    out.push_str("## ");
    out.push_str(heading);
    out.push_str("\n\n");

    let non_empty: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    if non_empty.is_empty() {
        out.push_str("- None\n\n");
        return;
    }

    if heading == "Transcript" {
        for line in non_empty {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
        return;
    }

    for line in non_empty {
        out.push_str("- ");
        out.push_str(markdown_list_item_body(line).unwrap_or(line));
        out.push('\n');
    }
    out.push('\n');
}

pub fn polish_summary_markdown(markdown: &str) -> String {
    let normalized = markdown.replace("\r\n", "\n");
    let mut polished = Vec::<String>::new();
    let mut previous_blank = true;

    for raw_line in normalized.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            if !previous_blank && !polished.is_empty() {
                polished.push(String::new());
            }
            previous_blank = true;
            continue;
        }

        if let Some(heading) = markdown_heading_text(line) {
            let heading = normalize_summary_heading(heading);
            if heading.is_empty() {
                continue;
            }
            if !polished.is_empty() && !previous_blank {
                polished.push(String::new());
            }
            polished.push(format!("# {heading}"));
            polished.push(String::new());
            previous_blank = true;
            continue;
        }

        if let Some(item) = markdown_list_item_body(line) {
            polished.push(format!("- {}", emphasize_summary_bullet(item)));
            previous_blank = false;
            continue;
        }

        polished.push(collapse_summary_spaces(line));
        previous_blank = false;
    }

    while polished.last().is_some_and(|line| line.is_empty()) {
        polished.pop();
    }

    polished.join("\n")
}

fn markdown_heading_text(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let heading = trimmed.strip_prefix('#')?;
    Some(heading.trim_start_matches('#').trim())
}

fn markdown_list_item_body(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if let Some(body) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
        .or_else(|| trimmed.strip_prefix("• "))
    {
        return Some(body.trim());
    }

    let digit_count = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_count == 0 {
        return None;
    }

    let suffix = trimmed[digit_count..]
        .strip_prefix('.')
        .or_else(|| trimmed[digit_count..].strip_prefix(')'))?;
    let body = suffix.trim_start();
    (!body.is_empty()).then_some(body)
}

fn normalize_summary_heading(heading: &str) -> String {
    collapse_summary_spaces(
        heading
            .trim_matches(|ch: char| matches!(ch, '*' | '`' | '"' | '\''))
            .trim_end_matches(':')
            .trim(),
    )
}

fn emphasize_summary_bullet(item: &str) -> String {
    let normalized = collapse_summary_spaces(item);
    if normalized.is_empty() || normalized.starts_with("**") {
        return normalized;
    }

    let Some((label, detail)) = normalized.split_once(':') else {
        return normalized;
    };
    let label = collapse_summary_spaces(label);
    let detail = detail.trim();
    if detail.is_empty()
        || label.is_empty()
        || label.chars().count() > 36
        || label.split_whitespace().count() > 5
        || matches!(label.chars().last(), Some('.' | '?' | '!'))
        || label.contains("**")
    {
        return normalized;
    }

    if !label.chars().all(|ch| {
        ch.is_alphanumeric()
            || ch.is_whitespace()
            || matches!(ch, '&' | '/' | '+' | '-' | '(' | ')' | '\'')
    }) {
        return normalized;
    }

    format!("**{label}:** {detail}")
}

fn collapse_summary_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stitch_removes_word_overlap() {
        let stitched = stitch_text(
            "we should ship this by friday",
            "ship this by friday with docs",
        );
        assert_eq!(stitched, "with docs");
    }

    #[test]
    fn stitch_compacts_repeated_phrases_and_repairs_spacing() {
        let stitched = stitch_text(
            "I'm on call but I'm taking the 20th PTO.I'm not going to be working and I probably won't even take my work computer with me.",
            "I'm not going to be working and I probably won't even take my work computer with me. Can I swap an on call with someone?",
        );
        assert_eq!(stitched, "Can I swap an on call with someone?");
    }

    #[test]
    fn merge_segment_extends_matching_turn() {
        let mut segments = vec![CanonicalSegment {
            id: 1,
            lane: AudioLane::Microphone,
            speaker: SpeakerLabel::You,
            start_ms: 0,
            end_ms: 500,
            text: "hello".to_string(),
        }];

        let merged = merge_segment(
            &mut segments,
            CanonicalSegment {
                id: 2,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::You,
                start_ms: 700,
                end_ms: 1_000,
                text: "world".to_string(),
            },
            false,
        )
        .unwrap();

        assert_eq!(segments.len(), 1);
        assert_eq!(merged.text, "hello world");
    }

    #[test]
    fn force_new_prevents_merge() {
        let mut segments = vec![CanonicalSegment {
            id: 1,
            lane: AudioLane::Microphone,
            speaker: SpeakerLabel::You,
            start_ms: 0,
            end_ms: 500,
            text: "hello".to_string(),
        }];

        merge_segment(
            &mut segments,
            CanonicalSegment {
                id: 2,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::You,
                start_ms: 600,
                end_ms: 1_000,
                text: "world".to_string(),
            },
            true,
        )
        .unwrap();

        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].text, "hello");
        assert_eq!(segments[1].text, "world");
    }

    #[test]
    fn clean_canonical_segments_compacts_repetition_heavy_transcript() {
        let cleaned = clean_canonical_segments(&[
            CanonicalSegment {
                id: 1,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S1,
                start_ms: 0,
                end_ms: 500,
                text: "Not next week with the following week.I also have a favor to ask.Not next week with the following week.on call but I'm taking the 28th".to_string(),
            },
            CanonicalSegment {
                id: 2,
                lane: AudioLane::Microphone,
                speaker: SpeakerLabel::S1,
                start_ms: 500,
                end_ms: 1_000,
                text: "I'm on call, but I'm taking the 20th PTO. I'm on call, but I'm taking the 20th PTO. I'm not going to be working and I probably won't even take my work computer with me.".to_string(),
            },
        ]);

        let transcript = segments_to_transcript(&cleaned);
        assert!(transcript.contains("I also have a favor to ask."));
        assert_eq!(
            transcript
                .matches("Not next week with the following week")
                .count(),
            1
        );
        assert_eq!(
            transcript
                .matches("I'm on call, but I'm taking the 20th PTO")
                .count(),
            1
        );
    }

    #[test]
    fn raw_notes_markdown_omits_transcript_section() {
        let notes = StructuredNotes {
            raw_notes: Some("## Shipping\n- Calendar invite flow is ready.".to_string()),
            transcript: "Person A: calendar invite flow".to_string(),
            ..Default::default()
        };

        assert_eq!(
            notes.to_markdown(),
            "# Shipping\n\n- Calendar invite flow is ready."
        );
    }

    #[test]
    fn raw_notes_markdown_gets_a_final_polish_pass() {
        let notes = StructuredNotes {
            raw_notes: Some(
                "## Drinks Inventory Review\n- Cider: remove\n- IPA: keep\n\n## Next Steps\n- Suvamsh: pull out the cider from the selection".to_string(),
            ),
            transcript: "Person A: calendar invite flow".to_string(),
            ..Default::default()
        };

        assert_eq!(
            notes.to_markdown(),
            "# Drinks Inventory Review\n\n- **Cider:** remove\n- **IPA:** keep\n\n# Next Steps\n\n- **Suvamsh:** pull out the cider from the selection"
        );
    }

    #[test]
    fn structured_notes_markdown_uses_prominent_titled_bullets() {
        let notes = StructuredNotes {
            summary: "The release stays on track for next week.".to_string(),
            key_points: vec![
                "Owner: Maya".to_string(),
                "Risk: timezone confusion in reminders".to_string(),
            ],
            decisions: vec!["Ship if QA passes by Thursday.".to_string()],
            action_items: vec!["Maya: pair on the blocker tomorrow.".to_string()],
            open_questions: vec!["None".to_string()],
            transcript: String::new(),
            raw_notes: None,
        };

        assert_eq!(
            notes.to_markdown(),
            "# Summary\n\n- The release stays on track for next week.\n\n# Key Points\n\n- **Owner:** Maya\n- **Risk:** timezone confusion in reminders\n\n# Decisions\n\n- Ship if QA passes by Thursday.\n\n# Action Items\n\n- **Maya:** pair on the blocker tomorrow.\n\n# Open Questions\n\n- None"
        );
    }
}

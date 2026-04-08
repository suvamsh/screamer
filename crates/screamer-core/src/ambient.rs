use serde::{Deserialize, Serialize};

pub const DEFAULT_CHUNK_SECONDS: u64 = 8;
pub const DEFAULT_OVERLAP_SECONDS: u64 = 1;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryTemplate {
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

impl Default for SummaryTemplate {
    fn default() -> Self {
        Self::General
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
        if let Some(raw) = &self.raw_notes {
            return raw.trim().to_string();
        }
        let mut out = String::new();
        push_section(&mut out, "Summary", &[self.summary.clone()]);
        push_section(&mut out, "Key Points", &self.key_points);
        push_section(&mut out, "Decisions", &self.decisions);
        push_section(&mut out, "Action Items", &self.action_items);
        push_section(&mut out, "Open Questions", &self.open_questions);
        out.trim().to_string()
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

pub fn stitch_text(existing: &str, incoming: &str) -> String {
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

    let existing_words: Vec<&str> = existing.split_whitespace().collect();
    let incoming_words: Vec<&str> = incoming.split_whitespace().collect();
    let max_overlap = existing_words.len().min(incoming_words.len()).min(12);

    for overlap in (1..=max_overlap).rev() {
        if existing_words[existing_words.len() - overlap..] == incoming_words[..overlap] {
            return incoming_words[overlap..].join(" ").trim().to_string();
        }
    }

    incoming.to_string()
}

pub fn merge_segment(
    segments: &mut Vec<CanonicalSegment>,
    mut incoming: CanonicalSegment,
    force_new: bool,
) -> Option<CanonicalSegment> {
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
                last.end_ms = incoming.end_ms;
                return Some(last.clone());
            }
        }
    }

    segments.push(incoming.clone());
    Some(incoming)
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

    if heading == "Summary" || heading == "Transcript" {
        for line in non_empty {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
        return;
    }

    for line in non_empty {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
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
    fn raw_notes_markdown_omits_transcript_section() {
        let notes = StructuredNotes {
            raw_notes: Some("## Shipping\n- Calendar invite flow is ready.".to_string()),
            transcript: "Person A: calendar invite flow".to_string(),
            ..Default::default()
        };

        assert_eq!(
            notes.to_markdown(),
            "## Shipping\n- Calendar invite flow is ready."
        );
    }
}

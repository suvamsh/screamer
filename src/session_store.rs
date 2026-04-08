use rusqlite::{params, Connection, OptionalExtension};
use screamer_core::ambient::{AmbientSessionState, CanonicalSegment, SummaryTemplate};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct SessionSummary {
    pub id: i64,
    pub title: String,
    pub state: AmbientSessionState,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
    pub live_notes_preview: String,
}

#[derive(Clone, Debug)]
pub struct SessionDetail {
    pub id: i64,
    pub title: String,
    pub state: AmbientSessionState,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub live_notes: String,
    pub structured_notes: String,
    pub transcript_markdown: String,
    pub scratch_pad: String,
    pub summary_template: SummaryTemplate,
    pub segments: Vec<CanonicalSegment>,
}

pub struct SessionStore {
    conn: Mutex<Connection>,
}

impl SessionStore {
    pub fn open_default() -> Result<Self, String> {
        let path = default_store_path()?;
        let parent = path
            .parent()
            .ok_or_else(|| "Unable to resolve session store directory".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create session store directory: {err}"))?;
        let conn = Connection::open(path)
            .map_err(|err| format!("Failed to open session database: {err}"))?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init()?;
        Ok(store)
    }

    pub fn create_session(&self, title: &str, summary_backend: &str) -> Result<i64, String> {
        let now = unix_ms();
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        conn.execute(
            "INSERT INTO sessions (
                title, state, started_at_ms, updated_at_ms, live_notes,
                structured_notes, transcript_markdown, summary_backend
            ) VALUES (?1, ?2, ?3, ?4, '', '', '', ?5)",
            params![
                title,
                state_to_db(AmbientSessionState::Recording),
                now,
                now,
                summary_backend
            ],
        )
        .map_err(|err| format!("Failed to create session: {err}"))?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_state(
        &self,
        session_id: i64,
        state: AmbientSessionState,
        ended_at_ms: Option<i64>,
    ) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        conn.execute(
            "UPDATE sessions
                SET state = ?2, updated_at_ms = ?3, ended_at_ms = COALESCE(?4, ended_at_ms)
              WHERE id = ?1",
            params![session_id, state_to_db(state), unix_ms(), ended_at_ms],
        )
        .map_err(|err| format!("Failed to update session state: {err}"))?;
        Ok(())
    }

    pub fn update_live_notes(&self, session_id: i64, live_notes: &str) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        conn.execute(
            "UPDATE sessions SET live_notes = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![session_id, live_notes, unix_ms()],
        )
        .map_err(|err| format!("Failed to update live notes: {err}"))?;
        Ok(())
    }

    pub fn update_scratch_pad(&self, session_id: i64, scratch_pad: &str) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        conn.execute(
            "UPDATE sessions SET scratch_pad = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![session_id, scratch_pad, unix_ms()],
        )
        .map_err(|err| format!("Failed to update scratch pad: {err}"))?;
        Ok(())
    }

    pub fn update_summary_template(
        &self,
        session_id: i64,
        template: SummaryTemplate,
    ) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        conn.execute(
            "UPDATE sessions SET summary_template = ?2, updated_at_ms = ?3 WHERE id = ?1",
            params![session_id, template.to_db(), unix_ms()],
        )
        .map_err(|err| format!("Failed to update summary template: {err}"))?;
        Ok(())
    }

    pub fn update_structured_notes(
        &self,
        session_id: i64,
        title: &str,
        structured_notes: &str,
        transcript_markdown: &str,
    ) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        conn.execute(
            "UPDATE sessions
                SET title = ?2,
                    structured_notes = ?3,
                    transcript_markdown = ?4,
                    updated_at_ms = ?5
              WHERE id = ?1",
            params![
                session_id,
                title,
                structured_notes,
                transcript_markdown,
                unix_ms()
            ],
        )
        .map_err(|err| format!("Failed to update structured notes: {err}"))?;
        Ok(())
    }

    pub fn append_segments(
        &self,
        session_id: i64,
        segments: &[CanonicalSegment],
    ) -> Result<(), String> {
        if segments.is_empty() {
            return Ok(());
        }

        let mut conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tx = conn
            .transaction()
            .map_err(|err| format!("Failed to start segment transaction: {err}"))?;
        let ordinal_base: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(ordinal), -1) FROM session_segments WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(|err| format!("Failed to query last segment ordinal: {err}"))?;

        for (offset, segment) in segments.iter().enumerate() {
            tx.execute(
                "INSERT INTO session_segments (
                    session_id, ordinal, segment_id, lane, speaker, start_ms, end_ms, text
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    session_id,
                    ordinal_base + offset as i64 + 1,
                    segment.id as i64,
                    lane_to_db(segment.lane),
                    speaker_to_db(segment.speaker),
                    segment.start_ms as i64,
                    segment.end_ms as i64,
                    segment.text,
                ],
            )
            .map_err(|err| format!("Failed to append session segment: {err}"))?;
        }

        tx.execute(
            "UPDATE sessions SET updated_at_ms = ?2 WHERE id = ?1",
            params![session_id, unix_ms()],
        )
        .map_err(|err| format!("Failed to update session timestamp: {err}"))?;
        tx.commit()
            .map_err(|err| format!("Failed to commit session segments: {err}"))?;
        Ok(())
    }

    pub fn replace_segments(
        &self,
        session_id: i64,
        segments: &[CanonicalSegment],
    ) -> Result<(), String> {
        let mut conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tx = conn
            .transaction()
            .map_err(|err| format!("Failed to start segment replacement transaction: {err}"))?;

        tx.execute(
            "DELETE FROM session_segments WHERE session_id = ?1",
            params![session_id],
        )
        .map_err(|err| format!("Failed to clear existing session segments: {err}"))?;

        for (ordinal, segment) in segments.iter().enumerate() {
            tx.execute(
                "INSERT INTO session_segments (
                    session_id, ordinal, segment_id, lane, speaker, start_ms, end_ms, text
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    session_id,
                    ordinal as i64,
                    segment.id as i64,
                    lane_to_db(segment.lane),
                    speaker_to_db(segment.speaker),
                    segment.start_ms as i64,
                    segment.end_ms as i64,
                    segment.text,
                ],
            )
            .map_err(|err| format!("Failed to insert replaced session segment: {err}"))?;
        }

        tx.execute(
            "UPDATE sessions SET updated_at_ms = ?2 WHERE id = ?1",
            params![session_id, unix_ms()],
        )
        .map_err(|err| format!("Failed to update session timestamp: {err}"))?;
        tx.commit()
            .map_err(|err| format!("Failed to commit replaced session segments: {err}"))?;
        Ok(())
    }

    pub fn update_last_segment(
        &self,
        session_id: i64,
        segment: &CanonicalSegment,
    ) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let updated = conn
            .execute(
                "UPDATE session_segments
                    SET segment_id = ?2,
                        lane = ?3,
                        speaker = ?4,
                        start_ms = ?5,
                        end_ms = ?6,
                        text = ?7
                  WHERE id = (
                        SELECT id
                          FROM session_segments
                         WHERE session_id = ?1
                      ORDER BY ordinal DESC
                         LIMIT 1
                  )",
                params![
                    session_id,
                    segment.id as i64,
                    lane_to_db(segment.lane),
                    speaker_to_db(segment.speaker),
                    segment.start_ms as i64,
                    segment.end_ms as i64,
                    segment.text,
                ],
            )
            .map_err(|err| format!("Failed to update session segment: {err}"))?;
        if updated == 0 {
            return Err("No session segment was available to update.".to_string());
        }

        conn.execute(
            "UPDATE sessions SET updated_at_ms = ?2 WHERE id = ?1",
            params![session_id, unix_ms()],
        )
        .map_err(|err| format!("Failed to update session timestamp: {err}"))?;
        Ok(())
    }

    pub fn list_recent_sessions(
        &self,
        limit: usize,
        search: Option<&str>,
    ) -> Result<Vec<SessionSummary>, String> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let limit = limit.max(1) as i64;

        let mut sessions = Vec::new();
        if let Some(search) = search.filter(|value| !value.trim().is_empty()) {
            let mut stmt = conn
                .prepare(
                    "SELECT s.id, s.title, s.state, s.started_at_ms, s.updated_at_ms, s.live_notes
                       FROM sessions s
                       JOIN sessions_fts fts ON fts.rowid = s.id
                      WHERE sessions_fts MATCH ?1
                      ORDER BY s.updated_at_ms DESC
                      LIMIT ?2",
                )
                .map_err(|err| format!("Failed to prepare session search: {err}"))?;
            let rows = stmt
                .query_map(params![fts_query(search), limit], map_session_summary)
                .map_err(|err| format!("Failed to query searched sessions: {err}"))?;
            for row in rows {
                sessions
                    .push(row.map_err(|err| format!("Failed to read searched session: {err}"))?);
            }
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT id, title, state, started_at_ms, updated_at_ms, live_notes
                       FROM sessions
                   ORDER BY updated_at_ms DESC
                      LIMIT ?1",
                )
                .map_err(|err| format!("Failed to prepare recent sessions query: {err}"))?;
            let rows = stmt
                .query_map(params![limit], map_session_summary)
                .map_err(|err| format!("Failed to query recent sessions: {err}"))?;
            for row in rows {
                sessions.push(row.map_err(|err| format!("Failed to read session summary: {err}"))?);
            }
        }
        Ok(sessions)
    }

    pub fn load_session(&self, session_id: i64) -> Result<Option<SessionDetail>, String> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let detail = conn
            .query_row(
                "SELECT id, title, state, started_at_ms, ended_at_ms, live_notes, structured_notes, transcript_markdown, COALESCE(scratch_pad, ''), COALESCE(summary_template, 'general')
                   FROM sessions
                  WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok(SessionDetail {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        state: state_from_db(&row.get::<_, String>(2)?),
                        started_at_ms: row.get(3)?,
                        ended_at_ms: row.get(4)?,
                        live_notes: row.get(5)?,
                        structured_notes: row.get(6)?,
                        transcript_markdown: row.get(7)?,
                        scratch_pad: row.get(8)?,
                        summary_template: SummaryTemplate::from_db(&row.get::<_, String>(9)?),
                        segments: Vec::new(),
                    })
                },
            )
            .optional()
            .map_err(|err| format!("Failed to load session detail: {err}"))?;

        let Some(mut detail) = detail else {
            return Ok(None);
        };

        let mut stmt = conn
            .prepare(
                "SELECT segment_id, lane, speaker, start_ms, end_ms, text
                   FROM session_segments
                  WHERE session_id = ?1
               ORDER BY ordinal ASC",
            )
            .map_err(|err| format!("Failed to prepare session segments query: {err}"))?;
        let rows = stmt
            .query_map(params![session_id], |row| {
                Ok(CanonicalSegment {
                    id: row.get::<_, i64>(0)? as u64,
                    lane: lane_from_db(&row.get::<_, String>(1)?),
                    speaker: speaker_from_db(&row.get::<_, String>(2)?),
                    start_ms: row.get::<_, i64>(3)? as u64,
                    end_ms: row.get::<_, i64>(4)? as u64,
                    text: row.get(5)?,
                })
            })
            .map_err(|err| format!("Failed to query session segments: {err}"))?;
        for row in rows {
            detail
                .segments
                .push(row.map_err(|err| format!("Failed to read session segment: {err}"))?);
        }

        Ok(Some(detail))
    }

    fn init(&self) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                state TEXT NOT NULL,
                started_at_ms INTEGER NOT NULL,
                ended_at_ms INTEGER,
                updated_at_ms INTEGER NOT NULL,
                live_notes TEXT NOT NULL,
                structured_notes TEXT NOT NULL,
                transcript_markdown TEXT NOT NULL,
                summary_backend TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS session_segments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id INTEGER NOT NULL,
                ordinal INTEGER NOT NULL,
                segment_id INTEGER NOT NULL,
                lane TEXT NOT NULL,
                speaker TEXT NOT NULL,
                start_ms INTEGER NOT NULL,
                end_ms INTEGER NOT NULL,
                text TEXT NOT NULL,
                FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS sessions_fts
                USING fts5(title, live_notes, structured_notes, transcript_markdown, content='sessions', content_rowid='id');
            CREATE TRIGGER IF NOT EXISTS sessions_ai AFTER INSERT ON sessions BEGIN
                INSERT INTO sessions_fts(rowid, title, live_notes, structured_notes, transcript_markdown)
                VALUES (new.id, new.title, new.live_notes, new.structured_notes, new.transcript_markdown);
            END;
            CREATE TRIGGER IF NOT EXISTS sessions_ad AFTER DELETE ON sessions BEGIN
                INSERT INTO sessions_fts(sessions_fts, rowid, title, live_notes, structured_notes, transcript_markdown)
                VALUES ('delete', old.id, old.title, old.live_notes, old.structured_notes, old.transcript_markdown);
            END;
            CREATE TRIGGER IF NOT EXISTS sessions_au AFTER UPDATE ON sessions BEGIN
                INSERT INTO sessions_fts(sessions_fts, rowid, title, live_notes, structured_notes, transcript_markdown)
                VALUES ('delete', old.id, old.title, old.live_notes, old.structured_notes, old.transcript_markdown);
                INSERT INTO sessions_fts(rowid, title, live_notes, structured_notes, transcript_markdown)
                VALUES (new.id, new.title, new.live_notes, new.structured_notes, new.transcript_markdown);
            END;
            ",
        )
        .map_err(|err| format!("Failed to initialize session database: {err}"))?;

        let _ = conn.execute(
            "ALTER TABLE sessions ADD COLUMN scratch_pad TEXT NOT NULL DEFAULT ''",
            [],
        );

        let _ = conn.execute(
            "ALTER TABLE sessions ADD COLUMN summary_template TEXT NOT NULL DEFAULT 'general'",
            [],
        );

        Ok(())
    }
}

fn default_store_path() -> Result<PathBuf, String> {
    let base = dirs::data_dir()
        .ok_or_else(|| "Unable to locate application data directory".to_string())?;
    Ok(base.join("Screamer").join("sessions.sqlite"))
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn fts_query(search: &str) -> String {
    search
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(|token| format!("{token}*"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn map_session_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionSummary> {
    Ok(SessionSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        state: state_from_db(&row.get::<_, String>(2)?),
        started_at_ms: row.get(3)?,
        updated_at_ms: row.get(4)?,
        live_notes_preview: preview(&row.get::<_, String>(5)?),
    })
}

fn preview(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "No notes yet".to_string();
    }
    let first_line = trimmed.lines().next().unwrap_or(trimmed).trim();
    let preview = first_line
        .strip_prefix("You:")
        .or_else(|| first_line.strip_prefix("S1:"))
        .or_else(|| first_line.strip_prefix("S2:"))
        .unwrap_or(first_line)
        .trim();
    let mut preview = preview.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((idx, _)) = preview.char_indices().nth(56) {
        preview.truncate(idx);
    }
    preview
}

fn state_to_db(state: AmbientSessionState) -> &'static str {
    match state {
        AmbientSessionState::Idle => "idle",
        AmbientSessionState::Recording => "recording",
        AmbientSessionState::Processing => "processing",
        AmbientSessionState::Completed => "completed",
        AmbientSessionState::Failed => "failed",
    }
}

fn state_from_db(value: &str) -> AmbientSessionState {
    match value {
        "recording" => AmbientSessionState::Recording,
        "processing" => AmbientSessionState::Processing,
        "completed" => AmbientSessionState::Completed,
        "failed" => AmbientSessionState::Failed,
        _ => AmbientSessionState::Idle,
    }
}

fn lane_to_db(lane: screamer_core::ambient::AudioLane) -> &'static str {
    match lane {
        screamer_core::ambient::AudioLane::Microphone => "microphone",
        screamer_core::ambient::AudioLane::SystemOutput => "system_output",
    }
}

fn lane_from_db(value: &str) -> screamer_core::ambient::AudioLane {
    match value {
        "system_output" => screamer_core::ambient::AudioLane::SystemOutput,
        _ => screamer_core::ambient::AudioLane::Microphone,
    }
}

fn speaker_to_db(speaker: screamer_core::ambient::SpeakerLabel) -> &'static str {
    match speaker {
        screamer_core::ambient::SpeakerLabel::You => "you",
        screamer_core::ambient::SpeakerLabel::S1 => "s1",
        screamer_core::ambient::SpeakerLabel::S2 => "s2",
        screamer_core::ambient::SpeakerLabel::S3 => "s3",
        screamer_core::ambient::SpeakerLabel::S4 => "s4",
        screamer_core::ambient::SpeakerLabel::S5 => "s5",
        screamer_core::ambient::SpeakerLabel::S6 => "s6",
    }
}

fn speaker_from_db(value: &str) -> screamer_core::ambient::SpeakerLabel {
    match value {
        "s1" => screamer_core::ambient::SpeakerLabel::S1,
        "s2" => screamer_core::ambient::SpeakerLabel::S2,
        "s3" => screamer_core::ambient::SpeakerLabel::S3,
        "s4" => screamer_core::ambient::SpeakerLabel::S4,
        "s5" => screamer_core::ambient::SpeakerLabel::S5,
        "s6" => screamer_core::ambient::SpeakerLabel::S6,
        _ => screamer_core::ambient::SpeakerLabel::You,
    }
}

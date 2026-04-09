use crate::ambient_final_pass::{run_native_diarization_final_pass, AmbientFinalPassResult};
use crate::ambient_pipeline::{self, ProcessState};
use crate::config::{AmbientFinalBackendPreference, Config};
use crate::logging;
use crate::recorder::Recorder;
use crate::session_store::{SessionDetail, SessionStore};
use crate::summary_backend::SummaryBackendRegistry;
use screamer_core::ambient::{
    AmbientSessionConfig, AmbientSessionState, CanonicalSegment, DiarizationEngine, SummaryTemplate,
};
use screamer_whisper::Transcriber;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct AmbientSessionSnapshot {
    pub id: i64,
    pub title: String,
    pub state: AmbientSessionState,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub live_notes: String,
    pub structured_notes: String,
    pub transcript_markdown: String,
    pub scratch_pad: String,
    pub segments: Vec<CanonicalSegment>,
    pub warning: Option<String>,
    pub microphone_enabled: bool,
    pub system_audio_requested: bool,
    pub system_audio_active: bool,
    pub summary_backend_label: String,
    pub summary_template: SummaryTemplate,
    pub elapsed_ms: u64,
}

pub struct AmbientController {
    store: Arc<SessionStore>,
    transcriber: Arc<Transcriber>,
    summary_registry: Arc<SummaryBackendRegistry>,
    diarization_engine: Arc<dyn DiarizationEngine>,
    runtime: Mutex<Option<RuntimeSession>>,
}

struct RuntimeSession {
    session_id: i64,
    started_at: Instant,
    stop_signal: Arc<AtomicBool>,
    snapshot: Arc<Mutex<AmbientSessionSnapshot>>,
    worker: Option<JoinHandle<()>>,
}

impl AmbientController {
    pub fn new(
        store: Arc<SessionStore>,
        transcriber: Arc<Transcriber>,
        summary_registry: Arc<SummaryBackendRegistry>,
        diarization_engine: Arc<dyn DiarizationEngine>,
    ) -> Self {
        Self {
            store,
            transcriber,
            summary_registry,
            diarization_engine,
            runtime: Mutex::new(None),
        }
    }

    pub fn system_audio_runtime_supported(&self) -> bool {
        false
    }

    pub fn system_audio_runtime_reason(&self) -> &'static str {
        "System output capture is gated for a newer backend build; microphone ambient capture is available now."
    }

    pub fn start_session(&self, config: &Config) -> Result<i64, String> {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if runtime.is_some() {
            return Err("An ambient session is already running.".to_string());
        }

        let ambient_config = AmbientSessionConfig {
            enable_microphone: config.ambient_microphone,
            enable_system_audio: config.ambient_system_audio,
            ..AmbientSessionConfig::default()
        };
        if !ambient_config.enable_microphone && !ambient_config.enable_system_audio {
            return Err(
                "Enable microphone or system audio in Settings before starting notetaker."
                    .to_string(),
            );
        }

        let summary_backend_label = if config.summary_backend_label().is_empty() {
            "Bundled Gemma 3 1B".to_string()
        } else {
            config.summary_backend_label().to_string()
        };
        let session_id = self
            .store
            .create_session("Ambient session", &summary_backend_label)?;
        let warning =
            if ambient_config.enable_system_audio && !self.system_audio_runtime_supported() {
                Some(self.system_audio_runtime_reason().to_string())
            } else {
                None
            };
        let snapshot = Arc::new(Mutex::new(AmbientSessionSnapshot {
            id: session_id,
            title: "Ambient session".to_string(),
            state: AmbientSessionState::Recording,
            started_at_ms: unix_ms(),
            ended_at_ms: None,
            live_notes: String::new(),
            structured_notes: String::new(),
            transcript_markdown: String::new(),
            scratch_pad: String::new(),
            segments: Vec::new(),
            warning,
            microphone_enabled: ambient_config.enable_microphone,
            system_audio_requested: ambient_config.enable_system_audio,
            system_audio_active: false,
            summary_backend_label: summary_backend_label.clone(),
            summary_template: SummaryTemplate::General,
            elapsed_ms: 0,
        }));
        let stop_signal = Arc::new(AtomicBool::new(false));
        let worker = spawn_runtime_worker(
            session_id,
            ambient_config,
            config.clone(),
            self.store.clone(),
            self.transcriber.clone(),
            self.summary_registry.clone(),
            self.diarization_engine.clone(),
            snapshot.clone(),
            stop_signal.clone(),
        );

        *runtime = Some(RuntimeSession {
            session_id,
            started_at: Instant::now(),
            stop_signal,
            snapshot,
            worker: Some(worker),
        });
        Ok(session_id)
    }

    pub fn stop_session(&self) -> Result<(), String> {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(session) = runtime.as_ref() else {
            return Err("No active ambient session.".to_string());
        };

        session.stop_signal.store(true, Ordering::SeqCst);
        if let Ok(mut snapshot) = session.snapshot.lock() {
            snapshot.state = AmbientSessionState::Processing;
        }
        self.store
            .update_state(session.session_id, AmbientSessionState::Processing, None)?;
        Ok(())
    }

    pub fn tick(&self) {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(session) = runtime.as_mut() else {
            return;
        };

        let worker_finished = session
            .worker
            .as_ref()
            .map(|handle| handle.is_finished())
            .unwrap_or(false);

        if let Ok(mut snapshot) = session.snapshot.lock() {
            snapshot.elapsed_ms = session.started_at.elapsed().as_millis() as u64;
        }

        if worker_finished {
            if let Some(worker) = session.worker.take() {
                let _ = worker.join();
            }
            *runtime = None;
        }
    }

    pub fn active_snapshot(&self) -> Option<AmbientSessionSnapshot> {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let session = runtime.as_ref()?;
        session
            .snapshot
            .lock()
            .ok()
            .map(|snapshot| snapshot.clone())
    }

    pub fn load_session(&self, session_id: i64) -> Result<Option<AmbientSessionSnapshot>, String> {
        if let Some(active) = self.active_snapshot() {
            if active.id == session_id {
                return Ok(Some(active));
            }
        }

        let detail = self.store.load_session(session_id)?;
        Ok(detail.map(snapshot_from_detail))
    }

    pub fn persist_live_notes(&self, session_id: i64, live_notes: &str) -> Result<(), String> {
        if let Some(active) = self.active_snapshot() {
            if active.id == session_id {
                let runtime = self
                    .runtime
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(session) = runtime.as_ref() {
                    if let Ok(mut snapshot) = session.snapshot.lock() {
                        snapshot.live_notes.clear();
                        snapshot.live_notes.push_str(live_notes);
                    }
                }
            }
        }
        self.store.update_live_notes(session_id, live_notes)
    }

    pub fn set_summary_template(
        &self,
        session_id: i64,
        template: SummaryTemplate,
    ) -> Result<(), String> {
        if let Some(active) = self.active_snapshot() {
            if active.id == session_id {
                let runtime = self
                    .runtime
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(session) = runtime.as_ref() {
                    if let Ok(mut snapshot) = session.snapshot.lock() {
                        snapshot.summary_template = template;
                    }
                }
            }
        }
        self.store.update_summary_template(session_id, template)
    }

    pub fn reprocess_session(&self, session_id: i64, config: &Config) -> Result<(), String> {
        // Don't reprocess if there's an active recording session
        if self.active_snapshot().is_some() {
            return Err("Cannot reprocess while a session is active.".to_string());
        }

        let detail = self
            .store
            .load_session(session_id)?
            .ok_or_else(|| "Session not found.".to_string())?;

        // Set state to Processing
        self.store
            .update_state(session_id, AmbientSessionState::Processing, None)?;

        let store = self.store.clone();
        let summary_registry = self.summary_registry.clone();
        let app_config = config.clone();
        std::thread::spawn(move || {
            let cleaned_segments =
                screamer_core::ambient::clean_canonical_segments(&detail.segments);
            let live_notes = screamer_core::ambient::segments_to_transcript(&cleaned_segments);
            if cleaned_segments != detail.segments {
                let _ = store.replace_segments(session_id, &cleaned_segments);
                let _ = store.update_live_notes(session_id, &live_notes);
            }
            let summarizer = summary_registry.summarizer_for_config(&app_config);
            let title_hint =
                summary_registry.concise_session_title(&app_config, &live_notes, &cleaned_segments);
            let notes_with_scratch = if detail.scratch_pad.trim().is_empty() {
                live_notes.clone()
            } else {
                format!(
                    "--- User Notes (Scratch Pad) ---\n{}\n--- End User Notes ---\n\n{}",
                    detail.scratch_pad, live_notes
                )
            };
            let structured_notes = summarizer
                .summarize(
                    &notes_with_scratch,
                    &cleaned_segments,
                    Some(&title_hint),
                    detail.summary_template,
                )
                .map(|notes| notes.to_markdown())
                .unwrap_or_else(|err| {
                    screamer_core::ambient::polish_summary_markdown(&format!(
                        "## Summary\n\n{}\n\n## Key Points\n\n- {}\n",
                        title_hint, err
                    ))
                });
            let transcript_markdown = live_notes.clone();

            // Generate a better title from the completed summary
            let title = summary_registry.title_from_summary(
                &app_config,
                &structured_notes,
                &live_notes,
                &cleaned_segments,
            );

            let final_state = if structured_notes.is_empty() {
                AmbientSessionState::Failed
            } else {
                AmbientSessionState::Completed
            };
            let _ = store.update_structured_notes(
                session_id,
                &title,
                &structured_notes,
                &transcript_markdown,
            );
            let ended_at = unix_ms();
            let _ = store.update_state(session_id, final_state, Some(ended_at));
            logging::log_ambient_session_report(
                "reprocess",
                session_id,
                &title,
                ambient_state_label(final_state),
                detail.started_at_ms,
                ended_at,
                &app_config.summary_backend_label(),
                detail.summary_template.to_db(),
                None,
                &transcript_markdown,
                &structured_notes,
            );
        });
        Ok(())
    }

    pub fn persist_scratch_pad(&self, session_id: i64, scratch_pad: &str) -> Result<(), String> {
        if let Some(active) = self.active_snapshot() {
            if active.id == session_id {
                let runtime = self
                    .runtime
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(session) = runtime.as_ref() {
                    if let Ok(mut snapshot) = session.snapshot.lock() {
                        snapshot.scratch_pad.clear();
                        snapshot.scratch_pad.push_str(scratch_pad);
                    }
                }
            }
        }
        self.store.update_scratch_pad(session_id, scratch_pad)
    }
}

fn spawn_runtime_worker(
    session_id: i64,
    ambient_config: AmbientSessionConfig,
    app_config: Config,
    store: Arc<SessionStore>,
    transcriber: Arc<Transcriber>,
    summary_registry: Arc<SummaryBackendRegistry>,
    diarization_engine: Arc<dyn DiarizationEngine>,
    snapshot: Arc<Mutex<AmbientSessionSnapshot>>,
    stop_signal: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let recorder = if ambient_config.enable_microphone {
            let recorder = Arc::new(Recorder::new());
            match recorder.start() {
                Ok(()) => Some(recorder),
                Err(err) => {
                    let _ = store.update_state(
                        session_id,
                        AmbientSessionState::Failed,
                        Some(unix_ms()),
                    );
                    if let Ok(mut state) = snapshot.lock() {
                        state.state = AmbientSessionState::Failed;
                        state.warning = Some(format!("Unable to start microphone capture: {err}"));
                    }
                    return;
                }
            }
        } else {
            None
        };

        let mut process_state = ProcessState::default();
        let mut final_samples = Vec::new();

        while !stop_signal.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1_200));
            if let Some(recorder) = &recorder {
                let snapshot_samples = recorder.snapshot();
                let _ = process_audio_snapshot(
                    session_id,
                    &ambient_config,
                    &transcriber,
                    &*diarization_engine,
                    &store,
                    &snapshot,
                    &snapshot_samples,
                    &mut process_state,
                );
            }
        }

        if let Some(recorder) = &recorder {
            final_samples = recorder.stop();
            let _ = process_audio_snapshot(
                session_id,
                &ambient_config,
                &transcriber,
                &*diarization_engine,
                &store,
                &snapshot,
                &final_samples,
                &mut process_state,
            );
        }

        let (
            started_at_ms,
            mut live_notes,
            mut segments,
            scratch_pad,
            summary_template,
            mut warning,
        ) = {
            let state = snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                state.started_at_ms,
                state.live_notes.clone(),
                state.segments.clone(),
                state.scratch_pad.clone(),
                state.summary_template,
                state.warning.clone(),
            )
        };

        if matches!(
            app_config.ambient_final_backend,
            AmbientFinalBackendPreference::NativeDiarization
        ) && !final_samples.is_empty()
        {
            match run_native_ambient_final_pass(&transcriber, &final_samples) {
                Ok(final_pass) => {
                    eprintln!(
                        "[screamer] Ambient final pass={} total={}ms speakers={} chars={}",
                        final_pass.engine,
                        final_pass.diagnostics.total_ms,
                        final_pass.diagnostics.detected_speakers,
                        final_pass.transcript_text.chars().count()
                    );
                    segments = final_pass.segments;
                    live_notes = screamer_core::ambient::segments_to_transcript(&segments);
                    let _ = store.replace_segments(session_id, &segments);
                    let _ = store.update_live_notes(session_id, &live_notes);
                    if let Some(diagnostic_warning) = final_pass.diagnostics.warning.as_deref() {
                        warning = Some(append_warning(warning.as_deref(), diagnostic_warning));
                    }
                    if let Ok(mut state) = snapshot.lock() {
                        state.live_notes = live_notes.clone();
                        state.segments = segments.clone();
                        state.transcript_markdown = live_notes.clone();
                        state.warning = warning.clone();
                    }
                }
                Err(err) => {
                    eprintln!(
                        "[screamer] Native diarization final pass failed; falling back to native transcript: {err}"
                    );
                    warning = Some(append_warning(
                        warning.as_deref(),
                        &format!(
                            "Native diarization final pass failed; using native transcript. {err}"
                        ),
                    ));
                    if let Ok(mut state) = snapshot.lock() {
                        state.warning = warning.clone();
                    }
                }
            }
        }

        let cleaned_segments = screamer_core::ambient::clean_canonical_segments(&segments);
        if cleaned_segments != segments {
            segments = cleaned_segments;
            live_notes = screamer_core::ambient::segments_to_transcript(&segments);
            let _ = store.replace_segments(session_id, &segments);
            let _ = store.update_live_notes(session_id, &live_notes);
            if let Ok(mut state) = snapshot.lock() {
                state.live_notes = live_notes.clone();
                state.segments = segments.clone();
                state.transcript_markdown = live_notes.clone();
            }
        }

        let summarizer = summary_registry.summarizer_for_config(&app_config);
        // Use a heuristic title hint for the summarizer prompt
        let title_hint =
            summary_registry.concise_session_title(&app_config, &live_notes, &segments);
        let notes_with_scratch = if scratch_pad.trim().is_empty() {
            live_notes.clone()
        } else {
            format!(
                "--- User Notes (Scratch Pad) ---\n{}\n--- End User Notes ---\n\n{}",
                scratch_pad, live_notes
            )
        };
        let structured_notes = summarizer
            .summarize(
                &notes_with_scratch,
                &segments,
                Some(&title_hint),
                summary_template,
            )
            .map(|notes| notes.to_markdown())
            .unwrap_or_else(|err| {
                screamer_core::ambient::polish_summary_markdown(&format!(
                    "## Summary\n\n{}\n\n## Key Points\n\n- {}\n",
                    title_hint, err
                ))
            });
        let transcript_markdown = screamer_core::ambient::segments_to_transcript(&segments);

        // Generate a better title from the completed summary
        let title = summary_registry.title_from_summary(
            &app_config,
            &structured_notes,
            &live_notes,
            &segments,
        );

        let final_state = if structured_notes.is_empty() {
            AmbientSessionState::Failed
        } else {
            AmbientSessionState::Completed
        };
        let ended_at = unix_ms();
        let _ = store.update_structured_notes(
            session_id,
            &title,
            &structured_notes,
            &transcript_markdown,
        );
        let _ = store.update_state(session_id, final_state, Some(ended_at));

        logging::log_ambient_session_report(
            "recording",
            session_id,
            &title,
            ambient_state_label(final_state),
            started_at_ms,
            ended_at,
            &app_config.summary_backend_label(),
            summary_template.to_db(),
            warning.as_deref(),
            &transcript_markdown,
            &structured_notes,
        );

        if let Ok(mut state) = snapshot.lock() {
            state.title = title;
            state.state = final_state;
            state.structured_notes = structured_notes;
            state.transcript_markdown = transcript_markdown;
            state.warning = warning;
            state.ended_at_ms = Some(ended_at);
            state.elapsed_ms = state
                .ended_at_ms
                .map(|ended_at_ms| ended_at_ms.saturating_sub(state.started_at_ms) as u64)
                .unwrap_or(state.elapsed_ms);
        }
    })
}

fn process_audio_snapshot(
    session_id: i64,
    ambient_config: &AmbientSessionConfig,
    transcriber: &Transcriber,
    diarization_engine: &dyn DiarizationEngine,
    store: &SessionStore,
    snapshot: &Arc<Mutex<AmbientSessionSnapshot>>,
    samples: &[f32],
    process_state: &mut ProcessState,
) -> Result<(), String> {
    let existing_segments = {
        let state = snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.segments.clone()
    };
    let mut merged_segments = existing_segments.clone();
    let did_change = ambient_pipeline::process_audio_snapshot(
        ambient_config,
        transcriber,
        diarization_engine,
        samples,
        process_state,
        &mut merged_segments,
    )?;

    if !did_change {
        return Ok(());
    }

    if let Some(updated_tail) = updated_existing_tail(&existing_segments, &merged_segments) {
        store.update_last_segment(session_id, updated_tail)?;
    }

    if merged_segments.len() > existing_segments.len() {
        store.append_segments(session_id, &merged_segments[existing_segments.len()..])?;
    }

    let live_notes = screamer_core::ambient::segments_to_transcript(&merged_segments);
    let transcript_markdown = live_notes.clone();

    let mut state = snapshot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.live_notes = live_notes.clone();
    state.segments = merged_segments;
    state.transcript_markdown = transcript_markdown;

    store.update_live_notes(session_id, &live_notes)?;
    Ok(())
}

fn updated_existing_tail<'a>(
    previous: &'a [CanonicalSegment],
    current: &'a [CanonicalSegment],
) -> Option<&'a CanonicalSegment> {
    let index = previous.len().checked_sub(1)?;
    let previous_tail = previous.get(index)?;
    let current_tail = current.get(index)?;
    (previous_tail != current_tail).then_some(current_tail)
}

fn snapshot_from_detail(detail: SessionDetail) -> AmbientSessionSnapshot {
    let elapsed_ms = detail
        .ended_at_ms
        .map(|ended_at_ms| ended_at_ms.saturating_sub(detail.started_at_ms) as u64)
        .unwrap_or_else(|| unix_ms().saturating_sub(detail.started_at_ms) as u64);
    AmbientSessionSnapshot {
        id: detail.id,
        title: detail.title,
        state: detail.state,
        started_at_ms: detail.started_at_ms,
        ended_at_ms: detail.ended_at_ms,
        live_notes: detail.live_notes,
        structured_notes: detail.structured_notes,
        transcript_markdown: detail.transcript_markdown,
        scratch_pad: detail.scratch_pad,
        segments: detail.segments,
        warning: None,
        microphone_enabled: true,
        system_audio_requested: true,
        system_audio_active: false,
        summary_backend_label: "Bundled Gemma 3 1B".to_string(),
        summary_template: detail.summary_template,
        elapsed_ms,
    }
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn ambient_state_label(state: AmbientSessionState) -> &'static str {
    match state {
        AmbientSessionState::Idle => "idle",
        AmbientSessionState::Recording => "recording",
        AmbientSessionState::Processing => "processing",
        AmbientSessionState::Completed => "completed",
        AmbientSessionState::Failed => "failed",
    }
}

fn run_native_ambient_final_pass(
    transcriber: &Transcriber,
    samples: &[f32],
) -> Result<AmbientFinalPassResult, String> {
    run_native_diarization_final_pass(samples, transcriber)
}

fn append_warning(existing: Option<&str>, incoming: &str) -> String {
    match existing.map(str::trim).filter(|text| !text.is_empty()) {
        Some(existing) if existing.contains(incoming) => existing.to_string(),
        Some(existing) => format!("{existing}\n{incoming}"),
        None => incoming.to_string(),
    }
}

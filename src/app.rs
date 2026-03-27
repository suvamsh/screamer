use crate::config::{Config, HOTKEYS, MODELS, POSITIONS};
use crate::overlay::{Overlay, WAVEFORM_BINS};
use crate::recorder::Recorder;
use crate::sound::SoundPlayer;
use crate::transcriber::Transcriber;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::sel;
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSApplication, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
};
use objc2_foundation::{MainThreadMarker, NSString};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const LIVE_TRANSCRIPTION_INTERVAL: Duration = Duration::from_millis(350);
const LIVE_TRANSCRIPTION_MIN_SAMPLES: usize = 9600;
const LIVE_TRANSCRIPTION_MIN_DELTA: usize = 2400;
const LIVE_TRANSCRIPTION_MAX_SAMPLES: usize = 192_000;
const LIVE_TRANSCRIPTION_PADDING_SAMPLES: usize = 8000;
const LIVE_TRANSCRIPT_MAX_CHARS: usize = 180;
const SOUND_EFFECT_ARM_DELAY: Duration = Duration::from_millis(140);
const SPEECH_DETECTION_LOOKBACK_SAMPLES: usize = 16_000;
const SPEECH_DETECTION_FRAME_SAMPLES: usize = 320;
const SPEECH_DETECTION_FRAME_RMS_GATE: f32 = 0.006;
const SPEECH_DETECTION_MIN_ACTIVE_FRAMES: usize = 3;

// Thread-local overlay reference so menu handlers can update position without relaunch.
// Safe because all menu handlers and overlay access run on the main thread.
thread_local! {
    static OVERLAY: RefCell<Option<Rc<RefCell<Overlay>>>> = const { RefCell::new(None) };
    static LIVE_TRANSCRIPTION_ENABLED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
    static SOUND_EFFECTS_ENABLED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

pub struct App {
    _status_item: Retained<NSStatusItem>,
    overlay: Rc<RefCell<Overlay>>,
    recorder: Arc<Recorder>,
    sound_player: Rc<SoundPlayer>,
    transcriber: Arc<Transcriber>,
    is_recording: Arc<AtomicBool>,
    live_transcription_enabled: Arc<AtomicBool>,
    sound_effects_enabled: Arc<AtomicBool>,
    live_transcript: Arc<Mutex<String>>,
    pending_completion_sound: Arc<AtomicBool>,
    recording_session: Arc<AtomicU64>,
}

pub struct AppInitError {
    pub title: &'static str,
    pub message: String,
}

// ─── ObjC Menu Handler ───────────────────────────────────────────────────────

fn menu_handler_class() -> &'static AnyClass {
    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        let superclass = AnyClass::get(c"NSObject").unwrap();
        let mut builder = ClassBuilder::new(c"ScreamerMenuHandler", superclass).unwrap();

        unsafe {
            builder.add_method(
                sel!(selectModel:),
                select_model_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(selectHotkey:),
                select_hotkey_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(selectPosition:),
                select_position_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(toggleLiveTranscription:),
                toggle_live_transcription_action
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(toggleSoundEffects:),
                toggle_sound_effects_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
        }

        builder.register()
    })
}

fn get_menu_handler() -> *const AnyObject {
    static HANDLER: OnceLock<usize> = OnceLock::new();
    *HANDLER.get_or_init(|| {
        let cls = menu_handler_class();
        let obj: *mut AnyObject = unsafe { msg_send![cls, new] };
        obj as usize
    }) as *const AnyObject
}

extern "C" fn select_model_action(_this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject) {
    let tag: isize = unsafe { msg_send![sender, tag] };
    if let Some(model_info) = MODELS.get(tag as usize) {
        eprintln!("[screamer] Model selected: {}", model_info.id);

        if Transcriber::find_model(model_info.id).is_none() {
            eprintln!("[screamer] Model not found: {}", model_info.id);
            if let Some(mtm) = MainThreadMarker::new() {
                let alert = NSAlert::new(mtm);
                alert.setAlertStyle(NSAlertStyle::Warning);
                alert.setMessageText(&NSString::from_str("Model Not Downloaded"));
                alert.setInformativeText(&NSString::from_str(&format!(
                    "The {} model ({}) hasn't been downloaded yet.\n\n\
                     Run this in Terminal:\n\
                     cd {} && ./download_model.sh {}",
                    model_info.label,
                    model_info.size,
                    std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    model_info.id
                )));
                alert.runModal();
            }
            return;
        }

        let mut config = Config::load();
        config.model = model_info.id.to_string();
        config.save();
        relaunch();
    }
}

extern "C" fn select_hotkey_action(_this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject) {
    let tag: isize = unsafe { msg_send![sender, tag] };
    if let Some(hotkey_info) = HOTKEYS.get(tag as usize) {
        eprintln!("[screamer] Hotkey selected: {}", hotkey_info.id);
        let mut config = Config::load();
        config.hotkey = hotkey_info.id.to_string();
        config.save();
        relaunch();
    }
}

extern "C" fn select_position_action(_this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject) {
    let tag: isize = unsafe { msg_send![sender, tag] };
    if let Some(pos_info) = POSITIONS.get(tag as usize) {
        eprintln!("[screamer] Position selected: {}", pos_info.label);

        let mut config = Config::load();
        config.overlay_position = pos_info.id;
        config.save();

        // Apply immediately without relaunch
        if let Some(mtm) = MainThreadMarker::new() {
            OVERLAY.with(|cell| {
                if let Some(overlay) = cell.borrow().as_ref() {
                    if let Ok(mut ov) = overlay.try_borrow_mut() {
                        ov.set_position(mtm, pos_info.id);
                    }
                }
            });
        }
    }
}

extern "C" fn toggle_live_transcription_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let enabled = LIVE_TRANSCRIPTION_ENABLED.with(|cell| {
        let borrowed = cell.borrow();
        let flag = borrowed.as_ref()?;

        let enabled = !flag.load(Ordering::Relaxed);
        flag.store(enabled, Ordering::Relaxed);
        Some(enabled)
    });

    let Some(enabled) = enabled else {
        return;
    };

    let mut config = Config::load();
    config.live_transcription = enabled;
    config.save();

    let state = if enabled { 1isize } else { 0isize };
    let _: () = unsafe { msg_send![sender, setState: state] };
    eprintln!(
        "[screamer] Live transcription {}",
        if enabled { "enabled" } else { "disabled" }
    );
}

extern "C" fn toggle_sound_effects_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let enabled = SOUND_EFFECTS_ENABLED.with(|cell| {
        let borrowed = cell.borrow();
        let flag = borrowed.as_ref()?;

        let enabled = !flag.load(Ordering::Relaxed);
        flag.store(enabled, Ordering::Relaxed);
        Some(enabled)
    });

    let Some(enabled) = enabled else {
        return;
    };

    let mut config = Config::load();
    config.sound_effects = enabled;
    config.save();

    let state = if enabled { 1isize } else { 0isize };
    let _: () = unsafe { msg_send![sender, setState: state] };
    eprintln!(
        "[screamer] Sound effects {}",
        if enabled { "enabled" } else { "disabled" }
    );
}

fn relaunch() {
    eprintln!("[screamer] Relaunching...");
    if let Ok(exe) = std::env::current_exe() {
        if let Some(app_path) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            let app_str = app_path.display().to_string();
            eprintln!("[screamer] Relaunching from: {}", app_str);
            let _ = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("sleep 0.5 && open -a '{}'", app_str))
                .spawn();
        } else {
            let exe_str = exe.display().to_string();
            eprintln!("[screamer] Relaunching binary: {}", exe_str);
            let _ = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("sleep 0.5 && '{}'", exe_str))
                .spawn();
        }
    }

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        app.terminate(None);
    }
}

// ─── App ─────────────────────────────────────────────────────────────────────

impl App {
    pub fn load_transcriber(config: &Config) -> Result<Arc<Transcriber>, AppInitError> {
        let model_path = match Transcriber::find_model(&config.model) {
            Some(p) => p,
            None => {
                return Err(AppInitError {
                    title: "Model Not Found",
                    message: format!(
                        "Could not find the whisper model 'ggml-{}.en.bin'.\n\n\
                         Please run: ./download_model.sh {}\n\n\
                         This will download the model to the models/ directory.",
                        config.model, config.model
                    ),
                });
            }
        };

        log::info!("Loading model from: {:?}", model_path);

        let transcriber = match Transcriber::new(&model_path) {
            Ok(t) => Arc::new(t),
            Err(e) => {
                return Err(AppInitError {
                    title: "Transcription Error",
                    message: e,
                });
            }
        };

        log::info!("Transcriber runtime: {}", transcriber.runtime_summary());
        log::info!("Model loaded successfully");

        Ok(transcriber)
    }

    pub fn new_with_transcriber(
        mtm: MainThreadMarker,
        config: Config,
        transcriber: Arc<Transcriber>,
    ) -> Result<Self, AppInitError> {
        let recorder = Arc::new(Recorder::new());
        let overlay = Rc::new(RefCell::new(Overlay::new(mtm, config.overlay_position)));
        let sound_player = Rc::new(SoundPlayer::new(mtm));

        // Store overlay reference for position menu handler
        OVERLAY.with(|cell| {
            *cell.borrow_mut() = Some(overlay.clone());
        });

        let live_transcription_enabled = Arc::new(AtomicBool::new(config.live_transcription));
        LIVE_TRANSCRIPTION_ENABLED.with(|cell| {
            *cell.borrow_mut() = Some(live_transcription_enabled.clone());
        });
        let sound_effects_enabled = Arc::new(AtomicBool::new(config.sound_effects));
        SOUND_EFFECTS_ENABLED.with(|cell| {
            *cell.borrow_mut() = Some(sound_effects_enabled.clone());
        });

        let status_bar = NSStatusBar::systemStatusBar();
        let status_item = status_bar.statusItemWithLength(objc2_app_kit::NSSquareStatusItemLength);

        if let Some(button) = status_item.button(mtm) {
            let icon_name = NSString::from_str("menubarTemplate");
            if let Some(image) = objc2_app_kit::NSImage::imageNamed(&icon_name) {
                unsafe {
                    let _: () = msg_send![&image, setTemplate: true];
                    let _: () =
                        msg_send![&image, setSize: objc2_core_foundation::CGSize::new(18.0, 18.0)];
                }
                button.setImage(Some(&image));
            } else {
                button.setTitle(&NSString::from_str("\u{1f3a4}"));
            }
        }

        let menu = Self::build_menu(mtm, &config);
        status_item.setMenu(Some(&menu));

        Ok(Self {
            _status_item: status_item,
            overlay,
            recorder,
            sound_player,
            transcriber,
            is_recording: Arc::new(AtomicBool::new(false)),
            live_transcription_enabled,
            sound_effects_enabled,
            live_transcript: Arc::new(Mutex::new(String::new())),
            pending_completion_sound: Arc::new(AtomicBool::new(false)),
            recording_session: Arc::new(AtomicU64::new(0)),
        })
    }

    fn build_menu(mtm: MainThreadMarker, config: &Config) -> Retained<NSMenu> {
        let handler = get_menu_handler();

        unsafe {
            let menu = NSMenu::new(mtm);

            let status_line = NSMenuItem::new(mtm);
            status_line.setTitle(&NSString::from_str("Screamer — Ready"));
            status_line.setEnabled(false);
            menu.addItem(&status_line);

            menu.addItem(&NSMenuItem::separatorItem(mtm));

            // ── Model submenu ──
            let model_item = NSMenuItem::new(mtm);
            model_item.setTitle(&NSString::from_str(&format!("Model: {}", config.model)));
            let model_submenu = NSMenu::new(mtm);
            model_submenu.setTitle(&NSString::from_str("Model"));

            for (i, model_info) in MODELS.iter().enumerate() {
                let item = NSMenuItem::new(mtm);
                let available = Transcriber::find_model(model_info.id).is_some();
                let title = if available {
                    format!("{} ({})", model_info.label, model_info.size)
                } else {
                    format!(
                        "{} ({}) — Not Downloaded",
                        model_info.label, model_info.size
                    )
                };
                item.setTitle(&NSString::from_str(&title));
                item.setTag(i as isize);
                let _: () = msg_send![&*item, setTarget: handler];
                item.setAction(Some(sel!(selectModel:)));

                if model_info.id == config.model {
                    item.setState(1);
                }

                model_submenu.addItem(&item);
            }

            model_item.setSubmenu(Some(&model_submenu));
            menu.addItem(&model_item);

            // ── Hotkey submenu ──
            let hotkey_item = NSMenuItem::new(mtm);
            hotkey_item.setTitle(&NSString::from_str(&format!(
                "Hotkey: {}",
                config.hotkey_label()
            )));
            let hotkey_submenu = NSMenu::new(mtm);
            hotkey_submenu.setTitle(&NSString::from_str("Hotkey"));

            for (i, hotkey_info) in HOTKEYS.iter().enumerate() {
                let item = NSMenuItem::new(mtm);
                item.setTitle(&NSString::from_str(hotkey_info.label));
                item.setTag(i as isize);
                let _: () = msg_send![&*item, setTarget: handler];
                item.setAction(Some(sel!(selectHotkey:)));

                if hotkey_info.id == config.hotkey {
                    item.setState(1);
                }

                hotkey_submenu.addItem(&item);
            }

            hotkey_item.setSubmenu(Some(&hotkey_submenu));
            menu.addItem(&hotkey_item);

            // ── Position submenu ──
            let pos_item = NSMenuItem::new(mtm);
            pos_item.setTitle(&NSString::from_str(&format!(
                "Position: {}",
                config.position_label()
            )));
            let pos_submenu = NSMenu::new(mtm);
            pos_submenu.setTitle(&NSString::from_str("Position"));

            for (i, pos_info) in POSITIONS.iter().enumerate() {
                let item = NSMenuItem::new(mtm);
                item.setTitle(&NSString::from_str(pos_info.label));
                item.setTag(i as isize);
                let _: () = msg_send![&*item, setTarget: handler];
                item.setAction(Some(sel!(selectPosition:)));

                if pos_info.id == config.overlay_position {
                    item.setState(1);
                }

                pos_submenu.addItem(&item);
            }

            pos_item.setSubmenu(Some(&pos_submenu));
            menu.addItem(&pos_item);

            let live_item = NSMenuItem::new(mtm);
            live_item.setTitle(&NSString::from_str("Live Transcription"));
            let _: () = msg_send![&*live_item, setTarget: handler];
            live_item.setAction(Some(sel!(toggleLiveTranscription:)));
            if config.live_transcription {
                live_item.setState(1);
            }
            menu.addItem(&live_item);

            let sound_item = NSMenuItem::new(mtm);
            sound_item.setTitle(&NSString::from_str("Sound Effects"));
            let _: () = msg_send![&*sound_item, setTarget: handler];
            sound_item.setAction(Some(sel!(toggleSoundEffects:)));
            if config.sound_effects {
                sound_item.setState(1);
            }
            menu.addItem(&sound_item);

            menu.addItem(&NSMenuItem::separatorItem(mtm));

            // ── Quit ──
            let quit_item = NSMenuItem::new(mtm);
            quit_item.setTitle(&NSString::from_str("Quit Screamer"));
            quit_item.setAction(Some(sel!(terminate:)));
            quit_item.setKeyEquivalent(&NSString::from_str("q"));
            menu.addItem(&quit_item);

            menu
        }
    }

    pub fn start(&self, mtm: MainThreadMarker) {
        let recorder = self.recorder.clone();
        let sound_player_press = self.sound_player.clone();
        let transcriber = self.transcriber.clone();
        let is_recording = self.is_recording.clone();
        let live_transcription_enabled = self.live_transcription_enabled.clone();
        let sound_effects_enabled = self.sound_effects_enabled.clone();
        let live_transcript = self.live_transcript.clone();
        let pending_completion_sound = self.pending_completion_sound.clone();
        let recording_session = self.recording_session.clone();

        let rec_press = recorder.clone();
        let rec_release = recorder.clone();
        let trans_press = transcriber.clone();
        let trans_release = transcriber.clone();
        let is_rec_press = is_recording.clone();
        let is_rec_release = is_recording.clone();
        let live_transcription_enabled_press = live_transcription_enabled.clone();
        let sound_effects_enabled_press = sound_effects_enabled.clone();
        let sound_effects_enabled_release = sound_effects_enabled.clone();
        let live_transcript_press = live_transcript.clone();
        let live_transcript_release = live_transcript.clone();
        let pending_completion_sound_release = pending_completion_sound.clone();
        let recording_session_press = recording_session.clone();

        let hotkey = crate::hotkey::Hotkey::new();

        hotkey.start_on_main_thread(
            mtm,
            move || {
                if is_rec_press
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let session = recording_session_press.fetch_add(1, Ordering::SeqCst) + 1;
                    if let Ok(mut transcript) = live_transcript_press.lock() {
                        transcript.clear();
                    }
                    rec_press.reset_buffers();

                    if sound_effects_enabled_press.load(Ordering::Relaxed) {
                        sound_player_press.play_recording_start();

                        let recorder = rec_press.clone();
                        let transcriber = trans_press.clone();
                        let is_recording = is_rec_press.clone();
                        let live_enabled = live_transcription_enabled_press.clone();
                        let live_transcript = live_transcript_press.clone();
                        let recording_session = recording_session_press.clone();

                        std::thread::spawn(move || {
                            std::thread::sleep(SOUND_EFFECT_ARM_DELAY);

                            start_recording_capture(
                                recorder,
                                transcriber,
                                is_recording,
                                live_enabled,
                                live_transcript,
                                recording_session,
                                session,
                            );
                        });
                    } else {
                        start_recording_capture(
                            rec_press.clone(),
                            trans_press.clone(),
                            is_rec_press.clone(),
                            live_transcription_enabled_press.clone(),
                            live_transcript_press.clone(),
                            recording_session_press.clone(),
                            session,
                        );
                    }
                    eprintln!("[screamer] Recording armed");
                }
            },
            move || {
                if is_rec_release
                    .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let release_t0 = std::time::Instant::now();
                    let samples = rec_release.stop();
                    if let Ok(mut transcript) = live_transcript_release.lock() {
                        transcript.clear();
                    }
                    eprintln!("[screamer] Recording stopped, {} samples", samples.len());

                    if samples.len() < 4800 {
                        eprintln!("[screamer] Recording too short, skipping");
                        if sound_effects_enabled_release.load(Ordering::Relaxed) {
                            pending_completion_sound_release.store(true, Ordering::SeqCst);
                        }
                        return;
                    }

                    if !samples_contain_speech(&samples) {
                        eprintln!("[screamer] Recording was silence, skipping transcription");
                        if sound_effects_enabled_release.load(Ordering::Relaxed) {
                            pending_completion_sound_release.store(true, Ordering::SeqCst);
                        }
                        return;
                    }

                    let t = trans_release.clone();
                    let pending_completion_sound = pending_completion_sound_release.clone();
                    let sound_effects_enabled = sound_effects_enabled_release.clone();
                    let stop_ms = release_t0.elapsed().as_millis();
                    std::thread::spawn(move || {
                        match t.transcribe_profiled(&samples) {
                            Ok(result) if !result.text.is_empty() => {
                                eprintln!(
                                    "[screamer] Transcribed in {}ms: {}",
                                    result.profile.total.as_millis(),
                                    result.text
                                );

                                let paste_t0 = std::time::Instant::now();
                                crate::paster::paste(&result.text);
                                let paste_ms = paste_t0.elapsed().as_millis();

                                eprintln!(
                                    "[screamer] Latency breakdown: stop={}ms | state={}ms | infer={}ms | extract={}ms | paste={}ms | total={}ms",
                                    stop_ms,
                                    result.profile.state_acquire.as_millis(),
                                    result.profile.inference.as_millis(),
                                    result.profile.extract.as_millis(),
                                    paste_ms,
                                    release_t0.elapsed().as_millis()
                                );
                            }
                            Ok(_) => {
                                eprintln!("[screamer] Empty transcription, skipping paste");
                            }
                            Err(e) => {
                                eprintln!("[screamer] Transcription error: {}", e);
                            }
                        }

                        if sound_effects_enabled.load(Ordering::Relaxed) {
                            pending_completion_sound.store(true, Ordering::SeqCst);
                        }
                    });
                }
            },
        );

        self.start_waveform_timer();
    }

    fn start_waveform_timer(&self) {
        let recorder = self.recorder.clone();
        let overlay = self.overlay.clone();
        let sound_player = self.sound_player.clone();
        let is_recording = self.is_recording.clone();
        let live_transcription_enabled = self.live_transcription_enabled.clone();
        let sound_effects_enabled = self.sound_effects_enabled.clone();
        let live_transcript = self.live_transcript.clone();
        let pending_completion_sound = self.pending_completion_sound.clone();

        unsafe {
            use objc2::ClassType;
            use objc2_foundation::NSTimer;

            let block = block2::RcBlock::new(move |_timer: *mut objc2::runtime::AnyObject| {
                let recording = is_recording.load(Ordering::Relaxed);
                if !sound_effects_enabled.load(Ordering::Relaxed) {
                    pending_completion_sound.store(false, Ordering::SeqCst);
                } else if !recording && pending_completion_sound.swap(false, Ordering::SeqCst) {
                    sound_player.play_processing_done();
                }

                if let Ok(mut ov) = overlay.try_borrow_mut() {
                    if recording {
                        if !ov.is_visible() {
                            ov.show();
                        }
                        let waveform = recorder.latest_waveform(WAVEFORM_BINS);
                        let transcript = if live_transcription_enabled.load(Ordering::Relaxed) {
                            live_transcript
                                .lock()
                                .map(|text| text.clone())
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };
                        ov.update_waveform(&waveform);
                        ov.update_transcript(&transcript);
                    } else if ov.is_visible() {
                        ov.hide();
                    }
                }
            });

            let _timer: Retained<NSTimer> = msg_send![
                <NSTimer as ClassType>::class(),
                scheduledTimerWithTimeInterval: (1.0f64 / 30.0f64),
                repeats: true,
                block: &*block
            ];
        }
    }

    pub fn show_alert(mtm: MainThreadMarker, title: &str, message: &str) {
        let alert = NSAlert::new(mtm);
        alert.setAlertStyle(NSAlertStyle::Critical);
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(message));
        alert.runModal();
    }
}

fn start_recording_capture(
    recorder: Arc<Recorder>,
    transcriber: Arc<Transcriber>,
    is_recording: Arc<AtomicBool>,
    live_transcription_enabled: Arc<AtomicBool>,
    live_transcript: Arc<Mutex<String>>,
    recording_session: Arc<AtomicU64>,
    session: u64,
) {
    if !is_recording.load(Ordering::Relaxed) || recording_session.load(Ordering::Relaxed) != session
    {
        return;
    }

    recorder.start();

    if !is_recording.load(Ordering::Relaxed) || recording_session.load(Ordering::Relaxed) != session
    {
        let _ = recorder.stop();
        return;
    }

    if live_transcription_enabled.load(Ordering::Relaxed) {
        spawn_live_transcription_worker(
            recorder,
            transcriber,
            is_recording,
            live_transcription_enabled,
            live_transcript,
            recording_session,
            session,
        );
    }

    eprintln!("[screamer] Audio capture started");
}

fn spawn_live_transcription_worker(
    recorder: Arc<Recorder>,
    transcriber: Arc<Transcriber>,
    is_recording: Arc<AtomicBool>,
    live_transcription_enabled: Arc<AtomicBool>,
    live_transcript: Arc<Mutex<String>>,
    recording_session: Arc<AtomicU64>,
    session: u64,
) {
    std::thread::spawn(move || {
        let mut last_transcribed_samples = 0usize;
        let mut last_text = String::new();

        while is_recording.load(Ordering::Relaxed)
            && recording_session.load(Ordering::Relaxed) == session
        {
            std::thread::sleep(LIVE_TRANSCRIPTION_INTERVAL);

            if !is_recording.load(Ordering::Relaxed)
                || recording_session.load(Ordering::Relaxed) != session
            {
                break;
            }

            if !live_transcription_enabled.load(Ordering::Relaxed) {
                if let Ok(mut transcript) = live_transcript.lock() {
                    transcript.clear();
                }
                continue;
            }

            let samples = recorder.snapshot();
            if samples.len() < LIVE_TRANSCRIPTION_MIN_SAMPLES {
                continue;
            }

            if samples.len().saturating_sub(last_transcribed_samples) < LIVE_TRANSCRIPTION_MIN_DELTA
            {
                continue;
            }

            if !samples_contain_speech(recent_speech_window(&samples)) {
                if let Ok(mut transcript) = live_transcript.lock() {
                    transcript.clear();
                }
                last_text.clear();
                continue;
            }

            let padded_samples = padded_live_samples(&samples);

            match transcriber.try_transcribe(&padded_samples) {
                Ok(Some(text)) => {
                    last_transcribed_samples = samples.len();
                    let display_text = format_live_transcript(&text);

                    if display_text.is_empty() || display_text == last_text {
                        continue;
                    }

                    if !is_recording.load(Ordering::Relaxed)
                        || recording_session.load(Ordering::Relaxed) != session
                    {
                        break;
                    }

                    if let Ok(mut transcript) = live_transcript.lock() {
                        transcript.clear();
                        transcript.push_str(&display_text);
                    }
                    eprintln!("[screamer] Live partial: {}", display_text);
                    last_text = display_text;
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("[screamer] Live transcription error: {}", err);
                }
            }
        }
    });
}

fn padded_live_samples(samples: &[f32]) -> Vec<f32> {
    let samples = live_preview_window(samples);
    let mut padded = Vec::with_capacity(samples.len() + LIVE_TRANSCRIPTION_PADDING_SAMPLES);
    padded.extend_from_slice(samples);
    padded.resize(samples.len() + LIVE_TRANSCRIPTION_PADDING_SAMPLES, 0.0);
    padded
}

fn live_preview_window(samples: &[f32]) -> &[f32] {
    let start = samples.len().saturating_sub(LIVE_TRANSCRIPTION_MAX_SAMPLES);
    &samples[start..]
}

fn recent_speech_window(samples: &[f32]) -> &[f32] {
    let start = samples
        .len()
        .saturating_sub(SPEECH_DETECTION_LOOKBACK_SAMPLES);
    &samples[start..]
}

fn samples_contain_speech(samples: &[f32]) -> bool {
    let mut active_frames = 0usize;

    for frame in samples.chunks(SPEECH_DETECTION_FRAME_SAMPLES) {
        if frame_rms(frame) >= SPEECH_DETECTION_FRAME_RMS_GATE {
            active_frames += 1;
            if active_frames >= SPEECH_DETECTION_MIN_ACTIVE_FRAMES {
                return true;
            }
        }
    }

    false
}

fn frame_rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }

    (frame.iter().map(|sample| sample * sample).sum::<f32>() / frame.len() as f32).sqrt()
}

fn format_live_transcript(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let total_chars = trimmed.chars().count();
    if total_chars <= LIVE_TRANSCRIPT_MAX_CHARS {
        return trimmed.to_string();
    }

    let tail_start = trimmed
        .char_indices()
        .nth(total_chars - LIVE_TRANSCRIPT_MAX_CHARS)
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let tail = trimmed[tail_start..].trim_start();
    format!("...{}", tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_preview_window_caps_to_recent_audio() {
        let samples: Vec<f32> = (0..(LIVE_TRANSCRIPTION_MAX_SAMPLES + 10))
            .map(|v| v as f32)
            .collect();
        let window = live_preview_window(&samples);

        assert_eq!(window.len(), LIVE_TRANSCRIPTION_MAX_SAMPLES);
        assert_eq!(window[0], 10.0);
        assert_eq!(window[window.len() - 1], samples[samples.len() - 1]);
    }

    #[test]
    fn live_transcript_formatting_keeps_recent_suffix() {
        let input = "alpha ".repeat(64);
        let formatted = format_live_transcript(&input);

        assert!(formatted.starts_with("..."));
        assert!(formatted.ends_with("alpha"));
        assert!(formatted.chars().count() <= LIVE_TRANSCRIPT_MAX_CHARS + 3);
    }

    #[test]
    fn silence_is_not_detected_as_speech() {
        let samples = vec![0.0; SPEECH_DETECTION_LOOKBACK_SAMPLES];
        assert!(!samples_contain_speech(&samples));
    }

    #[test]
    fn low_noise_is_not_detected_as_speech() {
        let samples = vec![0.002; SPEECH_DETECTION_LOOKBACK_SAMPLES];
        assert!(!samples_contain_speech(&samples));
    }

    #[test]
    fn sustained_voice_is_detected_as_speech() {
        let samples =
            vec![0.02; SPEECH_DETECTION_FRAME_SAMPLES * SPEECH_DETECTION_MIN_ACTIVE_FRAMES];
        assert!(samples_contain_speech(&samples));
    }
}

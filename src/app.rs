use crate::config::{AppAppearance, Config, HOTKEYS, MODELS, POSITIONS};
use crate::logging;
use crate::overlay::{Overlay, WAVEFORM_BINS};
use crate::permission_window::PermissionWindow;
use crate::permissions;
use crate::recorder::Recorder;
use crate::settings_window::SettingsWindow;
use crate::sound::SoundPlayer;
use crate::theme;
use crate::transcriber::Transcriber;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::sel;
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSApplication, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
};
use objc2_foundation::{MainThreadMarker, NSString};
use std::cell::Cell;
use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::ops::Range;
use std::path::{Path, PathBuf};
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
const SPEECH_DETECTION_LOOKBACK_SAMPLES: usize = 16_000;
const SPEECH_DETECTION_FRAME_SAMPLES: usize = 320;
const SPEECH_DETECTION_FRAME_RMS_GATE: f32 = 0.006;
const SPEECH_DETECTION_MIN_ACTIVE_FRAMES: usize = 3;
const SPEECH_TRIM_PADDING_SAMPLES: usize = 1600;
const FINAL_TRANSCRIPTION_MIN_SAMPLES: usize = 1600;
const SHORT_UTTERANCE_FINAL_MIN_SAMPLES: usize =
    SPEECH_DETECTION_FRAME_SAMPLES * SHORT_UTTERANCE_MIN_ACTIVE_FRAMES;
const SHORT_UTTERANCE_MAX_SAMPLES: usize = 12_800;
const SHORT_UTTERANCE_FRAME_RMS_GATE: f32 = 0.004;
const SHORT_UTTERANCE_MIN_ACTIVE_FRAMES: usize = 2;
const SHORT_UTTERANCE_MIN_PEAK: f32 = 0.02;
const RELAUNCH_DELAY_SECONDS: &str = "0.5";
const RELAUNCH_SHELL: &str = "/bin/sh";
const OPEN_COMMAND: &str = "/usr/bin/open";
const ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
const MICROPHONE_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";

// Thread-local overlay reference so menu handlers can update position without relaunch.
// Safe because all menu handlers and overlay access run on the main thread.
thread_local! {
    static OVERLAY: RefCell<Option<Rc<RefCell<Overlay>>>> = const { RefCell::new(None) };
    static HOTKEY_MONITOR: RefCell<Option<Rc<crate::hotkey::Hotkey>>> = const { RefCell::new(None) };
    static LIVE_TRANSCRIPTION_ENABLED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
    static SOUND_EFFECTS_ENABLED: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
    static SETTINGS_WINDOW: RefCell<Option<Rc<SettingsWindow>>> = const { RefCell::new(None) };
    static ACCESSIBILITY_WINDOW: RefCell<Option<Rc<PermissionWindow>>> = const { RefCell::new(None) };
    static ACCESSIBILITY_GRANTED: Cell<bool> = const { Cell::new(false) };
    static ACCESSIBILITY_HELPER_DISMISSED: Cell<bool> = const { Cell::new(false) };
    static STATUS_ITEM: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
}

static MICROPHONE_PERMISSION_GUIDANCE_SHOWN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct SpeechDetectionConfig {
    frame_rms_gate: f32,
    min_active_frames: usize,
}

#[derive(Clone, Copy)]
enum FinalSpeechWindowKind {
    Standard,
    ShortUtterance,
}

struct FinalSpeechWindow {
    range: Range<usize>,
    kind: FinalSpeechWindowKind,
}

pub struct App {
    _status_item: Retained<NSStatusItem>,
    overlay: Rc<RefCell<Overlay>>,
    hotkey: Rc<crate::hotkey::Hotkey>,
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

pub fn install_main_menu(mtm: MainThreadMarker, app: &NSApplication) {
    let handler = get_menu_handler();

    unsafe {
        let main_menu = NSMenu::new(mtm);
        let app_item = NSMenuItem::new(mtm);
        app_item.setTitle(&NSString::from_str("Screamer"));

        let app_menu = NSMenu::new(mtm);
        app_menu.setTitle(&NSString::from_str("Screamer"));

        let settings_item = NSMenuItem::new(mtm);
        settings_item.setTitle(&NSString::from_str("Settings..."));
        let _: () = msg_send![&*settings_item, setTarget: handler];
        settings_item.setAction(Some(sel!(showSettings:)));
        settings_item.setKeyEquivalent(&NSString::from_str(","));
        app_menu.addItem(&settings_item);

        app_menu.addItem(&NSMenuItem::separatorItem(mtm));

        let quit_item = NSMenuItem::new(mtm);
        quit_item.setTitle(&NSString::from_str("Quit Screamer"));
        quit_item.setAction(Some(sel!(terminate:)));
        quit_item.setKeyEquivalent(&NSString::from_str("q"));
        app_menu.addItem(&quit_item);

        app_item.setSubmenu(Some(&app_menu));
        main_menu.addItem(&app_item);
        app.setMainMenu(Some(&main_menu));

        let _: () = msg_send![app, setDelegate: handler];
    }
}

pub fn show_settings_window() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    let app = NSApplication::sharedApplication(mtm);
    app.activate();
    SETTINGS_WINDOW.with(|cell| {
        if let Some(window) = cell.borrow().as_ref() {
            window.show();
        }
    });
}

pub fn show_accessibility_window() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    set_accessibility_helper_dismissed(false);

    let app = NSApplication::sharedApplication(mtm);
    app.activate();
    ACCESSIBILITY_WINDOW.with(|cell| {
        if let Some(window) = cell.borrow().as_ref() {
            window.show();
        }
    });
}

fn set_accessibility_helper_dismissed(dismissed: bool) {
    ACCESSIBILITY_HELPER_DISMISSED.with(|cell| {
        cell.set(dismissed);
    });

    let mut config = Config::load();
    if config.accessibility_helper_dismissed != dismissed {
        config.accessibility_helper_dismissed = dismissed;
        config.save();
    }
}

pub fn sync_accessibility_window() {
    let granted = permissions::has_accessibility_permission();
    let changed = ACCESSIBILITY_GRANTED.with(|cell| {
        let previous = cell.get();
        cell.set(granted);
        previous != granted
    });

    ACCESSIBILITY_WINDOW.with(|cell| {
        if let Some(window) = cell.borrow().as_ref() {
            if granted {
                window.hide();
            }
        }
    });

    if changed {
        if let Some(mtm) = MainThreadMarker::new() {
            rebuild_status_menu(mtm);
        }
    }
}

fn open_accessibility_settings() {
    set_accessibility_helper_dismissed(false);
    if let Err(err) = std::process::Command::new(OPEN_COMMAND)
        .arg(ACCESSIBILITY_SETTINGS_URL)
        .spawn()
    {
        eprintln!("[screamer] Failed to open Accessibility settings: {err}");
    }
}

fn open_microphone_settings() {
    if let Err(err) = std::process::Command::new(OPEN_COMMAND)
        .arg(MICROPHONE_SETTINGS_URL)
        .spawn()
    {
        eprintln!("[screamer] Failed to open Microphone settings: {err}");
    }
}

fn show_missing_microphone_permission_guidance() {
    let should_show = MICROPHONE_PERMISSION_GUIDANCE_SHOWN
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();

    if !should_show {
        return;
    }

    show_settings_window();

    if let Some(mtm) = MainThreadMarker::new() {
        App::show_alert(
            mtm,
            "Microphone Permission Required",
            "Screamer can't record until microphone access is enabled. Open Screamer Settings and click Microphone to jump to System Settings, then try recording again.",
        );
    }
}

fn dismiss_accessibility_helper() {
    set_accessibility_helper_dismissed(true);
    ACCESSIBILITY_WINDOW.with(|cell| {
        if let Some(window) = cell.borrow().as_ref() {
            window.hide();
        }
    });
    show_settings_window();
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
            builder.add_method(
                sel!(showSettings:),
                show_settings_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(openAccessibilitySettings:),
                open_accessibility_settings_action
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(openMicrophoneSettings:),
                open_microphone_settings_action
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(dismissAccessibilityHelper:),
                dismiss_accessibility_helper_action
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(selectModelPopup:),
                select_model_popup_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(selectHotkeyPopup:),
                select_hotkey_popup_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(selectPositionPopup:),
                select_position_popup_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(setLiveTranscriptionEnabled:),
                set_live_transcription_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(setAppearanceMode:),
                set_appearance_mode_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(setSoundEffectsEnabled:),
                set_sound_effects_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(applicationShouldHandleReopen:hasVisibleWindows:),
                application_should_handle_reopen
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, Bool) -> Bool,
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
    apply_model_selection(tag as usize);
}

extern "C" fn select_hotkey_action(_this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject) {
    let tag: isize = unsafe { msg_send![sender, tag] };
    apply_hotkey_selection(tag as usize);
}

extern "C" fn select_position_action(_this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject) {
    let tag: isize = unsafe { msg_send![sender, tag] };
    apply_position_selection(tag as usize);
}

extern "C" fn toggle_live_transcription_action(
    _this: *mut AnyObject,
    _sel: Sel,
    _sender: *mut AnyObject,
) {
    let enabled = LIVE_TRANSCRIPTION_ENABLED.with(|cell| {
        let borrowed = cell.borrow();
        let flag = borrowed.as_ref()?;
        Some(!flag.load(Ordering::Relaxed))
    });
    let Some(enabled) = enabled else {
        return;
    };
    set_live_transcription_enabled(enabled);
}

extern "C" fn toggle_sound_effects_action(
    _this: *mut AnyObject,
    _sel: Sel,
    _sender: *mut AnyObject,
) {
    let enabled = SOUND_EFFECTS_ENABLED.with(|cell| {
        let borrowed = cell.borrow();
        let flag = borrowed.as_ref()?;
        Some(!flag.load(Ordering::Relaxed))
    });
    let Some(enabled) = enabled else {
        return;
    };
    set_sound_effects_enabled(enabled);
}

extern "C" fn show_settings_action(_this: *mut AnyObject, _sel: Sel, _sender: *mut AnyObject) {
    show_settings_window();
}

extern "C" fn open_accessibility_settings_action(
    _this: *mut AnyObject,
    _sel: Sel,
    _sender: *mut AnyObject,
) {
    open_accessibility_settings();
}

extern "C" fn open_microphone_settings_action(
    _this: *mut AnyObject,
    _sel: Sel,
    _sender: *mut AnyObject,
) {
    open_microphone_settings();
}

extern "C" fn dismiss_accessibility_helper_action(
    _this: *mut AnyObject,
    _sel: Sel,
    _sender: *mut AnyObject,
) {
    dismiss_accessibility_helper();
}

extern "C" fn select_model_popup_action(_this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject) {
    let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
    apply_model_selection(index as usize);
}

extern "C" fn select_hotkey_popup_action(_this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject) {
    let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
    apply_hotkey_selection(index as usize);
}

extern "C" fn select_position_popup_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
    apply_position_selection(index as usize);
}

extern "C" fn set_live_transcription_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let state: isize = unsafe { msg_send![sender, state] };
    set_live_transcription_enabled(state != 0);
}

extern "C" fn set_appearance_mode_action(_this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject) {
    let selected: isize = unsafe { msg_send![sender, selectedSegment] };
    let appearance = if selected == 1 {
        AppAppearance::Light
    } else {
        AppAppearance::Dark
    };
    set_app_appearance(appearance);
}

extern "C" fn set_sound_effects_action(_this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject) {
    let state: isize = unsafe { msg_send![sender, state] };
    set_sound_effects_enabled(state != 0);
}

extern "C" fn application_should_handle_reopen(
    _this: *mut AnyObject,
    _sel: Sel,
    _sender: *mut AnyObject,
    _has_visible_windows: Bool,
) -> Bool {
    show_settings_window();
    Bool::YES
}

fn relaunch() {
    eprintln!("[screamer] Relaunching...");
    match std::env::current_exe() {
        Ok(exe) => {
            let launch_result = if let Some(app_path) = bundled_app_path(&exe) {
                eprintln!("[screamer] Relaunching app bundle: {}", app_path.display());
                spawn_delayed_command(
                    OsStr::new(OPEN_COMMAND),
                    &[OsString::from("-a"), app_path.into_os_string()],
                )
            } else {
                eprintln!("[screamer] Relaunching binary: {}", exe.display());
                spawn_delayed_command(exe.as_os_str(), &[])
            };

            if let Err(err) = launch_result {
                eprintln!("[screamer] Failed to schedule relaunch: {err}");
            }
        }
        Err(err) => {
            eprintln!("[screamer] Failed to resolve current executable for relaunch: {err}");
        }
    }

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        app.terminate(None);
    }
}

fn apply_model_selection(index: usize) {
    let Some(model_info) = MODELS.get(index) else {
        sync_settings_window(&Config::load());
        return;
    };

    let mut config = Config::load();
    if config.model == model_info.id {
        sync_settings_window(&config);
        return;
    }

    eprintln!("[screamer] Model selected: {}", model_info.id);
    if Transcriber::find_model(model_info.id).is_none() {
        eprintln!("[screamer] Model not found: {}", model_info.id);
        show_missing_model_alert(model_info.label, model_info.size, model_info.id);
        sync_settings_window(&config);
        return;
    }

    config.model = model_info.id.to_string();
    config.save();
    relaunch();
}

fn apply_hotkey_selection(index: usize) {
    let Some(hotkey_info) = HOTKEYS.get(index) else {
        sync_settings_window(&Config::load());
        return;
    };

    let mut config = Config::load();
    if config.hotkey == hotkey_info.id {
        sync_settings_window(&config);
        return;
    }

    eprintln!("[screamer] Hotkey selected: {}", hotkey_info.id);
    config.hotkey = hotkey_info.id.to_string();
    config.save();

    HOTKEY_MONITOR.with(|cell| {
        if let Some(hotkey) = cell.borrow().as_ref() {
            hotkey.set_hotkey(hotkey_info.id);
        }
    });

    if let Some(mtm) = MainThreadMarker::new() {
        rebuild_status_menu(mtm);
    }
    sync_settings_window(&config);
}

fn apply_position_selection(index: usize) {
    let Some(pos_info) = POSITIONS.get(index) else {
        sync_settings_window(&Config::load());
        return;
    };

    eprintln!("[screamer] Position selected: {}", pos_info.label);
    let mut config = Config::load();
    config.overlay_position = pos_info.id;
    config.save();

    if let Some(mtm) = MainThreadMarker::new() {
        OVERLAY.with(|cell| {
            if let Some(overlay) = cell.borrow().as_ref() {
                if let Ok(mut ov) = overlay.try_borrow_mut() {
                    ov.set_position(mtm, pos_info.id);
                }
            }
        });
        rebuild_status_menu(mtm);
    }
    sync_settings_window(&config);
}

fn set_live_transcription_enabled(enabled: bool) {
    LIVE_TRANSCRIPTION_ENABLED.with(|cell| {
        if let Some(flag) = cell.borrow().as_ref() {
            flag.store(enabled, Ordering::Relaxed);
        }
    });

    let mut config = Config::load();
    config.live_transcription = enabled;
    config.save();

    if let Some(mtm) = MainThreadMarker::new() {
        rebuild_status_menu(mtm);
    }
    sync_settings_window(&config);
    eprintln!(
        "[screamer] Live transcription {}",
        if enabled { "enabled" } else { "disabled" }
    );
}

fn set_app_appearance(appearance: AppAppearance) {
    let mut config = Config::load();
    if config.appearance == appearance {
        sync_settings_window(&config);
        return;
    }

    config.appearance = appearance;
    config.save();

    if let Some(mtm) = MainThreadMarker::new() {
        theme::apply_app_appearance(mtm, appearance);
        OVERLAY.with(|cell| {
            if let Some(overlay) = cell.borrow().as_ref() {
                if let Ok(mut ov) = overlay.try_borrow_mut() {
                    ov.set_appearance(appearance);
                }
            }
        });
        rebuild_status_menu(mtm);
    }

    sync_settings_window(&config);
    eprintln!("[screamer] Appearance set to {}", config.appearance_label());
}

fn set_sound_effects_enabled(enabled: bool) {
    SOUND_EFFECTS_ENABLED.with(|cell| {
        if let Some(flag) = cell.borrow().as_ref() {
            flag.store(enabled, Ordering::Relaxed);
        }
    });

    let mut config = Config::load();
    config.sound_effects = enabled;
    config.save();

    if let Some(mtm) = MainThreadMarker::new() {
        rebuild_status_menu(mtm);
    }
    sync_settings_window(&config);
    eprintln!(
        "[screamer] Sound effects {}",
        if enabled { "enabled" } else { "disabled" }
    );
}

fn sync_settings_window(config: &Config) {
    SETTINGS_WINDOW.with(|cell| {
        if let Some(window) = cell.borrow().as_ref() {
            window.sync(config);
        }
    });
}

fn rebuild_status_menu(mtm: MainThreadMarker) {
    let config = Config::load();
    STATUS_ITEM.with(|cell| {
        if let Some(status_item) = cell.borrow().as_ref() {
            let menu = App::build_menu(mtm, &config);
            status_item.setMenu(Some(&menu));
        }
    });
}

fn show_missing_model_alert(label: &str, size: &str, id: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&NSString::from_str("Model Not Downloaded"));
    alert.setInformativeText(&NSString::from_str(&format!(
        "The {} model ({}) hasn't been downloaded yet.\n\n\
         Run this in Terminal:\n\
         cd {} && ./download_model.sh {}",
        label,
        size,
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        id
    )));
    alert.runModal();
}

fn menu_model_title(id: &str) -> String {
    if let Some(model) = MODELS.iter().find(|model| model.id == id) {
        model.label.to_string()
    } else {
        id.to_string()
    }
}

fn selectable_model_title(id: &str) -> String {
    let Some(model) = MODELS.iter().find(|model| model.id == id) else {
        return id.to_string();
    };

    if Transcriber::find_model(model.id).is_some() {
        format!("{} ({})", model.label, model.size)
    } else {
        format!("{} ({}) — Not Downloaded", model.label, model.size)
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
        match transcriber.warm_up(config.live_transcription) {
            Ok(duration) => {
                log::info!(
                    "Transcriber warmup completed in {}ms (live_preview={})",
                    duration.as_millis(),
                    if config.live_transcription {
                        "yes"
                    } else {
                        "no"
                    }
                );
            }
            Err(err) => {
                eprintln!("[screamer] Warmup failed, continuing without it: {err}");
            }
        }
        log::info!("Model loaded successfully");

        Ok(transcriber)
    }

    pub fn new_with_transcriber(
        mtm: MainThreadMarker,
        config: Config,
        transcriber: Arc<Transcriber>,
    ) -> Result<Self, AppInitError> {
        let recorder = Arc::new(Recorder::new());
        theme::apply_app_appearance(mtm, config.appearance);
        let overlay = Rc::new(RefCell::new(Overlay::new(
            mtm,
            config.overlay_position,
            config.appearance,
        )));
        let hotkey = Rc::new(crate::hotkey::Hotkey::new(&config));
        let sound_player = Rc::new(SoundPlayer::new(mtm));

        // Store overlay reference for position menu handler
        OVERLAY.with(|cell| {
            *cell.borrow_mut() = Some(overlay.clone());
        });
        HOTKEY_MONITOR.with(|cell| {
            *cell.borrow_mut() = Some(hotkey.clone());
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

        STATUS_ITEM.with(|cell| {
            *cell.borrow_mut() = Some(status_item.clone());
        });

        let settings_window = SettingsWindow::new(mtm, &config, get_menu_handler());
        SETTINGS_WINDOW.with(|cell| {
            *cell.borrow_mut() = Some(settings_window);
        });

        let accessibility_window = PermissionWindow::new(mtm, get_menu_handler());
        ACCESSIBILITY_WINDOW.with(|cell| {
            *cell.borrow_mut() = Some(accessibility_window);
        });
        ACCESSIBILITY_GRANTED.with(|cell| {
            cell.set(permissions::has_accessibility_permission());
        });
        ACCESSIBILITY_HELPER_DISMISSED.with(|cell| {
            cell.set(config.accessibility_helper_dismissed);
        });

        let menu = Self::build_menu(mtm, &config);
        status_item.setMenu(Some(&menu));

        Ok(Self {
            _status_item: status_item,
            overlay,
            hotkey,
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
            let accessibility_granted = permissions::has_accessibility_permission();

            let status_line = NSMenuItem::new(mtm);
            let status_title = if accessibility_granted {
                "Screamer — Ready"
            } else {
                "Screamer — Accessibility Required"
            };
            status_line.setTitle(&NSString::from_str(status_title));
            status_line.setEnabled(false);
            menu.addItem(&status_line);

            let open_item = NSMenuItem::new(mtm);
            open_item.setTitle(&NSString::from_str("Open Screamer"));
            let _: () = msg_send![&*open_item, setTarget: handler];
            open_item.setAction(Some(sel!(showSettings:)));
            menu.addItem(&open_item);

            if !accessibility_granted {
                let access_item = NSMenuItem::new(mtm);
                access_item.setTitle(&NSString::from_str("Enable Accessibility Access"));
                let _: () = msg_send![&*access_item, setTarget: handler];
                access_item.setAction(Some(sel!(openAccessibilitySettings:)));
                menu.addItem(&access_item);
            }

            menu.addItem(&NSMenuItem::separatorItem(mtm));

            // ── Model submenu ──
            let model_item = NSMenuItem::new(mtm);
            model_item.setTitle(&NSString::from_str(&format!(
                "Model: {}",
                menu_model_title(&config.model)
            )));
            let model_submenu = NSMenu::new(mtm);
            model_submenu.setTitle(&NSString::from_str("Model"));

            for (i, model_info) in MODELS.iter().enumerate() {
                let item = NSMenuItem::new(mtm);
                let title = selectable_model_title(model_info.id);
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

        let hotkey = self.hotkey.clone();

        hotkey.start_on_main_thread(
            mtm,
            move || {
                if is_rec_press
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    match permissions::prepare_microphone_permission() {
                        permissions::MicrophonePermissionOutcome::Granted => {}
                        permissions::MicrophonePermissionOutcome::Prompted => {
                            eprintln!(
                                "[screamer] Requested microphone permission; waiting for the user to respond"
                            );
                            is_rec_press.store(false, Ordering::SeqCst);
                            return;
                        }
                        permissions::MicrophonePermissionOutcome::Denied => {
                            eprintln!(
                                "[screamer] Microphone permission missing; blocking recording before capture starts"
                            );
                            is_rec_press.store(false, Ordering::SeqCst);
                            show_missing_microphone_permission_guidance();
                            return;
                        }
                    }

                    let session = recording_session_press.fetch_add(1, Ordering::SeqCst) + 1;
                    if let Ok(mut transcript) = live_transcript_press.lock() {
                        transcript.clear();
                    }
                    rec_press.reset_buffers();

                    if sound_effects_enabled_press.load(Ordering::Relaxed) {
                        sound_player_press.play_recording_start();
                    }
                    start_recording_capture(
                        rec_press.clone(),
                        trans_press.clone(),
                        is_rec_press.clone(),
                        live_transcription_enabled_press.clone(),
                        live_transcript_press.clone(),
                        recording_session_press.clone(),
                        session,
                    );
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

                    let Some(transcribe_window) = final_transcription_window(&samples) else {
                        eprintln!("[screamer] Recording was silence, skipping transcription");
                        if sound_effects_enabled_release.load(Ordering::Relaxed) {
                            pending_completion_sound_release.store(true, Ordering::SeqCst);
                        }
                        return;
                    };

                    let trimmed_len = transcribe_window.range.end - transcribe_window.range.start;
                    let min_required =
                        minimum_final_transcription_samples(transcribe_window.kind);
                    if trimmed_len < min_required {
                        eprintln!(
                            "[screamer] Recording too short after trimming silence ({} samples), skipping",
                            trimmed_len
                        );
                        if sound_effects_enabled_release.load(Ordering::Relaxed) {
                            pending_completion_sound_release.store(true, Ordering::SeqCst);
                        }
                        return;
                    }

                    if trimmed_len != samples.len() {
                        eprintln!(
                            "[screamer] Trimmed silence: {} -> {} samples",
                            samples.len(),
                            trimmed_len
                        );
                    }
                    if matches!(transcribe_window.kind, FinalSpeechWindowKind::ShortUtterance) {
                        eprintln!(
                            "[screamer] Salvaging brief utterance with relaxed speech gate ({} samples)",
                            trimmed_len
                        );
                    }

                    let t = trans_release.clone();
                    let pending_completion_sound = pending_completion_sound_release.clone();
                    let sound_effects_enabled = sound_effects_enabled_release.clone();
                    let stop_ms = release_t0.elapsed().as_millis();
                    std::thread::spawn(move || {
                        match t.transcribe_profiled(&samples[transcribe_window.range]) {
                            Ok(result) if !result.text.is_empty() => {
                                eprintln!(
                                    "[screamer] Transcribed in {}ms ({} chars)",
                                    result.profile.total.as_millis(),
                                    result.text.chars().count()
                                );
                                logging::log_transcript("Final transcript", &result.text);

                                if !permissions::has_accessibility_permission() {
                                    eprintln!(
                                        "[screamer] Accessibility permission missing; automatic paste may fail for this build"
                                    );
                                }

                                let paste_t0 = std::time::Instant::now();
                                let paste_result = crate::paster::paste(&result.text);
                                let paste_ms = paste_t0.elapsed().as_millis();

                                match paste_result {
                                    Ok(()) => {
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
                                    Err(err) => {
                                        eprintln!(
                                            "[screamer] Paste failed after {}ms: {}",
                                            paste_ms, err
                                        );
                                    }
                                }
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
                sync_accessibility_window();

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

    if !permissions::has_microphone_permission() {
        eprintln!(
            "[screamer] Microphone permission missing during capture start; skipping audio capture"
        );
        is_recording.store(false, Ordering::SeqCst);
        return;
    }

    if let Err(err) = recorder.start() {
        eprintln!("[screamer] Failed to start audio capture: {err}");
        is_recording.store(false, Ordering::SeqCst);
        if let Some(mtm) = MainThreadMarker::new() {
            App::show_alert(mtm, "Microphone Permission Required", &err);
        }
        return;
    }

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
                    logging::log_transcript("Live partial", &display_text);
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
    speech_activity_bounds(samples).is_some()
}

fn final_transcription_window(samples: &[f32]) -> Option<FinalSpeechWindow> {
    if let Some((start, end)) = speech_activity_bounds(samples) {
        let range = padded_speech_range(samples.len(), start, end);
        if range.end.saturating_sub(range.start)
            >= minimum_final_transcription_samples(FinalSpeechWindowKind::Standard)
        {
            return Some(FinalSpeechWindow {
                range,
                kind: FinalSpeechWindowKind::Standard,
            });
        }
    }

    if samples.len() > SHORT_UTTERANCE_MAX_SAMPLES {
        return None;
    }

    let short_config = SpeechDetectionConfig {
        frame_rms_gate: SHORT_UTTERANCE_FRAME_RMS_GATE,
        min_active_frames: SHORT_UTTERANCE_MIN_ACTIVE_FRAMES,
    };
    let (start, end) = speech_activity_bounds_with_config(samples, short_config)?;
    let range = padded_speech_range(samples.len(), start, end);
    if range.end.saturating_sub(range.start)
        < minimum_final_transcription_samples(FinalSpeechWindowKind::ShortUtterance)
    {
        return None;
    }
    if max_abs_sample(&samples[range.clone()]) < SHORT_UTTERANCE_MIN_PEAK {
        return None;
    }

    Some(FinalSpeechWindow {
        range,
        kind: FinalSpeechWindowKind::ShortUtterance,
    })
}

fn speech_activity_bounds(samples: &[f32]) -> Option<(usize, usize)> {
    speech_activity_bounds_with_config(
        samples,
        SpeechDetectionConfig {
            frame_rms_gate: SPEECH_DETECTION_FRAME_RMS_GATE,
            min_active_frames: SPEECH_DETECTION_MIN_ACTIVE_FRAMES,
        },
    )
}

fn speech_activity_bounds_with_config(
    samples: &[f32],
    config: SpeechDetectionConfig,
) -> Option<(usize, usize)> {
    let mut first_active = None;
    let mut last_active_end = 0usize;
    let mut active_frames = 0usize;

    for (frame_idx, frame) in samples.chunks(SPEECH_DETECTION_FRAME_SAMPLES).enumerate() {
        if frame_rms(frame) < config.frame_rms_gate {
            continue;
        }

        active_frames += 1;
        let frame_start = frame_idx * SPEECH_DETECTION_FRAME_SAMPLES;
        first_active.get_or_insert(frame_start);
        last_active_end = frame_start + frame.len();
    }

    if active_frames < config.min_active_frames {
        return None;
    }

    Some((first_active.unwrap_or(0), last_active_end))
}

fn padded_speech_range(total_len: usize, start: usize, end: usize) -> Range<usize> {
    start.saturating_sub(SPEECH_TRIM_PADDING_SAMPLES)
        ..(end + SPEECH_TRIM_PADDING_SAMPLES).min(total_len)
}

fn minimum_final_transcription_samples(kind: FinalSpeechWindowKind) -> usize {
    match kind {
        FinalSpeechWindowKind::Standard => FINAL_TRANSCRIPTION_MIN_SAMPLES,
        FinalSpeechWindowKind::ShortUtterance => SHORT_UTTERANCE_FINAL_MIN_SAMPLES,
    }
}

fn max_abs_sample(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
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

fn bundled_app_path(exe: &Path) -> Option<PathBuf> {
    let app_path = exe.parent()?.parent()?.parent()?.to_path_buf();
    if app_path.extension().and_then(|ext| ext.to_str()) == Some("app") {
        Some(app_path)
    } else {
        None
    }
}

fn spawn_delayed_command(program: &OsStr, args: &[OsString]) -> Result<(), String> {
    let mut command = std::process::Command::new(RELAUNCH_SHELL);
    command
        .arg("-c")
        .arg("sleep \"$1\"; shift; exec \"$@\"")
        .arg("screamer-relaunch")
        .arg(RELAUNCH_DELAY_SECONDS)
        .arg(program);

    for arg in args {
        command.arg(arg);
    }

    command
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("unable to launch helper process: {err}"))
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

    #[test]
    fn trimmed_speech_range_drops_outer_silence_with_padding() {
        let mut samples = vec![0.0; SPEECH_DETECTION_FRAME_SAMPLES * 10];
        samples.extend(vec![0.02; SPEECH_DETECTION_FRAME_SAMPLES * 6]);
        samples.extend(vec![0.0; SPEECH_DETECTION_FRAME_SAMPLES * 8]);

        let range = final_transcription_window(&samples)
            .expect("speech should be detected")
            .range;

        assert_eq!(range.start, 1600);
        assert_eq!(range.end, 6720);
    }

    #[test]
    fn final_transcription_window_salvages_brief_phrase() {
        let samples = vec![0.02; SPEECH_DETECTION_FRAME_SAMPLES * 2];

        let window = final_transcription_window(&samples).expect("brief speech should survive");
        assert_eq!(window.range.start, 0);
        assert_eq!(window.range.end, samples.len());
        assert!(matches!(window.kind, FinalSpeechWindowKind::ShortUtterance));
    }

    #[test]
    fn final_transcription_window_ignores_single_short_spike() {
        let samples = vec![0.03; SPEECH_DETECTION_FRAME_SAMPLES];

        assert!(final_transcription_window(&samples).is_none());
    }
}

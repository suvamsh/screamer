use crate::ambient_controller::AmbientController;
use crate::config::{
    AppAppearance, Config, SummaryBackendPreference, HOTKEYS, MODELS, POSITIONS, VISION_PROVIDERS,
};
use crate::diarization::default_diarization_engine;
use crate::highlight::HighlightOverlay;
use crate::logging;
use crate::main_window::MainWindow;
use crate::overlay::{Overlay, WAVEFORM_BINS};
use crate::permission_window::PermissionWindow;
use crate::permissions;
use crate::recorder::Recorder;
use crate::session_store::SessionStore;
use crate::sound::SoundPlayer;
use crate::summary_backend::SummaryBackendRegistry;
use crate::theme;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::sel;
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSApplication, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
};
use objc2_foundation::{MainThreadMarker, NSString};
use screamer_core::session::{
    prepare_final_transcription, FinalSpeechWindowKind, FinalTranscriptionAction,
    LivePreviewAction, LivePreviewState, LIVE_TRANSCRIPTION_INTERVAL,
};
use screamer_whisper::Transcriber;
use std::cell::Cell;
use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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
    static MAIN_WINDOW: RefCell<Option<Rc<MainWindow>>> = const { RefCell::new(None) };
    static ACCESSIBILITY_WINDOW: RefCell<Option<Rc<PermissionWindow>>> = const { RefCell::new(None) };
    static ACCESSIBILITY_GRANTED: Cell<bool> = const { Cell::new(false) };
    static ACCESSIBILITY_HELPER_DISMISSED: Cell<bool> = const { Cell::new(false) };
    static STATUS_ITEM: RefCell<Option<Retained<NSStatusItem>>> = const { RefCell::new(None) };
}

static MICROPHONE_PERMISSION_GUIDANCE_SHOWN: AtomicBool = AtomicBool::new(false);
static AMBIENT_CONTROLLER: OnceLock<Arc<AmbientController>> = OnceLock::new();

pub struct App {
    _status_item: Retained<NSStatusItem>,
    overlay: Rc<RefCell<Overlay>>,
    main_window: Rc<MainWindow>,
    hotkey: Rc<crate::hotkey::Hotkey>,
    recorder: Arc<Recorder>,
    sound_player: Rc<SoundPlayer>,
    transcriber: Arc<Transcriber>,
    ambient_controller: Arc<AmbientController>,
    _session_store: Arc<SessionStore>,
    _summary_registry: Arc<SummaryBackendRegistry>,
    is_recording: Arc<AtomicBool>,
    live_transcription_enabled: Arc<AtomicBool>,
    sound_effects_enabled: Arc<AtomicBool>,
    live_transcript: Arc<Mutex<String>>,
    pending_completion_sound: Arc<AtomicBool>,
    recording_session: Arc<AtomicU64>,
    /// Bumped when vision enters Loading or Response so stale TTS callbacks cannot clear a newer flow.
    vision_ui_epoch: Arc<AtomicU64>,
    vision_overlay_state: Arc<Mutex<crate::overlay::VisionOverlayState>>,
    highlight_overlay: Rc<RefCell<HighlightOverlay>>,
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
    MAIN_WINDOW.with(|cell| {
        if let Some(window) = cell.borrow().as_ref() {
            window.show_settings();
        }
    });
}

pub fn show_main_window() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    let app = NSApplication::sharedApplication(mtm);
    app.activate();
    MAIN_WINDOW.with(|cell| {
        if let Some(window) = cell.borrow().as_ref() {
            window.show_home();
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
                sel!(showHomePage:),
                show_home_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(showSettingsPage:),
                show_settings_page_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
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
                sel!(selectVisionBackendPopup:),
                select_vision_backend_popup_action
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(selectVisionHotkeyPopup:),
                select_vision_hotkey_popup_action
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
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
                sel!(setAmbientMicrophoneEnabled:),
                set_ambient_microphone_enabled_action
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(setAmbientSystemAudioEnabled:),
                set_ambient_system_audio_enabled_action
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(selectSummaryModelPopup:),
                select_summary_model_popup_action
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(selectAmbientFinalBackendPopup:),
                select_ambient_final_backend_popup_action
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(startAmbientSession:),
                start_ambient_session_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(openOrStartAmbientSession:),
                open_or_start_ambient_session_action
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(stopAmbientSession:),
                stop_ambient_session_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(setSummaryTemplate:),
                set_summary_template_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(openSessionFromSidebar:),
                open_session_from_sidebar_action
                    as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(openSessionFromHome:),
                open_session_from_home_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(reprocessSession:),
                reprocess_session_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
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

extern "C" fn show_home_action(_this: *mut AnyObject, _sel: Sel, _sender: *mut AnyObject) {
    show_main_window();
}

extern "C" fn show_settings_page_action(_this: *mut AnyObject, _sel: Sel, _sender: *mut AnyObject) {
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

extern "C" fn select_vision_backend_popup_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
    apply_vision_provider_selection(index as usize);
}

extern "C" fn select_vision_hotkey_popup_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
    apply_vision_hotkey_selection(index as usize);
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

extern "C" fn set_ambient_microphone_enabled_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let state: isize = unsafe { msg_send![sender, state] };
    set_ambient_microphone_enabled(state != 0);
}

extern "C" fn set_ambient_system_audio_enabled_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let state: isize = unsafe { msg_send![sender, state] };
    set_ambient_system_audio_enabled(state != 0);
}

extern "C" fn select_summary_model_popup_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
    apply_summary_model_selection(index as usize);
}

extern "C" fn select_ambient_final_backend_popup_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
    apply_ambient_final_backend_selection(index as usize);
}

extern "C" fn start_ambient_session_action(
    _this: *mut AnyObject,
    _sel: Sel,
    _sender: *mut AnyObject,
) {
    start_ambient_session();
}

extern "C" fn open_or_start_ambient_session_action(
    _this: *mut AnyObject,
    _sel: Sel,
    _sender: *mut AnyObject,
) {
    open_or_start_ambient_session();
}

extern "C" fn stop_ambient_session_action(
    _this: *mut AnyObject,
    _sel: Sel,
    _sender: *mut AnyObject,
) {
    stop_ambient_session();
}

extern "C" fn set_summary_template_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let index: isize = unsafe { msg_send![sender, indexOfSelectedItem] };
    set_summary_template(index as usize);
}

extern "C" fn reprocess_session_action(_this: *mut AnyObject, _sel: Sel, _sender: *mut AnyObject) {
    reprocess_current_session();
}

extern "C" fn open_session_from_sidebar_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let tag: isize = unsafe { msg_send![sender, tag] };
    open_session_from_sidebar_index(tag as usize);
}

extern "C" fn open_session_from_home_action(
    _this: *mut AnyObject,
    _sel: Sel,
    sender: *mut AnyObject,
) {
    let tag: isize = unsafe { msg_send![sender, tag] };
    open_session_from_home_index(tag as usize);
}

extern "C" fn application_should_handle_reopen(
    _this: *mut AnyObject,
    _sel: Sel,
    _sender: *mut AnyObject,
    _has_visible_windows: Bool,
) -> Bool {
    show_main_window();
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

fn apply_vision_hotkey_selection(index: usize) {
    let Some(hotkey_info) = HOTKEYS.get(index) else {
        sync_settings_window(&Config::load());
        return;
    };

    let mut config = Config::load();
    if config.vision_hotkey == hotkey_info.id {
        sync_settings_window(&config);
        return;
    }

    eprintln!("[screamer] Vision hotkey selected: {}", hotkey_info.id);
    config.vision_hotkey = hotkey_info.id.to_string();
    config.save();

    HOTKEY_MONITOR.with(|cell| {
        if let Some(hotkey) = cell.borrow().as_ref() {
            hotkey.set_vision_hotkey(hotkey_info.id);
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

fn apply_vision_provider_selection(index: usize) {
    let Some(entry) = VISION_PROVIDERS.get(index) else {
        sync_settings_window(&Config::load());
        return;
    };

    let mut config = Config::load();
    if config.vision_provider == entry.id {
        sync_settings_window(&config);
        return;
    }

    eprintln!(
        "[screamer] Vision screen-help backend: {}",
        entry.label
    );
    config.vision_provider = entry.id;
    config.save();

    if let Some(mtm) = MainThreadMarker::new() {
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

fn set_ambient_microphone_enabled(enabled: bool) {
    let mut config = Config::load();
    config.ambient_microphone = enabled;
    config.save();
    sync_settings_window(&config);
}

fn set_ambient_system_audio_enabled(enabled: bool) {
    let mut config = Config::load();
    config.ambient_system_audio = enabled;
    config.save();
    sync_settings_window(&config);
}

fn apply_ambient_final_backend_selection(index: usize) {
    let backend = MAIN_WINDOW.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|window| window.ambient_final_backend_for_index(index))
    });
    let Some(backend) = backend else {
        sync_settings_window(&Config::load());
        return;
    };

    let mut config = Config::load();
    if config.ambient_final_backend == backend {
        sync_settings_window(&config);
        return;
    }

    config.ambient_final_backend = backend;
    config.save();
    sync_settings_window(&config);

    eprintln!(
        "[screamer] Ambient final backend set to {}",
        config.ambient_final_backend_label()
    );
}

fn apply_summary_model_selection(index: usize) {
    let option = MAIN_WINDOW.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|window| window.summary_option_for_index(index))
    });
    let Some(option) = option else {
        sync_settings_window(&Config::load());
        return;
    };

    let mut config = Config::load();
    config.summary_backend = option.backend;
    if matches!(option.backend, SummaryBackendPreference::Ollama) {
        config.summary_ollama_model = option.value;
    }
    config.save();
    sync_settings_window(&config);
}

fn start_ambient_session() {
    let Some(controller) = AMBIENT_CONTROLLER.get() else {
        return;
    };
    let config = Config::load();
    if config.ambient_microphone {
        match permissions::prepare_microphone_permission() {
            permissions::MicrophonePermissionOutcome::Granted => {}
            permissions::MicrophonePermissionOutcome::Prompted => {
                eprintln!(
                    "[screamer] Requested microphone permission before ambient session start"
                );
                return;
            }
            permissions::MicrophonePermissionOutcome::Denied => {
                show_missing_microphone_permission_guidance();
                return;
            }
        }
    }
    let maybe_result = controller.start_session(&config);

    match maybe_result {
        Ok(session_id) => {
            MAIN_WINDOW.with(|cell| {
                if let Some(window) = cell.borrow().as_ref() {
                    window.show_session(session_id);
                }
            });
        }
        Err(err) => {
            if let Some(mtm) = MainThreadMarker::new() {
                App::show_alert(mtm, "Unable to start notetaker", &err);
            }
        }
    }
}

fn open_or_start_ambient_session() {
    let active_session_id = AMBIENT_CONTROLLER
        .get()
        .and_then(|controller| controller.active_snapshot().map(|snapshot| snapshot.id));

    if let Some(session_id) = active_session_id {
        MAIN_WINDOW.with(|cell| {
            if let Some(window) = cell.borrow().as_ref() {
                window.show_session(session_id);
            }
        });
        return;
    }

    start_ambient_session();
}

fn stop_ambient_session() {
    let Some(controller) = AMBIENT_CONTROLLER.get() else {
        return;
    };
    let maybe_result = controller.stop_session();
    if let Err(err) = maybe_result {
        if let Some(mtm) = MainThreadMarker::new() {
            App::show_alert(mtm, "Unable to stop notetaker", &err);
        }
    }
}

fn reprocess_current_session() {
    let session_id = MAIN_WINDOW.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|window| window.current_session_id())
    });
    let Some(session_id) = session_id else {
        return;
    };
    if session_id == 0 {
        return;
    }
    let Some(controller) = AMBIENT_CONTROLLER.get() else {
        return;
    };
    let config = Config::load();
    if let Err(err) = controller.reprocess_session(session_id, &config) {
        if let Some(mtm) = MainThreadMarker::new() {
            App::show_alert(mtm, "Unable to reprocess session", &err);
        }
    }
}

fn set_summary_template(index: usize) {
    MAIN_WINDOW.with(|cell| {
        if let Some(window) = cell.borrow().as_ref() {
            window.set_summary_template(index);
        }
    });
}

fn open_session_from_sidebar_index(index: usize) {
    MAIN_WINDOW.with(|cell| {
        if let Some(window) = cell.borrow().as_ref() {
            if let Some(session_id) = window.session_id_for_sidebar_index(index) {
                window.show_session(session_id);
            }
        }
    });
}

fn open_session_from_home_index(index: usize) {
    MAIN_WINDOW.with(|cell| {
        if let Some(window) = cell.borrow().as_ref() {
            if let Some(session_id) = window.session_id_for_home_index(index) {
                window.show_session(session_id);
            }
        }
    });
}

fn sync_settings_window(config: &Config) {
    MAIN_WINDOW.with(|cell| {
        if let Some(window) = cell.borrow().as_ref() {
            window.sync_config(config);
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
        let session_store =
            Arc::new(
                SessionStore::open_default().map_err(|message| AppInitError {
                    title: "Session Store Error",
                    message,
                })?,
            );
        let summary_registry = Arc::new(SummaryBackendRegistry::detect());
        let ambient_controller = Arc::new(AmbientController::new(
            session_store.clone(),
            transcriber.clone(),
            summary_registry.clone(),
            default_diarization_engine(),
        ));
        let _ = AMBIENT_CONTROLLER.set(ambient_controller.clone());
        let recorder = Arc::new(Recorder::new());
        theme::apply_app_appearance(mtm, config.appearance);
        let overlay = Rc::new(RefCell::new(Overlay::new(
            mtm,
            config.overlay_position,
            config.appearance,
        )));
        let highlight_overlay = Rc::new(RefCell::new(HighlightOverlay::new(mtm)));
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

        let main_window = MainWindow::new(
            mtm,
            &config,
            get_menu_handler(),
            session_store.clone(),
            ambient_controller.clone(),
            summary_registry.clone(),
        );
        MAIN_WINDOW.with(|cell| {
            *cell.borrow_mut() = Some(main_window.clone());
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
            main_window,
            hotkey,
            recorder,
            sound_player,
            transcriber,
            ambient_controller,
            _session_store: session_store,
            _summary_registry: summary_registry,
            is_recording: Arc::new(AtomicBool::new(false)),
            live_transcription_enabled,
            sound_effects_enabled,
            live_transcript: Arc::new(Mutex::new(String::new())),
            pending_completion_sound: Arc::new(AtomicBool::new(false)),
            recording_session: Arc::new(AtomicU64::new(0)),
            vision_ui_epoch: Arc::new(AtomicU64::new(0)),
            vision_overlay_state: Arc::new(Mutex::new(crate::overlay::VisionOverlayState::Hidden)),
            highlight_overlay,
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
            open_item.setAction(Some(sel!(showHomePage:)));
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
        let hotkey = self.hotkey.clone();

        // ── Shared state clones for dictation ──
        let ambient_controller = self.ambient_controller.clone();
        let sound_player_dict_press = self.sound_player.clone();
        let is_rec_dict_press = self.is_recording.clone();
        let is_rec_dict_release = self.is_recording.clone();
        let rec_dict_press = self.recorder.clone();
        let rec_dict_release = self.recorder.clone();
        let trans_dict_press = self.transcriber.clone();
        let trans_dict_release = self.transcriber.clone();
        let live_enabled_dict = self.live_transcription_enabled.clone();
        let sfx_dict_press = self.sound_effects_enabled.clone();
        let sfx_dict_release = self.sound_effects_enabled.clone();
        let live_transcript_dict_press = self.live_transcript.clone();
        let live_transcript_dict_release = self.live_transcript.clone();
        let pending_sound_dict = self.pending_completion_sound.clone();
        let session_dict = self.recording_session.clone();
        let vision_state_dict = self.vision_overlay_state.clone();

        // ── Shared state clones for vision ──
        let is_rec_vis_press = self.is_recording.clone();
        let is_rec_vis_release = self.is_recording.clone();
        let rec_vis_press = self.recorder.clone();
        let rec_vis_release = self.recorder.clone();
        let trans_vis_press = self.transcriber.clone();
        let trans_vis_release = self.transcriber.clone();
        let live_enabled_vis = self.live_transcription_enabled.clone();
        let sfx_vis_press = self.sound_effects_enabled.clone();
        let sfx_vis_release = self.sound_effects_enabled.clone();
        let sound_player_vis_press = self.sound_player.clone();
        let live_transcript_vis_press = self.live_transcript.clone();
        let live_transcript_vis_release = self.live_transcript.clone();
        let pending_sound_vis = self.pending_completion_sound.clone();
        let session_vis = self.recording_session.clone();
        let vision_state_vis_press = self.vision_overlay_state.clone();
        let vision_state_vis_release = self.vision_overlay_state.clone();
        let vision_ui_epoch_vis = self.vision_ui_epoch.clone();
        let vision_screenshot: Arc<Mutex<Option<crate::screenshot::CapturedScreen>>> =
            Arc::new(Mutex::new(None));
        let vision_screenshot_release = vision_screenshot.clone();

        hotkey.start_on_main_thread(
            mtm,
            // ── Dictation press ──
            move || {
                crate::speech::stop();
                // Clear any previous vision response
                if let Ok(mut vs) = vision_state_dict.lock() {
                    *vs = crate::overlay::VisionOverlayState::Hidden;
                }
                if ambient_controller.active_snapshot().is_some() {
                    eprintln!("[screamer] Ignoring dictation hotkey while ambient session is active");
                    return;
                }
                if is_rec_dict_press
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    if !permissions::has_accessibility_permission() {
                        eprintln!("[screamer] Accessibility permission missing");
                        permissions::prompt_for_accessibility_permission();
                        is_rec_dict_press.store(false, Ordering::SeqCst);
                        show_accessibility_window();
                        return;
                    }
                    match permissions::prepare_microphone_permission() {
                        permissions::MicrophonePermissionOutcome::Granted => {}
                        permissions::MicrophonePermissionOutcome::Prompted => {
                            is_rec_dict_press.store(false, Ordering::SeqCst);
                            return;
                        }
                        permissions::MicrophonePermissionOutcome::Denied => {
                            is_rec_dict_press.store(false, Ordering::SeqCst);
                            show_missing_microphone_permission_guidance();
                            return;
                        }
                    }

                    let session = session_dict.fetch_add(1, Ordering::SeqCst) + 1;
                    if let Ok(mut t) = live_transcript_dict_press.lock() { t.clear(); }
                    rec_dict_press.reset_buffers();

                    if sfx_dict_press.load(Ordering::Relaxed) {
                        sound_player_dict_press.play_recording_start();
                    }
                    start_recording_capture(
                        rec_dict_press.clone(),
                        trans_dict_press.clone(),
                        is_rec_dict_press.clone(),
                        live_enabled_dict.clone(),
                        live_transcript_dict_press.clone(),
                        session_dict.clone(),
                        session,
                    );
                    eprintln!("[screamer] Dictation recording armed");
                }
            },
            // ── Dictation release ──
            move || {
                if is_rec_dict_release
                    .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let release_t0 = std::time::Instant::now();
                    let samples = rec_dict_release.stop();
                    if let Ok(mut t) = live_transcript_dict_release.lock() { t.clear(); }
                    eprintln!("[screamer] Dictation stopped, {} samples", samples.len());

                    let transcribe_window = match prepare_final_transcription(&samples) {
                        FinalTranscriptionAction::SkipSilence => {
                            eprintln!("[screamer] Recording was silence, skipping");
                            if sfx_dict_release.load(Ordering::Relaxed) {
                                pending_sound_dict.store(true, Ordering::SeqCst);
                            }
                            return;
                        }
                        FinalTranscriptionAction::SkipTooShort { trimmed_len } => {
                            eprintln!("[screamer] Recording too short ({} samples), skipping", trimmed_len);
                            if sfx_dict_release.load(Ordering::Relaxed) {
                                pending_sound_dict.store(true, Ordering::SeqCst);
                            }
                            return;
                        }
                        FinalTranscriptionAction::Ready(window) => {
                            let trimmed_len = window.range.end - window.range.start;
                            if trimmed_len != samples.len() {
                                eprintln!("[screamer] Trimmed silence: {} -> {} samples", samples.len(), trimmed_len);
                            }
                            if matches!(window.kind, FinalSpeechWindowKind::ShortUtterance) {
                                eprintln!("[screamer] Salvaging brief utterance ({} samples)", trimmed_len);
                            }
                            window
                        }
                    };

                    let t = trans_dict_release.clone();
                    let pending_sound = pending_sound_dict.clone();
                    let sfx = sfx_dict_release.clone();
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
                                    eprintln!("[screamer] Accessibility permission missing; paste may fail");
                                }

                                let paste_t0 = std::time::Instant::now();
                                let paste_result = crate::paster::paste(&result.text);
                                let paste_ms = paste_t0.elapsed().as_millis();

                                match paste_result {
                                    Ok(()) => {
                                        eprintln!(
                                            "[screamer] Latency: stop={}ms | state={}ms | infer={}ms | extract={}ms | paste={}ms | total={}ms",
                                            stop_ms,
                                            result.profile.state_acquire.as_millis(),
                                            result.profile.inference.as_millis(),
                                            result.profile.extract.as_millis(),
                                            paste_ms,
                                            release_t0.elapsed().as_millis()
                                        );
                                    }
                                    Err(err) => {
                                        eprintln!("[screamer] Paste failed after {}ms: {}", paste_ms, err);
                                    }
                                }
                            }
                            Ok(_) => eprintln!("[screamer] Empty transcription, skipping"),
                            Err(e) => eprintln!("[screamer] Transcription error: {}", e),
                        }

                        if sfx.load(Ordering::Relaxed) {
                            pending_sound.store(true, Ordering::SeqCst);
                        }
                    });
                }
            },
            // ── Vision press: capture screenshot, then start recording ──
            move || {
                crate::speech::stop();
                // Clear any previous vision response
                if let Ok(mut vs) = vision_state_vis_press.lock() {
                    *vs = crate::overlay::VisionOverlayState::Hidden;
                }
                // Capture screenshot immediately
                match crate::screenshot::capture_screen() {
                    Ok(screenshot) => {
                        logging::eprint_vision_verbose_line(&format!(
                            "[screamer] Vision screenshot captured: {}",
                            screenshot.path.display()
                        ));
                        logging::log_vision_event(
                            "capture",
                            &format!(
                                "path={} bounds=({:.1}, {:.1}, {:.1}, {:.1})",
                                screenshot.path.display(),
                                screenshot.bounds.x,
                                screenshot.bounds.y,
                                screenshot.bounds.width,
                                screenshot.bounds.height
                            ),
                        );
                        if let Ok(mut ss) = vision_screenshot.lock() {
                            if let Some(old) = ss.take() {
                                let _ = std::fs::remove_file(&old.path);
                            }
                            *ss = Some(screenshot);
                        }
                    }
                    Err(err) => {
                        eprintln!("[screamer] Vision screenshot failed: {err}");
                        return;
                    }
                }
                crate::speech::warm_up();

                let clear_vision_screenshot = || {
                    if let Ok(mut ss) = vision_screenshot.lock() {
                        if let Some(screenshot) = ss.take() {
                            let _ = std::fs::remove_file(screenshot.path);
                        }
                    }
                };

                // Start recording (same flow as dictation)
                if is_rec_vis_press
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    match permissions::prepare_microphone_permission() {
                        permissions::MicrophonePermissionOutcome::Granted => {}
                        permissions::MicrophonePermissionOutcome::Prompted => {
                            is_rec_vis_press.store(false, Ordering::SeqCst);
                            clear_vision_screenshot();
                            return;
                        }
                        permissions::MicrophonePermissionOutcome::Denied => {
                            is_rec_vis_press.store(false, Ordering::SeqCst);
                            clear_vision_screenshot();
                            show_missing_microphone_permission_guidance();
                            return;
                        }
                    }

                    let session = session_vis.fetch_add(1, Ordering::SeqCst) + 1;
                    if let Ok(mut t) = live_transcript_vis_press.lock() { t.clear(); }
                    rec_vis_press.reset_buffers();

                    if sfx_vis_press.load(Ordering::Relaxed) {
                        sound_player_vis_press.play_recording_start();
                    }
                    start_recording_capture(
                        rec_vis_press.clone(),
                        trans_vis_press.clone(),
                        is_rec_vis_press.clone(),
                        live_enabled_vis.clone(),
                        live_transcript_vis_press.clone(),
                        session_vis.clone(),
                        session,
                    );
                    eprintln!("[screamer] Vision recording armed");
                } else {
                    clear_vision_screenshot();
                }
            },
            // ── Vision release: stop recording, transcribe, send to LLM ──
            move || {
                if is_rec_vis_release
                    .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let release_t0 = std::time::Instant::now();
                    let samples = rec_vis_release.stop();
                    if let Ok(mut t) = live_transcript_vis_release.lock() { t.clear(); }
                    eprintln!("[screamer] Vision recording stopped, {} samples", samples.len());

                    let screenshot = vision_screenshot_release
                        .lock()
                        .ok()
                        .and_then(|mut ss| ss.take());

                    let transcribe_window = match prepare_final_transcription(&samples) {
                        FinalTranscriptionAction::SkipSilence => {
                            eprintln!("[screamer] Vision recording was silence, skipping");
                            if sfx_vis_release.load(Ordering::Relaxed) {
                                pending_sound_vis.store(true, Ordering::SeqCst);
                            }
                            if let Some(screenshot) = screenshot.as_ref() {
                                let _ = std::fs::remove_file(&screenshot.path);
                            }
                            if let Ok(mut vs) = vision_state_vis_release.lock() {
                                *vs = crate::overlay::VisionOverlayState::Hidden;
                            }
                            return;
                        }
                        FinalTranscriptionAction::SkipTooShort { trimmed_len } => {
                            eprintln!("[screamer] Vision recording too short ({} samples), skipping", trimmed_len);
                            if sfx_vis_release.load(Ordering::Relaxed) {
                                pending_sound_vis.store(true, Ordering::SeqCst);
                            }
                            if let Some(screenshot) = screenshot.as_ref() {
                                let _ = std::fs::remove_file(&screenshot.path);
                            }
                            if let Ok(mut vs) = vision_state_vis_release.lock() {
                                *vs = crate::overlay::VisionOverlayState::Hidden;
                            }
                            return;
                        }
                        FinalTranscriptionAction::Ready(window) => {
                            let trimmed_len = window.range.end - window.range.start;
                            if trimmed_len != samples.len() {
                                eprintln!("[screamer] Trimmed silence: {} -> {} samples", samples.len(), trimmed_len);
                            }
                            window
                        }
                    };

                    // Set loading state (bump epoch first so stale TTS dismiss callbacks cannot apply)
                    vision_ui_epoch_vis.fetch_add(1, Ordering::SeqCst);
                    if let Ok(mut vs) = vision_state_vis_release.lock() {
                        *vs = crate::overlay::VisionOverlayState::Loading;
                    }

                    let t = trans_vis_release.clone();
                    let pending_sound = pending_sound_vis.clone();
                    let sfx = sfx_vis_release.clone();
                    let vision_state = vision_state_vis_release.clone();
                    let vision_ui_epoch_thread = vision_ui_epoch_vis.clone();

                    std::thread::spawn(move || {
                        let mut should_play_completion_sound = true;
                        match t.transcribe_profiled(&samples[transcribe_window.range]) {
                            Ok(result) if !result.text.is_empty() => {
                                logging::eprint_vision_verbose_line(&format!(
                                    "[screamer] Vision transcribed in {}ms ({} chars)",
                                    result.profile.total.as_millis(),
                                    result.text.chars().count()
                                ));
                                logging::log_transcript("Vision transcript", &result.text);

                                if let Some(screenshot) = screenshot.as_ref() {
                                    logging::eprint_vision_verbose_line(
                                        "[screamer] Vision: asking model about screenshot...",
                                    );
                                    let vision_t0 = std::time::Instant::now();
                                    logging::log_vision_event(
                                        "app_dispatch",
                                        &format!(
                                            "screenshot={} transcript_chars={}",
                                            screenshot.path.display(),
                                            result.text.chars().count()
                                        ),
                                    );
                                    match crate::vision::ask_about_screen(
                                        &result.text,
                                        &screenshot.path,
                                    ) {
                                        Ok(vision_result) => {
                                            logging::eprint_vision_verbose_line(&format!(
                                                "[screamer] Vision response in {}ms ({} chars):\n{}",
                                                vision_t0.elapsed().as_millis(),
                                                vision_result.text.len(),
                                                vision_result.text
                                            ));
                                            logging::log_vision_event(
                                                "app_result",
                                                &format!(
                                                    "latency_ms={} spoken_chars={} highlight={}",
                                                    vision_t0.elapsed().as_millis(),
                                                    vision_result.text.chars().count(),
                                                    vision_result
                                                        .highlight
                                                        .as_ref()
                                                        .map(|point| point.describe())
                                                        .unwrap_or_else(|| "NONE".to_string())
                                                ),
                                            );
                                            let dismiss_epoch = vision_ui_epoch_thread
                                                .fetch_add(1, Ordering::SeqCst)
                                                + 1;
                                            let highlight = vision_result.highlight.map(|point| {
                                                crate::highlight::HighlightTarget {
                                                    point,
                                                    capture_bounds: screenshot.bounds,
                                                }
                                            });
                                            if let Ok(mut vs) = vision_state.lock() {
                                                *vs = crate::overlay::VisionOverlayState::Response(
                                                    vision_result.text.clone(),
                                                    highlight,
                                                );
                                            }
                                            let epoch_for_done = vision_ui_epoch_thread.clone();
                                            let vision_state_done = vision_state.clone();
                                            match crate::speech::speak_with_completion(
                                                &vision_result.text,
                                                move || {
                                                    if epoch_for_done.load(Ordering::SeqCst)
                                                        != dismiss_epoch
                                                    {
                                                        return;
                                                    }
                                                    if let Ok(mut vs) = vision_state_done.lock() {
                                                        *vs = crate::overlay::VisionOverlayState::Hidden;
                                                    }
                                                },
                                            ) {
                                                Ok(()) => {
                                                    should_play_completion_sound = false;
                                                }
                                                Err(err) => {
                                                    eprintln!("[screamer] Vision speech error: {err}");
                                                    if vision_ui_epoch_thread.load(Ordering::SeqCst)
                                                        == dismiss_epoch
                                                    {
                                                        if let Ok(mut vs) = vision_state.lock() {
                                                            *vs = crate::overlay::VisionOverlayState::Hidden;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Err(err) => {
                                            eprintln!("[screamer] Vision model error: {err}");
                                            if let Ok(mut vs) = vision_state.lock() {
                                                *vs = crate::overlay::VisionOverlayState::Error(err);
                                            }
                                        }
                                    }
                                } else {
                                    eprintln!("[screamer] Vision release but no screenshot found");
                                    if let Ok(mut vs) = vision_state.lock() {
                                        *vs = crate::overlay::VisionOverlayState::Hidden;
                                    }
                                }
                            }
                            Ok(_) => {
                                eprintln!("[screamer] Vision: empty transcription, skipping");
                                if let Ok(mut vs) = vision_state.lock() {
                                    *vs = crate::overlay::VisionOverlayState::Hidden;
                                }
                            }
                            Err(e) => {
                                eprintln!("[screamer] Vision transcription error: {}", e);
                                if let Ok(mut vs) = vision_state.lock() {
                                    *vs = crate::overlay::VisionOverlayState::Error(
                                        format!("Transcription failed: {e}"),
                                    );
                                }
                            }
                        }

                        if should_play_completion_sound && sfx.load(Ordering::Relaxed) {
                            pending_sound.store(true, Ordering::SeqCst);
                        }
                        if let Some(screenshot) = screenshot {
                            let _ = std::fs::remove_file(&screenshot.path);
                        }
                    });
                }
            },
        );

        self.start_waveform_timer();
    }

    fn start_waveform_timer(&self) {
        let main_window = self.main_window.clone();
        let ambient_controller = self.ambient_controller.clone();
        let recorder = self.recorder.clone();
        let overlay = self.overlay.clone();
        let highlight_overlay = self.highlight_overlay.clone();
        let sound_player = self.sound_player.clone();
        let is_recording = self.is_recording.clone();
        let live_transcription_enabled = self.live_transcription_enabled.clone();
        let sound_effects_enabled = self.sound_effects_enabled.clone();
        let live_transcript = self.live_transcript.clone();
        let pending_completion_sound = self.pending_completion_sound.clone();
        let vision_overlay_state = self.vision_overlay_state.clone();

        unsafe {
            use objc2::ClassType;
            use objc2_foundation::NSTimer;

            let block = block2::RcBlock::new(move |_timer: *mut objc2::runtime::AnyObject| {
                sync_accessibility_window();
                ambient_controller.tick();
                main_window.tick();

                let recording = is_recording.load(Ordering::Relaxed);
                if !sound_effects_enabled.load(Ordering::Relaxed) {
                    pending_completion_sound.store(false, Ordering::SeqCst);
                } else if !recording && pending_completion_sound.swap(false, Ordering::SeqCst) {
                    sound_player.play_processing_done();
                }

                // Read the current vision state
                let vision_state = vision_overlay_state
                    .lock()
                    .map(|vs| vs.clone())
                    .unwrap_or(crate::overlay::VisionOverlayState::Hidden);

                let vision_active =
                    !matches!(vision_state, crate::overlay::VisionOverlayState::Hidden);

                // Manage the screen highlight overlay
                if let Ok(mut hl) = highlight_overlay.try_borrow_mut() {
                    match &vision_state {
                        crate::overlay::VisionOverlayState::Response(_, Some(target)) => {
                            if !hl.is_visible() {
                                hl.show(target);
                            }
                        }
                        _ => {
                            hl.hide();
                        }
                    }
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
                        // Sync vision state to overlay
                        ov.set_vision_state(vision_state);
                    } else if vision_active {
                        // Keep overlay visible while vision is loading/showing response
                        if !ov.is_visible() {
                            ov.show();
                        }
                        // Recording has stopped; drive the waveform toward silence so the HUD
                        // does not look frozen mid-capture while Loading / Response are shown.
                        let silent = [0f32; WAVEFORM_BINS];
                        ov.update_waveform(&silent);
                        ov.update_transcript("");
                        ov.set_vision_state(vision_state);
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
        let mut preview_state = LivePreviewState::new();

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
                preview_state.clear();
                continue;
            }

            let samples = recorder.snapshot();
            let (padded_samples, observed_samples_len) = match preview_state.next_action(&samples) {
                LivePreviewAction::Skip => continue,
                LivePreviewAction::Clear => {
                    if let Ok(mut transcript) = live_transcript.lock() {
                        transcript.clear();
                    }
                    preview_state.clear();
                    continue;
                }
                LivePreviewAction::Transcribe {
                    padded_samples,
                    observed_samples_len,
                } => (padded_samples, observed_samples_len),
            };

            match transcriber.try_transcribe(&padded_samples) {
                Ok(Some(text)) => {
                    let Some(display_text) =
                        preview_state.register_transcription(observed_samples_len, &text)
                    else {
                        continue;
                    };

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
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("[screamer] Live transcription error: {}", err);
                }
            }
        }
    });
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

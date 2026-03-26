use crate::config::{Config, HOTKEYS, MODELS};
use crate::overlay::Overlay;
use crate::recorder::Recorder;
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

pub struct App {
    _status_item: Retained<NSStatusItem>,
    overlay: Rc<RefCell<Overlay>>,
    recorder: Arc<Recorder>,
    transcriber: Arc<Transcriber>,
    _config: Config,
    is_recording: Arc<AtomicBool>,
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

        // Check if model file exists before switching
        if Transcriber::find_model(model_info.id).is_none() {
            eprintln!("[screamer] Model not found: {}", model_info.id);
            // Show alert on main thread
            if let Some(mtm) = MainThreadMarker::new() {
                unsafe {
                    let alert = NSAlert::new(mtm);
                    alert.setAlertStyle(NSAlertStyle::Warning);
                    alert.setMessageText(&NSString::from_str("Model Not Downloaded"));
                    alert.setInformativeText(&NSString::from_str(&format!(
                        "The {} model ({}) hasn't been downloaded yet.\n\n\
                         Run this in Terminal:\n\
                         cd {} && ./download_model.sh {}",
                        model_info.label,
                        model_info.size,
                        std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
                        model_info.id
                    )));
                    alert.runModal();
                }
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

fn relaunch() {
    eprintln!("[screamer] Relaunching...");
    // Find the .app bundle path (go up from MacOS/Screamer → Contents → Screamer.app)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(app_path) = exe.parent()   // MacOS/
            .and_then(|p| p.parent())           // Contents/
            .and_then(|p| p.parent())           // Screamer.app
        {
            let app_str = app_path.display().to_string();
            eprintln!("[screamer] Relaunching from: {}", app_str);
            let _ = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("sleep 0.5 && open -a '{}'", app_str))
                .spawn();
        } else {
            // Running from cargo — just relaunch the binary directly
            let exe_str = exe.display().to_string();
            eprintln!("[screamer] Relaunching binary: {}", exe_str);
            let _ = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("sleep 0.5 && '{}'", exe_str))
                .spawn();
        }
    }

    // Terminate current instance
    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        unsafe { app.terminate(None) };
    }
}

// ─── App ─────────────────────────────────────────────────────────────────────

impl App {
    pub fn new(mtm: MainThreadMarker) -> Option<Self> {
        let config = Config::load();

        // Find and load whisper model
        let model_path = match Transcriber::find_model(&config.model) {
            Some(p) => p,
            None => {
                Self::show_alert(
                    mtm,
                    "Model Not Found",
                    &format!(
                        "Could not find the whisper model 'ggml-{}.en.bin'.\n\n\
                         Please run: ./download_model.sh {}\n\n\
                         This will download the model to the models/ directory.",
                        config.model, config.model
                    ),
                );
                return None;
            }
        };

        log::info!("Loading model from: {:?}", model_path);

        let transcriber = match Transcriber::new(&model_path) {
            Ok(t) => Arc::new(t),
            Err(e) => {
                Self::show_alert(mtm, "Transcription Error", &e);
                return None;
            }
        };

        log::info!("Model loaded successfully");

        let recorder = Arc::new(Recorder::new());
        let overlay = Rc::new(RefCell::new(Overlay::new(mtm)));

        // Create menubar status item
        let status_bar = unsafe { NSStatusBar::systemStatusBar() };
        let status_item = unsafe {
            status_bar.statusItemWithLength(objc2_app_kit::NSSquareStatusItemLength)
        };

        // Set the button icon
        if let Some(button) = status_item.button(mtm) {
            let icon_name = NSString::from_str("menubarTemplate");
            if let Some(image) = objc2_app_kit::NSImage::imageNamed(&icon_name) {
                unsafe {
                    let _: () = msg_send![&image, setTemplate: true];
                    let _: () = msg_send![&image, setSize: objc2_core_foundation::CGSize::new(18.0, 18.0)];
                }
                button.setImage(Some(&image));
            } else {
                button.setTitle(&NSString::from_str("\u{1f3a4}"));
            }
        }

        // Build menu
        let menu = Self::build_menu(mtm, &config);

        unsafe {
            status_item.setMenu(Some(&menu));
        }

        Some(Self {
            _status_item: status_item,
            overlay,
            recorder,
            transcriber,
            _config: config,
            is_recording: Arc::new(AtomicBool::new(false)),
        })
    }

    fn build_menu(mtm: MainThreadMarker, config: &Config) -> Retained<NSMenu> {
        let handler = get_menu_handler();

        unsafe {
            let menu = NSMenu::new(mtm);

            // ── Status line ──
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
                    format!("{} ({}) — Not Downloaded", model_info.label, model_info.size)
                };
                item.setTitle(&NSString::from_str(&title));
                item.setTag(i as isize);
                let _: () = msg_send![&*item, setTarget: handler];
                item.setAction(Some(sel!(selectModel:)));

                // Checkmark on current model
                if model_info.id == config.model {
                    item.setState(1); // NSControlStateValueOn
                }

                model_submenu.addItem(&item);
            }

            model_item.setSubmenu(Some(&model_submenu));
            menu.addItem(&model_item);

            // ── Hotkey submenu ──
            let hotkey_item = NSMenuItem::new(mtm);
            hotkey_item.setTitle(&NSString::from_str(&format!("Hotkey: {}", config.hotkey_label())));
            let hotkey_submenu = NSMenu::new(mtm);
            hotkey_submenu.setTitle(&NSString::from_str("Hotkey"));

            for (i, hotkey_info) in HOTKEYS.iter().enumerate() {
                let item = NSMenuItem::new(mtm);
                item.setTitle(&NSString::from_str(hotkey_info.label));
                item.setTag(i as isize);
                let _: () = msg_send![&*item, setTarget: handler];
                item.setAction(Some(sel!(selectHotkey:)));

                // Checkmark on current hotkey
                if hotkey_info.id == config.hotkey {
                    item.setState(1); // NSControlStateValueOn
                }

                hotkey_submenu.addItem(&item);
            }

            hotkey_item.setSubmenu(Some(&hotkey_submenu));
            menu.addItem(&hotkey_item);

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

    /// Start the hotkey listener and timer for waveform updates
    pub fn start(&self, mtm: MainThreadMarker) {
        let recorder = self.recorder.clone();
        let transcriber = self.transcriber.clone();
        let is_recording = self.is_recording.clone();

        let rec_for_press = recorder.clone();
        let rec_for_release = recorder.clone();
        let trans_for_release = transcriber.clone();
        let is_rec_press = is_recording.clone();
        let is_rec_release = is_recording.clone();

        let hotkey = crate::hotkey::Hotkey::new();

        hotkey.start_on_main_thread(
            mtm,
            move || {
                // Key down — start recording
                if is_rec_press
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    rec_for_press.start();
                    eprintln!("[screamer] Recording started");
                }
            },
            move || {
                // Key up — stop recording, transcribe, paste
                if is_rec_release
                    .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let samples = rec_for_release.stop();
                    eprintln!("[screamer] Recording stopped, {} samples", samples.len());

                    if samples.len() < 4800 {
                        eprintln!("[screamer] Recording too short, skipping");
                        return;
                    }

                    let trans = trans_for_release.clone();
                    std::thread::spawn(move || {
                        let t0 = std::time::Instant::now();
                        match trans.transcribe(&samples) {
                            Ok(text) if !text.is_empty() => {
                                let inference_ms = t0.elapsed().as_millis();
                                eprintln!("[screamer] Transcribed in {}ms: {}", inference_ms, text);
                                crate::paster::paste(&text);
                                let total_ms = t0.elapsed().as_millis();
                                eprintln!("[screamer] Total latency (inference+paste): {}ms", total_ms);
                            }
                            Ok(_) => {
                                eprintln!("[screamer] Empty transcription, skipping paste");
                            }
                            Err(e) => {
                                eprintln!("[screamer] Transcription error: {}", e);
                            }
                        }
                    });
                }
            },
        );

        // Start NSTimer for overlay waveform updates (20fps)
        self.start_waveform_timer();
    }

    fn start_waveform_timer(&self) {
        let recorder = self.recorder.clone();
        let overlay = self.overlay.clone();
        let is_recording = self.is_recording.clone();

        unsafe {
            use objc2::ClassType;
            use objc2_foundation::NSTimer;

            let block = block2::RcBlock::new(move |_timer: *mut objc2::runtime::AnyObject| {
                let recording = is_recording.load(Ordering::Relaxed);
                if let Ok(mut ov) = overlay.try_borrow_mut() {
                    if recording {
                        if !ov.is_visible() {
                            ov.show();
                        }
                        let amps = recorder.amplitudes();
                        ov.update_amplitudes(&amps);
                    } else if ov.is_visible() {
                        ov.hide();
                    }
                }
            });

            let _timer: Retained<NSTimer> = msg_send![
                <NSTimer as ClassType>::class(),
                scheduledTimerWithTimeInterval: 0.05f64,
                repeats: true,
                block: &*block
            ];
        }
    }

    fn show_alert(mtm: MainThreadMarker, title: &str, message: &str) {
        unsafe {
            let alert = NSAlert::new(mtm);
            alert.setAlertStyle(NSAlertStyle::Critical);
            alert.setMessageText(&NSString::from_str(title));
            alert.setInformativeText(&NSString::from_str(message));
            alert.runModal();
        }
    }
}

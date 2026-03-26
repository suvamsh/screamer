use crate::config::Config;
use crate::overlay::Overlay;
use crate::recorder::Recorder;
use crate::transcriber::Transcriber;
use objc2::rc::Retained;
use objc2::sel;
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSMenu, NSMenuItem, NSStatusBar,
    NSStatusItem,
};
use objc2_foundation::{MainThreadMarker, NSString};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct App {
    _status_item: Retained<NSStatusItem>,
    overlay: Rc<RefCell<Overlay>>,
    recorder: Arc<Recorder>,
    transcriber: Arc<Transcriber>,
    _config: Config,
    is_recording: Arc<AtomicBool>,
}

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
                         Please run: ./download_model.sh\n\n\
                         This will download the model to the models/ directory.",
                        config.model
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

        // Set the button icon from template image
        if let Some(button) = status_item.button(mtm) {
            let icon_name = NSString::from_str("menubarTemplate");
            if let Some(image) = objc2_app_kit::NSImage::imageNamed(&icon_name) {
                unsafe {
                    let _: () = objc2::msg_send![&image, setTemplate: true];
                    let _: () = objc2::msg_send![&image, setSize: objc2_core_foundation::CGSize::new(18.0, 18.0)];
                }
                button.setImage(Some(&image));
            } else {
                button.setTitle(&NSString::from_str("\u{1f3a4}"));
            }
        }

        // Build menu
        let menu = unsafe {
            let menu = NSMenu::new(mtm);

            // Status line
            let status_line = NSMenuItem::new(mtm);
            status_line.setTitle(&NSString::from_str("Screamer \u{2014} Idle"));
            status_line.setEnabled(false);
            menu.addItem(&status_line);

            menu.addItem(&NSMenuItem::separatorItem(mtm));

            // Model info
            let model_label = NSMenuItem::new(mtm);
            model_label.setTitle(&NSString::from_str(&format!("Model: {}", config.model)));
            model_label.setEnabled(false);
            menu.addItem(&model_label);

            // Hotkey info
            let hotkey_label = NSMenuItem::new(mtm);
            hotkey_label.setTitle(&NSString::from_str("Hotkey: Left Control"));
            hotkey_label.setEnabled(false);
            menu.addItem(&hotkey_label);

            menu.addItem(&NSMenuItem::separatorItem(mtm));

            // Quit
            let quit_item = NSMenuItem::new(mtm);
            quit_item.setTitle(&NSString::from_str("Quit Screamer"));
            quit_item.setAction(Some(sel!(terminate:)));
            quit_item.setKeyEquivalent(&NSString::from_str("q"));
            menu.addItem(&quit_item);

            menu
        };

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
            use objc2::msg_send;
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

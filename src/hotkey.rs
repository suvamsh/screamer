use crate::config::{Config, HOTKEYS};
use objc2::msg_send;
use objc2_foundation::MainThreadMarker;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

// Modifier flags for detecting Option tap
const OPTION_MODIFIER: u64 = 0x00080000;

pub struct Hotkey {
    is_pressed: Arc<AtomicBool>,
    selected_index: Arc<AtomicUsize>,
}

impl Hotkey {
    pub fn new(config: &Config) -> Self {
        Self {
            is_pressed: Arc::new(AtomicBool::new(false)),
            selected_index: Arc::new(AtomicUsize::new(hotkey_index_for_id(&config.hotkey))),
        }
    }

    pub fn set_hotkey(&self, hotkey_id: &str) {
        let index = hotkey_index_for_id(hotkey_id);
        let hotkey_info = HOTKEYS.get(index).unwrap_or(&HOTKEYS[0]);
        self.selected_index.store(index, Ordering::Relaxed);
        eprintln!(
            "[screamer] Updated hotkey to: {} (modifier=0x{:x}, device=0x{:x})",
            hotkey_info.label, hotkey_info.modifier_flag, hotkey_info.device_flag
        );
    }

    /// Start listening using NSEvent global monitor on the main thread.
    ///
    /// Three callbacks:
    /// - `on_press` / `on_release`: dictation hotkey (configurable single modifier)
    /// - `on_screenshot_tap`: fires when Option is pressed while dictation is active
    pub fn start_on_main_thread(
        &self,
        _mtm: MainThreadMarker,
        on_press: impl Fn() + 'static,
        on_release: impl Fn() + 'static,
        on_screenshot_tap: impl Fn() + 'static,
    ) {
        let is_pressed = self.is_pressed.clone();
        let selected_index = self.selected_index.clone();

        // Track whether Option was down on the previous event so we detect edges.
        let option_was_down = Arc::new(AtomicBool::new(false));

        let hotkey_info = HOTKEYS
            .get(selected_index.load(Ordering::Relaxed))
            .unwrap_or(&HOTKEYS[0]);

        eprintln!(
            "[screamer] Hotkey configured: {} (modifier=0x{:x}, device=0x{:x})",
            hotkey_info.label, hotkey_info.modifier_flag, hotkey_info.device_flag
        );
        eprintln!("[screamer] Screenshot tap: Option (while recording)");

        // NSEventMaskFlagsChanged = 1 << 12 = 4096
        let mask: u64 = 1 << 12;

        let block = block2::RcBlock::new(move |event: *mut objc2::runtime::AnyObject| {
            if event.is_null() {
                return;
            }
            let flags: u64 = unsafe { msg_send![event, modifierFlags] };
            let hotkey_info = HOTKEYS
                .get(selected_index.load(Ordering::Relaxed))
                .unwrap_or(&HOTKEYS[0]);

            // ── Dictation hotkey (configurable modifier) ──
            let modifier_down = (flags & hotkey_info.modifier_flag) != 0;
            let key_down = if hotkey_info.device_flag != 0 {
                modifier_down && (flags & hotkey_info.device_flag) != 0
            } else {
                modifier_down
            };

            let was_pressed = is_pressed.load(Ordering::SeqCst);

            if key_down && !was_pressed {
                eprintln!(
                    "[screamer] Hotkey {} PRESSED (flags=0x{:x})",
                    hotkey_info.label, flags
                );
                is_pressed.store(true, Ordering::SeqCst);
                on_press();
            } else if !key_down && was_pressed {
                eprintln!(
                    "[screamer] Hotkey {} RELEASED (flags=0x{:x})",
                    hotkey_info.label, flags
                );
                is_pressed.store(false, Ordering::SeqCst);
                on_release();
            }

            // ── Option tap detection (only while recording) ──
            let option_down = (flags & OPTION_MODIFIER) != 0;
            let option_was = option_was_down.swap(option_down, Ordering::SeqCst);

            if option_down && !option_was && is_pressed.load(Ordering::SeqCst) {
                eprintln!(
                    "[screamer] Option tapped during recording (flags=0x{:x})",
                    flags
                );
                on_screenshot_tap();
            }
        });

        unsafe {
            let _monitor: *mut objc2::runtime::AnyObject = msg_send![
                objc2::class!(NSEvent),
                addGlobalMonitorForEventsMatchingMask: mask,
                handler: &*block
            ];

            if _monitor.is_null() {
                eprintln!("[screamer] Failed to create NSEvent global monitor");
            } else {
                eprintln!("[screamer] NSEvent global monitor installed for FlagsChanged");
            }
        }
    }
}

fn hotkey_index_for_id(hotkey_id: &str) -> usize {
    HOTKEYS
        .iter()
        .position(|hotkey| hotkey.id == hotkey_id)
        .unwrap_or(0)
}

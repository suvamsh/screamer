use crate::config::{Config, HOTKEYS};
use objc2::msg_send;
use objc2_foundation::MainThreadMarker;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

pub struct Hotkey {
    dictation_pressed: Arc<AtomicBool>,
    vision_pressed: Arc<AtomicBool>,
    dictation_index: Arc<AtomicUsize>,
    vision_index: Arc<AtomicUsize>,
}

impl Hotkey {
    pub fn new(config: &Config) -> Self {
        Self {
            dictation_pressed: Arc::new(AtomicBool::new(false)),
            vision_pressed: Arc::new(AtomicBool::new(false)),
            dictation_index: Arc::new(AtomicUsize::new(hotkey_index_for_id(&config.hotkey))),
            vision_index: Arc::new(AtomicUsize::new(hotkey_index_for_id(&config.vision_hotkey))),
        }
    }

    pub fn set_hotkey(&self, hotkey_id: &str) {
        let index = hotkey_index_for_id(hotkey_id);
        let hotkey_info = HOTKEYS.get(index).unwrap_or(&HOTKEYS[0]);
        self.dictation_index.store(index, Ordering::Relaxed);
        eprintln!(
            "[screamer] Updated dictation hotkey to: {} (modifier=0x{:x}, device=0x{:x})",
            hotkey_info.label, hotkey_info.modifier_flag, hotkey_info.device_flag
        );
    }

    pub fn set_vision_hotkey(&self, hotkey_id: &str) {
        let index = hotkey_index_for_id(hotkey_id);
        let hotkey_info = HOTKEYS.get(index).unwrap_or(&HOTKEYS[2]);
        self.vision_index.store(index, Ordering::Relaxed);
        eprintln!(
            "[screamer] Updated vision hotkey to: {} (modifier=0x{:x}, device=0x{:x})",
            hotkey_info.label, hotkey_info.modifier_flag, hotkey_info.device_flag
        );
    }

    /// Start listening using NSEvent global monitor on the main thread.
    ///
    /// Two independent push-to-talk keys:
    /// - Dictation: `on_dictation_press` / `on_dictation_release` (configurable key)
    /// - Vision: `on_vision_press` / `on_vision_release` (configurable key, default Left Option)
    pub fn start_on_main_thread(
        &self,
        _mtm: MainThreadMarker,
        on_dictation_press: impl Fn() + 'static,
        on_dictation_release: impl Fn() + 'static,
        on_vision_press: impl Fn() + 'static,
        on_vision_release: impl Fn() + 'static,
    ) {
        let dictation_pressed = self.dictation_pressed.clone();
        let vision_pressed = self.vision_pressed.clone();
        let dictation_index = self.dictation_index.clone();
        let vision_index = self.vision_index.clone();

        let dict_info = HOTKEYS
            .get(dictation_index.load(Ordering::Relaxed))
            .unwrap_or(&HOTKEYS[0]);
        let vis_info = HOTKEYS
            .get(vision_index.load(Ordering::Relaxed))
            .unwrap_or(&HOTKEYS[2]);

        eprintln!(
            "[screamer] Dictation hotkey: {} (modifier=0x{:x}, device=0x{:x})",
            dict_info.label, dict_info.modifier_flag, dict_info.device_flag
        );
        eprintln!(
            "[screamer] Vision hotkey: {} (modifier=0x{:x}, device=0x{:x})",
            vis_info.label, vis_info.modifier_flag, vis_info.device_flag
        );

        // NSEventMaskFlagsChanged = 1 << 12 = 4096
        let mask: u64 = 1 << 12;

        let block = block2::RcBlock::new(move |event: *mut objc2::runtime::AnyObject| {
            if event.is_null() {
                return;
            }
            let flags: u64 = unsafe { msg_send![event, modifierFlags] };

            // ── Dictation hotkey ──
            let dict_info = HOTKEYS
                .get(dictation_index.load(Ordering::Relaxed))
                .unwrap_or(&HOTKEYS[0]);

            let dict_modifier_down = (flags & dict_info.modifier_flag) != 0;
            let dict_key_down = if dict_info.device_flag != 0 {
                dict_modifier_down && (flags & dict_info.device_flag) != 0
            } else {
                dict_modifier_down
            };

            let dict_was = dictation_pressed.load(Ordering::SeqCst);

            if dict_key_down && !dict_was {
                eprintln!(
                    "[screamer] Dictation {} PRESSED (flags=0x{:x})",
                    dict_info.label, flags
                );
                dictation_pressed.store(true, Ordering::SeqCst);
                on_dictation_press();
            } else if !dict_key_down && dict_was {
                eprintln!(
                    "[screamer] Dictation {} RELEASED (flags=0x{:x})",
                    dict_info.label, flags
                );
                dictation_pressed.store(false, Ordering::SeqCst);
                on_dictation_release();
            }

            // ── Vision hotkey ──
            let vis_info = HOTKEYS
                .get(vision_index.load(Ordering::Relaxed))
                .unwrap_or(&HOTKEYS[2]);

            let vis_modifier_down = (flags & vis_info.modifier_flag) != 0;
            let vis_key_down = if vis_info.device_flag != 0 {
                vis_modifier_down && (flags & vis_info.device_flag) != 0
            } else {
                vis_modifier_down
            };

            let vis_was = vision_pressed.load(Ordering::SeqCst);

            if vis_key_down && !vis_was {
                // Don't activate vision if dictation is already active
                if !dictation_pressed.load(Ordering::SeqCst) {
                    eprintln!(
                        "[screamer] Vision {} PRESSED (flags=0x{:x})",
                        vis_info.label, flags
                    );
                    vision_pressed.store(true, Ordering::SeqCst);
                    on_vision_press();
                }
            } else if !vis_key_down && vis_was {
                eprintln!(
                    "[screamer] Vision {} RELEASED (flags=0x{:x})",
                    vis_info.label, flags
                );
                vision_pressed.store(false, Ordering::SeqCst);
                on_vision_release();
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

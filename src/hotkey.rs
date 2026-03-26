use crate::config::Config;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::NSEvent;
use objc2_foundation::MainThreadMarker;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct Hotkey {
    is_pressed: Arc<AtomicBool>,
}

impl Hotkey {
    pub fn new() -> Self {
        Self {
            is_pressed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn check_permissions() -> bool {
        // NSEvent global monitor doesn't need accessibility for modifier keys
        true
    }

    /// Start listening using NSEvent global monitor on the main thread.
    /// Reads hotkey config to determine which modifier key to watch.
    pub fn start_on_main_thread(
        &self,
        mtm: MainThreadMarker,
        on_press: impl Fn() + 'static,
        on_release: impl Fn() + 'static,
    ) {
        let is_pressed = self.is_pressed.clone();

        let config = Config::load();
        let hotkey_info = config.hotkey_info();
        let modifier_flag = hotkey_info.modifier_flag;
        let device_flag = hotkey_info.device_flag;
        let hotkey_name = hotkey_info.label;

        eprintln!("[screamer] Hotkey configured: {} (modifier=0x{:x}, device=0x{:x})",
            hotkey_name, modifier_flag, device_flag);

        // NSEventMaskFlagsChanged = 1 << 12 = 4096
        let mask: u64 = 1 << 12;

        let block = block2::RcBlock::new(move |event: *mut objc2::runtime::AnyObject| {
            if event.is_null() {
                return;
            }
            let flags: u64 = unsafe { msg_send![event, modifierFlags] };

            // Check if the modifier is down (device-independent)
            let modifier_down = (flags & modifier_flag) != 0;

            // If we have a device-specific flag, also check that
            let key_down = if device_flag != 0 {
                modifier_down && (flags & device_flag) != 0
            } else {
                modifier_down
            };

            let was_pressed = is_pressed.load(Ordering::SeqCst);

            if key_down && !was_pressed {
                eprintln!("[screamer] Hotkey PRESSED (flags=0x{:x})", flags);
                is_pressed.store(true, Ordering::SeqCst);
                on_press();
            } else if !key_down && was_pressed {
                eprintln!("[screamer] Hotkey RELEASED (flags=0x{:x})", flags);
                is_pressed.store(false, Ordering::SeqCst);
                on_release();
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

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
    /// This must be called from the main thread before NSApp.run().
    pub fn start_on_main_thread(
        &self,
        mtm: MainThreadMarker,
        on_press: impl Fn() + 'static,
        on_release: impl Fn() + 'static,
    ) {
        let is_pressed = self.is_pressed.clone();

        // NSEventMaskFlagsChanged = 1 << 12 = 4096
        let mask: u64 = 1 << 12;

        let block = block2::RcBlock::new(move |event: *mut objc2::runtime::AnyObject| {
            if event.is_null() {
                return;
            }
            let flags: u64 = unsafe { msg_send![event, modifierFlags] };

            // kCGEventFlagMaskControl = 0x00040000 = 262144
            let control_down = (flags & 0x00040000) != 0;

            let was_pressed = is_pressed.load(Ordering::SeqCst);

            if control_down && !was_pressed {
                eprintln!("[screamer] Control PRESSED (flags=0x{:x})", flags);
                is_pressed.store(true, Ordering::SeqCst);
                on_press();
            } else if !control_down && was_pressed {
                eprintln!("[screamer] Control RELEASED (flags=0x{:x})", flags);
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

use core_graphics::event::{CGEvent, CGEventFlags, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use std::thread;
use std::time::Duration;

const VK_ANSI_V: CGKeyCode = 9;

pub fn paste(text: &str) {
    // 1. Write to clipboard
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if let Err(e) = clipboard.set_text(text) {
            log::error!("Failed to set clipboard: {}", e);
            return;
        }
    } else {
        log::error!("Failed to open clipboard");
        return;
    }

    // 2. Brief delay for clipboard sync
    thread::sleep(Duration::from_millis(10));

    // 3. Simulate Cmd+V
    unsafe {
        let source =
            CGEventSource::new(CGEventSourceStateID::HIDSystemState).expect("CGEventSource");

        let key_down =
            CGEvent::new_keyboard_event(source.clone(), VK_ANSI_V, true).expect("key down event");
        key_down.set_flags(CGEventFlags::CGEventFlagCommand);

        let key_up =
            CGEvent::new_keyboard_event(source, VK_ANSI_V, false).expect("key up event");
        key_up.set_flags(CGEventFlags::CGEventFlagCommand);

        key_down.post(core_graphics::event::CGEventTapLocation::HID);
        key_up.post(core_graphics::event::CGEventTapLocation::HID);
    }
}

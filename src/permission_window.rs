use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::sel;
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSButtonType, NSColor, NSFont, NSLineBreakMode, NSTextAlignment,
    NSTextField, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{MainThreadMarker, NSString};
use std::rc::Rc;

const WINDOW_WIDTH: f64 = 560.0;
const WINDOW_HEIGHT: f64 = 220.0;

pub struct PermissionWindow {
    window: Retained<NSWindow>,
}

impl PermissionWindow {
    pub fn new(mtm: MainThreadMarker, handler: *const AnyObject) -> Rc<Self> {
        let style = NSWindowStyleMask::Titled;
        let frame = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        );

        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<NSWindow>(),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str("Enable Accessibility for Screamer"));
        window.center();
        window.setMinSize(CGSize::new(WINDOW_WIDTH, WINDOW_HEIGHT));
        window.setMovableByWindowBackground(false);
        window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
        window.setHidesOnDeactivate(false);
        unsafe {
            window.setReleasedWhenClosed(false);
        }

        let content = window
            .contentView()
            .expect("permission window should have content view");

        let title = make_label(
            mtm,
            "Enable Accessibility for Screamer",
            CGRect::new(CGPoint::new(28.0, 164.0), CGSize::new(504.0, 28.0)),
            24.0,
            true,
        );
        content.addSubview(&title);

        let steps = make_wrapped_label(
            mtm,
            "1. Click “Open Accessibility Settings”.\n2. Enable Screamer.\n3. Return to the app.",
            CGRect::new(CGPoint::new(28.0, 96.0), CGSize::new(504.0, 44.0)),
            14.0,
        );
        content.addSubview(&steps);

        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Open Accessibility Settings"),
                Some(&*handler),
                Some(sel!(openAccessibilitySettings:)),
                mtm,
            )
        };
        button.setFrame(CGRect::new(
            CGPoint::new(28.0, 28.0),
            CGSize::new(220.0, 32.0),
        ));
        button.setButtonType(NSButtonType::MomentaryPushIn);
        content.addSubview(&button);

        let dismiss = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Dismiss"),
                Some(&*handler),
                Some(sel!(dismissAccessibilityHelper:)),
                mtm,
            )
        };
        dismiss.setFrame(CGRect::new(
            CGPoint::new(264.0, 28.0),
            CGSize::new(104.0, 32.0),
        ));
        dismiss.setButtonType(NSButtonType::MomentaryPushIn);
        content.addSubview(&dismiss);

        Rc::new(Self { window })
    }

    pub fn show(&self) {
        self.window.makeKeyAndOrderFront(None);
        self.window.orderFrontRegardless();
    }

    pub fn hide(&self) {
        self.window.orderOut(None);
    }
}

fn make_label(
    mtm: MainThreadMarker,
    text: &str,
    frame: CGRect,
    size: f64,
    bold: bool,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFrame(frame);
    label.setEditable(false);
    label.setSelectable(false);
    label.setBordered(false);
    label.setDrawsBackground(false);
    label.setAlignment(NSTextAlignment::Left);
    let font = if bold {
        NSFont::boldSystemFontOfSize(size)
    } else {
        NSFont::systemFontOfSize(size)
    };
    label.setFont(Some(&font));
    label
}

fn make_wrapped_label(
    mtm: MainThreadMarker,
    text: &str,
    frame: CGRect,
    size: f64,
) -> Retained<NSTextField> {
    let label = make_label(mtm, text, frame, size, false);
    label.setMaximumNumberOfLines(0);
    label.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    label
}

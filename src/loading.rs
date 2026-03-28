use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSColor, NSImage, NSImageScaling, NSImageView, NSPanel,
    NSProgressIndicator, NSProgressIndicatorStyle, NSTextAlignment, NSTextField, NSView,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation::{MainThreadMarker, NSDate, NSRunLoop, NSString};
use std::path::PathBuf;

const PANEL_WIDTH: f64 = 320.0;
const PANEL_HEIGHT: f64 = 208.0;
const UI_PUMP_INTERVAL_SECS: f64 = 1.0 / 120.0;
const CONTENT_WIDTH: f64 = PANEL_WIDTH - 48.0;
const LOGO_SIZE: f64 = 72.0;
const TITLE_HEIGHT: f64 = 26.0;
const SUBTITLE_HEIGHT: f64 = 20.0;
const DIVIDER_HEIGHT: f64 = 1.0;
const SPINNER_SIZE: f64 = 24.0;
const LOGO_TITLE_GAP: f64 = 10.0;
const TITLE_SUBTITLE_GAP: f64 = 6.0;
const SUBTITLE_DIVIDER_GAP: f64 = 10.0;
const DIVIDER_SPINNER_GAP: f64 = 15.0;

pub struct LoadingWindow {
    panel: Retained<NSPanel>,
}

impl LoadingWindow {
    pub fn show(mtm: MainThreadMarker, app: &NSApplication) -> Self {
        let style = NSWindowStyleMask::Borderless;
        let frame = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(PANEL_WIDTH, PANEL_HEIGHT),
        );

        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            mtm.alloc::<NSPanel>(),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        );
        panel.setFloatingPanel(true);
        panel.setLevel(8);
        panel.setOpaque(false);
        panel.setHasShadow(true);
        panel.setMovableByWindowBackground(false);
        panel.setBecomesKeyOnlyIfNeeded(false);
        panel.setWorksWhenModal(true);
        panel.setHidesOnDeactivate(false);
        panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.center();

        let content_view = panel
            .contentView()
            .expect("loading panel should have content view");
        content_view.setWantsLayer(true);
        if let Some(layer) = content_view.layer() {
            layer.setCornerRadius(22.0 as CGFloat);
            layer.setMasksToBounds(true);
            let background = NSColor::colorWithCalibratedWhite_alpha(0.08, 0.96);
            unsafe {
                let cg_color: *const std::ffi::c_void = msg_send![&background, CGColor];
                let _: () = msg_send![&*layer, setBackgroundColor: cg_color];
            }
        }

        // Keep the loading content as one centered stack with symmetric padding.
        let content_height = LOGO_SIZE
            + LOGO_TITLE_GAP
            + TITLE_HEIGHT
            + TITLE_SUBTITLE_GAP
            + SUBTITLE_HEIGHT
            + SUBTITLE_DIVIDER_GAP
            + DIVIDER_HEIGHT
            + DIVIDER_SPINNER_GAP
            + SPINNER_SIZE;
        let stack_bottom = (PANEL_HEIGHT - content_height) / 2.0;
        let spinner_y = stack_bottom;
        let divider_y = spinner_y + SPINNER_SIZE + DIVIDER_SPINNER_GAP;
        let subtitle_y = divider_y + DIVIDER_HEIGHT + SUBTITLE_DIVIDER_GAP;
        let title_y = subtitle_y + SUBTITLE_HEIGHT + TITLE_SUBTITLE_GAP;
        let logo_y = title_y + TITLE_HEIGHT + LOGO_TITLE_GAP;

        if let Some(logo) = load_logo(mtm) {
            let logo_view = NSImageView::new(mtm);
            logo_view.setFrame(CGRect::new(
                CGPoint::new((PANEL_WIDTH - LOGO_SIZE) / 2.0, logo_y),
                CGSize::new(LOGO_SIZE, LOGO_SIZE),
            ));
            logo_view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            logo_view.setImage(Some(&logo));
            content_view.addSubview(&logo_view);
        }

        let title = label(
            mtm,
            "Loading Screamer…",
            CGRect::new(
                CGPoint::new((PANEL_WIDTH - CONTENT_WIDTH) / 2.0, title_y),
                CGSize::new(CONTENT_WIDTH, TITLE_HEIGHT),
            ),
            18.0,
            0.96,
        );
        title.setAlignment(NSTextAlignment(2));
        content_view.addSubview(&title);

        let subtitle = label(
            mtm,
            "Warming up the transcription model",
            CGRect::new(
                CGPoint::new((PANEL_WIDTH - CONTENT_WIDTH) / 2.0, subtitle_y),
                CGSize::new(CONTENT_WIDTH, SUBTITLE_HEIGHT),
            ),
            12.5,
            0.72,
        );
        subtitle.setAlignment(NSTextAlignment(2));
        subtitle.setMaximumNumberOfLines(2);
        content_view.addSubview(&subtitle);

        let divider = NSView::new(mtm);
        divider.setFrame(CGRect::new(
            CGPoint::new((PANEL_WIDTH - CONTENT_WIDTH) / 2.0, divider_y),
            CGSize::new(CONTENT_WIDTH, DIVIDER_HEIGHT),
        ));
        divider.setWantsLayer(true);
        if let Some(layer) = divider.layer() {
            let border = NSColor::colorWithCalibratedWhite_alpha(1.0, 0.12);
            unsafe {
                let cg_color: *const std::ffi::c_void = msg_send![&border, CGColor];
                let _: () = msg_send![&*layer, setBackgroundColor: cg_color];
            }
        }
        content_view.addSubview(&divider);

        let spinner = NSProgressIndicator::initWithFrame(
            mtm.alloc::<NSProgressIndicator>(),
            CGRect::new(
                CGPoint::new((PANEL_WIDTH - SPINNER_SIZE) / 2.0, spinner_y),
                CGSize::new(SPINNER_SIZE, SPINNER_SIZE),
            ),
        );
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setIndeterminate(true);
        spinner.setDisplayedWhenStopped(true);
        unsafe {
            spinner.startAnimation(None);
        }
        content_view.addSubview(&spinner);

        app.activate();
        panel.makeKeyAndOrderFront(None);
        panel.orderFrontRegardless();
        panel.displayIfNeeded();

        // Give AppKit one short turn to paint before synchronous model loading begins.
        pump();

        Self { panel }
    }

    pub fn close(&self) {
        self.panel.orderOut(None);
        self.panel.close();
    }
}

pub fn pump() {
    let run_loop = NSRunLoop::currentRunLoop();
    let deadline = NSDate::dateWithTimeIntervalSinceNow(UI_PUMP_INTERVAL_SECS);
    run_loop.runUntilDate(&deadline);
}

fn label(
    mtm: MainThreadMarker,
    text: &str,
    frame: CGRect,
    font_size: f64,
    alpha: f64,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFrame(frame);
    label.setDrawsBackground(false);
    label.setBordered(false);
    label.setBezeled(false);
    label.setEditable(false);
    label.setSelectable(false);
    label.setTextColor(Some(&NSColor::colorWithCalibratedWhite_alpha(1.0, alpha)));
    label.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(font_size)));
    label
}

fn load_logo(mtm: MainThreadMarker) -> Option<Retained<NSImage>> {
    let path = find_logo_path()?;
    let path = path.to_str()?;
    NSImage::initWithContentsOfFile(mtm.alloc::<NSImage>(), &NSString::from_str(path))
}

fn find_logo_path() -> Option<PathBuf> {
    let bundled_base = std::env::current_exe().ok().and_then(|exe| {
        exe.parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("Resources"))
    });

    if let Some(base) = bundled_base {
        for name in ["image.png", "logo.png"] {
            let path = base.join(name);
            if path.exists() {
                return Some(path);
            }
        }
    }

    for name in ["image.png", "logo.png"] {
        let local = PathBuf::from("resources").join(name);
        if local.exists() {
            return Some(local);
        }
    }

    None
}

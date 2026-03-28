use crate::config::{Config, HOTKEYS, MODELS, POSITIONS};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::sel;
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSImage,
    NSImageScaling, NSImageView, NSPopUpButton, NSSwitch, NSTextAlignment, NSTextField, NSView,
    NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation::{MainThreadMarker, NSString};
use std::path::PathBuf;
use std::rc::Rc;

const WINDOW_WIDTH: f64 = 660.0;
const WINDOW_HEIGHT: f64 = 500.0;
const OUTER_PADDING: f64 = 24.0;
const CARD_WIDTH: f64 = WINDOW_WIDTH - OUTER_PADDING * 2.0;
const CARD_INSET: f64 = 18.0;
const CARD_SPACING: f64 = 8.0;
const ROW_HEIGHT: f64 = 50.0;
const POPUP_WIDTH: f64 = 238.0;
const POPUP_HEIGHT: f64 = 30.0;
const SWITCH_WIDTH: f64 = 52.0;
const SWITCH_HEIGHT: f64 = 28.0;
const LOGO_SIZE: f64 = 84.0;

pub struct SettingsWindow {
    window: Retained<NSWindow>,
    model_popup: Retained<NSPopUpButton>,
    hotkey_popup: Retained<NSPopUpButton>,
    position_popup: Retained<NSPopUpButton>,
    live_switch: Retained<NSSwitch>,
    sound_switch: Retained<NSSwitch>,
}

impl SettingsWindow {
    pub fn new(mtm: MainThreadMarker, config: &Config, handler: *const AnyObject) -> Rc<Self> {
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable;
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
        window.setTitle(&NSString::from_str("Screamer"));
        window.center();
        window.setMinSize(CGSize::new(WINDOW_WIDTH, WINDOW_HEIGHT));
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setMovableByWindowBackground(true);
        unsafe {
            window.setReleasedWhenClosed(false);
        }

        let background = brand_background();
        window.setBackgroundColor(Some(&background));

        let content = window
            .contentView()
            .expect("settings window should have content view");
        style_surface(&content, &background, &background, 0.0);

        let logo_y = WINDOW_HEIGHT - OUTER_PADDING - LOGO_SIZE - 10.0;
        if let Some(logo) = load_logo(mtm) {
            let logo_view = NSImageView::imageViewWithImage(&logo, mtm);
            logo_view.setFrame(CGRect::new(
                CGPoint::new((WINDOW_WIDTH - LOGO_SIZE) / 2.0, logo_y),
                CGSize::new(LOGO_SIZE, LOGO_SIZE),
            ));
            logo_view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            content.addSubview(&logo_view);
        }

        let title_y = logo_y - 34.0;
        let title = title_label(
            mtm,
            "Screamer",
            CGRect::new(
                CGPoint::new(OUTER_PADDING, title_y),
                CGSize::new(CARD_WIDTH, 28.0),
            ),
            24.0,
        );
        title.setAlignment(NSTextAlignment::Center);
        content.addSubview(&title);

        let model_popup = popup_button(
            mtm,
            CGRect::new(
                CGPoint::new(
                    CARD_WIDTH - CARD_INSET - POPUP_WIDTH,
                    (ROW_HEIGHT - POPUP_HEIGHT) / 2.0,
                ),
                CGSize::new(POPUP_WIDTH, POPUP_HEIGHT),
            ),
            handler,
            sel!(selectModelPopup:),
        );
        for model in MODELS {
            model_popup.addItemWithTitle(&NSString::from_str(&window_model_title(model.id)));
        }

        let hotkey_popup = popup_button(
            mtm,
            CGRect::new(
                CGPoint::new(
                    CARD_WIDTH - CARD_INSET - POPUP_WIDTH,
                    (ROW_HEIGHT - POPUP_HEIGHT) / 2.0,
                ),
                CGSize::new(POPUP_WIDTH, POPUP_HEIGHT),
            ),
            handler,
            sel!(selectHotkeyPopup:),
        );
        for hotkey in HOTKEYS {
            hotkey_popup.addItemWithTitle(&NSString::from_str(hotkey.label));
        }

        let position_popup = popup_button(
            mtm,
            CGRect::new(
                CGPoint::new(
                    CARD_WIDTH - CARD_INSET - POPUP_WIDTH,
                    (ROW_HEIGHT - POPUP_HEIGHT) / 2.0,
                ),
                CGSize::new(POPUP_WIDTH, POPUP_HEIGHT),
            ),
            handler,
            sel!(selectPositionPopup:),
        );
        for position in POSITIONS {
            position_popup.addItemWithTitle(&NSString::from_str(position.label));
        }

        let live_switch = switch_button(
            mtm,
            CGRect::new(
                CGPoint::new(
                    CARD_WIDTH - CARD_INSET - SWITCH_WIDTH,
                    (ROW_HEIGHT - SWITCH_HEIGHT) / 2.0,
                ),
                CGSize::new(SWITCH_WIDTH, SWITCH_HEIGHT),
            ),
            handler,
            sel!(setLiveTranscriptionEnabled:),
        );

        let sound_switch = switch_button(
            mtm,
            CGRect::new(
                CGPoint::new(
                    CARD_WIDTH - CARD_INSET - SWITCH_WIDTH,
                    (ROW_HEIGHT - SWITCH_HEIGHT) / 2.0,
                ),
                CGSize::new(SWITCH_WIDTH, SWITCH_HEIGHT),
            ),
            handler,
            sel!(setSoundEffectsEnabled:),
        );

        let mut row_y = title_y - 18.0 - ROW_HEIGHT;
        add_select_row(mtm, &content, row_y, "Model", &model_popup);

        row_y -= CARD_SPACING + ROW_HEIGHT;
        add_select_row(mtm, &content, row_y, "Hotkey", &hotkey_popup);

        row_y -= CARD_SPACING + ROW_HEIGHT;
        add_select_row(mtm, &content, row_y, "Overlay Position", &position_popup);

        row_y -= CARD_SPACING + ROW_HEIGHT;
        add_toggle_row(mtm, &content, row_y, "Live Transcription", &live_switch);

        row_y -= CARD_SPACING + ROW_HEIGHT;
        add_toggle_row(mtm, &content, row_y, "Sound Effects", &sound_switch);

        let settings = Rc::new(Self {
            window,
            model_popup,
            hotkey_popup,
            position_popup,
            live_switch,
            sound_switch,
        });
        settings.sync(config);
        settings
    }

    pub fn show(&self) {
        self.window.makeKeyAndOrderFront(None);
        self.window.orderFrontRegardless();
    }

    pub fn sync(&self, config: &Config) {
        if let Some(index) = MODELS.iter().position(|model| model.id == config.model) {
            self.model_popup.selectItemAtIndex(index as isize);
        }

        if let Some(index) = HOTKEYS.iter().position(|hotkey| hotkey.id == config.hotkey) {
            self.hotkey_popup.selectItemAtIndex(index as isize);
        }

        if let Some(index) = POSITIONS
            .iter()
            .position(|position| position.id == config.overlay_position)
        {
            self.position_popup.selectItemAtIndex(index as isize);
        }

        self.live_switch.setState(if config.live_transcription {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        self.sound_switch.setState(if config.sound_effects {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }
}

fn add_select_row(mtm: MainThreadMarker, content: &NSView, y: f64, title: &str, control: &NSView) {
    let card = row_card(
        mtm,
        CGRect::new(
            CGPoint::new(OUTER_PADDING, y),
            CGSize::new(CARD_WIDTH, ROW_HEIGHT),
        ),
    );
    content.addSubview(&card);
    add_row_label(mtm, &card, title);
    card.addSubview(control);
}

fn add_toggle_row(mtm: MainThreadMarker, content: &NSView, y: f64, title: &str, control: &NSView) {
    let card = row_card(
        mtm,
        CGRect::new(
            CGPoint::new(OUTER_PADDING, y),
            CGSize::new(CARD_WIDTH, ROW_HEIGHT),
        ),
    );
    content.addSubview(&card);
    add_row_label(mtm, &card, title);
    card.addSubview(control);
}

fn add_row_label(mtm: MainThreadMarker, card: &NSView, title: &str) {
    let accent_height = 20.0;
    let accent_y = (ROW_HEIGHT - accent_height) / 2.0;
    let accent = surface_view(
        mtm,
        CGRect::new(
            CGPoint::new(18.0, accent_y),
            CGSize::new(4.0, accent_height),
        ),
        &brand_gold(),
        &brand_gold(),
        2.0,
    );
    card.addSubview(&accent);

    let label = text_label(
        mtm,
        title,
        CGRect::new(
            CGPoint::new(CARD_INSET + 8.0, (ROW_HEIGHT - 22.0) / 2.0),
            CGSize::new(280.0, 22.0),
        ),
        15.5,
        &brand_text(),
        true,
    );
    card.addSubview(&label);
}

fn row_card(mtm: MainThreadMarker, frame: CGRect) -> Retained<NSView> {
    surface_view(mtm, frame, &brand_surface(), &brand_card_border(), 16.0)
}

fn popup_button(
    mtm: MainThreadMarker,
    frame: CGRect,
    handler: *const AnyObject,
    action: objc2::runtime::Sel,
) -> Retained<NSPopUpButton> {
    let popup = NSPopUpButton::initWithFrame_pullsDown(mtm.alloc::<NSPopUpButton>(), frame, false);
    popup.setFont(Some(&NSFont::systemFontOfSize(14.0)));
    unsafe {
        popup.setTarget(Some(&*handler));
        popup.setAction(Some(action));
    }
    popup
}

fn switch_button(
    mtm: MainThreadMarker,
    frame: CGRect,
    handler: *const AnyObject,
    action: objc2::runtime::Sel,
) -> Retained<NSSwitch> {
    let switch = NSSwitch::initWithFrame(mtm.alloc::<NSSwitch>(), frame);
    unsafe {
        switch.setTarget(Some(&*handler));
        switch.setAction(Some(action));
    }
    switch
}

fn surface_view(
    mtm: MainThreadMarker,
    frame: CGRect,
    background: &NSColor,
    border: &NSColor,
    radius: f64,
) -> Retained<NSView> {
    let view = NSView::new(mtm);
    view.setFrame(frame);
    style_surface(&view, background, border, radius);
    view
}

fn style_surface(view: &NSView, background: &NSColor, border: &NSColor, radius: f64) {
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(radius as CGFloat);
        layer.setMasksToBounds(true);
        layer.setBorderWidth(1.0);
        unsafe {
            let bg_color: *const std::ffi::c_void = msg_send![background, CGColor];
            let border_color: *const std::ffi::c_void = msg_send![border, CGColor];
            let _: () = msg_send![&*layer, setBackgroundColor: bg_color];
            let _: () = msg_send![&*layer, setBorderColor: border_color];
        }
    }
}

fn title_label(
    mtm: MainThreadMarker,
    text: &str,
    frame: CGRect,
    font_size: f64,
) -> Retained<NSTextField> {
    text_label(mtm, text, frame, font_size, &brand_text(), true)
}

fn text_label(
    mtm: MainThreadMarker,
    text: &str,
    frame: CGRect,
    font_size: f64,
    color: &NSColor,
    bold: bool,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFrame(frame);
    label.setDrawsBackground(false);
    label.setBordered(false);
    label.setBezeled(false);
    label.setEditable(false);
    label.setSelectable(false);
    label.setTextColor(Some(color));

    let font = if bold {
        NSFont::boldSystemFontOfSize(font_size)
    } else {
        NSFont::systemFontOfSize(font_size)
    };
    label.setFont(Some(&font));
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
        for name in ["logo.png", "image.png"] {
            let path = base.join(name);
            if path.exists() {
                return Some(path);
            }
        }
    }

    for name in ["resources/logo.png", "resources/image.png"] {
        let local = PathBuf::from(name);
        if local.exists() {
            return Some(local);
        }
    }

    None
}

fn brand_background() -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(0.07, 0.06, 0.05, 1.0)
}

fn brand_surface() -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(0.11, 0.095, 0.08, 1.0)
}

fn brand_text() -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(0.97, 0.95, 0.92, 1.0)
}

fn brand_gold() -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(0.86, 0.70, 0.34, 1.0)
}

fn brand_card_border() -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(0.86, 0.70, 0.34, 0.14)
}

fn window_model_title(id: &str) -> String {
    MODELS
        .iter()
        .find(|model| model.id == id)
        .map(|model| format!("{} ({})", model.label, model.size))
        .unwrap_or_else(|| id.to_string())
}

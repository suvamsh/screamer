use crate::config::{AppAppearance, Config, HOTKEYS, MODELS, POSITIONS};
use crate::theme;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::sel;
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSButtonType, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSFont, NSImage, NSImageScaling, NSImageView, NSPopUpButton,
    NSSegmentDistribution, NSSegmentStyle, NSSegmentSwitchTracking, NSSegmentedControl, NSSwitch,
    NSTextAlignment, NSTextField, NSView, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation::{MainThreadMarker, NSString};
use std::path::PathBuf;
use std::rc::Rc;

const WINDOW_WIDTH: f64 = 660.0;
const WINDOW_HEIGHT: f64 = 678.0;
const OUTER_PADDING: f64 = 24.0;
const CARD_WIDTH: f64 = WINDOW_WIDTH - OUTER_PADDING * 2.0;
const CARD_INSET: f64 = 18.0;
const CARD_SPACING: f64 = 8.0;
const ROW_HEIGHT: f64 = 50.0;
const POPUP_WIDTH: f64 = 238.0;
const POPUP_HEIGHT: f64 = 30.0;
const SWITCH_WIDTH: f64 = 52.0;
const SWITCH_HEIGHT: f64 = 28.0;
const APPEARANCE_TOGGLE_WIDTH: f64 = 196.0;
const APPEARANCE_TOGGLE_HEIGHT: f64 = 30.0;
const ACTION_BUTTON_HEIGHT: f64 = 30.0;
const PERMISSION_BUTTON_WIDTH: f64 = 114.0;
const PERMISSION_BUTTON_GAP: f64 = 10.0;
const PERMISSION_SHORTCUTS_WIDTH: f64 = PERMISSION_BUTTON_WIDTH * 2.0 + PERMISSION_BUTTON_GAP;
const LOGO_SIZE: f64 = 84.0;
const LOGO_BADGE_SIZE: f64 = 104.0;

struct RowThemeViews {
    card: Retained<NSView>,
    accent: Retained<NSView>,
    label: Retained<NSTextField>,
}

pub struct SettingsWindow {
    window: Retained<NSWindow>,
    content: Retained<NSView>,
    logo_badge: Retained<NSView>,
    title: Retained<NSTextField>,
    row_views: Vec<RowThemeViews>,
    model_popup: Retained<NSPopUpButton>,
    hotkey_popup: Retained<NSPopUpButton>,
    vision_hotkey_popup: Retained<NSPopUpButton>,
    position_popup: Retained<NSPopUpButton>,
    appearance_toggle: Retained<NSSegmentedControl>,
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

        let content = window
            .contentView()
            .expect("settings window should have content view");

        let background = theme::window_background(config.appearance);
        window.setBackgroundColor(Some(&background));
        style_surface(&content, &background, &background, 0.0);

        let logo_y = WINDOW_HEIGHT - OUTER_PADDING - LOGO_SIZE - 10.0;
        let logo_badge = surface_view(
            mtm,
            CGRect::new(
                CGPoint::new((WINDOW_WIDTH - LOGO_BADGE_SIZE) / 2.0, logo_y - 10.0),
                CGSize::new(LOGO_BADGE_SIZE, LOGO_BADGE_SIZE),
            ),
            &theme::logo_badge_background(config.appearance),
            &theme::logo_badge_border(config.appearance),
            LOGO_BADGE_SIZE / 2.0,
        );
        content.addSubview(&logo_badge);

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
        let title = text_label(
            mtm,
            "Screamer",
            CGRect::new(
                CGPoint::new(OUTER_PADDING, title_y),
                CGSize::new(CARD_WIDTH, 28.0),
            ),
            24.0,
            &theme::title_text(config.appearance),
            true,
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

        let vision_hotkey_popup = popup_button(
            mtm,
            CGRect::new(
                CGPoint::new(
                    CARD_WIDTH - CARD_INSET - POPUP_WIDTH,
                    (ROW_HEIGHT - POPUP_HEIGHT) / 2.0,
                ),
                CGSize::new(POPUP_WIDTH, POPUP_HEIGHT),
            ),
            handler,
            sel!(selectVisionHotkeyPopup:),
        );
        for hotkey in HOTKEYS {
            vision_hotkey_popup.addItemWithTitle(&NSString::from_str(hotkey.label));
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

        let appearance_toggle = appearance_toggle(
            mtm,
            CGRect::new(
                CGPoint::new(
                    CARD_WIDTH - CARD_INSET - APPEARANCE_TOGGLE_WIDTH,
                    (ROW_HEIGHT - APPEARANCE_TOGGLE_HEIGHT) / 2.0,
                ),
                CGSize::new(APPEARANCE_TOGGLE_WIDTH, APPEARANCE_TOGGLE_HEIGHT),
            ),
            handler,
        );

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

        let permission_shortcuts = permission_shortcuts_view(
            mtm,
            CGRect::new(
                CGPoint::new(
                    CARD_WIDTH - CARD_INSET - PERMISSION_SHORTCUTS_WIDTH,
                    (ROW_HEIGHT - ACTION_BUTTON_HEIGHT) / 2.0,
                ),
                CGSize::new(PERMISSION_SHORTCUTS_WIDTH, ACTION_BUTTON_HEIGHT),
            ),
            handler,
        );

        let mut row_y = title_y - 18.0 - ROW_HEIGHT;
        let mut row_views = Vec::new();
        row_views.push(add_row(
            mtm,
            &content,
            row_y,
            "Model",
            &model_popup,
            config.appearance,
        ));

        row_y -= CARD_SPACING + ROW_HEIGHT;
        row_views.push(add_row(
            mtm,
            &content,
            row_y,
            "Hotkey",
            &hotkey_popup,
            config.appearance,
        ));

        row_y -= CARD_SPACING + ROW_HEIGHT;
        row_views.push(add_row(
            mtm,
            &content,
            row_y,
            "Vision Hotkey",
            &vision_hotkey_popup,
            config.appearance,
        ));

        row_y -= CARD_SPACING + ROW_HEIGHT;
        row_views.push(add_row(
            mtm,
            &content,
            row_y,
            "Overlay Position",
            &position_popup,
            config.appearance,
        ));

        row_y -= CARD_SPACING + ROW_HEIGHT;
        row_views.push(add_row(
            mtm,
            &content,
            row_y,
            "Appearance",
            &appearance_toggle,
            config.appearance,
        ));

        row_y -= CARD_SPACING + ROW_HEIGHT;
        row_views.push(add_row(
            mtm,
            &content,
            row_y,
            "Live Transcription",
            &live_switch,
            config.appearance,
        ));

        row_y -= CARD_SPACING + ROW_HEIGHT;
        row_views.push(add_row(
            mtm,
            &content,
            row_y,
            "Sound Effects",
            &sound_switch,
            config.appearance,
        ));

        row_y -= CARD_SPACING + ROW_HEIGHT;
        row_views.push(add_row(
            mtm,
            &content,
            row_y,
            "Permissions",
            &permission_shortcuts,
            config.appearance,
        ));

        let settings = Rc::new(Self {
            window,
            content,
            logo_badge,
            title,
            row_views,
            model_popup,
            hotkey_popup,
            vision_hotkey_popup,
            position_popup,
            appearance_toggle,
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
        self.apply_theme(config.appearance);

        if let Some(index) = MODELS.iter().position(|model| model.id == config.model) {
            self.model_popup.selectItemAtIndex(index as isize);
        }

        if let Some(index) = HOTKEYS.iter().position(|hotkey| hotkey.id == config.hotkey) {
            self.hotkey_popup.selectItemAtIndex(index as isize);
        }

        if let Some(index) = HOTKEYS
            .iter()
            .position(|hotkey| hotkey.id == config.vision_hotkey)
        {
            self.vision_hotkey_popup.selectItemAtIndex(index as isize);
        }

        if let Some(index) = POSITIONS
            .iter()
            .position(|position| position.id == config.overlay_position)
        {
            self.position_popup.selectItemAtIndex(index as isize);
        }

        self.appearance_toggle
            .setSelectedSegment(appearance_segment_index(config.appearance));

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

    fn apply_theme(&self, appearance: AppAppearance) {
        let background = theme::window_background(appearance);
        self.window.setBackgroundColor(Some(&background));
        style_surface(&self.content, &background, &background, 0.0);

        self.logo_badge
            .setHidden(matches!(appearance, AppAppearance::Dark));
        style_surface(
            &self.logo_badge,
            &theme::logo_badge_background(appearance),
            &theme::logo_badge_border(appearance),
            LOGO_BADGE_SIZE / 2.0,
        );

        let title_color = theme::title_text(appearance);
        self.title.setTextColor(Some(&title_color));

        let card_background = theme::surface_background(appearance);
        let card_border = theme::card_border(appearance);
        let gold = theme::brand_gold();
        let label_color = theme::body_text(appearance);
        for row in &self.row_views {
            style_surface(&row.card, &card_background, &card_border, 16.0);
            style_surface(&row.accent, &gold, &gold, 2.0);
            row.label.setTextColor(Some(&label_color));
        }

        self.appearance_toggle
            .setSelectedSegmentBezelColor(Some(&theme::brand_gold()));
    }
}

fn add_row(
    mtm: MainThreadMarker,
    content: &NSView,
    y: f64,
    title: &str,
    control: &NSView,
    appearance: AppAppearance,
) -> RowThemeViews {
    let card = row_card(
        mtm,
        CGRect::new(
            CGPoint::new(OUTER_PADDING, y),
            CGSize::new(CARD_WIDTH, ROW_HEIGHT),
        ),
        appearance,
    );
    content.addSubview(&card);
    let (accent, label) = add_row_label(mtm, &card, title, appearance);
    card.addSubview(control);

    RowThemeViews {
        card,
        accent,
        label,
    }
}

fn add_row_label(
    mtm: MainThreadMarker,
    card: &NSView,
    title: &str,
    appearance: AppAppearance,
) -> (Retained<NSView>, Retained<NSTextField>) {
    let accent_height = 20.0;
    let accent_y = (ROW_HEIGHT - accent_height) / 2.0;
    let accent = surface_view(
        mtm,
        CGRect::new(
            CGPoint::new(18.0, accent_y),
            CGSize::new(4.0, accent_height),
        ),
        &theme::brand_gold(),
        &theme::brand_gold(),
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
        &theme::body_text(appearance),
        true,
    );
    card.addSubview(&label);

    (accent, label)
}

fn row_card(mtm: MainThreadMarker, frame: CGRect, appearance: AppAppearance) -> Retained<NSView> {
    surface_view(
        mtm,
        frame,
        &theme::surface_background(appearance),
        &theme::card_border(appearance),
        16.0,
    )
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

fn appearance_toggle(
    mtm: MainThreadMarker,
    frame: CGRect,
    handler: *const AnyObject,
) -> Retained<NSSegmentedControl> {
    let toggle = NSSegmentedControl::initWithFrame(mtm.alloc::<NSSegmentedControl>(), frame);
    toggle.setSegmentCount(2);
    toggle.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
    toggle.setSegmentStyle(NSSegmentStyle::Capsule);
    toggle.setSegmentDistribution(NSSegmentDistribution::FillEqually);
    toggle.setLabel_forSegment(&NSString::from_str("☾ Dark"), 0);
    toggle.setLabel_forSegment(&NSString::from_str("☀ Light"), 1);
    toggle.setSelectedSegmentBezelColor(Some(&theme::brand_gold()));
    let font = NSFont::systemFontOfSize(13.0);
    unsafe {
        toggle.setTarget(Some(&*handler));
        toggle.setAction(Some(sel!(setAppearanceMode:)));
        let _: () = msg_send![&*toggle, setFont: &*font];
    }
    toggle
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

fn action_button(
    mtm: MainThreadMarker,
    frame: CGRect,
    title: &str,
    handler: *const AnyObject,
    action: objc2::runtime::Sel,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(&*handler),
            Some(action),
            mtm,
        )
    };
    button.setFrame(frame);
    button.setButtonType(NSButtonType::MomentaryPushIn);
    button
}

fn permission_shortcuts_view(
    mtm: MainThreadMarker,
    frame: CGRect,
    handler: *const AnyObject,
) -> Retained<NSView> {
    let container = NSView::new(mtm);
    container.setFrame(frame);

    let microphone_button = action_button(
        mtm,
        CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(PERMISSION_BUTTON_WIDTH, ACTION_BUTTON_HEIGHT),
        ),
        "Microphone",
        handler,
        sel!(openMicrophoneSettings:),
    );
    container.addSubview(&microphone_button);

    let accessibility_button = action_button(
        mtm,
        CGRect::new(
            CGPoint::new(PERMISSION_BUTTON_WIDTH + PERMISSION_BUTTON_GAP, 0.0),
            CGSize::new(PERMISSION_BUTTON_WIDTH, ACTION_BUTTON_HEIGHT),
        ),
        "Accessibility",
        handler,
        sel!(openAccessibilitySettings:),
    );
    container.addSubview(&accessibility_button);

    container
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

fn appearance_segment_index(appearance: AppAppearance) -> isize {
    match appearance {
        AppAppearance::Dark => 0,
        AppAppearance::Light => 1,
    }
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

fn window_model_title(id: &str) -> String {
    MODELS
        .iter()
        .find(|model| model.id == id)
        .map(|model| format!("{} ({})", model.label, model.size))
        .unwrap_or_else(|| id.to_string())
}

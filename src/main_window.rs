use crate::ambient_controller::AmbientController;
use crate::branding;
use crate::config::{
    AmbientFinalBackendPreference, AppAppearance, Config, AMBIENT_FINAL_BACKENDS, HOTKEYS,
    MODELS, POSITIONS,
};
use crate::session_store::{SessionStore, SessionSummary};
use crate::summary_backend::{SummaryBackendRegistry, SummaryModelOption};
use crate::theme;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::sel;
use objc2_app_kit::{
    NSBackingStoreType, NSBorderType, NSButton, NSButtonType, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSFont, NSImageScaling, NSImageView, NSLineBreakMode, NSPopUpButton,
    NSScrollView, NSSegmentedControl, NSSwitch, NSTextAlignment, NSTextField, NSTextView, NSView,
    NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation::{MainThreadMarker, NSString};
use screamer_core::ambient::{CanonicalSegment, SummaryTemplate};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const WINDOW_WIDTH: f64 = 1360.0;
const WINDOW_HEIGHT: f64 = 860.0;
const SIDEBAR_WIDTH: f64 = 286.0;
const CONTENT_PADDING: f64 = 26.0;
const SESSION_LIST_LIMIT: usize = 5;
const SETTINGS_COLUMN_WIDTH: f64 = 760.0;

const ROUTE_HOME: i32 = 0;
const ROUTE_SESSION: i32 = 1;
const ROUTE_SETTINGS: i32 = 2;

pub struct MainWindow {
    handler: *const AnyObject,
    window: Retained<NSWindow>,
    root: Retained<NSView>,
    sidebar: Retained<NSView>,
    content_host: Retained<NSView>,
    search_field: Retained<NSTextField>,
    home_button: Retained<NSButton>,
    settings_button: Retained<NSButton>,
    sidebar_sessions_container: Retained<NSView>,
    home_view: Retained<NSView>,
    home_banner: Retained<NSTextField>,
    home_dictation_hint: Retained<NSTextField>,
    home_primary_button: Retained<NSButton>,
    home_recent_container: Retained<NSView>,
    session_view: Retained<NSView>,
    session_title: Retained<NSTextField>,
    session_status: Retained<NSTextField>,
    session_timer: Retained<NSTextField>,
    session_inputs: Retained<NSTextField>,
    session_warning: Retained<NSTextField>,
    session_stop_button: Retained<NSButton>,
    scratch_pad_scroll: Retained<NSScrollView>,
    scratch_pad_text: Retained<NSTextView>,
    transcript_scroll: Retained<NSScrollView>,
    transcript_container: Retained<NSView>,
    session_template_popup: Retained<NSPopUpButton>,
    session_structured_scroll: Retained<NSScrollView>,
    session_structured_text: Retained<NSTextView>,
    session_reprocess_button: Retained<NSButton>,
    session_processing_overlay: Retained<NSView>,
    session_processing_label: Retained<NSTextField>,
    settings_view: Retained<NSView>,
    settings_model_popup: Retained<NSPopUpButton>,
    settings_hotkey_popup: Retained<NSPopUpButton>,
    settings_position_popup: Retained<NSPopUpButton>,
    settings_appearance_toggle: Retained<NSSegmentedControl>,
    settings_live_switch: Retained<NSSwitch>,
    settings_sound_switch: Retained<NSSwitch>,
    settings_ambient_mic_switch: Retained<NSSwitch>,
    settings_ambient_system_switch: Retained<NSSwitch>,
    settings_ambient_final_popup: Retained<NSPopUpButton>,
    settings_summary_popup: Retained<NSPopUpButton>,
    route: Cell<i32>,
    current_session_id: Cell<i64>,
    loaded_session_id: Cell<i64>,
    last_rendered_segment_count: Cell<usize>,
    last_rendered_segment_signature: RefCell<String>,
    last_persisted_notes: RefCell<String>,
    last_persisted_scratch_pad: RefCell<String>,
    last_editor_persist_at: RefCell<Instant>,
    last_sidebar_sync_at: RefCell<Instant>,
    last_summary_sync_at: RefCell<Instant>,
    last_sidebar_query: RefCell<String>,
    last_recent_signature: RefCell<Vec<(i64, i64, String)>>,
    sidebar_session_ids: RefCell<Vec<i64>>,
    home_recent_ids: RefCell<Vec<i64>>,
    summary_options: RefCell<Vec<SummaryModelOption>>,
    config: RefCell<Config>,
    store: Arc<SessionStore>,
    ambient_controller: Arc<AmbientController>,
    summary_registry: Arc<SummaryBackendRegistry>,
}

impl MainWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mtm: MainThreadMarker,
        config: &Config,
        handler: *const AnyObject,
        store: Arc<SessionStore>,
        ambient_controller: Arc<AmbientController>,
        summary_registry: Arc<SummaryBackendRegistry>,
    ) -> Rc<Self> {
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;
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
        window.setMinSize(CGSize::new(1180.0, 760.0));
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setMovableByWindowBackground(true);
        unsafe {
            window.setReleasedWhenClosed(false);
        }

        let root = window
            .contentView()
            .expect("main window should have content view");
        style_surface(
            &root,
            &theme::window_background(config.appearance),
            &theme::window_background(config.appearance),
            0.0,
        );

        let sidebar = surface_view(
            mtm,
            CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(SIDEBAR_WIDTH, WINDOW_HEIGHT),
            ),
            &theme::surface_background(config.appearance),
            &theme::card_border(config.appearance),
            0.0,
        );
        root.addSubview(&sidebar);

        let content_host = NSView::new(mtm);
        content_host.setFrame(CGRect::new(
            CGPoint::new(SIDEBAR_WIDTH, 0.0),
            CGSize::new(WINDOW_WIDTH - SIDEBAR_WIDTH, WINDOW_HEIGHT),
        ));
        root.addSubview(&content_host);

        let sidebar_brand_badge = surface_view(
            mtm,
            CGRect::new(
                CGPoint::new(18.0, WINDOW_HEIGHT - 100.0),
                CGSize::new(46.0, 46.0),
            ),
            &theme::window_background(config.appearance),
            &theme::card_border(config.appearance),
            23.0,
        );
        sidebar.addSubview(&sidebar_brand_badge);
        if let Some(logo) = branding::load_logo(mtm) {
            let logo_view = NSImageView::imageViewWithImage(&logo, mtm);
            logo_view.setFrame(CGRect::new(
                CGPoint::new(23.0, WINDOW_HEIGHT - 95.0),
                CGSize::new(36.0, 36.0),
            ));
            logo_view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            sidebar.addSubview(&logo_view);
        }

        let sidebar_title = text_label(
            mtm,
            "Screamer",
            CGRect::new(
                CGPoint::new(76.0, WINDOW_HEIGHT - 90.0),
                CGSize::new(SIDEBAR_WIDTH - 94.0, 22.0),
            ),
            18.0,
            &theme::title_text(config.appearance),
            true,
        );
        sidebar.addSubview(&sidebar_title);

        let sidebar_subtitle = text_label(
            mtm,
            "Offline voice notes",
            CGRect::new(
                CGPoint::new(76.0, WINDOW_HEIGHT - 112.0),
                CGSize::new(SIDEBAR_WIDTH - 94.0, 18.0),
            ),
            12.0,
            &theme::secondary_text(config.appearance),
            false,
        );
        sidebar.addSubview(&sidebar_subtitle);

        let search_field = text_field(
            mtm,
            CGRect::new(
                CGPoint::new(18.0, WINDOW_HEIGHT - 156.0),
                CGSize::new(SIDEBAR_WIDTH - 36.0, 38.0),
            ),
            "",
            "Search notes and sessions",
            config.appearance,
        );
        sidebar.addSubview(&search_field);

        let home_button = sidebar_button(
            mtm,
            CGRect::new(
                CGPoint::new(18.0, WINDOW_HEIGHT - 208.0),
                CGSize::new(SIDEBAR_WIDTH - 36.0, 40.0),
            ),
            "Home",
            handler,
            sel!(showHomePage:),
        );
        sidebar.addSubview(&home_button);

        let sidebar_label = text_label(
            mtm,
            "Recent sessions",
            CGRect::new(
                CGPoint::new(20.0, WINDOW_HEIGHT - 248.0),
                CGSize::new(SIDEBAR_WIDTH - 40.0, 18.0),
            ),
            12.0,
            &theme::secondary_text(config.appearance),
            false,
        );
        sidebar.addSubview(&sidebar_label);

        let sidebar_sessions_container = NSView::new(mtm);
        sidebar_sessions_container.setFrame(CGRect::new(
            CGPoint::new(18.0, 114.0),
            CGSize::new(SIDEBAR_WIDTH - 36.0, WINDOW_HEIGHT - 378.0),
        ));
        sidebar.addSubview(&sidebar_sessions_container);

        let settings_button = sidebar_button(
            mtm,
            CGRect::new(
                CGPoint::new(18.0, 24.0),
                CGSize::new(SIDEBAR_WIDTH - 36.0, 42.0),
            ),
            "Settings",
            handler,
            sel!(showSettingsPage:),
        );
        sidebar.addSubview(&settings_button);

        let home_view = NSView::new(mtm);
        home_view.setFrame(content_bounds());
        content_host.addSubview(&home_view);

        let home_heading = text_label(
            mtm,
            "Capture notes without the clutter",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 112.0),
                CGSize::new(760.0, 44.0),
            ),
            34.0,
            &theme::title_text(config.appearance),
            true,
        );
        home_view.addSubview(&home_heading);

        let home_banner = text_label(
            mtm,
            "",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 148.0),
                CGSize::new(860.0, 24.0),
            ),
            14.0,
            &theme::secondary_text(config.appearance),
            false,
        );
        home_view.addSubview(&home_banner);

        let home_card = surface_view(
            mtm,
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 356.0),
                CGSize::new(610.0, 172.0),
            ),
            &theme::surface_background(config.appearance),
            &theme::card_border(config.appearance),
            22.0,
        );
        home_view.addSubview(&home_card);

        let home_logo_badge = surface_view(
            mtm,
            CGRect::new(CGPoint::new(28.0, 42.0), CGSize::new(88.0, 88.0)),
            &theme::window_background(config.appearance),
            &theme::card_border(config.appearance),
            44.0,
        );
        home_card.addSubview(&home_logo_badge);
        if let Some(logo) = branding::load_logo(mtm) {
            let logo_view = NSImageView::imageViewWithImage(&logo, mtm);
            logo_view.setFrame(CGRect::new(
                CGPoint::new(38.0, 52.0),
                CGSize::new(68.0, 68.0),
            ));
            logo_view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            home_card.addSubview(&logo_view);
        }

        let home_card_title = text_label(
            mtm,
            "Start an ambient session",
            CGRect::new(CGPoint::new(140.0, 112.0), CGSize::new(300.0, 26.0)),
            24.0,
            &theme::title_text(config.appearance),
            true,
        );
        home_card.addSubview(&home_card_title);

        let home_card_body = wrapped_text_label(
            mtm,
            "Capture the conversation, clean up the notes, and leave with a concise local summary.",
            CGRect::new(CGPoint::new(140.0, 76.0), CGSize::new(306.0, 34.0)),
            13.0,
            &theme::secondary_text(config.appearance),
            false,
        );
        home_card.addSubview(&home_card_body);

        for (index, label) in ["Offline", "Whisper", "Gemma"].iter().enumerate() {
            let badge = badge_view(
                mtm,
                label,
                CGRect::new(
                    CGPoint::new(140.0 + index as f64 * 96.0, 30.0),
                    CGSize::new(84.0, 30.0),
                ),
                config.appearance,
            );
            home_card.addSubview(&badge);
        }

        let home_primary_button = primary_button(
            mtm,
            CGRect::new(CGPoint::new(454.0, 30.0), CGSize::new(126.0, 42.0)),
            "Start session",
            handler,
            sel!(startAmbientSession:),
        );
        home_card.addSubview(&home_primary_button);

        let dictation_card = surface_view(
            mtm,
            CGRect::new(
                CGPoint::new(CONTENT_PADDING + 634.0, WINDOW_HEIGHT - 356.0),
                CGSize::new(360.0, 172.0),
            ),
            &theme::surface_background(config.appearance),
            &theme::card_border(config.appearance),
            22.0,
        );
        home_view.addSubview(&dictation_card);

        let dictation_accent = surface_view(
            mtm,
            CGRect::new(CGPoint::new(24.0, 120.0), CGSize::new(10.0, 10.0)),
            &theme::brand_gold(),
            &theme::brand_gold(),
            5.0,
        );
        dictation_card.addSubview(&dictation_accent);

        let dictation_title = text_label(
            mtm,
            "Dictate anywhere",
            CGRect::new(CGPoint::new(24.0, 104.0), CGSize::new(260.0, 24.0)),
            22.0,
            &theme::title_text(config.appearance),
            true,
        );
        dictation_card.addSubview(&dictation_title);

        let home_dictation_hint = wrapped_text_label(
            mtm,
            "",
            CGRect::new(CGPoint::new(24.0, 64.0), CGSize::new(304.0, 34.0)),
            13.0,
            &theme::secondary_text(config.appearance),
            false,
        );
        dictation_card.addSubview(&home_dictation_hint);

        let dictation_footer = text_label(
            mtm,
            "Hold to speak. Release to paste.",
            CGRect::new(CGPoint::new(24.0, 28.0), CGSize::new(280.0, 16.0)),
            12.0,
            &theme::secondary_text(config.appearance),
            false,
        );
        dictation_card.addSubview(&dictation_footer);

        let recent_heading = text_label(
            mtm,
            "Recent sessions",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 412.0),
                CGSize::new(300.0, 24.0),
            ),
            18.0,
            &theme::title_text(config.appearance),
            true,
        );
        home_view.addSubview(&recent_heading);

        let home_recent_container = NSView::new(mtm);
        home_recent_container.setFrame(CGRect::new(
            CGPoint::new(CONTENT_PADDING, 54.0),
            CGSize::new(988.0, WINDOW_HEIGHT - 476.0),
        ));
        home_view.addSubview(&home_recent_container);

        let session_view = NSView::new(mtm);
        session_view.setFrame(content_bounds());
        session_view.setHidden(true);
        content_host.addSubview(&session_view);

        let session_title = text_label(
            mtm,
            "Ambient session",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 110.0),
                CGSize::new(620.0, 34.0),
            ),
            28.0,
            &theme::title_text(config.appearance),
            true,
        );
        session_view.addSubview(&session_title);

        let session_status = text_label(
            mtm,
            "",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 144.0),
                CGSize::new(300.0, 22.0),
            ),
            13.0,
            &theme::secondary_text(config.appearance),
            false,
        );
        session_view.addSubview(&session_status);

        let session_timer = text_label(
            mtm,
            "",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING + 300.0, WINDOW_HEIGHT - 144.0),
                CGSize::new(160.0, 22.0),
            ),
            13.0,
            &theme::secondary_text(config.appearance),
            false,
        );
        session_view.addSubview(&session_timer);

        let session_inputs = text_label(
            mtm,
            "",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING + 468.0, WINDOW_HEIGHT - 144.0),
                CGSize::new(360.0, 22.0),
            ),
            13.0,
            &theme::secondary_text(config.appearance),
            false,
        );
        session_view.addSubview(&session_inputs);

        let session_warning = text_label(
            mtm,
            "",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 174.0),
                CGSize::new(920.0, 22.0),
            ),
            12.0,
            &theme::brand_gold(),
            false,
        );
        session_view.addSubview(&session_warning);

        let session_stop_button = primary_button(
            mtm,
            CGRect::new(
                CGPoint::new(WINDOW_WIDTH - SIDEBAR_WIDTH - 172.0, WINDOW_HEIGHT - 128.0),
                CGSize::new(132.0, 42.0),
            ),
            "Finish",
            handler,
            sel!(stopAmbientSession:),
        );
        session_view.addSubview(&session_stop_button);

        let scratch_pad_heading = text_label(
            mtm,
            "Scratch pad",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 218.0),
                CGSize::new(280.0, 22.0),
            ),
            18.0,
            &theme::title_text(config.appearance),
            true,
        );
        session_view.addSubview(&scratch_pad_heading);

        let scratch_pad_top = WINDOW_HEIGHT - 410.0;
        let (scratch_pad_scroll, scratch_pad_text) = editor_scroll_view(
            mtm,
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, scratch_pad_top),
                CGSize::new(588.0, 166.0),
            ),
            config.appearance,
            true,
        );
        session_view.addSubview(&scratch_pad_scroll);

        let transcript_heading = text_label(
            mtm,
            "Live transcription",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, scratch_pad_top - 34.0),
                CGSize::new(280.0, 22.0),
            ),
            18.0,
            &theme::title_text(config.appearance),
            true,
        );
        session_view.addSubview(&transcript_heading);

        let transcript_area_height = scratch_pad_top - 34.0 - 12.0 - 60.0;
        let transcript_scroll = {
            let frame = CGRect::new(
                CGPoint::new(CONTENT_PADDING, 60.0),
                CGSize::new(588.0, transcript_area_height),
            );
            let scroll = NSScrollView::new(mtm);
            scroll.setFrame(frame);
            scroll.setHasVerticalScroller(true);
            scroll.setBorderType(NSBorderType::NoBorder);
            scroll.setDrawsBackground(false);
            style_surface(
                &scroll,
                &theme::surface_background(config.appearance),
                &theme::card_border(config.appearance),
                18.0,
            );
            scroll
        };
        let transcript_container = NSView::new(mtm);
        transcript_container.setFrame(CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(588.0, transcript_area_height),
        ));
        transcript_scroll.setDocumentView(Some(&transcript_container));
        session_view.addSubview(&transcript_scroll);

        let structured_heading = text_label(
            mtm,
            "Summary",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING + 618.0, WINDOW_HEIGHT - 218.0),
                CGSize::new(120.0, 22.0),
            ),
            18.0,
            &theme::title_text(config.appearance),
            true,
        );
        session_view.addSubview(&structured_heading);

        let session_template_popup = popup_button(
            mtm,
            CGRect::new(
                CGPoint::new(CONTENT_PADDING + 618.0 + 130.0, WINDOW_HEIGHT - 220.0),
                CGSize::new(160.0, 26.0),
            ),
            handler,
            sel!(setSummaryTemplate:),
        );
        for template in SummaryTemplate::all() {
            session_template_popup.addItemWithTitle(&NSString::from_str(template.label()));
        }
        session_view.addSubview(&session_template_popup);

        let (session_structured_scroll, session_structured_text) = editor_scroll_view(
            mtm,
            CGRect::new(
                CGPoint::new(CONTENT_PADDING + 618.0, 60.0),
                CGSize::new(392.0, WINDOW_HEIGHT - 306.0),
            ),
            config.appearance,
            false,
        );
        session_view.addSubview(&session_structured_scroll);

        // Reprocess button — shown for completed/failed sessions
        let session_reprocess_button = primary_button(
            mtm,
            CGRect::new(
                CGPoint::new(WINDOW_WIDTH - SIDEBAR_WIDTH - 172.0, WINDOW_HEIGHT - 128.0),
                CGSize::new(132.0, 42.0),
            ),
            "Reprocess",
            handler,
            sel!(reprocessSession:),
        );
        session_reprocess_button.setHidden(true);
        session_view.addSubview(&session_reprocess_button);

        // Processing overlay — covers transcript + scratch pad areas during processing
        let processing_overlay_frame = CGRect::new(
            CGPoint::new(CONTENT_PADDING, 60.0),
            CGSize::new(588.0, WINDOW_HEIGHT - 240.0),
        );
        let session_processing_overlay = surface_view(
            mtm,
            processing_overlay_frame,
            &theme::processing_overlay_background(config.appearance),
            &theme::card_border(config.appearance),
            18.0,
        );
        session_processing_overlay.setHidden(true);

        let spinner_label = text_label(
            mtm,
            "◉",
            CGRect::new(
                CGPoint::new(0.0, processing_overlay_frame.size.height / 2.0 + 10.0),
                CGSize::new(processing_overlay_frame.size.width, 28.0),
            ),
            22.0,
            &theme::processing_accent(),
            false,
        );
        spinner_label.setAlignment(NSTextAlignment::Center);
        session_processing_overlay.addSubview(&spinner_label);

        let session_processing_label = text_label(
            mtm,
            "Processing session\u{2026}",
            CGRect::new(
                CGPoint::new(0.0, processing_overlay_frame.size.height / 2.0 - 18.0),
                CGSize::new(processing_overlay_frame.size.width, 22.0),
            ),
            15.0,
            &theme::secondary_text(config.appearance),
            true,
        );
        session_processing_label.setAlignment(NSTextAlignment::Center);
        session_processing_overlay.addSubview(&session_processing_label);

        let processing_hint = text_label(
            mtm,
            "Generating summary from transcript and notes",
            CGRect::new(
                CGPoint::new(0.0, processing_overlay_frame.size.height / 2.0 - 42.0),
                CGSize::new(processing_overlay_frame.size.width, 18.0),
            ),
            12.0,
            &theme::secondary_text(config.appearance),
            false,
        );
        processing_hint.setAlignment(NSTextAlignment::Center);
        session_processing_overlay.addSubview(&processing_hint);

        session_view.addSubview(&session_processing_overlay);

        let settings_view = NSView::new(mtm);
        settings_view.setFrame(content_bounds());
        settings_view.setHidden(true);
        content_host.addSubview(&settings_view);

        let settings_heading = text_label(
            mtm,
            "Settings",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 112.0),
                CGSize::new(420.0, 34.0),
            ),
            30.0,
            &theme::title_text(config.appearance),
            true,
        );
        settings_view.addSubview(&settings_heading);

        let settings_subtitle = wrapped_text_label(
            mtm,
            "Keep the defaults lean. Pick a hotkey, a summary model, and get back to recording.",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 148.0),
                CGSize::new(720.0, 20.0),
            ),
            13.0,
            &theme::secondary_text(config.appearance),
            false,
        );
        settings_view.addSubview(&settings_subtitle);

        let settings_ambient_final_label = text_label(
            mtm,
            "Ambient final pass",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 188.0),
                CGSize::new(220.0, 18.0),
            ),
            12.5,
            &theme::secondary_text(config.appearance),
            true,
        );
        settings_view.addSubview(&settings_ambient_final_label);

        let settings_ambient_final_popup = popup_button(
            mtm,
            CGRect::new(
                CGPoint::new(CONTENT_PADDING + 468.0, WINDOW_HEIGHT - 198.0),
                CGSize::new(260.0, 30.0),
            ),
            handler,
            sel!(selectAmbientFinalBackendPopup:),
        );
        for backend in AMBIENT_FINAL_BACKENDS {
            settings_ambient_final_popup.addItemWithTitle(&NSString::from_str(backend.label));
        }
        settings_view.addSubview(&settings_ambient_final_popup);

        let settings_summary_popup = popup_button(
            mtm,
            CGRect::new(CGPoint::new(468.0, 11.0), CGSize::new(260.0, 32.0)),
            handler,
            sel!(selectSummaryModelPopup:),
        );
        add_settings_row(
            mtm,
            &settings_view,
            config.appearance,
            "Summary model",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 242.0),
                CGSize::new(SETTINGS_COLUMN_WIDTH, 54.0),
            ),
            &settings_summary_popup,
        );

        let settings_model_popup = popup_button(
            mtm,
            CGRect::new(CGPoint::new(468.0, 11.0), CGSize::new(260.0, 32.0)),
            handler,
            sel!(selectModelPopup:),
        );
        for model in MODELS {
            settings_model_popup.addItemWithTitle(&NSString::from_str(model.label));
        }
        add_settings_row(
            mtm,
            &settings_view,
            config.appearance,
            "Whisper model",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 310.0),
                CGSize::new(SETTINGS_COLUMN_WIDTH, 54.0),
            ),
            &settings_model_popup,
        );

        let settings_hotkey_popup = popup_button(
            mtm,
            CGRect::new(CGPoint::new(468.0, 11.0), CGSize::new(260.0, 32.0)),
            handler,
            sel!(selectHotkeyPopup:),
        );
        for hotkey in HOTKEYS {
            settings_hotkey_popup.addItemWithTitle(&NSString::from_str(hotkey.label));
        }
        add_settings_row(
            mtm,
            &settings_view,
            config.appearance,
            "Push-to-talk key",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 378.0),
                CGSize::new(SETTINGS_COLUMN_WIDTH, 54.0),
            ),
            &settings_hotkey_popup,
        );

        let settings_position_popup = popup_button(
            mtm,
            CGRect::new(CGPoint::new(468.0, 11.0), CGSize::new(260.0, 32.0)),
            handler,
            sel!(selectPositionPopup:),
        );
        for position in POSITIONS {
            settings_position_popup.addItemWithTitle(&NSString::from_str(position.label));
        }
        add_settings_row(
            mtm,
            &settings_view,
            config.appearance,
            "Overlay position",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 446.0),
                CGSize::new(SETTINGS_COLUMN_WIDTH, 54.0),
            ),
            &settings_position_popup,
        );

        let settings_appearance_toggle = appearance_toggle(
            mtm,
            CGRect::new(CGPoint::new(468.0, 11.0), CGSize::new(260.0, 32.0)),
            handler,
        );
        add_settings_row(
            mtm,
            &settings_view,
            config.appearance,
            "Appearance",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 514.0),
                CGSize::new(SETTINGS_COLUMN_WIDTH, 54.0),
            ),
            &settings_appearance_toggle,
        );

        let settings_live_switch = switch_button(
            mtm,
            CGRect::new(CGPoint::new(676.0, 13.0), CGSize::new(52.0, 28.0)),
            handler,
            sel!(setLiveTranscriptionEnabled:),
        );
        add_settings_row(
            mtm,
            &settings_view,
            config.appearance,
            "Live preview",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 582.0),
                CGSize::new(SETTINGS_COLUMN_WIDTH, 54.0),
            ),
            &settings_live_switch,
        );

        let settings_sound_switch = switch_button(
            mtm,
            CGRect::new(CGPoint::new(676.0, 13.0), CGSize::new(52.0, 28.0)),
            handler,
            sel!(setSoundEffectsEnabled:),
        );
        add_settings_row(
            mtm,
            &settings_view,
            config.appearance,
            "Sounds",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 650.0),
                CGSize::new(SETTINGS_COLUMN_WIDTH, 54.0),
            ),
            &settings_sound_switch,
        );

        let settings_ambient_mic_switch = switch_button(
            mtm,
            CGRect::new(CGPoint::new(676.0, 13.0), CGSize::new(52.0, 28.0)),
            handler,
            sel!(setAmbientMicrophoneEnabled:),
        );
        add_settings_row(
            mtm,
            &settings_view,
            config.appearance,
            "Microphone lane",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 718.0),
                CGSize::new(SETTINGS_COLUMN_WIDTH, 54.0),
            ),
            &settings_ambient_mic_switch,
        );

        let settings_ambient_system_switch = switch_button(
            mtm,
            CGRect::new(CGPoint::new(676.0, 13.0), CGSize::new(52.0, 28.0)),
            handler,
            sel!(setAmbientSystemAudioEnabled:),
        );
        add_settings_row(
            mtm,
            &settings_view,
            config.appearance,
            "System audio lane",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 786.0),
                CGSize::new(SETTINGS_COLUMN_WIDTH, 54.0),
            ),
            &settings_ambient_system_switch,
        );

        let permission_shortcuts = permission_shortcuts_view(
            mtm,
            CGRect::new(CGPoint::new(496.0, 12.0), CGSize::new(232.0, 30.0)),
            handler,
        );
        add_settings_row(
            mtm,
            &settings_view,
            config.appearance,
            "Permissions",
            CGRect::new(
                CGPoint::new(CONTENT_PADDING, WINDOW_HEIGHT - 854.0),
                CGSize::new(SETTINGS_COLUMN_WIDTH, 54.0),
            ),
            &permission_shortcuts,
        );

        let window = Rc::new(Self {
            handler,
            window,
            root,
            sidebar,
            content_host,
            search_field,
            home_button,
            settings_button,
            sidebar_sessions_container,
            home_view,
            home_banner,
            home_dictation_hint,
            home_primary_button,
            home_recent_container,
            session_view,
            session_title,
            session_status,
            session_timer,
            session_inputs,
            session_warning,
            session_stop_button,
            scratch_pad_scroll,
            scratch_pad_text,
            transcript_scroll,
            transcript_container,
            session_template_popup,
            session_structured_scroll,
            session_structured_text,
            session_reprocess_button,
            session_processing_overlay,
            session_processing_label,
            settings_view,
            settings_model_popup,
            settings_hotkey_popup,
            settings_position_popup,
            settings_appearance_toggle,
            settings_live_switch,
            settings_sound_switch,
            settings_ambient_mic_switch,
            settings_ambient_system_switch,
            settings_ambient_final_popup,
            settings_summary_popup,
            route: Cell::new(ROUTE_HOME),
            current_session_id: Cell::new(0),
            loaded_session_id: Cell::new(0),
            last_rendered_segment_count: Cell::new(0),
            last_rendered_segment_signature: RefCell::new(String::new()),
            last_persisted_notes: RefCell::new(String::new()),
            last_persisted_scratch_pad: RefCell::new(String::new()),
            last_editor_persist_at: RefCell::new(Instant::now()),
            last_sidebar_sync_at: RefCell::new(Instant::now() - Duration::from_secs(5)),
            last_summary_sync_at: RefCell::new(Instant::now() - Duration::from_secs(5)),
            last_sidebar_query: RefCell::new(String::new()),
            last_recent_signature: RefCell::new(Vec::new()),
            sidebar_session_ids: RefCell::new(Vec::new()),
            home_recent_ids: RefCell::new(Vec::new()),
            summary_options: RefCell::new(Vec::new()),
            config: RefCell::new(config.clone()),
            store,
            ambient_controller,
            summary_registry,
        });

        window.sync_config(config);
        window.tick();
        window
    }

    pub fn show(&self) {
        self.window.makeKeyAndOrderFront(None);
        self.window.orderFrontRegardless();
    }

    pub fn show_home(&self) {
        self.route.set(ROUTE_HOME);
        self.apply_route_visibility();
        self.show();
    }

    pub fn show_settings(&self) {
        self.route.set(ROUTE_SETTINGS);
        self.apply_route_visibility();
        self.show();
    }

    pub fn show_session(&self, session_id: i64) {
        self.route.set(ROUTE_SESSION);
        self.current_session_id.set(session_id);
        self.apply_route_visibility();
        self.show();
        self.tick();
    }

    pub fn sync_config(&self, config: &Config) {
        self.config.replace(config.clone());
        self.apply_theme(config.appearance);

        if let Some(index) = MODELS.iter().position(|model| model.id == config.model) {
            self.settings_model_popup.selectItemAtIndex(index as isize);
        }
        if let Some(index) = HOTKEYS.iter().position(|hotkey| hotkey.id == config.hotkey) {
            self.settings_hotkey_popup.selectItemAtIndex(index as isize);
        }
        if let Some(index) = POSITIONS
            .iter()
            .position(|position| position.id == config.overlay_position)
        {
            self.settings_position_popup
                .selectItemAtIndex(index as isize);
        }

        self.settings_appearance_toggle.setSelectedSegment(
            if matches!(config.appearance, AppAppearance::Light) {
                1
            } else {
                0
            },
        );
        self.settings_live_switch
            .setState(if config.live_transcription {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        self.settings_sound_switch
            .setState(if config.sound_effects {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        self.settings_ambient_mic_switch
            .setState(if config.ambient_microphone {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        self.settings_ambient_system_switch
            .setState(if config.ambient_system_audio {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        if let Some(index) = AMBIENT_FINAL_BACKENDS
            .iter()
            .position(|backend| backend.id == config.ambient_final_backend)
        {
            self.settings_ambient_final_popup
                .selectItemAtIndex(index as isize);
        }
        self.sync_summary_popup(config);
    }

    pub fn tick(&self) {
        let config = self.config.borrow().clone();
        self.sync_summary_popup_if_needed(&config);
        self.sync_sidebar_sessions();
        self.sync_home(&config);
        self.sync_session(&config);
        self.persist_editor_if_needed();
    }

    pub fn current_session_id(&self) -> i64 {
        self.current_session_id.get()
    }

    pub fn session_id_for_sidebar_index(&self, index: usize) -> Option<i64> {
        self.sidebar_session_ids.borrow().get(index).copied()
    }

    pub fn session_id_for_home_index(&self, index: usize) -> Option<i64> {
        self.home_recent_ids.borrow().get(index).copied()
    }

    pub fn summary_option_for_index(&self, index: usize) -> Option<SummaryModelOption> {
        self.summary_options.borrow().get(index).cloned()
    }

    pub fn ambient_final_backend_for_index(
        &self,
        index: usize,
    ) -> Option<AmbientFinalBackendPreference> {
        AMBIENT_FINAL_BACKENDS.get(index).map(|backend| backend.id)
    }

    pub fn set_summary_template(&self, index: usize) {
        let templates = SummaryTemplate::all();
        let template = templates.get(index).copied().unwrap_or_default();
        let session_id = self.current_session_id.get();
        if session_id != 0 {
            let _ = self
                .ambient_controller
                .set_summary_template(session_id, template);
        }
    }

    fn sync_sidebar_sessions(&self) {
        let search = self.search_field.stringValue().to_string();
        let trimmed_search = search.trim().to_string();
        let query_changed = *self.last_sidebar_query.borrow() != trimmed_search;
        if !query_changed
            && self.last_sidebar_sync_at.borrow().elapsed() < Duration::from_millis(500)
        {
            return;
        }

        let recent = self
            .store
            .list_recent_sessions(SESSION_LIST_LIMIT, Some(trimmed_search.as_str()))
            .unwrap_or_default();
        let signature = recent
            .iter()
            .map(|session| (session.id, session.updated_at_ms, session.title.clone()))
            .collect::<Vec<_>>();
        if !query_changed && *self.last_recent_signature.borrow() == signature {
            self.last_sidebar_sync_at.replace(Instant::now());
            return;
        }

        let appearance = self.config.borrow().appearance;

        rebuild_session_button_list(
            &self.sidebar_sessions_container,
            &recent,
            &mut self.sidebar_session_ids.borrow_mut(),
            SessionButtonKind::Sidebar,
            self.handler,
            appearance,
        );
        rebuild_session_button_list(
            &self.home_recent_container,
            &recent,
            &mut self.home_recent_ids.borrow_mut(),
            SessionButtonKind::Home,
            self.handler,
            appearance,
        );
        self.last_sidebar_query.replace(trimmed_search);
        self.last_recent_signature.replace(signature);
        self.last_sidebar_sync_at.replace(Instant::now());
    }

    fn sync_home(&self, config: &Config) {
        let active = self.ambient_controller.active_snapshot();
        if let Some(active) = active {
            self.home_banner
                .setStringValue(&NSString::from_str(&format!(
                    "Session live now · {} · {}",
                    format_elapsed(active.elapsed_ms),
                    active.summary_backend_label
                )));
            self.home_primary_button
                .setTitle(&NSString::from_str("Open live"));
        } else if self.ambient_controller.system_audio_runtime_supported() {
            self.home_banner.setStringValue(&NSString::from_str(
                "A fast local workspace for meetings, voice memos, and quick capture.",
            ));
            self.home_primary_button
                .setTitle(&NSString::from_str("Start session"));
        } else {
            self.home_banner.setStringValue(&NSString::from_str(
                self.ambient_controller.system_audio_runtime_reason(),
            ));
            self.home_primary_button
                .setTitle(&NSString::from_str("Start session"));
        }

        self.home_dictation_hint
            .setStringValue(&NSString::from_str(&format!(
                "Hold {} to speak anywhere on your Mac.",
                config.hotkey_label()
            )));
    }

    fn sync_session(&self, _config: &Config) {
        if self.route.get() != ROUTE_SESSION {
            return;
        }

        let session_id = if self.current_session_id.get() != 0 {
            self.current_session_id.get()
        } else if let Some(active) = self.ambient_controller.active_snapshot() {
            active.id
        } else {
            return;
        };

        let Ok(Some(session)) = self.ambient_controller.load_session(session_id) else {
            return;
        };

        let appearance = self.config.borrow().appearance;

        let session_changed = self.loaded_session_id.get() != session.id;
        if session_changed {
            self.loaded_session_id.set(session.id);
            self.last_rendered_segment_count.set(usize::MAX);
            self.last_rendered_segment_signature
                .replace("__invalidated__".to_string());
            self.session_structured_text
                .setString(&NSString::from_str(&session.structured_notes));
            self.last_persisted_notes
                .replace(session.live_notes.clone());
            self.scratch_pad_text
                .setString(&NSString::from_str(&session.scratch_pad));
            self.last_persisted_scratch_pad
                .replace(session.scratch_pad.clone());
        }

        let segment_signature = segment_content_signature(&session.segments);
        if segment_signature != *self.last_rendered_segment_signature.borrow() {
            rebuild_transcript_bubbles(
                &self.transcript_container,
                &self.transcript_scroll,
                &session.segments,
                appearance,
            );
            self.last_rendered_segment_count.set(session.segments.len());
            self.last_rendered_segment_signature.replace(segment_signature);
            self.last_persisted_notes
                .replace(session.live_notes.clone());
        }

        let structured_text = self.session_structured_text.string().to_string();
        if structured_text != session.structured_notes {
            self.session_structured_text
                .setString(&NSString::from_str(&session.structured_notes));
        }

        self.session_title
            .setStringValue(&NSString::from_str(&display_session_title(&session.title)));
        self.session_status
            .setStringValue(&NSString::from_str(status_label(session.state)));
        self.session_timer
            .setStringValue(&NSString::from_str(&format_elapsed(session.elapsed_ms)));
        self.session_inputs
            .setStringValue(&NSString::from_str(&format!(
                "Inputs: {}{}",
                if session.microphone_enabled {
                    "microphone"
                } else {
                    "off"
                },
                if session.system_audio_requested {
                    if session.system_audio_active {
                        " + system output"
                    } else {
                        " + system output (requested)"
                    }
                } else {
                    ""
                }
            )));
        self.session_warning.setStringValue(&NSString::from_str(
            session.warning.as_deref().unwrap_or(""),
        ));
        let is_recording = matches!(
            session.state,
            screamer_core::ambient::AmbientSessionState::Recording
        );
        let is_processing = matches!(
            session.state,
            screamer_core::ambient::AmbientSessionState::Processing
        );
        let is_finished = matches!(
            session.state,
            screamer_core::ambient::AmbientSessionState::Completed
                | screamer_core::ambient::AmbientSessionState::Failed
        );

        // Stop button visible only while recording
        self.session_stop_button.setEnabled(is_recording);
        self.session_stop_button.setHidden(!is_recording);

        // Reprocess button visible only after session is finished
        self.session_reprocess_button.setHidden(!is_finished);
        self.session_reprocess_button.setEnabled(is_finished);

        // Processing overlay covers transcript + scratch pad area
        self.session_processing_overlay.setHidden(!is_processing);

        // Animate the processing label text with a rotating indicator
        if is_processing {
            let tick = (session.elapsed_ms / 400) % 4;
            let dots = match tick {
                0 => "Processing session",
                1 => "Processing session.",
                2 => "Processing session..",
                _ => "Processing session...",
            };
            self.session_processing_label
                .setStringValue(&NSString::from_str(dots));
        }

        // Lock scratch pad during processing (read-only); editable otherwise
        self.scratch_pad_text.setEditable(!is_processing);

        // Status label gets color treatment for processing
        if is_processing {
            self.session_status
                .setTextColor(Some(&theme::processing_accent()));
            self.session_status
                .setStringValue(&NSString::from_str("◉ Processing\u{2026}"));
        } else {
            let appearance = self.config.borrow().appearance;
            self.session_status
                .setTextColor(Some(&theme::secondary_text(appearance)));
        }

        let template_index = SummaryTemplate::all()
            .iter()
            .position(|t| *t == session.summary_template)
            .unwrap_or(0);
        self.session_template_popup
            .selectItemAtIndex(template_index as isize);
        self.session_template_popup.setEnabled(is_recording);
    }

    fn persist_editor_if_needed(&self) {
        if self.route.get() != ROUTE_SESSION || self.current_session_id.get() == 0 {
            return;
        }
        if self.last_editor_persist_at.borrow().elapsed() < Duration::from_millis(700) {
            return;
        }

        let current_scratch = self.scratch_pad_text.string().to_string();
        if current_scratch != *self.last_persisted_scratch_pad.borrow() {
            let _ = self
                .ambient_controller
                .persist_scratch_pad(self.current_session_id.get(), &current_scratch);
            self.last_persisted_scratch_pad.replace(current_scratch);
        }
        self.last_editor_persist_at.replace(Instant::now());
    }

    fn sync_summary_popup_if_needed(&self, config: &Config) {
        if self.route.get() != ROUTE_SETTINGS
            && self.last_summary_sync_at.borrow().elapsed() < Duration::from_secs(2)
        {
            return;
        }
        self.sync_summary_popup(config);
        self.last_summary_sync_at.replace(Instant::now());
    }

    fn sync_summary_popup(&self, config: &Config) {
        let options = self.summary_registry.options(config);
        let current_labels: Vec<String> = self
            .summary_options
            .borrow()
            .iter()
            .map(|option| option.label.clone())
            .collect();
        let next_labels: Vec<String> = options.iter().map(|option| option.label.clone()).collect();
        if current_labels != next_labels {
            unsafe {
                let _: () = msg_send![&*self.settings_summary_popup, removeAllItems];
            }
            for option in &options {
                self.settings_summary_popup
                    .addItemWithTitle(&NSString::from_str(&option.label));
            }
            self.summary_options.replace(options.clone());
        }

        let selected = options
            .iter()
            .position(|option| match config.summary_backend {
                crate::config::SummaryBackendPreference::Bundled => {
                    option.backend == crate::config::SummaryBackendPreference::Bundled
                }
                crate::config::SummaryBackendPreference::Ollama => {
                    option.backend == crate::config::SummaryBackendPreference::Ollama
                        && option.value == config.summary_ollama_model
                }
            })
            .unwrap_or(0);
        self.settings_summary_popup
            .selectItemAtIndex(selected as isize);
    }

    fn apply_route_visibility(&self) {
        self.home_view.setHidden(self.route.get() != ROUTE_HOME);
        self.session_view
            .setHidden(self.route.get() != ROUTE_SESSION);
        self.settings_view
            .setHidden(self.route.get() != ROUTE_SETTINGS);
    }

    fn apply_theme(&self, appearance: AppAppearance) {
        let background = theme::window_background(appearance);
        self.window.setBackgroundColor(Some(&background));
        style_surface(&self.root, &background, &background, 0.0);
        style_surface(
            &self.sidebar,
            &theme::surface_background(appearance),
            &theme::card_border(appearance),
            0.0,
        );
        for view in [&self.home_primary_button, &self.session_stop_button] {
            unsafe {
                let _: () =
                    msg_send![&**view, setContentTintColor: &*theme::title_text(appearance)];
            }
        }
    }
}

#[derive(Clone, Copy)]
enum SessionButtonKind {
    Sidebar,
    Home,
}

fn rebuild_session_button_list(
    container: &NSView,
    sessions: &[SessionSummary],
    ids: &mut Vec<i64>,
    kind: SessionButtonKind,
    handler: *const AnyObject,
    appearance: AppAppearance,
) {
    let subviews = container.subviews();
    for index in (0..subviews.count()).rev() {
        let view = subviews.objectAtIndex(index);
        view.removeFromSuperview();
    }

    ids.clear();
    if sessions.is_empty() {
        let mtm = MainThreadMarker::new().expect("empty-state rebuild should be on main thread");
        match kind {
            SessionButtonKind::Sidebar => {
                let empty = text_label(
                    mtm,
                    "No sessions yet",
                    CGRect::new(
                        CGPoint::new(8.0, container.frame().size.height - 24.0),
                        CGSize::new(container.frame().size.width - 16.0, 18.0),
                    ),
                    12.0,
                    &theme::secondary_text(appearance),
                    false,
                );
                container.addSubview(&empty);
            }
            SessionButtonKind::Home => {
                let card = surface_view(
                    mtm,
                    CGRect::new(
                        CGPoint::new(0.0, container.frame().size.height - 124.0),
                        CGSize::new(container.frame().size.width, 96.0),
                    ),
                    &theme::surface_background(appearance),
                    &theme::card_border(appearance),
                    20.0,
                );
                let title = text_label(
                    mtm,
                    "No recent sessions yet",
                    CGRect::new(CGPoint::new(24.0, 54.0), CGSize::new(280.0, 22.0)),
                    17.0,
                    &theme::title_text(appearance),
                    true,
                );
                let body = text_label(
                    mtm,
                    "Finish a recording and the cleaned-up recap will show up here.",
                    CGRect::new(
                        CGPoint::new(24.0, 30.0),
                        CGSize::new(container.frame().size.width - 48.0, 18.0),
                    ),
                    12.0,
                    &theme::secondary_text(appearance),
                    false,
                );
                card.addSubview(&title);
                card.addSubview(&body);
                container.addSubview(&card);
            }
        }
        return;
    }

    for (index, session) in sessions.iter().enumerate() {
        ids.push(session.id);
        let frame = match kind {
            SessionButtonKind::Sidebar => CGRect::new(
                CGPoint::new(
                    0.0,
                    container.frame().size.height - 58.0 - index as f64 * 62.0,
                ),
                CGSize::new(container.frame().size.width, 50.0),
            ),
            SessionButtonKind::Home => CGRect::new(
                CGPoint::new(
                    0.0,
                    container.frame().size.height - 86.0 - index as f64 * 92.0,
                ),
                CGSize::new(container.frame().size.width, 78.0),
            ),
        };
        let mtm = MainThreadMarker::new().expect("button rebuild should be on main thread");
        let card = surface_view(
            mtm,
            frame,
            &theme::surface_background(appearance),
            &theme::card_border(appearance),
            match kind {
                SessionButtonKind::Sidebar => 16.0,
                SessionButtonKind::Home => 20.0,
            },
        );
        container.addSubview(&card);

        let accent_color = match session.state {
            screamer_core::ambient::AmbientSessionState::Recording => theme::brand_gold(),
            screamer_core::ambient::AmbientSessionState::Processing => theme::processing_accent(),
            screamer_core::ambient::AmbientSessionState::Completed => {
                theme::completed_accent(appearance)
            }
            screamer_core::ambient::AmbientSessionState::Failed => theme::failed_accent(),
            _ => theme::brand_gold(),
        };
        let accent = surface_view(
            mtm,
            CGRect::new(
                CGPoint::new(14.0, frame.size.height - 20.0),
                CGSize::new(8.0, 8.0),
            ),
            &accent_color,
            &accent_color,
            4.0,
        );
        card.addSubview(&accent);

        let button = unsafe {
            let selector = match kind {
                SessionButtonKind::Sidebar => sel!(openSessionFromSidebar:),
                SessionButtonKind::Home => sel!(openSessionFromHome:),
            };
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(""),
                Some(&*handler),
                Some(selector),
                mtm,
            )
        };
        button.setFrame(frame);
        button.setTag(index as isize);
        button.setButtonType(NSButtonType::MomentaryPushIn);
        button.setBezelStyle(objc2_app_kit::NSBezelStyle::ShadowlessSquare);
        button.setBordered(false);
        button.setTitle(&NSString::from_str(""));
        let tooltip = NSString::from_str(&session.title);
        unsafe {
            let _: () = msg_send![&*button, setToolTip: Some(&*tooltip)];
        }
        container.addSubview(&button);

        match kind {
            SessionButtonKind::Sidebar => {
                let title = text_label(
                    mtm,
                    &compact_sidebar_text(&display_session_title(&session.title), 28),
                    CGRect::new(
                        CGPoint::new(30.0, 24.0),
                        CGSize::new(frame.size.width - 42.0, 16.0),
                    ),
                    13.0,
                    &theme::title_text(appearance),
                    true,
                );
                let subtitle = text_label(
                    mtm,
                    &session_meta_line(session),
                    CGRect::new(
                        CGPoint::new(30.0, 8.0),
                        CGSize::new(frame.size.width - 42.0, 14.0),
                    ),
                    11.5,
                    &theme::secondary_text(appearance),
                    false,
                );
                card.addSubview(&title);
                card.addSubview(&subtitle);
            }
            SessionButtonKind::Home => {
                let title = text_label(
                    mtm,
                    &display_session_title(&session.title),
                    CGRect::new(
                        CGPoint::new(20.0, 48.0),
                        CGSize::new(frame.size.width - 40.0, 18.0),
                    ),
                    16.0,
                    &theme::title_text(appearance),
                    true,
                );
                let preview = text_label(
                    mtm,
                    &session_preview_line(session),
                    CGRect::new(
                        CGPoint::new(20.0, 28.0),
                        CGSize::new(frame.size.width - 40.0, 16.0),
                    ),
                    12.5,
                    &theme::secondary_text(appearance),
                    false,
                );
                let meta = text_label(
                    mtm,
                    &session_meta_line(session),
                    CGRect::new(
                        CGPoint::new(20.0, 10.0),
                        CGSize::new(frame.size.width - 40.0, 14.0),
                    ),
                    11.5,
                    &theme::secondary_text(appearance),
                    false,
                );
                card.addSubview(&title);
                card.addSubview(&preview);
                card.addSubview(&meta);
            }
        }
    }
}

fn content_bounds() -> CGRect {
    CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(WINDOW_WIDTH - SIDEBAR_WIDTH, WINDOW_HEIGHT),
    )
}

fn editor_scroll_view(
    mtm: MainThreadMarker,
    frame: CGRect,
    appearance: AppAppearance,
    editable: bool,
) -> (Retained<NSScrollView>, Retained<NSTextView>) {
    let scroll = NSTextView::scrollableTextView(mtm);
    scroll.setFrame(frame);
    scroll.setBorderType(NSBorderType::NoBorder);
    scroll.setHasVerticalScroller(true);

    let document = scroll
        .documentView()
        .expect("scrollable text view should have document view");
    let text_view = document
        .downcast::<NSTextView>()
        .expect("scrollable text view document should be an NSTextView");
    text_view.setEditable(editable);
    text_view.setSelectable(true);
    text_view.setRichText(false);
    text_view.setDrawsBackground(true);
    text_view.setBackgroundColor(&theme::surface_background(appearance));
    text_view.setFont(Some(&NSFont::systemFontOfSize(14.0)));
    text_view.setString(&NSString::from_str(""));
    text_view.setTextContainerInset(CGSize::new(12.0, 14.0));

    style_surface(
        &scroll,
        &theme::surface_background(appearance),
        &theme::card_border(appearance),
        18.0,
    );

    (scroll, text_view)
}

fn add_settings_row(
    mtm: MainThreadMarker,
    parent: &NSView,
    appearance: AppAppearance,
    label: &str,
    frame: CGRect,
    control: &NSView,
) {
    let row = surface_view(
        mtm,
        frame,
        &theme::surface_background(appearance),
        &theme::card_border(appearance),
        16.0,
    );
    let text = text_label(
        mtm,
        label,
        CGRect::new(
            CGPoint::new(18.0, (frame.size.height - 22.0) / 2.0),
            CGSize::new(260.0, 22.0),
        ),
        14.0,
        &theme::title_text(appearance),
        true,
    );
    row.addSubview(&text);
    row.addSubview(control);
    parent.addSubview(&row);
}

fn sidebar_button(
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
    button.setBezelStyle(objc2_app_kit::NSBezelStyle::Rounded);
    button
}

fn primary_button(
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
    button.setBezelStyle(objc2_app_kit::NSBezelStyle::Rounded);
    button
}

fn popup_button(
    mtm: MainThreadMarker,
    frame: CGRect,
    handler: *const AnyObject,
    action: objc2::runtime::Sel,
) -> Retained<NSPopUpButton> {
    let popup = NSPopUpButton::initWithFrame_pullsDown(mtm.alloc::<NSPopUpButton>(), frame, false);
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
    toggle.setLabel_forSegment(&NSString::from_str("Dark"), 0);
    toggle.setLabel_forSegment(&NSString::from_str("Light"), 1);
    unsafe {
        toggle.setTarget(Some(&*handler));
        toggle.setAction(Some(sel!(setAppearanceMode:)));
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
    button.setBezelStyle(objc2_app_kit::NSBezelStyle::Rounded);
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
        CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(110.0, 30.0)),
        "Microphone",
        handler,
        sel!(openMicrophoneSettings:),
    );
    container.addSubview(&microphone_button);

    let accessibility_button = action_button(
        mtm,
        CGRect::new(CGPoint::new(122.0, 0.0), CGSize::new(110.0, 30.0)),
        "Accessibility",
        handler,
        sel!(openAccessibilitySettings:),
    );
    container.addSubview(&accessibility_button);

    container
}

fn badge_view(
    mtm: MainThreadMarker,
    label: &str,
    frame: CGRect,
    appearance: AppAppearance,
) -> Retained<NSView> {
    let badge = surface_view(
        mtm,
        frame,
        &theme::window_background(appearance),
        &theme::card_border(appearance),
        frame.size.height / 2.0,
    );
    let text = text_label(
        mtm,
        label,
        CGRect::new(
            CGPoint::new(12.0, (frame.size.height - 16.0) / 2.0),
            CGSize::new(frame.size.width - 24.0, 16.0),
        ),
        11.5,
        &theme::secondary_text(appearance),
        true,
    );
    text.setAlignment(NSTextAlignment::Center);
    badge.addSubview(&text);
    badge
}

fn text_field(
    mtm: MainThreadMarker,
    frame: CGRect,
    value: &str,
    placeholder: &str,
    appearance: AppAppearance,
) -> Retained<NSTextField> {
    let field = NSTextField::initWithFrame(mtm.alloc::<NSTextField>(), frame);
    field.setStringValue(&NSString::from_str(value));
    field.setPlaceholderString(Some(&NSString::from_str(placeholder)));
    field.setTextColor(Some(&theme::title_text(appearance)));
    field.setBackgroundColor(Some(&theme::surface_background(appearance)));
    field.setBezeled(true);
    field
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
    label.setAlignment(NSTextAlignment::Left);
    let font = if bold {
        NSFont::boldSystemFontOfSize(font_size)
    } else {
        NSFont::systemFontOfSize(font_size)
    };
    label.setFont(Some(&font));
    label.setMaximumNumberOfLines(1);
    unsafe {
        let _: () = msg_send![&*label, setUsesSingleLineMode: true];
        let _: () = msg_send![&*label, setLineBreakMode: 4usize];
    }
    label
}

fn wrapped_text_label(
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
    label.setAlignment(NSTextAlignment::Left);
    let font = if bold {
        NSFont::boldSystemFontOfSize(font_size)
    } else {
        NSFont::systemFontOfSize(font_size)
    };
    label.setFont(Some(&font));
    label.setMaximumNumberOfLines(0);
    label.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    label
}

fn compact_sidebar_text(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut result = String::new();
    for ch in trimmed.chars() {
        if result.chars().count() >= max_chars {
            break;
        }
        result.push(ch);
    }
    result.trim().to_string()
}

fn display_session_title(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed
        .strip_prefix("You:")
        .or_else(|| collapsed.strip_prefix("S1:"))
        .or_else(|| collapsed.strip_prefix("S2:"))
        .unwrap_or(&collapsed)
        .trim();
    if trimmed.is_empty() {
        "Ambient Session".to_string()
    } else {
        trimmed.to_string()
    }
}

fn session_preview_line(session: &SessionSummary) -> String {
    let preview = compact_sidebar_text(&session.live_notes_preview, 120);
    if preview.is_empty() || preview.eq_ignore_ascii_case("no notes yet") {
        "No notes yet".to_string()
    } else {
        preview
    }
}

fn session_meta_line(session: &SessionSummary) -> String {
    match session.state {
        screamer_core::ambient::AmbientSessionState::Processing => {
            "◉ Processing\u{2026}".to_string()
        }
        _ => format!(
            "{} · {}",
            status_label(session.state),
            format_relative_time(session.updated_at_ms)
        ),
    }
}

fn format_relative_time(timestamp_ms: i64) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(timestamp_ms);
    let delta_seconds = ((now_ms - timestamp_ms).max(0) / 1_000) as u64;

    match delta_seconds {
        0..=59 => "Just now".to_string(),
        60..=3_599 => format!("{}m ago", delta_seconds / 60),
        3_600..=86_399 => format!("{}h ago", delta_seconds / 3_600),
        _ => format!("{}d ago", delta_seconds / 86_400),
    }
}

fn format_elapsed(elapsed_ms: u64) -> String {
    let total_seconds = elapsed_ms / 1_000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}

fn status_label(state: screamer_core::ambient::AmbientSessionState) -> &'static str {
    match state {
        screamer_core::ambient::AmbientSessionState::Idle => "Idle",
        screamer_core::ambient::AmbientSessionState::Recording => "Recording",
        screamer_core::ambient::AmbientSessionState::Processing => "Processing",
        screamer_core::ambient::AmbientSessionState::Completed => "Completed",
        screamer_core::ambient::AmbientSessionState::Failed => "Failed",
    }
}

fn rebuild_transcript_bubbles(
    container: &NSView,
    scroll: &NSScrollView,
    segments: &[CanonicalSegment],
    appearance: AppAppearance,
) {
    let mtm = MainThreadMarker::from(container);

    let subviews: Vec<_> = container.subviews().iter().collect();
    for view in subviews {
        view.removeFromSuperview();
    }

    if segments.is_empty() {
        return;
    }

    let container_width = container.frame().size.width;
    let padding: f64 = 14.0;
    let bubble_spacing: f64 = 6.0;
    let max_bubble_width: f64 = container_width * 0.78;
    let inner_padding: f64 = 10.0;
    let speaker_height: f64 = 16.0;
    let speaker_gap: f64 = 2.0;

    let font = NSFont::systemFontOfSize(13.0);
    let speaker_font = NSFont::boldSystemFontOfSize(11.0);

    struct BubbleLayout {
        height: f64,
        text_height: f64,
    }

    let mut layouts: Vec<BubbleLayout> = Vec::with_capacity(segments.len());
    for segment in segments {
        let text = segment.text.trim();
        let estimated_chars_per_line = ((max_bubble_width - inner_padding * 2.0) / 7.5) as usize;
        let line_count = if estimated_chars_per_line == 0 {
            1
        } else {
            (text.len() / estimated_chars_per_line).max(1)
        };
        let text_height = line_count as f64 * 18.0;
        let bubble_height = inner_padding * 2.0 + speaker_height + speaker_gap + text_height;
        layouts.push(BubbleLayout {
            height: bubble_height,
            text_height,
        });
    }

    let total_height: f64 = layouts.iter().map(|l| l.height).sum::<f64>()
        + bubble_spacing * (segments.len() as f64 - 1.0)
        + padding * 2.0;

    let view_height = total_height.max(container.frame().size.height);
    container.setFrame(CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(container_width, view_height),
    ));

    let mut y_cursor = view_height - padding;

    for (i, segment) in segments.iter().enumerate() {
        let layout = &layouts[i];
        let speaker_idx = segment.speaker.index();

        let bubble_bg = theme::bubble_for_speaker(speaker_idx, appearance);
        let speaker_color = theme::speaker_color_for_index(speaker_idx, appearance);

        let bubble_x = if speaker_idx == 0 {
            container_width - padding - max_bubble_width
        } else {
            padding
        };

        y_cursor -= layout.height;
        let bubble = surface_view(
            mtm,
            CGRect::new(
                CGPoint::new(bubble_x, y_cursor),
                CGSize::new(max_bubble_width, layout.height),
            ),
            &bubble_bg,
            &bubble_bg,
            14.0,
        );

        let speaker = text_label(
            mtm,
            segment.speaker.display_name(),
            CGRect::new(
                CGPoint::new(
                    inner_padding,
                    layout.height - inner_padding - speaker_height,
                ),
                CGSize::new(max_bubble_width - inner_padding * 2.0, speaker_height),
            ),
            11.0,
            &speaker_color,
            true,
        );
        speaker.setFont(Some(&speaker_font));
        bubble.addSubview(&speaker);

        let text_label_view = wrapped_text_label(
            mtm,
            segment.text.trim(),
            CGRect::new(
                CGPoint::new(inner_padding, inner_padding),
                CGSize::new(max_bubble_width - inner_padding * 2.0, layout.text_height),
            ),
            13.0,
            &theme::body_text(appearance),
            false,
        );
        text_label_view.setFont(Some(&font));
        text_label_view.setSelectable(true);
        bubble.addSubview(&text_label_view);

        container.addSubview(&bubble);
        y_cursor -= bubble_spacing;
    }

    unsafe {
        let clip_view = scroll.contentView();
        let max_y = (view_height - scroll.frame().size.height).max(0.0);
        let _: () = msg_send![&*clip_view, scrollToPoint: CGPoint::new(0.0, max_y)];
        scroll.reflectScrolledClipView(&clip_view);
    }
}

fn segment_content_signature(segments: &[CanonicalSegment]) -> String {
    use std::fmt::Write;
    let mut sig = String::with_capacity(segments.len() * 20);
    for s in segments {
        let _ = write!(sig, "{}:{}:{};", s.id, s.speaker.index(), s.text.len());
    }
    sig
}

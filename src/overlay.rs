use crate::config::{AppAppearance, OverlayPosition};
use crate::theme;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBackingStoreType, NSBorderType, NSColor, NSFont, NSLineBreakMode, NSPanel, NSScreen,
    NSScrollView, NSTextAlignment, NSTextField, NSTextView, NSView, NSVisualEffectMaterial,
    NSVisualEffectView, NSWindowStyleMask,
};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation::{MainThreadMarker, NSString};

pub const WAVEFORM_BINS: usize = 64;

const WINDOW_WIDTH: f64 = 380.0;
const WINDOW_HEIGHT: f64 = 124.0;
const NUM_BARS: usize = WAVEFORM_BINS;
const BAR_WIDTH: f64 = 2.3;
const BAR_SPACING: f64 = 2.6;
const BAR_MIN_HEIGHT: f64 = 2.0;
const CORNER_RADIUS: f64 = 18.0;
const PADDING_X: f64 = 16.0;
const PADDING_Y: f64 = 12.0;
const SECTION_SPACING: f64 = 8.0;
const TRANSCRIPT_HEIGHT: f64 = 36.0;
const WAVEFORM_HEIGHT: f64 =
    WINDOW_HEIGHT - (PADDING_Y * 2.0) - TRANSCRIPT_HEIGHT - SECTION_SPACING;
const BAR_MAX_HEIGHT: f64 = WAVEFORM_HEIGHT;

const POSITION_MARGIN: f64 = 40.0;

// Vision response area
const VISION_DIVIDER_HEIGHT: f64 = 1.0;
const VISION_RESPONSE_HEIGHT: f64 = 180.0;
const VISION_EXTRA_HEIGHT: f64 =
    SECTION_SPACING + VISION_DIVIDER_HEIGHT + SECTION_SPACING + VISION_RESPONSE_HEIGHT;

/// The current state of the vision response area.
#[derive(Clone, Debug, PartialEq)]
pub enum VisionOverlayState {
    /// Not in vision mode — normal dictation overlay.
    Hidden,
    /// Vision model is processing (show spinner/loading text).
    Loading,
    /// Vision model returned a response, with an optional highlight region.
    Response(String, Option<crate::highlight::HighlightTarget>),
    /// Vision model errored.
    Error(String),
}

pub struct Overlay {
    panel: Retained<NSPanel>,
    effect_view: Retained<NSVisualEffectView>,
    bar_views: Vec<Retained<NSView>>,
    transcript_label: Retained<NSTextField>,
    // Vision UI elements
    vision_divider: Retained<NSView>,
    vision_scroll: Retained<NSScrollView>,
    vision_text: Retained<NSTextView>,
    current_heights: [f64; NUM_BARS],
    current_transcript: String,
    visible: bool,
    position: OverlayPosition,
    appearance: AppAppearance,
    vision_state: VisionOverlayState,
}

fn waveform_level_for_bar(bar_idx: usize, waveform: &[f32]) -> f64 {
    if waveform.is_empty() {
        return 0.0;
    }

    let sample_idx = bar_idx * waveform.len() / NUM_BARS;
    waveform[sample_idx.min(waveform.len() - 1)].clamp(0.0, 1.0) as f64
}

fn smooth_height(current: f64, target: f64) -> f64 {
    let smoothing = if target > current { 0.62 } else { 0.36 };
    let next = current + (target - current) * smoothing;
    next.clamp(BAR_MIN_HEIGHT, BAR_MAX_HEIGHT)
}

impl Overlay {
    pub fn new(
        mtm: MainThreadMarker,
        position: OverlayPosition,
        appearance: AppAppearance,
    ) -> Self {
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;

        let frame = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        );

        let panel = {
            let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<NSPanel>(),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            );
            panel.setLevel(25);
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setHasShadow(true);
            panel.setMovableByWindowBackground(false);
            panel.setHidesOnDeactivate(false);
            panel.setAlphaValue(0.0);
            panel.setCollectionBehavior(
                objc2_app_kit::NSWindowCollectionBehavior::CanJoinAllSpaces
                    | objc2_app_kit::NSWindowCollectionBehavior::Stationary
                    | objc2_app_kit::NSWindowCollectionBehavior::IgnoresCycle,
            );
            panel
        };

        // Frosted glass background
        let effect_view = {
            let view = NSVisualEffectView::new(mtm);
            view.setFrame(frame);
            view.setMaterial(NSVisualEffectMaterial::HUDWindow);
            view.setBlendingMode(objc2_app_kit::NSVisualEffectBlendingMode::BehindWindow);
            view.setState(objc2_app_kit::NSVisualEffectState::Active);
            view.setWantsLayer(true);
            if let Some(layer) = view.layer() {
                layer.setCornerRadius(CORNER_RADIUS as CGFloat);
                layer.setMasksToBounds(true);
            }
            view
        };

        let usable_width = WINDOW_WIDTH - PADDING_X * 2.0;
        let total_bars_width = NUM_BARS as f64 * BAR_WIDTH + (NUM_BARS - 1) as f64 * BAR_SPACING;
        let bars_x_offset = PADDING_X + (usable_width - total_bars_width) / 2.0;

        let transcript_label = {
            let label = NSTextField::labelWithString(&NSString::from_str(""), mtm);
            label.setFrame(Self::transcript_frame(position));
            label.setDrawsBackground(false);
            label.setBordered(false);
            label.setBezeled(false);
            label.setEditable(false);
            label.setSelectable(false);
            label.setAlignment(NSTextAlignment(2));
            label.setMaximumNumberOfLines(2);
            label.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
            label.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(12.5)));
            label
        };

        // Vision divider (thin horizontal line)
        let vision_divider = {
            let view = NSView::new(mtm);
            view.setFrame(CGRect::new(
                CGPoint::new(PADDING_X, 0.0),
                CGSize::new(WINDOW_WIDTH - PADDING_X * 2.0, VISION_DIVIDER_HEIGHT),
            ));
            view.setWantsLayer(true);
            view.setHidden(true);
            view
        };

        // Vision response — scrollable text view
        let (vision_scroll, vision_text) = {
            let scroll_frame = CGRect::new(
                CGPoint::new(PADDING_X, PADDING_Y),
                CGSize::new(WINDOW_WIDTH - PADDING_X * 2.0, VISION_RESPONSE_HEIGHT),
            );
            let scroll = NSTextView::scrollableTextView(mtm);
            scroll.setFrame(scroll_frame);
            scroll.setBorderType(NSBorderType::NoBorder);
            scroll.setHasVerticalScroller(true);
            scroll.setDrawsBackground(false);
            scroll.setHidden(true);

            let document = scroll
                .documentView()
                .expect("scrollable text view should have document view");
            let text_view = document
                .downcast::<NSTextView>()
                .expect("scrollable text view document should be an NSTextView");
            text_view.setEditable(false);
            text_view.setSelectable(true);
            text_view.setRichText(false);
            text_view.setDrawsBackground(false);
            text_view.setFont(Some(&NSFont::systemFontOfSize(11.5)));
            text_view.setString(&NSString::from_str(""));
            text_view.setTextContainerInset(CGSize::new(0.0, 4.0));

            (scroll, text_view)
        };

        let mut bar_views = Vec::with_capacity(NUM_BARS);
        for i in 0..NUM_BARS {
            let x = bars_x_offset + i as f64 * (BAR_WIDTH + BAR_SPACING);
            let bar = {
                let view = NSView::new(mtm);
                let waveform_origin_y = Self::waveform_origin_y(position);
                let y = waveform_origin_y + (WAVEFORM_HEIGHT - BAR_MIN_HEIGHT) / 2.0;
                view.setFrame(CGRect::new(
                    CGPoint::new(x, y),
                    CGSize::new(BAR_WIDTH, BAR_MIN_HEIGHT),
                ));
                view.setWantsLayer(true);
                if let Some(layer) = view.layer() {
                    layer.setCornerRadius((BAR_WIDTH / 2.0) as CGFloat);
                }
                view
            };
            bar_views.push(bar);
        }

        {
            let content_view = panel.contentView().unwrap();
            content_view.addSubview(&effect_view);
            effect_view.addSubview(&transcript_label);
            effect_view.addSubview(&vision_divider);
            effect_view.addSubview(&vision_scroll);
            for bar in &bar_views {
                effect_view.addSubview(bar);
            }
        }

        let s = Self {
            panel,
            effect_view,
            bar_views,
            transcript_label,
            vision_divider,
            vision_scroll,
            vision_text,
            current_heights: [BAR_MIN_HEIGHT; NUM_BARS],
            current_transcript: String::new(),
            visible: false,
            position,
            appearance,
            vision_state: VisionOverlayState::Hidden,
        };
        s.apply_theme();
        s.apply_position(mtm);
        s
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.current_heights = [BAR_MIN_HEIGHT; NUM_BARS];
        self.panel.orderFrontRegardless();
        self.panel.setAlphaValue(1.0);
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.current_heights = [BAR_MIN_HEIGHT; NUM_BARS];
        self.current_transcript.clear();
        self.transcript_label
            .setStringValue(&NSString::from_str(""));
        self.set_vision_state(VisionOverlayState::Hidden);
        self.panel.setAlphaValue(0.0);
        self.panel.orderOut(None);
    }

    pub fn update_waveform(&mut self, waveform: &[f32]) {
        if !self.visible {
            return;
        }

        for bar_idx in 0..NUM_BARS {
            let level = waveform_level_for_bar(bar_idx, waveform);
            let target = BAR_MIN_HEIGHT + level * (BAR_MAX_HEIGHT - BAR_MIN_HEIGHT);
            self.current_heights[bar_idx] = smooth_height(self.current_heights[bar_idx], target);

            let h = self.current_heights[bar_idx];
            let y = Self::waveform_origin_y(self.position) + (WAVEFORM_HEIGHT - h) / 2.0;

            let mut frame: CGRect = self.bar_views[bar_idx].frame();
            frame.origin.y = y as CGFloat;
            frame.size.height = h as CGFloat;
            self.bar_views[bar_idx].setFrame(frame);
        }
    }

    pub fn update_transcript(&mut self, transcript: &str) {
        if !self.visible || self.current_transcript == transcript {
            return;
        }

        self.current_transcript.clear();
        self.current_transcript.push_str(transcript);
        self.transcript_label
            .setStringValue(&NSString::from_str(transcript));
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Update the vision response area. Call from the waveform timer.
    pub fn set_vision_state(&mut self, state: VisionOverlayState) {
        if self.vision_state == state {
            return;
        }
        self.vision_state = state;
        self.apply_vision_layout();
    }

    pub fn set_position(&mut self, mtm: MainThreadMarker, position: OverlayPosition) {
        self.position = position;
        self.layout_content();
        self.apply_position(mtm);
    }

    pub fn set_appearance(&mut self, appearance: AppAppearance) {
        self.appearance = appearance;
        self.apply_theme();
    }

    fn is_vision_expanded(&self) -> bool {
        !matches!(self.vision_state, VisionOverlayState::Hidden)
    }

    fn current_window_height(&self) -> f64 {
        if self.is_vision_expanded() {
            WINDOW_HEIGHT + VISION_EXTRA_HEIGHT
        } else {
            WINDOW_HEIGHT
        }
    }

    fn apply_vision_layout(&mut self) {
        let expanded = self.is_vision_expanded();
        let h = self.current_window_height();

        // Resize panel and effect view
        let mut panel_frame = self.panel.frame();
        // Adjust origin.y so the top stays in place (macOS coordinates are bottom-up)
        let height_diff = h - panel_frame.size.height;
        panel_frame.origin.y -= height_diff;
        panel_frame.size.height = h;
        self.panel.setFrame_display(panel_frame, true);

        self.effect_view.setFrame(CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(WINDOW_WIDTH, h),
        ));

        // Position divider and response scroll at the bottom of the expanded area
        if expanded {
            let divider_y = PADDING_Y + VISION_RESPONSE_HEIGHT + SECTION_SPACING;
            self.vision_divider.setFrame(CGRect::new(
                CGPoint::new(PADDING_X, divider_y),
                CGSize::new(WINDOW_WIDTH - PADDING_X * 2.0, VISION_DIVIDER_HEIGHT),
            ));
            self.vision_scroll.setFrame(CGRect::new(
                CGPoint::new(PADDING_X, PADDING_Y),
                CGSize::new(WINDOW_WIDTH - PADDING_X * 2.0, VISION_RESPONSE_HEIGHT),
            ));

            self.vision_divider.setHidden(false);
            self.vision_scroll.setHidden(false);

            let (display, color) = match &self.vision_state {
                VisionOverlayState::Loading => (
                    "⏳ Thinking about your screen...".to_string(),
                    theme::overlay_vision_loading_text(self.appearance),
                ),
                VisionOverlayState::Response(text, _) => (
                    format!("✦ {text}"),
                    theme::overlay_vision_label_text(self.appearance),
                ),
                VisionOverlayState::Error(err) => (
                    format!("⚠ {err}"),
                    NSColor::colorWithRed_green_blue_alpha(1.0, 0.4, 0.3, 0.9),
                ),
                VisionOverlayState::Hidden => unreachable!(),
            };

            let ns_display = NSString::from_str(&display);
            self.vision_text.setString(&ns_display);
            self.vision_text.setTextColor_range(
                Some(&color),
                objc2_foundation::NSRange::new(0, ns_display.length()),
            );

            // Apply divider color
            if let Some(layer) = self.vision_divider.layer() {
                let border_color = theme::overlay_panel_border(self.appearance);
                unsafe {
                    let cg: *const std::ffi::c_void = msg_send![&border_color, CGColor];
                    let _: () = msg_send![&*layer, setBackgroundColor: cg];
                }
            }
        } else {
            self.vision_divider.setHidden(true);
            self.vision_scroll.setHidden(true);
            self.vision_text.setString(&NSString::from_str(""));
        }

        // Re-layout the waveform and transcript for the new height
        self.layout_content();
    }

    fn apply_position(&self, mtm: MainThreadMarker) {
        if let Some(screen) = NSScreen::mainScreen(mtm) {
            let sf = screen.frame();
            let h = self.current_window_height();
            let x = (sf.size.width - WINDOW_WIDTH) / 2.0;
            let y = match self.position {
                OverlayPosition::Center => (sf.size.height - h) / 2.0,
                OverlayPosition::Top => sf.size.height - h - POSITION_MARGIN,
                OverlayPosition::Bottom => POSITION_MARGIN,
            };
            self.panel.setFrameOrigin(CGPoint::new(x, y));
        }
    }

    fn layout_content(&mut self) {
        let h = self.current_window_height();
        // Transcript and waveform are at the top portion (above the vision area)
        let vision_offset = if self.is_vision_expanded() {
            VISION_EXTRA_HEIGHT
        } else {
            0.0
        };

        // Transcript frame — sits at the top of the base window area
        let transcript_y = if Self::transcript_above_waveform(self.position) {
            vision_offset + WINDOW_HEIGHT - PADDING_Y - TRANSCRIPT_HEIGHT
        } else {
            vision_offset + PADDING_Y
        };
        self.transcript_label.setFrame(CGRect::new(
            CGPoint::new(PADDING_X, transcript_y),
            CGSize::new(WINDOW_WIDTH - PADDING_X * 2.0, TRANSCRIPT_HEIGHT),
        ));

        let waveform_origin_y = if Self::transcript_above_waveform(self.position) {
            vision_offset + PADDING_Y
        } else {
            vision_offset + PADDING_Y + TRANSCRIPT_HEIGHT + SECTION_SPACING
        };
        for (index, bar) in self.bar_views.iter().enumerate() {
            let x = Self::bars_x_offset() + index as f64 * (BAR_WIDTH + BAR_SPACING);
            let h = self.current_heights[index];
            let y = waveform_origin_y + (WAVEFORM_HEIGHT - h) / 2.0;
            bar.setFrame(CGRect::new(CGPoint::new(x, y), CGSize::new(BAR_WIDTH, h)));
        }
    }

    fn bars_x_offset() -> f64 {
        let usable_width = WINDOW_WIDTH - PADDING_X * 2.0;
        let total_bars_width = NUM_BARS as f64 * BAR_WIDTH + (NUM_BARS - 1) as f64 * BAR_SPACING;
        PADDING_X + (usable_width - total_bars_width) / 2.0
    }

    fn transcript_frame(position: OverlayPosition) -> CGRect {
        let y = if Self::transcript_above_waveform(position) {
            WINDOW_HEIGHT - PADDING_Y - TRANSCRIPT_HEIGHT
        } else {
            PADDING_Y
        };

        CGRect::new(
            CGPoint::new(PADDING_X, y),
            CGSize::new(WINDOW_WIDTH - PADDING_X * 2.0, TRANSCRIPT_HEIGHT),
        )
    }

    fn waveform_origin_y(position: OverlayPosition) -> f64 {
        if Self::transcript_above_waveform(position) {
            PADDING_Y
        } else {
            PADDING_Y + TRANSCRIPT_HEIGHT + SECTION_SPACING
        }
    }

    fn transcript_above_waveform(position: OverlayPosition) -> bool {
        !matches!(position, OverlayPosition::Top)
    }

    fn apply_theme(&self) {
        let transcript_color = theme::overlay_transcript_text(self.appearance);
        self.transcript_label.setTextColor(Some(&transcript_color));

        if let Some(layer) = self.effect_view.layer() {
            layer.setBorderWidth(1.0);
            unsafe {
                let bg = theme::overlay_panel_fill(self.appearance);
                let bg_color: *const std::ffi::c_void = msg_send![&bg, CGColor];
                let border = theme::overlay_panel_border(self.appearance);
                let border_color: *const std::ffi::c_void = msg_send![&border, CGColor];
                let _: () = msg_send![&*layer, setBackgroundColor: bg_color];
                let _: () = msg_send![&*layer, setBorderColor: border_color];
            }
        }

        for (index, bar) in self.bar_views.iter().enumerate() {
            if let Some(layer) = bar.layer() {
                let dist_from_center = ((index as f64) - (NUM_BARS - 1) as f64 / 2.0).abs()
                    / ((NUM_BARS - 1) as f64 / 2.0);
                let glow = 1.0 - dist_from_center * 0.45;
                let ns_color = theme::overlay_bar_color(self.appearance, glow);
                unsafe {
                    let cg_color: *const std::ffi::c_void = msg_send![&ns_color, CGColor];
                    let _: () = msg_send![&*layer, setBackgroundColor: cg_color];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoothing_moves_toward_target() {
        let next = smooth_height(BAR_MIN_HEIGHT, BAR_MAX_HEIGHT);
        assert!(next > BAR_MIN_HEIGHT);
    }

    #[test]
    fn smoothing_flattens_toward_silence() {
        let next = smooth_height(BAR_MAX_HEIGHT, BAR_MIN_HEIGHT);
        assert!(next < BAR_MAX_HEIGHT);
    }

    #[test]
    fn waveform_mapping_preserves_left_to_right_order() {
        let waveform = [0.0, 0.2, 0.4, 1.0];
        assert_eq!(waveform_level_for_bar(0, &waveform), 0.0);
        assert!(waveform_level_for_bar(NUM_BARS - 1, &waveform) > 0.9);
    }
}

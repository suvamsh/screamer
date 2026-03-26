use crate::config::OverlayPosition;
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSScreen, NSView, NSVisualEffectMaterial,
    NSVisualEffectView, NSWindowStyleMask,
};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation::MainThreadMarker;

const WINDOW_WIDTH: f64 = 340.0;
const WINDOW_HEIGHT: f64 = 80.0;
const NUM_BARS: usize = 56;
const HALF_BARS: usize = NUM_BARS / 2;
const BAR_WIDTH: f64 = 3.0;
const BAR_SPACING: f64 = 2.5;
const BAR_MIN_HEIGHT: f64 = 5.0;
const CORNER_RADIUS: f64 = 18.0;
const PADDING_X: f64 = 14.0;
const PADDING_Y: f64 = 8.0;
const BAR_MAX_HEIGHT: f64 = WINDOW_HEIGHT - PADDING_Y * 2.0;

const POSITION_MARGIN: f64 = 40.0;

pub struct Overlay {
    panel: Retained<NSPanel>,
    bar_views: Vec<Retained<NSView>>,
    current_heights: [f64; NUM_BARS],
    velocities: [f64; NUM_BARS],
    /// Ring buffer of amplitude history. Index via `history_head`.
    history: [f64; HALF_BARS],
    history_head: usize,
    visible: bool,
    frame_count: u64,
    position: OverlayPosition,
}

impl Overlay {
    pub fn new(mtm: MainThreadMarker, position: OverlayPosition) -> Self {
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
        let total_bars_width =
            NUM_BARS as f64 * BAR_WIDTH + (NUM_BARS - 1) as f64 * BAR_SPACING;
        let bars_x_offset = PADDING_X + (usable_width - total_bars_width) / 2.0;

        let mut bar_views = Vec::with_capacity(NUM_BARS);
        for i in 0..NUM_BARS {
            let x = bars_x_offset + i as f64 * (BAR_WIDTH + BAR_SPACING);
            let bar = {
                let view = NSView::new(mtm);
                let y = (WINDOW_HEIGHT - BAR_MIN_HEIGHT) / 2.0;
                view.setFrame(CGRect::new(
                    CGPoint::new(x, y),
                    CGSize::new(BAR_WIDTH, BAR_MIN_HEIGHT),
                ));
                view.setWantsLayer(true);
                if let Some(layer) = view.layer() {
                    let dist_from_center = ((i as f64) - (NUM_BARS - 1) as f64 / 2.0).abs()
                        / ((NUM_BARS - 1) as f64 / 2.0);
                    let center = 1.0 - dist_from_center;
                    let center_sq = center * center;
                    let r = 0.30 + 0.70 * center_sq;
                    let g = 0.20 + 0.64 * center_sq;
                    let b = 0.05 + 0.05 * center_sq;
                    let alpha = 0.60 + 0.40 * center;
                    let ns_color = NSColor::colorWithRed_green_blue_alpha(r, g, b, alpha);
                    unsafe {
                        let cg_color: *const std::ffi::c_void = msg_send![&ns_color, CGColor];
                        let _: () = msg_send![&*layer, setBackgroundColor: cg_color];
                    }
                    layer.setCornerRadius((BAR_WIDTH / 2.0) as CGFloat);
                }
                view
            };
            bar_views.push(bar);
        }

        {
            let content_view = panel.contentView().unwrap();
            content_view.addSubview(&effect_view);
            for bar in &bar_views {
                effect_view.addSubview(bar);
            }
        }

        let s = Self {
            panel,
            bar_views,
            current_heights: [BAR_MIN_HEIGHT; NUM_BARS],
            velocities: [0.0; NUM_BARS],
            history: [0.0; HALF_BARS],
            history_head: 0,
            visible: false,
            frame_count: 0,
            position,
        };
        s.apply_position(mtm);
        s
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.frame_count = 0;
        self.history = [0.0; HALF_BARS];
        self.history_head = 0;
        self.panel.orderFrontRegardless();
        self.panel.setAlphaValue(1.0);
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.current_heights = [BAR_MIN_HEIGHT; NUM_BARS];
        self.velocities = [0.0; NUM_BARS];
        self.history = [0.0; HALF_BARS];
        self.history_head = 0;
        self.panel.setAlphaValue(0.0);
        self.panel.orderOut(None);
    }

    pub fn update_amplitude(&mut self, raw_amp: f32) {
        if !self.visible {
            return;
        }

        self.frame_count += 1;
        let time = self.frame_count as f64 * 0.05;

        // Single-stage boost with good dynamic range:
        // RMS 0.01 → 0.17, RMS 0.05 → 0.39, RMS 0.1 → 0.55, RMS 0.33 → 1.0
        let boosted = ((raw_amp as f64) * 3.0).min(1.0).sqrt();

        // Insert new amplitude into ring buffer
        self.history[self.history_head] = boosted;
        self.history_head = (self.history_head + 1) % HALF_BARS;

        // Map history to bars: mirror from center outward
        // history_head points to oldest entry; (history_head - 1) is newest (center)
        for k in 0..HALF_BARS {
            let left_bar = HALF_BARS - 1 - k;
            let right_bar = HALF_BARS + k;

            // k=0 is center (newest), k=HALF_BARS-1 is edge (oldest)
            // newest is at (history_head - 1), k-th from newest is (history_head - 1 - k)
            let hist_idx = (self.history_head + HALF_BARS - 1 - k) % HALF_BARS;
            let amp = self.history[hist_idx];

            let phase = k as f64 * 0.3 + time * 2.5;
            let idle_wave = (phase.sin() * 0.5 + 0.5) * 0.06;

            let edge_fade = 1.0 - (k as f64 / HALF_BARS as f64) * 0.15;

            let signal = (amp * edge_fade + idle_wave).min(1.0);
            let target = BAR_MIN_HEIGHT + signal * (BAR_MAX_HEIGHT - BAR_MIN_HEIGHT);

            for bar_idx in [left_bar, right_bar] {
                let diff = target - self.current_heights[bar_idx];
                if diff > 0.0 {
                    self.velocities[bar_idx] += diff * 0.3;
                    self.velocities[bar_idx] *= 0.6;
                } else {
                    self.velocities[bar_idx] += diff * 0.05;
                    self.velocities[bar_idx] *= 0.85;
                }

                self.current_heights[bar_idx] += self.velocities[bar_idx];
                self.current_heights[bar_idx] = self.current_heights[bar_idx]
                    .max(BAR_MIN_HEIGHT)
                    .min(BAR_MAX_HEIGHT);

                let h = self.current_heights[bar_idx];
                let y = (WINDOW_HEIGHT - h) / 2.0;

                let mut frame: CGRect = self.bar_views[bar_idx].frame();
                frame.origin.y = y as CGFloat;
                frame.size.height = h as CGFloat;
                self.bar_views[bar_idx].setFrame(frame);
            }
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn set_position(&mut self, mtm: MainThreadMarker, position: OverlayPosition) {
        self.position = position;
        self.apply_position(mtm);
    }

    fn apply_position(&self, mtm: MainThreadMarker) {
        if let Some(screen) = NSScreen::mainScreen(mtm) {
            let sf = screen.frame();
            let x = (sf.size.width - WINDOW_WIDTH) / 2.0;
            let y = match self.position {
                OverlayPosition::Center => (sf.size.height - WINDOW_HEIGHT) / 2.0,
                OverlayPosition::Top => sf.size.height - WINDOW_HEIGHT - POSITION_MARGIN,
                OverlayPosition::Bottom => POSITION_MARGIN,
            };
            self.panel.setFrameOrigin(CGPoint::new(x, y));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_physics_attack() {
        // Verify spring moves toward target on positive diff
        let mut height = BAR_MIN_HEIGHT;
        let mut velocity = 0.0;
        let target = BAR_MAX_HEIGHT;

        for _ in 0..20 {
            let diff = target - height;
            velocity += diff * 0.3;
            velocity *= 0.6;
            height += velocity;
            height = height.max(BAR_MIN_HEIGHT).min(BAR_MAX_HEIGHT);
        }

        assert!(height > BAR_MIN_HEIGHT + 10.0, "spring should move toward target");
    }

    #[test]
    fn spring_physics_decay() {
        // Verify spring decays slowly — doesn't snap to min in just a few frames
        let mut height = BAR_MAX_HEIGHT;
        let mut velocity = 0.0;
        let target = BAR_MIN_HEIGHT;

        // Simulate 5 frames of decay
        for _ in 0..5 {
            let diff = target - height;
            velocity += diff * 0.05;
            velocity *= 0.85;
            height += velocity;
            height = height.max(BAR_MIN_HEIGHT).min(BAR_MAX_HEIGHT);
        }

        // After only 5 frames, should still be well above minimum (slow decay)
        assert!(height < BAR_MAX_HEIGHT, "spring should start decaying");
        assert!(height > BAR_MIN_HEIGHT + 5.0, "decay should be gradual over 5 frames");
    }

    #[test]
    fn ring_buffer_ordering() {
        // Verify ring buffer reads newest-first
        let mut history = [0.0f64; HALF_BARS];
        let mut head = 0;

        // Insert values 1..=HALF_BARS
        for i in 1..=HALF_BARS {
            history[head] = i as f64;
            head = (head + 1) % HALF_BARS;
        }

        // k=0 should be newest (HALF_BARS), k=HALF_BARS-1 should be oldest (1)
        for k in 0..HALF_BARS {
            let hist_idx = (head + HALF_BARS - 1 - k) % HALF_BARS;
            let expected = (HALF_BARS - k) as f64;
            assert_eq!(history[hist_idx], expected,
                "k={} should map to value {}", k, expected);
        }
    }
}

use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSScreen,
    NSView, NSVisualEffectMaterial, NSVisualEffectView,
    NSWindowStyleMask,
};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation::MainThreadMarker;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const WINDOW_WIDTH: f64 = 340.0;
const WINDOW_HEIGHT: f64 = 80.0;
const NUM_BARS: usize = 56;
const BAR_WIDTH: f64 = 3.0;
const BAR_SPACING: f64 = 2.5;
const BAR_MIN_HEIGHT: f64 = 5.0;
const CORNER_RADIUS: f64 = 18.0;
const PADDING_X: f64 = 14.0;
const PADDING_Y: f64 = 8.0;

pub struct Overlay {
    panel: Retained<NSPanel>,
    bar_views: Vec<Retained<NSView>>,
    current_heights: Vec<f64>,
    // Per-bar velocity for springy overshoot
    velocities: Vec<f64>,
    is_visible: Arc<AtomicBool>,
    frame_count: u64,
}

impl Overlay {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let style = NSWindowStyleMask::Borderless
            | NSWindowStyleMask::NonactivatingPanel;

        let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(WINDOW_WIDTH, WINDOW_HEIGHT));

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

        // Bars fill edge-to-edge with just a bit of padding
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
                    // Gradient: edges are soft cyan, center glows white
                    let t = i as f64 / (NUM_BARS - 1) as f64;
                    let center = 1.0 - (2.0 * (t - 0.5)).abs(); // 0 at edges, 1 at center
                    let center_sq = center * center; // sharper falloff
                    let r = 0.25 + 0.75 * center_sq;
                    let g = 0.65 + 0.35 * center_sq;
                    let b = 1.0;
                    let alpha = 0.55 + 0.45 * center;
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

        // Assemble
        {
            let content_view = panel.contentView().unwrap();
            content_view.addSubview(&effect_view);
            for bar in &bar_views {
                effect_view.addSubview(bar);
            }
        }

        // Center of screen
        if let Some(screen) = NSScreen::mainScreen(mtm) {
            let screen_frame = screen.frame();
            let x = (screen_frame.size.width - WINDOW_WIDTH) / 2.0;
            let y = (screen_frame.size.height - WINDOW_HEIGHT) / 2.0;
            panel.setFrameOrigin(CGPoint::new(x, y));
        }

        Self {
            panel,
            bar_views,
            current_heights: vec![BAR_MIN_HEIGHT; NUM_BARS],
            velocities: vec![0.0; NUM_BARS],
            is_visible: Arc::new(AtomicBool::new(false)),
            frame_count: 0,
        }
    }

    pub fn show(&mut self) {
        self.is_visible.store(true, Ordering::Relaxed);
        self.frame_count = 0;
        self.panel.orderFrontRegardless();
        self.panel.setAlphaValue(1.0);
    }

    pub fn hide(&mut self) {
        self.is_visible.store(false, Ordering::Relaxed);
        for h in &mut self.current_heights {
            *h = BAR_MIN_HEIGHT;
        }
        for v in &mut self.velocities {
            *v = 0.0;
        }
        self.panel.setAlphaValue(0.0);
        self.panel.orderOut(None);
    }

    pub fn update_amplitudes(&mut self, amplitudes: &[f32]) {
        if !self.is_visible.load(Ordering::Relaxed) {
            return;
        }

        self.frame_count += 1;
        let bar_max_height = WINDOW_HEIGHT - PADDING_Y * 2.0;
        let time = self.frame_count as f64 * 0.05; // 50ms per frame

        for (i, bar) in self.bar_views.iter().enumerate() {
            let amp = if i < amplitudes.len() {
                amplitudes[i] as f64
            } else {
                0.0
            };

            // Aggressive boost so even quiet speech fills the bars
            let boosted = (amp * 5.0).min(1.0).powf(0.5);

            // Add a subtle idle wave so bars never look dead
            let phase = i as f64 * 0.4 + time * 2.5;
            let idle_wave = (phase.sin() * 0.5 + 0.5) * 0.08;

            let signal = boosted + idle_wave;
            let target = BAR_MIN_HEIGHT + signal.min(1.0) * (bar_max_height - BAR_MIN_HEIGHT);

            // Spring physics: overshoot on attack, smooth settle
            let diff = target - self.current_heights[i];
            if diff > 0.0 {
                // Attack: spring toward target with velocity
                self.velocities[i] += diff * 0.3;
                self.velocities[i] *= 0.6; // damping
            } else {
                // Decay: slow drift down
                self.velocities[i] += diff * 0.05;
                self.velocities[i] *= 0.85; // less damping = more float
            }

            self.current_heights[i] += self.velocities[i];
            self.current_heights[i] = self.current_heights[i]
                .max(BAR_MIN_HEIGHT)
                .min(bar_max_height);

            let h = self.current_heights[i];
            let y = (WINDOW_HEIGHT - h) / 2.0;

            let mut frame: CGRect = bar.frame();
            frame.origin.y = y as CGFloat;
            frame.size.height = h as CGFloat;
            bar.setFrame(frame);
        }
    }

    pub fn is_visible(&self) -> bool {
        self.is_visible.load(Ordering::Relaxed)
    }
}

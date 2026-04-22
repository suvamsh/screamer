use crate::logging;
use crate::screenshot::ScreenCaptureBounds;
use crate::vision::{PointerSide, ScreenPoint};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{NSBackingStoreType, NSColor, NSPanel, NSView, NSWindowStyleMask};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation::MainThreadMarker;

/// Arrow length (in points) from the arrow base to the tip.
const ARROW_LENGTH: f64 = 42.0;

/// Arrow head size (half-width of the arrowhead triangle).
const ARROW_HEAD_SIZE: f64 = 10.0;

/// Stroke width for the pointer line.
const POINTER_STROKE_WIDTH: f64 = 2.5;

/// Keep some breathing room so the arrow body does not end up pinned to the edge.
const EDGE_MARGIN: f64 = 20.0;

#[derive(Clone, Debug, PartialEq)]
pub struct HighlightTarget {
    pub point: ScreenPoint,
    pub capture_bounds: ScreenCaptureBounds,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ArrowGeometry {
    requested_side: PointerSide,
    resolved_side: PointerSide,
    tip: CGPoint,
    base: CGPoint,
}

/// The fullscreen transparent overlay that draws a pointer arrow on screen.
pub struct HighlightOverlay {
    panel: Retained<NSPanel>,
    draw_view: HighlightDrawView,
    visible: bool,
}

/// Use layer-based drawing with CAShapeLayer for a lightweight pointer overlay.
struct HighlightDrawView {
    container: Retained<NSView>,
    arrow_layer: *mut objc2::runtime::AnyObject,
    glow_layer: *mut objc2::runtime::AnyObject,
}

unsafe impl Send for HighlightDrawView {}
unsafe impl Sync for HighlightDrawView {}

impl HighlightOverlay {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(1.0, 1.0));

        let panel = {
            let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<NSPanel>(),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            );
            panel.setLevel(26);
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setHasShadow(false);
            panel.setMovableByWindowBackground(false);
            panel.setHidesOnDeactivate(false);
            panel.setIgnoresMouseEvents(true);
            panel.setAlphaValue(0.0);
            panel.setCollectionBehavior(
                objc2_app_kit::NSWindowCollectionBehavior::CanJoinAllSpaces
                    | objc2_app_kit::NSWindowCollectionBehavior::Stationary
                    | objc2_app_kit::NSWindowCollectionBehavior::IgnoresCycle,
            );
            panel
        };

        let container = {
            let view = NSView::new(mtm);
            view.setFrame(frame);
            view.setWantsLayer(true);
            view
        };

        let (glow_layer, arrow_layer) = unsafe {
            let ca_shape_class = objc2::runtime::AnyClass::get(c"CAShapeLayer").unwrap();

            let glow: *mut objc2::runtime::AnyObject = msg_send![ca_shape_class, new];
            let _: () = msg_send![glow, setFillColor: std::ptr::null::<std::ffi::c_void>()];
            let _: () = msg_send![glow, setLineWidth: (POINTER_STROKE_WIDTH + 6.0) as CGFloat];
            let _: () = msg_send![glow, setOpacity: 0.0f32];

            let arrow: *mut objc2::runtime::AnyObject = msg_send![ca_shape_class, new];
            let _: () = msg_send![arrow, setLineWidth: POINTER_STROKE_WIDTH as CGFloat];

            if let Some(layer) = container.layer() {
                let _: () = msg_send![&*layer, addSublayer: glow];
                let _: () = msg_send![&*layer, addSublayer: arrow];
            }

            (glow, arrow)
        };

        {
            let content_view = panel.contentView().unwrap();
            content_view.addSubview(&container);
        }

        Self {
            panel,
            draw_view: HighlightDrawView {
                container,
                arrow_layer,
                glow_layer,
            },
            visible: false,
        }
    }

    /// Show the highlight using the same virtual-desktop bounds as the captured screenshot.
    pub fn show(&mut self, target: &HighlightTarget) {
        let overlay_frame = CGRect::new(
            CGPoint::new(target.capture_bounds.x, target.capture_bounds.y),
            CGSize::new(target.capture_bounds.width, target.capture_bounds.height),
        );

        self.panel.setFrame_display(overlay_frame, false);
        self.draw_view.container.setFrame(CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(overlay_frame.size.width, overlay_frame.size.height),
        ));

        let target_point = screen_point_to_overlay_point(&target.point, target.capture_bounds);
        let geometry = resolve_arrow_geometry(target_point, target.point.side, overlay_frame);

        logging::log_highlight_event(
            "show",
            &format!(
                "capture_bounds=({:.1}, {:.1}, {:.1}, {:.1}) target={} requested_side={} resolved_side={} tip=({:.1}, {:.1}) base=({:.1}, {:.1}) rotation_deg={:.1}",
                target.capture_bounds.x,
                target.capture_bounds.y,
                target.capture_bounds.width,
                target.capture_bounds.height,
                target.point.describe(),
                geometry.requested_side.as_str(),
                geometry.resolved_side.as_str(),
                geometry.tip.x,
                geometry.tip.y,
                geometry.base.x,
                geometry.base.y,
                geometry.resolved_side.rotation_degrees(),
            ),
        );

        self.draw_arrow(&geometry);

        self.visible = true;
        self.panel.orderFrontRegardless();
        self.panel.setAlphaValue(1.0);
        self.animate_glow_pulse();
    }

    pub fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.panel.setAlphaValue(0.0);
        self.panel.orderOut(None);
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    fn draw_arrow(&self, geometry: &ArrowGeometry) {
        let arrow_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.86, 0.70, 0.34, 0.90);
        let glow_color = NSColor::colorWithSRGBRed_green_blue_alpha(0.86, 0.70, 0.34, 0.25);

        let dx = geometry.tip.x - geometry.base.x;
        let dy = geometry.tip.y - geometry.base.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1.0 {
            return;
        }

        let ux = dx / len;
        let uy = dy / len;
        let px = -uy;
        let py = ux;

        let head_base_x = geometry.tip.x - ux * ARROW_HEAD_SIZE * 1.8;
        let head_base_y = geometry.tip.y - uy * ARROW_HEAD_SIZE * 1.8;
        let left_x = head_base_x + px * ARROW_HEAD_SIZE;
        let left_y = head_base_y + py * ARROW_HEAD_SIZE;
        let right_x = head_base_x - px * ARROW_HEAD_SIZE;
        let right_y = head_base_y - py * ARROW_HEAD_SIZE;

        unsafe {
            let cg_path = arrow_path(
                geometry.base.x,
                geometry.base.y,
                head_base_x,
                head_base_y,
                geometry.tip.x,
                geometry.tip.y,
                left_x,
                left_y,
                right_x,
                right_y,
            );

            let color_cg: *const std::ffi::c_void = msg_send![&arrow_color, CGColor];
            let glow_cg: *const std::ffi::c_void = msg_send![&glow_color, CGColor];
            let _: () = msg_send![self.draw_view.glow_layer, setStrokeColor: glow_cg];
            let _: () = msg_send![self.draw_view.glow_layer, setFillColor: glow_cg];
            let _: () = msg_send![self.draw_view.glow_layer, setPath: cg_path];
            let _: () = msg_send![self.draw_view.glow_layer, setOpacity: 1.0f32];
            let _: () = msg_send![self.draw_view.arrow_layer, setStrokeColor: color_cg];
            let _: () = msg_send![self.draw_view.arrow_layer, setFillColor: color_cg];
            let _: () = msg_send![self.draw_view.arrow_layer, setPath: cg_path];

            CGPathRelease(cg_path as *const _);
        }
    }

    fn animate_glow_pulse(&self) {
        unsafe {
            let ca_anim_class = objc2::runtime::AnyClass::get(c"CABasicAnimation").unwrap();
            let key = objc2_foundation::NSString::from_str("opacity");
            let anim: *mut objc2::runtime::AnyObject =
                msg_send![ca_anim_class, animationWithKeyPath: &*key];

            let from = objc2_foundation::NSNumber::new_f32(0.3);
            let to = objc2_foundation::NSNumber::new_f32(1.0);
            let _: () = msg_send![anim, setFromValue: &*from];
            let _: () = msg_send![anim, setToValue: &*to];
            let _: () = msg_send![anim, setDuration: 0.8f64];
            let _: () = msg_send![anim, setAutoreverses: true];
            let _: () = msg_send![anim, setRepeatCount: f32::INFINITY];

            let anim_key = objc2_foundation::NSString::from_str("glowPulse");
            let _: () =
                msg_send![self.draw_view.glow_layer, addAnimation: anim, forKey: &*anim_key];
        }
    }
}

extern "C" {
    fn CGPathCreateMutable() -> *mut std::ffi::c_void;
    fn CGPathMoveToPoint(
        path: *mut std::ffi::c_void,
        m: *const std::ffi::c_void,
        x: CGFloat,
        y: CGFloat,
    );
    fn CGPathAddLineToPoint(
        path: *mut std::ffi::c_void,
        m: *const std::ffi::c_void,
        x: CGFloat,
        y: CGFloat,
    );
    fn CGPathCloseSubpath(path: *mut std::ffi::c_void);
    fn CGPathRelease(path: *const std::ffi::c_void);
}

fn arrow_path(
    base_x: CGFloat,
    base_y: CGFloat,
    head_base_x: CGFloat,
    head_base_y: CGFloat,
    tip_x: CGFloat,
    tip_y: CGFloat,
    left_x: CGFloat,
    left_y: CGFloat,
    right_x: CGFloat,
    right_y: CGFloat,
) -> *const std::ffi::c_void {
    unsafe {
        let path = CGPathCreateMutable();
        CGPathMoveToPoint(path, std::ptr::null(), base_x, base_y);
        CGPathAddLineToPoint(path, std::ptr::null(), head_base_x, head_base_y);
        CGPathMoveToPoint(path, std::ptr::null(), tip_x, tip_y);
        CGPathAddLineToPoint(path, std::ptr::null(), left_x, left_y);
        CGPathAddLineToPoint(path, std::ptr::null(), right_x, right_y);
        CGPathCloseSubpath(path);
        path as *const _
    }
}

fn screen_point_to_overlay_point(
    point: &ScreenPoint,
    capture_bounds: ScreenCaptureBounds,
) -> CGPoint {
    let width = capture_bounds.width.max(1.0);
    let height = capture_bounds.height.max(1.0);
    let x = (point.x_pct / 100.0 * width).clamp(0.0, width);
    let y_from_top = point.y_pct / 100.0 * height;
    let y = (height - y_from_top).clamp(0.0, height);

    CGPoint::new(clamp_axis(x, width), clamp_axis(y, height))
}

fn clamp_axis(value: f64, max: f64) -> f64 {
    if max <= 2.0 {
        return value.clamp(0.0, max);
    }
    value.clamp(1.0, max - 1.0)
}

fn resolve_arrow_geometry(
    point: CGPoint,
    requested_side: PointerSide,
    screen_frame: CGRect,
) -> ArrowGeometry {
    for side in side_candidates(requested_side) {
        let geometry = geometry_for_side(point, requested_side, side);
        if geometry_fits(&geometry, screen_frame) {
            return geometry;
        }
    }

    geometry_for_side(point, requested_side, requested_side)
}

fn side_candidates(side: PointerSide) -> [PointerSide; 4] {
    match side {
        PointerSide::Left => [
            PointerSide::Left,
            PointerSide::Right,
            PointerSide::Top,
            PointerSide::Bottom,
        ],
        PointerSide::Right => [
            PointerSide::Right,
            PointerSide::Left,
            PointerSide::Top,
            PointerSide::Bottom,
        ],
        PointerSide::Top => [
            PointerSide::Top,
            PointerSide::Bottom,
            PointerSide::Right,
            PointerSide::Left,
        ],
        PointerSide::Bottom => [
            PointerSide::Bottom,
            PointerSide::Top,
            PointerSide::Right,
            PointerSide::Left,
        ],
    }
}

fn geometry_for_side(
    point: CGPoint,
    requested_side: PointerSide,
    resolved_side: PointerSide,
) -> ArrowGeometry {
    let base = match resolved_side {
        PointerSide::Left => CGPoint::new(point.x - ARROW_LENGTH, point.y),
        PointerSide::Right => CGPoint::new(point.x + ARROW_LENGTH, point.y),
        PointerSide::Top => CGPoint::new(point.x, point.y + ARROW_LENGTH),
        PointerSide::Bottom => CGPoint::new(point.x, point.y - ARROW_LENGTH),
    };

    ArrowGeometry {
        requested_side,
        resolved_side,
        tip: point,
        base,
    }
}

fn geometry_fits(geometry: &ArrowGeometry, screen_frame: CGRect) -> bool {
    geometry.tip.x >= 0.0
        && geometry.tip.x <= screen_frame.size.width
        && geometry.tip.y >= 0.0
        && geometry.tip.y <= screen_frame.size.height
        && geometry.base.x >= EDGE_MARGIN
        && geometry.base.x <= screen_frame.size.width - EDGE_MARGIN
        && geometry.base.y >= EDGE_MARGIN
        && geometry.base.y <= screen_frame.size.height - EDGE_MARGIN
}

#[cfg(test)]
mod tests {
    use super::{resolve_arrow_geometry, screen_point_to_overlay_point, ARROW_LENGTH};
    use crate::screenshot::ScreenCaptureBounds;
    use crate::vision::{PointerSide, ScreenPoint};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    #[test]
    fn maps_percentages_using_virtual_desktop_bounds() {
        let capture_bounds = ScreenCaptureBounds {
            x: -1440.0,
            y: 0.0,
            width: 3488.0,
            height: 1198.0,
        };
        let point = ScreenPoint {
            x_pct: 1707.0 / capture_bounds.width * 100.0,
            y_pct: 520.0 / capture_bounds.height * 100.0,
            side: PointerSide::Left,
        };

        let mapped = screen_point_to_overlay_point(&point, capture_bounds);
        let mapped_y_from_top = capture_bounds.height - mapped.y;

        assert!((mapped.x - 1707.0).abs() < 0.01);
        assert!((mapped_y_from_top - 520.0).abs() < 0.01);
    }

    #[test]
    fn flips_side_when_requested_side_would_go_offscreen() {
        let geometry = resolve_arrow_geometry(
            CGPoint::new(10.0, 200.0),
            PointerSide::Left,
            CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(500.0, 400.0)),
        );

        assert_eq!(geometry.requested_side, PointerSide::Left);
        assert_eq!(geometry.resolved_side, PointerSide::Right);
        assert!((geometry.base.x - (geometry.tip.x + ARROW_LENGTH)).abs() < 0.01);
    }
}

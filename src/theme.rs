use crate::config::AppAppearance;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSColor,
};
use objc2_foundation::MainThreadMarker;

pub fn apply_app_appearance(mtm: MainThreadMarker, appearance: AppAppearance) {
    let app = NSApplication::sharedApplication(mtm);
    if let Some(ns_appearance) = ns_appearance(appearance) {
        app.setAppearance(Some(&ns_appearance));
    }
}

pub fn window_background(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.07, 0.06, 0.05, 1.0),
        AppAppearance::Light => srgb(0.97, 0.95, 0.92, 1.0),
    }
}

pub fn surface_background(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.11, 0.095, 0.08, 1.0),
        AppAppearance::Light => srgb(1.0, 0.99, 0.96, 1.0),
    }
}

pub fn title_text(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.97, 0.95, 0.92, 1.0),
        AppAppearance::Light => srgb(0.15, 0.11, 0.08, 1.0),
    }
}

pub fn body_text(appearance: AppAppearance) -> Retained<NSColor> {
    title_text(appearance)
}

pub fn secondary_text(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.82, 0.78, 0.73, 1.0),
        AppAppearance::Light => srgb(0.39, 0.31, 0.24, 1.0),
    }
}

pub fn brand_gold() -> Retained<NSColor> {
    srgb(0.86, 0.70, 0.34, 1.0)
}

pub fn processing_accent() -> Retained<NSColor> {
    srgb(0.90, 0.65, 0.20, 1.0)
}

pub fn completed_accent(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.35, 0.72, 0.45, 1.0),
        AppAppearance::Light => srgb(0.22, 0.55, 0.30, 1.0),
    }
}

pub fn failed_accent() -> Retained<NSColor> {
    srgb(0.82, 0.30, 0.25, 1.0)
}

pub fn processing_overlay_background(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.09, 0.08, 0.06, 0.85),
        AppAppearance::Light => srgb(0.97, 0.95, 0.92, 0.85),
    }
}

pub fn card_border(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.86, 0.70, 0.34, 0.14),
        AppAppearance::Light => srgb(0.18, 0.14, 0.10, 0.10),
    }
}

pub fn session_panel_background(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.095, 0.082, 0.070, 1.0),
        AppAppearance::Light => srgb(0.985, 0.975, 0.955, 1.0),
    }
}

pub fn session_panel_border(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.93, 0.83, 0.58, 0.28),
        AppAppearance::Light => srgb(0.36, 0.28, 0.16, 0.16),
    }
}

pub fn scratch_pad_background(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.12, 0.102, 0.085, 1.0),
        AppAppearance::Light => srgb(1.0, 0.985, 0.96, 1.0),
    }
}

pub fn scratch_pad_border(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.95, 0.79, 0.42, 0.42),
        AppAppearance::Light => srgb(0.48, 0.34, 0.14, 0.24),
    }
}

pub fn scratch_pad_hint(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.92, 0.80, 0.52, 0.88),
        AppAppearance::Light => srgb(0.52, 0.36, 0.12, 0.88),
    }
}

pub fn logo_badge_background(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.12, 0.10, 0.08, 0.0),
        AppAppearance::Light => srgb(0.06, 0.05, 0.05, 0.98),
    }
}

pub fn logo_badge_border(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.0, 0.0, 0.0, 0.0),
        AppAppearance::Light => srgb(0.86, 0.70, 0.34, 0.22),
    }
}

pub fn overlay_transcript_text(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(1.0, 1.0, 1.0, 0.90),
        AppAppearance::Light => srgb(0.08, 0.08, 0.08, 0.92),
    }
}

pub fn overlay_panel_fill(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.08, 0.06, 0.05, 0.22),
        AppAppearance::Light => srgb(1.0, 0.99, 0.97, 0.48),
    }
}

pub fn overlay_panel_border(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.86, 0.70, 0.34, 0.12),
        AppAppearance::Light => srgb(0.14, 0.12, 0.10, 0.12),
    }
}

pub fn overlay_bar_color(appearance: AppAppearance, glow: f64) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => {
            let r = 0.98;
            let g = 0.80 + 0.16 * glow;
            let b = 0.22 + 0.08 * glow;
            let alpha = 0.40 + 0.24 * glow;
            NSColor::colorWithRed_green_blue_alpha(r, g, b, alpha)
        }
        AppAppearance::Light => {
            let r = 0.73 + 0.08 * glow;
            let g = 0.52 + 0.10 * glow;
            let b = 0.10 + 0.05 * glow;
            let alpha = 0.66 + 0.18 * glow;
            NSColor::colorWithRed_green_blue_alpha(r, g, b, alpha)
        }
    }
}

pub fn overlay_vision_label_text(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.55, 0.85, 1.0, 0.90),
        AppAppearance::Light => srgb(0.10, 0.40, 0.65, 0.92),
    }
}

pub fn overlay_vision_loading_text(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(1.0, 1.0, 1.0, 0.50),
        AppAppearance::Light => srgb(0.08, 0.08, 0.08, 0.50),
    }
}

pub fn loading_panel_background(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => NSColor::colorWithCalibratedWhite_alpha(0.08, 0.96),
        AppAppearance::Light => srgb(0.99, 0.98, 0.95, 0.98),
    }
}

pub fn loading_divider(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => NSColor::colorWithCalibratedWhite_alpha(1.0, 0.12),
        AppAppearance::Light => srgb(0.12, 0.10, 0.08, 0.10),
    }
}

pub fn bubble_you(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.20, 0.17, 0.13, 1.0),
        AppAppearance::Light => srgb(0.93, 0.91, 0.87, 1.0),
    }
}

pub fn bubble_other(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.14, 0.12, 0.10, 1.0),
        AppAppearance::Light => srgb(0.97, 0.95, 0.92, 1.0),
    }
}

pub fn bubble_for_speaker(speaker_index: usize, appearance: AppAppearance) -> Retained<NSColor> {
    match speaker_index {
        0 => bubble_you(appearance),
        _ => bubble_other(appearance),
    }
}

pub fn speaker_you(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.86, 0.70, 0.34, 1.0),
        AppAppearance::Light => srgb(0.60, 0.45, 0.15, 1.0),
    }
}

pub fn speaker_other(appearance: AppAppearance) -> Retained<NSColor> {
    match appearance {
        AppAppearance::Dark => srgb(0.55, 0.70, 0.82, 1.0),
        AppAppearance::Light => srgb(0.25, 0.40, 0.55, 1.0),
    }
}

pub fn speaker_color_for_index(
    speaker_index: usize,
    appearance: AppAppearance,
) -> Retained<NSColor> {
    match speaker_index {
        0 => speaker_you(appearance),
        1 => speaker_other(appearance),
        2 => match appearance {
            AppAppearance::Dark => srgb(0.70, 0.55, 0.80, 1.0),
            AppAppearance::Light => srgb(0.45, 0.30, 0.58, 1.0),
        },
        3 => match appearance {
            AppAppearance::Dark => srgb(0.55, 0.78, 0.58, 1.0),
            AppAppearance::Light => srgb(0.25, 0.50, 0.30, 1.0),
        },
        4 => match appearance {
            AppAppearance::Dark => srgb(0.82, 0.60, 0.55, 1.0),
            AppAppearance::Light => srgb(0.58, 0.32, 0.28, 1.0),
        },
        _ => match appearance {
            AppAppearance::Dark => srgb(0.70, 0.72, 0.55, 1.0),
            AppAppearance::Light => srgb(0.45, 0.47, 0.28, 1.0),
        },
    }
}

fn ns_appearance(appearance: AppAppearance) -> Option<Retained<NSAppearance>> {
    let name = unsafe {
        match appearance {
            AppAppearance::Dark => NSAppearanceNameDarkAqua,
            AppAppearance::Light => NSAppearanceNameAqua,
        }
    };
    NSAppearance::appearanceNamed(name)
}

fn srgb(r: f64, g: f64, b: f64, a: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a)
}

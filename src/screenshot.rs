use core_foundation::array::CFArray;
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::dictionary::{CFDictionaryGetValueIfPresent, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFStringRef;
use core_graphics::display::CGDisplay;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::image::CGImage;
use core_graphics::window::{
    copy_window_info, create_image, create_image_from_array, kCGNullWindowID,
    kCGWindowImageBestResolution, kCGWindowImageBoundsIgnoreFraming,
    kCGWindowListOptionOnScreenOnly, kCGWindowNumber, kCGWindowOwnerPID,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const SCREENSHOT_IMAGE_OPTIONS: u32 =
    kCGWindowImageBestResolution | kCGWindowImageBoundsIgnoreFraming;

static NEXT_SCREENSHOT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenCaptureBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug)]
pub struct CapturedScreen {
    pub path: PathBuf,
    pub bounds: ScreenCaptureBounds,
}

/// Captures the whole desktop and saves it as a PNG to a temporary file.
/// Returns the saved screenshot plus the virtual desktop bounds used to capture it.
pub fn capture_screen() -> Result<CapturedScreen, String> {
    let (bounds, display_count) = active_display_bounds()?;
    let image = match capture_windows_excluding_self(bounds)? {
        Some(image) => image,
        None => create_image(
            bounds,
            kCGWindowListOptionOnScreenOnly,
            kCGNullWindowID,
            SCREENSHOT_IMAGE_OPTIONS,
        )
        .or_else(|| CGDisplay::main().image())
        .ok_or("Failed to capture screen (CoreGraphics returned null)")?,
    };

    let path = screenshot_path();
    save_cgimage_as_png(&image, &path)?;
    crate::logging::eprint_vision_verbose_line(&format!(
        "[screamer] Vision screenshot saved {}x{} from {} display(s)",
        image.width(),
        image.height(),
        display_count
    ));

    Ok(CapturedScreen {
        path,
        bounds: ScreenCaptureBounds {
            x: bounds.origin.x,
            y: bounds.origin.y,
            width: bounds.size.width,
            height: bounds.size.height,
        },
    })
}

fn active_display_bounds() -> Result<(CGRect, usize), String> {
    let display_ids = CGDisplay::active_displays()
        .map_err(|err| format!("Failed to enumerate active displays: {err}"))?;
    if display_ids.is_empty() {
        return Err("No active displays found".to_string());
    }

    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for display_id in &display_ids {
        let bounds = CGDisplay::new(*display_id).bounds();
        min_x = min_x.min(bounds.origin.x);
        min_y = min_y.min(bounds.origin.y);
        max_x = max_x.max(bounds.origin.x + bounds.size.width);
        max_y = max_y.max(bounds.origin.y + bounds.size.height);
    }

    let bounds = CGRect::new(
        &CGPoint::new(min_x, min_y),
        &CGSize::new(max_x - min_x, max_y - min_y),
    );
    Ok((bounds, display_ids.len()))
}

fn capture_windows_excluding_self(bounds: CGRect) -> Result<Option<CGImage>, String> {
    let window_ids = window_ids_excluding_self()?;
    if window_ids.is_empty() {
        return Ok(None);
    }

    let window_numbers = window_ids
        .into_iter()
        .map(|window_id| CFNumber::from(i64::from(window_id)))
        .collect::<Vec<_>>();
    let window_array = CFArray::from_CFTypes(&window_numbers).into_untyped();

    Ok(create_image_from_array(
        bounds,
        window_array,
        SCREENSHOT_IMAGE_OPTIONS,
    ))
}

fn window_ids_excluding_self() -> Result<Vec<u32>, String> {
    let Some(window_info) = copy_window_info(kCGWindowListOptionOnScreenOnly, kCGNullWindowID)
    else {
        return Ok(Vec::new());
    };

    let current_pid = i64::from(std::process::id());
    let mut window_ids = Vec::new();

    for dict_ptr in window_info.iter() {
        let dict = *dict_ptr as CFDictionaryRef;
        if dict.is_null() {
            continue;
        }

        let owner_pid = dictionary_i64(dict, unsafe { kCGWindowOwnerPID });
        if owner_pid == Some(current_pid) {
            continue;
        }

        if let Some(window_id) = dictionary_i64(dict, unsafe { kCGWindowNumber }) {
            if window_id > 0 {
                if let Ok(window_id) = u32::try_from(window_id) {
                    window_ids.push(window_id);
                }
            }
        }
    }

    Ok(window_ids)
}

fn dictionary_i64(dict: CFDictionaryRef, key: CFStringRef) -> Option<i64> {
    let mut value = std::ptr::null();
    let found = unsafe { CFDictionaryGetValueIfPresent(dict, key.cast(), &mut value) };
    if found == 0 || value.is_null() {
        return None;
    }

    let value = unsafe { CFType::wrap_under_get_rule(value as CFTypeRef) };
    value
        .downcast::<CFNumber>()
        .and_then(|number| number.to_i64())
}

fn screenshot_path() -> PathBuf {
    let id = NEXT_SCREENSHOT_ID.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("screamer-vision-{}-{id}.png", std::process::id()))
}

fn save_cgimage_as_png(image: &CGImage, path: &std::path::Path) -> Result<(), String> {
    use core_foundation::base::{CFRelease, TCFType};
    use core_foundation::string::CFString;
    use core_foundation::url::CFURL;

    extern "C" {
        fn CGImageDestinationCreateWithURL(
            url: core_foundation::url::CFURLRef,
            type_: core_foundation::string::CFStringRef,
            count: usize,
            options: *const std::ffi::c_void,
        ) -> *mut std::ffi::c_void;

        fn CGImageDestinationAddImage(
            destination: *mut std::ffi::c_void,
            image: *const std::ffi::c_void,
            properties: *const std::ffi::c_void,
        );

        fn CGImageDestinationFinalize(destination: *mut std::ffi::c_void) -> bool;
    }

    // Get the raw CGImageRef pointer via the inner field
    // CGImage wraps a CGImageRef internally
    let image_ptr: *const std::ffi::c_void = unsafe {
        // CGImage stores the raw pointer; use transmute to access it
        std::mem::transmute_copy(image)
    };

    let url = CFURL::from_path(path, false)
        .ok_or_else(|| format!("Failed to create CFURL for {}", path.display()))?;
    let png_type = CFString::new("public.png");

    unsafe {
        let destination = CGImageDestinationCreateWithURL(
            url.as_concrete_TypeRef(),
            png_type.as_concrete_TypeRef(),
            1,
            std::ptr::null(),
        );
        if destination.is_null() {
            return Err("Failed to create CGImageDestination".to_string());
        }

        CGImageDestinationAddImage(destination, image_ptr, std::ptr::null());
        let success = CGImageDestinationFinalize(destination);
        CFRelease(destination as _);

        if success {
            Ok(())
        } else {
            Err("Failed to finalize PNG write".to_string())
        }
    }
}

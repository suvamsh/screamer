use core_graphics::display::CGDisplay;
use std::path::PathBuf;

/// Captures the main display and saves it as a PNG to a temporary file.
/// Returns the path to the saved screenshot.
pub fn capture_screen() -> Result<PathBuf, String> {
    let display = CGDisplay::main();
    let image = display
        .image()
        .ok_or("Failed to capture screen (CGDisplayCreateImage returned null)")?;

    let path = std::env::temp_dir().join("screamer_vision_screenshot.png");
    save_cgimage_as_png(&image, &path)?;
    Ok(path)
}

fn save_cgimage_as_png(
    image: &core_graphics::image::CGImage,
    path: &std::path::Path,
) -> Result<(), String> {
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

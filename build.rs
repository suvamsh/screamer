fn main() {
    // whisper-rs handles whisper.cpp compilation via its build script.
    // We just need to link the Apple frameworks we use directly.
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=Carbon");

    // Set minimum macOS deployment target
    println!("cargo:rustc-env=MACOSX_DEPLOYMENT_TARGET=13.0");
}

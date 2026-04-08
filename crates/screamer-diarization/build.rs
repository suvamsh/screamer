fn main() {
    if std::env::var("CARGO_CFG_TARGET_VENDOR").ok().as_deref() == Some("apple")
        && std::env::var_os("CARGO_FEATURE_ORT_COREML").is_some()
    {
        println!("cargo:rustc-link-lib=framework=CoreML");
    }
}

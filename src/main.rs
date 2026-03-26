mod app;
mod config;
mod hotkey;
mod overlay;
mod paster;
mod recorder;
mod transcriber;

use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;
use std::fs::OpenOptions;

fn main() {
    // Redirect stderr to a log file so we can debug when launched via `open`
    let log_path = "/tmp/screamer.log";
    if let Ok(file) = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path)
    {
        use std::os::unix::io::IntoRawFd;
        let fd = file.into_raw_fd();
        unsafe {
            libc::dup2(fd, 2); // redirect stderr
            libc::close(fd);
        }
    }

    eprintln!("[screamer] Starting up (PID: {})", std::process::id());

    // Initialize logging
    oslog::OsLogger::new("com.screamer.app")
        .level_filter(log::LevelFilter::Info)
        .init()
        .ok();

    let mtm = MainThreadMarker::new().expect("Must run on main thread");

    // Initialize NSApplication
    let ns_app = NSApplication::sharedApplication(mtm);
    ns_app.setActivationPolicy(
        objc2_app_kit::NSApplicationActivationPolicy::Accessory,
    );

    // Check accessibility permissions
    if !hotkey::Hotkey::check_permissions() {
        eprintln!("[screamer] WARNING: Accessibility permissions not granted");
    }

    // Create app (loads model, sets up menubar)
    let app = match app::App::new(mtm) {
        Some(a) => a,
        None => {
            eprintln!("[screamer] Failed to initialize app");
            return;
        }
    };

    // Start hotkey listener and waveform timer
    app.start(mtm);

    eprintln!("[screamer] Ready — hold Left Control to record");

    // Run the main event loop (blocks forever)
    ns_app.run();
}

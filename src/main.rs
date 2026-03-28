mod app;
mod audio;
mod config;
mod hardware;
mod hotkey;
mod loading;
mod model_paths;
mod overlay;
mod paster;
mod recorder;
mod settings_window;
mod sound;
mod transcriber;

use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;
use std::fs::OpenOptions;
use std::sync::mpsc;

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
    let _ = ns_app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Regular);
    app::install_main_menu(mtm, &ns_app);
    ns_app.finishLaunching();

    let loading = loading::LoadingWindow::show(mtm, &ns_app);

    // Check accessibility permissions
    if !hotkey::Hotkey::check_permissions() {
        eprintln!("[screamer] WARNING: Accessibility permissions not granted");
    }

    let config = config::Config::load();
    let (tx, rx) = mpsc::sync_channel(1);
    let load_config = config.clone();
    std::thread::spawn(move || {
        let result = app::App::load_transcriber(&load_config);
        let _ = tx.send(result);
    });

    let transcriber = loop {
        match rx.try_recv() {
            Ok(result) => break result,
            Err(mpsc::TryRecvError::Empty) => loading::pump(),
            Err(mpsc::TryRecvError::Disconnected) => {
                break Err(app::AppInitError {
                    title: "Startup Error",
                    message: "The background model loader exited unexpectedly.".to_string(),
                })
            }
        }
    };

    // Create app (sets up menubar/UI after background model loading completes)
    let app = match transcriber
        .and_then(|transcriber| app::App::new_with_transcriber(mtm, config, transcriber))
    {
        Ok(a) => a,
        Err(err) => {
            loading.close();
            app::App::show_alert(mtm, err.title, &err.message);
            eprintln!("[screamer] Failed to initialize app");
            return;
        }
    };

    // Start hotkey listener and waveform timer
    app.start(mtm);
    loading.close();
    app::show_settings_window();

    eprintln!("[screamer] Ready — hold Left Control to record");

    // Run the main event loop (blocks forever)
    ns_app.run();
}

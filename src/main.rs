mod ambient_controller;
mod ambient_final_pass;
mod ambient_pipeline;
mod ambient_whisperx;
mod app;
mod branding;
mod bundled_llm;
mod config;
mod diarization;
mod highlight;
mod hotkey;
mod loading;
mod logging;
mod main_window;
mod overlay;
mod paster;
mod permission_window;
mod permissions;
mod recorder;
mod screenshot;
mod session_store;
mod settings_window;
mod sound;
mod speech;
mod summary_backend;
mod theme;
mod vision;
mod vision_gemini;
mod vision_openai;

use objc2_app_kit::NSApplication;
use objc2_foundation::MainThreadMarker;
use std::sync::mpsc;

fn main() {
    // Redirect stderr to a per-user log file so GUI launches remain debuggable
    // without writing logs to a world-readable temp directory.
    logging::init_stderr_log();

    eprintln!("[screamer] Starting up (PID: {})", std::process::id());
    if let Some(path) = logging::active_log_path() {
        eprintln!("[screamer] Log file: {}", path.display());
    }

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

    let mut config = config::Config::load();
    theme::apply_app_appearance(mtm, config.appearance);

    let loading = loading::LoadingWindow::show(mtm, &ns_app, config.appearance);

    let should_prompt_accessibility =
        !permissions::has_accessibility_permission() && config.show_accessibility_helper_on_launch;
    let permission_status = permissions::request_startup_permissions(should_prompt_accessibility);
    if config.show_accessibility_helper_on_launch {
        config.show_accessibility_helper_on_launch = false;
        config.save();
    }
    if !permission_status.microphone_granted {
        eprintln!("[screamer] WARNING: Microphone permission not granted");
    }
    if !permission_status.accessibility_granted {
        eprintln!("[screamer] WARNING: Accessibility permission not granted");
    }

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
    app::show_main_window();
    if !permission_status.accessibility_granted && should_prompt_accessibility {
        app::show_accessibility_window();
    }

    eprintln!("[screamer] Ready — hold Left Control to record");

    // Run the main event loop (blocks forever)
    ns_app.run();
}

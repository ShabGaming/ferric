#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
compile_error!("Ferric currently targets Windows.");

mod desktop;
mod diagnostics;
mod identity;
mod media;
mod memory;
mod settings;
mod startup;

use std::{
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Manager, WindowEvent};

struct AppState {
    data_directory: PathBuf,
    media: Mutex<Option<media::MediaSession>>,
    settings: Mutex<settings::Settings>,
    settings_path: PathBuf,
    normal_memory: bool,
    profile_memory: bool,
    profiling_started: AtomicBool,
    creating_window: AtomicBool,
    tray_ready: AtomicBool,
}

fn main() {
    if let Err(error) = run() {
        desktop::report_error(&format!("YouTube Music could not start: {error}"));
        std::process::exit(1);
    }
}

fn run() -> tauri::Result<()> {
    let context = tauri::generate_context!();
    identity::set_process_id(&context.config().identifier).map_err(std::io::Error::other)?;
    let normal_memory = std::env::args().any(|arg| arg == "--normal-memory");
    let profile_memory = std::env::args().any(|arg| arg == "--profile-memory");
    let startup = std::env::args().any(|arg| arg == "--startup");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if !args.iter().any(|arg| arg == "--startup") {
                desktop::open(app);
            }
        }))
        .on_window_event(|window, event| {
            if window.label() != "music" {
                return;
            }
            let state = window.state::<AppState>();
            match event {
                WindowEvent::Destroyed => {
                    state.media.lock().unwrap().take();
                }
                WindowEvent::CloseRequested { api, .. } => {
                    let close_to_tray = state.settings.lock().unwrap().close_to_tray;
                    if close_to_tray && state.tray_ready.load(Ordering::Relaxed) {
                        match window.hide() {
                            Ok(()) => {
                                api.prevent_close();
                                if !state.normal_memory
                                    && let Some(webview) =
                                        window.app_handle().get_webview_window("music")
                                {
                                    memory::set_background(&webview, true);
                                }
                            }
                            Err(error) => desktop::report_error(&format!(
                                "Could not hide YouTube Music: {error}"
                            )),
                        }
                    }
                }
                WindowEvent::Focused(focused) if !state.normal_memory => {
                    if let Some(webview) = window.app_handle().get_webview_window("music") {
                        memory::set_background(&webview, !focused);
                    }
                }
                _ => {}
            }
        })
        .setup(move |app| {
            let directory = app.path().local_data_dir()?.join("Ferric");
            std::fs::create_dir_all(&directory)?;
            if let Err(error) = startup::refresh_registration() {
                desktop::report_error(&format!(
                    "Could not update the Windows startup path: {error}"
                ));
            }
            if let Err(error) = identity::register(
                &app.config().identifier,
                app.config()
                    .product_name
                    .as_deref()
                    .unwrap_or("YouTube Music"),
                &directory,
            ) {
                desktop::report_error(&format!("Could not register the Windows app name: {error}"));
            }
            let settings_path = directory.join("settings.json");
            let settings = settings::Settings::load(&settings_path).unwrap_or_else(|error| {
                desktop::report_error(&format!(
                    "Could not read settings. Using defaults for this session: {error}"
                ));
                settings::Settings::default()
            });
            app.manage(AppState {
                data_directory: directory,
                media: Mutex::new(None),
                settings: Mutex::new(settings),
                settings_path,
                normal_memory,
                profile_memory,
                profiling_started: AtomicBool::new(false),
                creating_window: AtomicBool::new(false),
                tray_ready: AtomicBool::new(false),
            });
            match desktop::build_tray(app.handle()) {
                Ok(()) => app
                    .state::<AppState>()
                    .tray_ready
                    .store(true, Ordering::Relaxed),
                Err(error) => {
                    desktop::report_error(&format!("The system tray is unavailable: {error}"))
                }
            }
            if !startup || !app.state::<AppState>().tray_ready.load(Ordering::Relaxed) {
                app.state::<AppState>()
                    .creating_window
                    .store(true, Ordering::Relaxed);
                desktop::create_window(app.handle())?;
                app.state::<AppState>()
                    .creating_window
                    .store(false, Ordering::Relaxed);
            }
            Ok(())
        })
        .run(context)
}

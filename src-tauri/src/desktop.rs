use crate::{AppState, diagnostics, media, memory, startup};
use std::sync::atomic::Ordering;
use tauri::{
    AppHandle, Manager, WebviewUrl, WebviewWindowBuilder,
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::PageLoadEvent,
};
use windows::{
    Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
    core::HSTRING,
};

pub fn report_error(message: &str) {
    eprintln!("{message}");
    let message = HSTRING::from(message);
    // I use a native dialog so an error does not need another WebView.
    unsafe {
        MessageBoxW(
            None,
            &message,
            &HSTRING::from("YouTube Music"),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub fn create_window(app: &AppHandle) -> tauri::Result<()> {
    let profile = app.state::<AppState>().data_directory.join("webview");
    std::fs::create_dir_all(&profile)?;
    let window = WebviewWindowBuilder::new(
        app,
        "music",
        WebviewUrl::External("https://music.youtube.com".parse().unwrap()),
    )
    .title("YouTube Music")
    .inner_size(1200.0, 800.0)
    .min_inner_size(800.0, 600.0)
    .data_directory(profile)
    .additional_browser_args("--disable-features=HardwareMediaKeyHandling")
    .initialization_script(include_str!("media.js"))
    .on_navigation(|url| url.scheme() == "https")
    .on_page_load(|window, payload| {
        let state = window.state::<AppState>();
        if payload.event() == PageLoadEvent::Started {
            if let Some(session) = state.media.lock().unwrap().as_ref() {
                session.clear();
            }
        } else {
            let _ = window.eval("window.__ferricMediaRefresh?.()");
        }
        if !state.normal_memory && payload.event() == PageLoadEvent::Finished {
            memory::set_background(&window, !window.is_focused().unwrap_or(true));
        }
        if state.profile_memory
            && payload.event() == PageLoadEvent::Finished
            && payload.url().host_str() == Some("music.youtube.com")
            && !state.profiling_started.swap(true, Ordering::Relaxed)
        {
            diagnostics::start(window.clone());
        }
    })
    .build()?;
    match media::attach(&window) {
        Ok(session) => *app.state::<AppState>().media.lock().unwrap() = Some(session),
        Err(error) => report_error(&format!(
            "Could not initialize Windows media controls: {error}"
        )),
    }
    let _ = window.eval("window.__ferricMediaRefresh?.()");
    Ok(())
}

pub fn open(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("music") {
        let result = window
            .unminimize()
            .and_then(|()| window.show())
            .and_then(|()| window.set_focus());
        if let Err(error) = result {
            report_error(&format!("Could not open YouTube Music: {error}"));
        }
        return;
    }
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if state.creating_window.swap(true, Ordering::Relaxed) {
        return;
    }
    let app = app.clone();
    // I build a lazy WebView off the event callback to avoid a Windows event-loop deadlock.
    std::thread::spawn(move || {
        if let Err(error) = create_window(&app) {
            report_error(&format!("Could not open YouTube Music: {error}"));
        }
        app.state::<AppState>()
            .creating_window
            .store(false, Ordering::Relaxed);
    });
}

pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open_item = MenuItem::with_id(app, "open", "Open YouTube Music", true, None::<&str>)?;
    let startup_enabled = startup::enabled().unwrap_or_else(|error| {
        report_error(&format!("Could not read Windows startup settings: {error}"));
        false
    });
    let startup_item = CheckMenuItem::with_id(
        app,
        "startup",
        "Run on Windows startup",
        true,
        startup_enabled,
        None::<&str>,
    )?;
    let close_item = CheckMenuItem::with_id(
        app,
        "close-to-tray",
        "Close to tray",
        true,
        app.state::<AppState>()
            .settings
            .lock()
            .unwrap()
            .close_to_tray,
        None::<&str>,
    )?;
    let settings_menu = Submenu::with_items(app, "Settings", true, &[&startup_item, &close_item])?;
    let exit_item = MenuItem::with_id(app, "exit", "Exit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open_item, &settings_menu, &separator, &exit_item])?;
    let app_menu = Submenu::with_items(app, "YouTube Music", true, &[&exit_item])?;
    app.set_menu(Menu::with_items(app, &[&app_menu, &settings_menu])?)?;

    TrayIconBuilder::with_id("ferric")
        .icon(Image::new(include_bytes!("../icons/tray.rgba"), 32, 32))
        .tooltip("YouTube Music")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => open(app),
            "exit" => app.exit(0),
            "startup" => {
                let result = startup::enabled().and_then(|enabled| {
                    startup::set_enabled(!enabled)?;
                    Ok(!enabled)
                });
                match result {
                    Ok(enabled) => {
                        let _ = startup_item.set_checked(enabled);
                    }
                    Err(error) => {
                        report_error(&format!("Could not change Windows startup: {error}"))
                    }
                }
            }
            "close-to-tray" => {
                let result = {
                    let state = app.state::<AppState>();
                    let mut settings = state.settings.lock().unwrap();
                    let mut updated = settings.clone();
                    updated.close_to_tray = !updated.close_to_tray;
                    updated.save(&state.settings_path).map(|()| {
                        *settings = updated;
                        settings.close_to_tray
                    })
                };
                match result {
                    Ok(enabled) => {
                        let _ = close_item.set_checked(enabled);
                    }
                    Err(error) => report_error(&format!("Could not save settings: {error}")),
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                open(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

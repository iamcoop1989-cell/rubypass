#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod gateway;
mod network_watch;
mod routing;
mod scheduler;
mod status;
mod updater;

use commands::AppState;
use std::sync::Mutex;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

fn main() {
    env_logger::init();
    let cfg = config::load();

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(AppState(Mutex::new(cfg)))
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::get_config,
            commands::toggle_bypass,
            commands::update_subnets,
            commands::set_autostart,
            commands::set_update_schedule,
        ])
        .setup(|app| {
            setup_tray(app)?;
            setup_first_launch(app);
            network_watch::start(app.handle().clone());
            scheduler::start(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                window.hide().unwrap();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error running RuBypass");
}

fn setup_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Открыть", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "Включить", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выйти", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &toggle, &quit])?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .on_tray_icon_event(|tray, event| {
            // On macOS DoubleClick is not emitted; use Click instead.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
            "toggle" => {
                let state = app.state::<AppState>();
                let _ = commands::toggle_bypass(app.clone(), state);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn setup_first_launch(app: &mut tauri::App) {
    let is_first = !updater::subnet_file().exists();
    let handle = app.handle().clone();

    if is_first {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.show();
        }
        std::thread::spawn(move || {
            if let Ok(count) = updater::download_and_save() {
                let state = handle.state::<commands::AppState>();
                {
                    let mut cfg = state.0.lock().unwrap();
                    cfg.last_updated = Some(
                        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
                    );
                    let _ = config::save(&cfg);
                }
                log::info!("Downloaded {} subnets, enabling bypass", count);
                let _ = commands::enable_bypass_inner(&handle, &state);
            }
        });
    } else {
        std::thread::spawn(move || {
            let state = handle.state::<commands::AppState>();
            let enabled = state.0.lock().unwrap().bypass_enabled;
            if enabled {
                let _ = commands::enable_bypass_inner(&handle, &state);
            }
        });
    }
}

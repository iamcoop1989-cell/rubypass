#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod gateway;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod helper;
mod network_watch;
#[cfg(target_os = "windows")]
mod pac;
mod routing;
mod scheduler;
mod status;
mod updater;

use commands::AppState;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

fn main() {
    let cfg = config::load();

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::new(cfg))
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::get_config,
            commands::get_app_version,
            commands::toggle_bypass,
            commands::update_subnets,
            commands::set_autostart,
            commands::set_update_schedule,
            commands::toggle_windows_proxy_alpha,
            commands::clear_all_routes,
        ])
        .setup(|app| {
            setup_tray(app)?;
            setup_first_launch(app);
            network_watch::start(app.handle().clone());
            scheduler::start(app.handle().clone());
            let update_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Delay so the app finishes launching before showing a dialog
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                check_for_updates(update_handle).await;
            });
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

/// Show the main window, reloading its content if the WebView failed to load
/// (can happen when the app is launched as a LaunchAgent before the system
/// WebView is fully initialised on macOS).
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
        // If Tauri's JS bridge isn't present the page is blank — reload it.
        let _ = win.eval(
            "if (!window.__TAURI__) { window.location.reload(); }"
        );
    }
}

fn setup_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Открыть", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "Включить", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выйти", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &toggle, &quit])?;

    let initial_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon-red.png"))
        .expect("icon-red.png is a valid RGBA PNG");

    TrayIconBuilder::with_id("main")
        .icon(initial_icon)
        .menu(&menu)
        .on_tray_icon_event(|tray, event| {
            // On macOS DoubleClick is not emitted; use Click instead.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => {
                show_main_window(app);
            }
            "toggle" => {
                let state = app.state::<AppState>();
                let _ = commands::toggle_bypass(app.clone(), state);
            }
            "quit" => {
                use tauri_plugin_dialog::DialogExt;
                let bypass_enabled = app
                    .state::<AppState>()
                    .0.lock().unwrap()
                    .config.bypass_enabled;

                if bypass_enabled {
                    let app2 = app.clone();
                    app.dialog()
                        .message("Маршруты для обхода блокировок останутся активными после выхода.\nОчистить перед выходом?")
                        .title("RuBypass")
                        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom(
                            "Очистить и выйти".to_string(),
                            "Просто выйти".to_string(),
                        ))
                        .show(move |clear| {
                            #[cfg(target_os = "windows")]
                            let _ = pac::restore();
                            if clear {
                                let state = app2.state::<AppState>();
                                let _ = commands::disable_bypass_inner(&app2, &state);
                            }
                            app2.exit(0);
                        });
                } else {
                    #[cfg(target_os = "windows")]
                    let _ = pac::restore();
                    app.exit(0);
                }
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

async fn check_for_updates(app: tauri::AppHandle) {
    use tauri_plugin_updater::UpdaterExt;
    let update = match app.updater() {
        Ok(u) => match u.check().await {
            Ok(Some(u)) => u,
            _ => return,
        },
        Err(_) => return,
    };
    use tauri_plugin_dialog::DialogExt;
    let install = app
        .dialog()
        .message(format!(
            "Доступна новая версия {}.\nУстановить сейчас?",
            update.version
        ))
        .title("Обновление RuBypass")
        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom(
            "Установить".to_string(),
            "Позже".to_string(),
        ))
        .blocking_show();
    if install {
        if let Err(e) = update.download_and_install(|_, _| {}, || {}).await {
            log::warn!("Update failed: {e}");
        } else {
            app.restart();
        }
    }
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
                    let mut inner = state.0.lock().unwrap();
                    inner.config.last_updated = Some(
                        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
                    );
                    let _ = config::save(&inner.config);
                }
                log::info!("Downloaded {} subnets, enabling bypass", count);
                let _ = commands::enable_bypass_inner(&handle, &state);
            }
        });
    } else {
        std::thread::spawn(move || {
            let state = handle.state::<commands::AppState>();
            let enabled = state.0.lock().unwrap().config.bypass_enabled;
            if enabled {
                let _ = commands::enable_bypass_inner(&handle, &state);
            }
        });
    }
}

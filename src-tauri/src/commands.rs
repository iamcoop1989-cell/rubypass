// src-tauri/src/commands.rs
use crate::{config, routing, status, updater};
use std::sync::Mutex;
use tauri::{AppHandle, State};

pub struct AppState(pub Mutex<crate::config::Config>);

#[tauri::command]
pub fn get_status(state: State<AppState>) -> status::AppStatus {
    let cfg = state.0.lock().unwrap();
    status::collect(cfg.bypass_enabled, cfg.last_updated.clone())
}

#[tauri::command]
pub fn get_config(state: State<AppState>) -> crate::config::Config {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
pub fn toggle_bypass(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let enabled = state.0.lock().unwrap().bypass_enabled;
    if enabled {
        disable_bypass_inner(&app, &state)
    } else {
        enable_bypass_inner(&app, &state)
    }
}

#[tauri::command]
pub fn update_subnets(app: AppHandle, state: State<AppState>) -> Result<usize, String> {
    let count = updater::download_and_save()?;
    {
        let mut cfg = state.0.lock().unwrap();
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        cfg.last_updated = Some(now);
        config::save(&cfg)?;
    }
    let was_enabled = state.0.lock().unwrap().bypass_enabled;
    if was_enabled {
        disable_bypass_inner(&app, &state)?;
        enable_bypass_inner(&app, &state)?;
    }
    Ok(count)
}

#[tauri::command]
pub fn set_autostart(
    enabled: bool,
    app: AppHandle,
    state: State<AppState>,
) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let mut cfg = state.0.lock().unwrap();
    cfg.autostart = enabled;
    config::save(&cfg)?;
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())
    } else {
        manager.disable().map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn set_update_schedule(
    schedule: config::UpdateSchedule,
    state: State<AppState>,
) -> Result<(), String> {
    let mut cfg = state.0.lock().unwrap();
    cfg.update_schedule = schedule;
    config::save(&cfg)
}

pub fn enable_bypass_inner(
    app: &AppHandle,
    state: &State<AppState>,
) -> Result<(), String> {
    set_tray_icon(app, TrayState::Loading);

    // Clone what we need, then drop the lock before the slow part
    let (subnets, gateway) = {
        let cfg = state.0.lock().unwrap();
        let gw = cfg.gateway_override.clone()
            .unwrap_or_else(|| crate::gateway::detect().unwrap_or_default());
        let subnets = updater::load_subnets()?;
        (subnets, gw)
    };

    if gateway.is_empty() {
        set_tray_icon(app, TrayState::Inactive);
        return Err("Не удалось определить шлюз. Проверьте подключение к роутеру.".to_string());
    }

    // Lock is released here — route operations run without holding it
    let added = routing::add_routes(&subnets, &gateway);
    if added == 0 && !subnets.is_empty() {
        log::warn!("add_routes returned 0 successes for {} subnets", subnets.len());
    }

    // Re-lock to update state
    let mut cfg = state.0.lock().unwrap();
    cfg.bypass_enabled = true;
    config::save(&cfg)?;
    set_tray_icon(app, TrayState::Active);
    Ok(())
}

pub fn disable_bypass_inner(
    app: &AppHandle,
    state: &State<AppState>,
) -> Result<(), String> {
    set_tray_icon(app, TrayState::Loading);

    let (subnets, gateway) = {
        let cfg = state.0.lock().unwrap();
        let gw = cfg.gateway_override.clone()
            .unwrap_or_else(|| crate::gateway::detect().unwrap_or_default());
        let subnets = updater::load_subnets().unwrap_or_default();
        (subnets, gw)
    };

    routing::remove_routes(&subnets, &gateway);

    let mut cfg = state.0.lock().unwrap();
    cfg.bypass_enabled = false;
    config::save(&cfg)?;
    set_tray_icon(app, TrayState::Inactive);
    Ok(())
}

pub enum TrayState {
    Active,
    Inactive,
    Loading,
}

pub fn set_tray_icon(app: &AppHandle, state: TrayState) {
    if let Some(tray) = app.tray_by_id("main") {
        let icon_bytes: &[u8] = match state {
            TrayState::Active => include_bytes!("../icons/icon-green.png"),
            TrayState::Inactive => include_bytes!("../icons/icon-red.png"),
            TrayState::Loading => include_bytes!("../icons/icon-yellow.png"),
        };
        if let Ok(icon) = tauri::image::Image::from_bytes(icon_bytes) {
            let _ = tray.set_icon(Some(icon));
        }
    }
}

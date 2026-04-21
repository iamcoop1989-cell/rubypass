use crate::{config, routing, status, updater};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};

pub struct AppState(pub Mutex<AppStateInner>);

pub struct AppStateInner {
    pub config: crate::config::Config,
    /// Subnets cached in memory — avoids disk reads on every network change.
    pub subnets_cache: Option<Vec<String>>,
}

impl AppState {
    pub fn new(config: crate::config::Config) -> Self {
        AppState(Mutex::new(AppStateInner { config, subnets_cache: None }))
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Load subnets from cache if available, otherwise from disk (and cache them).
fn load_subnets_cached(inner: &mut AppStateInner) -> Result<Vec<String>, String> {
    if let Some(ref cached) = inner.subnets_cache {
        return Ok(cached.clone());
    }
    let subnets = updater::load_subnets()?;
    inner.subnets_cache = Some(subnets.clone());
    Ok(subnets)
}

// ── commands ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_status(state: State<AppState>) -> status::AppStatus {
    let inner = state.0.lock().unwrap();
    status::collect(inner.config.bypass_enabled, inner.config.last_updated.clone())
}

#[tauri::command]
pub fn get_config(state: State<AppState>) -> crate::config::Config {
    state.0.lock().unwrap().config.clone()
}

#[tauri::command]
pub async fn toggle_bypass(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let enabled = state.0.lock().unwrap().config.bypass_enabled;
    let app2 = app.clone();
    let result = tokio::task::spawn_blocking(move || {
        let state2 = app2.state::<AppState>();
        if enabled {
            disable_bypass_inner(&app2, &state2)
        } else {
            enable_bypass_inner(&app2, &state2)
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    result
}

#[tauri::command]
pub async fn update_subnets(app: AppHandle, _state: State<'_, AppState>) -> Result<usize, String> {
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || {
        let count = updater::download_and_save()?;
        let state2 = app2.state::<AppState>();
        let was_enabled = {
            let mut inner = state2.0.lock().unwrap();
            let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            inner.config.last_updated = Some(now);
            config::save(&inner.config)?;
            // Invalidate cache so fresh subnets are loaded on next enable
            inner.subnets_cache = None;
            inner.config.bypass_enabled
        };
        if was_enabled {
            disable_bypass_inner(&app2, &state2)?;
            enable_bypass_inner(&app2, &state2)?;
        }
        Ok(count)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn set_autostart(enabled: bool, app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let mut inner = state.0.lock().unwrap();
    inner.config.autostart = enabled;
    config::save(&inner.config)?;
    drop(inner);

    #[cfg(target_os = "windows")]
    {
        // requireAdministrator in the manifest breaks registry-based autostart
        // (Windows won't auto-elevate Run key entries). Use Task Scheduler instead.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let exe_path = exe.to_string_lossy().to_string();
            let status = std::process::Command::new("schtasks")
                .args([
                    "/create", "/tn", "RuBypass",
                    "/tr", &exe_path,
                    "/sc", "onlogon",
                    "/rl", "highest",
                    "/f",
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err("Не удалось создать задачу автозапуска".to_string());
            }
        } else {
            let _ = std::process::Command::new("schtasks")
                .args(["/delete", "/tn", "RuBypass", "/f"])
                .creation_flags(CREATE_NO_WINDOW)
                .status();
        }
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        use tauri_plugin_autostart::ManagerExt;
        let manager = app.autolaunch();
        if enabled {
            manager.enable().map_err(|e| e.to_string())
        } else {
            manager.disable().map_err(|e| e.to_string())
        }
    }
}

#[tauri::command]
pub fn set_update_schedule(
    schedule: config::UpdateSchedule,
    state: State<AppState>,
) -> Result<(), String> {
    let mut inner = state.0.lock().unwrap();
    inner.config.update_schedule = schedule;
    config::save(&inner.config)
}

/// Remove every route we may have installed, regardless of current bypass state.
/// Useful after crashes or repeated testing left stale routes behind.
#[tauri::command]
pub async fn clear_all_routes(app: AppHandle, state: State<'_, AppState>) -> Result<usize, String> {
    let app2 = app.clone();
    tokio::task::spawn_blocking(move || {
        let state2 = app2.state::<AppState>();
        let gateway = {
            let inner = state2.0.lock().unwrap();
            inner.config.gateway_override.clone()
                .unwrap_or_else(|| crate::gateway::detect().unwrap_or_default())
        };
        if gateway.is_empty() {
            return Err("Не удалось определить шлюз".to_string());
        }
        let live = crate::routing::routes_via_gateway(&gateway);
        let removed = crate::routing::remove_routes(&live, &gateway);
        // Mark as disabled since all routes are gone.
        let mut inner = state2.0.lock().unwrap();
        inner.config.bypass_enabled = false;
        let _ = config::save(&inner.config);
        set_tray_icon(&app2, TrayState::Inactive);
        Ok(removed)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── bypass logic ─────────────────────────────────────────────────────────────

pub fn enable_bypass_inner(app: &AppHandle, state: &State<AppState>) -> Result<(), String> {
    start_spinner(app.clone());

    let (subnets, gateway) = {
        let mut inner = state.0.lock().unwrap();
        let gw = inner.config.gateway_override.clone()
            .unwrap_or_else(|| crate::gateway::detect().unwrap_or_default());
        let subnets = load_subnets_cached(&mut inner)?;
        (subnets, gw)
    };

    if gateway.is_empty() {
        stop_spinner();
        set_tray_icon(app, TrayState::Inactive);
        return Err("Не удалось определить шлюз. Проверьте подключение к роутеру.".to_string());
    }

    let added = routing::add_routes(&subnets, &gateway);
    log::info!("enable_bypass: added {}/{} routes via {}", added, subnets.len(), gateway);
    if added == 0 && !subnets.is_empty() {
        stop_spinner();
        set_tray_icon(app, TrayState::Inactive);
        return Err(format!(
            "Не удалось добавить маршруты (шлюз: {}). Проверьте логи.",
            gateway
        ));
    }

    let mut inner = state.0.lock().unwrap();
    inner.config.bypass_enabled = true;
    config::save(&inner.config)?;
    stop_spinner();
    set_tray_icon(app, TrayState::Active);
    Ok(())
}

pub fn disable_bypass_inner(app: &AppHandle, state: &State<AppState>) -> Result<(), String> {
    start_spinner(app.clone());

    let (subnets, gateway) = {
        let mut inner = state.0.lock().unwrap();
        let gw = inner.config.gateway_override.clone()
            .unwrap_or_else(|| crate::gateway::detect().unwrap_or_default());
        let subnets = load_subnets_cached(&mut inner).unwrap_or_default();
        (subnets, gw)
    };

    routing::remove_routes(&subnets, &gateway);

    let mut inner = state.0.lock().unwrap();
    inner.config.bypass_enabled = false;
    config::save(&inner.config)?;
    stop_spinner();
    set_tray_icon(app, TrayState::Inactive);
    Ok(())
}

/// On network change: update existing routes to new gateway instead of
/// full delete + re-add (twice as fast).
pub fn reapply_bypass_inner(
    app: &AppHandle,
    state: &State<AppState>,
    old_gateway: &str,
) -> Result<(), String> {
    start_spinner(app.clone());

    let (subnets, new_gateway) = {
        let mut inner = state.0.lock().unwrap();
        let gw = inner.config.gateway_override.clone()
            .unwrap_or_else(|| crate::gateway::detect().unwrap_or_default());
        let subnets = load_subnets_cached(&mut inner)?;
        (subnets, gw)
    };

    if new_gateway.is_empty() {
        stop_spinner();
        return Err("Не удалось определить шлюз".to_string());
    }

    routing::change_routes(&subnets, old_gateway, &new_gateway);

    stop_spinner();
    set_tray_icon(app, TrayState::Active);
    Ok(())
}

// ── tray icon ────────────────────────────────────────────────────────────────

pub enum TrayState {
    Active,
    Inactive,
    #[allow(dead_code)]
    Loading,
}

pub fn set_tray_icon(app: &AppHandle, state: TrayState) {
    let bytes: &'static [u8] = match state {
        TrayState::Active   => include_bytes!("../icons/icon-green.png"),
        TrayState::Inactive => include_bytes!("../icons/icon-red.png"),
        TrayState::Loading  => include_bytes!("../icons/spinner_0.png"),
    };
    let runner = app.clone();
    let inner  = app.clone();
    let _ = runner.run_on_main_thread(move || {
        if let Some(tray) = inner.tray_by_id("main") {
            if let Ok(icon) = tauri::image::Image::from_bytes(bytes) {
                let _ = tray.set_icon(Some(icon));
            }
        }
    });
}

static SPINNER_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn start_spinner(app: AppHandle) {
    use std::sync::atomic::Ordering;
    if SPINNER_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        const FRAMES: &[&'static [u8]] = &[
            include_bytes!("../icons/spinner_0.png"),
            include_bytes!("../icons/spinner_1.png"),
            include_bytes!("../icons/spinner_2.png"),
            include_bytes!("../icons/spinner_3.png"),
            include_bytes!("../icons/spinner_4.png"),
            include_bytes!("../icons/spinner_5.png"),
            include_bytes!("../icons/spinner_6.png"),
            include_bytes!("../icons/spinner_7.png"),
        ];
        let mut frame = 0usize;
        while SPINNER_RUNNING.load(Ordering::SeqCst) {
            let bytes = FRAMES[frame];
            let runner = app.clone();
            let inner  = app.clone();
            let _ = runner.run_on_main_thread(move || {
                if let Some(tray) = inner.tray_by_id("main") {
                    if let Ok(icon) = tauri::image::Image::from_bytes(bytes) {
                        let _ = tray.set_icon(Some(icon));
                    }
                }
            });
            frame = (frame + 1) % FRAMES.len();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
}

pub fn stop_spinner() {
    SPINNER_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
}

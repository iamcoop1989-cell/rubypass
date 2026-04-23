// src-tauri/src/network_watch.rs
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        poll_loop(app);
    });
}

fn poll_loop(app: AppHandle) {
    let mut last_gateway = crate::gateway::detect().ok();
    // Debounce: don't reapply routes more than once per 30 seconds.
    // On Windows, DHCP init causes many rapid gateway changes at startup.
    let mut last_reapply = Instant::now() - Duration::from_secs(60);

    loop {
        std::thread::sleep(Duration::from_secs(10));

        let current_gateway = crate::gateway::detect().ok();

        if current_gateway != last_gateway {
            log::info!(
                "Gateway changed: {:?} → {:?}",
                last_gateway,
                current_gateway
            );
            let old = last_gateway.take();
            last_gateway = current_gateway;

            std::thread::sleep(Duration::from_secs(3));

            if last_reapply.elapsed() >= Duration::from_secs(30) {
                last_reapply = Instant::now();
                handle_network_change(&app, old);
            } else {
                log::info!("Gateway change debounced (last reapply was <30s ago)");
            }
        }
    }
}

fn handle_network_change(app: &AppHandle, old_gateway: Option<String>) {
    use crate::commands::AppState;

    let state = app.state::<AppState>();
    let bypass_enabled = state.0.lock().unwrap().config.bypass_enabled;

    if !bypass_enabled {
        return;
    }

    log::info!("Network changed, reapplying routes");

    // Use change_routes (faster) if we know the old gateway, else full disable+enable.
    let result = if let Some(ref old_gw) = old_gateway {
        crate::commands::reapply_bypass_inner(app, &state, old_gw)
    } else {
        crate::commands::disable_bypass_inner(app, &state)
            .and_then(|_| crate::commands::enable_bypass_inner(app, &state))
    };

    if let Err(e) = result {
        log::warn!("Failed to reapply routes: {}", e);
    }

    let _ = app.emit("network-changed", ());
}

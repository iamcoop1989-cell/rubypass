// src-tauri/src/scheduler.rs
use crate::config::UpdateSchedule;
use chrono::{DateTime, Duration, Utc};
use tauri::{AppHandle, Manager};

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        loop {
            // Check every hour whether an update is due
            std::thread::sleep(std::time::Duration::from_secs(3600));
            let state = app.state::<crate::commands::AppState>();
            let cfg = state.0.lock().unwrap().clone();
            drop(state);

            if should_update(&cfg) {
                log::info!("Scheduled update triggered");
                if let Ok(count) = crate::updater::download_and_save() {
                    let state2 = app.state::<crate::commands::AppState>();
                    {
                        let mut cfg2 = state2.0.lock().unwrap();
                        cfg2.last_updated = Some(
                            Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
                        );
                        let _ = crate::config::save(&cfg2);
                    }
                    log::info!("Scheduled update done: {} subnets", count);
                    let bypass_enabled = state2.0.lock().unwrap().bypass_enabled;
                    if bypass_enabled {
                        let _ = crate::commands::disable_bypass_inner(&app, &state2);
                        let _ = crate::commands::enable_bypass_inner(&app, &state2);
                    }
                }
            }
        }
    });
}

pub fn should_update(cfg: &crate::config::Config) -> bool {
    // Never schedule always wins — short-circuit before checking last_updated.
    if cfg.update_schedule == UpdateSchedule::Never {
        return false;
    }

    let last = match &cfg.last_updated {
        None => return true,
        Some(s) => match s.parse::<DateTime<Utc>>() {
            Ok(dt) => dt,
            Err(_) => return true,
        },
    };
    let threshold = match cfg.update_schedule {
        UpdateSchedule::Never   => return false,
        UpdateSchedule::Daily   => Duration::days(1),
        UpdateSchedule::Weekly  => Duration::weeks(1),
        UpdateSchedule::Monthly => Duration::days(30),
    };
    Utc::now() > last + threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, UpdateSchedule};

    fn cfg_with(schedule: UpdateSchedule, last: Option<&str>) -> Config {
        Config {
            bypass_enabled: false,
            autostart: false,
            update_schedule: schedule,
            last_updated: last.map(str::to_string),
            gateway_override: None,
        }
    }

    #[test]
    fn test_never_never_updates() {
        let cfg = cfg_with(UpdateSchedule::Never, None);
        assert!(!should_update(&cfg));
    }

    #[test]
    fn test_updates_when_no_last_updated() {
        let cfg = cfg_with(UpdateSchedule::Weekly, None);
        assert!(should_update(&cfg));
    }

    #[test]
    fn test_weekly_not_due_if_recent() {
        let recent = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let cfg = cfg_with(UpdateSchedule::Weekly, Some(&recent));
        assert!(!should_update(&cfg));
    }

    #[test]
    fn test_weekly_due_if_old() {
        let old = (Utc::now() - Duration::days(8))
            .format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let cfg = cfg_with(UpdateSchedule::Weekly, Some(&old));
        assert!(should_update(&cfg));
    }
}

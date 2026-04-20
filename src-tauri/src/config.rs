// src-tauri/src/config.rs
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub bypass_enabled: bool,
    pub autostart: bool,
    pub update_schedule: UpdateSchedule,
    pub last_updated: Option<String>,  // ISO 8601
    pub gateway_override: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum UpdateSchedule {
    Never,
    Daily,
    Weekly,
    Monthly,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bypass_enabled: false,
            autostart: false,
            update_schedule: UpdateSchedule::Weekly,
            last_updated: None,
            gateway_override: None,
        }
    }
}

pub fn config_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".rubypass").join("config.json")
}

pub fn data_dir() -> PathBuf {
    config_path().parent().unwrap().to_path_buf()
}

pub fn load() -> Config {
    let path = config_path();
    if !path.exists() {
        return Config::default();
    }
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    serde_json::from_str(&content).unwrap_or_default()
}

pub fn save(config: &Config) -> Result<(), String> {
    let path = config_path();
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // Serialise tests that mutate the HOME env var so they don't race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[allow(deprecated)]
    fn temp_home() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        env::set_var("HOME", dir.path());
        (dir, guard)
    }

    #[test]
    fn test_default_config_loads_when_missing() {
        let (_dir, _guard) = temp_home();
        let cfg = load();
        assert!(!cfg.bypass_enabled);
        assert_eq!(cfg.update_schedule, UpdateSchedule::Weekly);
    }

    #[test]
    fn test_save_and_reload() {
        let (_dir, _guard) = temp_home();
        let mut cfg = Config::default();
        cfg.bypass_enabled = true;
        cfg.autostart = true;
        save(&cfg).unwrap();

        let reloaded = load();
        assert!(reloaded.bypass_enabled);
        assert!(reloaded.autostart);
    }

    #[test]
    fn test_invalid_json_falls_back_to_default() {
        let (_dir, _guard) = temp_home();
        let path = config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        let cfg = load();
        assert!(!cfg.bypass_enabled);
    }
}

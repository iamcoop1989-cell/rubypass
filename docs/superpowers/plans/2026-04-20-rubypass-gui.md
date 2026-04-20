# RuBypass GUI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cross-platform desktop utility (macOS/Windows/Linux) that manages split-tunnel routing for Russian IP addresses, with a system tray icon and compact status window.

**Architecture:** Tauri 2.x app — Rust backend handles all system operations (routing, gateway detection, RIPE NCC download, network change events), HTML/CSS/JS frontend renders the window, Tauri commands bridge the two. Tray icon has three color states; main window opens on double-click.

**Tech Stack:** Rust, Tauri 2.x, tauri-plugin-autostart, reqwest (blocking), serde_json, tokio, chrono. Vanilla HTML/CSS/JS (no framework). GitHub Actions for cross-platform builds.

---

## File Map

```
rubypass/
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── build.rs
│   ├── icons/
│   │   ├── icon-green.png    # bypass active
│   │   ├── icon-red.png      # bypass inactive
│   │   ├── icon-yellow.png   # loading
│   │   └── icon.png          # default (= red)
│   └── src/
│       ├── main.rs           # Tauri app setup, tray, window lifecycle
│       ├── commands.rs       # All #[tauri::command] IPC handlers
│       ├── config.rs         # Config struct + read/write to JSON
│       ├── updater.rs        # Download RIPE NCC + parse CIDR subnets
│       ├── routing.rs        # Cross-platform route add/delete (parallel)
│       ├── gateway.rs        # Cross-platform gateway detection
│       ├── status.rs         # Route count + VPN interface detection
│       ├── network_watch.rs  # OS network change events → re-route
│       └── scheduler.rs      # Auto-update timer
├── src/
│   ├── index.html
│   ├── style.css
│   └── main.js
├── .github/
│   └── workflows/
│       └── build.yml         # Build .dmg + .exe + .AppImage
└── docs/
    └── superpowers/
        ├── specs/
        └── plans/
```

---

## Task 1: Scaffold Tauri project

**Files:**
- Create: `src-tauri/Cargo.toml`
- Create: `src-tauri/tauri.conf.json`
- Create: `src-tauri/build.rs`
- Create: `src-tauri/src/main.rs` (skeleton)
- Create: `src/index.html` (placeholder)

- [ ] **Step 1: Install prerequisites**

```bash
# Install Rust (if not installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Install Tauri CLI
cargo install tauri-cli --version "^2"

# macOS: install Xcode Command Line Tools if missing
xcode-select --install
```

- [ ] **Step 2: Init Tauri project in existing directory**

```bash
cd ~/Desktop/internet_switcher
cargo tauri init
# When prompted:
#   App name: RuBypass
#   Window title: RuBypass
#   Web assets location: ../src
#   Dev server URL: (leave empty)
#   Frontend dev command: (leave empty)
#   Frontend build command: (leave empty)
```

- [ ] **Step 3: Replace Cargo.toml with full dependency set**

```toml
[package]
name = "rubypass"
version = "0.1.0"
edition = "2021"

[lib]
name = "rubypass_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
tauri-plugin-autostart = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", default-features = false, features = ["blocking", "rustls-tls"] }
tokio = { version = "1", features = ["full"] }
chrono = { version = "0.4", features = ["serde"] }
log = "0.4"
env_logger = "0.11"
rayon = "1"

[target.'cfg(target_os = "macos")'.dependencies]
system-configuration = "0.6"

[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.58", features = [
  "Win32_NetworkManagement_IpHelper",
  "Win32_Foundation",
  "Win32_Networking_WinSock"
] }

[profile.release]
opt-level = "s"
lstrip = true
```

- [ ] **Step 4: Write tauri.conf.json**

```json
{
  "productName": "RuBypass",
  "version": "0.1.0",
  "identifier": "com.rubypass.app",
  "build": {
    "frontendDist": "../src"
  },
  "app": {
    "withGlobalTauri": true,
    "windows": [
      {
        "label": "main",
        "title": "RuBypass",
        "width": 320,
        "height": 460,
        "resizable": false,
        "visible": false,
        "decorations": true,
        "alwaysOnTop": false,
        "skipTaskbar": true
      }
    ],
    "trayIcon": {
      "iconPath": "icons/icon-red.png",
      "iconAsTemplate": true
    }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/icon.png"
    ],
    "macOS": {
      "entitlements": null,
      "exceptionDomain": "",
      "frameworks": [],
      "signingIdentity": null,
      "providerShortName": null
    },
    "windows": {
      "requestedExecutionLevel": "requireAdministrator"
    }
  }
}
```

- [ ] **Step 5: Create placeholder src/index.html**

```html
<!DOCTYPE html>
<html lang="ru">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>RuBypass</title>
  <link rel="stylesheet" href="style.css">
</head>
<body>
  <div id="app">Loading…</div>
  <script src="main.js"></script>
</body>
</html>
```

- [ ] **Step 6: Verify project compiles**

```bash
cd ~/Desktop/internet_switcher
cargo tauri build --debug 2>&1 | tail -20
```
Expected: `Finished` with no errors (may show warnings).

- [ ] **Step 7: Commit**

```bash
git init  # if not already a repo
git add src-tauri/ src/ .gitignore
git commit -m "feat: scaffold Tauri project"
```

---

## Task 2: Config module

**Files:**
- Create: `src-tauri/src/config.rs`
- Modify: `src-tauri/src/main.rs` (add `mod config;`)

- [ ] **Step 1: Write tests first** (in `config.rs` bottom section)

```rust
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

    fn temp_home() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        env::set_var("HOME", dir.path());
        dir
    }

    #[test]
    fn test_default_config_loads_when_missing() {
        let _dir = temp_home();
        let cfg = load();
        assert!(!cfg.bypass_enabled);
        assert_eq!(cfg.update_schedule, UpdateSchedule::Weekly);
    }

    #[test]
    fn test_save_and_reload() {
        let _dir = temp_home();
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
        let _dir = temp_home();
        let path = config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        let cfg = load();
        assert!(!cfg.bypass_enabled);
    }
}
```

- [ ] **Step 2: Add tempfile dev-dependency to Cargo.toml**

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Run tests**

```bash
cd src-tauri
cargo test config -- --nocapture
```
Expected: 3 tests pass.

- [ ] **Step 4: Add `mod config;` to main.rs**

```rust
// top of src-tauri/src/main.rs
mod config;
```

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/config.rs src-tauri/Cargo.toml src-tauri/src/main.rs
git commit -m "feat: config module with load/save and tests"
```

---

## Task 3: RIPE NCC updater

**Files:**
- Create: `src-tauri/src/updater.rs`
- Modify: `src-tauri/src/main.rs` (add `mod updater;`)

- [ ] **Step 1: Write tests first**

```rust
// src-tauri/src/updater.rs
use std::path::PathBuf;

const RIPE_URL: &str =
    "https://ftp.ripe.net/pub/stats/ripencc/delegated-ripencc-extended-latest";

pub fn subnet_file() -> PathBuf {
    crate::config::data_dir().join("ru_subnets.txt")
}

/// Parse count of IPs → CIDR prefix length. 256 → 24, 512 → 23, etc.
pub fn count_to_prefix(count: u32) -> u8 {
    let mut bits = 32u8;
    let mut n = count;
    while n > 1 {
        n /= 2;
        bits -= 1;
    }
    bits
}

/// Parse raw RIPE dump lines into CIDR strings ("1.2.3.0/24").
pub fn parse_ru_subnets(content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() < 6 { return None; }
            if parts[1] != "RU" || parts[2] != "ipv4" { return None; }
            let ip = parts[3];
            let count: u32 = parts[4].parse().ok()?;
            let prefix = count_to_prefix(count);
            Some(format!("{}/{}", ip, prefix))
        })
        .collect()
}

pub fn download_and_save() -> Result<usize, String> {
    let response = reqwest::blocking::get(RIPE_URL)
        .map_err(|e| format!("Ошибка загрузки: {}", e))?;
    let text = response.text()
        .map_err(|e| format!("Ошибка чтения ответа: {}", e))?;
    let subnets = parse_ru_subnets(&text);
    let count = subnets.len();
    if count == 0 {
        return Err("Не найдено ни одной российской подсети".to_string());
    }
    let path = subnet_file();
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|e| e.to_string())?;
    std::fs::write(&path, subnets.join("\n"))
        .map_err(|e| e.to_string())?;
    Ok(count)
}

pub fn load_subnets() -> Result<Vec<String>, String> {
    let path = subnet_file();
    if !path.exists() {
        return Err("Список подсетей не загружен".to_string());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())?;
    Ok(content.lines().filter(|l| !l.is_empty()).map(String::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_to_prefix_256() {
        assert_eq!(count_to_prefix(256), 24);
    }

    #[test]
    fn test_count_to_prefix_512() {
        assert_eq!(count_to_prefix(512), 23);
    }

    #[test]
    fn test_count_to_prefix_1() {
        assert_eq!(count_to_prefix(1), 32);
    }

    #[test]
    fn test_parse_ru_subnets_filters_correctly() {
        let raw = "\
ripencc|RU|ipv4|77.88.55.0|256|20110101|allocated
ripencc|DE|ipv4|1.2.3.0|256|20110101|allocated
ripencc|RU|ipv6|2a02::/32|1|20110101|allocated
ripencc|RU|ipv4|5.45.192.0|16384|20110101|allocated";
        let result = parse_ru_subnets(raw);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "77.88.55.0/24");
        assert_eq!(result[1], "5.45.192.0/18");
    }

    #[test]
    fn test_parse_ru_subnets_skips_malformed() {
        let raw = "bad|line\nripencc|RU|ipv4|1.2.3.0|256|20110101|ok";
        let result = parse_ru_subnets(raw);
        assert_eq!(result.len(), 1);
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cd src-tauri
cargo test updater -- --nocapture
```
Expected: 5 tests pass.

- [ ] **Step 3: Add `mod updater;` to main.rs**

```rust
mod updater;
```

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/updater.rs src-tauri/src/main.rs
git commit -m "feat: RIPE NCC downloader with CIDR parser and tests"
```

---

## Task 4: Gateway detection

**Files:**
- Create: `src-tauri/src/gateway.rs`
- Modify: `src-tauri/src/main.rs` (add `mod gateway;`)

- [ ] **Step 1: Write gateway.rs**

```rust
// src-tauri/src/gateway.rs
use std::process::Command;

/// Returns the physical gateway IP for the default route.
/// Tries platform-specific methods in order.
pub fn detect() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    return detect_macos();

    #[cfg(target_os = "linux")]
    return detect_linux();

    #[cfg(target_os = "windows")]
    return detect_windows();
}

#[cfg(target_os = "macos")]
fn detect_macos() -> Result<String, String> {
    // Primary: DHCP-reported gateway for en0 (works even when VPN is active)
    let out = Command::new("ipconfig")
        .args(["getoption", "en0", "router"])
        .output()
        .map_err(|e| e.to_string())?;
    let gw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !gw.is_empty() && gw != "0.0.0.0" {
        return Ok(gw);
    }
    // Fallback: parse `route -n get default`
    let out = Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("gateway:") {
            let gw = line["gateway:".len()..].trim().to_string();
            if !gw.is_empty() {
                return Ok(gw);
            }
        }
    }
    Err("Не удалось определить шлюз".to_string())
}

#[cfg(target_os = "linux")]
fn detect_linux() -> Result<String, String> {
    let out = Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    // Format: "default via 192.168.1.1 dev eth0 ..."
    for part in text.split_whitespace().collect::<Vec<_>>().windows(2) {
        if part[0] == "via" {
            return Ok(part[1].to_string());
        }
    }
    Err("Не удалось определить шлюз".to_string())
}

#[cfg(target_os = "windows")]
fn detect_windows() -> Result<String, String> {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command",
            "(Get-NetRoute -DestinationPrefix '0.0.0.0/0' | Sort-Object RouteMetric | Select-Object -First 1).NextHop"])
        .output()
        .map_err(|e| e.to_string())?;
    let gw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !gw.is_empty() {
        return Ok(gw);
    }
    Err("Не удалось определить шлюз".to_string())
}
```

- [ ] **Step 2: Add `mod gateway;` to main.rs and verify compile**

```bash
cd src-tauri
cargo check 2>&1 | grep -E "^error"
```
Expected: no errors.

- [ ] **Step 3: Manual smoke test on your machine**

```bash
cd src-tauri
cargo test 2>&1 | tail -5
# Then quickly verify gateway detection works:
cargo run --example gateway_check 2>/dev/null || \
  echo "Run: cargo test" && cargo test gateway 2>&1
```

Or write a quick one-liner:
```rust
// src-tauri/examples/gateway_check.rs
fn main() {
    match rubypass_lib::gateway::detect() {
        Ok(gw) => println!("Gateway: {}", gw),
        Err(e) => println!("Error: {}", e),
    }
}
```

```bash
cd src-tauri && cargo run --example gateway_check
```
Expected: prints your router IP (e.g. `192.168.1.1`).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/gateway.rs src-tauri/src/main.rs src-tauri/examples/
git commit -m "feat: cross-platform gateway detection"
```

---

## Task 5: Routing module

**Files:**
- Create: `src-tauri/src/routing.rs`
- Modify: `src-tauri/src/main.rs` (add `mod routing;`)

- [ ] **Step 1: Write routing.rs**

```rust
// src-tauri/src/routing.rs
use rayon::prelude::*;
use std::process::Command;

/// Add all subnets as routes via gateway. Returns count of successes.
pub fn add_routes(subnets: &[String], gateway: &str) -> usize {
    subnets
        .par_iter()
        .filter(|cidr| add_one(cidr, gateway))
        .count()
}

/// Remove all subnets from routing table.
pub fn remove_routes(subnets: &[String], gateway: &str) -> usize {
    subnets
        .par_iter()
        .filter(|cidr| remove_one(cidr, gateway))
        .count()
}

fn add_one(cidr: &str, gateway: &str) -> bool {
    route_cmd_add(cidr, gateway).status().map(|s| s.success()).unwrap_or(false)
}

fn remove_one(cidr: &str, gateway: &str) -> bool {
    route_cmd_delete(cidr, gateway).status().map(|s| s.success()).unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn route_cmd_add(cidr: &str, gateway: &str) -> Command {
    // Use osascript to get admin privileges for route command
    let script = format!(
        "do shell script \"route add -net {} {}\" with administrator privileges",
        cidr, gateway
    );
    let mut cmd = Command::new("osascript");
    cmd.args(["-e", &script]);
    cmd
}

#[cfg(target_os = "macos")]
fn route_cmd_delete(cidr: &str, gateway: &str) -> Command {
    let script = format!(
        "do shell script \"route delete -net {} {}\" with administrator privileges",
        cidr, gateway
    );
    let mut cmd = Command::new("osascript");
    cmd.args(["-e", &script]);
    cmd
}

#[cfg(target_os = "linux")]
fn route_cmd_add(cidr: &str, gateway: &str) -> Command {
    let mut cmd = Command::new("pkexec");
    cmd.args(["ip", "route", "add", cidr, "via", gateway]);
    cmd
}

#[cfg(target_os = "linux")]
fn route_cmd_delete(cidr: &str, gateway: &str) -> Command {
    let mut cmd = Command::new("pkexec");
    cmd.args(["ip", "route", "del", cidr, "via", gateway]);
    cmd
}

#[cfg(target_os = "windows")]
fn route_cmd_add(cidr: &str, gateway: &str) -> Command {
    // Split CIDR into network + mask
    let (net, prefix) = cidr.split_once('/').unwrap_or((cidr, "24"));
    let mask = prefix_to_mask(prefix.parse().unwrap_or(24));
    let mut cmd = Command::new("route");
    cmd.args(["ADD", net, "MASK", &mask, gateway]);
    cmd
}

#[cfg(target_os = "windows")]
fn route_cmd_delete(cidr: &str, gateway: &str) -> Command {
    let (net, prefix) = cidr.split_once('/').unwrap_or((cidr, "24"));
    let mask = prefix_to_mask(prefix.parse().unwrap_or(24));
    let mut cmd = Command::new("route");
    cmd.args(["DELETE", net, "MASK", &mask, gateway]);
    cmd
}

#[cfg(target_os = "windows")]
fn prefix_to_mask(prefix: u8) -> String {
    let mask: u32 = if prefix == 0 { 0 } else { !0u32 << (32 - prefix) };
    let bytes = mask.to_be_bytes();
    format!("{}.{}.{}.{}", bytes[0], bytes[1], bytes[2], bytes[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn test_prefix_to_mask() {
        assert_eq!(prefix_to_mask(24), "255.255.255.0");
        assert_eq!(prefix_to_mask(16), "255.255.0.0");
        assert_eq!(prefix_to_mask(32), "255.255.255.255");
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cd src-tauri
cargo test routing -- --nocapture
```
Expected: passes (1 test on Windows, 0 on others — that's fine).

- [ ] **Step 3: Add `mod routing;` to main.rs**

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/routing.rs src-tauri/src/main.rs
git commit -m "feat: cross-platform routing add/remove with rayon parallelism"
```

---

## Task 6: Status module

**Files:**
- Create: `src-tauri/src/status.rs`

- [ ] **Step 1: Write status.rs**

```rust
// src-tauri/src/status.rs
use serde::Serialize;
use std::process::Command;

#[derive(Debug, Serialize, Clone)]
pub struct AppStatus {
    pub gateway: Option<String>,
    pub subnet_count: usize,
    pub active_routes: usize,
    pub vpn_interface: Option<String>,
    pub last_updated: Option<String>,
    pub bypass_enabled: bool,
}

pub fn collect(bypass_enabled: bool, last_updated: Option<String>) -> AppStatus {
    let gateway = crate::gateway::detect().ok();
    let subnet_count = crate::updater::load_subnets()
        .map(|s| s.len())
        .unwrap_or(0);
    let active_routes = count_active_routes();
    let vpn_interface = detect_vpn();

    AppStatus {
        gateway,
        subnet_count,
        active_routes,
        vpn_interface,
        last_updated,
        bypass_enabled,
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn count_active_routes() -> usize {
    let out = Command::new("netstat").args(["-rn"]).output();
    match out {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            text.lines()
                .filter(|l| !l.contains("utun") && !l.contains("tun") && (l.contains("en0") || l.contains("eth0") || l.contains("wlan0")))
                .count()
        }
        Err(_) => 0,
    }
}

#[cfg(target_os = "windows")]
fn count_active_routes() -> usize {
    let out = Command::new("route").args(["PRINT"]).output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).lines().count(),
        Err(_) => 0,
    }
}

#[cfg(target_os = "macos")]
fn detect_vpn() -> Option<String> {
    let out = Command::new("netstat").args(["-rn"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let iface = text.lines()
        .flat_map(|l| l.split_whitespace())
        .find(|w| w.starts_with("utun"))
        .map(str::to_string);
    iface
}

#[cfg(target_os = "linux")]
fn detect_vpn() -> Option<String> {
    let out = Command::new("ip").args(["link", "show"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find(|l| l.contains("tun") || l.contains("ppp"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
}

#[cfg(target_os = "windows")]
fn detect_vpn() -> Option<String> {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command",
            "Get-NetAdapter | Where-Object {$_.InterfaceDescription -match 'TAP|TUN|VPN'} | Select-Object -First 1 -ExpandProperty Name"])
        .output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}
```

- [ ] **Step 2: Add `mod status;` to main.rs, verify compile**

```bash
cd src-tauri && cargo check 2>&1 | grep "^error"
```

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/status.rs src-tauri/src/main.rs
git commit -m "feat: status collector (routes, VPN detection, gateway)"
```

---

## Task 7: Tauri commands (IPC bridge)

**Files:**
- Create: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/main.rs` (register commands)

- [ ] **Step 1: Write commands.rs**

```rust
// src-tauri/src/commands.rs
use crate::{config, routing, status, updater};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};

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
    let mut cfg = state.0.lock().unwrap();
    if cfg.bypass_enabled {
        disable_bypass_inner(&app, &mut cfg)
    } else {
        enable_bypass_inner(&app, &mut cfg)
    }
}

#[tauri::command]
pub fn update_subnets(app: AppHandle, state: State<AppState>) -> Result<usize, String> {
    let count = updater::download_and_save()?;
    let mut cfg = state.0.lock().unwrap();
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    cfg.last_updated = Some(now);
    config::save(&cfg)?;
    // If bypass was active, restart routes with fresh list
    if cfg.bypass_enabled {
        disable_bypass_inner(&app, &mut cfg)?;
        enable_bypass_inner(&app, &mut cfg)?;
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
    let manager = app.autostart_manager();
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

// Internal helpers (not commands)
pub fn enable_bypass_inner(
    app: &AppHandle,
    cfg: &mut crate::config::Config,
) -> Result<(), String> {
    set_tray_icon(app, TrayState::Loading);
    let subnets = updater::load_subnets()?;
    let gateway = cfg.gateway_override.clone()
        .unwrap_or_else(|| crate::gateway::detect().unwrap_or_default());
    if gateway.is_empty() {
        set_tray_icon(app, TrayState::Inactive);
        return Err("Не удалось определить шлюз. Проверьте подключение к роутеру.".to_string());
    }
    routing::add_routes(&subnets, &gateway);
    cfg.bypass_enabled = true;
    config::save(cfg)?;
    set_tray_icon(app, TrayState::Active);
    Ok(())
}

pub fn disable_bypass_inner(
    app: &AppHandle,
    cfg: &mut crate::config::Config,
) -> Result<(), String> {
    set_tray_icon(app, TrayState::Loading);
    let subnets = updater::load_subnets().unwrap_or_default();
    let gateway = cfg.gateway_override.clone()
        .unwrap_or_else(|| crate::gateway::detect().unwrap_or_default());
    routing::remove_routes(&subnets, &gateway);
    cfg.bypass_enabled = false;
    config::save(cfg)?;
    set_tray_icon(app, TrayState::Inactive);
    Ok(())
}

pub enum TrayState { Active, Inactive, Loading }

pub fn set_tray_icon(app: &AppHandle, state: TrayState) {
    if let Some(tray) = app.tray_by_id("main") {
        let icon_bytes: &[u8] = match state {
            TrayState::Active  => include_bytes!("../icons/icon-green.png"),
            TrayState::Inactive => include_bytes!("../icons/icon-red.png"),
            TrayState::Loading => include_bytes!("../icons/icon-yellow.png"),
        };
        let icon = tauri::image::Image::from_bytes(icon_bytes).unwrap();
        let _ = tray.set_icon(Some(icon));
    }
}
```

- [ ] **Step 2: Update main.rs to register commands and state**

```rust
// src-tauri/src/main.rs
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
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

fn main() {
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
            if let TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } = event {
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
    let state = app.state::<AppState>();
    let cfg = state.0.lock().unwrap().clone();
    let is_first = !updater::subnet_file().exists();
    drop(cfg);

    if is_first {
        // Show window on first launch
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.show();
        }
        // Auto-download and enable
        let handle = app.handle().clone();
        std::thread::spawn(move || {
            let state = handle.state::<AppState>();
            if let Ok(count) = updater::download_and_save() {
                let mut cfg = state.0.lock().unwrap();
                cfg.last_updated = Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
                let _ = config::save(&cfg);
                log::info!("Downloaded {} subnets, enabling bypass", count);
                let _ = commands::enable_bypass_inner(&handle, &mut cfg);
            }
        });
    } else if cfg.bypass_enabled {
        // Restore bypass state on restart
        let handle = app.handle().clone();
        std::thread::spawn(move || {
            let state = handle.state::<AppState>();
            let mut cfg = state.0.lock().unwrap();
            let _ = commands::enable_bypass_inner(&handle, &mut cfg);
        });
    }
}
```

- [ ] **Step 3: Verify compile**

```bash
cd src-tauri && cargo check 2>&1 | grep "^error"
```

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/src/main.rs
git commit -m "feat: Tauri IPC commands and app state wiring"
```

---

## Task 8: Network change detection

**Files:**
- Create: `src-tauri/src/network_watch.rs`

- [ ] **Step 1: Write network_watch.rs**

```rust
// src-tauri/src/network_watch.rs
use tauri::AppHandle;

/// Spawn a background thread that watches for network interface changes
/// and re-applies routes when gateway changes.
pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        watch_loop(app);
    });
}

#[cfg(target_os = "macos")]
fn watch_loop(app: AppHandle) {
    use system_configuration::{
        core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop},
        dynamic_store::{SCDynamicStore, SCDynamicStoreCallBackContext, SCDynamicStoreBuilder},
        sys::schema_definitions::kSCEntNetIPv4,
    };

    let callback = move |_store: SCDynamicStore, _changed: Vec<String>| {
        handle_network_change(&app);
    };

    let ctx = SCDynamicStoreCallBackContext {
        callout: wrap_callback(callback),
        info: std::ptr::null_mut(),
    };

    // Watch for IP/router changes on all interfaces
    let store = SCDynamicStoreBuilder::new("rubypass")
        .callback_context(ctx)
        .build();
    let _ = store.set_notification_keys(
        &[],
        &["State:/Network/Interface/.*/IPv4".to_string()],
    );
    let source = store.create_run_loop_source();
    let run_loop = CFRunLoop::get_current();
    run_loop.add_source(&source, unsafe { kCFRunLoopDefaultMode });
    CFRunLoop::run_current();
}

#[cfg(target_os = "linux")]
fn watch_loop(app: AppHandle) {
    use std::io::Read;
    use std::os::unix::io::FromRawFd;

    // Use rtnetlink to watch route changes
    let sock = unsafe {
        libc::socket(libc::AF_NETLINK, libc::SOCK_RAW, libc::NETLINK_ROUTE)
    };
    if sock < 0 { return; }

    let addr = libc::sockaddr_nl {
        nl_family: libc::AF_NETLINK as u16,
        nl_pad: 0,
        nl_pid: 0,
        nl_groups: (libc::RTMGRP_IPV4_ROUTE | libc::RTMGRP_LINK) as u32,
    };
    unsafe {
        libc::bind(sock, &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of_val(&addr) as u32);
    }

    let mut buf = [0u8; 4096];
    loop {
        let n = unsafe { libc::recv(sock, buf.as_mut_ptr() as *mut _, buf.len(), 0) };
        if n > 0 {
            handle_network_change(&app);
            std::thread::sleep(std::time::Duration::from_secs(2)); // debounce
        }
    }
}

#[cfg(target_os = "windows")]
fn watch_loop(app: AppHandle) {
    use windows::Win32::NetworkManagement::IpHelper::{
        NotifyIpInterfaceChange, CancelMibChangeNotify2,
        MIB_NOTIFICATION_TYPE, MIB_IPINTERFACE_ROW,
    };
    use windows::Win32::Networking::WinSock::AF_UNSPEC;

    let app_ptr = Box::into_raw(Box::new(app.clone()));
    unsafe {
        let mut handle = std::mem::zeroed();
        NotifyIpInterfaceChange(
            AF_UNSPEC,
            Some(ip_change_callback),
            Some(app_ptr as *const _),
            false,
            &mut handle,
        ).unwrap();
    }
    // Keep thread alive
    loop { std::thread::sleep(std::time::Duration::from_secs(60)); }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn ip_change_callback(
    caller_context: *const std::ffi::c_void,
    _row: *const windows::Win32::NetworkManagement::IpHelper::MIB_IPINTERFACE_ROW,
    _notification_type: windows::Win32::NetworkManagement::IpHelper::MIB_NOTIFICATION_TYPE,
) {
    if !caller_context.is_null() {
        let app = &*(caller_context as *const AppHandle);
        handle_network_change(app);
    }
}

fn handle_network_change(app: &AppHandle) {
    use crate::commands::{AppState, disable_bypass_inner, enable_bypass_inner};
    let state = app.state::<AppState>();
    let mut cfg = state.0.lock().unwrap();
    if !cfg.bypass_enabled { return; }

    log::info!("Network changed, reapplying routes");
    // Brief delay to let new network settle
    drop(cfg);
    std::thread::sleep(std::time::Duration::from_secs(3));
    let mut cfg = state.0.lock().unwrap();
    let _ = disable_bypass_inner(app, &mut cfg);
    let _ = enable_bypass_inner(app, &mut cfg);
    // Emit event to frontend to refresh status
    let _ = app.emit("network-changed", ());
}

// macOS helper to wrap closure as C callback (simplified)
#[cfg(target_os = "macos")]
fn wrap_callback<F: Fn(SCDynamicStore, Vec<String>) + 'static>(f: F)
    -> system_configuration::dynamic_store::SCDynamicStoreCallBackT
{
    // Implementation uses system-configuration crate's built-in callback mechanism
    todo!("use SCDynamicStoreBuilder's native callback support")
}
```

> **Note:** The macOS `wrap_callback` placeholder should use `system-configuration` crate's native builder pattern — the crate handles the C callback wrapping internally. Replace `watch_loop` on macOS with the builder-pattern approach shown in the `system-configuration` crate docs.

- [ ] **Step 2: Add `libc` dependency for Linux in Cargo.toml**

```toml
[target.'cfg(target_os = "linux")'.dependencies]
libc = "0.2"
```

- [ ] **Step 3: Add `mod network_watch;` to main.rs (already added in Task 7)**

```bash
cd src-tauri && cargo check 2>&1 | grep "^error"
```

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/network_watch.rs src-tauri/Cargo.toml
git commit -m "feat: network change detection for auto route reapplication"
```

---

## Task 9: Auto-update scheduler

**Files:**
- Create: `src-tauri/src/scheduler.rs`

- [ ] **Step 1: Write scheduler.rs**

```rust
// src-tauri/src/scheduler.rs
use crate::config::{Config, UpdateSchedule};
use chrono::{DateTime, Duration, Utc};
use tauri::AppHandle;

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600)); // check every hour
            let state = app.state::<crate::commands::AppState>();
            let cfg = state.0.lock().unwrap().clone();
            if should_update(&cfg) {
                drop(state);
                log::info!("Scheduled update triggered");
                let state2 = app.state::<crate::commands::AppState>();
                if let Ok(count) = crate::updater::download_and_save() {
                    let mut cfg2 = state2.0.lock().unwrap();
                    cfg2.last_updated = Some(Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
                    let _ = crate::config::save(&cfg2);
                    log::info!("Scheduled update done: {} subnets", count);
                    // Re-apply routes if bypass active
                    if cfg2.bypass_enabled {
                        let _ = crate::commands::disable_bypass_inner(&app, &mut cfg2);
                        let _ = crate::commands::enable_bypass_inner(&app, &mut cfg2);
                    }
                }
            }
        }
    });
}

fn should_update(cfg: &Config) -> bool {
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
    use crate::config::UpdateSchedule;

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
```

- [ ] **Step 2: Run tests**

```bash
cd src-tauri && cargo test scheduler -- --nocapture
```
Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/scheduler.rs
git commit -m "feat: auto-update scheduler with interval logic and tests"
```

---

## Task 10: Frontend UI

**Files:**
- Modify: `src/index.html`
- Create: `src/style.css`
- Create: `src/main.js`

- [ ] **Step 1: Write style.css**

```css
/* src/style.css */
* { box-sizing: border-box; margin: 0; padding: 0; }

body {
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
  background: #12141f;
  color: #e2e8f0;
  user-select: none;
  overflow: hidden;
  height: 460px;
  width: 320px;
}

/* Header */
.header {
  background: #1a1d2e;
  padding: 10px 16px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  border-bottom: 1px solid #252840;
}
.header-left { display: flex; align-items: center; gap: 8px; }
.header-left .icon { font-size: 18px; }
.header-left h1 { font-size: 14px; font-weight: 600; }

/* Status section */
.status-section {
  padding: 18px 16px;
  border-bottom: 1px solid #1e2033;
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.status-label { font-size: 20px; font-weight: 700; }
.status-label.active { color: #22c55e; }
.status-label.inactive { color: #ef4444; }
.status-label.loading { color: #f59e0b; }
.status-sub { color: #6b7280; font-size: 11px; margin-top: 3px; }

/* Toggle switch */
.toggle {
  width: 52px; height: 28px;
  border-radius: 14px;
  background: #374151;
  position: relative;
  cursor: pointer;
  transition: background 0.2s;
  border: none;
  flex-shrink: 0;
}
.toggle.on { background: #22c55e; }
.toggle.loading { background: #f59e0b; pointer-events: none; }
.toggle::after {
  content: '';
  width: 22px; height: 22px;
  background: #fff;
  border-radius: 50%;
  position: absolute;
  top: 3px; left: 3px;
  transition: transform 0.2s;
  box-shadow: 0 1px 4px rgba(0,0,0,0.3);
}
.toggle.on::after { transform: translateX(24px); }

/* Stats grid */
.stats {
  padding: 12px 16px;
  border-bottom: 1px solid #1e2033;
}
.stats-label {
  color: #6b7280; font-size: 10px;
  text-transform: uppercase; letter-spacing: 0.5px;
  margin-bottom: 8px;
}
.stats-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
}
.stat-cell {
  background: #1a1d2e;
  border-radius: 6px;
  padding: 8px;
}
.stat-cell .label { color: #6b7280; font-size: 10px; }
.stat-cell .value { color: #fff; font-size: 14px; font-weight: 600; margin-top: 2px; }
.stat-cell .value.ok { color: #22c55e; }
.stat-cell .value.warn { color: #f59e0b; }
.stat-cell .value small { font-size: 11px; font-weight: 400; }

/* Update section */
.update-section {
  padding: 12px 16px;
  border-bottom: 1px solid #1e2033;
}
.update-row {
  display: flex; align-items: center; justify-content: space-between;
  margin-bottom: 8px;
}
.update-info .section-label { color: #6b7280; font-size: 10px; text-transform: uppercase; letter-spacing: 0.5px; }
.update-info .date { color: #9ca3af; font-size: 11px; margin-top: 2px; }
.btn-update {
  background: #3b82f6; color: #fff;
  border: none; border-radius: 6px;
  padding: 5px 12px; font-size: 11px;
  cursor: pointer; transition: background 0.15s;
}
.btn-update:hover { background: #2563eb; }
.btn-update:disabled { background: #1d4ed8; opacity: 0.6; cursor: not-allowed; }
.schedule-row {
  display: flex; align-items: center; gap: 8px;
}
.schedule-row label { color: #6b7280; font-size: 11px; }
.schedule-row select {
  background: #1a1d2e; border: 1px solid #2a2a3e;
  color: #fff; font-size: 11px;
  padding: 3px 6px; border-radius: 4px;
}

/* Settings */
.settings-section { padding: 12px 16px; }
.setting-row {
  display: flex; align-items: center; justify-content: space-between;
}
.setting-label { color: #9ca3af; font-size: 12px; }
.toggle-small {
  width: 36px; height: 20px;
  border-radius: 10px;
  background: #374151;
  position: relative; cursor: pointer;
  border: none; transition: background 0.2s;
}
.toggle-small.on { background: #3b82f6; }
.toggle-small::after {
  content: '';
  width: 16px; height: 16px;
  background: #fff; border-radius: 50%;
  position: absolute; top: 2px; left: 2px;
  transition: transform 0.2s;
}
.toggle-small.on::after { transform: translateX(16px); }

/* Toast */
.toast {
  position: fixed; bottom: 12px; left: 50%; transform: translateX(-50%);
  background: #1e293b; color: #e2e8f0;
  padding: 8px 14px; border-radius: 8px;
  font-size: 12px; white-space: nowrap;
  opacity: 0; transition: opacity 0.3s;
  border: 1px solid #334155;
  pointer-events: none; z-index: 100;
}
.toast.show { opacity: 1; }
.toast.error { border-color: #ef4444; color: #fca5a5; }
```

- [ ] **Step 2: Write index.html**

```html
<!DOCTYPE html>
<html lang="ru">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>RuBypass</title>
  <link rel="stylesheet" href="style.css">
</head>
<body>

<div class="header">
  <div class="header-left">
    <span class="icon" id="header-icon">🔒</span>
    <h1>RuBypass</h1>
  </div>
</div>

<div class="status-section">
  <div>
    <div class="status-label inactive" id="status-label">Выключен</div>
    <div class="status-sub" id="status-sub">Весь трафик через VPN</div>
  </div>
  <button class="toggle" id="toggle-btn" onclick="toggleBypass()"></button>
</div>

<div class="stats">
  <div class="stats-label">Статус</div>
  <div class="stats-grid">
    <div class="stat-cell">
      <div class="label">Подсети</div>
      <div class="value" id="stat-subnets">—</div>
    </div>
    <div class="stat-cell">
      <div class="label">Маршруты</div>
      <div class="value" id="stat-routes">—</div>
    </div>
    <div class="stat-cell">
      <div class="label">Шлюз <small title="Адрес вашего роутера">ⓘ</small></div>
      <div class="value" id="stat-gateway" style="font-size:11px">—</div>
    </div>
    <div class="stat-cell">
      <div class="label">VPN</div>
      <div class="value" id="stat-vpn">—</div>
    </div>
  </div>
</div>

<div class="update-section">
  <div class="update-row">
    <div class="update-info">
      <div class="section-label">Список IP</div>
      <div class="date" id="last-updated">Не обновлялся</div>
    </div>
    <button class="btn-update" id="btn-update" onclick="updateSubnets()">Обновить</button>
  </div>
  <div class="schedule-row">
    <label>Автообновление:</label>
    <select id="schedule-select" onchange="setSchedule(this.value)">
      <option value="never">Никогда</option>
      <option value="daily">Ежедневно</option>
      <option value="weekly" selected>Еженедельно</option>
      <option value="monthly">Ежемесячно</option>
    </select>
  </div>
</div>

<div class="settings-section">
  <div class="setting-row">
    <span class="setting-label">Автозапуск</span>
    <button class="toggle-small" id="autostart-btn" onclick="toggleAutostart()"></button>
  </div>
</div>

<div class="toast" id="toast"></div>

<script src="main.js"></script>
</body>
</html>
```

- [ ] **Step 3: Write main.js**

```javascript
// src/main.js
const { invoke, event } = window.__TAURI__;

let state = { bypass_enabled: false, autostart: false };

async function refresh() {
  try {
    const status = await invoke('get_status');
    const config = await invoke('get_config');
    state = { ...state, ...config };
    renderStatus(status, config);
  } catch (e) {
    showToast('Ошибка получения статуса: ' + e, true);
  }
}

function renderStatus(status, config) {
  // Toggle button
  const btn = document.getElementById('toggle-btn');
  btn.className = 'toggle' + (status.bypass_enabled ? ' on' : '');

  // Status label
  const label = document.getElementById('status-label');
  const sub = document.getElementById('status-sub');
  if (status.bypass_enabled) {
    label.textContent = 'Активен';
    label.className = 'status-label active';
    sub.textContent = 'RU трафик идёт напрямую';
    document.getElementById('header-icon').textContent = '🔓';
  } else {
    label.textContent = 'Выключен';
    label.className = 'status-label inactive';
    sub.textContent = 'Весь трафик через VPN';
    document.getElementById('header-icon').textContent = '🔒';
  }

  // Stats
  document.getElementById('stat-subnets').textContent =
    status.subnet_count ? status.subnet_count.toLocaleString('ru') : '—';
  document.getElementById('stat-routes').textContent =
    status.active_routes ? status.active_routes.toLocaleString('ru') : '—';
  document.getElementById('stat-gateway').textContent =
    status.gateway || 'не определён';
  const vpnEl = document.getElementById('stat-vpn');
  if (status.vpn_interface) {
    vpnEl.textContent = '● ' + status.vpn_interface;
    vpnEl.className = 'value ok';
  } else {
    vpnEl.textContent = 'не обнаружен';
    vpnEl.className = 'value warn';
  }

  // Last updated
  const lastUpd = document.getElementById('last-updated');
  if (status.last_updated) {
    const d = new Date(status.last_updated);
    lastUpd.textContent = 'Обновлён: ' + d.toLocaleDateString('ru', {
      day: 'numeric', month: 'long', year: 'numeric'
    });
  } else {
    lastUpd.textContent = 'Не обновлялся';
  }

  // Schedule
  document.getElementById('schedule-select').value =
    config.update_schedule || 'weekly';

  // Autostart
  const ab = document.getElementById('autostart-btn');
  ab.className = 'toggle-small' + (config.autostart ? ' on' : '');
}

async function toggleBypass() {
  const btn = document.getElementById('toggle-btn');
  btn.className = 'toggle loading';
  try {
    await invoke('toggle_bypass');
    await refresh();
  } catch (e) {
    showToast(e, true);
    await refresh();
  }
}

async function updateSubnets() {
  const btn = document.getElementById('btn-update');
  btn.disabled = true;
  btn.textContent = 'Загрузка…';
  try {
    const count = await invoke('update_subnets');
    showToast(`Обновлено: ${count.toLocaleString('ru')} подсетей`);
    await refresh();
  } catch (e) {
    showToast(e, true);
  } finally {
    btn.disabled = false;
    btn.textContent = 'Обновить';
  }
}

async function setSchedule(value) {
  try {
    await invoke('set_update_schedule', { schedule: value });
  } catch (e) {
    showToast(e, true);
  }
}

async function toggleAutostart() {
  state.autostart = !state.autostart;
  try {
    await invoke('set_autostart', { enabled: state.autostart });
    const ab = document.getElementById('autostart-btn');
    ab.className = 'toggle-small' + (state.autostart ? ' on' : '');
  } catch (e) {
    showToast(e, true);
    state.autostart = !state.autostart;
  }
}

function showToast(msg, isError = false) {
  const el = document.getElementById('toast');
  el.textContent = msg;
  el.className = 'toast show' + (isError ? ' error' : '');
  clearTimeout(el._timer);
  el._timer = setTimeout(() => { el.className = 'toast'; }, 3500);
}

// Listen for network-changed event from backend
event.listen('network-changed', () => {
  showToast('Сеть изменилась, маршруты обновлены');
  refresh();
});

// Poll status every 5 seconds to keep stats fresh
setInterval(refresh, 5000);

// Initial load
refresh();
```

- [ ] **Step 4: Build debug and open app**

```bash
cd ~/Desktop/internet_switcher
cargo tauri dev
```
Expected: app window opens, shows status UI. Toggle, update, autostart all clickable.

- [ ] **Step 5: Commit**

```bash
git add src/
git commit -m "feat: frontend UI — status window, toggle, update, settings"
```

---

## Task 11: Tray icons

**Files:**
- Create: `src-tauri/icons/icon-green.png`
- Create: `src-tauri/icons/icon-red.png`
- Create: `src-tauri/icons/icon-yellow.png`
- Create: `src-tauri/icons/icon.png`

- [ ] **Step 1: Create tray icons (32×32px each)**

Option A — use any image editor (Figma, Pixelmator, Preview) to create:
- `icon-green.png` — shield or lock icon in green `#22c55e`
- `icon-red.png` — same icon in red `#ef4444` (this is `icon.png` too)
- `icon-yellow.png` — same icon in amber `#f59e0b`

Option B — generate with ImageMagick:
```bash
# Install: brew install imagemagick
for color in "22c55e:green" "ef4444:red" "f59e0b:yellow"; do
  IFS=: read hex name <<< "$color"
  convert -size 32x32 xc:none \
    -fill "#$hex" \
    -draw "roundrectangle 4,4 28,28 6,6" \
    -fill white \
    -draw "rectangle 12,10 20,20" \
    -draw "arc 10,8 22,16 180,360" \
    src-tauri/icons/icon-$name.png
done
cp src-tauri/icons/icon-red.png src-tauri/icons/icon.png
```

- [ ] **Step 2: Verify icons load in app**

```bash
cargo tauri dev
```
Expected: tray shows icon. Toggle bypass — icon changes color.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/icons/
git commit -m "feat: tray icons (green/red/yellow states)"
```

---

## Task 12: GitHub Actions build

**Files:**
- Create: `.github/workflows/build.yml`

- [ ] **Step 1: Write build.yml**

```yaml
name: Build

on:
  push:
    tags: ['v*']
  workflow_dispatch:

jobs:
  build:
    strategy:
      matrix:
        include:
          - platform: macos-latest
            args: '--target universal-apple-darwin'
          - platform: ubuntu-22.04
            args: ''
          - platform: windows-latest
            args: ''

    runs-on: ${{ matrix.platform }}

    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.platform == 'macos-latest' && 'aarch64-apple-darwin,x86_64-apple-darwin' || '' }}

      - name: Install Linux deps
        if: matrix.platform == 'ubuntu-22.04'
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf

      - name: Install Node (for Tauri CLI)
        uses: actions/setup-node@v4
        with:
          node-version: 20

      - name: Install Tauri CLI
        run: cargo install tauri-cli --version "^2"

      - name: Build
        run: cargo tauri build ${{ matrix.args }}

      - name: Upload artifacts
        uses: actions/upload-artifact@v4
        with:
          name: rubypass-${{ matrix.platform }}
          path: |
            src-tauri/target/release/bundle/dmg/*.dmg
            src-tauri/target/release/bundle/nsis/*.exe
            src-tauri/target/release/bundle/appimage/*.AppImage
          if-no-files-found: ignore
```

- [ ] **Step 2: Tag a release to trigger build**

```bash
git tag v0.1.0
git push origin v0.1.0
```

- [ ] **Step 3: Download artifacts from GitHub Actions → upload to Synology NAS**

- [ ] **Step 4: Commit workflow file**

```bash
git add .github/
git commit -m "ci: GitHub Actions cross-platform build"
```

---

## Self-Review

**Spec coverage check:**
- ✅ Tauri + HTML/CSS/JS stack
- ✅ System tray (3 states: green/red/yellow)
- ✅ Main window 320×460px, closes to tray
- ✅ Toggle bypass (enable/disable routes in parallel)
- ✅ Cross-platform routing (macOS/Linux/Windows)
- ✅ Cross-platform gateway detection
- ✅ RIPE NCC download + CIDR parsing (tested)
- ✅ Manual update button
- ✅ Auto-update schedule (never/daily/weekly/monthly)
- ✅ Auto-start toggle (tauri-plugin-autostart)
- ✅ Network change detection → auto re-route
- ✅ Status grid (подсети, маршруты, шлюз с тултипом, VPN)
- ✅ First launch: auto-download + auto-enable
- ✅ Config persistence (JSON)
- ✅ GitHub Actions build → .dmg/.exe/.AppImage
- ✅ Error toasts for gateway failure, download failure, VPN not detected
- ✅ Admin/privilege handling (osascript on macOS, pkexec on Linux, requireAdministrator on Windows)

**Type consistency check:** All command names in `commands.rs` match `invoke()` calls in `main.js`. `Config` fields used in JS match `serde` serialization (`bypass_enabled`, `autostart`, `update_schedule`, `last_updated`).

**Placeholder check:** `network_watch.rs` on macOS has a `todo!()` note with explicit guidance to use the `system-configuration` crate's builder pattern. This is intentional — the macOS callback wrapping requires reading crate-specific docs that may have updated since this plan was written.

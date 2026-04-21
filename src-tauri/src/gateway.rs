// src-tauri/src/gateway.rs
use std::process::Command;

/// Returns the physical gateway IP for the default route.
/// Tries platform-specific methods in order.
pub fn detect() -> Result<String, String> {
    detect_platform()
}

#[cfg(target_os = "macos")]
fn detect_platform() -> Result<String, String> {
    detect_macos()
}

#[cfg(target_os = "linux")]
fn detect_platform() -> Result<String, String> {
    detect_linux()
}

#[cfg(target_os = "windows")]
fn detect_platform() -> Result<String, String> {
    detect_windows()
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn detect_platform() -> Result<String, String> {
    Err("Платформа не поддерживается".to_string())
}

#[cfg(target_os = "macos")]
fn detect_macos() -> Result<String, String> {
    // Primary: DHCP-reported gateway for en0 (works even when VPN is active)
    let out = Command::new("ipconfig")
        .args(["getoption", "en0", "router"])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        let gw = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !gw.is_empty() && gw != "0.0.0.0" {
            return Ok(gw);
        }
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
    let words: Vec<&str> = text.split_whitespace().collect();
    for pair in words.windows(2) {
        if pair[0] == "via" {
            return Ok(pair[1].to_string());
        }
    }
    Err("Не удалось определить шлюз".to_string())
}

#[cfg(target_os = "windows")]
fn detect_windows() -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    // `route print 0.0.0.0` is fast (no PowerShell startup) and shows the default route.
    // Output line: "          0.0.0.0          0.0.0.0      192.168.1.1  ..."
    let out = Command::new("route")
        .args(["print", "0.0.0.0"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 && parts[0] == "0.0.0.0" && parts[1] == "0.0.0.0" {
            let gw = parts[2];
            if !gw.is_empty() && gw != "On-link" {
                return Ok(gw.to_string());
            }
        }
    }
    Err("Не удалось определить шлюз".to_string())
}

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

fn count_active_routes() -> usize {
    let gateway = match crate::gateway::detect() {
        Ok(gw) => gw,
        Err(_) => return 0,
    };
    crate::routing::routes_via_gateway(&gateway).len()
}

#[cfg(target_os = "macos")]
fn detect_vpn() -> Option<String> {
    let out = Command::new("netstat").args(["-rn"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .flat_map(|l| l.split_whitespace())
        .find(|w| w.starts_with("utun"))
        .map(str::to_string)
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
        .args([
            "-NoProfile",
            "-Command",
            "Get-NetAdapter | Where-Object {$_.InterfaceDescription -match 'TAP|TUN|VPN'} | Select-Object -First 1 -ExpandProperty Name",
        ])
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn detect_vpn() -> Option<String> {
    None
}

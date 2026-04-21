use std::process::Command;
use std::sync::Mutex;

// Prevents concurrent routing operations from spawning multiple elevated windows.
static ROUTING_LOCK: Mutex<()> = Mutex::new(());

/// Validate that a string looks like an IPv4 address or CIDR — no shell metacharacters.
fn is_safe_ip(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '/')
}

/// Add all subnets as routes via gateway.
pub fn add_routes(subnets: &[String], gateway: &str) -> usize {
    run_batched(subnets, gateway, true)
}

/// Remove all subnets from routing table.
/// Queries the live routing table so stale routes from old lists are also cleaned up.
pub fn remove_routes(_subnets: &[String], gateway: &str) -> usize {
    let live = routes_via_gateway(gateway);
    if !live.is_empty() {
        run_batched(&live, gateway, false)
    } else {
        // Fallback to file-based list if we can't read the routing table
        run_batched(_subnets, gateway, false)
    }
}

/// Change gateway on existing routes — faster than delete+add on network switch.
pub fn change_routes(subnets: &[String], old_gateway: &str, new_gateway: &str) -> usize {
    run_change(subnets, old_gateway, new_gateway)
}

/// Batch all route commands into one privileged call to avoid repeated password prompts.
#[cfg(target_os = "macos")]
fn run_batched(subnets: &[String], gateway: &str, add: bool) -> usize {
    if !is_safe_ip(gateway) { return 0; }
    let valid: Vec<&str> = subnets.iter()
        .filter(|cidr| is_safe_ip(cidr))
        .map(String::as_str)
        .collect();
    if valid.is_empty() { return 0; }

    let action = if add { "add" } else { "delete" };

    // Prefer the installed helper (no password prompt). Install it on first use.
    if crate::helper::is_installed() || crate::helper::install().is_ok() {
        return crate::helper::run(action, &valid, gateway);
    }

    // Fallback: single osascript call (one password prompt).
    let route_action = if add { "add -net" } else { "delete -net" };
    let mut script = String::with_capacity(valid.len() * 45);
    for cidr in &valid {
        script.push_str(&format!("route {} {} {} 2>/dev/null || true\n", route_action, cidr, gateway));
    }
    let tmp = if add { "/tmp/rubypass_add.sh" } else { "/tmp/rubypass_del.sh" };
    if std::fs::write(tmp, &script).is_err() { return 0; }
    let osa = format!("do shell script \"sh {}\" with administrator privileges", tmp);
    let ok = Command::new("osascript")
        .args(["-e", &osa])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = std::fs::remove_file(tmp);
    if ok { valid.len() } else { 0 }
}

#[cfg(target_os = "linux")]
fn run_batched(subnets: &[String], gateway: &str, add: bool) -> usize {
    if !is_safe_ip(gateway) { return 0; }
    let valid: Vec<&str> = subnets.iter()
        .filter(|cidr| is_safe_ip(cidr))
        .map(String::as_str)
        .collect();
    if valid.is_empty() { return 0; }

    let action = if add { "add" } else { "delete" };

    // Prefer the installed helper (no password prompt). Install it on first use.
    if crate::helper::is_installed() || crate::helper::install().is_ok() {
        return crate::helper::run(action, &valid, gateway);
    }

    // Fallback: pkexec (one prompt per call).
    let ip_action = if add { "add" } else { "del" };
    let mut script = String::with_capacity(valid.len() * 45);
    script.push_str("#!/bin/sh\n");
    for cidr in &valid {
        script.push_str(&format!("ip route {} {} via {} 2>/dev/null || true\n", ip_action, cidr, gateway));
    }
    let tmp = if add { "/tmp/rubypass_add.sh" } else { "/tmp/rubypass_del.sh" };
    if std::fs::write(tmp, &script).is_err() { return 0; }
    let _ = Command::new("chmod").args(["+x", tmp]).status();
    let ok = Command::new("pkexec")
        .args(["--", "sh", tmp])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = std::fs::remove_file(tmp);
    if ok { valid.len() } else { 0 }
}

#[cfg(target_os = "windows")]
fn run_batched(subnets: &[String], gateway: &str, add: bool) -> usize {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    if !is_safe_ip(gateway) { return 0; }

    let valid: Vec<&str> = subnets.iter()
        .filter(|cidr| is_safe_ip(cidr))
        .map(String::as_str)
        .collect();
    if valid.is_empty() { return 0; }

    // Serialize routing operations — wait for any in-progress operation to finish.
    // try_lock was causing deletes to be skipped while adds were running,
    // leading to duplicate routes accumulating.
    let _lock = ROUTING_LOCK.lock().unwrap();

    // Write a single batch file and run one cmd.exe instead of N route.exe processes.
    // This eliminates N visible console windows and is significantly faster.
    let mut script = String::from("@echo off\r\n");
    let mut count = 0usize;
    for cidr in &valid {
        let Some((net, prefix_str)) = cidr.split_once('/') else { continue; };
        let Ok(prefix) = prefix_str.parse::<u8>() else { continue; };
        let mask = prefix_to_mask(prefix);
        if add {
            script.push_str(&format!("route ADD {} MASK {} {} >nul 2>&1\r\n", net, mask, gateway));
        } else {
            script.push_str(&format!("route DELETE {} MASK {} >nul 2>&1\r\n", net, mask));
        }
        count += 1;
    }
    if count == 0 { return 0; }

    let tmp = std::env::temp_dir().join(if add { "rubypass_add.bat" } else { "rubypass_del.bat" });
    if std::fs::write(&tmp, &script).is_err() { return 0; }
    let ok = Command::new("cmd")
        .args(["/c", tmp.to_str().unwrap_or("")])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = std::fs::remove_file(&tmp);
    if ok { count } else { 0 }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn run_batched(_subnets: &[String], _gateway: &str, _add: bool) -> usize { 0 }

// ── change routes (network switch optimisation) ───────────────────────────────

#[cfg(target_os = "macos")]
fn run_change(subnets: &[String], _old_gateway: &str, new_gateway: &str) -> usize {
    if !is_safe_ip(new_gateway) { return 0; }
    let valid: Vec<&str> = subnets.iter()
        .filter(|cidr| is_safe_ip(cidr))
        .map(String::as_str)
        .collect();
    if valid.is_empty() { return 0; }

    // Reinstall helper if needed so it supports the 'change' action.
    if crate::helper::is_installed() || crate::helper::install().is_ok() {
        return crate::helper::run("change", &valid, new_gateway);
    }

    // Fallback: single osascript call (one password prompt).
    let mut script = String::with_capacity(valid.len() * 80);
    for cidr in &valid {
        script.push_str(&format!(
            "route change -net {c} {gw} 2>/dev/null || route add -net {c} {gw} 2>/dev/null || true\n",
            c = cidr, gw = new_gateway
        ));
    }
    let tmp = "/tmp/rubypass_change.sh";
    if std::fs::write(tmp, &script).is_err() { return 0; }
    let osa = format!("do shell script \"sh {}\" with administrator privileges", tmp);
    let ok = Command::new("osascript").args(["-e", &osa]).status()
        .map(|s| s.success()).unwrap_or(false);
    let _ = std::fs::remove_file(tmp);
    if ok { valid.len() } else { 0 }
}

#[cfg(target_os = "linux")]
fn run_change(subnets: &[String], old_gateway: &str, new_gateway: &str) -> usize {
    if !is_safe_ip(new_gateway) { return 0; }
    let valid: Vec<&str> = subnets.iter()
        .filter(|cidr| is_safe_ip(cidr))
        .map(String::as_str)
        .collect();
    if valid.is_empty() { return 0; }

    // Use helper's change action (ip route change, faster than delete+add).
    if crate::helper::is_installed() || crate::helper::install().is_ok() {
        return crate::helper::run("change", &valid, new_gateway);
    }

    // Fallback: delete old routes, add under new gateway.
    run_batched(subnets, old_gateway, false);
    run_batched(subnets, new_gateway, true)
}

#[cfg(target_os = "windows")]
fn run_change(subnets: &[String], old_gateway: &str, new_gateway: &str) -> usize {
    run_batched(subnets, old_gateway, false);
    run_batched(subnets, new_gateway, true)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn run_change(_subnets: &[String], _old_gateway: &str, _new_gateway: &str) -> usize { 0 }

#[cfg(target_os = "windows")]
fn prefix_to_mask(prefix: u8) -> String {
    let mask: u32 = if prefix == 0 { 0 } else { !0u32 << (32 - prefix) };
    let bytes = mask.to_be_bytes();
    format!("{}.{}.{}.{}", bytes[0], bytes[1], bytes[2], bytes[3])
}

/// Read routes from the live system routing table that go via `gateway`.
/// Returns CIDR strings we actually installed (handles list drift between updates).
#[cfg(target_os = "macos")]
pub(crate) fn routes_via_gateway(gateway: &str) -> Vec<String> {
    if !is_safe_ip(gateway) { return vec![]; }
    let out = match Command::new("netstat").args(["-rn", "-f", "inet"]).output() {
        Ok(o) => o,
        Err(_) => return vec![],
    };
    // netstat -rn output: Destination  Gateway  Flags  ...
    // We want lines where Gateway column == our gateway
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let dest = cols.next()?;
            let gw   = cols.next()?;
            if gw != gateway { return None; }
            // dest is either "x.x.x.x/prefix" or "x.x.x.x" (host route)
            if dest.contains('/') && is_safe_ip(dest) {
                Some(dest.to_string())
            } else {
                None
            }
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub(crate) fn routes_via_gateway(gateway: &str) -> Vec<String> {
    if !is_safe_ip(gateway) { return vec![]; }
    // `ip route show` lines: "10.0.0.0/8 via 192.168.1.1 dev eth0 ..."
    let out = match Command::new("ip").args(["route", "show"]).output() {
        Ok(o) => o,
        Err(_) => return vec![],
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let dest = words.next()?;
            // find "via <gateway>" pair
            let has_gw = line.split_whitespace()
                .collect::<Vec<_>>()
                .windows(2)
                .any(|w| w[0] == "via" && w[1] == gateway);
            if has_gw && dest.contains('/') && is_safe_ip(dest) {
                Some(dest.to_string())
            } else {
                None
            }
        })
        .collect()
}

#[cfg(target_os = "windows")]
pub(crate) fn routes_via_gateway(gateway: &str) -> Vec<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    if !is_safe_ip(gateway) { return vec![]; }
    // `route print -4` lists all IPv4 routes without launching PowerShell.
    // Line format: "    10.0.0.0    255.0.0.0    192.168.1.1    192.168.1.100    25"
    let out = match Command::new("route")
        .args(["print", "-4"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && parts[2] == gateway && is_safe_ip(parts[0]) && is_safe_ip(parts[1]) {
                let prefix = mask_to_prefix(parts[1])?;
                Some(format!("{}/{}", parts[0], prefix))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(target_os = "windows")]
fn mask_to_prefix(mask: &str) -> Option<u8> {
    let parts: Vec<u8> = mask.split('.').filter_map(|p| p.parse().ok()).collect();
    if parts.len() != 4 { return None; }
    Some(u32::from_be_bytes([parts[0], parts[1], parts[2], parts[3]]).count_ones() as u8)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub(crate) fn routes_via_gateway(_gateway: &str) -> Vec<String> { vec![] }

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

    #[test]
    fn test_is_safe_ip_blocks_injection() {
        assert!(is_safe_ip("192.168.1.1"));
        assert!(is_safe_ip("10.0.0.0/8"));
        assert!(!is_safe_ip("10.0.0.0/8\"; rm -rf /"));
        assert!(!is_safe_ip("192.168.1.1 && whoami"));
    }
}

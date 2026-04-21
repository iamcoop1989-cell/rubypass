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
    if !is_safe_ip(gateway) { return 0; }
    let valid: Vec<&str> = subnets.iter()
        .filter(|cidr| is_safe_ip(cidr))
        .map(String::as_str)
        .collect();
    let count = valid.len();
    if count == 0 { return 0; }

    let mut lines: Vec<String> = Vec::with_capacity(count);
    for cidr in &valid {
        let Some((net, prefix_str)) = cidr.split_once('/') else { continue; };
        let Ok(prefix) = prefix_str.parse::<u8>() else { continue; };
        let mask = prefix_to_mask(prefix);
        if add {
            lines.push(format!("route ADD {} MASK {} {}", net, mask, gateway));
        } else {
            lines.push(format!("route DELETE {} MASK {} {}", net, mask, gateway));
        }
    }

    let script = lines.join("\r\n");
    let tmp = if add { "C:\\Windows\\Temp\\rubypass_add.bat" } else { "C:\\Windows\\Temp\\rubypass_del.bat" };
    if std::fs::write(tmp, &script).is_err() { return 0; }

    // If another routing operation is already running, drop this one rather than queue.
    let _lock = match ROUTING_LOCK.try_lock() {
        Ok(g) => g,
        Err(_) => {
            log::info!("Routing already in progress, skipping duplicate call");
            return 0;
        }
    };

    let ps = format!(
        "Start-Process cmd -ArgumentList '/c {}' -Verb RunAs -WindowStyle Hidden -Wait",
        tmp
    );
    let ok = Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let _ = std::fs::remove_file(tmp);
    if ok { lines.len() } else { 0 }
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
    if !is_safe_ip(gateway) { return vec![]; }
    let ps = format!(
        "Get-NetRoute | Where-Object {{ $_.NextHop -eq '{}' }} | \
         Select-Object -ExpandProperty DestinationPrefix",
        gateway
    );
    let out = match Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps])
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| s.contains('/') && is_safe_ip(s))
        .map(str::to_string)
        .collect()
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

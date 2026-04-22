use std::process::Command;

// Prevents concurrent routing operations from spawning multiple elevated windows.
#[cfg(target_os = "windows")]
use std::sync::Mutex;
#[cfg(target_os = "windows")]
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
    use windows::Win32::Foundation::ERROR_OBJECT_ALREADY_EXISTS;
    use windows::Win32::NetworkManagement::IpHelper::{
        CreateIpForwardEntry2, DeleteIpForwardEntry2, InitializeIpForwardEntry,
        MIB_IPFORWARD_ROW2,
    };
    use windows::Win32::Networking::WinSock::AF_INET;

    if !is_safe_ip(gateway) { return 0; }
    let Ok(gw_ip) = gateway.parse::<std::net::Ipv4Addr>() else { return 0 };
    let gw_s_addr = ip_to_s_addr(gw_ip);

    let valid: Vec<(u32, u8)> = subnets.iter()
        .filter(|s| is_safe_ip(s))
        .filter_map(|cidr| {
            let (net, pfx) = cidr.split_once('/')?;
            let ip: std::net::Ipv4Addr = net.parse().ok()?;
            let prefix: u8 = pfx.parse().ok()?;
            Some((ip_to_s_addr(ip), prefix))
        })
        .collect();
    if valid.is_empty() { return 0; }

    // Find the InterfaceIndex of the physical adapter that owns this gateway.
    // With multiple adapters (VPN + physical) Windows may not auto-determine
    // the correct interface when InterfaceIndex=0, causing silent failures.
    let if_index = find_interface_index_for_gateway(gw_s_addr);
    log::info!("routing: action={} gateway={} if_index={} subnets={}",
        if add {"add"} else {"del"}, gateway, if_index, valid.len());

    // Serialize so delete+add pairs from concurrent callers never interleave.
    let _lock = ROUTING_LOCK.lock().unwrap();

    let total = valid.len();
    let mut success = 0usize;
    let mut first_err_logged = false;
    for (net_s_addr, prefix) in &valid {
        let (net_s_addr, prefix) = (*net_s_addr, *prefix);
        unsafe {
            let mut row: MIB_IPFORWARD_ROW2 = std::mem::zeroed();
            InitializeIpForwardEntry(&mut row);

            row.InterfaceIndex = if_index;

            row.DestinationPrefix.PrefixLength = prefix;
            row.DestinationPrefix.Prefix.Ipv4.sin_family = AF_INET;
            row.DestinationPrefix.Prefix.Ipv4.sin_addr.S_un.S_addr = net_s_addr;

            row.NextHop.Ipv4.sin_family = AF_INET;
            row.NextHop.Ipv4.sin_addr.S_un.S_addr = gw_s_addr;

            // Metric 1 ensures our direct routes beat VPN routes for the same prefix.
            row.Metric = 1;

            let ok = if add {
                let r = CreateIpForwardEntry2(&row);
                if r.is_err() && !first_err_logged {
                    log::error!("CreateIpForwardEntry2 failed: {:?}", r);
                    first_err_logged = true;
                }
                r.is_ok() || r == ERROR_OBJECT_ALREADY_EXISTS
            } else {
                DeleteIpForwardEntry2(&row).is_ok()
            };
            if ok { success += 1; }
        }
    }
    log::info!("routing: done — {}/{} succeeded", success, total);
    success
}

/// Find the InterfaceIndex of the adapter that already has a route via this
/// gateway. With multiple adapters (VPN, TAP, physical) Windows may fail to
/// auto-determine the interface when InterfaceIndex = 0.
#[cfg(target_os = "windows")]
fn find_interface_index_for_gateway(gw_s_addr: u32) -> u32 {
    use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIpForwardTable2, MIB_IPFORWARD_TABLE2};
    use windows::Win32::Networking::WinSock::AF_INET;
    unsafe {
        let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
        if GetIpForwardTable2(AF_INET, &mut table).is_err() || table.is_null() {
            return 0;
        }
        let num = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), num);
        // Find any existing route with this gateway as NextHop (typically
        // the default route 0.0.0.0/0 added by the OS for this adapter).
        let idx = rows.iter()
            .find(|row| row.NextHop.Ipv4.sin_addr.S_un.S_addr == gw_s_addr)
            .map(|row| row.InterfaceIndex)
            .unwrap_or(0);
        FreeMibTable(table as *const _);
        idx
    }
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

/// Convert an IPv4 address to WinSock S_addr (network byte order as u32).
#[cfg(target_os = "windows")]
fn ip_to_s_addr(ip: std::net::Ipv4Addr) -> u32 {
    // from_ne_bytes places the octets in memory sequentially (network byte order).
    u32::from_ne_bytes(ip.octets())
}

/// Convert WinSock S_addr back to Ipv4Addr.
#[cfg(target_os = "windows")]
fn s_addr_to_ip(s_addr: u32) -> std::net::Ipv4Addr {
    // S_addr is big-endian in memory; from_be gives the canonical host-order value.
    std::net::Ipv4Addr::from(u32::from_be(s_addr))
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
    use windows::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIpForwardTable2, MIB_IPFORWARD_TABLE2,
    };
    use windows::Win32::Networking::WinSock::AF_INET;

    if !is_safe_ip(gateway) { return vec![]; }
    let Ok(gw_ip) = gateway.parse::<std::net::Ipv4Addr>() else { return vec![] };
    let gw_s_addr = ip_to_s_addr(gw_ip);

    unsafe {
        let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
        if GetIpForwardTable2(AF_INET, &mut table).is_err() || table.is_null() {
            return vec![];
        }
        let num = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), num);
        let result: Vec<String> = rows.iter()
            .filter_map(|row| {
                // GetIpForwardTable2(AF_INET) already filters to IPv4,
                // but Windows may not populate si_family in returned rows —
                // match by raw S_addr only.
                if row.NextHop.Ipv4.sin_addr.S_un.S_addr != gw_s_addr { return None; }
                let prefix = row.DestinationPrefix.PrefixLength;
                if prefix == 0 { return None; }
                let ip = s_addr_to_ip(row.DestinationPrefix.Prefix.Ipv4.sin_addr.S_un.S_addr);
                Some(format!("{}/{}", ip, prefix))
            })
            .collect();
        FreeMibTable(table as *const _);
        result
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub(crate) fn routes_via_gateway(_gateway: &str) -> Vec<String> { vec![] }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_safe_ip_blocks_injection() {
        assert!(is_safe_ip("192.168.1.1"));
        assert!(is_safe_ip("10.0.0.0/8"));
        assert!(!is_safe_ip("10.0.0.0/8\"; rm -rf /"));
        assert!(!is_safe_ip("192.168.1.1 && whoami"));
    }
}

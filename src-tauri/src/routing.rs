use rayon::prelude::*;
use std::process::Command;

/// Validate that a string looks like an IPv4 address or CIDR — no shell metacharacters.
fn is_safe_ip(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '/')
}

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
    if !is_safe_ip(cidr) || !is_safe_ip(gateway) { return false; }
    route_cmd_add(cidr, gateway).status().map(|s| s.success()).unwrap_or(false)
}

fn remove_one(cidr: &str, gateway: &str) -> bool {
    if !is_safe_ip(cidr) || !is_safe_ip(gateway) { return false; }
    route_cmd_delete(cidr, gateway).status().map(|s| s.success()).unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn route_cmd_add(cidr: &str, gateway: &str) -> Command {
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
    cmd.args(["--", "ip", "route", "add", cidr, "via", gateway]);
    cmd
}

#[cfg(target_os = "linux")]
fn route_cmd_delete(cidr: &str, gateway: &str) -> Command {
    let mut cmd = Command::new("pkexec");
    cmd.args(["--", "ip", "route", "del", cidr, "via", gateway]);
    cmd
}

#[cfg(target_os = "windows")]
fn route_cmd_add(cidr: &str, gateway: &str) -> Command {
    let Some((net, prefix_str)) = cidr.split_once('/') else {
        return Command::new("cmd"); // will fail — malformed CIDR
    };
    let Ok(prefix) = prefix_str.parse::<u8>() else {
        return Command::new("cmd");
    };
    let mask = prefix_to_mask(prefix);
    let mut cmd = Command::new("route");
    cmd.args(["ADD", net, "MASK", &mask, gateway]);
    cmd
}

#[cfg(target_os = "windows")]
fn route_cmd_delete(cidr: &str, gateway: &str) -> Command {
    let Some((net, prefix_str)) = cidr.split_once('/') else {
        return Command::new("cmd");
    };
    let Ok(prefix) = prefix_str.parse::<u8>() else {
        return Command::new("cmd");
    };
    let mask = prefix_to_mask(prefix);
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

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn route_cmd_add(_cidr: &str, _gateway: &str) -> Command {
    Command::new("true")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn route_cmd_delete(_cidr: &str, _gateway: &str) -> Command {
    Command::new("true")
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

    #[test]
    fn test_is_safe_ip_blocks_injection() {
        assert!(is_safe_ip("192.168.1.1"));
        assert!(is_safe_ip("10.0.0.0/8"));
        assert!(!is_safe_ip("10.0.0.0/8\"; rm -rf /"));
        assert!(!is_safe_ip("192.168.1.1 && whoami"));
    }
}

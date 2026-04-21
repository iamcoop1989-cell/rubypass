use serde::Serialize;
#[cfg(any(target_os = "macos", target_os = "linux"))]
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
    use windows::Win32::NetworkManagement::IpHelper::{GetAdaptersInfo, IP_ADAPTER_INFO};
    const NO_ERROR: u32 = 0;
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    unsafe {
        let mut size: u32 = 0;
        let _ = GetAdaptersInfo(None, &mut size);
        if size == 0 { return None; }
        let count = size as usize / std::mem::size_of::<IP_ADAPTER_INFO>() + 2;
        let mut buf: Vec<IP_ADAPTER_INFO> = Vec::with_capacity(count);
        buf.set_len(count);
        let ret = GetAdaptersInfo(Some(buf.as_mut_ptr()), &mut size);
        if ret != NO_ERROR && ret != ERROR_BUFFER_OVERFLOW {
            return None;
        }
        let mut ptr = buf.as_ptr();
        while !ptr.is_null() {
            let adapter = &*ptr;
            let desc = std::ffi::CStr::from_ptr(adapter.Description.as_ptr())
                .to_string_lossy()
                .to_lowercase();
            if desc.contains("tap") || desc.contains("tun") || desc.contains("vpn")
                || desc.contains("wireguard") || desc.contains("openvpn")
            {
                let name = std::ffi::CStr::from_ptr(adapter.AdapterName.as_ptr())
                    .to_string_lossy()
                    .to_string();
                return Some(name);
            }
            ptr = adapter.Next;
        }
        None
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn detect_vpn() -> Option<String> {
    None
}

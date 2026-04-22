#![cfg(target_os = "windows")]

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const INTERNET_SETTINGS: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
const SNAPSHOT_FILE: &str = "windows_proxy_snapshot.json";
const PAC_FILE: &str = "rubypass.pac";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProxySnapshot {
    proxy_enable: Option<u32>,
    proxy_server: Option<String>,
    auto_config_url: Option<String>,
}

#[derive(Debug, Clone)]
struct ProxySettings {
    proxy_enable: u32,
    proxy_server: Option<String>,
    auto_config_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Cidr {
    network: u32,
    prefix: u8,
}

pub fn install(subnets: &[String]) -> Result<(), String> {
    let settings = read_proxy_settings()?;
    if settings.proxy_enable == 0 {
        log::info!("PAC skipped: system proxy is disabled");
        return Ok(());
    }
    let Some(proxy) = proxy_for_pac(settings.proxy_server.as_deref()) else {
        log::info!("PAC skipped: no static system proxy detected");
        return Ok(());
    };

    save_snapshot_once(&settings)?;

    let pac = generate_pac(&proxy, subnets);
    let pac_path = pac_path();
    std::fs::create_dir_all(pac_path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&pac_path, pac).map_err(|e| e.to_string())?;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(INTERNET_SETTINGS, winreg::enums::KEY_WRITE)
        .map_err(|e| e.to_string())?;
    key.set_value("ProxyEnable", &0u32)
        .map_err(|e| e.to_string())?;
    let _: Result<(), _> = key.delete_value("ProxyServer");
    key.set_value("AutoConfigURL", &file_url(&pac_path))
        .map_err(|e| e.to_string())?;

    refresh_wininet();
    log::info!("PAC installed for proxy {proxy}");
    Ok(())
}

pub fn restore() -> Result<(), String> {
    let path = snapshot_path();
    if !path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let snapshot: ProxySnapshot = serde_json::from_str(&content).map_err(|e| e.to_string())?;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(INTERNET_SETTINGS, winreg::enums::KEY_WRITE)
        .map_err(|e| e.to_string())?;

    match snapshot.proxy_enable {
        Some(value) => key
            .set_value("ProxyEnable", &value)
            .map_err(|e| e.to_string())?,
        None => {
            let _: Result<(), _> = key.delete_value("ProxyEnable");
        }
    }
    match snapshot.proxy_server {
        Some(value) => key
            .set_value("ProxyServer", &value)
            .map_err(|e| e.to_string())?,
        None => {
            let _: Result<(), _> = key.delete_value("ProxyServer");
        }
    }
    match snapshot.auto_config_url {
        Some(value) => key
            .set_value("AutoConfigURL", &value)
            .map_err(|e| e.to_string())?,
        None => {
            let _: Result<(), _> = key.delete_value("AutoConfigURL");
        }
    }

    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(pac_path());
    refresh_wininet();
    log::info!("PAC restored original Windows proxy settings");
    Ok(())
}

fn read_proxy_settings() -> Result<ProxySettings, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey(INTERNET_SETTINGS)
        .map_err(|e| e.to_string())?;

    Ok(ProxySettings {
        proxy_enable: key.get_value("ProxyEnable").unwrap_or(0u32),
        proxy_server: key.get_value("ProxyServer").ok(),
        auto_config_url: key.get_value("AutoConfigURL").ok(),
    })
}

fn save_snapshot_once(settings: &ProxySettings) -> Result<(), String> {
    let path = snapshot_path();
    if path.exists() {
        return Ok(());
    }

    let snapshot = ProxySnapshot {
        proxy_enable: Some(settings.proxy_enable),
        proxy_server: settings.proxy_server.clone(),
        auto_config_url: settings.auto_config_url.clone(),
    };
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&snapshot).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

fn proxy_for_pac(proxy_server: Option<&str>) -> Option<String> {
    let raw = proxy_server?.trim();
    if raw.is_empty() {
        return None;
    }

    if raw.contains(';') || raw.contains('=') {
        for key in ["https", "http", "socks"] {
            if let Some(value) = raw.split(';').find_map(|part| {
                let (name, value) = part.split_once('=')?;
                name.eq_ignore_ascii_case(key).then_some(value.trim())
            }) {
                return normalize_proxy(value, key == "socks");
            }
        }
        None
    } else {
        normalize_proxy(raw, false)
    }
}

fn normalize_proxy(value: &str, socks: bool) -> Option<String> {
    let value = value
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_start_matches("socks://")
        .trim_start_matches("socks5://");
    if value.is_empty() {
        return None;
    }
    if socks {
        Some(format!("SOCKS {value}"))
    } else {
        Some(format!("PROXY {value}"))
    }
}

fn generate_pac(proxy: &str, subnets: &[String]) -> String {
    let cidrs = aggregate_subnets(subnets);
    let mut checks = String::new();
    for cidr in cidrs {
        let net = Ipv4Addr::from(cidr.network);
        let mask = Ipv4Addr::from(prefix_mask(cidr.prefix));
        checks.push_str(&format!(
            "      isInNet(ip, \"{}\", \"{}\") ||\n",
            net, mask
        ));
    }

    if checks.ends_with(" ||\n") {
        checks.truncate(checks.len() - 4);
        checks.push('\n');
    }

    let ip_block = if checks.is_empty() {
        String::new()
    } else {
        format!(
            r#"
  var ip = dnsResolve(host);
  if (ip && (
{checks}  )) {{
    return "DIRECT";
  }}
"#
        )
    };

    format!(
        r#"function FindProxyForURL(url, host) {{
  host = host.toLowerCase();

  if (dnsDomainIs(host, ".ru") ||
      dnsDomainIs(host, ".su") ||
      dnsDomainIs(host, ".рф") ||
      dnsDomainIs(host, ".рус") ||
      dnsDomainIs(host, ".москва") ||
      dnsDomainIs(host, ".moscow")) {{
    return "DIRECT";
  }}
{ip_block}

  return "{proxy}";
}}
"#
    )
}

fn aggregate_subnets(subnets: &[String]) -> Vec<Cidr> {
    let mut set: BTreeSet<Cidr> = subnets
        .iter()
        .filter_map(|s| parse_cidr(s))
        .map(normalize_cidr)
        .collect();

    loop {
        let current: Vec<Cidr> = set.iter().copied().collect();
        let mut changed = false;

        for cidr in current {
            if cidr.prefix == 0 || !set.contains(&cidr) {
                continue;
            }
            let sibling = Cidr {
                network: cidr.network ^ block_size(cidr.prefix),
                prefix: cidr.prefix,
            };
            if !set.contains(&sibling) {
                continue;
            }

            let parent = normalize_cidr(Cidr {
                network: cidr.network,
                prefix: cidr.prefix - 1,
            });
            set.remove(&cidr);
            set.remove(&sibling);
            set.insert(parent);
            changed = true;
            break;
        }

        if !changed {
            break;
        }
    }

    set.into_iter().collect()
}

fn parse_cidr(value: &str) -> Option<Cidr> {
    let (ip, prefix) = value.split_once('/')?;
    let ip: Ipv4Addr = ip.parse().ok()?;
    let prefix: u8 = prefix.parse().ok()?;
    if prefix > 32 {
        return None;
    }
    Some(Cidr {
        network: u32::from(ip),
        prefix,
    })
}

fn normalize_cidr(cidr: Cidr) -> Cidr {
    Cidr {
        network: cidr.network & prefix_mask(cidr.prefix),
        prefix: cidr.prefix,
    }
}

fn prefix_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

fn block_size(prefix: u8) -> u32 {
    1u32 << (32 - prefix)
}

fn snapshot_path() -> PathBuf {
    crate::config::data_dir().join(SNAPSHOT_FILE)
}

fn pac_path() -> PathBuf {
    crate::config::data_dir().join(PAC_FILE)
}

fn file_url(path: &std::path::Path) -> String {
    format!("file:///{}", path.to_string_lossy().replace('\\', "/"))
}

fn refresh_wininet() {
    use windows::Win32::Networking::WinInet::{
        InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
    };

    unsafe {
        let _ = InternetSetOptionW(None, INTERNET_OPTION_SETTINGS_CHANGED, None, 0);
        let _ = InternetSetOptionW(None, INTERNET_OPTION_REFRESH, None, 0);
    }
}

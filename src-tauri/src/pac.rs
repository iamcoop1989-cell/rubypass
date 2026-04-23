#![cfg(target_os = "windows")]

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
const SNAPSHOT_FILE: &str = "windows_proxy_snapshot.json";
const PAC_FILE: &str = "rubypass.pac";
const ROUTER_HOST: &str = "127.0.0.1";
const ROUTER_PORT: u16 = 17890;

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

#[derive(Debug, Clone)]
enum ProxyKind {
    Http,
    Socks5,
}

#[derive(Debug, Clone)]
struct UpstreamProxy {
    kind: ProxyKind,
    host: String,
    port: u16,
}

#[derive(Debug)]
struct RouterHandle {
    shutdown: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

#[derive(Debug)]
struct RouterState {
    upstream: UpstreamProxy,
    cidrs: Vec<Cidr>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PacDiagnostics {
    pub active: bool,
    pub direct_count: u64,
    pub vpn_count: u64,
    pub proxy_signature: String,
    pub last_decisions: Vec<String>,
}

#[derive(Debug, Default)]
struct RouterMetrics {
    direct_count: AtomicU64,
    vpn_count: AtomicU64,
    last_decisions: Mutex<Vec<String>>,
}

static ROUTER: OnceLock<Mutex<Option<RouterHandle>>> = OnceLock::new();
static METRICS: OnceLock<RouterMetrics> = OnceLock::new();

impl UpstreamProxy {
    fn to_pac_directive(&self) -> String {
        match self.kind {
            ProxyKind::Http => format!("PROXY {}:{}", self.host, self.port),
            ProxyKind::Socks5 => format!("SOCKS {}:{}", self.host, self.port),
        }
    }
}

pub fn install(subnets: &[String]) -> Result<(), String> {
    let settings = read_proxy_settings()?;
    if settings.proxy_enable == 0 {
        let pac_url = file_url(&pac_path());
        if settings.auto_config_url.as_deref() == Some(pac_url.as_str()) {
            return sync(subnets);
        }
        return Err("Системный proxy VPN не обнаружен".to_string());
    }
    let Some(upstream) = proxy_for_router(settings.proxy_server.as_deref()) else {
        return Err("Статический proxy VPN не обнаружен".to_string());
    };

    save_snapshot_once(&settings)?;
    install_router_pac(upstream, subnets)
}

pub fn sync(subnets: &[String]) -> Result<(), String> {
    let settings = read_proxy_settings()?;
    let pac_url = file_url(&pac_path());

    if settings.proxy_enable != 0 {
        let Some(upstream) = proxy_for_router(settings.proxy_server.as_deref()) else {
            log::info!("PAC sync skipped: enabled system proxy is not static");
            return Ok(());
        };

        save_snapshot_replace(&settings)?;
        install_router_pac(upstream, subnets)?;
        log::info!("PAC synced after Windows proxy change");
        return Ok(());
    }

    if settings.auto_config_url.as_deref() != Some(pac_url.as_str()) {
        log::info!("PAC sync skipped: RuBypass PAC is not active");
        return Ok(());
    }

    let Some(snapshot) = read_snapshot()? else {
        log::info!("PAC sync skipped: no proxy snapshot exists");
        return Ok(());
    };
    let Some(upstream) = proxy_for_router(snapshot.proxy_server.as_deref()) else {
        log::info!("PAC sync skipped: snapshot has no static proxy");
        return Ok(());
    };

    install_router_pac(upstream, subnets)?;
    log::info!("PAC synced using existing RuBypass snapshot");
    Ok(())
}

pub fn proxy_signature() -> String {
    match read_proxy_settings() {
        Ok(settings) => format!(
            "enabled={};server={};pac={}",
            settings.proxy_enable,
            settings.proxy_server.unwrap_or_default(),
            settings.auto_config_url.unwrap_or_default()
        ),
        Err(e) => format!("error={e}"),
    }
}

pub fn diagnostics() -> PacDiagnostics {
    let active = ROUTER
        .get()
        .and_then(|lock| lock.lock().ok().map(|router| router.is_some()))
        .unwrap_or(false);
    let metrics = METRICS.get_or_init(RouterMetrics::default);
    let last_decisions = metrics
        .last_decisions
        .lock()
        .map(|items| items.clone())
        .unwrap_or_default();

    PacDiagnostics {
        active,
        direct_count: metrics.direct_count.load(Ordering::Relaxed),
        vpn_count: metrics.vpn_count.load(Ordering::Relaxed),
        proxy_signature: proxy_signature(),
        last_decisions,
    }
}

pub fn restore() -> Result<(), String> {
    stop_router();

    let path = snapshot_path();
    let pac_url = file_url(&pac_path());
    let snapshot = read_snapshot()?;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(INTERNET_SETTINGS, winreg::enums::KEY_WRITE)
        .map_err(|e| e.to_string())?;

    if let Some(snapshot) = snapshot {
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
    } else if read_proxy_settings()?.auto_config_url.as_deref() == Some(pac_url.as_str()) {
        let _: Result<(), _> = key.delete_value("AutoConfigURL");
        log::warn!("PAC snapshot was missing; removed RuBypass AutoConfigURL only");
    } else {
        let _ = std::fs::remove_file(pac_path());
        refresh_wininet();
        return Ok(());
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

    save_snapshot_replace(settings)
}

fn save_snapshot_replace(settings: &ProxySettings) -> Result<(), String> {
    let path = snapshot_path();
    let snapshot = ProxySnapshot {
        proxy_enable: Some(settings.proxy_enable),
        proxy_server: settings.proxy_server.clone(),
        auto_config_url: settings.auto_config_url.clone(),
    };
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&snapshot).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

fn read_snapshot() -> Result<Option<ProxySnapshot>, String> {
    let path = snapshot_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let snapshot = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    Ok(Some(snapshot))
}

fn proxy_for_router(proxy_server: Option<&str>) -> Option<UpstreamProxy> {
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

fn normalize_proxy(value: &str, socks: bool) -> Option<UpstreamProxy> {
    let value = value
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_start_matches("socks://")
        .trim_start_matches("socks5://");
    if value.is_empty() {
        return None;
    }
    let (host, port) = value.rsplit_once(':')?;
    let port = port.parse().ok()?;
    let host = host.trim_matches(['[', ']']).to_string();
    if host == ROUTER_HOST && port == ROUTER_PORT {
        return None;
    }
    Some(UpstreamProxy {
        kind: if socks {
            ProxyKind::Socks5
        } else {
            ProxyKind::Http
        },
        host,
        port,
    })
}

fn generate_pac(upstream: &UpstreamProxy) -> String {
    let fallback = upstream.to_pac_directive();
    format!(
        r#"function FindProxyForURL(url, host) {{
  return "PROXY {ROUTER_HOST}:{ROUTER_PORT}; {fallback}";
}}
"#
    )
}

fn install_router_pac(upstream: UpstreamProxy, subnets: &[String]) -> Result<(), String> {
    start_router(upstream.clone(), subnets)?;

    let pac = generate_pac(&upstream);
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
    log::info!(
        "PAC installed for RuBypass router, upstream={}:{}",
        upstream.host,
        upstream.port
    );
    Ok(())
}

fn start_router(upstream: UpstreamProxy, subnets: &[String]) -> Result<(), String> {
    stop_router();
    reset_metrics();

    let listener = TcpListener::bind((ROUTER_HOST, ROUTER_PORT)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let state = Arc::new(RouterState {
        upstream,
        cidrs: aggregate_subnets(subnets),
    });
    let thread_shutdown = Arc::clone(&shutdown);
    let join = thread::spawn(move || {
        while !thread_shutdown.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let state = Arc::clone(&state);
                    thread::spawn(move || {
                        if let Err(e) = handle_client(stream, state) {
                            log::debug!("proxy-router client failed: {e}");
                        }
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    log::warn!("proxy-router accept failed: {e}");
                    thread::sleep(Duration::from_millis(250));
                }
            }
        }
    });

    let mut router = ROUTER.get_or_init(|| Mutex::new(None)).lock().unwrap();
    *router = Some(RouterHandle {
        shutdown,
        join: Some(join),
    });
    log::info!("proxy-router started on {ROUTER_HOST}:{ROUTER_PORT}");
    Ok(())
}

fn stop_router() {
    let Some(lock) = ROUTER.get() else { return };
    let mut router = lock.lock().unwrap();
    let Some(mut handle) = router.take() else {
        return;
    };
    handle.shutdown.store(true, Ordering::SeqCst);
    let _ = TcpStream::connect((ROUTER_HOST, ROUTER_PORT));
    if let Some(join) = handle.join.take() {
        let _ = join.join();
    }
    log::info!("proxy-router stopped");
}

fn handle_client(mut client: TcpStream, state: Arc<RouterState>) -> Result<(), String> {
    let request = read_http_head(&mut client)?;
    let head = String::from_utf8_lossy(&request);
    let mut lines = head.lines();
    let first_line = lines.next().ok_or_else(|| "empty request".to_string())?;

    if let Some(authority) = first_line
        .strip_prefix("CONNECT ")
        .and_then(|s| s.split_whitespace().next())
    {
        let target = parse_authority(authority, 443)?;
        let direct = should_direct(&target.host, &state);
        record_decision(&target.host, direct);
        if direct {
            let upstream = TcpStream::connect((target.host.as_str(), target.port))
                .map_err(|e| e.to_string())?;
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .map_err(|e| e.to_string())?;
            tunnel(client, upstream);
        } else {
            match state.upstream.kind {
                ProxyKind::Http => {
                    let mut upstream = connect_upstream(&state.upstream)?;
                    upstream.write_all(&request).map_err(|e| e.to_string())?;
                    tunnel(client, upstream);
                }
                ProxyKind::Socks5 => {
                    let upstream = connect_socks5(&state.upstream, &target.host, target.port)?;
                    client
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .map_err(|e| e.to_string())?;
                    tunnel(client, upstream);
                }
            }
        }
        return Ok(());
    }

    let target = parse_plain_http_target(first_line, &head)?;
    let direct = should_direct(&target.host, &state);
    record_decision(&target.host, direct);
    if direct {
        let mut upstream =
            TcpStream::connect((target.host.as_str(), target.port)).map_err(|e| e.to_string())?;
        upstream
            .write_all(&rewrite_absolute_form(&request, &target.host))
            .map_err(|e| e.to_string())?;
        tunnel(client, upstream);
    } else {
        match state.upstream.kind {
            ProxyKind::Http => {
                let mut upstream = connect_upstream(&state.upstream)?;
                upstream.write_all(&request).map_err(|e| e.to_string())?;
                tunnel(client, upstream);
            }
            ProxyKind::Socks5 => {
                let mut upstream = connect_socks5(&state.upstream, &target.host, target.port)?;
                upstream
                    .write_all(&rewrite_absolute_form(&request, &target.host))
                    .map_err(|e| e.to_string())?;
                tunnel(client, upstream);
            }
        }
    }

    Ok(())
}

fn reset_metrics() {
    let metrics = METRICS.get_or_init(RouterMetrics::default);
    metrics.direct_count.store(0, Ordering::Relaxed);
    metrics.vpn_count.store(0, Ordering::Relaxed);
    if let Ok(mut items) = metrics.last_decisions.lock() {
        items.clear();
    }
}

fn record_decision(host: &str, direct: bool) {
    let metrics = METRICS.get_or_init(RouterMetrics::default);
    if direct {
        metrics.direct_count.fetch_add(1, Ordering::Relaxed);
    } else {
        metrics.vpn_count.fetch_add(1, Ordering::Relaxed);
    }
    let route = if direct { "DIRECT" } else { "VPN" };
    log::info!("PAC alpha route: {host} -> {route}");
    if let Ok(mut items) = metrics.last_decisions.lock() {
        items.push(format!("{host} -> {route}"));
        if items.len() > 8 {
            items.remove(0);
        }
    }
}

fn read_http_head(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).map_err(|e| e.to_string())?;
        if read == 0 {
            return Err("connection closed before request head".to_string());
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(buf);
        }
        if buf.len() > 64 * 1024 {
            return Err("request head is too large".to_string());
        }
    }
}

#[derive(Debug)]
struct Target {
    host: String,
    port: u16,
}

fn parse_authority(authority: &str, default_port: u16) -> Result<Target, String> {
    if let Some((host, port)) = authority.rsplit_once(':') {
        let port = port
            .parse()
            .map_err(|_| "invalid target port".to_string())?;
        Ok(Target {
            host: host.trim_matches(['[', ']']).to_string(),
            port,
        })
    } else {
        Ok(Target {
            host: authority.to_string(),
            port: default_port,
        })
    }
}

fn parse_plain_http_target(first_line: &str, head: &str) -> Result<Target, String> {
    let url = first_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "missing request target".to_string())?;
    if let Some(rest) = url.strip_prefix("http://") {
        let authority = rest.split('/').next().unwrap_or(rest);
        return parse_authority(authority, 80);
    }

    for line in head.lines() {
        if let Some(host) = line
            .strip_prefix("Host:")
            .or_else(|| line.strip_prefix("host:"))
        {
            return parse_authority(host.trim(), 80);
        }
    }

    Err("missing Host header".to_string())
}

fn rewrite_absolute_form(request: &[u8], host: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(request);
    let Some(first_line_end) = text.find("\r\n") else {
        return request.to_vec();
    };
    let first = &text[..first_line_end];
    let mut parts = first.split_whitespace();
    let Some(method) = parts.next() else {
        return request.to_vec();
    };
    let Some(url) = parts.next() else {
        return request.to_vec();
    };
    let Some(version) = parts.next() else {
        return request.to_vec();
    };

    let Some(rest) = url.strip_prefix("http://") else {
        return request.to_vec();
    };
    let path_start = rest.find('/').unwrap_or(rest.len());
    let path = if path_start < rest.len() {
        &rest[path_start..]
    } else {
        "/"
    };

    let rewritten = format!(
        "{method} {path} {version}\r\n{}",
        &text[first_line_end + 2..]
    );
    if rewritten.contains("\r\nHost:") || rewritten.contains("\r\nhost:") {
        rewritten.into_bytes()
    } else {
        let insert = format!("{method} {path} {version}\r\nHost: {host}\r\n");
        format!("{insert}{}", &text[first_line_end + 2..]).into_bytes()
    }
}

fn should_direct(host: &str, state: &RouterState) -> bool {
    let host = host.trim_end_matches('.').to_lowercase();
    if host.ends_with(".ru")
        || host.ends_with(".su")
        || host.ends_with(".рф")
        || host.ends_with(".рус")
        || host.ends_with(".москва")
        || host.ends_with(".moscow")
    {
        return true;
    }

    // Alpha mode is intentionally conservative: browsers and embedded webviews
    // can proxy HTTPS requests using literal IP authorities, and broad IP-based
    // DIRECT matching made too much non-RU traffic bypass the VPN. Keep the
    // subnet set loaded for future session-aware routing, but for now route
    // only explicit RU hostnames directly.
    let _ = state;
    false
}

fn cidr_contains_any(ip: u32, cidrs: &[Cidr]) -> bool {
    cidrs
        .iter()
        .any(|cidr| (ip & prefix_mask(cidr.prefix)) == cidr.network)
}

fn connect_upstream(proxy: &UpstreamProxy) -> Result<TcpStream, String> {
    TcpStream::connect((proxy.host.as_str(), proxy.port)).map_err(|e| e.to_string())
}

fn connect_socks5(
    proxy: &UpstreamProxy,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream, String> {
    let mut stream = connect_upstream(proxy)?;
    stream
        .write_all(&[0x05, 0x01, 0x00])
        .map_err(|e| e.to_string())?;
    let mut response = [0u8; 2];
    stream
        .read_exact(&mut response)
        .map_err(|e| e.to_string())?;
    if response != [0x05, 0x00] {
        return Err("SOCKS5 upstream rejected no-auth handshake".to_string());
    }

    let host = target_host.as_bytes();
    if host.len() > u8::MAX as usize {
        return Err("SOCKS5 target host is too long".to_string());
    }
    let mut request = Vec::with_capacity(7 + host.len());
    request.extend_from_slice(&[0x05, 0x01, 0x00, 0x03, host.len() as u8]);
    request.extend_from_slice(host);
    request.extend_from_slice(&target_port.to_be_bytes());
    stream.write_all(&request).map_err(|e| e.to_string())?;

    let mut head = [0u8; 4];
    stream.read_exact(&mut head).map_err(|e| e.to_string())?;
    if head[1] != 0x00 {
        return Err(format!("SOCKS5 connect failed: {}", head[1]));
    }
    match head[3] {
        0x01 => {
            let mut rest = [0u8; 6];
            stream.read_exact(&mut rest).map_err(|e| e.to_string())?;
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).map_err(|e| e.to_string())?;
            let mut rest = vec![0u8; len[0] as usize + 2];
            stream.read_exact(&mut rest).map_err(|e| e.to_string())?;
        }
        0x04 => {
            let mut rest = [0u8; 18];
            stream.read_exact(&mut rest).map_err(|e| e.to_string())?;
        }
        _ => return Err("SOCKS5 returned unknown address type".to_string()),
    }
    Ok(stream)
}

fn tunnel(a: TcpStream, b: TcpStream) {
    let Ok(mut a_read) = a.try_clone() else {
        return;
    };
    let Ok(mut b_read) = b.try_clone() else {
        return;
    };
    let mut a_write = a;
    let mut b_write = b;

    let left = thread::spawn(move || {
        let _ = std::io::copy(&mut a_read, &mut b_write);
    });
    let right = thread::spawn(move || {
        let _ = std::io::copy(&mut b_read, &mut a_write);
    });
    let _ = left.join();
    let _ = right.join();
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

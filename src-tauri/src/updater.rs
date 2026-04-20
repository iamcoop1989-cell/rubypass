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

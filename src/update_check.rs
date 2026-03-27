use std::fs;

use crate::config;

const CACHE_FILE_NAME: &str = ".update-check";
const CACHE_TTL_SECS: u64 = 24 * 60 * 60; // 24 hours
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub update_available: bool,
    pub current_version: String,
    pub latest_version: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CacheData {
    latest_version: String,
    checked_at: u64,
}

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn cache_file() -> std::path::PathBuf {
    config::yeehaw_dir().join(CACHE_FILE_NAME)
}

fn read_cache() -> Option<CacheData> {
    let path = cache_file();
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    let data: CacheData = serde_json::from_str(&content).ok()?;
    if now_epoch().saturating_sub(data.checked_at) < CACHE_TTL_SECS {
        Some(data)
    } else {
        None // Cache expired
    }
}

fn write_cache(latest_version: &str) {
    let data = CacheData {
        latest_version: latest_version.to_string(),
        checked_at: now_epoch(),
    };
    if let Ok(content) = serde_json::to_string(&data) {
        let _ = fs::write(cache_file(), content);
    }
}

fn fetch_latest_version() -> Option<String> {
    // Check crates.io API for latest version
    let resp = ureq::get("https://crates.io/api/v1/crates/yeehaw")
        .set("User-Agent", "yeehaw-update-check")
        .call()
        .ok()?;

    let body: serde_json::Value = resp.into_json().ok()?;
    body["crate"]["max_version"]
        .as_str()
        .map(|s| s.to_string())
}

fn compare_versions(current: &str, latest: &str) -> i32 {
    let parse = |v: &str| -> Vec<u32> {
        v.split('.').map(|n| n.parse().unwrap_or(0)).collect()
    };
    let c = parse(current);
    let l = parse(latest);

    let c_major = c.first().copied().unwrap_or(0);
    let c_minor = c.get(1).copied().unwrap_or(0);
    let c_patch = c.get(2).copied().unwrap_or(0);
    let l_major = l.first().copied().unwrap_or(0);
    let l_minor = l.get(1).copied().unwrap_or(0);
    let l_patch = l.get(2).copied().unwrap_or(0);

    if l_major != c_major { return if l_major > c_major { 1 } else { -1 }; }
    if l_minor != c_minor { return if l_minor > c_minor { 1 } else { -1 }; }
    if l_patch != c_patch { return if l_patch > c_patch { 1 } else { -1 }; }
    0
}

/// Get current version info (sync, uses cache)
pub fn get_version_info() -> (String, Option<String>) {
    let current = CURRENT_VERSION.to_string();
    let cached = read_cache();
    (current, cached.map(|c| c.latest_version))
}

/// Check for updates. Returns cached data if available, otherwise fetches.
pub fn check_for_updates() -> Option<UpdateInfo> {
    let current_version = CURRENT_VERSION.to_string();

    // Try cache first
    if let Some(cached) = read_cache() {
        return Some(UpdateInfo {
            update_available: compare_versions(&current_version, &cached.latest_version) > 0,
            current_version,
            latest_version: cached.latest_version,
        });
    }

    // Fetch latest version
    let latest_version = fetch_latest_version()?;
    write_cache(&latest_version);

    Some(UpdateInfo {
        update_available: compare_versions(&current_version, &latest_version) > 0,
        current_version,
        latest_version,
    })
}

/// Format update notification message
pub fn format_update_message(info: &UpdateInfo) -> String {
    format!(
        "Update available: {} → {}\nRun: cargo install yeehaw",
        info.current_version, info.latest_version
    )
}

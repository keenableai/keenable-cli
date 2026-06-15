use serde_json::json;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config;
use crate::constants::{GITHUB_REPO, UPDATE_CHECK_INTERVAL_SECONDS};

fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn parse_version(v: &str) -> Option<Vec<u32>> {
    let v = v.strip_prefix('v').unwrap_or(v);
    v.split('.').map(|part| part.parse::<u32>().ok()).collect()
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn is_homebrew_install() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let path = exe.to_string_lossy();
    path.contains("/Cellar/") || path.contains("/homebrew/") || path.contains("/linuxbrew/")
}

pub fn install_hint() -> String {
    if is_homebrew_install() {
        "brew update && brew upgrade keenable-cli".to_string()
    } else if cfg!(windows) {
        "powershell -c \"irm https://github.com/keenableai/keenable-cli/releases/latest/download/keenable-cli-installer.ps1 | iex\"".to_string()
    } else {
        "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/keenableai/keenable-cli/releases/latest/download/keenable-cli-installer.sh | sh".to_string()
    }
}

async fn fetch_latest_version() -> Option<String> {
    let client = reqwest::Client::builder()
        .user_agent("keenable-cli")
        .build()
        .ok()?;
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    let data: serde_json::Value = resp.json().await.ok()?;
    let tag = data["tag_name"].as_str()?;
    Some(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

pub async fn check_for_update() -> Option<String> {
    let cache_file = config::update_check_file();

    // Check cache
    let mut cached_version: Option<String> = None;
    if let Ok(content) = fs::read_to_string(&cache_file)
        && let Ok(cache) = serde_json::from_str::<serde_json::Value>(&content)
    {
        let last_check = cache["last_check"].as_u64().unwrap_or(0);
        cached_version = cache["latest_version"].as_str().map(String::from);
        // saturating: a clock step backwards must not wrap into "stale"
        if now_epoch().saturating_sub(last_check) < UPDATE_CHECK_INTERVAL_SECONDS {
            let cached = cached_version?;
            if is_newer(&cached, current_version()) {
                return Some(cached);
            }
            return None;
        }
    }

    let latest = fetch_latest_version().await;

    // Record the attempt even on failure — otherwise an unreachable GitHub
    // (firewall, rate limit) re-adds the 5s fetch to every single command.
    let cache = json!({
        "last_check": now_epoch(),
        "latest_version": latest.as_deref().or(cached_version.as_deref()),
    });
    if let Some(dir) = cache_file.parent() {
        fs::create_dir_all(dir).ok();
    }
    fs::write(
        &cache_file,
        serde_json::to_string(&cache).unwrap_or_default(),
    )
    .ok();

    let latest = latest?;
    if is_newer(&latest, current_version()) {
        Some(latest)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_v_prefixed() {
        assert_eq!(parse_version("1.2.3"), Some(vec![1, 2, 3]));
        assert_eq!(parse_version("v0.1.20"), Some(vec![0, 1, 20]));
    }

    #[test]
    fn rejects_non_numeric() {
        assert_eq!(parse_version("1.2.x"), None);
        assert_eq!(parse_version("nightly"), None);
    }

    #[test]
    fn newer_only_when_strictly_greater() {
        assert!(is_newer("0.1.21", "0.1.20"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.20", "0.1.20")); // equal is not newer
        assert!(!is_newer("0.1.19", "0.1.20")); // older
    }

    #[test]
    fn shorter_version_compares_lexicographically() {
        // [0,2] > [0,1,20]: the minor bump wins regardless of patch length.
        assert!(is_newer("0.2", "0.1.20"));
    }

    #[test]
    fn unparseable_is_never_newer() {
        assert!(!is_newer("garbage", "0.1.20"));
        assert!(!is_newer("0.1.21", "garbage"));
    }
}

//! Background update check: fetches latest version from R2, caches for 24h.
//! Non-blocking, never delays the user's command.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const VERSION_URL: &str = "https://releases.savants.dev/latest/version.txt";
const CHECK_INTERVAL: Duration = Duration::from_secs(86400); // 24 hours
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn cache_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".savants").join("data").join("last_version_check"))
}

/// Read cached version + timestamp. Returns (version, timestamp) if fresh enough.
fn read_cache() -> Option<(String, SystemTime)> {
    let path = cache_path()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let mut lines = content.lines();
    let version = lines.next()?.trim().to_string();
    let ts_secs: u64 = lines.next()?.trim().parse().ok()?;
    let ts = SystemTime::UNIX_EPOCH + Duration::from_secs(ts_secs);
    Some((version, ts))
}

fn write_cache(version: &str) {
    if let Some(path) = cache_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let _ = std::fs::write(&path, format!("{}\n{}", version, now));
    }
}

/// Spawn a background task that checks for updates and prints a notice.
/// Never blocks the main command.
pub fn check_background() {
    tokio::spawn(async {
        // Check cache first
        if let Some((cached_version, ts)) = read_cache() {
            if let Ok(elapsed) = SystemTime::now().duration_since(ts) {
                if elapsed < CHECK_INTERVAL {
                    // Cache is fresh, just print if there's an update
                    print_update_notice(&cached_version);
                    return;
                }
            }
        }

        // Fetch latest version
        if let Ok(response) = reqwest::Client::new()
            .get(VERSION_URL)
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            if let Ok(body) = response.text().await {
                let latest = body.trim().to_string();
                if !latest.is_empty() && latest.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                    write_cache(&latest);
                    print_update_notice(&latest);
                }
            }
        }
    });
}

fn print_update_notice(latest: &str) {
    if latest != CURRENT_VERSION && !latest.is_empty() {
        // Simple semver comparison: only notify if latest is newer
        if is_newer(latest, CURRENT_VERSION) {
            eprintln!(
                "\n  \x1b[36m>\x1b[0m Update available: \x1b[2m{}\x1b[0m -> \x1b[1m{}\x1b[0m",
                CURRENT_VERSION, latest
            );
            eprintln!("    Run: \x1b[1mcurl -fsSL savants.sh | sh\x1b[0m\n");
        }
    }
}

fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> {
        v.split('.').filter_map(|s| s.parse().ok()).collect()
    };
    let l = parse(latest);
    let c = parse(current);
    for i in 0..3 {
        let lv = l.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if lv > cv { return true; }
        if lv < cv { return false; }
    }
    false
}

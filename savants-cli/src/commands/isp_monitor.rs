//! ISP uptime and quality monitor.
//!
//! Pings gateway, ISP hop, and external targets every 30s.
//! Logs results to ~/.savants/isp_history.jsonl for trend analysis.
//! Sends alerts via Gotify when connectivity degrades or drops.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

const PROBE_INTERVAL_SECS: u64 = 30;
const PING_COUNT: u32 = 3;
const HISTORY_FILE: &str = "isp_history.jsonl";
const MAX_HISTORY_LINES: usize = 100_000; // ~3 days at 30s intervals

/// Targets to probe
const TARGETS: &[(&str, &str)] = &[
    ("gateway", ""),          // auto-detected
    ("dns_cloudflare", "1.1.1.1"),
    ("dns_google", "8.8.8.8"),
    ("cdn_cloudflare", "104.16.132.229"), // cloudflare.com
];

#[derive(serde::Serialize)]
struct ProbeResult {
    timestamp: u64,
    target: String,
    host: String,
    loss_pct: f64,
    avg_ms: f64,
    max_ms: f64,
    jitter_ms: f64,
    reachable: bool,
}

#[derive(serde::Serialize)]
struct OutageEvent {
    timestamp: u64,
    duration_secs: u64,
    target: String,
    event_type: String, // "down", "degraded", "recovered"
}

fn detect_gateway() -> Option<String> {
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("route")
            .args(["-n", "get", "default"])
            .output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        s.lines().find(|l| l.contains("gateway:"))
            .and_then(|l| l.split(':').nth(1))
            .map(|s| s.trim().to_string())
    } else {
        // Linux: /proc/net/route
        std::fs::read_to_string("/proc/net/route").ok()
            .and_then(|r| r.lines().skip(1).find(|l| {
                let p: Vec<&str> = l.split_whitespace().collect();
                p.len() > 2 && p[1] == "00000000"
            }).and_then(|l| {
                let hex = l.split_whitespace().nth(2)?;
                if hex.len() == 8 {
                    let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                    let c = u8::from_str_radix(&hex[2..4], 16).ok()?;
                    let d = u8::from_str_radix(&hex[0..2], 16).ok()?;
                    Some(format!("{}.{}.{}.{}", a, b, c, d))
                } else { None }
            }))
    }
}

fn ping_stats(host: &str, count: u32) -> (f64, f64, f64) {
    let args = if cfg!(target_os = "macos") {
        vec!["-c".to_string(), count.to_string(), "-W".to_string(), "2000".to_string(), host.to_string()]
    } else {
        vec!["-c".to_string(), count.to_string(), "-W".to_string(), "2".to_string(), host.to_string()]
    };

    let out = match std::process::Command::new("ping").args(&args).output() {
        Ok(o) => o,
        Err(_) => return (100.0, 0.0, 0.0),
    };

    let raw = String::from_utf8_lossy(&out.stdout);

    // Parse packet loss
    let loss = raw.lines()
        .find(|l| l.contains("packet loss") || l.contains("packets transmitted"))
        .and_then(|l| {
            l.split_whitespace()
                .find(|w| w.ends_with('%'))
                .and_then(|w| w.trim_end_matches('%').parse::<f64>().ok())
        })
        .unwrap_or(100.0);

    // Parse rtt
    let (avg, max) = raw.lines()
        .find(|l| l.contains("min/avg/max"))
        .and_then(|l| {
            let stats = l.rsplit('=').next()?.trim();
            let parts: Vec<&str> = stats.split('/').collect();
            if parts.len() >= 3 {
                let avg = parts[1].trim().parse::<f64>().ok()?;
                let max = parts[2].trim().parse::<f64>().ok()?;
                Some((avg, max))
            } else { None }
        })
        .unwrap_or((0.0, 0.0));

    (loss, avg, max)
}

fn now_ts() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// HTTP-based reachability check (works without CAP_NET_RAW)
fn http_probe(url: &str) -> (f64, f64) {
    // Returns (latency_ms, loss_pct)
    let start = std::time::Instant::now();
    let result = std::process::Command::new("curl")
        .args(["-sf", "--max-time", "5", "-o", "/dev/null", "-w", "%{time_total}", url])
        .output();

    match result {
        Ok(o) if o.status.success() => {
            let time_str = String::from_utf8_lossy(&o.stdout);
            let secs: f64 = time_str.trim().parse().unwrap_or(0.0);
            (secs * 1000.0, 0.0) // latency_ms, 0% loss
        }
        _ => (0.0, 100.0) // unreachable
    }
}

/// Run one probe cycle and return results
pub fn probe_once() -> Vec<ProbeResult> {
    let gateway = detect_gateway().unwrap_or_default();
    let mut results = Vec::new();

    // Try ping first, fall back to HTTP probes
    let ping_works = {
        let (loss, _, _) = ping_stats("1.1.1.1", 1);
        loss < 100.0
    };

    for (name, host) in TARGETS {
        let actual_host: &str = if *name == "gateway" { &gateway } else { host };
        if actual_host.is_empty() { continue; }

        let (loss, avg, max) = if ping_works {
            ping_stats(actual_host, PING_COUNT)
        } else if *name == "gateway" {
            // Gateway: try ping even if it might fail
            ping_stats(actual_host, PING_COUNT)
        } else {
            // HTTP fallback for external targets
            let url = match *name {
                "dns_cloudflare" => "https://1.1.1.1/dns-query",
                "dns_google" => "https://dns.google/resolve?name=example.com",
                "cdn_cloudflare" => "https://www.cloudflare.com/cdn-cgi/trace",
                _ => &format!("http://{}", actual_host),
            };
            let (latency, loss) = http_probe(url);
            (loss, latency, latency)
        };
        let jitter = max - avg;

        results.push(ProbeResult {
            timestamp: now_ts(),
            target: name.to_string(),
            host: actual_host.to_string(),
            loss_pct: loss,
            avg_ms: avg,
            max_ms: max,
            jitter_ms: jitter,
            reachable: loss < 100.0,
        });
    }

    results
}

/// Append probe results to history file
fn append_history(results: &[ProbeResult]) {
    let path = dirs::home_dir().unwrap_or_default()
        .join(".savants").join(HISTORY_FILE);

    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        for r in results {
            if let Ok(json) = serde_json::to_string(r) {
                let _ = writeln!(f, "{}", json);
            }
        }
    }

    // Rotate if too large
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 50_000_000 { // 50MB
            // Keep last half
            if let Ok(content) = std::fs::read_to_string(&path) {
                let lines: Vec<&str> = content.lines().collect();
                let keep = &lines[lines.len() / 2..];
                let _ = std::fs::write(&path, keep.join("\n"));
            }
        }
    }
}

/// Send alert via Gotify if configured
fn alert_gotify(title: &str, message: &str, priority: u8) {
    let url = std::env::var("SAVANTS_GOTIFY_URL")
        .or_else(|_| std::env::var("GOTIFY_URL"))
        .unwrap_or_default();
    let token = std::env::var("SAVANTS_GOTIFY_TOKEN")
        .or_else(|_| std::env::var("GOTIFY_TOKEN"))
        .unwrap_or_default();

    if url.is_empty() || token.is_empty() { return; }

    let body = serde_json::json!({
        "title": title,
        "message": message,
        "priority": priority,
    });

    let _ = std::process::Command::new("curl")
        .args(["-s", "-X", "POST",
            &format!("{}/message?token={}", url, token),
            "-H", "Content-Type: application/json",
            "-d", &serde_json::to_string(&body).unwrap_or_default()])
        .output();
}

/// Main ISP monitor loop
pub fn run_monitor() {
    use colored::Colorize;

    println!("{}", "ISP Monitor starting...".bold());
    println!("  Probing every {}s: gateway + 1.1.1.1 + 8.8.8.8", PROBE_INTERVAL_SECS);
    println!("  History: ~/.savants/{}", HISTORY_FILE);
    println!();

    let mut last_down: Option<u64> = None;
    let mut consecutive_failures = 0u32;

    loop {
        let results = probe_once();
        append_history(&results);

        // Check for outage (external targets unreachable)
        let external_down = results.iter()
            .filter(|r| r.target != "gateway")
            .all(|r| !r.reachable);

        let gateway_down = results.iter()
            .find(|r| r.target == "gateway")
            .map(|r| !r.reachable)
            .unwrap_or(false);

        if external_down && !gateway_down {
            // ISP down (can reach gateway but not internet)
            consecutive_failures += 1;
            if consecutive_failures == 2 { // Alert after 1 minute of outage
                let ts = now_ts();
                last_down = Some(ts);
                println!("{} {} ISP DOWN - gateway reachable but no internet",
                    chrono_now(), "CRITICAL".red().bold());
                alert_gotify(
                    "ISP Outage Detected",
                    "Can reach gateway but all external targets unreachable. Internet is down.",
                    9,
                );
            } else if consecutive_failures % 10 == 0 {
                let mins = consecutive_failures as u64 * PROBE_INTERVAL_SECS / 60;
                println!("{} {} ISP still down ({}min)", chrono_now(), "CRITICAL".red(), mins);
            }
        } else if gateway_down {
            consecutive_failures += 1;
            if consecutive_failures == 2 {
                last_down = Some(now_ts());
                println!("{} {} NETWORK DOWN - gateway unreachable", chrono_now(), "CRITICAL".red().bold());
                alert_gotify(
                    "Network Down",
                    "Gateway is unreachable. Local network issue or router down.",
                    10,
                );
            }
        } else {
            // Online
            if consecutive_failures >= 2 {
                let duration = last_down.map(|d| now_ts() - d).unwrap_or(0);
                println!("{} {} Connectivity restored after {}s", chrono_now(), "RECOVERED".green().bold(), duration);
                alert_gotify(
                    "ISP Recovered",
                    &format!("Internet connectivity restored after {} seconds of downtime.", duration),
                    5,
                );
            }
            consecutive_failures = 0;
            last_down = None;

            // Check for degradation
            for r in &results {
                if r.target != "gateway" && r.loss_pct > 20.0 {
                    println!("{} {} {}: {:.0}% loss, avg {:.1}ms",
                        chrono_now(), "DEGRADED".yellow(), r.target, r.loss_pct, r.avg_ms);
                } else if r.avg_ms > 100.0 {
                    println!("{} {} {}: high latency {:.1}ms",
                        chrono_now(), "SLOW".yellow(), r.target, r.avg_ms);
                }
            }

            // Normal output every 10 cycles
            if consecutive_failures == 0 {
                let gw = results.iter().find(|r| r.target == "gateway");
                let ext = results.iter().find(|r| r.target == "dns_cloudflare");
                if let (Some(g), Some(e)) = (gw, ext) {
                    // Only print every 5 minutes to reduce noise
                    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                    let c = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if c % 10 == 0 {
                        println!("{} {} gw:{:.1}ms ext:{:.1}ms loss:{:.0}%",
                            chrono_now(), "OK".green(), g.avg_ms, e.avg_ms, e.loss_pct);
                    }
                }
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(PROBE_INTERVAL_SECS));
    }
}

fn chrono_now() -> String {
    let ts = now_ts();
    let secs = ts % 86400;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

/// Get ISP history summary for the last N hours
pub fn history_summary(hours: u64) -> serde_json::Value {
    let path = dirs::home_dir().unwrap_or_default()
        .join(".savants").join(HISTORY_FILE);

    let cutoff = now_ts() - hours * 3600;
    let mut total_probes = 0u64;
    let mut down_probes = 0u64;
    let mut total_latency = 0.0f64;
    let mut max_latency = 0.0f64;
    let mut outages: Vec<(u64, u64)> = Vec::new(); // (start, end)
    let mut current_outage_start: Option<u64> = None;

    if let Ok(content) = std::fs::read_to_string(&path) {
        for line in content.lines() {
            if let Ok(r) = serde_json::from_str::<serde_json::Value>(line) {
                let ts = r["timestamp"].as_u64().unwrap_or(0);
                if ts < cutoff { continue; }
                if r["target"].as_str() != Some("dns_cloudflare") { continue; }

                total_probes += 1;
                let reachable = r["reachable"].as_bool().unwrap_or(false);
                let avg = r["avg_ms"].as_f64().unwrap_or(0.0);

                if !reachable {
                    down_probes += 1;
                    if current_outage_start.is_none() {
                        current_outage_start = Some(ts);
                    }
                } else {
                    if let Some(start) = current_outage_start.take() {
                        outages.push((start, ts));
                    }
                    total_latency += avg;
                    if avg > max_latency { max_latency = avg; }
                }
            }
        }
    }

    let uptime_pct = if total_probes > 0 {
        (total_probes - down_probes) as f64 / total_probes as f64 * 100.0
    } else { 100.0 };

    let avg_latency = if total_probes > down_probes {
        total_latency / (total_probes - down_probes) as f64
    } else { 0.0 };

    serde_json::json!({
        "hours": hours,
        "total_probes": total_probes,
        "down_probes": down_probes,
        "uptime_pct": format!("{:.2}", uptime_pct),
        "avg_latency_ms": format!("{:.1}", avg_latency),
        "max_latency_ms": format!("{:.1}", max_latency),
        "outages": outages.len(),
        "total_downtime_secs": outages.iter().map(|(s, e)| e - s).sum::<u64>(),
    })
}

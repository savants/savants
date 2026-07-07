use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use colored::*;
use serde_json::Value;

// ── Embedded guard profile JSON files ──────────────────────────────────────

const PROFILES: &[(&str, &str)] = &[
    ("minimal.json", include_str!("../../../packages/guard-profiles/presets/minimal.json")),
    ("standard.json", include_str!("../../../packages/guard-profiles/presets/standard.json")),
    ("paranoid.json", include_str!("../../../packages/guard-profiles/presets/paranoid.json")),
    ("comprehensive.json", include_str!("../../../packages/guard-profiles/presets/comprehensive.json")),
    ("battle-tested.json", include_str!("../../../packages/guard-profiles/presets/battle-tested.json")),
    ("secrets.json", include_str!("../../../packages/guard-profiles/presets/secrets.json")),
    ("git-safe.json", include_str!("../../../packages/guard-profiles/presets/git-safe.json")),
    ("infra-safe.json", include_str!("../../../packages/guard-profiles/presets/infra-safe.json")),
    ("publish-safe.json", include_str!("../../../packages/guard-profiles/presets/publish-safe.json")),
    ("k8s-safe.json", include_str!("../../../packages/guard-profiles/presets/k8s-safe.json")),
    ("k8s-secrets.json", include_str!("../../../packages/guard-profiles/presets/k8s-secrets.json")),
    ("nixos-safe.json", include_str!("../../../packages/guard-profiles/presets/nixos-safe.json")),
    ("filesystem-safe.json", include_str!("../../../packages/guard-profiles/presets/filesystem-safe.json")),
    ("credentials-safe.json", include_str!("../../../packages/guard-profiles/presets/credentials-safe.json")),
    ("database-safe.json", include_str!("../../../packages/guard-profiles/presets/database-safe.json")),
    ("cloud-safe.json", include_str!("../../../packages/guard-profiles/presets/cloud-safe.json")),
    ("network-safe.json", include_str!("../../../packages/guard-profiles/presets/network-safe.json")),
    ("system-safe.json", include_str!("../../../packages/guard-profiles/presets/system-safe.json")),
    ("cicd-safe.json", include_str!("../../../packages/guard-profiles/presets/cicd-safe.json")),
    ("persistence-safe.json", include_str!("../../../packages/guard-profiles/presets/persistence-safe.json")),
];

// ── Path helpers ───────────────────────────────────────────────────────────

fn savants_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".savants")
}

fn rules_path() -> PathBuf {
    savants_dir().join("guard-rules.json")
}

fn state_path() -> PathBuf {
    savants_dir().join("guard-state.json")
}

fn pause_path() -> PathBuf {
    savants_dir().join("guard-paused")
}

fn stats_path() -> PathBuf {
    savants_dir().join("hook-stats.jsonl")
}

fn lock_path() -> PathBuf {
    savants_dir().join("profiles.lock")
}

fn disabled_path() -> PathBuf {
    savants_dir().join("disabled-rules.json")
}

fn sync_path() -> PathBuf {
    savants_dir().join("guard-sync.json")
}

fn profiles_dir() -> PathBuf {
    savants_dir().join("profiles")
}

fn custom_profiles_dir() -> PathBuf {
    savants_dir().join("custom-profiles")
}

fn routing_path() -> PathBuf {
    savants_dir().join("smart-routing.enabled")
}

fn cloud_api() -> String {
    std::env::var("SAVANTS_CLOUD_API")
        .unwrap_or_else(|_| "https://api.savants.cloud/api/v1/profiles".to_string())
}

fn cloud_guard_api() -> String {
    std::env::var("SAVANTS_CLOUD_GUARD_API")
        .unwrap_or_else(|_| "https://api.savants.cloud/api/v1/guard".to_string())
}

// ── JSON helpers ───────────────────────────────────────────────────────────

fn read_json_array(path: &PathBuf) -> Vec<Value> {
    match fs::read_to_string(path) {
        Ok(s) => serde_json::from_str::<Vec<Value>>(&s).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn read_json_object(path: &PathBuf) -> serde_json::Map<String, Value> {
    match fs::read_to_string(path) {
        Ok(s) => serde_json::from_str::<serde_json::Map<String, Value>>(&s).unwrap_or_default(),
        Err(_) => serde_json::Map::new(),
    }
}

fn write_json_array(path: &PathBuf, arr: &[Value]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let json = serde_json::to_string_pretty(arr).unwrap_or_else(|_| "[]".to_string());
    if let Err(e) = fs::write(path, json) {
        eprintln!("Error writing {}: {}", path.display(), e);
    }
}

fn write_json_object(path: &PathBuf, obj: &serde_json::Map<String, Value>) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let json = serde_json::to_string_pretty(obj).unwrap_or_else(|_| "{}".to_string());
    if let Err(e) = fs::write(path, json) {
        eprintln!("Error writing {}: {}", path.display(), e);
    }
}

// ── Duration parsing ───────────────────────────────────────────────────────

fn parse_duration(s: &str) -> Option<i64> {
    if s.is_empty() {
        return None;
    }
    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: i64 = num_str.parse().ok()?;
    match unit {
        "s" => Some(num),
        "m" => Some(num * 60),
        "h" => Some(num * 3600),
        _ => None,
    }
}

// ── Relative time formatting ───────────────────────────────────────────────

fn relative_time(iso: &str) -> String {
    if iso.is_empty() {
        return "never".to_string();
    }
    let dt = match DateTime::parse_from_rfc3339(iso) {
        Ok(d) => d.with_timezone(&Utc),
        Err(_) => {
            // Try parsing without timezone
            match iso.parse::<DateTime<Utc>>() {
                Ok(d) => d,
                Err(_) => return iso.to_string(),
            }
        }
    };
    let now = Utc::now();
    let secs = (now - dt).num_seconds();
    if secs < 0 {
        return iso.to_string();
    }
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

// ── API key resolution ─────────────────────────────────────────────────────

fn get_api_key() -> Option<String> {
    if let Ok(key) = std::env::var("SAVANTS_API_KEY") {
        if !key.is_empty() {
            return Some(key);
        }
    }
    let state_file = savants_dir().join("state.json");
    let obj = read_json_object(&state_file);
    obj.get("cloud_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn get_machine_id() -> String {
    gethostname::gethostname()
        .to_string_lossy()
        .to_string()
}

// ── Profile loader ─────────────────────────────────────────────────────────

/// Load profile rules from embedded data, ~/.savants/profiles/, or ~/.savants/custom-profiles/
fn load_profile_rules(name: &str) -> Option<Vec<Value>> {
    // 1. Try embedded profiles
    let filename = format!("{}.json", name);
    for (pname, content) in PROFILES {
        if *pname == filename {
            return serde_json::from_str::<Vec<Value>>(content).ok();
        }
    }
    // 2. Try on-disk profiles dir
    let disk_path = profiles_dir().join(&filename);
    if disk_path.exists() {
        let arr = read_json_array(&disk_path);
        if !arr.is_empty() {
            return Some(arr);
        }
    }
    // 3. Try custom profiles dir
    let custom_path = custom_profiles_dir().join(&filename);
    if custom_path.exists() {
        let arr = read_json_array(&custom_path);
        if !arr.is_empty() {
            return Some(arr);
        }
    }
    None
}

// ── Sync embedded profiles to disk ─────────────────────────────────────────

fn ensure_profiles() {
    let dir = profiles_dir();
    fs::create_dir_all(&dir).ok();
    for (name, content) in PROFILES {
        let path = dir.join(name);
        let needs_write = match fs::read_to_string(&path) {
            Ok(existing) => existing != *content,
            Err(_) => true,
        };
        if needs_write {
            fs::write(&path, content).ok();
        }
    }
}

// ── Read stats events ──────────────────────────────────────────────────────

fn read_stats_events() -> Vec<Value> {
    let path = stats_path();
    if !path.exists() {
        return Vec::new();
    }
    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    BufReader::new(file)
        .lines()
        .filter_map(|line| line.ok())
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect()
}

// ── HTTP client helper ─────────────────────────────────────────────────────

fn http_get(url: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;
    let resp = client.get(url).send().map_err(|e| format!("{}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.text().map_err(|e| format!("{}", e))
}

fn http_post_json(url: &str, body: &Value, auth: Option<&str>) -> Result<Value, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("savants-cli/0.27.5")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;
    let mut req = client.post(url).json(body);
    if let Some(token) = auth {
        req = req.bearer_auth(token);
    }
    let resp = req.send().map_err(|e| format!("{}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json::<Value>().map_err(|e| format!("{}", e))
}

fn http_get_json(url: &str, auth: Option<&str>) -> Result<Value, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("savants-cli/0.27.5")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;
    let mut req = client.get(url);
    if let Some(token) = auth {
        req = req.bearer_auth(token);
    }
    let resp = req.send().map_err(|e| format!("{}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json::<Value>().map_err(|e| format!("{}", e))
}

// ═══════════════════════════════════════════════════════════════════════════
//  Command implementations
// ═══════════════════════════════════════════════════════════════════════════

fn cmd_preset(args: &[String]) {
    let preset_str = args.first().map(|s| s.as_str()).unwrap_or("standard");
    let profile_names: Vec<&str> = preset_str.split('+').collect();

    let mut all_rules: Vec<Value> = Vec::new();
    let mut loaded_lines: Vec<String> = Vec::new();

    for name in &profile_names {
        match load_profile_rules(name) {
            Some(rules) => {
                let count = rules.len();
                for r in rules {
                    if !all_rules.contains(&r) {
                        all_rules.push(r);
                    }
                }
                loaded_lines.push(format!("  {} {} ({} rules)", "\u{2713}".green(), name, count));
            }
            None => {
                eprintln!("Unknown profile: {}", name);
                eprintln!("Built-in: minimal, standard, paranoid, comprehensive, battle-tested, nixos-safe,");
                eprintln!("  filesystem-safe, credentials-safe, git-safe, database-safe, k8s-safe,");
                eprintln!("  cloud-safe, network-safe, publish-safe, system-safe, cicd-safe, persistence-safe");
                eprintln!();
                eprintln!("Custom profiles: place JSON files in ~/.savants/custom-profiles/");
                eprintln!("  system-safe, cicd-safe, persistence-safe, secrets, infra-safe");
                std::process::exit(1);
            }
        }
    }

    let dir = savants_dir();
    fs::create_dir_all(&dir).ok();
    write_json_array(&rules_path(), &all_rules);

    // Write guard-state.json
    let mut state = serde_json::Map::new();
    state.insert("preset".to_string(), Value::String(preset_str.to_string()));
    let profiles_arr: Vec<Value> = profile_names.iter().map(|s| Value::String(s.to_string())).collect();
    state.insert("profiles".to_string(), Value::Array(profiles_arr));
    write_json_object(&state_path(), &state);

    println!("Guard profiles activated:");
    for line in &loaded_lines {
        println!("{}", line);
    }
    println!();
    println!("Total: {} rules \u{2192} {}", all_rules.len(), rules_path().display());
    println!();
    println!("Your AI coding agent is now protected.");
    println!("Use --dangerously-skip-permissions with confidence.");
}

fn cmd_on() {
    let pf = pause_path();
    if pf.exists() {
        fs::remove_file(&pf).ok();
    }
    let rp = rules_path();
    if rp.exists() {
        let rules = read_json_array(&rp);
        println!("Guard resumed. {} rules active.", rules.len());
    } else {
        println!("Guard resumed (no rules loaded).");
        println!("  Run: savants guard preset standard");
    }
}

fn cmd_off(args: &[String]) {
    let pf = pause_path();
    if let Some(parent) = pf.parent() {
        fs::create_dir_all(parent).ok();
    }

    if let Some(duration_str) = args.first() {
        match parse_duration(duration_str) {
            Some(secs) => {
                let expiry = Utc::now() + Duration::seconds(secs);
                let expiry_str = expiry.to_rfc3339();
                if let Err(e) = fs::write(&pf, &expiry_str) {
                    eprintln!("Error: {}", e);
                    return;
                }
                println!("Guard paused for {}.", duration_str);
                println!("  Resumes automatically at {}", expiry.format("%H:%M:%S"));
                println!("  Or manually: savants guard on");
            }
            None => {
                eprintln!("Usage: savants guard off [10m|1h|30s]");
                std::process::exit(1);
            }
        }
    } else {
        // Indefinite pause — create empty file
        if let Err(e) = fs::File::create(&pf) {
            eprintln!("Error: {}", e);
            return;
        }
        println!("Guard paused (indefinitely).");
        println!("  Resume: savants guard on");
    }
}

fn cmd_status() {
    let pf = pause_path();
    let rp = rules_path();
    let sf = stats_path();

    println!();

    // Pause status
    if pf.exists() {
        let content = fs::read_to_string(&pf).unwrap_or_default();
        if content.trim().is_empty() {
            println!("  Guard: {}", "PAUSED (indefinitely)".yellow());
        } else {
            println!("  Guard: {} (resumes at {})", "PAUSED".yellow(), content.trim());
        }
        println!("  Resume: savants guard on");
    } else if rp.exists() {
        let rules = read_json_array(&rp);
        let blocks = rules.iter().filter(|r| r.as_str().map(|s| s.contains("then block")).unwrap_or(false)).count();
        let suggests = rules.iter().filter(|r| r.as_str().map(|s| s.contains("then suggest")).unwrap_or(false)).count();
        let rewrites = rules.iter().filter(|r| r.as_str().map(|s| s.contains("then rewrite")).unwrap_or(false)).count();
        let asks = rules.iter().filter(|r| {
            r.as_str().map(|s| s.contains("then ask") || s.contains("then require_approval")).unwrap_or(false)
        }).count();

        println!("  Guard: {} ({} rules)", "ACTIVE".green(), rules.len());
        println!();
        println!("    {} block (hard stop)", blocks);
        println!("    {} suggest (alternative offered)", suggests);
        println!("    {} rewrite (silent command swap)", rewrites);
        println!("    {} ask (requires approval)", asks);
        println!();

        // Read preset from guard-state.json
        let gs = read_json_object(&state_path());
        let preset = gs.get("preset").and_then(|v| v.as_str()).unwrap_or("standard");
        println!("  Profile: {}", preset);
        println!("  Rules file: {}", rp.display());
    } else {
        println!("  Guard: {} (no rules loaded)", "INACTIVE".red());
        println!("  Activate: savants guard preset standard");
    }

    // Stats
    if sf.exists() {
        let events = read_stats_events();
        let blocks = events.iter().filter(|e| {
            e.get("action").and_then(|v| v.as_str()) == Some("block")
                && e.get("reason").and_then(|v| v.as_str()) == Some("guard_rule")
        }).count();
        let allows = events.iter().filter(|e| e.get("action").and_then(|v| v.as_str()) == Some("allow")).count();
        let suggests = events.iter().filter(|e| e.get("action").and_then(|v| v.as_str()) == Some("suggest")).count();
        let rewrites = events.iter().filter(|e| e.get("action").and_then(|v| v.as_str()) == Some("rewrite")).count();
        let asks = events.iter().filter(|e| e.get("action").and_then(|v| v.as_str()) == Some("ask")).count();
        let total = events.len();

        println!();
        println!("  Recent activity:");
        println!("    {} total events", total);
        println!("    {} blocked, {} suggested, {} rewritten, {} asked", blocks, suggests, rewrites, asks);
        println!("    {} allowed", allows);

        if total > 0 {
            let block_pct = blocks as f64 / total as f64 * 100.0;
            println!("    Block rate: {:.1}% ({}/{} events)", block_pct, blocks, total);
            if blocks == 0 && total > 50 {
                println!("    Note: 0 blocks in {} events. Your guard rules are active but", total);
                println!("    no dangerous actions have been attempted. This is normal for safe workflows.");
            }
        }

        // Last event timestamp
        if let Some(last) = events.last() {
            if let Some(ts) = last.get("ts").and_then(|v| v.as_str()) {
                println!("    Last event: {}", ts);
            }
        }

        // Top triggered rules
        let triggered: Vec<&Value> = events.iter().filter(|e| {
            e.get("reason").and_then(|v| v.as_str()) == Some("guard_rule")
                && e.get("detail").and_then(|v| v.as_str()).is_some()
        }).collect();

        if !triggered.is_empty() {
            let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
            for e in &triggered {
                if let Some(detail) = e.get("detail").and_then(|v| v.as_str()) {
                    *counts.entry(detail).or_insert(0) += 1;
                }
            }
            let mut sorted: Vec<_> = counts.into_iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(&a.1));
            let top: Vec<_> = sorted.into_iter().take(3).collect();
            if !top.is_empty() {
                println!();
                println!("  Top triggered rules:");
                for (rule, count) in &top {
                    println!("    {}x  {}", count, rule);
                }
            }
        }

        // Never-triggered rules audit
        if rp.exists() {
            let rules = read_json_array(&rp);
            if !rules.is_empty() {
                let triggered_details: std::collections::HashSet<String> = events.iter()
                    .filter(|e| e.get("reason").and_then(|v| v.as_str()) == Some("guard_rule"))
                    .filter_map(|e| e.get("detail").and_then(|v| v.as_str()).map(|s| s.to_string()))
                    .collect();
                let never_triggered: Vec<_> = rules.iter()
                    .filter_map(|r| r.as_str())
                    .filter(|r| !triggered_details.contains(*r))
                    .collect();
                if !never_triggered.is_empty() {
                    println!("    Never triggered: {} rules", never_triggered.len());
                    for r in never_triggered.iter().take(5) {
                        println!("      {}", r);
                    }
                    if never_triggered.len() > 5 {
                        println!("      ... and {} more (run savants guard list)", never_triggered.len() - 5);
                    }
                }
            }
        }

        // Recent non-allow events
        let non_allow: Vec<&Value> = events.iter()
            .filter(|e| e.get("action").and_then(|v| v.as_str()) != Some("allow"))
            .collect();
        if !non_allow.is_empty() {
            let recent: Vec<_> = non_allow.iter().rev().take(3).collect();
            println!();
            println!("  Recent guard events:");
            for e in recent.iter().rev() {
                let ts = e.get("ts").and_then(|v| v.as_str()).unwrap_or("?");
                let ts_short = if ts.len() > 19 { &ts[..19] } else { ts };
                let action = e.get("action").and_then(|v| v.as_str()).unwrap_or("?");
                let detail = e.get("detail").and_then(|v| v.as_str()).unwrap_or("");
                let tool = e.get("tool").and_then(|v| v.as_str()).unwrap_or("?");
                println!("    [{}] {} {}: {}", ts_short, action, tool, detail);
            }
        }
    }

    // Telemetry status
    let main_state_path = savants_dir().join("state.json");
    if main_state_path.exists() {
        let state = read_json_object(&main_state_path);
        let env_off = std::env::var("DO_NOT_TRACK").as_deref() == Ok("1")
            || std::env::var("SAVANTS_DO_NOT_TRACK").as_deref() == Ok("1");

        println!();
        if env_off {
            println!("  Telemetry: disabled (env var)");
        } else {
            let enabled = state.get("telemetry_enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            if enabled {
                let tid = state.get("telemetry_id").and_then(|v| v.as_str()).unwrap_or("");
                if tid.len() > 12 {
                    println!("  Telemetry: on (id: {}...)", &tid[..12]);
                } else {
                    println!("  Telemetry: on (id: {})", tid);
                }
            } else {
                println!("  Telemetry: off");
            }
        }
        println!("    Manage: savants config telemetry [on|off|status]");

        // Cloud sync status
        let has_token = state.get("cloud_token")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if has_token {
            println!("  Cloud sync: connected (savants guard sync to push events)");
        } else {
            println!("  Cloud sync: not configured (run savants connect for team features)");
        }
    } else {
        println!("  Cloud sync: not configured (run savants connect for team features)");
    }
    println!();
}

fn cmd_list() {
    let rp = rules_path();
    if !rp.exists() {
        println!("No guard rules active. Run: savants guard preset standard");
        return;
    }

    // Show pause status
    let pf = pause_path();
    if pf.exists() {
        let content = fs::read_to_string(&pf).unwrap_or_default();
        if content.trim().is_empty() {
            println!("  {} — run: savants guard on", "PAUSED (indefinitely)".yellow());
        } else {
            println!("  {} (resumes at {})", "PAUSED".yellow(), content.trim());
        }
        println!();
    }

    let rules = read_json_array(&rp);
    println!("{} active guard rules:", rules.len());
    println!();
    for (i, r) in rules.iter().enumerate() {
        let rule_str = r.as_str().unwrap_or("");
        let icon = if rule_str.contains("then block") {
            "x"
        } else {
            "!"
        };
        println!("  {:>2}. [{}] {}", i + 1, icon, rule_str);
    }
    println!();
    println!("Disable a rule: savants guard disable <number>");
}

fn cmd_add(args: &[String]) {
    let rule = args.join(" ");
    if rule.is_empty() {
        println!("Usage: savants guard add \"when tool eq 'Bash' and command contains 'rm' then block\"");
        std::process::exit(1);
    }

    let rp = rules_path();
    if !rp.exists() {
        write_json_array(&rp, &[]);
    }

    let mut rules = read_json_array(&rp);
    let rule_val = Value::String(rule.clone());
    if rules.contains(&rule_val) {
        println!("Rule already exists");
    } else {
        rules.push(rule_val);
        write_json_array(&rp, &rules);
        println!("Added: {}", rule);
        println!("Total: {} rules active", rules.len());
    }
}

fn cmd_remove(args: &[String]) {
    let rule = args.join(" ");
    if rule.is_empty() {
        println!("Usage: savants guard remove \"<rule text>\"");
        std::process::exit(1);
    }

    let rp = rules_path();
    let mut rules = read_json_array(&rp);
    let rule_val = Value::String(rule.clone());
    if let Some(pos) = rules.iter().position(|r| r == &rule_val) {
        rules.remove(pos);
        write_json_array(&rp, &rules);
        println!("Removed: {}", rule);
        println!("Total: {} rules active", rules.len());
    } else {
        println!("Rule not found");
    }
}

fn cmd_disable(args: &[String]) {
    let target = args.join(" ");
    if target.is_empty() {
        println!("Usage:");
        println!("  savants guard disable 3          # disable rule #3 (see 'savants guard list')");
        println!("  savants guard disable 'rm -rf'   # disable rules matching 'rm -rf'");
        std::process::exit(1);
    }

    let rp = rules_path();
    if !rp.exists() {
        println!("No rules loaded.");
        std::process::exit(1);
    }

    let mut rules = read_json_array(&rp);
    let mut disabled = read_json_array(&disabled_path());
    let mut removed: Vec<Value> = Vec::new();

    // Try as number first
    if let Ok(idx) = target.parse::<usize>() {
        let idx = idx - 1; // 1-based to 0-based
        if idx < rules.len() {
            removed.push(rules.remove(idx));
        } else {
            println!("Rule #{} not found. You have {} rules.", idx + 1, rules.len());
            std::process::exit(1);
        }
    } else {
        // Substring match
        let target_lower = target.to_lowercase();
        let (matched, remaining): (Vec<_>, Vec<_>) = rules.into_iter().partition(|r| {
            r.as_str().map(|s| s.to_lowercase().contains(&target_lower)).unwrap_or(false)
        });
        removed = matched;
        rules = remaining;
    }

    if removed.is_empty() {
        println!("No rules matching \"{}\"", target);
        println!("Run: savants guard list");
    } else {
        for r in &removed {
            if !disabled.contains(r) {
                disabled.push(r.clone());
            }
            println!("Disabled: {}", r.as_str().unwrap_or(""));
        }
        write_json_array(&rp, &rules);
        write_json_array(&disabled_path(), &disabled);
        println!("{} rules remaining", rules.len());
        println!("Re-enable with: savants guard enable <number>");
    }
}

fn cmd_enable(args: &[String]) {
    let target = args.first().map(|s| s.as_str()).unwrap_or("");
    if target.is_empty() {
        println!("Usage:");
        println!("  savants guard enable 1           # re-enable disabled rule #1");
        println!("  savants guard enable 'rm -rf'    # re-enable rules matching 'rm -rf'");
        println!();
        println!("See disabled rules: savants guard disabled");
        std::process::exit(1);
    }

    let dp = disabled_path();
    if !dp.exists() {
        println!("No disabled rules found.");
        return;
    }

    let mut disabled = read_json_array(&dp);
    if disabled.is_empty() {
        println!("No disabled rules to re-enable.");
        return;
    }

    let mut rules = read_json_array(&rules_path());
    let mut restored: Vec<Value> = Vec::new();

    if let Ok(idx) = target.parse::<usize>() {
        let idx = idx - 1;
        if idx < disabled.len() {
            restored.push(disabled.remove(idx));
        } else {
            println!("Disabled rule #{} not found. You have {} disabled rules.", idx + 1, disabled.len());
            std::process::exit(1);
        }
    } else {
        let target_lower = target.to_lowercase();
        let (matched, remaining): (Vec<_>, Vec<_>) = disabled.into_iter().partition(|r| {
            r.as_str().map(|s| s.to_lowercase().contains(&target_lower)).unwrap_or(false)
        });
        restored = matched;
        disabled = remaining;
    }

    if restored.is_empty() {
        println!("No disabled rules matching \"{}\"", target);
        println!("Run: savants guard disabled");
    } else {
        for r in &restored {
            if !rules.contains(r) {
                rules.push(r.clone());
            }
            println!("Re-enabled: {}", r.as_str().unwrap_or(""));
        }
        write_json_array(&rules_path(), &rules);
        write_json_array(&dp, &disabled);
        println!("{} rules active", rules.len());
    }
}

fn cmd_disabled() {
    let dp = disabled_path();
    let disabled = read_json_array(&dp);
    if disabled.is_empty() {
        println!("No disabled rules.");
        return;
    }
    println!("{} disabled rules:", disabled.len());
    println!();
    for (i, r) in disabled.iter().enumerate() {
        println!("  {:>2}. {}", i + 1, r.as_str().unwrap_or(""));
    }
    println!();
    println!("Re-enable: savants guard enable <number>");
}

fn cmd_stats() {
    let sf = stats_path();
    if !sf.exists() {
        println!("No guard events yet. Use Claude Code with savants guard enabled.");
        return;
    }

    let events = read_stats_events();
    let blocks = events.iter().filter(|e| {
        e.get("action").and_then(|v| v.as_str()) == Some("block")
            && e.get("reason").and_then(|v| v.as_str()) == Some("guard_rule")
    }).count();
    let allows = events.iter().filter(|e| e.get("action").and_then(|v| v.as_str()) == Some("allow")).count();
    let total = events.len();

    println!("Guard Statistics");
    println!("{}", "=".repeat(40));
    println!("Total intercepted:  {}", total);
    println!("Blocked by guard:   {}", blocks);
    println!("Allowed:            {}", allows);
    println!();

    if blocks > 0 {
        println!("Recent blocks:");
        let block_events: Vec<&Value> = events.iter().filter(|e| {
            e.get("action").and_then(|v| v.as_str()) == Some("block")
                && e.get("reason").and_then(|v| v.as_str()) == Some("guard_rule")
        }).collect();
        for b in block_events.iter().rev().take(5).rev() {
            let tool = b.get("tool").and_then(|v| v.as_str()).unwrap_or("?");
            let detail = b.get("detail").and_then(|v| v.as_str()).unwrap_or("");
            println!("  \u{1f6d1} {} \u{2014} {}", tool, detail);
        }
    }

    println!();
    let rules = read_json_array(&rules_path());
    println!("Active rules:       {}", rules.len());
    println!("Blocks prevented:   {}", blocks);
    if blocks > 0 {
        println!();
        println!("  Your guardrails prevented {} potentially dangerous actions.", blocks);
    }
    println!();
    println!("--- Upgrade to Pro ---");
    println!("  See what your TEAM blocked:        savants.cloud/dashboard/guard-log");
    println!("  Update rules without restarting:   savants.cloud/dashboard/guard-rules");
    println!("  Share rules across all developers: managed mode");
}

fn cmd_profiles() {
    println!("Available guard profiles:");
    println!();
    println!("  Core profiles:");
    println!("  minimal            10 rules  Catastrophic actions only");
    println!("  standard           25 rules  Recommended for daily use");
    println!("  paranoid           26 rules  Maximum safety");
    println!("  comprehensive     232 rules  Everything (all categories combined)");
    println!();
    println!("  Category profiles:");
    println!("  filesystem-safe    21 rules  File system destruction (rm -rf, dd, mkfs, shred)");
    println!("  credentials-safe   56 rules  Sensitive files, API keys, SSH keys, .env, secrets");
    println!("  git-safe           20 rules  Git force push, reset hard, branch delete, filter-branch");
    println!("  database-safe      16 rules  DROP, DELETE/UPDATE without WHERE, TRUNCATE, Redis FLUSH");
    println!("  k8s-safe           26 rules  Kubernetes, Docker, Helm (namespace delete, privileged)");
    println!("  cloud-safe         17 rules  AWS, GCP, Azure, Terraform, Pulumi destroy/delete");
    println!("  network-safe       22 rules  curl+secrets, curl|sh, reverse tunnels, ngrok, scp");
    println!("  publish-safe       16 rules  npm/PyPI/crates.io publish, supply chain attacks");
    println!("  system-safe        20 rules  chmod 777, useradd, iptables, reboot, kill");
    println!("  cicd-safe           8 rules  Workflow edits, secret deletion, Makefile modification");
    println!("  persistence-safe   14 rules  Reverse shells, crontab, systemd units, shell rc files");
    println!();
    println!("  Legacy profiles:");
    println!("  secrets            27 rules  Credential and token protection (use credentials-safe)");
    println!("  infra-safe         13 rules  Infrastructure protection (use k8s-safe+cloud-safe)");
    println!();
    println!("Combine with +: savants guard preset standard+credentials-safe+git-safe");
}

fn cmd_reset() {
    let rp = rules_path();
    let pf = pause_path();
    if rp.exists() {
        fs::remove_file(&rp).ok();
    }
    if pf.exists() {
        fs::remove_file(&pf).ok();
    }
    println!("Guard rules cleared. No protection active.");
    println!("Run: savants guard preset standard");
}

fn cmd_why() {
    let sf = stats_path();
    if !sf.exists() {
        println!("No guard events recorded yet.");
        return;
    }

    let events = read_stats_events();
    let blocks: Vec<&Value> = events.iter()
        .filter(|e| e.get("action").and_then(|v| v.as_str()) == Some("block"))
        .collect();

    if blocks.is_empty() {
        println!("No blocked events found.");
        return;
    }

    let last = blocks.last().unwrap();
    let ts = last.get("ts").and_then(|v| v.as_str()).unwrap_or("");
    let detail = last.get("detail").and_then(|v| v.as_str()).unwrap_or("");
    let rule = last.get("matched_rule")
        .or_else(|| last.get("detail"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let action = last.get("action").and_then(|v| v.as_str()).unwrap_or("block");

    let ago = relative_time(ts);

    println!("Last blocked: {}", ago);
    println!("  Command: {}", detail);
    println!("  Rule: {}", rule);
    println!("  Action: {}", action);
}

fn cmd_routing(args: &[String]) {
    let action = args.first().map(|s| s.as_str()).unwrap_or("status");
    let rf = routing_path();

    match action {
        "on" => {
            if let Some(parent) = rf.parent() {
                fs::create_dir_all(parent).ok();
            }
            fs::File::create(&rf).ok();
            println!("Smart routing ENABLED.");
            println!("  grep \u{2192} semantic_search, read \u{2192} file_skeleton");
            println!("  Savants will redirect code search to indexed tools.");
        }
        "off" => {
            if rf.exists() {
                fs::remove_file(&rf).ok();
            }
            println!("Smart routing DISABLED.");
            println!("  grep, read, cat work normally. Only guard rules are active.");
        }
        _ => {
            if rf.exists() {
                println!("Smart routing: ON");
                println!("  Turn off: savants guard routing off");
            } else {
                println!("Smart routing: OFF (default)");
                println!("  Turn on:  savants guard routing on");
            }
        }
    }
}

// ── Install ────────────────────────────────────────────────────────────────

fn cmd_install(args: &[String]) {
    let profile_name = match args.first() {
        Some(s) => s.as_str(),
        None => {
            println!("Usage: savants guard install <source>");
            println!();
            println!("Sources:");
            println!("  @user/name          Cloud profile (latest version)");
            println!("  @user/name@1.2.0    Cloud profile (exact version)");
            println!("  @user/name@^1       Cloud profile (semver range)");
            println!("  nixos-safe           Community profile from GitHub");
            println!("  https://...          Raw URL to a JSON rules file");
            println!();
            println!("Examples:");
            println!("  savants guard install @miguel/nixos-flake-only");
            println!("  savants guard install @miguel/nixos-flake-only@^1");
            println!("  savants guard install nixos-safe");
            println!("  savants guard install https://example.com/rules.json");
            std::process::exit(1);
        }
    };

    let custom_dir = custom_profiles_dir();
    fs::create_dir_all(&custom_dir).ok();

    if profile_name.starts_with('@') {
        install_cloud(profile_name, &custom_dir);
    } else if profile_name.starts_with("https://") || profile_name.starts_with("http://") {
        install_url(profile_name, &custom_dir);
    } else {
        install_community(profile_name, &custom_dir);
    }
}

fn install_cloud(profile_name: &str, custom_dir: &PathBuf) {
    let handle_raw = &profile_name[1..]; // strip leading @
    let (handle, version_spec) = if let Some(at_pos) = handle_raw.rfind('@') {
        // Check it's not the owner/name separator (first @)
        let candidate = &handle_raw[..at_pos];
        if candidate.contains('/') {
            (candidate.to_string(), handle_raw[at_pos + 1..].to_string())
        } else {
            (handle_raw.to_string(), String::new())
        }
    } else {
        (handle_raw.to_string(), String::new())
    };

    let parts: Vec<&str> = handle.splitn(2, '/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        eprintln!("Invalid handle. Use: @owner/name");
        std::process::exit(1);
    }
    let owner = parts[0];
    let name = parts[1];

    println!("Installing @{}/{}...", owner, name);

    let api_url = if version_spec.is_empty() {
        format!("{}/{}/{}", cloud_api(), owner, name)
    } else {
        format!("{}/{}/{}/{}", cloud_api(), owner, name, version_spec)
    };

    let response = match http_get(&api_url) {
        Ok(r) => r,
        Err(_) => {
            eprintln!("  Profile @{}/{} not found on savants.cloud", owner, name);
            std::process::exit(1);
        }
    };

    let data: Value = match serde_json::from_str(&response) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("  Error: invalid response from cloud API");
            std::process::exit(1);
        }
    };

    let installed_version = data.get("version").and_then(|v| v.as_str()).unwrap_or("");
    let rules = match data.get("rules") {
        Some(r) => r.clone(),
        None => {
            eprintln!("  Error: invalid response from cloud API");
            std::process::exit(1);
        }
    };

    if installed_version.is_empty() {
        eprintln!("  Error: invalid response from cloud API");
        std::process::exit(1);
    }

    let dest = custom_dir.join(format!("{}.json", name));

    // Read previous version from lock file
    let lp = lock_path();
    let lock = read_json_object(&lp);
    let lock_key = format!("@{}/{}", owner, name);
    let prev_version = lock.get(&lock_key)
        .and_then(|v| v.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Write rules
    let rules_json = serde_json::to_string_pretty(&rules).unwrap_or_else(|_| "[]".to_string());
    fs::write(&dest, &rules_json).ok();
    let rule_count = rules.as_array().map(|a| a.len()).unwrap_or(0);
    println!("  Installed: @{}/{}@{} ({} rules)", owner, name, installed_version, rule_count);

    // Update lock file
    let mut lock = read_json_object(&lp);
    let mut entry = serde_json::Map::new();
    entry.insert("version".to_string(), Value::String(installed_version.to_string()));
    let pinned = if version_spec.is_empty() { installed_version } else { &version_spec };
    entry.insert("pinned".to_string(), Value::String(pinned.to_string()));
    entry.insert("installed".to_string(), Value::String(Utc::now().format("%Y-%m-%d").to_string()));
    if !prev_version.is_empty() {
        entry.insert("previous".to_string(), Value::String(prev_version));
    }
    lock.insert(lock_key.clone(), Value::Object(entry));
    write_json_object(&lp, &lock);

    // Fire-and-forget install notification (best effort, don't block)
    let notify_url = format!("{}/{}/{}/install", cloud_api(), owner, name);
    let payload = serde_json::json!({"version": installed_version});
    // Use a thread so we don't block
    std::thread::spawn(move || {
        let _ = http_post_json(&notify_url, &payload, None);
    });

    println!();
    println!("  Activate: savants guard preset standard+{}", name);
    println!("  View:     cat {}", dest.display());
}

fn install_url(url: &str, custom_dir: &PathBuf) {
    let url_name = url.rsplit('/').next().unwrap_or("rules").trim_end_matches(".json");
    let dest = custom_dir.join(format!("{}.json", url_name));

    println!("Installing from URL...");

    let body = match http_get(url) {
        Ok(b) => b,
        Err(_) => {
            eprintln!("  Failed to download from URL");
            std::process::exit(1);
        }
    };

    // Validate JSON
    match serde_json::from_str::<Vec<Value>>(&body) {
        Ok(rules) => {
            fs::write(&dest, &body).ok();
            println!("  Installed: {} rules", rules.len());
            println!();
            println!("  Activate: savants guard preset standard+{}", url_name);
            println!("  View:     cat {}", dest.display());
        }
        Err(_) => {
            eprintln!("  Error: downloaded file is not valid JSON");
            std::process::exit(1);
        }
    }
}

fn install_community(name: &str, custom_dir: &PathBuf) {
    let dest = custom_dir.join(format!("{}.json", name));
    let url = format!(
        "https://raw.githubusercontent.com/savants/savants/main/packages/guard-profiles/community/{}.json",
        name
    );

    println!("Installing {}...", name);

    match http_get(&url) {
        Ok(body) => {
            match serde_json::from_str::<Vec<Value>>(&body) {
                Ok(rules) => {
                    fs::write(&dest, &body).ok();
                    println!("  Installed: {} rules", rules.len());
                    println!();
                    println!("  Activate: savants guard preset standard+{}", name);
                    println!("  View:     cat {}", dest.display());
                }
                Err(_) => {
                    eprintln!("  Error: downloaded file is not valid JSON");
                    fs::remove_file(&dest).ok();
                    std::process::exit(1);
                }
            }
        }
        Err(_) => {
            eprintln!("  Profile '{}' not found in community registry.", name);
            eprintln!();
            eprintln!("  Browse available: https://github.com/savants/savants/tree/main/packages/guard-profiles/community");
            eprintln!("  Create your own:  ~/.savants/custom-profiles/{}.json", name);
            std::process::exit(1);
        }
    }
}

// ── Share ──────────────────────────────────────────────────────────────────

fn cmd_share(args: &[String]) {
    let mut profile_name = String::new();
    let mut version = "1.0.0".to_string();
    let mut description = String::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--version" => {
                if i + 1 < args.len() {
                    version = args[i + 1].clone();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--description" => {
                if i + 1 < args.len() {
                    description = args[i + 1].clone();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            _ => {
                if profile_name.is_empty() {
                    profile_name = args[i].clone();
                }
                i += 1;
            }
        }
    }

    if profile_name.is_empty() {
        println!("Usage: savants guard share <profile-name> [--version 1.0.0] [--description \"...\"]");
        println!();
        println!("Publishes ~/.savants/custom-profiles/<name>.json to savants.cloud");
        println!();
        println!("Examples:");
        println!("  savants guard share my-rules --version 1.0.0");
        println!("  savants guard share nixos-flake-only --version 1.1.0 --description \"NixOS flake safety\"");
        std::process::exit(1);
    }

    let source = custom_profiles_dir().join(format!("{}.json", profile_name));
    if !source.exists() {
        eprintln!("Profile not found: {}", source.display());
        eprintln!();
        eprintln!("Create it first:");
        eprintln!("  mkdir -p {}", custom_profiles_dir().display());
        eprintln!("  echo '[\"when tool eq ...\" ]' > {}", source.display());
        std::process::exit(1);
    }

    let api_key = match get_api_key() {
        Some(k) => k,
        None => {
            eprintln!("Not authenticated. Run: savants connect");
            std::process::exit(1);
        }
    };

    let rules = read_json_array(&source);
    let mut payload = serde_json::json!({
        "name": profile_name,
        "version": version,
        "rules": rules,
    });
    if !description.is_empty() {
        payload.as_object_mut().unwrap().insert("description".to_string(), Value::String(description));
    }

    println!("Publishing {}@{}...", profile_name, version);

    match http_post_json(
        &format!("{}/publish", cloud_api()),
        &payload,
        Some(&api_key),
    ) {
        Ok(resp) => {
            let handle = resp.get("handle").and_then(|v| v.as_str()).unwrap_or("");
            println!("  Shared! Install with: savants guard install {}", handle);
        }
        Err(e) => {
            eprintln!("  Error: {}", e);
            std::process::exit(1);
        }
    }
}

// ── Rollback ───────────────────────────────────────────────────────────────

fn cmd_rollback(args: &[String]) {
    let profile_name = match args.first() {
        Some(s) => s.as_str(),
        None => {
            println!("Usage: savants guard rollback @owner/name");
            println!();
            println!("Restores the previously installed version from profiles.lock");
            std::process::exit(1);
        }
    };

    let handle = if profile_name.starts_with('@') {
        profile_name.to_string()
    } else {
        format!("@{}", profile_name)
    };

    let lp = lock_path();
    if !lp.exists() {
        eprintln!("No profiles.lock found. Nothing to rollback.");
        std::process::exit(1);
    }

    let lock = read_json_object(&lp);
    let entry = match lock.get(&handle) {
        Some(v) => v,
        None => {
            eprintln!("  {} not found in profiles.lock", handle);
            std::process::exit(1);
        }
    };

    let prev = entry.get("previous").and_then(|v| v.as_str()).unwrap_or("");
    if prev.is_empty() {
        eprintln!("  No previous version recorded for {}", handle);
        std::process::exit(1);
    }

    let current = entry.get("version").and_then(|v| v.as_str()).unwrap_or("?");
    println!("  Rolling back {} from {} to {}...", handle, current, prev);

    // Re-install the previous version
    let clean = handle.trim_start_matches('@');
    let install_arg = format!("@{}@{}", clean, prev);
    cmd_install(&[install_arg]);
}

// ── Versions ───────────────────────────────────────────────────────────────

fn cmd_versions(args: &[String]) {
    let profile_name = match args.first() {
        Some(s) => s.as_str(),
        None => {
            println!("Usage: savants guard versions @owner/name");
            std::process::exit(1);
        }
    };

    let handle = profile_name.trim_start_matches('@');
    let parts: Vec<&str> = handle.splitn(2, '/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        eprintln!("Invalid handle. Use: @owner/name");
        std::process::exit(1);
    }
    let (owner, name) = (parts[0], parts[1]);

    let url = format!("{}/{}/{}/versions", cloud_api(), owner, name);
    let data: Value = match http_get(&url).and_then(|body| {
        serde_json::from_str(&body).map_err(|e| format!("{}", e))
    }) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("  Profile @{}/{} not found", owner, name);
            std::process::exit(1);
        }
    };

    // Current installed version
    let lp = lock_path();
    let lock = read_json_object(&lp);
    let lock_key = format!("@{}/{}", owner, name);
    let current = lock.get(&lock_key)
        .and_then(|v| v.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let disp_handle = data.get("handle").and_then(|v| v.as_str()).unwrap_or(&lock_key);
    println!("Versions of {}:", disp_handle);
    println!();

    if let Some(versions) = data.get("versions").and_then(|v| v.as_array()) {
        for v in versions {
            let ver = v.get("version").and_then(|x| x.as_str()).unwrap_or("?");
            let rule_count = v.get("rule_count").and_then(|x| x.as_u64()).unwrap_or(0);
            let installs = v.get("installs").and_then(|x| x.as_u64()).unwrap_or(0);
            let created = v.get("created_at").and_then(|x| x.as_str()).unwrap_or("");
            let marker = if ver == current { " (installed)" } else { "" };
            println!("  {:>10}  {:>3} rules  {:>4} installs  {}{}", ver, rule_count, installs, created, marker);
        }
    }
}

// ── Browse ─────────────────────────────────────────────────────────────────

fn cmd_browse(args: &[String]) {
    let mut tag_filter = String::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--tag" && i + 1 < args.len() {
            tag_filter = args[i + 1].clone();
            i += 2;
        } else {
            i += 1;
        }
    }

    let url = if tag_filter.is_empty() {
        format!("{}/browse", cloud_api())
    } else {
        format!("{}/browse?tag={}", cloud_api(), tag_filter)
    };

    let data: Value = match http_get(&url).and_then(|body| {
        serde_json::from_str(&body).map_err(|e| format!("{}", e))
    }) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("  Could not reach savants.cloud");
            std::process::exit(1);
        }
    };

    let profiles = if data.is_array() {
        data.as_array().cloned().unwrap_or_default()
    } else {
        data.get("profiles").and_then(|v| v.as_array()).cloned().unwrap_or_default()
    };

    if profiles.is_empty() {
        println!("No profiles found.");
        return;
    }

    if tag_filter.is_empty() {
        println!("Popular Guard Profiles");
    } else {
        println!("Guard Profiles (tag: {})", tag_filter);
    }
    println!();

    for p in &profiles {
        let handle = p.get("handle")
            .or_else(|| p.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let version = p.get("version").and_then(|v| v.as_str()).unwrap_or("?");
        let rule_count = p.get("rule_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let installs = p.get("installs").and_then(|v| v.as_u64()).unwrap_or(0);
        let desc = p.get("description").and_then(|v| v.as_str()).unwrap_or("");

        let inst_str = if installs >= 1000 {
            format!("{:.1}K", installs as f64 / 1000.0)
        } else {
            format!("{}", installs)
        };

        println!("  {:<25} v{:<6} {:>3} rules  {:>6} installs", handle, version, rule_count, inst_str);
        if !desc.is_empty() {
            println!("    {}", desc);
        }
        println!();
    }

    println!("Install: savants guard install @owner/name");
}

// ── Update ─────────────────────────────────────────────────────────────────

fn cmd_update(args: &[String]) {
    let mut target = String::new();
    let mut check_only = false;

    for a in args {
        match a.as_str() {
            "--check" => check_only = true,
            _ => {
                if target.is_empty() {
                    target = a.clone();
                }
            }
        }
    }

    let lp = lock_path();
    if !lp.exists() {
        println!("No profiles.lock found. Install a profile first:");
        println!("  savants guard install @owner/name");
        std::process::exit(1);
    }

    let lock = read_json_object(&lp);
    if lock.is_empty() {
        println!("No installed profiles to update.");
        return;
    }

    struct UpdateInfo {
        handle: String,
        current: String,
        latest: String,
        rules: Value,
    }

    let mut updates: Vec<UpdateInfo> = Vec::new();

    for (handle, entry) in &lock {
        if !target.is_empty() && handle != &target && handle != &format!("@{}", target) {
            continue;
        }

        let pinned = entry.get("pinned")
            .or_else(|| entry.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let current = entry.get("version").and_then(|v| v.as_str()).unwrap_or("");

        let clean = handle.trim_start_matches('@');
        let api_url = if pinned.starts_with('^') || pinned.starts_with('~') {
            format!("{}/{}/{}", cloud_api(), clean, pinned)
        } else {
            format!("{}/{}", cloud_api(), clean)
        };

        match http_get(&api_url).and_then(|body| {
            serde_json::from_str::<Value>(&body).map_err(|e| format!("{}", e))
        }) {
            Ok(data) => {
                let latest = data.get("version").and_then(|v| v.as_str()).unwrap_or("");
                if !latest.is_empty() && latest != current {
                    updates.push(UpdateInfo {
                        handle: handle.clone(),
                        current: current.to_string(),
                        latest: latest.to_string(),
                        rules: data.get("rules").cloned().unwrap_or(Value::Array(Vec::new())),
                    });
                } else {
                    println!("  {}: up to date ({})", handle, current);
                }
            }
            Err(e) => {
                println!("  {}: check failed ({})", handle, e);
            }
        }
    }

    if updates.is_empty() {
        println!();
        println!("All profiles are up to date.");
        return;
    }

    println!();

    let mut lock = read_json_object(&lp);
    for u in &updates {
        if check_only {
            println!("  {}: {} -> {} (update available)", u.handle, u.current, u.latest);
        } else {
            let name = u.handle.trim_start_matches('@').rsplit('/').next().unwrap_or("");
            let dest = custom_profiles_dir().join(format!("{}.json", name));
            fs::create_dir_all(custom_profiles_dir()).ok();
            let json = serde_json::to_string_pretty(&u.rules).unwrap_or_else(|_| "[]".to_string());
            fs::write(&dest, &json).ok();
            let rule_count = u.rules.as_array().map(|a| a.len()).unwrap_or(0);

            // Update lock
            if let Some(entry) = lock.get_mut(&u.handle).and_then(|v| v.as_object_mut()) {
                entry.insert("previous".to_string(), Value::String(u.current.clone()));
                entry.insert("version".to_string(), Value::String(u.latest.clone()));
                entry.insert("installed".to_string(), Value::String(Utc::now().format("%Y-%m-%d").to_string()));
            }

            println!("  {}: {} -> {} ({} rules)", u.handle, u.current, u.latest, rule_count);
        }
    }

    if !check_only && !updates.is_empty() {
        write_json_object(&lp, &lock);
        println!();
        println!("Updated {} profile(s).", updates.len());
    }

    if check_only && !updates.is_empty() {
        println!();
        println!("{} update(s) available. Run: savants guard update", updates.len());
    }
}

// ── Pin ────────────────────────────────────────────────────────────────────

fn cmd_pin(args: &[String]) {
    if args.len() < 2 {
        println!("Usage: savants guard pin @owner/name 1.2.0");
        println!();
        println!("Sets an exact version pin in profiles.lock.");
        println!("Re-downloads if current version doesn't match.");
        std::process::exit(1);
    }

    let profile_name = &args[0];
    let pin_version = &args[1];

    let handle = if profile_name.starts_with('@') {
        profile_name.clone()
    } else {
        format!("@{}", profile_name)
    };

    let lp = lock_path();
    if !lp.exists() {
        eprintln!("No profiles.lock found. Install the profile first:");
        eprintln!("  savants guard install {}", handle);
        std::process::exit(1);
    }

    let mut lock = read_json_object(&lp);
    let entry = match lock.get_mut(&handle).and_then(|v| v.as_object_mut()) {
        Some(e) => e,
        None => {
            eprintln!("{} not found in profiles.lock", handle);
            eprintln!("Install first: savants guard install {}", handle);
            std::process::exit(1);
        }
    };

    let current = entry.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string();
    entry.insert("pinned".to_string(), Value::String(pin_version.to_string()));
    write_json_object(&lp, &lock);

    println!("Pinned {} to version {}", handle, pin_version);

    if current != *pin_version {
        println!("Current version ({}) differs from pin ({})", current, pin_version);
        println!("Re-downloading...");
        let clean = handle.trim_start_matches('@');
        let install_arg = format!("@{}@{}", clean, pin_version);
        cmd_install(&[install_arg]);
    } else {
        println!("Current version already matches pin.");
    }
}

// ── Sync ───────────────────────────────────────────────────────────────────

fn cmd_sync(args: &[String]) {
    let subcmd = args.first().map(|s| s.as_str()).unwrap_or("");

    match subcmd {
        "push" => sync_push(),
        "pull" => sync_pull(),
        "status" => sync_status(),
        "auto" => sync_auto(&args[1..]),
        "events" => sync_events(),
        _ => {
            println!("Usage: savants guard sync <command>");
            println!();
            println!("Commands:");
            println!("  push          Push local guard config to cloud");
            println!("  pull          Pull guard config from cloud");
            println!("  status        Show local vs cloud sync status");
            println!("  auto on|off   Enable/disable automatic sync checking");
            println!("  events        Sync guard events (blocks/allows) to cloud");
        }
    }
}

fn sync_push() {
    let api_key = match get_api_key() {
        Some(k) => k,
        None => {
            eprintln!("Not authenticated. Run: savants connect");
            std::process::exit(1);
        }
    };

    let rp = rules_path();
    if !rp.exists() {
        eprintln!("No guard rules to push. Run: savants guard preset standard");
        std::process::exit(1);
    }

    let rules = read_json_array(&rp);
    let gs = read_json_object(&state_path());
    let preset = gs.get("preset").and_then(|v| v.as_str()).unwrap_or("");
    let machine_id = get_machine_id();

    let payload = serde_json::json!({
        "rules": rules,
        "preset": if preset.is_empty() { Value::Null } else { Value::String(preset.to_string()) },
        "custom_rules": [],
        "machine_id": machine_id,
    });

    match http_post_json(
        &format!("{}/config", cloud_guard_api()),
        &payload,
        Some(&api_key),
    ) {
        Ok(result) => {
            let version = result.get("version").and_then(|v| v.as_str())
                .or_else(|| result.get("version").and_then(|v| v.as_u64()).map(|_| ""))
                .unwrap_or("?");
            let version_str = if version.is_empty() {
                result.get("version").map(|v| v.to_string()).unwrap_or_else(|| "?".to_string())
            } else {
                version.to_string()
            };
            let count = result.get("rules_count").and_then(|v| v.as_u64()).unwrap_or(rules.len() as u64);
            println!("Config synced to cloud ({} rules, version {})", count, version_str);

            // Update sync state
            let sp = sync_path();
            let mut sync_state = read_json_object(&sp);
            sync_state.insert("local_version".to_string(), result.get("version").cloned().unwrap_or(Value::String(version_str.clone())));
            sync_state.insert("cloud_version".to_string(), result.get("version").cloned().unwrap_or(Value::String(version_str)));
            sync_state.insert("last_push".to_string(), Value::String(Utc::now().to_rfc3339()));
            sync_state.insert("machine_id".to_string(), Value::String(machine_id));
            write_json_object(&sp, &sync_state);
        }
        Err(e) => {
            eprintln!("Sync error: {}", e);
            std::process::exit(1);
        }
    }
}

fn sync_pull() {
    let api_key = match get_api_key() {
        Some(k) => k,
        None => {
            eprintln!("Not authenticated. Run: savants connect");
            std::process::exit(1);
        }
    };

    match http_get_json(
        &format!("{}/config", cloud_guard_api()),
        Some(&api_key),
    ) {
        Ok(result) => {
            let rules = result.get("rules").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let version = result.get("version").map(|v| v.to_string()).unwrap_or_else(|| "?".to_string());

            write_json_array(&rules_path(), &rules);
            println!("Pulled config from cloud ({} rules, version {})", rules.len(), version);

            // Update sync state
            let sp = sync_path();
            let mut sync_state = read_json_object(&sp);
            sync_state.insert("local_version".to_string(), result.get("version").cloned().unwrap_or(Value::Null));
            sync_state.insert("cloud_version".to_string(), result.get("version").cloned().unwrap_or(Value::Null));
            sync_state.insert("last_check".to_string(), Value::String(Utc::now().to_rfc3339()));
            sync_state.insert("machine_id".to_string(), Value::String(get_machine_id()));
            write_json_object(&sp, &sync_state);

            // Update guard-state.json with preset if returned
            if let Some(preset) = result.get("preset").and_then(|v| v.as_str()) {
                if !preset.is_empty() {
                    let mut gs = read_json_object(&state_path());
                    gs.insert("preset".to_string(), Value::String(preset.to_string()));
                    write_json_object(&state_path(), &gs);
                }
            }
        }
        Err(e) => {
            eprintln!("Pull error: {}", e);
            std::process::exit(1);
        }
    }
}

fn sync_status() {
    let sp = sync_path();
    let rp = rules_path();

    let sync_state = read_json_object(&sp);
    let local_rules = read_json_array(&rp);

    let local_version = sync_state.get("local_version").map(|v| v.to_string()).unwrap_or_else(|| "0".to_string());
    let cloud_version = sync_state.get("cloud_version").map(|v| v.to_string()).unwrap_or_else(|| "0".to_string());
    let auto_sync = sync_state.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    let last_check = sync_state.get("last_check").and_then(|v| v.as_str()).unwrap_or("");
    let last_push = sync_state.get("last_push").and_then(|v| v.as_str()).unwrap_or("");

    println!("Guard Sync Status");
    println!("  Local:  {} rules (version {}, updated {})", local_rules.len(), local_version, relative_time(last_push));
    println!("  Cloud:  version {} (checked {})", cloud_version, relative_time(last_check));

    if local_version == cloud_version && cloud_version != "0" {
        println!("  Status: IN SYNC");
    } else if cloud_version > local_version {
        println!("  Status: OUT OF SYNC \u{2014} run 'savants guard sync pull' to update");
    } else if local_version > cloud_version && cloud_version != "0" {
        println!("  Status: LOCAL AHEAD \u{2014} run 'savants guard sync push' to upload");
    } else {
        println!("  Status: NOT SYNCED \u{2014} run 'savants guard sync push' to start");
    }

    let auto_str = if auto_sync { "on (checks every 5 min)" } else { "off" };
    println!("  Auto-sync: {}", auto_str);
}

fn sync_auto(args: &[String]) {
    let action = args.first().map(|s| s.as_str()).unwrap_or("");
    let sp = sync_path();

    match action {
        "on" => {
            let mut sync_state = read_json_object(&sp);
            sync_state.insert("enabled".to_string(), Value::Bool(true));
            write_json_object(&sp, &sync_state);
            println!("Auto-sync enabled. Guard config will sync every 5 minutes.");
        }
        "off" => {
            let mut sync_state = read_json_object(&sp);
            sync_state.insert("enabled".to_string(), Value::Bool(false));
            write_json_object(&sp, &sync_state);
            println!("Auto-sync disabled.");
        }
        _ => {
            println!("Usage: savants guard sync auto on|off");
        }
    }
}

fn sync_events() {
    let api_key = match get_api_key() {
        Some(k) => k,
        None => {
            eprintln!("No API key found. Set SAVANTS_API_KEY or run savants connect.");
            eprintln!("  Sign up: savants.cloud/activate");
            std::process::exit(1);
        }
    };

    let sf = stats_path();
    if !sf.exists() {
        println!("No local events to sync.");
        return;
    }

    let sync_marker = savants_dir().join("guard-sync-offset");
    let offset: usize = fs::read_to_string(&sync_marker)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);

    let all_events = read_stats_events();
    let events: Vec<&Value> = all_events.iter().skip(offset).collect();

    if events.is_empty() {
        println!("No new events to sync.");
        return;
    }

    // Build event payloads
    let event_payloads: Vec<Value> = events.iter().map(|e| {
        let detail = e.get("detail").and_then(|v| v.as_str()).unwrap_or("");
        let action = e.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let tool = e.get("tool").and_then(|v| v.as_str()).unwrap_or("");
        let reason = e.get("reason").and_then(|v| v.as_str()).unwrap_or("");
        let ts = e.get("ts").and_then(|v| v.as_str()).unwrap_or("");

        let action_str = if action == "block" {
            &detail[..detail.len().min(120)]
        } else {
            ""
        };
        let matched_rule = if reason == "guard_rule" {
            Some(&detail[..detail.len().min(200)])
        } else {
            None
        };
        let result = if ["block", "suggest", "rewrite", "ask"].contains(&action) {
            "blocked"
        } else {
            "allowed"
        };

        let mut obj = serde_json::json!({
            "context_hash": format!("{}", detail.len()), // simple hash stand-in
            "action": action_str,
            "tool": tool,
            "result": result,
            "timestamp": ts,
        });
        if let Some(mr) = matched_rule {
            obj.as_object_mut().unwrap().insert("matched_rule".to_string(), Value::String(mr.to_string()));
        }
        obj
    }).collect();

    // Batch in groups of 200
    let mut total_sent = 0;
    for batch in event_payloads.chunks(200) {
        let payload = serde_json::json!({"events": batch});
        match http_post_json(
            "https://api.savants.cloud/api/v1/guard/events",
            &payload,
            Some(&api_key),
        ) {
            Ok(_) => total_sent += batch.len(),
            Err(e) => {
                eprintln!("Sync error: {}", e);
                break;
            }
        }
    }

    // Update offset
    let new_offset = offset + total_sent;
    fs::write(&sync_marker, new_offset.to_string()).ok();
    println!("Synced {} events to Savants Cloud.", total_sent);
    println!("  View: savants.cloud/dashboard/guard-analytics");
}

// ── Help ───────────────────────────────────────────────────────────────────

fn cmd_help() {
    println!("savants guard \u{2014} composable guardrails for AI coding agents");
    println!();
    println!("Commands:");
    println!("  preset <profiles>  Set active profiles (e.g. standard+secrets+git-safe)");
    println!("  install <source>   Install a profile (@user/name, community, or URL)");
    println!("  browse             Browse popular profiles on savants.cloud");
    println!("  update [name]      Update installed profiles to latest version");
    println!("  pin @u/n <ver>     Pin a profile to an exact version");
    println!("  share <name>       Share a profile to savants.cloud");
    println!("  versions @u/n      List all versions of a cloud profile");
    println!("  rollback @u/n      Rollback to previous installed version");
    println!("  on                 Resume guard protection");
    println!("  off [duration]     Pause guard (e.g. off 10m, off 1h, off = indefinite)");
    println!("  status             Show guard state (active/paused/inactive)");
    println!("  why / last-block   Show the last blocked event");
    println!("  disable <n|text>   Disable a specific rule (reversible)");
    println!("  enable <n|text>    Re-enable a disabled rule");
    println!("  disabled           List all disabled rules");
    println!("  add <rule>         Add a custom guard rule");
    println!("  remove <rule>      Remove a guard rule");
    println!("  list               Show all active rules");
    println!("  stats              Show guard statistics (blocks, allows)");
    println!("  sync push          Push guard config to cloud");
    println!("  sync pull          Pull guard config from cloud");
    println!("  sync status        Show sync status (local vs cloud)");
    println!("  sync auto on|off   Toggle automatic sync");
    println!("  sync events        Sync guard events to cloud");
    println!("  profiles           List available profiles");
    println!("  routing on|off     Toggle smart code routing");
    println!("  reset              Clear all guard rules");
    println!();
    println!("Quick start:");
    println!("  savants guard preset standard");
    println!();
    println!("When blocked:");
    println!("  savants guard off 10m     # pause for 10 minutes");
    println!("  savants guard disable 3   # disable rule #3 only");
    println!("  SAVANTS_GUARD=off claude  # disable for one session");
}

// ═══════════════════════════════════════════════════════════════════════════
//  Entry point — called from main.rs
// ═══════════════════════════════════════════════════════════════════════════

pub fn run(args: Vec<String>) {
    ensure_profiles();

    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    let rest = if args.len() > 1 { &args[1..] } else { &[] };

    match cmd {
        "preset" => cmd_preset(rest),
        "on" => cmd_on(),
        "off" => cmd_off(rest),
        "status" => cmd_status(),
        "list" => cmd_list(),
        "add" => cmd_add(rest),
        "remove" => cmd_remove(rest),
        "disable" => cmd_disable(rest),
        "enable" => cmd_enable(rest),
        "disabled" => cmd_disabled(),
        "stats" => cmd_stats(),
        "profiles" => cmd_profiles(),
        "reset" => cmd_reset(),
        "sync" => cmd_sync(rest),
        "install" => cmd_install(rest),
        "share" | "publish" => cmd_share(rest),
        "rollback" => cmd_rollback(rest),
        "versions" => cmd_versions(rest),
        "browse" => cmd_browse(rest),
        "update" => cmd_update(rest),
        "pin" => cmd_pin(rest),
        "why" | "last-block" => cmd_why(),
        "routing" => cmd_routing(rest),
        _ => cmd_help(),
    }
}

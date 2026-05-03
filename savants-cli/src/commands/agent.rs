//! Savants infrastructure agent.
//!
//! Runs on a server/cluster, registers with savants.cloud, polls for queries,
//! executes them locally (host health, kubectl, logs), sends results back.
//!
//! Usage: savants agent start [--name my-server]

use std::collections::HashMap;

const POLL_INTERVAL_SECS: u64 = 5;
const HEARTBEAT_INTERVAL_SECS: u64 = 60;
const WATCH_INTERVAL_SECS: u64 = 60; // Health check every 60s

// Thresholds - no config needed, sensible defaults
const MEMORY_WARN_PCT: f64 = 85.0;
const MEMORY_CRIT_PCT: f64 = 95.0;
const DISK_WARN_PCT: u32 = 85;
const DISK_CRIT_PCT: u32 = 95;
const LOAD_WARN_MULTIPLIER: f64 = 2.0; // load > 2x CPU count

pub async fn start(name: Option<String>) {
    let state = crate::config::State::load();
    let cloud_url = "https://api.savants.cloud";
    let token = match state.cloud_token() {
        Some(t) => t,
        None => {
            eprintln!("Not connected to cloud. Run: savants connect");
            std::process::exit(1);
        }
    };

    let agent_name = name.unwrap_or_else(|| {
        hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    });

    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let version = env!("CARGO_PKG_VERSION").to_string();

    // Detect capabilities
    let mut capabilities = vec!["host_health".to_string(), "security_scan".to_string()];
    if which("kubectl") {
        capabilities.push("pod_status".to_string());
        capabilities.push("pod_logs".to_string());
        capabilities.push("k8s_events".to_string());
    }
    if which("docker") {
        capabilities.push("docker_status".to_string());
    }
    if which("aws") || std::env::var("AWS_ACCESS_KEY_ID").is_ok() {
        capabilities.push("aws_health".to_string());
    }
    if which("gcloud") || std::env::var("GOOGLE_APPLICATION_CREDENTIALS").is_ok() {
        capabilities.push("gcp_health".to_string());
    }
    if std::env::var("CF_API_TOKEN").is_ok() || std::env::var("CLOUDFLARE_API_TOKEN").is_ok() {
        capabilities.push("cloudflare_health".to_string());
    }

    println!("Savants agent starting...");
    println!("  Name: {}", agent_name);
    println!("  OS: {} / {}", os, arch);
    println!("  Capabilities: {:?}", capabilities);
    println!("  Cloud: {}", cloud_url);

    // Register with cloud
    let client = reqwest::Client::new();
    let reg = client
        .post(format!("{}/api/v1/agents/register", cloud_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({
            "name": agent_name,
            "hostname": &agent_name,
            "os": os,
            "arch": arch,
            "capabilities": capabilities,
            "version": version,
        }))
        .send()
        .await;

    let agent_id = match reg {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let id = body["agent_id"].as_str().unwrap_or("").to_string();
            println!("  Registered: {}", id);
            id
        }
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            eprintln!("Registration failed ({}): {}", status, text);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Cannot reach cloud: {}", e);
            std::process::exit(1);
        }
    };

    println!("  Polling for queries...\n");

    let mut last_heartbeat = std::time::Instant::now();
    let mut last_watch = std::time::Instant::now() - std::time::Duration::from_secs(WATCH_INTERVAL_SECS); // trigger immediately
    let mut known_issues: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Main loop: poll for queries + proactive health watch
    loop {
        // Proactive health watch
        if last_watch.elapsed().as_secs() >= WATCH_INTERVAL_SECS {
            let findings = watch_health();
            for finding in &findings {
                // Only notify once per issue (until it clears)
                if !known_issues.contains(&finding.key) {
                    known_issues.insert(finding.key.clone());
                    println!("[watch] {} - {}", finding.severity, finding.message);

                    // Send to cloud for notification routing
                    let _ = client
                        .post(format!("{}/api/v1/agents/notify", cloud_url))
                        .header("Authorization", format!("Bearer {}", token))
                        .json(&serde_json::json!({
                            "agent_id": agent_id,
                            "agent_name": agent_name,
                            "severity": finding.severity,
                            "category": finding.category,
                            "title": finding.title,
                            "message": finding.message,
                            "key": finding.key,
                            "metadata": finding.metadata,
                        }))
                        .send()
                        .await;
                }
            }

            // Clear resolved issues
            let active_keys: std::collections::HashSet<String> =
                findings.iter().map(|f| f.key.clone()).collect();
            known_issues.retain(|k| active_keys.contains(k));

            last_watch = std::time::Instant::now();
        }

        // Heartbeat
        if last_heartbeat.elapsed().as_secs() >= HEARTBEAT_INTERVAL_SECS {
            let _ = client
                .post(format!("{}/api/v1/agents/heartbeat", cloud_url))
                .header("Authorization", format!("Bearer {}", token))
                .json(&serde_json::json!({"agent_id": agent_id}))
                .send()
                .await;
            last_heartbeat = std::time::Instant::now();
        }

        // Poll for queries
        let poll = client
            .get(format!(
                "{}/api/v1/agents/poll?agent_id={}",
                cloud_url, agent_id
            ))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await;

        if let Ok(resp) = poll {
            if resp.status().is_success() {
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                if let Some(queries) = body["queries"].as_array() {
                    for query in queries {
                        let query_id = query["id"].as_str().unwrap_or("").to_string();
                        let tool = query["tool"].as_str().unwrap_or("");
                        let input: serde_json::Value =
                            serde_json::from_str(query["input"].as_str().unwrap_or("{}"))
                                .unwrap_or_default();

                        println!("[query] {} ({})", tool, query_id);

                        let result = execute_tool(tool, &input);

                        let _ = client
                            .post(format!("{}/api/v1/agents/result", cloud_url))
                            .header("Authorization", format!("Bearer {}", token))
                            .json(&serde_json::json!({
                                "query_id": query_id,
                                "result": result,
                            }))
                            .send()
                            .await;

                        println!("[done]  {}", tool);
                    }
                }
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;
    }
}

pub fn status() {
    let state = crate::config::State::load();
    if state.cloud_token().is_none() {
        println!("Not connected to cloud. Run: savants connect");
        return;
    }
    println!("Agent status: use 'savants agent start' to run the agent");
    println!("Capabilities:");
    println!("  host_health: always");
    println!("  pod_status: {}", if which("kubectl") { "yes" } else { "no (kubectl not found)" });
    println!("  pod_logs: {}", if which("kubectl") { "yes" } else { "no (kubectl not found)" });
}

fn execute_tool(tool: &str, input: &serde_json::Value) -> serde_json::Value {
    match tool {
        "host_health" => host_health(),
        "pod_status" => pod_status(input),
        "pod_logs" => pod_logs(input),
        _ => serde_json::json!({"error": format!("Unknown tool: {}", tool)}),
    }
}

fn host_health() -> serde_json::Value {
    let mut info: HashMap<String, serde_json::Value> = HashMap::new();

    // OS
    if let Ok(os) = std::fs::read_to_string("/etc/os-release") {
        for line in os.lines() {
            if let Some(name) = line.strip_prefix("PRETTY_NAME=") {
                info.insert("os".into(), name.trim_matches('"').into());
                break;
            }
        }
    }

    // Uptime
    if let Ok(uptime) = std::fs::read_to_string("/proc/uptime") {
        if let Some(secs) = uptime.split_whitespace().next().and_then(|s| s.parse::<f64>().ok()) {
            info.insert("uptime_hours".into(), ((secs / 3600.0) as u64).into());
        }
    }

    // Load
    if let Ok(load) = std::fs::read_to_string("/proc/loadavg") {
        let parts: Vec<&str> = load.split_whitespace().collect();
        if parts.len() >= 3 {
            info.insert("load_1m".into(), parts[0].parse::<f64>().unwrap_or(0.0).into());
            info.insert("load_5m".into(), parts[1].parse::<f64>().unwrap_or(0.0).into());
            info.insert("load_15m".into(), parts[2].parse::<f64>().unwrap_or(0.0).into());
        }
    }

    // CPU count
    if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
        let cores = cpuinfo.lines().filter(|l| l.starts_with("processor")).count();
        info.insert("cpus".into(), cores.into());
    }

    // Memory
    if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
        let mut total = 0u64;
        let mut avail = 0u64;
        for line in meminfo.lines() {
            if let Some(val) = line.strip_prefix("MemTotal:") {
                total = val.trim().split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
            } else if let Some(val) = line.strip_prefix("MemAvailable:") {
                avail = val.trim().split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
            }
        }
        if total > 0 {
            info.insert("memory_total_mb".into(), (total / 1024).into());
            info.insert("memory_used_mb".into(), ((total - avail) / 1024).into());
            info.insert("memory_percent".into(), (((total - avail) as f64 / total as f64) * 100.0).into());
        }
    }

    // Disk
    if let Ok(out) = std::process::Command::new("df")
        .args(["-h", "--output=target,size,used,avail,pcent", "-x", "tmpfs", "-x", "devtmpfs"])
        .output()
    {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout);
            let disks: Vec<&str> = raw.lines().skip(1).filter(|l| !l.trim().is_empty()).collect();
            info.insert("disks".into(), disks.into());
        }
    }

    // Failed systemd units
    if let Ok(out) = std::process::Command::new("systemctl")
        .args(["--failed", "--no-pager", "--plain", "--no-legend"])
        .output()
    {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout);
            let failed: Vec<String> = raw.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.split_whitespace().next().unwrap_or(l).to_string())
                .collect();
            info.insert("failed_services".into(), failed.into());
        }
    }

    serde_json::json!(info)
}

fn pod_status(input: &serde_json::Value) -> serde_json::Value {
    let mut args = vec!["get", "pods", "-o", "json"];
    let ns = input["namespace"].as_str();
    let ns_flag;
    if let Some(n) = ns {
        ns_flag = format!("-n={}", n);
        args.push(&ns_flag);
    } else {
        args.push("--all-namespaces");
    }

    let output = match std::process::Command::new("kubectl").args(&args).output() {
        Ok(o) => o,
        Err(e) => return serde_json::json!({"error": format!("kubectl failed: {}", e)}),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return serde_json::json!({"error": format!("kubectl error: {}", stderr.trim())});
    }

    let json: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => return serde_json::json!({"error": format!("parse error: {}", e)}),
    };

    let name_filter = input["name"].as_str();
    let status_filter = input["status"].as_str();

    let items = json["items"].as_array().cloned().unwrap_or_default();
    let mut pods = Vec::new();

    for item in &items {
        let meta = &item["metadata"];
        let pod_name = meta["name"].as_str().unwrap_or("?");
        let pod_ns = meta["namespace"].as_str().unwrap_or("?");
        let phase = item["status"]["phase"].as_str().unwrap_or("Unknown");

        let mut pod_status = phase.to_string();
        if let Some(containers) = item["status"]["containerStatuses"].as_array() {
            for c in containers {
                if let Some(reason) = c["state"]["waiting"]["reason"].as_str() {
                    if reason == "CrashLoopBackOff" || reason == "ErrImagePull" {
                        pod_status = reason.to_string();
                    }
                }
            }
        }

        let restarts: u64 = item["status"]["containerStatuses"]
            .as_array()
            .map(|arr| arr.iter().map(|c| c["restartCount"].as_u64().unwrap_or(0)).sum())
            .unwrap_or(0);

        if let Some(nf) = name_filter {
            if !pod_name.contains(nf) { continue; }
        }
        if let Some(sf) = status_filter {
            if !pod_status.eq_ignore_ascii_case(sf) { continue; }
        }

        pods.push(serde_json::json!({
            "namespace": pod_ns,
            "name": pod_name,
            "status": pod_status,
            "restarts": restarts,
        }));
    }

    serde_json::json!({"pods": pods, "count": pods.len()})
}

fn pod_logs(input: &serde_json::Value) -> serde_json::Value {
    let pod = match input["pod"].as_str() {
        Some(p) => p,
        None => return serde_json::json!({"error": "pod required"}),
    };
    let namespace = input["namespace"].as_str().unwrap_or("default");
    let tail = input["lines"].as_i64().unwrap_or(100);
    let min_severity = input["min_severity"].as_str().unwrap_or("WARN");

    // Find pod by substring
    let find = std::process::Command::new("kubectl")
        .args(["get", "pods", "-n", namespace, "-o", "jsonpath={.items[*].metadata.name}"])
        .output();

    let matched_pod = match find {
        Ok(out) => {
            let list = String::from_utf8_lossy(&out.stdout);
            match list.split_whitespace().find(|p| p.contains(pod)) {
                Some(p) => p.to_string(),
                None => return serde_json::json!({"error": format!("No pod matching '{}' in {}", pod, namespace)}),
            }
        }
        Err(e) => return serde_json::json!({"error": format!("kubectl failed: {}", e)}),
    };

    let output = std::process::Command::new("kubectl")
        .args(["logs", &matched_pod, "-n", namespace, "--tail", &tail.to_string()])
        .output();

    let raw = match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        Ok(out) => return serde_json::json!({"error": String::from_utf8_lossy(&out.stderr).trim().to_string()}),
        Err(e) => return serde_json::json!({"error": format!("kubectl logs failed: {}", e)}),
    };

    let min_rank: u8 = match min_severity.to_uppercase().as_str() {
        "INFO" => 0, "WARN" => 1, "ERROR" | "ERR" => 2, _ => 1,
    };

    let mut filtered = Vec::new();
    for line in raw.lines() {
        let lower = line.to_lowercase();
        let rank = if lower.contains("error") || lower.contains("fatal") || lower.contains("panic") { 2 }
            else if lower.contains("warn") { 1 }
            else { 0 };
        if rank >= min_rank {
            let label = match rank { 2 => "ERROR", 1 => "WARN", _ => "INFO" };
            filtered.push(serde_json::json!({"severity": label, "line": line}));
        }
    }

    serde_json::json!({
        "pod": matched_pod,
        "total_lines": raw.lines().count(),
        "filtered_lines": filtered.len(),
        "min_severity": min_severity,
        "logs": filtered.into_iter().take(50).collect::<Vec<_>>(),
    })
}

// ── Proactive health watch ──

struct Finding {
    key: String,       // Dedup key (e.g. "memory_high")
    severity: String,  // "warning" or "critical"
    category: String,  // "memory", "disk", "load", "service", "pod"
    title: String,
    message: String,
    metadata: serde_json::Value,
}

fn watch_health() -> Vec<Finding> {
    let mut findings = Vec::new();

    // ── Security checks ──
    findings.extend(watch_security());

    // ── K8s events ──
    findings.extend(watch_k8s_events());

    // ── Cloud providers ──
    findings.extend(watch_cloud_providers());

    // Memory
    if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
        let mut total = 0u64;
        let mut avail = 0u64;
        for line in meminfo.lines() {
            if let Some(val) = line.strip_prefix("MemTotal:") {
                total = val.trim().split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
            } else if let Some(val) = line.strip_prefix("MemAvailable:") {
                avail = val.trim().split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
            }
        }
        if total > 0 {
            let pct = (total - avail) as f64 / total as f64 * 100.0;
            if pct >= MEMORY_CRIT_PCT {
                findings.push(Finding {
                    key: "memory_critical".into(),
                    severity: "critical".into(),
                    category: "memory".into(),
                    title: format!("Memory critical: {:.0}%", pct),
                    message: format!("Memory at {:.0}% ({} / {} MB). Risk of OOM.", pct, (total - avail) / 1024, total / 1024),
                    metadata: serde_json::json!({"percent": pct, "used_mb": (total - avail) / 1024, "total_mb": total / 1024}),
                });
            } else if pct >= MEMORY_WARN_PCT {
                findings.push(Finding {
                    key: "memory_high".into(),
                    severity: "warning".into(),
                    category: "memory".into(),
                    title: format!("Memory high: {:.0}%", pct),
                    message: format!("Memory at {:.0}% ({} / {} MB).", pct, (total - avail) / 1024, total / 1024),
                    metadata: serde_json::json!({"percent": pct}),
                });
            }
        }
    }

    // Load
    if let Ok(load) = std::fs::read_to_string("/proc/loadavg") {
        let load_1m: f64 = load.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
            let cores = cpuinfo.lines().filter(|l| l.starts_with("processor")).count() as f64;
            if cores > 0.0 && load_1m > cores * LOAD_WARN_MULTIPLIER {
                findings.push(Finding {
                    key: "load_high".into(),
                    severity: "warning".into(),
                    category: "load".into(),
                    title: format!("Load high: {:.1} ({:.0} cores)", load_1m, cores),
                    message: format!("1-min load {:.1} exceeds {}x CPU count ({:.0}).", load_1m, LOAD_WARN_MULTIPLIER, cores),
                    metadata: serde_json::json!({"load_1m": load_1m, "cores": cores}),
                });
            }
        }
    }

    // Disk
    if let Ok(out) = std::process::Command::new("df")
        .args(["--output=target,pcent", "-x", "tmpfs", "-x", "devtmpfs"])
        .output()
    {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout);
            for line in raw.lines().skip(1) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let mount = parts[0];
                    let pct: u32 = parts[1].trim_end_matches('%').parse().unwrap_or(0);
                    if pct >= DISK_CRIT_PCT {
                        findings.push(Finding {
                            key: format!("disk_critical_{}", mount),
                            severity: "critical".into(),
                            category: "disk".into(),
                            title: format!("Disk critical: {} at {}%", mount, pct),
                            message: format!("Disk {} is {}% full. Risk of data loss.", mount, pct),
                            metadata: serde_json::json!({"mount": mount, "percent": pct}),
                        });
                    } else if pct >= DISK_WARN_PCT {
                        findings.push(Finding {
                            key: format!("disk_high_{}", mount),
                            severity: "warning".into(),
                            category: "disk".into(),
                            title: format!("Disk high: {} at {}%", mount, pct),
                            message: format!("Disk {} is {}% full.", mount, pct),
                            metadata: serde_json::json!({"mount": mount, "percent": pct}),
                        });
                    }
                }
            }
        }
    }

    // Failed systemd services
    if let Ok(out) = std::process::Command::new("systemctl")
        .args(["--failed", "--no-pager", "--plain", "--no-legend"])
        .output()
    {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout);
            let failed: Vec<String> = raw.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.split_whitespace().next().unwrap_or(l).to_string())
                .collect();
            for svc in &failed {
                findings.push(Finding {
                    key: format!("service_failed_{}", svc),
                    severity: "warning".into(),
                    category: "service".into(),
                    title: format!("Service failed: {}", svc),
                    message: format!("systemd unit {} has failed.", svc),
                    metadata: serde_json::json!({"service": svc}),
                });
            }
        }
    }

    // K8s: pods not running
    if which("kubectl") {
        if let Ok(out) = std::process::Command::new("kubectl")
            .args(["get", "pods", "--all-namespaces", "-o", "json"])
            .output()
        {
            if out.status.success() {
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    if let Some(items) = json["items"].as_array() {
                        for item in items {
                            let name = item["metadata"]["name"].as_str().unwrap_or("?");
                            let ns = item["metadata"]["namespace"].as_str().unwrap_or("?");
                            let phase = item["status"]["phase"].as_str().unwrap_or("Unknown");

                            // Check container statuses for CrashLoopBackOff
                            let mut bad_status = None;
                            if let Some(containers) = item["status"]["containerStatuses"].as_array() {
                                for c in containers {
                                    if let Some(reason) = c["state"]["waiting"]["reason"].as_str() {
                                        if reason == "CrashLoopBackOff" || reason == "ErrImagePull" || reason == "ImagePullBackOff" {
                                            bad_status = Some(reason.to_string());
                                        }
                                    }
                                }
                                let restarts: u64 = containers.iter()
                                    .map(|c| c["restartCount"].as_u64().unwrap_or(0))
                                    .sum();
                                if restarts > 10 {
                                    bad_status.get_or_insert_with(|| format!("{} restarts", restarts));
                                }
                            }

                            if let Some(status) = bad_status {
                                findings.push(Finding {
                                    key: format!("pod_{}_{}", ns, name),
                                    severity: "critical".into(),
                                    category: "pod".into(),
                                    title: format!("Pod {}/{}: {}", ns, name, status),
                                    message: format!("Pod {}/{} is {}. Phase: {}.", ns, name, status, phase),
                                    metadata: serde_json::json!({"pod": name, "namespace": ns, "status": status}),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    findings
}

// ── K8s events watch ──

fn watch_k8s_events() -> Vec<Finding> {
    let mut findings = Vec::new();
    if !which("kubectl") {
        return findings;
    }

    // Get recent warning/error events from the last 5 minutes
    let output = std::process::Command::new("kubectl")
        .args([
            "get", "events", "--all-namespaces",
            "--field-selector=type!=Normal",
            "-o", "json",
            "--sort-by=.lastTimestamp",
        ])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(items) = json["items"].as_array() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    for item in items.iter().rev().take(20) {
                        let reason = item["reason"].as_str().unwrap_or("?");
                        let message = item["message"].as_str().unwrap_or("");
                        let ns = item["involvedObject"]["namespace"].as_str().unwrap_or("?");
                        let obj_name = item["involvedObject"]["name"].as_str().unwrap_or("?");
                        let obj_kind = item["involvedObject"]["kind"].as_str().unwrap_or("?");
                        let count = item["count"].as_u64().unwrap_or(1);

                        // Only alert on significant events
                        let is_significant = matches!(
                            reason,
                            "FailedScheduling" | "FailedMount" | "FailedAttachVolume"
                                | "BackOff" | "Unhealthy" | "FailedCreate"
                                | "EvictionThresholdMet" | "OOMKilling"
                                | "NodeNotReady" | "NetworkNotReady"
                        );

                        if is_significant && count > 1 {
                            let severity = if matches!(reason, "OOMKilling" | "EvictionThresholdMet" | "NodeNotReady") {
                                "critical"
                            } else {
                                "warning"
                            };

                            findings.push(Finding {
                                key: format!("k8s_event_{}_{}_{}", ns, obj_name, reason),
                                severity: severity.into(),
                                category: "k8s_event".into(),
                                title: format!("{}: {}/{} ({}x)", reason, ns, obj_name, count),
                                message: format!(
                                    "{} {} {}/{}: {} (occurred {} times)",
                                    obj_kind, reason, ns, obj_name, message.chars().take(200).collect::<String>(), count
                                ),
                                metadata: serde_json::json!({
                                    "reason": reason, "namespace": ns,
                                    "object": obj_name, "kind": obj_kind, "count": count,
                                }),
                            });
                        }
                    }
                }
            }
        }
    }

    findings
}

// ── Cloud provider watch ──

fn watch_cloud_providers() -> Vec<Finding> {
    let mut findings = Vec::new();

    // AWS health check
    if which("aws") || std::env::var("AWS_ACCESS_KEY_ID").is_ok() {
        findings.extend(watch_aws());
    }

    // GCP health check
    if which("gcloud") || std::env::var("GOOGLE_APPLICATION_CREDENTIALS").is_ok() {
        findings.extend(watch_gcp());
    }

    // Cloudflare health check
    if std::env::var("CF_API_TOKEN").is_ok() || std::env::var("CLOUDFLARE_API_TOKEN").is_ok() {
        findings.extend(watch_cloudflare());
    }

    findings
}

fn watch_aws() -> Vec<Finding> {
    let mut findings = Vec::new();

    // Check for recent CloudTrail security events
    let output = std::process::Command::new("aws")
        .args([
            "cloudtrail", "lookup-events",
            "--lookup-attributes", "AttributeKey=EventName,AttributeValue=ConsoleLogin",
            "--max-items", "5",
            "--output", "json",
        ])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(events) = json["Events"].as_array() {
                    for event in events {
                        let username = event["Username"].as_str().unwrap_or("?");
                        let event_name = event["EventName"].as_str().unwrap_or("?");

                        // Check for root console login
                        if username == "root" || username == "Root" {
                            findings.push(Finding {
                                key: format!("aws_root_login"),
                                severity: "critical".into(),
                                category: "aws".into(),
                                title: "AWS root account console login".into(),
                                message: format!("Root account logged into AWS console. Use IAM users instead."),
                                metadata: serde_json::json!({"username": username, "event": event_name}),
                            });
                        }
                    }
                }
            }
        }
    }

    // Check for unencrypted S3 buckets
    let output = std::process::Command::new("aws")
        .args(["s3api", "list-buckets", "--output", "json"])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(buckets) = json["Buckets"].as_array() {
                    if buckets.len() > 0 {
                        // Just report count for now, detailed scan is expensive
                        findings.push(Finding {
                            key: "aws_buckets_scanned".into(),
                            severity: "info".into(),
                            category: "aws".into(),
                            title: format!("AWS: {} S3 buckets", buckets.len()),
                            message: format!("Monitoring {} S3 buckets.", buckets.len()),
                            metadata: serde_json::json!({"bucket_count": buckets.len()}),
                        });
                    }
                }
            }
        }
    }

    findings
}

fn watch_gcp() -> Vec<Finding> {
    let mut findings = Vec::new();

    // Check for GCP audit log anomalies
    let output = std::process::Command::new("gcloud")
        .args([
            "logging", "read",
            "severity>=WARNING AND protoPayload.@type=\"type.googleapis.com/google.cloud.audit.AuditLog\"",
            "--limit=10", "--format=json",
        ])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            if let Ok(events) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout) {
                for event in events.iter().take(5) {
                    let method = event["protoPayload"]["methodName"].as_str().unwrap_or("?");
                    let principal = event["protoPayload"]["authenticationInfo"]["principalEmail"]
                        .as_str()
                        .unwrap_or("?");
                    let severity = event["severity"].as_str().unwrap_or("WARNING");

                    // Flag IAM changes, service account key creation
                    let is_sensitive = method.contains("SetIamPolicy")
                        || method.contains("CreateServiceAccountKey")
                        || method.contains("DeleteFirewallRule");

                    if is_sensitive {
                        findings.push(Finding {
                            key: format!("gcp_audit_{}", method),
                            severity: "warning".into(),
                            category: "gcp".into(),
                            title: format!("GCP: {} by {}", method, principal),
                            message: format!("Sensitive API call: {} executed by {}.", method, principal),
                            metadata: serde_json::json!({"method": method, "principal": principal}),
                        });
                    }
                }
            }
        }
    }

    findings
}

fn watch_cloudflare() -> Vec<Finding> {
    let mut findings = Vec::new();

    let token = std::env::var("CF_API_TOKEN")
        .or_else(|_| std::env::var("CLOUDFLARE_API_TOKEN"))
        .unwrap_or_default();

    if token.is_empty() {
        return findings;
    }

    // Check Workers errors via analytics API
    // For now, just verify API connectivity
    let output = std::process::Command::new("curl")
        .args([
            "-sf", "--max-time", "5",
            "-H", &format!("Authorization: Bearer {}", token),
            "https://api.cloudflare.com/client/v4/user/tokens/verify",
        ])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if json["success"].as_bool() != Some(true) {
                    findings.push(Finding {
                        key: "cloudflare_token_invalid".into(),
                        severity: "warning".into(),
                        category: "cloudflare".into(),
                        title: "Cloudflare API token invalid".into(),
                        message: "The configured Cloudflare API token failed verification.".into(),
                        metadata: serde_json::json!({}),
                    });
                }
            }
        }
    }

    findings
}

// ── Security watch ──

fn watch_security() -> Vec<Finding> {
    let mut findings = Vec::new();
    let baseline_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("baselines");
    let _ = std::fs::create_dir_all(&baseline_dir);

    // 1. Suspicious processes (crypto miners, reverse shells)
    if let Ok(proc_dir) = std::fs::read_dir("/proc") {
        let suspicious_names = [
            "xmrig", "minerd", "cpuminer", "cgminer", "bfgminer", "ethminer",
            "kdevtmpfsi", "kinsing", "dbused", "dbus-daemon-", // common malware names
        ];
        let suspicious_cmdlines = [
            "stratum+tcp", "stratum+ssl", // mining pool protocols
            "bash -i >& /dev/tcp",        // reverse shell
            "nc -e /bin",                 // netcat shell
            "/dev/tcp/",                  // bash reverse shell
            "python -c 'import socket",  // python reverse shell
        ];

        for entry in proc_dir.flatten() {
            let pid = match entry.file_name().to_string_lossy().parse::<u32>() {
                Ok(p) => p,
                Err(_) => continue,
            };

            let cmdline_path = format!("/proc/{}/cmdline", pid);
            let comm_path = format!("/proc/{}/comm", pid);

            let comm = std::fs::read_to_string(&comm_path).unwrap_or_default();
            let comm = comm.trim();
            let cmdline = std::fs::read_to_string(&cmdline_path)
                .unwrap_or_default()
                .replace('\0', " ");

            // Check process name
            for name in &suspicious_names {
                if comm.contains(name) {
                    // Get CPU usage
                    let cpu = get_process_cpu(pid).unwrap_or(0.0);
                    findings.push(Finding {
                        key: format!("suspicious_process_{}_{}", name, pid),
                        severity: "critical".into(),
                        category: "security".into(),
                        title: format!("Suspicious process: {} (PID {})", comm, pid),
                        message: format!(
                            "Process '{}' matches known malware signature '{}'. CPU: {:.0}%. Cmdline: {}",
                            comm, name, cpu, cmdline.chars().take(200).collect::<String>()
                        ),
                        metadata: serde_json::json!({"pid": pid, "comm": comm, "cpu": cpu}),
                    });
                }
            }

            // Check cmdline patterns
            for pattern in &suspicious_cmdlines {
                if cmdline.contains(pattern) {
                    findings.push(Finding {
                        key: format!("suspicious_cmdline_{}_{}", pid, &pattern[..8.min(pattern.len())]),
                        severity: "critical".into(),
                        category: "security".into(),
                        title: format!("Suspicious command: PID {}", pid),
                        message: format!(
                            "Process {} running suspicious command matching '{}': {}",
                            comm, pattern, cmdline.chars().take(300).collect::<String>()
                        ),
                        metadata: serde_json::json!({"pid": pid, "pattern": pattern}),
                    });
                }
            }

            // High CPU unknown process (potential miner)
            if !comm.is_empty() {
                let cpu = get_process_cpu(pid).unwrap_or(0.0);
                if cpu > 90.0 {
                    let known_high_cpu = ["cc1", "gcc", "rustc", "cargo", "node", "python", "java", "go", "make", "ninja", "nix"];
                    if !known_high_cpu.iter().any(|k| comm.contains(k)) {
                        findings.push(Finding {
                            key: format!("high_cpu_process_{}", pid),
                            severity: "warning".into(),
                            category: "security".into(),
                            title: format!("High CPU process: {} ({:.0}%)", comm, cpu),
                            message: format!(
                                "Unknown process '{}' (PID {}) using {:.0}% CPU. Could be a crypto miner.",
                                comm, pid, cpu
                            ),
                            metadata: serde_json::json!({"pid": pid, "comm": comm, "cpu": cpu}),
                        });
                    }
                }
            }
        }
    }

    // 2. SSH authorized_keys changes
    let ssh_baseline = baseline_dir.join("ssh_keys.txt");
    let mut current_keys = String::new();
    for user_dir in &["/root", "/home"] {
        if let Ok(entries) = std::fs::read_dir(user_dir) {
            for entry in entries.flatten() {
                let auth_keys = entry.path().join(".ssh").join("authorized_keys");
                if auth_keys.exists() {
                    if let Ok(content) = std::fs::read_to_string(&auth_keys) {
                        current_keys.push_str(&format!("{}:\n{}\n", auth_keys.display(), content));
                    }
                }
            }
        }
        // Also check the path directly (for /root)
        let auth_keys = std::path::PathBuf::from(user_dir).join(".ssh").join("authorized_keys");
        if auth_keys.exists() {
            if let Ok(content) = std::fs::read_to_string(&auth_keys) {
                current_keys.push_str(&format!("{}:\n{}\n", auth_keys.display(), content));
            }
        }
    }

    if !current_keys.is_empty() {
        if let Ok(baseline) = std::fs::read_to_string(&ssh_baseline) {
            if baseline != current_keys {
                findings.push(Finding {
                    key: "ssh_keys_changed".into(),
                    severity: "critical".into(),
                    category: "security".into(),
                    title: "SSH authorized_keys changed".into(),
                    message: "SSH authorized keys have been modified since last baseline. Verify no unauthorized keys were added.".into(),
                    metadata: serde_json::json!({"baseline_size": baseline.len(), "current_size": current_keys.len()}),
                });
            }
        } else {
            // First run - save baseline
            let _ = std::fs::write(&ssh_baseline, &current_keys);
        }
    }

    // 3. New cron jobs
    let cron_baseline = baseline_dir.join("cron_jobs.txt");
    let mut current_crons = String::new();
    for cron_dir in &["/etc/cron.d", "/etc/cron.daily", "/etc/cron.hourly", "/var/spool/cron/crontabs"] {
        if let Ok(entries) = std::fs::read_dir(cron_dir) {
            for entry in entries.flatten() {
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    current_crons.push_str(&format!("{}:\n{}\n", entry.path().display(), content));
                }
            }
        }
    }
    // User crontabs
    if let Ok(out) = std::process::Command::new("crontab").args(["-l"]).output() {
        if out.status.success() {
            current_crons.push_str(&format!("user_crontab:\n{}\n", String::from_utf8_lossy(&out.stdout)));
        }
    }

    if !current_crons.is_empty() {
        if let Ok(baseline) = std::fs::read_to_string(&cron_baseline) {
            if baseline != current_crons {
                findings.push(Finding {
                    key: "cron_jobs_changed".into(),
                    severity: "warning".into(),
                    category: "security".into(),
                    title: "Cron jobs changed".into(),
                    message: "Scheduled tasks have been modified. Verify no unauthorized cron jobs were added.".into(),
                    metadata: serde_json::json!({}),
                });
            }
        } else {
            let _ = std::fs::write(&cron_baseline, &current_crons);
        }
    }

    // 4. New user accounts
    let passwd_baseline = baseline_dir.join("passwd.txt");
    if let Ok(current_passwd) = std::fs::read_to_string("/etc/passwd") {
        if let Ok(baseline) = std::fs::read_to_string(&passwd_baseline) {
            let baseline_users: std::collections::HashSet<&str> = baseline.lines().collect();
            let new_users: Vec<&str> = current_passwd.lines()
                .filter(|l| !baseline_users.contains(l))
                .collect();
            for user_line in &new_users {
                let username = user_line.split(':').next().unwrap_or("?");
                findings.push(Finding {
                    key: format!("new_user_{}", username),
                    severity: "critical".into(),
                    category: "security".into(),
                    title: format!("New user account: {}", username),
                    message: format!("User '{}' was added to /etc/passwd since last baseline.", username),
                    metadata: serde_json::json!({"user": username}),
                });
            }
        } else {
            let _ = std::fs::write(&passwd_baseline, &current_passwd);
        }
    }

    // 5. Unusual outbound connections
    if let Ok(out) = std::process::Command::new("ss")
        .args(["-tnp", "state", "established"])
        .output()
    {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout);
            let well_known_ports = [80, 443, 53, 22, 6443, 8443, 5432, 3306, 6379, 8080, 9090, 10250];
            for line in raw.lines().skip(1) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 5 {
                    let remote = parts[4]; // peer address
                    if let Some(port_str) = remote.rsplit(':').next() {
                        if let Ok(port) = port_str.parse::<u16>() {
                            // Flag connections to unusual high ports (potential C2)
                            if port > 10000 && !well_known_ports.contains(&port) {
                                let process = parts.get(5).unwrap_or(&"?");
                                // Skip known services
                                if !process.contains("kubelet") && !process.contains("containerd")
                                    && !process.contains("flannel") && !process.contains("coredns")
                                    && !process.contains("cloudflared")
                                {
                                    findings.push(Finding {
                                        key: format!("outbound_{}_{}", remote, port),
                                        severity: "warning".into(),
                                        category: "security".into(),
                                        title: format!("Unusual outbound connection to port {}", port),
                                        message: format!("Connection to {} from process {}. Non-standard port.", remote, process),
                                        metadata: serde_json::json!({"remote": remote, "port": port, "process": process}),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 6. K8s: privileged pods
    if which("kubectl") {
        if let Ok(out) = std::process::Command::new("kubectl")
            .args(["get", "pods", "--all-namespaces", "-o", "json"])
            .output()
        {
            if out.status.success() {
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    if let Some(items) = json["items"].as_array() {
                        for item in items {
                            let name = item["metadata"]["name"].as_str().unwrap_or("?");
                            let ns = item["metadata"]["namespace"].as_str().unwrap_or("?");

                            // Skip kube-system - those are expected to be privileged
                            if ns == "kube-system" { continue; }

                            if let Some(containers) = item["spec"]["containers"].as_array() {
                                for c in containers {
                                    let privileged = c["securityContext"]["privileged"].as_bool().unwrap_or(false);
                                    let host_network = item["spec"]["hostNetwork"].as_bool().unwrap_or(false);
                                    let host_pid = item["spec"]["hostPID"].as_bool().unwrap_or(false);

                                    if privileged {
                                        findings.push(Finding {
                                            key: format!("privileged_pod_{}_{}", ns, name),
                                            severity: "warning".into(),
                                            category: "security".into(),
                                            title: format!("Privileged pod: {}/{}", ns, name),
                                            message: format!("Pod {}/{} container {} runs as privileged. Container escape risk.", ns, name, c["name"].as_str().unwrap_or("?")),
                                            metadata: serde_json::json!({"pod": name, "namespace": ns}),
                                        });
                                    }
                                    if host_network || host_pid {
                                        findings.push(Finding {
                                            key: format!("host_access_pod_{}_{}", ns, name),
                                            severity: "warning".into(),
                                            category: "security".into(),
                                            title: format!("Host access pod: {}/{}", ns, name),
                                            message: format!("Pod {}/{} has hostNetwork={} hostPID={}.", ns, name, host_network, host_pid),
                                            metadata: serde_json::json!({"pod": name, "namespace": ns, "host_network": host_network, "host_pid": host_pid}),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    findings
}

fn get_process_cpu(pid: u32) -> Option<f64> {
    // Read /proc/[pid]/stat for CPU time
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let fields: Vec<&str> = stat.split_whitespace().collect();
    if fields.len() < 15 { return None; }
    let utime: u64 = fields[13].parse().ok()?;
    let stime: u64 = fields[14].parse().ok()?;

    // Read system uptime
    let uptime_str = std::fs::read_to_string("/proc/uptime").ok()?;
    let uptime: f64 = uptime_str.split_whitespace().next()?.parse().ok()?;

    // Process start time (field 21, in clock ticks)
    let starttime: u64 = fields[21].parse().ok()?;
    let hz = 100u64; // clock ticks per second (standard on Linux)

    let total_time = utime + stime;
    let seconds = uptime - (starttime as f64 / hz as f64);
    if seconds <= 0.0 { return None; }

    Some((total_time as f64 / hz as f64) / seconds * 100.0)
}

fn which(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

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
    let mut capabilities = vec!["host_health".to_string()];
    if which("kubectl") {
        capabilities.push("pod_status".to_string());
        capabilities.push("pod_logs".to_string());
    }
    if which("docker") {
        capabilities.push("docker_status".to_string());
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

fn which(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

//! Savants infrastructure agent.
//!
//! Runs on a server/cluster, registers with savants.cloud, polls for queries,
//! executes them locally (host health, kubectl, logs), sends results back.
//!
//! Usage: savants agent start [--name my-server]

use std::collections::HashMap;

const POLL_INTERVAL_SECS: u64 = 5;
const HEARTBEAT_INTERVAL_SECS: u64 = 60;
const WATCH_INTERVAL_SECS: u64 = 60;
const GIT_WATCH_INTERVAL_SECS: u64 = 10; // Check for pushes every 10s

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

    // Save token to OSS state file so MCP binary auto-detects cloud mode
    let oss_state_path = dirs::home_dir().unwrap_or_default().join(".savants").join("state.json");
    if let Ok(raw) = std::fs::read_to_string(&oss_state_path) {
        if let Ok(mut oss_state) = serde_json::from_str::<serde_json::Value>(&raw) {
            oss_state["cloud_token"] = serde_json::json!(token);
            let _ = std::fs::write(&oss_state_path, serde_json::to_string_pretty(&oss_state).unwrap_or_default());
        }
    }

    let agent_name = name.unwrap_or_else(|| {
        hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    });

    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let version = env!("CARGO_PKG_VERSION").to_string();

    // Detect capabilities
    let mut capabilities = vec!["host_health".to_string(), "host_story".to_string(), "security_scan".to_string()];
    if which("kubectl") {
        capabilities.push("pod_status".to_string());
        capabilities.push("pod_logs".to_string());
        capabilities.push("k8s_events".to_string());
    }
    if which("docker") {
        capabilities.push("docker_status".to_string());
    }
    if which("execsnoop") || which("tcpretrans") || which("bpftrace") {
        capabilities.push("ebpf_snapshot".to_string());
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
            "machine_id": get_machine_id(),
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

    // Fetch cloud project list - only index repos that have a matching cloud project
    let cloud_projects: Vec<String> = match client
        .get(format!("{}/api/v1/projects", cloud_url))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            body["projects"].as_array().unwrap_or(&vec![])
                .iter()
                .filter_map(|p| p["slug"].as_str().or(p["name"].as_str()).map(|s| s.to_lowercase()))
                .collect()
        }
        _ => vec![],
    };

    // Discover git repos and filter to only cloud-registered projects
    let all_repos = discover_git_repos();
    let repos: Vec<std::path::PathBuf> = all_repos.into_iter().filter(|r| {
        let name = r.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
        cloud_projects.contains(&name)
    }).collect();

    if !repos.is_empty() {
        println!("  Indexing {} repos (matched to cloud projects):", repos.len());
        for r in &repos {
            println!("    {}", r.display());
        }
    }
    if !cloud_projects.is_empty() && repos.is_empty() {
        println!("  Cloud projects: {:?} (no matching local repos found)", cloud_projects);
    }

    // Load last-known heads from cache, or empty (triggers initial full upload)
    let heads_cache = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("agent_heads.json");
    let mut repo_heads: HashMap<String, String> = std::fs::read_to_string(&heads_cache)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let initial_upload = repo_heads.is_empty();
    if initial_upload && !repos.is_empty() {
        println!("  First run: will upload {} repos to cloud graph", repos.len());
    }

    // ── eBPF: try to load probes (auto-fallback if unavailable) ──
    // Must store the handle to keep the BPF program alive and read maps.
    #[cfg(feature = "ebpf")]
    let mut ebpf_handle: Option<crate::ebpf_loader::BpfHandle> = None;
    #[cfg(feature = "ebpf")]
    {
        if crate::ebpf_loader::can_load() {
            match crate::ebpf_loader::load_and_attach() {
                Ok(handle) => {
                    println!("  eBPF: {} probes loaded: {}", handle.probes.len(), handle.probes.join(", "));
                    ebpf_handle = Some(handle);
                }
                Err(e) => {
                    println!("  eBPF: load failed ({}) - /proc fallback active", e);
                }
            }
        } else {
            println!("  eBPF: unavailable (no CAP_BPF) - /proc fallback active");
        }
    }
    #[cfg(not(feature = "ebpf"))]
    {
        if crate::ebpf_loader::can_load() {
            match crate::ebpf_loader::load_and_attach() {
                Ok(_) => {}
                Err(e) => println!("  eBPF: {} - /proc fallback active", e),
            }
        } else {
            println!("  eBPF: unavailable (no CAP_BPF) - /proc fallback active");
        }
    }

    println!("  Polling for queries...\n");

    let mut last_heartbeat = std::time::Instant::now();
    let mut last_watch = std::time::Instant::now() - std::time::Duration::from_secs(WATCH_INTERVAL_SECS);
    let mut last_git_watch = std::time::Instant::now() - std::time::Duration::from_secs(GIT_WATCH_INTERVAL_SECS); // trigger immediately
    let mut known_issues: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Main loop: poll for queries + proactive health watch
    loop {
        // Proactive health watch
        if last_watch.elapsed().as_secs() >= WATCH_INTERVAL_SECS {
            let mut findings = watch_health();

            // Read all eBPF probe maps
            #[cfg(feature = "ebpf")]
            if let Some(ref handle) = ebpf_handle {
                let snap = handle.read_snapshot();

                // TCP retransmits
                if let Some(ref rt) = snap.retransmit {
                    if rt.total > 0 {
                        let sev = if rt.is_link_issue { "critical" } else if rt.total > 100 { "warning" } else { "info" };
                        findings.push(Finding {
                            key: "ebpf_tcp_retransmit".into(),
                            severity: sev.into(),
                            category: "network".into(),
                            title: format!("eBPF: {} retransmits to {} destinations", rt.total, rt.dest_count),
                            message: format!("{}. Top: {}", rt.diagnosis,
                                rt.by_dest.iter().take(5).map(|(ip, c)| format!("{}({})", ip, c)).collect::<Vec<_>>().join(", ")),
                            metadata: serde_json::json!({"probe": "tcp_retransmit", "data": rt}),
                        });
                    }
                }

                // Block I/O latency
                if let Some(ref bio) = snap.biolatency {
                    if bio.p99_us > 50_000 { // p99 > 50ms = worth reporting
                        findings.push(Finding {
                            key: "ebpf_biolatency".into(),
                            severity: if bio.p99_us > 500_000 { "critical" } else { "warning" }.into(),
                            category: "disk".into(),
                            title: format!("eBPF: disk I/O p99={:.1}ms ({} ops)", bio.p99_us as f64 / 1000.0, bio.total),
                            message: format!("Block I/O latency: p50={:.1}ms p99={:.1}ms max={:.1}ms over {} I/O ops",
                                bio.p50_us as f64 / 1000.0, bio.p99_us as f64 / 1000.0, bio.max_us as f64 / 1000.0, bio.total),
                            metadata: serde_json::json!({"probe": "biolatency", "data": bio}),
                        });
                    }
                }

                // CPU run queue latency
                if let Some(ref rq) = snap.runqlat {
                    if rq.p99_us > 10_000 { // p99 > 10ms = CPU saturation
                        findings.push(Finding {
                            key: "ebpf_runqlat".into(),
                            severity: if rq.p99_us > 100_000 { "critical" } else { "warning" }.into(),
                            category: "cpu".into(),
                            title: format!("eBPF: CPU queue p99={:.1}ms ({} wakeups)", rq.p99_us as f64 / 1000.0, rq.total),
                            message: format!("Run queue latency: p50={:.1}ms p99={:.1}ms. Tasks waiting for CPU.",
                                rq.p50_us as f64 / 1000.0, rq.p99_us as f64 / 1000.0),
                            metadata: serde_json::json!({"probe": "runqlat", "data": rq}),
                        });
                    }
                }

                // Packet drops
                if let Some(ref drops) = snap.tcpdrop {
                    if drops.total > 100 {
                        findings.push(Finding {
                            key: "ebpf_tcpdrop".into(),
                            severity: if drops.total > 1000 { "critical" } else { "warning" }.into(),
                            category: "network".into(),
                            title: format!("eBPF: {} kernel packet drops (top: {})", drops.total, drops.top_reason),
                            message: format!("Kernel dropped {} packets. By reason: {}",
                                drops.total,
                                drops.by_reason.iter().take(5).map(|(r, c)| format!("{}({})", r, c)).collect::<Vec<_>>().join(", ")),
                            metadata: serde_json::json!({"probe": "tcpdrop", "data": drops}),
                        });
                    }
                }

                // OOM kills
                if let Some(ref oom) = snap.oomkill {
                    if oom.total > 0 {
                        findings.push(Finding {
                            key: "ebpf_oomkill".into(),
                            severity: "critical".into(),
                            category: "memory".into(),
                            title: format!("eBPF: {} OOM kills detected", oom.total),
                            message: format!("{} processes killed by OOM killer", oom.total),
                            metadata: serde_json::json!({"probe": "oomkill", "data": oom}),
                        });
                    }
                }

                // TCP connection lifecycle
                if let Some(ref life) = snap.tcplife {
                    if life.short_lived > 50 {
                        let top_ports: String = life.by_port.iter().take(5)
                            .map(|(p, c)| format!(":{} ({})", p, c)).collect::<Vec<_>>().join(", ");
                        findings.push(Finding {
                            key: "ebpf_tcplife".into(),
                            severity: if life.short_lived > 200 { "warning" } else { "info" }.into(),
                            category: "network".into(),
                            title: format!("eBPF: {} connections ({} short-lived)", life.total_conns, life.short_lived),
                            message: format!("{} TCP connections destroyed, {} lasted <1s. Top ports: {}",
                                life.total_conns, life.short_lived, top_ports),
                            metadata: serde_json::json!({"probe": "tcplife", "data": life}),
                        });
                    }
                }

                // TCP connection resets
                if let Some(ref resets) = snap.tcpconnlat {
                    if resets.total > 10 {
                        findings.push(Finding {
                            key: "ebpf_tcp_resets".into(),
                            severity: if resets.is_link_issue { "critical" } else { "warning" }.into(),
                            category: "network".into(),
                            title: format!("eBPF: {} TCP resets to {} destinations", resets.total, resets.dest_count),
                            message: format!("{}. Top: {}", resets.diagnosis,
                                resets.by_dest.iter().take(5).map(|(ip, c)| format!("{}({})", ip, c)).collect::<Vec<_>>().join(", ")),
                            metadata: serde_json::json!({"probe": "tcpconnlat", "data": resets}),
                        });
                    }
                }
            }
            for finding in &findings {
                // Dedup logic:
                // - Info: send once, suppress until cleared
                // - Warning: send once, resend every 10 minutes if still active
                // - Critical: send every cycle until cleared (never suppress)
                let is_new = !known_issues.contains(&finding.key);
                let is_critical = finding.severity == "critical";
                let is_warning = finding.severity == "warning";

                let should_send = is_new || is_critical;

                if is_new {
                    println!("[watch] {} - {}", finding.severity, finding.message);
                }

                if should_send {
                    known_issues.insert(finding.key.clone());

                    // Direct notification for warning+ (agent is inside the cluster)
                    if is_critical || is_warning {
                        if let Ok(gotify_url) = std::env::var("SAVANTS_GOTIFY_URL") {
                            if let Ok(gotify_token) = std::env::var("SAVANTS_GOTIFY_TOKEN") {
                                let priority = if is_critical { 8 } else { 5 };
                                let _ = client
                                    .post(format!("{}/message", gotify_url))
                                    .header("X-Gotify-Key", &gotify_token)
                                    .json(&serde_json::json!({
                                        "title": format!("[{}] {}", finding.severity.to_uppercase(), finding.title),
                                        "message": format!("{}\n\nAgent: {}\nCategory: {}", finding.message, agent_name, finding.category),
                                        "priority": priority,
                                    }))
                                    .send()
                                    .await
                                    .ok();
                            }
                        }
                    }

                    // Send to cloud for audit log + additional routing
                    match client
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
                        .await
                    {
                        Ok(resp) if !resp.status().is_success() => {
                            eprintln!("[notify] {} failed: HTTP {}", finding.key, resp.status());
                            // Remove from known_issues so it retries next cycle
                            known_issues.remove(&finding.key);
                        }
                        Err(e) => {
                            eprintln!("[notify] {} error: {}", finding.key, e);
                            known_issues.remove(&finding.key);
                        }
                        _ => {} // success
                    }
                }
            }

            // Clear resolved issues
            let active_keys: std::collections::HashSet<String> =
                findings.iter().map(|f| f.key.clone()).collect();
            known_issues.retain(|k| active_keys.contains(k));

            last_watch = std::time::Instant::now();
        }

        // Poll for queries FIRST (must be responsive)
        let poll = client
            .get(format!(
                "{}/api/v1/agents/poll?agent_id={}",
                cloud_url, agent_id
            ))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await;

        if let Ok(resp) = poll {
            let status_code = resp.status();
            if status_code.is_success() {
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                let queries_arr = body["queries"].as_array();
                if let Some(queries) = queries_arr {
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
            } else {
                eprintln!("[poll] HTTP {}", status_code);
            }
        } else if let Err(e) = poll {
            eprintln!("[poll] error: {}", e);
        }

        // Git watch: detect pushes, reindex, upload to D1 (runs AFTER poll)
        if last_git_watch.elapsed().as_secs() >= GIT_WATCH_INTERVAL_SECS {
            if repos.is_empty() {
                eprintln!("[git] no repos to watch");
            }
            for repo in &repos {
                let repo_str = repo.to_string_lossy().to_string();
                let current_head = git_head(repo).unwrap_or_default();
                let previous_head = repo_heads.get(&repo_str).cloned().unwrap_or_default();

                if current_head.is_empty() {
                    eprintln!("[git] {} HEAD is empty (git_head failed)", repo.display());
                }

                if !current_head.is_empty() && current_head != previous_head {
                    let repo_name = repo.file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| "unknown".to_string());

                    println!("[git] {} changed: {} -> {}", repo_name, &previous_head[..7.min(previous_head.len())], &current_head[..7.min(current_head.len())]);

                    // Parse the repo
                    let mut parser = crate::code_parser::CodeParser::new(&repo_name);
                    let result = parser.parse_repo(&repo_str);
                    println!("[git] {} parsed: {} files, {} entities, {} calls", repo_name, result.files, result.entities.len(), result.call_sites.len());

                    // Upload to cloud D1
                    let upload = client
                        .post(format!("{}/api/v1/ingest/parse-result", cloud_url))
                        .header("Authorization", format!("Bearer {}", token))
                        .json(&serde_json::json!({
                            "repo": repo_name,
                            "files": result.files,
                            "entities": result.entities,
                            "call_sites": result.call_sites,
                        }))
                        .send()
                        .await;

                    match upload {
                        Ok(resp) if resp.status().is_success() => {
                            let body: serde_json::Value = resp.json().await.unwrap_or_default();
                            println!("[git] {} uploaded: {} nodes, {} edges",
                                repo_name,
                                body["nodes"].as_u64().unwrap_or(0),
                                body["edges"].as_u64().unwrap_or(0),
                            );
                        }
                        Ok(resp) => {
                            let text = resp.text().await.unwrap_or_default();
                            eprintln!("[git] {} upload failed: {}", repo_name, text);
                        }
                        Err(e) => {
                            eprintln!("[git] {} upload error: {}", repo_name, e);
                        }
                    }

                    // Track deploy: check if this commit is being deployed to k8s
                    if which("kubectl") {
                        if let Some(deploy_info) = detect_deploy(&repo_name, &current_head) {
                            println!("[deploy] {} -> {}", repo_name, deploy_info);
                            let _ = client
                                .post(format!("{}/api/v1/agents/notify", cloud_url))
                                .header("Authorization", format!("Bearer {}", token))
                                .json(&serde_json::json!({
                                    "agent_id": agent_id,
                                    "agent_name": agent_name,
                                    "severity": "info",
                                    "category": "deploy",
                                    "title": format!("Deploy: {} @ {}", repo_name, &current_head[..7]),
                                    "message": deploy_info,
                                    "key": format!("deploy_{}_{}", repo_name, &current_head[..7]),
                                    "metadata": serde_json::json!({"repo": repo_name, "commit": current_head}),
                                }))
                                .send()
                                .await;
                        }
                    }

                    repo_heads.insert(repo_str, current_head);
                }
            }
            // Persist heads cache so restarts don't re-upload everything
            if let Ok(json) = serde_json::to_string(&repo_heads) {
                let _ = std::fs::write(&heads_cache, json);
            }
            last_git_watch = std::time::Instant::now();
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
        "host_story" => host_story(input),
        "pod_status" => pod_status(input),
        "pod_logs" => pod_logs(input),
        "ebpf_snapshot" => ebpf_snapshot(input),
        _ => serde_json::json!({"error": format!("Unknown tool: {}", tool)}),
    }
}

/// Post-mortem analysis: scan journald for a time window and return
/// a structured timeline of events grouped by service and severity.
/// Builds the event story from historical data for debugging past incidents.
///
/// Note: eBPF probes (runqlat, tcpretrans, biolatency) only capture real-time
/// data - they are NOT available for past incidents. For post-mortem analysis,
/// this tool uses journald logs which retain days/weeks of history depending
/// on disk space and journal configuration.
fn host_story(input: &serde_json::Value) -> serde_json::Value {
    let since = input["since"].as_str().unwrap_or("30 minutes ago");
    let until = input["until"].as_str().unwrap_or("now");
    let service = input["service"].as_str(); // optional: filter to one service

    let mut args = vec![
        "--since", since, "--until", until,
        "--no-pager", "-o", "short", "--quiet", "--no-hostname",
    ];
    if let Some(svc) = service {
        args.push("-u");
        args.push(svc);
    }

    let output = match std::process::Command::new("journalctl").args(&args).output() {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        Ok(out) => return serde_json::json!({"error": format!("journalctl failed: {}", String::from_utf8_lossy(&out.stderr))}),
        Err(e) => return serde_json::json!({"error": format!("Cannot run journalctl: {}", e)}),
    };

    let failure_patterns = [
        "deadline exceeded", "timed out", "i/o timeout", "connection reset",
        "connection refused", "unreachable", "failed", "error", "panic",
        "oom", "killed", "segfault", "denied", "refused", "503", "502",
        "no route", "network is unreachable", "broken pipe",
    ];

    // Group events by service and minute
    let mut by_service: HashMap<String, Vec<String>> = HashMap::new();
    let mut timeline: Vec<serde_json::Value> = Vec::new();
    let mut total_lines = 0;
    let mut error_lines = 0;

    for line in output.lines() {
        total_lines += 1;
        let lower = line.to_lowercase();
        let is_error = failure_patterns.iter().any(|p| lower.contains(p));

        if is_error {
            error_lines += 1;
            // Format: "May 05 17:19:07 unit[pid]: message" - unit is at index 3
            let unit = line.split_whitespace().nth(3)
                .unwrap_or("unknown")
                .split('[').next()
                .unwrap_or("unknown")
                .trim_end_matches(':')
                .to_string();
            by_service.entry(unit).or_default().push(line.to_string());
        }
    }

    // Build per-service summaries
    let mut services: Vec<serde_json::Value> = Vec::new();
    let mut service_list: Vec<(&String, &Vec<String>)> = by_service.iter().collect();
    service_list.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    for (svc, lines) in service_list.iter().take(20) {
        // Extract time of first and last error
        let first_time = lines.first().and_then(|l| l.get(..15)).unwrap_or("?");
        let last_time = lines.last().and_then(|l| l.get(..15)).unwrap_or("?");

        services.push(serde_json::json!({
            "service": svc,
            "error_count": lines.len(),
            "first_error": first_time,
            "last_error": last_time,
            "sample_lines": lines.iter().take(5).cloned().collect::<Vec<String>>(),
        }));
    }

    // Build minute-by-minute timeline
    let mut by_minute: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for line in output.lines() {
        let lower = line.to_lowercase();
        if failure_patterns.iter().any(|p| lower.contains(p)) {
            let minute = line.get(..12).unwrap_or("?").to_string();
            *by_minute.entry(minute).or_default() += 1;
        }
    }

    for (minute, count) in &by_minute {
        timeline.push(serde_json::json!({"time": minute, "errors": count}));
    }

    // Determine probable root cause: service with the EARLIEST first error
    // The first thing to break is usually the trigger for everything else.
    let root_cause = if !service_list.is_empty() {
        let mut earliest_svc = &service_list[0];
        let mut earliest_time = service_list[0].1.first().and_then(|l| l.get(..15)).unwrap_or("z");
        for item in &service_list {
            if let Some(first_line) = item.1.first() {
                let time = first_line.get(..15).unwrap_or("z");
                if time < earliest_time {
                    earliest_time = time;
                    earliest_svc = item;
                }
            }
        }
        Some(serde_json::json!({
            "service": earliest_svc.0,
            "first_error_time": earliest_time,
            "total_errors": earliest_svc.1.len(),
            "first_error_line": earliest_svc.1.first().map(|l| &l[..l.len().min(120)]),
            "reasoning": "First service to report errors - likely the trigger for cascading failures",
        }))
    } else { None };

    serde_json::json!({
        "window": {"since": since, "until": until},
        "data_source": "journald (historical logs, not real-time eBPF)",
        "note": "eBPF probes only capture real-time data. For post-mortem analysis, journald provides the historical record.",
        "summary": {
            "total_log_lines": total_lines,
            "error_lines": error_lines,
            "services_affected": by_service.len(),
            "error_rate_pct": if total_lines > 0 { error_lines as f64 / total_lines as f64 * 100.0 } else { 0.0 },
        },
        "services": services,
        "timeline": timeline,
        "probable_root_cause": root_cause,
    })
}

/// Run eBPF tools briefly and return a snapshot of kernel-level activity.
/// Uses bcc-tools (execsnoop, tcpretrans, cachestat, opensnoop).
fn ebpf_snapshot(input: &serde_json::Value) -> serde_json::Value {
    let duration = input["seconds"].as_u64().unwrap_or(5);
    let max_secs = std::cmp::min(duration, 30); // cap at 30s
    let mut results: HashMap<String, serde_json::Value> = HashMap::new();

    // execsnoop: new processes (5s capture)
    if which("execsnoop") {
        if let Ok(out) = std::process::Command::new("sudo")
            .args(["timeout", &format!("{}s", max_secs), "execsnoop"])
            .output()
        {
            let raw = String::from_utf8_lossy(&out.stdout);
            let procs: Vec<serde_json::Value> = raw.lines().skip(1).take(50).map(|l| {
                let parts: Vec<&str> = l.split_whitespace().collect();
                serde_json::json!({
                    "comm": parts.first().unwrap_or(&"?"),
                    "pid": parts.get(1).unwrap_or(&"?"),
                    "ppid": parts.get(2).unwrap_or(&"?"),
                    "ret": parts.get(3).unwrap_or(&"?"),
                    "args": parts.get(4..).map(|a| a.join(" ")).unwrap_or_default(),
                })
            }).collect();
            results.insert("new_processes".into(), serde_json::json!({
                "count": procs.len(),
                "duration_sec": max_secs,
                "processes": procs,
            }));
        }
    }

    // tcpretrans: TCP retransmissions (5s capture)
    if which("tcpretrans") {
        if let Ok(out) = std::process::Command::new("sudo")
            .args(["timeout", &format!("{}s", max_secs), "tcpretrans"])
            .output()
        {
            let raw = String::from_utf8_lossy(&out.stdout);
            let retrans: Vec<serde_json::Value> = raw.lines().skip(1).take(50).map(|l| {
                let parts: Vec<&str> = l.split_whitespace().collect();
                serde_json::json!({
                    "time": parts.first().unwrap_or(&"?"),
                    "pid": parts.get(1).unwrap_or(&"?"),
                    "ip": parts.get(2).unwrap_or(&"?"),
                    "src": parts.get(3).unwrap_or(&"?"),
                    "dst": parts.get(4).unwrap_or(&"?"),
                    "state": parts.get(5).unwrap_or(&"?"),
                })
            }).collect();
            results.insert("tcp_retransmits".into(), serde_json::json!({
                "count": retrans.len(),
                "duration_sec": max_secs,
                "retransmits": retrans,
            }));
        }
    }

    // cachestat: filesystem cache stats (5s capture)
    if which("cachestat") {
        if let Ok(out) = std::process::Command::new("sudo")
            .args(["timeout", &format!("{}s", max_secs), "cachestat", "1"])
            .output()
        {
            let raw = String::from_utf8_lossy(&out.stdout);
            let samples: Vec<serde_json::Value> = raw.lines().skip(1).take(10).filter_map(|l| {
                let parts: Vec<&str> = l.split_whitespace().collect();
                if parts.len() >= 5 {
                    Some(serde_json::json!({
                        "hits": parts.get(0).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0),
                        "misses": parts.get(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0),
                        "dirties": parts.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0),
                        "read_hit_pct": parts.get(3).unwrap_or(&"?"),
                        "write_hit_pct": parts.get(4).unwrap_or(&"?"),
                    }))
                } else { None }
            }).collect();
            results.insert("cache_stats".into(), serde_json::json!({
                "samples": samples,
                "duration_sec": max_secs,
            }));
        }
    }

    // opensnoop: file opens (5s capture)
    if which("opensnoop") {
        if let Ok(out) = std::process::Command::new("sudo")
            .args(["timeout", &format!("{}s", max_secs), "opensnoop"])
            .output()
        {
            let raw = String::from_utf8_lossy(&out.stdout);
            let opens: Vec<serde_json::Value> = raw.lines().skip(1).take(50).map(|l| {
                let parts: Vec<&str> = l.split_whitespace().collect();
                serde_json::json!({
                    "pid": parts.first().unwrap_or(&"?"),
                    "comm": parts.get(1).unwrap_or(&"?"),
                    "fd": parts.get(2).unwrap_or(&"?"),
                    "err": parts.get(3).unwrap_or(&"?"),
                    "path": parts.get(4..).map(|a| a.join(" ")).unwrap_or_default(),
                })
            }).collect();
            results.insert("file_opens".into(), serde_json::json!({
                "count": opens.len(),
                "duration_sec": max_secs,
                "opens": opens,
            }));
        }
    }

    let tools_available: Vec<&str> = ["execsnoop", "tcpretrans", "cachestat", "opensnoop"]
        .iter().filter(|t| which(t)).copied().collect();
    let tools_missing: Vec<&str> = ["execsnoop", "tcpretrans", "cachestat", "opensnoop", "biolatency", "biosnoop", "ext4slower", "tcplife"]
        .iter().filter(|t| !which(t)).copied().collect();

    serde_json::json!({
        "tools_available": tools_available,
        "tools_missing": tools_missing,
        "duration_sec": max_secs,
        "data": results,
    })
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

    // ── Network interfaces ──
    let mut interfaces: Vec<serde_json::Value> = Vec::new();
    if let Ok(proc_net) = std::fs::read_to_string("/proc/net/dev") {
        for line in proc_net.lines().skip(2) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 12 { continue; }
            let iface = parts[0].trim_end_matches(':');
            if iface == "lo" || iface.starts_with("veth") || iface.starts_with("br-") || iface.starts_with("docker") || iface.starts_with("flannel") || iface.starts_with("cni") { continue; }
            let rx_bytes: u64 = parts[1].parse().unwrap_or(0);
            let tx_bytes: u64 = parts[9].parse().unwrap_or(0);
            let rx_drops: u64 = parts[4].parse().unwrap_or(0);
            let tx_drops: u64 = parts[12].parse().unwrap_or(0);
            // sysfs: operstate and power management (no external tools needed)
            let operstate = std::fs::read_to_string(format!("/sys/class/net/{}/operstate", iface))
                .unwrap_or_default().trim().to_string();
            let dev_power = std::fs::read_to_string(format!("/sys/class/net/{}/device/power/control", iface))
                .unwrap_or_default().trim().to_string();
            let carrier = std::fs::read_to_string(format!("/sys/class/net/{}/carrier", iface))
                .unwrap_or_default().trim().to_string();
            let speed = std::fs::read_to_string(format!("/sys/class/net/{}/speed", iface))
                .unwrap_or_default().trim().to_string();
            let iface_type = if iface.starts_with("wl") { "wifi" } else if iface.starts_with("en") || iface.starts_with("eth") { "ethernet" } else if iface.starts_with("tailscale") { "vpn" } else { "other" };
            let mut iface_json = serde_json::json!({
                "name": iface,
                "rx_gb": rx_bytes / 1_073_741_824,
                "tx_gb": tx_bytes / 1_073_741_824,
                "rx_drops": rx_drops,
                "tx_drops": tx_drops,
                "type": iface_type,
                "operstate": operstate,
                "carrier": carrier,
            });
            if !dev_power.is_empty() {
                iface_json["device_power_control"] = dev_power.clone().into();
            }
            if !speed.is_empty() && speed != "-1" {
                iface_json["speed_mbps"] = speed.into();
            }
            // For WiFi: check power_save via sysfs (auto = power-save likely on)
            if iface_type == "wifi" {
                let ps = if let Ok(output) = std::process::Command::new("iw")
                    .args(["dev", iface, "get", "power_save"]).output() {
                    let raw = String::from_utf8_lossy(&output.stdout);
                    if raw.contains("on") { "on" } else { "off" }.to_string()
                } else {
                    if dev_power == "auto" { "on (sysfs)" } else { "off (sysfs)" }.to_string()
                };
                iface_json["power_save"] = ps.into();
            }
            interfaces.push(iface_json);
        }
    }
    if !interfaces.is_empty() {
        info.insert("network_interfaces".into(), interfaces.into());
    }

    // ── DNS ──
    if let Ok(resolv) = std::fs::read_to_string("/etc/resolv.conf") {
        let nameservers: Vec<&str> = resolv.lines()
            .filter(|l| l.starts_with("nameserver"))
            .filter_map(|l| l.split_whitespace().nth(1))
            .collect();
        info.insert("dns_nameservers".into(), serde_json::json!(nameservers));

        // Test actual resolution
        let mut dns_ok = true;
        if let Ok(out) = std::process::Command::new("getent").args(["hosts", "google.com"]).output() {
            if !out.status.success() { dns_ok = false; }
        }
        info.insert("dns_resolving".into(), dns_ok.into());
    }

    // ── Default route ──
    if let Ok(out) = std::process::Command::new("ip").args(["route", "show", "default"]).output() {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout);
            let routes: Vec<&str> = raw.lines().collect();
            info.insert("default_routes".into(), serde_json::json!(routes));
        }
    }

    // ── Tailscale ──
    if which("tailscale") {
        if let Ok(out) = std::process::Command::new("tailscale").args(["status", "--json"]).output() {
            if out.status.success() {
                if let Ok(status) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    let self_online = status["Self"]["Online"].as_bool().unwrap_or(false);
                    let health = status["Health"].as_array()
                        .map(|h| h.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>())
                        .unwrap_or_default();
                    info.insert("tailscale".into(), serde_json::json!({
                        "online": self_online,
                        "ip": status["Self"]["TailscaleIPs"][0].as_str().unwrap_or("?"),
                        "health_warnings": health,
                    }));
                }
            }
        }
    }

    // ── USE Method: Brendan Gregg's system performance checklist ──

    // CPU per-core utilization from /proc/stat
    if let Ok(stat) = std::fs::read_to_string("/proc/stat") {
        let mut cpu_lines: Vec<serde_json::Value> = Vec::new();
        for line in stat.lines() {
            if line.starts_with("cpu") && !line.starts_with("cpu ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 8 {
                    let user: u64 = parts[1].parse().unwrap_or(0);
                    let system: u64 = parts[3].parse().unwrap_or(0);
                    let idle: u64 = parts[4].parse().unwrap_or(0);
                    let iowait: u64 = parts[5].parse().unwrap_or(0);
                    let steal: u64 = parts[8].parse().unwrap_or(0);
                    let total = user + system + idle + iowait + steal;
                    cpu_lines.push(serde_json::json!({
                        "core": parts[0],
                        "user_pct": if total > 0 { (user as f64 / total as f64 * 100.0) as u32 } else { 0 },
                        "system_pct": if total > 0 { (system as f64 / total as f64 * 100.0) as u32 } else { 0 },
                        "iowait_pct": if total > 0 { (iowait as f64 / total as f64 * 100.0) as u32 } else { 0 },
                        "steal_pct": if total > 0 { (steal as f64 / total as f64 * 100.0) as u32 } else { 0 },
                        "idle_pct": if total > 0 { (idle as f64 / total as f64 * 100.0) as u32 } else { 0 },
                    }));
                }
            }
            // CPU saturation: run queue
            if line.starts_with("procs_running") {
                if let Some(r) = line.split_whitespace().nth(1).and_then(|s| s.parse::<u64>().ok()) {
                    info.insert("cpu_run_queue".into(), r.into());
                }
            }
            // Context switches
            if line.starts_with("ctxt") {
                if let Some(c) = line.split_whitespace().nth(1).and_then(|s| s.parse::<u64>().ok()) {
                    info.insert("context_switches".into(), c.into());
                }
            }
        }
        if !cpu_lines.is_empty() {
            info.insert("cpu_per_core".into(), cpu_lines.into());
        }
    }

    // Memory saturation: swap activity from /proc/vmstat
    if let Ok(vmstat) = std::fs::read_to_string("/proc/vmstat") {
        let mut swap = serde_json::Map::new();
        let mut oom = 0u64;
        for line in vmstat.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 { continue; }
            match parts[0] {
                "pswpin" => { swap.insert("swap_in_pages".into(), parts[1].parse::<u64>().unwrap_or(0).into()); }
                "pswpout" => { swap.insert("swap_out_pages".into(), parts[1].parse::<u64>().unwrap_or(0).into()); }
                "pgpgin" => { swap.insert("page_in_kb".into(), parts[1].parse::<u64>().unwrap_or(0).into()); }
                "pgpgout" => { swap.insert("page_out_kb".into(), parts[1].parse::<u64>().unwrap_or(0).into()); }
                "oom_kill" => { oom = parts[1].parse().unwrap_or(0); }
                _ => {}
            }
        }
        if !swap.is_empty() {
            swap.insert("oom_kills".into(), oom.into());
            info.insert("memory_saturation".into(), serde_json::Value::Object(swap));
        }
    }

    // Disk I/O from /proc/diskstats
    if let Ok(diskstats) = std::fs::read_to_string("/proc/diskstats") {
        let mut disks_io: Vec<serde_json::Value> = Vec::new();
        for line in diskstats.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 14 { continue; }
            let dev = parts[2];
            // Only real disks (sda, nvme0n1, vda) not partitions
            if dev.starts_with("loop") || dev.starts_with("dm-") || dev.contains('p') && dev.len() > 4 { continue; }
            if dev.ends_with(|c: char| c.is_ascii_digit()) && (dev.starts_with("sd") || dev.starts_with("vd")) && dev.len() > 3 { continue; }

            let reads: u64 = parts[3].parse().unwrap_or(0);
            let read_ms: u64 = parts[6].parse().unwrap_or(0);
            let writes: u64 = parts[7].parse().unwrap_or(0);
            let write_ms: u64 = parts[10].parse().unwrap_or(0);
            let io_in_progress: u64 = parts[11].parse().unwrap_or(0);
            let io_ms: u64 = parts[12].parse().unwrap_or(0);
            let weighted_ms: u64 = parts[13].parse().unwrap_or(0);

            if reads + writes > 0 {
                disks_io.push(serde_json::json!({
                    "device": dev,
                    "reads": reads,
                    "writes": writes,
                    "read_ms": read_ms,
                    "write_ms": write_ms,
                    "io_in_progress": io_in_progress,
                    "io_ms_total": io_ms,
                    "weighted_io_ms": weighted_ms,
                    "avg_read_ms": if reads > 0 { read_ms as f64 / reads as f64 } else { 0.0 },
                    "avg_write_ms": if writes > 0 { write_ms as f64 / writes as f64 } else { 0.0 },
                }));
            }
        }
        if !disks_io.is_empty() {
            info.insert("disk_io".into(), disks_io.into());
        }
    }

    // TCP stats from /proc/net/snmp
    if let Ok(snmp) = std::fs::read_to_string("/proc/net/snmp") {
        let lines: Vec<&str> = snmp.lines().collect();
        for i in 0..lines.len() {
            if lines[i].starts_with("Tcp:") && i + 1 < lines.len() && lines[i + 1].starts_with("Tcp:") {
                let headers: Vec<&str> = lines[i].split_whitespace().collect();
                let values: Vec<&str> = lines[i + 1].split_whitespace().collect();
                let mut tcp = serde_json::Map::new();
                for (j, h) in headers.iter().enumerate() {
                    if j < values.len() {
                        match *h {
                            "ActiveOpens" | "PassiveOpens" | "AttemptFails" | "EstabResets" |
                            "CurrEstab" | "RetransSegs" | "InErrs" | "OutRsts" | "InSegs" | "OutSegs" => {
                                tcp.insert(h.to_string(), values[j].parse::<u64>().unwrap_or(0).into());
                            }
                            _ => {}
                        }
                    }
                }
                if !tcp.is_empty() {
                    info.insert("tcp".into(), serde_json::Value::Object(tcp));
                }
                break;
            }
        }
    }

    // PSI (Pressure Stall Information) - Linux 4.20+
    for resource in &["cpu", "memory", "io"] {
        if let Ok(psi) = std::fs::read_to_string(format!("/proc/pressure/{}", resource)) {
            for line in psi.lines() {
                if line.starts_with("some") {
                    // Parse: some avg10=0.00 avg60=0.00 avg300=0.00 total=12345
                    let mut psi_data = serde_json::Map::new();
                    for part in line.split_whitespace().skip(1) {
                        if let Some((k, v)) = part.split_once('=') {
                            psi_data.insert(k.to_string(), v.parse::<f64>().unwrap_or(0.0).into());
                        }
                    }
                    info.insert(format!("pressure_{}", resource), serde_json::Value::Object(psi_data));
                    break;
                }
            }
        }
    }

    // File descriptor usage
    if let Ok(fdr) = std::fs::read_to_string("/proc/sys/fs/file-nr") {
        let parts: Vec<&str> = fdr.split_whitespace().collect();
        if parts.len() >= 3 {
            info.insert("file_descriptors".into(), serde_json::json!({
                "allocated": parts[0].parse::<u64>().unwrap_or(0),
                "free": parts[1].parse::<u64>().unwrap_or(0),
                "max": parts[2].parse::<u64>().unwrap_or(0),
            }));
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

    // Suppress noisy findings during boot (uptime < 10 min)
    let uptime_secs = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(9999.0);
    let booting = uptime_secs < 600.0;

    // ── Kernel probes (process exec, network connections, FD pressure) ──
    findings.extend(watch_kernel());

    // ── System logs (journald + dmesg) ──
    findings.extend(watch_logs());

    // ── Network + DNS health ──
    findings.extend(watch_network());

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

    // Boot suppression: during first 10 min, suppress noisy post-reboot findings
    if booting {
        let boot_noise = [
            "service_errors_k3s", "ebpf_tcpdrop", "ebpf_tcp_resets",
            "dmesg_new_errors", "dmesg_network_events",
        ];
        findings.retain(|f| !boot_noise.iter().any(|n| f.key.starts_with(n)));
    }

    // Suppress known-good noise: EFI vars (tiny filesystem, always ~93%)
    findings.retain(|f| !f.message.contains("efivars"));

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

// ── Network + DNS watch ──

// ── Log + state watcher ──
// Design principle: NEVER hardcode what to look for.
// 1. Collect ALL journald errors (the raw lines)
// 2. Collect ALL sysfs state for network interfaces
// 3. Detect deltas in /proc counters
// 4. Send everything to cloud - temporal correlation handles the rest
// The cloud side + LLM interprets what went wrong. The agent just collects.

// ── Kernel probes ──
// Process exec, network connections, listeners, FD pressure.
// Pure /proc + /sys - works on every Linux including NixOS.
// Converts kernel_probes::KernelEvent into Finding for unified reporting.

fn watch_kernel() -> Vec<Finding> {
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    // Report probe capability status - degraded mode if no eBPF
    let has_ebpf = crate::kernel_probes::ebpf_available();
    let probes = crate::kernel_probes::available_probes();
    let unavailable: Vec<&str> = probes.iter()
        .filter(|(_, tier, avail)| *tier == "always" && !avail)
        .map(|(name, _, _)| *name)
        .collect();

    let mut findings = Vec::new();

    if !has_ebpf {
        findings.push(Finding {
            key: "probe_mode_degraded".into(),
            severity: "info".into(),
            category: "agent".into(),
            title: "Running in /proc fallback mode (no eBPF)".into(),
            message: format!(
                "eBPF not available (BTF missing or no CAP_BPF). Using /proc polling at 60s resolution. \
                Missing real-time probes: runqlat (CPU queue latency), tcprtt (TCP round-trip). \
                To enable full resolution: ensure /sys/kernel/btf/vmlinux exists and agent has CAP_BPF.",
            ),
            metadata: serde_json::json!({
                "ebpf_available": false,
                "mode": "procfs_fallback",
                "resolution_sec": 60,
                "missing_probes": ["runqlat", "tcprtt"],
                "degraded_probes": ["biolatency", "cachestat", "tcpretrans", "oomkill"],
            }),
        });
    }

    if !unavailable.is_empty() {
        findings.push(Finding {
            key: "probes_unavailable".into(),
            severity: "warning".into(),
            category: "agent".into(),
            title: format!("{} Tier-1 probes unavailable", unavailable.len()),
            message: format!(
                "Cannot run probes: {}. These provide critical performance data. \
                Check that /proc and /sys are mounted.",
                unavailable.join(", ")
            ),
            metadata: serde_json::json!({"unavailable": unavailable}),
        });
    }

    let events = crate::kernel_probes::poll_all(&hostname);

    // Aggregate by template key (probe + comm), not per-PID.
    // "5 new curl processes" is one finding, not 5.
    let mut grouped: std::collections::HashMap<String, Vec<&crate::kernel_probes::KernelEvent>> =
        std::collections::HashMap::new();

    for event in &events {
        if event.severity == "ignore" { continue; }

        // Template key: probe + comm (no PID, no port, no IP)
        // This groups all "process_exec curl" events together
        let template = format!("kernel_{}_{}",
            event.probe,
            if event.comm.is_empty() { "unknown" } else { &event.comm }
        );
        grouped.entry(template).or_default().push(event);
    }

    for (key, group) in &grouped {
        let first = group[0];
        let count = group.len();

        // Only report warning+ severity, or info with count > 1 (anomalous volume)
        let severity = if group.iter().any(|e| e.severity == "critical") { "critical" }
            else if group.iter().any(|e| e.severity == "high") { "high" }
            else if group.iter().any(|e| e.severity == "warning") { "warning" }
            else if count > 10 { "warning" } // unusually high volume = worth reporting
            else { "info" };

        // Only send info-level if it's interesting (containers, suspicious)
        if severity == "info" && first.container_id.is_none() && count <= 3 {
            continue; // boring host process, skip
        }

        let category = match first.probe.as_str() {
            "process_exec" => "process",
            "network_connect" | "network_listen" => "network",
            "fd_pressure" => "system",
            _ => "kernel",
        };

        let title = if count == 1 {
            format!("[{}] {}", first.probe, first.comm)
        } else {
            format!("[{}] {} (x{})", first.probe, first.comm, count)
        };

        // Include samples (up to 5) in metadata for the cloud to correlate
        let samples: Vec<serde_json::Value> = group.iter().take(5).map(|e| {
            serde_json::json!({
                "pid": e.pid, "ppid": e.ppid, "uid": e.uid,
                "container_id": e.container_id,
                "namespace": e.namespace,
                "pod": e.pod,
                "detail": e.detail,
            })
        }).collect();

        findings.push(Finding {
            key: key.clone(),
            severity: severity.into(),
            category: category.into(),
            title,
            message: format!(
                "{} {} events in last interval. comm={} container={} ns={}",
                count, first.probe, first.comm,
                first.container_id.as_deref().unwrap_or("-"),
                first.namespace.as_deref().unwrap_or("-"),
            ),
            metadata: serde_json::json!({
                "probe": first.probe,
                "count": count,
                "severity": severity,
                "samples": samples,
                "source": "/proc",
            }),
        });
    }

    findings
}

fn watch_logs() -> Vec<Finding> {
    let mut findings = Vec::new();

    // 1. ALL journald errors from the last watch interval - no filtering by pattern
    if let Ok(out) = std::process::Command::new("journalctl")
        .args(["--since", "2 minutes ago", "--priority", "err", "--no-pager",
               "-o", "short", "--quiet", "--no-hostname"])
        .output()
    {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout);
            let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
            if !lines.is_empty() {
                // Group by systemd unit (the process name before the colon)
                let mut by_unit: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
                for line in &lines {
                    // Format: "May 05 17:31:11 unit[pid]: message"
                    let unit = line.split(']').next()
                        .and_then(|s| s.split_whitespace().last())
                        .unwrap_or("unknown")
                        .split('[').next()
                        .unwrap_or("unknown")
                        .to_string();
                    by_unit.entry(unit).or_default().push(line.to_string());
                }

                // One finding per unit with errors - raw log lines included
                for (unit, unit_lines) in &by_unit {
                    let severity = if unit_lines.len() > 10 { "critical" } else { "warning" };
                    findings.push(Finding {
                        key: format!("journal_errors_{}", unit),
                        severity: severity.into(),
                        category: "system".into(),
                        title: format!("{}: {} errors in 2 min", unit, unit_lines.len()),
                        message: format!(
                            "{} error-level messages from {}. Recent:\n{}",
                            unit_lines.len(),
                            unit,
                            unit_lines.iter().rev().take(5)
                                .cloned().collect::<Vec<_>>().join("\n")
                        ),
                        metadata: serde_json::json!({
                            "unit": unit,
                            "count": unit_lines.len(),
                            "source": "journalctl",
                            "sample_lines": unit_lines.iter().rev().take(10)
                                .cloned().collect::<Vec<String>>(),
                        }),
                    });
                }
            }
        }
    }

    // 2. dmesg: ALL kernel errors - read raw, don't interpret
    if let Ok(out) = std::process::Command::new("dmesg")
        .args(["--level", "err,crit,alert,emerg", "--ctime", "--nopager"])
        .output()
    {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout);
            // dmesg with --ctime gives human-readable timestamps
            // We cache the last line count to only report new messages
            let cache_path = dirs::home_dir()
                .unwrap_or_default()
                .join(".savants")
                .join("dmesg_line_count");
            let prev_count: usize = std::fs::read_to_string(&cache_path)
                .ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);

            let all_lines: Vec<&str> = raw.lines().collect();
            let new_count = all_lines.len();

            if new_count > prev_count {
                let new_lines: Vec<&str> = all_lines[prev_count..].to_vec();
                if !new_lines.is_empty() {
                    findings.push(Finding {
                        key: "dmesg_new_errors".into(),
                        severity: "warning".into(),
                        category: "kernel".into(),
                        title: format!("{} new kernel errors", new_lines.len()),
                        message: format!(
                            "{} new error/crit/alert messages in kernel ring buffer:\n{}",
                            new_lines.len(),
                            new_lines.iter().rev().take(10).cloned().collect::<Vec<_>>().join("\n")
                        ),
                        metadata: serde_json::json!({
                            "count": new_lines.len(),
                            "source": "dmesg",
                            "lines": new_lines.iter().rev().take(20).cloned().collect::<Vec<&str>>(),
                        }),
                    });
                }
            }
            let _ = std::fs::write(&cache_path, new_count.to_string());
        }
    }

    // 2b. dmesg: network/driver events at ALL levels (WiFi re-auth, driver errors, link state)
    // These are often the TRUE root cause that err-level misses.
    if let Ok(out) = std::process::Command::new("dmesg")
        .args(["--facility", "kern", "--ctime", "--nopager"])
        .output()
    {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout);
            let cache_path = dirs::home_dir()
                .unwrap_or_default()
                .join(".savants")
                .join("dmesg_net_count");
            let prev_count: usize = std::fs::read_to_string(&cache_path)
                .ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);

            let all_lines: Vec<&str> = raw.lines().collect();
            let new_count = all_lines.len();

            if new_count > prev_count {
                // Filter new lines for network/driver events
                let net_events: Vec<&str> = all_lines[prev_count..].iter()
                    .filter(|l| {
                        let lower = l.to_lowercase();
                        lower.contains("authenticate") || lower.contains("deauth")
                        || lower.contains("disassoc") || lower.contains("associated")
                        || lower.contains("iwlwifi") || lower.contains("link is")
                        || lower.contains("carrier") || lower.contains("link down")
                        || lower.contains("link up") || lower.contains("nic link")
                        || lower.contains("netdev watchdog") || lower.contains("tx timeout")
                        || lower.contains("firmware") && (lower.contains("error") || lower.contains("crash"))
                    })
                    .cloned()
                    .collect();

                if !net_events.is_empty() {
                    findings.push(Finding {
                        key: "dmesg_network_events".into(),
                        severity: if net_events.iter().any(|l| l.to_lowercase().contains("deauth") || l.to_lowercase().contains("disassoc") || l.to_lowercase().contains("tx timeout")) {
                            "critical"
                        } else { "warning" }.into(),
                        category: "network".into(),
                        title: format!("{} network/driver events in dmesg", net_events.len()),
                        message: format!(
                            "{} kernel network events detected. These often precede connectivity failures:\n{}",
                            net_events.len(),
                            net_events.iter().take(10).cloned().collect::<Vec<_>>().join("\n")
                        ),
                        metadata: serde_json::json!({
                            "count": net_events.len(),
                            "source": "dmesg/kern",
                            "lines": net_events.iter().take(20).cloned().collect::<Vec<&str>>(),
                        }),
                    });
                }
            }
            let _ = std::fs::write(&cache_path, new_count.to_string());
        }
    }

    // 3. Network interface state changes via sysfs (zero external tool dependencies)
    if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
        for entry in entries.flatten() {
            let iface = entry.file_name().to_string_lossy().to_string();
            if iface == "lo" || iface.starts_with("veth") || iface.starts_with("br-")
                || iface.starts_with("docker") || iface.starts_with("cni") { continue; }

            let operstate = std::fs::read_to_string(format!("/sys/class/net/{}/operstate", iface))
                .unwrap_or_default().trim().to_string();
            let carrier = std::fs::read_to_string(format!("/sys/class/net/{}/carrier", iface))
                .ok().map(|s| s.trim().to_string()).unwrap_or_default();

            // Any interface that's not "up" and should be (has carrier history) is noteworthy
            if operstate == "down" || operstate == "dormant" || operstate == "lowerlayerdown" {
                // Check if this is a "real" interface (not a tunnel/bridge with no traffic)
                let rx_bytes: u64 = std::fs::read_to_string(format!("/sys/class/net/{}/statistics/rx_bytes", iface))
                    .ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
                if rx_bytes > 0 {
                    findings.push(Finding {
                        key: format!("iface_state_{}", iface),
                        severity: if operstate == "down" { "critical" } else { "warning" }.into(),
                        category: "network".into(),
                        title: format!("Interface {} operstate: {}", iface, operstate),
                        message: format!(
                            "Network interface {} is in '{}' state (carrier: {}). \
                            This interface has carried {} GB of traffic. \
                            State '{}' can cause intermittent connectivity, packet drops, and dependent service failures.",
                            iface, operstate, if carrier == "1" { "up" } else { "down" },
                            rx_bytes / 1_073_741_824, operstate
                        ),
                        metadata: serde_json::json!({
                            "interface": iface,
                            "operstate": operstate,
                            "carrier": carrier,
                            "rx_gb": rx_bytes / 1_073_741_824,
                            "source": "sysfs",
                        }),
                    });
                }
            }
        }
    }

    // 4. TCP retransmit spike detection (delta-based from /proc/net/snmp)
    if let Ok(snmp) = std::fs::read_to_string("/proc/net/snmp") {
        let mut retrans: u64 = 0;
        let mut out_segs: u64 = 0;
        let mut tcp_values = false;
        for line in snmp.lines() {
            if line.starts_with("Tcp:") && !tcp_values {
                tcp_values = true;
                continue;
            }
            if line.starts_with("Tcp:") && tcp_values {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() > 12 {
                    out_segs = parts[10].parse().unwrap_or(0);
                    retrans = parts[12].parse().unwrap_or(0);
                }
                break;
            }
        }

        let cache_path = dirs::home_dir()
            .unwrap_or_default()
            .join(".savants")
            .join("tcp_retrans_cache");

        if let Ok(cached) = std::fs::read_to_string(&cache_path) {
            let parts: Vec<&str> = cached.trim().split(',').collect();
            if parts.len() == 2 {
                let prev_retrans: u64 = parts[0].parse().unwrap_or(0);
                let prev_segs: u64 = parts[1].parse().unwrap_or(0);
                let delta_retrans = retrans.saturating_sub(prev_retrans);
                let delta_segs = out_segs.saturating_sub(prev_segs);

                if delta_segs > 100 && delta_retrans > 0 {
                    let rate = delta_retrans as f64 / delta_segs as f64 * 100.0;
                    if rate > 5.0 {
                        findings.push(Finding {
                            key: "tcp_retransmit_spike".into(),
                            severity: if rate > 20.0 { "critical" } else { "warning" }.into(),
                            category: "network".into(),
                            title: format!("TCP retransmit spike: {:.1}%", rate),
                            message: format!(
                                "{} retransmits out of {} segments ({:.1}%) in last interval.",
                                delta_retrans, delta_segs, rate
                            ),
                            metadata: serde_json::json!({
                                "retransmit_rate_pct": rate,
                                "delta_retrans": delta_retrans,
                                "delta_segments": delta_segs,
                                "source": "/proc/net/snmp",
                            }),
                        });
                    }
                }
            }
        }
        let _ = std::fs::write(&cache_path, format!("{},{}", retrans, out_segs));
    }

    // 5. PSI (Pressure Stall Information) - detects resource starvation the OS itself reports
    for resource in &["cpu", "memory", "io"] {
        if let Ok(psi) = std::fs::read_to_string(format!("/proc/pressure/{}", resource)) {
            for line in psi.lines() {
                if line.starts_with("some ") || line.starts_with("full ") {
                    let kind = if line.starts_with("some") { "some" } else { "full" };
                    if let Some(avg10_str) = line.split("avg10=").nth(1) {
                        let avg10: f64 = avg10_str.split_whitespace().next()
                            .and_then(|s| s.parse().ok()).unwrap_or(0.0);
                        let threshold = if kind == "full" { 10.0 } else { 25.0 };
                        if avg10 > threshold {
                            findings.push(Finding {
                                key: format!("psi_{}_{}", resource, kind),
                                severity: if avg10 > 50.0 { "critical" } else { "warning" }.into(),
                                category: resource.to_string(),
                                title: format!("PSI {}/{}: {:.1}%", resource, kind, avg10),
                                message: format!(
                                    "Pressure Stall Information: {} {} avg10={:.1}%. \
                                    Tasks are stalling waiting for {}.",
                                    resource, kind, avg10, resource
                                ),
                                metadata: serde_json::json!({
                                    "resource": resource, "kind": kind, "avg10": avg10,
                                    "raw": line,
                                    "source": format!("/proc/pressure/{}", resource),
                                }),
                            });
                        }
                    }
                }
            }
        }
    }

    // 6. Critical services that log errors at INFO level (not caught by --priority err)
    // Many services (Tailscale, cloudflared, CoreDNS) log failures as info, not error.
    // We scan their journals and pattern-match for failure keywords.
    let critical_services = ["tailscaled", "cloudflared", "coredns", "k3s"];
    let failure_patterns = [
        "deadline exceeded", "timed out", "i/o timeout", "connection reset",
        "connection refused", "unreachable", "failed", "error", "panic",
        "map response long-poll timed out", "no route to host",
    ];

    for svc in &critical_services {
        if let Ok(out) = std::process::Command::new("journalctl")
            .args(["-u", svc, "--since", "2 minutes ago", "--no-pager", "-o", "short", "--quiet", "--no-hostname"])
            .output()
        {
            if !out.status.success() { continue; }
            let raw = String::from_utf8_lossy(&out.stdout);
            let error_lines: Vec<&str> = raw.lines()
                .filter(|l| {
                    let lower = l.to_lowercase();
                    failure_patterns.iter().any(|p| lower.contains(p))
                })
                .collect();

            if !error_lines.is_empty() {
                findings.push(Finding {
                    key: format!("service_errors_{}", svc),
                    severity: if error_lines.len() > 10 { "critical" } else if error_lines.len() > 3 { "warning" } else { "info" }.into(),
                    category: "service".into(),
                    title: format!("{}: {} error events in 2 min", svc, error_lines.len()),
                    message: format!(
                        "{} failure-pattern matches in {} logs. Recent:\n{}",
                        error_lines.len(), svc,
                        error_lines.iter().rev().take(5).cloned().collect::<Vec<_>>().join("\n")
                    ),
                    metadata: serde_json::json!({
                        "service": svc,
                        "count": error_lines.len(),
                        "source": "journalctl",
                        "sample_lines": error_lines.iter().rev().take(10).cloned().collect::<Vec<&str>>(),
                    }),
                });
            }
        }
    }

    findings
}

/// Ping a host N times, return (loss%, avg_ms, max_ms)
fn ping_stats(host: &str, count: u32) -> (f64, f64, f64) {
    let output = std::process::Command::new("ping")
        .args(["-c", &count.to_string(), "-W", "2", "-i", "0.2", host])
        .output();
    match output {
        Ok(o) => {
            let raw = String::from_utf8_lossy(&o.stdout);
            // Parse "X% packet loss"
            let loss = raw.lines()
                .find(|l| l.contains("packet loss"))
                .and_then(|l| l.split_whitespace()
                    .find(|w| w.ends_with('%'))
                    .and_then(|w| w.trim_end_matches('%').parse::<f64>().ok()))
                .unwrap_or(if o.status.success() { 0.0 } else { 100.0 });
            // Parse "rtt min/avg/max/mdev = X/Y/Z/W ms"
            let (avg, max) = raw.lines()
                .find(|l| l.contains("rtt") || l.contains("round-trip"))
                .and_then(|l| {
                    let nums = l.split('=').nth(1)?.trim().split('/').collect::<Vec<_>>();
                    let avg = nums.get(1).and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0);
                    let max = nums.get(2).and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(0.0);
                    Some((avg, max))
                })
                .unwrap_or((0.0, 0.0));
            (loss, avg, max)
        }
        Err(_) => (100.0, 0.0, 0.0),
    }
}

fn watch_network() -> Vec<Finding> {
    let mut findings = Vec::new();

    // 0. Network path probe: gateway jitter + packet loss + WiFi channel health
    // Uses 5 pings (not 1) to catch micro-burst loss. Measures RTT jitter.
    let gateway = std::fs::read_to_string("/proc/net/route").ok()
        .and_then(|r| r.lines().skip(1).find(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            parts.len() > 2 && parts[1] == "00000000"
        }).and_then(|l| {
            let hex = l.split_whitespace().nth(2)?;
            if hex.len() == 8 {
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let c = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let d = u8::from_str_radix(&hex[0..2], 16).ok()?;
                Some(format!("{}.{}.{}.{}", a, b, c, d))
            } else { None }
        }));

    // WiFi signal + quality
    let (signal_dbm, link_quality) = std::fs::read_to_string("/proc/net/wireless").ok()
        .and_then(|w| w.lines().last().and_then(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            let quality = parts.get(2).and_then(|s| s.trim_end_matches('.').parse::<i32>().ok()).unwrap_or(0);
            let signal = parts.get(3).and_then(|s| s.trim_end_matches('.').parse::<i32>().ok()).unwrap_or(0);
            Some((signal, quality))
        })).unwrap_or((0, 0));

    if let Some(ref gw) = gateway {
        // 5 pings to gateway (catches micro-burst loss that 1 ping misses)
        let (gw_loss, gw_avg, gw_max) = ping_stats(gw, 5);
        let (ext_loss, ext_avg, ext_max) = ping_stats("1.1.1.1", 5);

        if gw_loss > 0.0 {
            findings.push(Finding {
                key: "gateway_packet_loss".into(),
                severity: if gw_loss >= 40.0 { "critical" } else { "warning" }.into(),
                category: "network".into(),
                title: format!("Gateway {}: {:.0}% packet loss, avg {:.1}ms max {:.1}ms", gw, gw_loss, gw_avg, gw_max),
                message: format!(
                    "Packet loss to gateway {} ({:.0}%). WiFi signal: {} dBm (quality {}). \
                    RTT avg={:.1}ms max={:.1}ms. {} \
                    This is between your host and the WiFi AP - not the ISP.",
                    gw, gw_loss, signal_dbm, link_quality, gw_avg, gw_max,
                    if gw_max > 50.0 { "High jitter suggests WiFi channel congestion." }
                    else { "Low jitter suggests AP buffer overflow." }
                ),
                metadata: serde_json::json!({
                    "gateway": gw, "loss_pct": gw_loss, "avg_ms": gw_avg, "max_ms": gw_max,
                    "signal_dbm": signal_dbm, "link_quality": link_quality,
                    "diagnosis": "WiFi/AP issue",
                }),
            });
        } else if ext_loss > 0.0 {
            findings.push(Finding {
                key: "upstream_packet_loss".into(),
                severity: if ext_loss >= 40.0 { "critical" } else { "warning" }.into(),
                category: "network".into(),
                title: format!("ISP: {:.0}% packet loss (gateway OK)", ext_loss),
                message: format!(
                    "Gateway {} is fine (0% loss, {:.1}ms). But {:.0}% loss to 1.1.1.1 (avg {:.1}ms, max {:.1}ms). \
                    WiFi signal {} dBm is not the problem. This is your ISP.",
                    gw, gw_avg, ext_loss, ext_avg, ext_max, signal_dbm
                ),
                metadata: serde_json::json!({
                    "gateway": gw, "gw_loss_pct": 0, "ext_loss_pct": ext_loss,
                    "ext_avg_ms": ext_avg, "ext_max_ms": ext_max,
                    "signal_dbm": signal_dbm, "diagnosis": "ISP/upstream issue",
                }),
            });
        } else if gw_max > 50.0 {
            // No loss but high jitter = congestion starting
            findings.push(Finding {
                key: "gateway_high_jitter".into(),
                severity: "warning".into(),
                category: "network".into(),
                title: format!("Gateway jitter: avg {:.1}ms max {:.1}ms", gw_avg, gw_max),
                message: format!(
                    "Gateway {} responds but with high jitter (max {:.1}ms). \
                    WiFi signal {} dBm. This often precedes packet loss.",
                    gw, gw_max, signal_dbm
                ),
                metadata: serde_json::json!({
                    "gateway": gw, "avg_ms": gw_avg, "max_ms": gw_max,
                    "signal_dbm": signal_dbm, "diagnosis": "WiFi congestion starting",
                }),
            });
        }
    }

    // 0b. WiFi channel survey (if available via iw)
    if let Ok(output) = std::process::Command::new("iw")
        .args(["dev", "wlp170s0", "survey", "dump"])
        .output()
    {
        if output.status.success() {
            let raw = String::from_utf8_lossy(&output.stdout);
            // Parse the "in use" frequency's busy time percentage
            let mut in_use = false;
            let mut channel_busy_pct: Option<f64> = None;
            let mut frequency = String::new();
            for line in raw.lines() {
                if line.contains("[in use]") {
                    in_use = true;
                    frequency = line.split_whitespace().nth(1).unwrap_or("?").to_string();
                }
                if in_use {
                    if let Some(busy) = line.strip_prefix("\t\tchannel busy time:") {
                        let busy_ms: f64 = busy.trim().split_whitespace().next()
                            .and_then(|s| s.parse().ok()).unwrap_or(0.0);
                        if let Some(active) = raw.lines().find(|l| l.contains("channel active time:")) {
                            let active_ms: f64 = active.trim().split_whitespace().nth(3)
                                .and_then(|s| s.parse().ok()).unwrap_or(1.0);
                            if active_ms > 0.0 {
                                channel_busy_pct = Some(busy_ms / active_ms * 100.0);
                            }
                        }
                        break;
                    }
                }
            }
            if let Some(busy) = channel_busy_pct {
                if busy > 50.0 {
                    findings.push(Finding {
                        key: "wifi_channel_congested".into(),
                        severity: if busy > 80.0 { "critical" } else { "warning" }.into(),
                        category: "network".into(),
                        title: format!("WiFi channel {} is {:.0}% busy", frequency, busy),
                        message: format!(
                            "WiFi channel ({} MHz) is {:.0}% busy. This causes micro-burst packet loss \
                            even with good signal ({} dBm). Consider switching to a less congested channel \
                            or using the ethernet adapter.",
                            frequency, busy, signal_dbm
                        ),
                        metadata: serde_json::json!({
                            "frequency_mhz": frequency, "busy_pct": busy,
                            "signal_dbm": signal_dbm, "diagnosis": "WiFi channel congestion",
                        }),
                    });
                }
            }
        }
    }

    // 1. DNS resolution health - test actual resolution
    let dns_targets = ["google.com", "cloudflare.com", "github.com"];
    let mut dns_failures = 0;
    for target in &dns_targets {
        let result = std::process::Command::new("getent")
            .args(["hosts", target])
            .output();
        match result {
            Ok(out) if !out.status.success() => dns_failures += 1,
            Err(_) => dns_failures += 1,
            _ => {}
        }
    }
    if dns_failures > 0 {
        findings.push(Finding {
            key: "dns_resolution_failing".into(),
            severity: if dns_failures >= 2 { "critical" } else { "warning" }.into(),
            category: "network".into(),
            title: format!("DNS resolution failing ({}/{} targets)", dns_failures, dns_targets.len()),
            message: format!("Failed to resolve {} of {} test domains. DNS infrastructure may be degraded.", dns_failures, dns_targets.len()),
            metadata: serde_json::json!({"failures": dns_failures, "targets": dns_targets.len()}),
        });
    }

    // 2. Check /etc/resolv.conf for problematic config
    if let Ok(resolv) = std::fs::read_to_string("/etc/resolv.conf") {
        let nameservers: Vec<&str> = resolv.lines()
            .filter(|l| l.starts_with("nameserver"))
            .map(|l| l.split_whitespace().nth(1).unwrap_or("?"))
            .collect();

        // Report DNS configuration
        findings.push(Finding {
            key: "dns_config".into(),
            severity: "info".into(),
            category: "network".into(),
            title: format!("DNS nameservers: {}", nameservers.join(", ")),
            message: format!("Configured nameservers: {}. Count: {}.", nameservers.join(", "), nameservers.len()),
            metadata: serde_json::json!({"nameservers": nameservers, "count": nameservers.len()}),
        });
    }

    // 3. WiFi as primary interface (server should use ethernet)
    if let Ok(proc_net) = std::fs::read_to_string("/proc/net/dev") {
        let mut wifi_bytes: u64 = 0;
        let mut eth_bytes: u64 = 0;
        let mut wifi_name = String::new();
        let mut eth_name = String::new();

        for line in proc_net.lines().skip(2) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 { continue; }
            let iface = parts[0].trim_end_matches(':');
            let rx_bytes: u64 = parts[1].parse().unwrap_or(0);

            if iface.starts_with("wl") || iface.starts_with("wlan") {
                wifi_bytes = rx_bytes;
                wifi_name = iface.to_string();
            } else if iface.starts_with("en") || iface.starts_with("eth") {
                eth_bytes = rx_bytes;
                eth_name = iface.to_string();
            }
        }

        // Report interface traffic distribution
        if wifi_bytes > 0 || eth_bytes > 0 {
            findings.push(Finding {
                key: "network_traffic_distribution".into(),
                severity: "info".into(),
                category: "network".into(),
                title: "Network interface traffic".into(),
                message: format!(
                    "WiFi ({}): {} GB received. Ethernet ({}): {} GB received.",
                    if wifi_name.is_empty() { "none" } else { &wifi_name },
                    wifi_bytes / 1_073_741_824,
                    if eth_name.is_empty() { "none" } else { &eth_name },
                    eth_bytes / 1_073_741_824
                ),
                metadata: serde_json::json!({"wifi": wifi_name, "wifi_gb": wifi_bytes / 1_073_741_824, "eth": eth_name, "eth_gb": eth_bytes / 1_073_741_824}),
            });
        }

        // Report WiFi power save + operstate via sysfs (no external tools needed)
        if !wifi_name.is_empty() {
            // Check operstate: DORMANT means 802.1X or power-save issues
            let operstate = std::fs::read_to_string(format!("/sys/class/net/{}/operstate", wifi_name))
                .unwrap_or_default().trim().to_string();
            // Check kernel power management
            let dev_power = std::fs::read_to_string(format!("/sys/class/net/{}/device/power/control", wifi_name))
                .unwrap_or_default().trim().to_string();
            // Try iw if available, fall back to sysfs
            let power_save = if let Ok(output) = std::process::Command::new("iw")
                .args(["dev", &wifi_name, "get", "power_save"])
                .output()
            {
                let raw = String::from_utf8_lossy(&output.stdout);
                if raw.contains("on") { "on".to_string() } else { "off".to_string() }
            } else {
                // Fallback: device/power/control "auto" usually means power-save is on
                if dev_power == "auto" { "on (sysfs)".to_string() } else { "off (sysfs)".to_string() }
            };

            let ps_on = power_save.starts_with("on");
            let is_dormant = operstate == "dormant";

            // Power save ON on a server is a problem, not informational
            if ps_on || is_dormant {
                findings.push(Finding {
                    key: format!("wifi_power_save_{}", wifi_name),
                    severity: "warning".into(),
                    category: "network".into(),
                    title: format!("WiFi {} power save active (operstate: {})", wifi_name, operstate),
                    message: format!(
                        "WiFi {} power_save={}, device/power/control={}, operstate={}. \
                        Power save causes intermittent packet drops, DNS timeouts, and tunnel disconnections. \
                        Fix: iw dev {} set power_save off",
                        wifi_name, power_save, dev_power, operstate, wifi_name
                    ),
                    metadata: serde_json::json!({
                        "interface": wifi_name,
                        "power_save": power_save,
                        "operstate": operstate,
                        "device_power_control": dev_power,
                    }),
                });
            } else {
                findings.push(Finding {
                    key: format!("wifi_power_save_{}", wifi_name),
                    severity: "info".into(),
                    category: "network".into(),
                    title: format!("WiFi {} power save: off (operstate: {})", wifi_name, operstate),
                    message: format!("WiFi {} power_save={}, operstate={}. Good.", wifi_name, power_save, operstate),
                    metadata: serde_json::json!({"interface": wifi_name, "power_save": power_save, "operstate": operstate}),
                });
            }
        }
    }

    // 4. Tailscale health check
    if which("tailscale") {
        if let Ok(output) = std::process::Command::new("tailscale")
            .args(["status", "--json"])
            .output()
        {
            if output.status.success() {
                let raw = String::from_utf8_lossy(&output.stdout);
                if let Ok(status) = serde_json::from_str::<serde_json::Value>(&raw) {
                    // Check health warnings
                    if let Some(health) = status["Health"].as_array() {
                        for warning in health {
                            if let Some(msg) = warning.as_str() {
                                findings.push(Finding {
                                    key: format!("tailscale_health_{}", &msg[..20.min(msg.len())]),
                                    severity: "warning".into(),
                                    category: "network".into(),
                                    title: "Tailscale health warning".into(),
                                    message: msg.to_string(),
                                    metadata: serde_json::json!({}),
                                });
                            }
                        }
                    }

                    // Check if self is online
                    if let Some(self_status) = status["Self"]["Online"].as_bool() {
                        if !self_status {
                            findings.push(Finding {
                                key: "tailscale_offline".into(),
                                severity: "critical".into(),
                                category: "network".into(),
                                title: "Tailscale node is offline".into(),
                                message: "This node shows as offline in Tailscale. VPN connectivity and MagicDNS are degraded.".into(),
                                metadata: serde_json::json!({}),
                            });
                        }
                    }
                }
            }
        }
    }

    // 5. Network interface errors/drops
    if let Ok(proc_net) = std::fs::read_to_string("/proc/net/dev") {
        for line in proc_net.lines().skip(2) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 12 { continue; }
            let iface = parts[0].trim_end_matches(':');
            if iface == "lo" || iface.starts_with("veth") || iface.starts_with("br-") || iface.starts_with("docker") || iface.starts_with("flannel") || iface.starts_with("cni") { continue; }

            let rx_errors: u64 = parts[3].parse().unwrap_or(0);
            let rx_drops: u64 = parts[4].parse().unwrap_or(0);
            let tx_errors: u64 = parts[11].parse().unwrap_or(0);
            let tx_drops: u64 = parts[12].parse().unwrap_or(0);

            let total_issues = rx_errors + rx_drops + tx_errors + tx_drops;
            if total_issues > 100 {
                findings.push(Finding {
                    key: format!("iface_errors_{}", iface),
                    severity: if total_issues > 10000 { "critical" } else { "warning" }.into(),
                    category: "network".into(),
                    title: format!("Network errors on {}: {} total", iface, total_issues),
                    message: format!("{}: rx_errors={}, rx_drops={}, tx_errors={}, tx_drops={}", iface, rx_errors, rx_drops, tx_errors, tx_drops),
                    metadata: serde_json::json!({"interface": iface, "rx_errors": rx_errors, "rx_drops": rx_drops, "tx_errors": tx_errors, "tx_drops": tx_drops}),
                });
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

// ── Git watch helpers ──

fn discover_git_repos() -> Vec<std::path::PathBuf> {
    let mut repos = Vec::new();

    // Check common locations
    // When running as root (for eBPF), also check SUDO_USER's home
    let home = dirs::home_dir().unwrap_or_default();
    let sudo_home = std::env::var("SUDO_USER").ok()
        .map(|u| std::path::PathBuf::from(format!("/home/{}", u)));
    let mut search_dirs = vec![
        home.join("git"),
        home.join("projects"),
        home.join("src"),
        home.join("repos"),
        std::path::PathBuf::from("/opt"),
        std::path::PathBuf::from("/srv"),
    ];
    if let Some(ref sh) = sudo_home {
        search_dirs.insert(0, sh.join("git"));
        search_dirs.insert(1, sh.join("projects"));
        search_dirs.insert(2, sh.join("src"));
    }

    for dir in search_dirs {
        if !dir.exists() { continue; }
        // Check top-level dirs for .git
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.join(".git").exists() {
                    repos.push(path.clone());
                }
                // One level deeper (for org/repo structure like ~/git/bernadinm/savants)
                if path.is_dir() {
                    if let Ok(sub_entries) = std::fs::read_dir(&path) {
                        for sub in sub_entries.flatten() {
                            if sub.path().join(".git").exists() {
                                repos.push(sub.path());
                            }
                        }
                    }
                }
            }
        }
    }

    repos.sort();
    repos.dedup();
    repos
}

fn git_head(repo: &std::path::Path) -> Option<String> {
    let repo_str = repo.to_string_lossy();

    // Fetch latest from remote (non-blocking, ignore errors)
    let _ = std::process::Command::new("git")
        .args(["-C", &repo_str, "fetch", "--quiet"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // Detect default branch from git, fall back to HEAD
    let default_ref = std::process::Command::new("git")
        .args(["-C", &repo_str, "symbolic-ref", "refs/remotes/origin/HEAD"])
        .output().ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let refs: Vec<&str> = if !default_ref.is_empty() {
        vec![default_ref.as_str(), "HEAD"]
    } else {
        vec!["HEAD"]
    };
    for branch in &refs {
        let output = std::process::Command::new("git")
            .args(["-C", &repo_str, "rev-parse", branch])
            .output()
            .ok();
        if let Some(out) = output {
            if out.status.success() {
                return Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
            }
        }
    }
    None
}

fn detect_deploy(repo_name: &str, commit: &str) -> Option<String> {
    // Check if any k8s deployment has an image tag matching this commit
    let output = std::process::Command::new("kubectl")
        .args(["get", "deployments", "--all-namespaces", "-o", "json"])
        .output()
        .ok()?;

    if !output.status.success() { return None; }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let items = json["items"].as_array()?;
    let short_commit = &commit[..7.min(commit.len())];

    for item in items {
        let name = item["metadata"]["name"].as_str().unwrap_or("?");
        let ns = item["metadata"]["namespace"].as_str().unwrap_or("?");

        if let Some(containers) = item["spec"]["template"]["spec"]["containers"].as_array() {
            for c in containers {
                let image = c["image"].as_str().unwrap_or("");
                // Match by commit SHA in image tag or by repo name
                if image.contains(short_commit) || (image.to_lowercase().contains(&repo_name.to_lowercase()) && image.contains(':')) {
                    return Some(format!("{}/{} image={}", ns, name, image));
                }
            }
        }
    }

    None
}

fn get_machine_id() -> String {
    // Linux: /etc/machine-id
    if let Ok(id) = std::fs::read_to_string("/etc/machine-id") {
        return id.trim().to_string();
    }
    // macOS: IOPlatformUUID
    if let Ok(out) = std::process::Command::new("ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
    {
        let raw = String::from_utf8_lossy(&out.stdout);
        for line in raw.lines() {
            if line.contains("IOPlatformUUID") {
                if let Some(uuid) = line.split('"').nth(3) {
                    return uuid.to_string();
                }
            }
        }
    }
    // Fallback: hostname
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
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

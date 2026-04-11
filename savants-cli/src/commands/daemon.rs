//! Savants daemon — single process that watches all infrastructure.
//!
//! Like dockerd or tailscaled: one daemon, manages everything.
//!
//!   savants daemon start     # start in background
//!   savants daemon status    # show what's being watched
//!   savants daemon stop      # graceful shutdown
//!   savants daemon logs      # tail the daemon log

use colored::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn daemon_pid_file() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".savants").join("daemon.pid")
}

fn daemon_log_file() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".savants").join("daemon.log")
}

fn is_daemon_running() -> Option<u32> {
    let pid_file = daemon_pid_file();
    if !pid_file.exists() { return None; }
    let pid: u32 = fs::read_to_string(&pid_file).ok()?.trim().parse().ok()?;
    // Check if process is alive
    if std::path::Path::new(&format!("/proc/{}", pid)).exists() {
        Some(pid)
    } else {
        let _ = fs::remove_file(&pid_file);
        None
    }
}

pub fn start() {
    if let Some(pid) = is_daemon_running() {
        println!("Savants daemon already running (pid {})", pid);
        return;
    }

    println!("{}", "Starting Savants daemon...".bold());

    // Ensure the graph engine is running first
    let embedded = crate::embedded::EmbeddedEngine::new();
    match embedded.ensure_running() {
        Ok(true) => println!("  {} Memory engine: started", "●".green()),
        Ok(false) => println!("  {} Memory engine: already running", "●".green()),
        Err(e) => {
            eprintln!("  {} Memory engine: {}", "●".red(), e);
            return;
        }
    }

    let log_file = daemon_log_file();
    let savants_bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("savants"));

    // The daemon is just `savants daemon run` in the background
    // Forward ALL current env vars so Gotify/webhook config is preserved
    let log = fs::File::create(&log_file).expect("Cannot create daemon log");
    let mut cmd = Command::new(&savants_bin);
    cmd.args(["daemon", "run"])
        .env("SAVANTS_PORT", embedded.port.to_string())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .stdin(Stdio::null());

    // Forward alert config env vars
    for key in &["SAVANTS_GOTIFY_URL", "SAVANTS_GOTIFY_TOKEN", "SAVANTS_WEBHOOK_URL",
                 "KUBECONFIG", "HOME", "PATH", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY",
                 "AWS_DEFAULT_REGION", "GOOGLE_APPLICATION_CREDENTIALS"] {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }

    let child = cmd.spawn().expect("Failed to start daemon");

    let pid = child.id();
    fs::write(daemon_pid_file(), pid.to_string()).expect("Cannot write pid file");

    println!("  {} Daemon started (pid {})", "●".green(), pid);
    println!("  Log: {}", log_file.display());
    println!();
    println!("The daemon will automatically discover and watch:");
    println!("  - All K8s clusters in your kubeconfig");
    println!("  - Host metrics (every 30s)");
    println!("  - eBPF security probes (if running as root)");
    println!();
    println!("Check status: {}", "savants daemon status".cyan());
    println!("View logs:    {}", "savants daemon logs".cyan());
    println!("Stop:         {}", "savants daemon stop".cyan());
}

pub fn stop() {
    match is_daemon_running() {
        Some(pid) => {
            // Send SIGTERM
            unsafe { libc::kill(pid as i32, libc::SIGTERM); }
            println!("Savants daemon stopped (pid {})", pid);
            let _ = fs::remove_file(daemon_pid_file());
        }
        None => {
            println!("Savants daemon is not running.");
        }
    }
}

pub fn status() {
    match is_daemon_running() {
        Some(pid) => {
            println!("{} Savants daemon: {} (pid {})", "●".green(), "running".green(), pid);
            println!();

            // Show what's being watched by querying the graph
            let state = crate::config::State::load();
            if let Ok(client) = crate::graph::GraphClient::new(&state.graph_name()) {
                // Hosts
                if let Ok(r) = client.query("MATCH (h:Host) RETURN h.hostname, h.cpu_percent, h.memory_percent", &[]) {
                    println!("{}:", "Hosts".bold());
                    for row in &r.rows {
                        println!("  {} — CPU {:.0}%, Memory {:.0}%",
                            row[0].as_str(), row[1].as_f64(), row[2].as_f64());
                    }
                }

                // Clusters
                println!();
                println!("{}:", "Clusters".bold());
                for cluster in &["astra-k3s", "taria-prod", "taria-dev", "default"] {
                    let graph_name = cluster.replace("-", "_");
                    if let Ok(cc) = crate::graph::GraphClient::new(&graph_name) {
                        if let Ok(r) = cc.query("MATCH (p:K8sPod) RETURN p.status, count(p) ORDER BY count(p) DESC", &[]) {
                            if !r.rows.is_empty() {
                                let status_str: String = r.rows.iter()
                                    .map(|r| format!("{} {}", r[1].as_i64(), r[0].as_str()))
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                println!("  {} — {}", cluster.cyan(), status_str);
                            }
                        }
                    }
                }

                // eBPF
                println!();
                if let Ok(r) = client.query("MATCH (e:KernelSecurityEvent) RETURN count(e)", &[]) {
                    let count = r.rows.first().map(|r| r[0].as_i64()).unwrap_or(0);
                    if count > 0 {
                        println!("{}: {} events captured", "eBPF".bold(), count);
                    } else {
                        println!("{}: no events (not running or no activity)", "eBPF".bold());
                    }
                }
            }
        }
        None => {
            println!("{} Savants daemon: {}", "●".red(), "not running".red());
            println!("Start with: {}", "savants daemon start".cyan());
        }
    }
}

pub fn logs() {
    let log_file = daemon_log_file();
    if !log_file.exists() {
        println!("No daemon log found. Start the daemon first.");
        return;
    }
    // Tail the log
    let _ = Command::new("tail")
        .args(["-f", "-n", "50"])
        .arg(&log_file)
        .status();
}

/// The actual daemon loop — called by `savants daemon run`.
/// Discovers clusters, starts watchers, monitors host, runs forever.
pub async fn run() {
    println!("Savants daemon starting...");

    let state = crate::config::State::load();
    let port = state.graph_port();
    std::env::set_var("SAVANTS_PORT", port.to_string());

    // Discover K8s clusters from kubeconfig
    let clusters = discover_kubeconfig_clusters();
    println!("Found {} K8s clusters: {:?}", clusters.len(), clusters);

    // Start host monitoring loop in a dedicated thread (GraphClient isn't Send)
    std::thread::spawn(|| {
        loop {
            let state = crate::config::State::load();
            if let Ok(client) = crate::graph::GraphClient::new(&state.graph_name()) {
                let ingestor = crate::host::HostIngestor::new(client, None, 20);
                let stats = ingestor.snapshot();
                println!("[host] {}", stats.summary().lines().next().unwrap_or(""));
            }
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
    });

    // Start K8s snapshot loops in dedicated threads
    // (GraphClient isn't Send, so we use OS threads + per-thread tokio runtimes)
    for cluster in &clusters {
        let cluster = cluster.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                loop {
                    println!("[k8s] Snapshot for {}...", cluster);
                    match crate::k8s::K8sIngestor::kube_client_from_kubeconfig(Some(&cluster)).await {
                        Ok(kube_client) => {
                            let graph_name = crate::config::State::cluster_graph_name(&cluster);
                            match crate::graph::GraphClient::new(&graph_name) {
                                Ok(graph) => {
                                    let ingestor = crate::k8s::K8sIngestor::new(
                                        graph, cluster.clone(), kube_client
                                    );
                                    let stats = ingestor.snapshot().await;
                                    println!("[k8s] {} — {}", cluster,
                                        stats.summary().lines().next().unwrap_or(""));
                                }
                                Err(e) => println!("[k8s] {} graph error: {}", cluster, e),
                            }
                        }
                        Err(e) => println!("[k8s] {} connect error: {}", cluster, e),
                    }
                    // Re-snapshot every 60s
                    tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                }
            });
        });
    }

    // Start alert + repo + AWS monitoring thread
    std::thread::spawn(|| {
        let alert_config = crate::alerts::AlertConfig::from_env();
        if alert_config.is_configured() {
            println!("[alerts] Gotify/webhook alerting enabled");
        }

        // Discover git repos to watch
        let repo_paths = discover_git_repos();
        let mut repo_watchers: Vec<super::watch_repo::RepoWatcher> = repo_paths
            .iter()
            .map(|(path, branch)| {
                println!("[repo] Watching {} (branch: {})", path, branch);
                super::watch_repo::RepoWatcher::new(path, branch)
            })
            .collect();

        loop {
            // 1. Check alerts (graph-based: CrashLoop, disk, security, WiFi)
            let state = crate::config::State::load();
            if let Ok(client) = crate::graph::GraphClient::new(&state.graph_name()) {
                crate::alerts::check_and_alert(&client, &alert_config);
            }

            // 2. Poll GitHub repos for new commits
            for watcher in &mut repo_watchers {
                if watcher.poll() {
                    // New commits detected — notify
                    if alert_config.is_configured() {
                        let title = format!("New commits in {}", watcher.repo_path.rsplit('/').next().unwrap_or("repo"));
                        let msg = format!("Code changes detected. Re-indexing recommended.");
                        fire_gotify(&alert_config, &title, &msg, 3);
                    }
                }
            }

            // 3. Check AWS health (lightweight — just API status calls)
            check_aws_health(&alert_config);

            std::thread::sleep(std::time::Duration::from_secs(60)); // check every minute
        }
    });

    // Start cloud cost ingestion thread (every 6 hours)
    std::thread::spawn(|| {
        // Run immediately on startup, then every 6 hours
        loop {
            let state = crate::config::State::load();
            if let Ok(client) = crate::graph::GraphClient::new(&state.graph_name()) {
                // GCP: auto-discover projects and billing exports
                let gcp_configs = crate::cloud_cost::discover_gcp_billing();
                for (project, dataset, table) in &gcp_configs {
                    match crate::cloud_cost::ingest_gcp_costs(&client, project, dataset, table) {
                        Ok(n) => println!("[cost] GCP {}: {} services ingested", project, n),
                        Err(e) => println!("[cost] GCP {} error: {}", project, e),
                    }
                }
                if gcp_configs.is_empty() {
                    println!("[cost] GCP: no billing exports found (run: gcloud auth login)");
                }

                // AWS: auto-discovers from configured profile
                match crate::cloud_cost::ingest_aws_costs(&client) {
                    Ok(n) => println!("[cost] AWS: {} services ingested", n),
                    Err(e) => println!("[cost] AWS: {}", e),
                }

                // Check for cost anomalies
                let anomalies = crate::cloud_cost::check_cost_anomalies(&client);
                for (_service, _cost, msg) in &anomalies {
                    println!("[cost] ⚠️ {}", msg);
                }
            }

            std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
        }
    });

    // Wait forever (or until Ctrl-C)
    println!("Daemon running. Ctrl-C to stop.");
    tokio::signal::ctrl_c().await.ok();
    println!("Daemon shutting down...");
}

fn discover_git_repos() -> Vec<(String, String)> {
    // Look for known repo paths
    let candidates = [
        ("/home/miguel/git/sourcecoders-ai/talent-pipeline", "main"),
        ("/home/miguel/git/bernadinm/savants", "master"),
    ];
    candidates.iter()
        .filter(|(path, _)| Path::new(path).join(".git").exists())
        .map(|(p, b)| (p.to_string(), b.to_string()))
        .collect()
}

fn fire_gotify(config: &crate::alerts::AlertConfig, title: &str, message: &str, priority: u8) {
    if let (Some(url), Some(token)) = (&config.gotify_url, &config.gotify_token) {
        let url = format!("{}/message?token={}", url, token);
        let body = serde_json::json!({
            "title": format!("Savants: {}", title),
            "message": message,
            "priority": priority,
        });
        let _ = std::thread::spawn(move || {
            let client = reqwest::blocking::Client::new();
            let _ = client.post(&url)
                .json(&body)
                .timeout(std::time::Duration::from_secs(5))
                .send();
        });
    }
}

fn check_aws_health(config: &crate::alerts::AlertConfig) {
    // Check EKS cluster status
    for cluster in &["taria-prod-eks", "taria-dev-eks"] {
        let output = Command::new("aws")
            .args(["eks", "describe-cluster", "--name", cluster, "--region", "us-west-2",
                   "--query", "cluster.status", "--output", "text"])
            .output();
        if let Ok(o) = output {
            let status = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !status.is_empty() && status != "ACTIVE" {
                fire_gotify(config, &format!("EKS {} unhealthy", cluster), &format!("Status: {}", status), 8);
            }
        }
    }

    // Check RDS health
    let output = Command::new("aws")
        .args(["rds", "describe-db-instances",
               "--query", "DBInstances[?DBInstanceStatus!=`available`].{Name:DBInstanceIdentifier,Status:DBInstanceStatus}",
               "--output", "text"])
        .output();
    if let Ok(o) = output {
        let unhealthy = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if !unhealthy.is_empty() {
            fire_gotify(config, "RDS unhealthy", &unhealthy, 8);
        }
    }

    // Check CloudWatch alarms in ALARM state
    let output = Command::new("aws")
        .args(["cloudwatch", "describe-alarms", "--state-value", "ALARM",
               "--query", "MetricAlarms[].AlarmName", "--output", "text"])
        .output();
    if let Ok(o) = output {
        let alarms = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if !alarms.is_empty() {
            fire_gotify(config, "CloudWatch ALARM", &alarms, 8);
        }
    }
}

fn discover_kubeconfig_clusters() -> Vec<String> {
    let kube_config = dirs::home_dir()
        .map(|h| h.join(".kube/config"))
        .filter(|p| p.exists());

    let Some(config_path) = kube_config else { return vec![]; };
    let content = fs::read_to_string(&config_path).unwrap_or_default();

    let mut contexts = vec![];
    let mut in_contexts = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "contexts:" {
            in_contexts = true;
            continue;
        }
        if in_contexts && trimmed.starts_with("- name:") {
            let name = trimmed.trim_start_matches("- name:").trim().trim_matches('"');
            // Skip known-dead clusters
            if name.contains("do-sfo3") { continue; }
            contexts.push(name.to_string());
        }
        if in_contexts && !trimmed.is_empty() && !trimmed.starts_with('-')
            && !trimmed.starts_with("name:") && !trimmed.starts_with("context:")
            && !trimmed.starts_with("cluster:") && !trimmed.starts_with("user:")
            && !trimmed.starts_with("namespace:") && !trimmed.starts_with('#')
            && !trimmed.starts_with("- ")
        {
            in_contexts = false;
        }
    }
    contexts
}

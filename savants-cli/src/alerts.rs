//! Alert system — pushes urgent notifications to Gotify, Slack, or webhooks.
//!
//! The daemon calls `check_and_alert()` after each ingest cycle.
//! It compares the current state against alert rules and fires
//! notifications for new issues only (deduplicates by alert ID).

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static FIRED_ALERTS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

#[derive(Debug, Clone)]
pub struct Alert {
    pub id: String,
    pub severity: AlertSeverity,
    pub title: String,
    pub message: String,
    pub source: String,   // "k8s:taria-prod", "host:astra", "ebpf:astra"
}

#[derive(Debug, Clone, Copy)]
pub enum AlertSeverity {
    Info,     // Gotify priority 1
    Warning,  // Gotify priority 5
    Critical, // Gotify priority 8
    Emergency,// Gotify priority 10
}

impl AlertSeverity {
    fn gotify_priority(&self) -> u8 {
        match self {
            Self::Info => 1,
            Self::Warning => 5,
            Self::Critical => 8,
            Self::Emergency => 10,
        }
    }
}

/// Configuration for alert destinations.
pub struct AlertConfig {
    pub gotify_url: Option<String>,
    pub gotify_token: Option<String>,
    pub webhook_url: Option<String>,
    pub min_severity: AlertSeverity,
}

impl AlertConfig {
    pub fn from_env() -> Self {
        Self {
            gotify_url: std::env::var("SAVANTS_GOTIFY_URL").ok(),
            gotify_token: std::env::var("SAVANTS_GOTIFY_TOKEN").ok(),
            webhook_url: std::env::var("SAVANTS_WEBHOOK_URL").ok(),
            min_severity: AlertSeverity::Warning,
        }
    }

    pub fn is_configured(&self) -> bool {
        self.gotify_url.is_some() || self.webhook_url.is_some()
    }
}

/// Check the graph for alertable conditions and fire notifications.
pub fn check_and_alert(client: &crate::graph::GraphClient, config: &AlertConfig) {
    if !config.is_configured() { return; }

    let mut alerts = vec![];

    // ── K8s alerts ──

    // CrashLoopBackOff pods
    for cluster_graph in &["taria_prod", "taria_dev", "default"] {
        if let Ok(cc) = crate::graph::GraphClient::new(cluster_graph) {
            if let Ok(r) = cc.query(
                "MATCH (p:K8sPod) WHERE p.status = 'CrashLoopBackOff' RETURN p.name, p.namespace, p.restart_count",
                &[],
            ) {
                for row in &r.rows {
                    let name = row[0].as_str();
                    let ns = row[1].as_str();
                    let restarts = row[2].as_i64();
                    let cluster = cluster_graph.replace("_", "-");
                    alerts.push(Alert {
                        id: format!("crashloop:{}:{}:{}", cluster, ns, name),
                        severity: AlertSeverity::Critical,
                        title: format!("CrashLoopBackOff: {}/{}", ns, name),
                        message: format!(
                            "Pod {}/{} on cluster {} is in CrashLoopBackOff ({} restarts)",
                            ns, name, cluster, restarts
                        ),
                        source: format!("k8s:{}", cluster),
                    });
                }
            }

            // Failed pods
            if let Ok(r) = cc.query(
                "MATCH (p:K8sPod) WHERE p.status = 'Failed' RETURN p.name, p.namespace",
                &[],
            ) {
                for row in &r.rows {
                    let name = row[0].as_str();
                    let ns = row[1].as_str();
                    let cluster = cluster_graph.replace("_", "-");
                    alerts.push(Alert {
                        id: format!("failed:{}:{}:{}", cluster, ns, name),
                        severity: AlertSeverity::Warning,
                        title: format!("Pod Failed: {}/{}", ns, name),
                        message: format!(
                            "Pod {}/{} on cluster {} is in Failed state",
                            ns, name, cluster
                        ),
                        source: format!("k8s:{}", cluster),
                    });
                }
            }
        }
    }

    // ── Host alerts ──

    // Disk > 85%
    if let Ok(r) = client.query(
        "MATCH (d:HostDisk) WHERE d.percent > 85 RETURN d.hostname, d.mountpoint, d.percent",
        &[],
    ) {
        for row in &r.rows {
            let host = row[0].as_str();
            let mount = row[1].as_str();
            let pct = row[2].as_f64();
            let sev = if pct > 95.0 { AlertSeverity::Emergency }
                      else if pct > 90.0 { AlertSeverity::Critical }
                      else { AlertSeverity::Warning };
            alerts.push(Alert {
                id: format!("disk:{}:{}", host, mount),
                severity: sev,
                title: format!("Disk {:.0}% full: {}", pct, mount),
                message: format!(
                    "Host {} disk {} is at {:.0}% capacity",
                    host, mount, pct
                ),
                source: format!("host:{}", host),
            });
        }
    }

    // Failed systemd units
    if let Ok(r) = client.query(
        "MATCH (u:SystemdUnit) WHERE u.active_state = 'failed' RETURN u.hostname, u.name",
        &[],
    ) {
        for row in &r.rows {
            let host = row[0].as_str();
            let unit = row[1].as_str();
            alerts.push(Alert {
                id: format!("systemd:{}:{}", host, unit),
                severity: AlertSeverity::Warning,
                title: format!("Systemd unit failed: {}", unit),
                message: format!("Host {} unit {} is in failed state", host, unit),
                source: format!("host:{}", host),
            });
        }
    }

    // WiFi high packet discard
    if let Ok(r) = client.query(
        "MATCH (w:WifiStatus) WHERE w.discarded > 5000 RETURN w.hostname, w.interface, w.discarded, w.signal_dbm",
        &[],
    ) {
        for row in &r.rows {
            let host = row[0].as_str();
            let iface = row[1].as_str();
            let discarded = row[2].as_i64();
            alerts.push(Alert {
                id: format!("wifi:{}:{}", host, iface),
                severity: AlertSeverity::Warning,
                title: format!("WiFi instability: {} dropped packets", discarded),
                message: format!(
                    "Host {} WiFi {} is dropping {} packets. Check band/channel/power management.",
                    host, iface, discarded
                ),
                source: format!("host:{}", host),
            });
        }
    }

    // ── eBPF security alerts ──
    if let Ok(r) = client.query(
        "MATCH (e:KernelSecurityEvent) WHERE e.severity IN ['CRITICAL', 'HIGH'] RETURN e.hostname, e.probe, e.comm, e.detail",
        &[],
    ) {
        for row in &r.rows {
            let host = row[0].as_str();
            let probe = row[1].as_str();
            let comm = row[2].as_str();
            let detail = row[3].as_str();
            alerts.push(Alert {
                id: format!("security:{}:{}:{}", host, probe, comm),
                severity: AlertSeverity::Critical,
                title: format!("Security: suspicious {} on {}", probe, host),
                message: detail.to_string(),
                source: format!("ebpf:{}", host),
            });
        }
    }

    // ── Deduplicate and fire ──
    let mut fired = FIRED_ALERTS.lock().unwrap();
    if fired.is_none() {
        *fired = Some(HashSet::new());
    }
    let seen = fired.as_mut().unwrap();

    for alert in &alerts {
        if seen.contains(&alert.id) { continue; }
        seen.insert(alert.id.clone());
        fire_alert(alert, config);
    }
}

fn fire_alert(alert: &Alert, config: &AlertConfig) {
    println!("[alert] {} — {}: {}",
        match alert.severity {
            AlertSeverity::Emergency => "🔴 EMERGENCY",
            AlertSeverity::Critical => "🟠 CRITICAL",
            AlertSeverity::Warning => "🟡 WARNING",
            AlertSeverity::Info => "🔵 INFO",
        },
        alert.title, alert.message
    );

    // Gotify
    if let (Some(url), Some(token)) = (&config.gotify_url, &config.gotify_token) {
        let gotify_url = format!("{}/message?token={}", url, token);
        let body = serde_json::json!({
            "title": format!("Savants: {}", alert.title),
            "message": alert.message,
            "priority": alert.severity.gotify_priority(),
            "extras": {
                "client::notification": {
                    "click": { "url": "https://savants.dev" }
                }
            }
        });

        // Fire and forget — don't block the daemon on notification delivery
        let _ = std::thread::spawn(move || {
            let client = reqwest::blocking::Client::new();
            let _ = client.post(&gotify_url)
                .json(&body)
                .timeout(std::time::Duration::from_secs(5))
                .send();
        });
    }

    // Generic webhook
    if let Some(webhook_url) = &config.webhook_url {
        let url = webhook_url.clone();
        let payload = serde_json::json!({
            "severity": format!("{:?}", alert.severity),
            "title": alert.title,
            "message": alert.message,
            "source": alert.source,
            "timestamp": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        });
        let _ = std::thread::spawn(move || {
            let client = reqwest::blocking::Client::new();
            let _ = client.post(&url)
                .json(&payload)
                .timeout(std::time::Duration::from_secs(5))
                .send();
        });
    }
}

/// Clear a specific alert (when the condition resolves).
pub fn clear_alert(id: &str) {
    if let Ok(mut fired) = FIRED_ALERTS.lock() {
        if let Some(set) = fired.as_mut() {
            set.remove(id);
        }
    }
}

/// Clear all alerts (on daemon restart).
pub fn clear_all() {
    if let Ok(mut fired) = FIRED_ALERTS.lock() {
        *fired = Some(HashSet::new());
    }
}

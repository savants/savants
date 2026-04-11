//! Alert system — pushes urgent notifications to Gotify, Slack, or webhooks.
//!
//! The daemon calls `check_and_alert()` after each ingest cycle.
//! It compares the current state against alert rules and fires
//! notifications for new issues only (deduplicates by alert ID).

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Tracks when each alert last fired. Re-fires after the cooldown period.
/// Critical: re-fire every 15 minutes. Warning: every 30 minutes. Info: once.
static FIRED_ALERTS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

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

    // ── Run the v2 dynamic diagnosis engine across all log events ──
    // This is the "savant brain" — it finds patterns, queries context,
    // and generates the FULL diagnosis with root cause + fix.
    run_smart_diagnosis(client, config, &mut alerts);

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

                    // Check for CAUSED_BY correlation
                    let cause = cc.query(
                        &format!(
                            "MATCH (e:LogEvent {{pod: '{}', namespace: '{}'}})-[r:CAUSED_BY]->(x) \
                             RETURN labels(x)[0], x.name, r.change_type, r.delta_seconds LIMIT 1",
                            name, ns
                        ),
                        &[],
                    ).ok().and_then(|r| r.rows.first().map(|row| {
                        format!("Likely caused by {} on {} ({}s before crash)",
                            row.get(2).map(|v| v.as_str()).unwrap_or("?"),
                            row.get(1).map(|v| v.as_str()).unwrap_or("?"),
                            row.get(3).map(|v| v.as_i64()).unwrap_or(0))
                    }));

                    // Check recent log errors for this pod
                    let top_error = cc.query(
                        &format!(
                            "MATCH (e:LogEvent {{pod: '{}', namespace: '{}'}}) \
                             WHERE e.severity IN ['ERROR','FATAL'] \
                             RETURN e.template_text, e.count ORDER BY e.count DESC LIMIT 1",
                            name, ns
                        ),
                        &[],
                    ).ok().and_then(|r| r.rows.first().map(|row| {
                        let tmpl: String = row[0].as_str().chars().take(100).collect();
                        format!("Top error (x{}): {}", row[1].as_i64(), tmpl)
                    }));

                    let mut message = format!(
                        "Pod {}/{} on {} — {} restarts\n",
                        ns, name, cluster, restarts
                    );
                    if let Some(cause) = cause {
                        message.push_str(&format!("\n{}\n", cause));
                    }
                    if let Some(error) = top_error {
                        message.push_str(&format!("\n{}\n", error));
                    }
                    message.push_str(&format!(
                        "\nCheck: kubectl --context {} -n {} logs {} --previous",
                        cluster, ns, name
                    ));

                    alerts.push(Alert {
                        id: format!("crashloop:{}:{}:{}", cluster, ns, name),
                        severity: AlertSeverity::Critical,
                        title: format!("CrashLoopBackOff: {}/{}", ns, name),
                        message,
                        source: format!("k8s:{}", cluster),
                    });
                }
            }

            // Failed pods — with smart context
            if let Ok(r) = cc.query(
                "MATCH (p:K8sPod) WHERE p.status = 'Failed' RETURN p.name, p.namespace, p.owner_kind, p.owner_name, p.image",
                &[],
            ) {
                for row in &r.rows {
                    let name = row[0].as_str();
                    let ns = row[1].as_str();
                    let owner_kind = row.get(2).map(|v| v.as_str()).unwrap_or("");
                    let owner_name = row.get(3).map(|v| v.as_str()).unwrap_or("");
                    let image = row.get(4).map(|v| v.as_str()).unwrap_or("");
                    let cluster = cluster_graph.replace("_", "-");

                    // Check if there's a healthy replacement (same owner, Running)
                    let has_replacement = if !owner_name.is_empty() {
                        cc.query(
                            &format!(
                                "MATCH (p:K8sPod) WHERE p.namespace = '{}' AND p.owner_name = '{}' AND p.status = 'Running' RETURN count(p)",
                                ns, owner_name
                            ),
                            &[],
                        ).ok()
                        .and_then(|r| r.rows.first().map(|r| r[0].as_i64()))
                        .unwrap_or(0) > 0
                    } else { false };

                    let (severity, diagnosis) = if has_replacement {
                        (AlertSeverity::Info, format!(
                            "Pod {}/{} on {} failed but a healthy replacement is running. \
                             This is a dead pod that K8s left behind.\n\n\
                             Fix: kubectl --context {} -n {} delete pod {}",
                            ns, name, cluster, cluster, ns, name
                        ))
                    } else {
                        // No replacement — check what it depends on
                        let deps = cc.query(
                            &format!(
                                "MATCH (p:K8sPod {{name: '{}', namespace: '{}'}})-[:READS]->(cm) RETURN labels(cm)[0], cm.name",
                                name, ns
                            ),
                            &[],
                        ).ok().map(|r| {
                            r.rows.iter()
                                .map(|r| format!("{}: {}", r[0].as_str(), r[1].as_str()))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }).unwrap_or_default();

                        (AlertSeverity::Warning, format!(
                            "Pod {}/{} on {} failed with no healthy replacement.\n\
                             Image: {}\n\
                             Owner: {}/{}\n\
                             {}\n\n\
                             Check logs: kubectl --context {} -n {} logs {} --previous",
                            ns, name, cluster,
                            image,
                            owner_kind, owner_name,
                            if deps.is_empty() { "No config dependencies found.".to_string() }
                            else { format!("Depends on: {}", deps) },
                            cluster, ns, name
                        ))
                    };

                    alerts.push(Alert {
                        id: format!("failed:{}:{}:{}", cluster, ns, name),
                        severity,
                        title: format!("Pod Failed: {}/{}", ns, name),
                        message: diagnosis,
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

    // ── Deduplicate with cooldown periods ──
    // Critical/Emergency: re-notify every 15 minutes if still active
    // Warning: re-notify every 30 minutes
    // Info: notify once, never repeat
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let mut fired = FIRED_ALERTS.lock().unwrap();
    if fired.is_none() {
        *fired = Some(HashMap::new());
    }
    let seen = fired.as_mut().unwrap();

    for alert in &alerts {
        let cooldown_secs = match alert.severity {
            AlertSeverity::Emergency => 900,  // 15 minutes
            AlertSeverity::Critical => 900,   // 15 minutes
            AlertSeverity::Warning => 1800,   // 30 minutes
            AlertSeverity::Info => u64::MAX,  // never repeat
        };

        let should_fire = match seen.get(&alert.id) {
            None => true,                              // never fired
            Some(last) => now - last >= cooldown_secs, // cooldown expired
        };

        if should_fire {
            seen.insert(alert.id.clone(), now);
            fire_alert(alert, config);
        }
    }

    // Clean up resolved alerts: if an ID was in `seen` but NOT in current alerts,
    // the issue resolved. Remove it so it can re-fire if it comes back.
    let current_ids: std::collections::HashSet<String> = alerts.iter().map(|a| a.id.clone()).collect();
    seen.retain(|id, _| current_ids.contains(id));
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

/// Run the v2 diagnosis engine: detect patterns → query graph for context → generate rich alerts.
fn run_smart_diagnosis(
    client: &crate::graph::GraphClient,
    config: &AlertConfig,
    alerts: &mut Vec<Alert>,
) {
    // Collect RECENT log events only — no stale alerts for already-fixed issues.
    // Only alert on events with last_seen in the last 30 minutes.
    let since_ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64() - 1800.0;
    let mut events: Vec<(String, i64, String)> = vec![];

    // Host log events (recent only)
    if let Ok(r) = client.query(
        &format!(
            "MATCH (e:HostLogEvent) WHERE e.severity IN ['ERROR', 'FATAL'] \
             AND e.last_seen >= {} \
             RETURN e.template_text, e.count, e.severity ORDER BY e.count DESC LIMIT 20",
            since_ts
        ),
        &[],
    ) {
        for row in &r.rows {
            events.push((row[0].as_str().to_string(), row[1].as_i64(), row[2].as_str().to_string()));
        }
    }

    // K8s log events from all clusters (recent only)
    for cluster_graph in &["taria_prod", "taria_dev", "default"] {
        if let Ok(cc) = crate::graph::GraphClient::new(cluster_graph) {
            if let Ok(r) = cc.query(
                &format!(
                    "MATCH (e:LogEvent) WHERE e.severity IN ['ERROR', 'FATAL'] \
                     AND e.last_seen >= {} \
                     RETURN e.template_text, e.count, e.severity ORDER BY e.count DESC LIMIT 20",
                    since_ts
                ),
                &[],
            ) {
                for row in &r.rows {
                    events.push((row[0].as_str().to_string(), row[1].as_i64(), row[2].as_str().to_string()));
                }
            }
        }
    }

    // Run the v2 dynamic diagnosis engine
    let diagnoses = crate::knowledge::diagnose_with_context(&events, client);

    for diag in &diagnoses {
        let severity = match diag.pattern.severity {
            crate::knowledge::Severity::Critical => AlertSeverity::Critical,
            crate::knowledge::Severity::Error => AlertSeverity::Critical,
            crate::knowledge::Severity::Warning => AlertSeverity::Warning,
            crate::knowledge::Severity::Info => AlertSeverity::Info,
        };

        // Build the rich message: explanation + context-aware fix
        let message = format!(
            "{}\n\n{}\n\n{}",
            diag.pattern.explanation,
            if diag.suggested_fix.is_empty() { diag.pattern.fix.to_string() } else { diag.suggested_fix.clone() },
            format!("({} occurrences)", diag.occurrences),
        );

        alerts.push(Alert {
            id: format!("knowledge:{}", diag.pattern.id),
            severity,
            title: format!("{:?}: {}", diag.pattern.category, diag.pattern.id.replace('-', " ")),
            message,
            source: "knowledge-engine".to_string(),
        });
    }
}

/// Clear a specific alert (when the condition resolves).
pub fn clear_alert(id: &str) {
    if let Ok(mut fired) = FIRED_ALERTS.lock() {
        if let Some(set) = fired.as_mut() {
            set.remove(&id.to_string());
        }
    }
}

/// Clear all alerts (on daemon restart).
pub fn clear_all() {
    if let Ok(mut fired) = FIRED_ALERTS.lock() {
        *fired = Some(HashMap::new());
    }
}

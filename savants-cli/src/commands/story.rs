use colored::*;
use crate::graph::GraphClient;

pub async fn run(
    since_minutes: u64,
    min_severity: &str,
    cluster_filter: Option<String>,
    host_filter: Option<String>,
) {
    let client = match GraphClient::new("savants") {
        Ok(c) if c.is_connected() => c,
        _ => {
            eprintln!("{}", "Graph not connected. Run 'savants up' first.".red());
            return;
        }
    };

    let sev_list = severity_filter(min_severity);
    let since = if since_minutes > 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        now - (since_minutes as f64 * 60.0)
    } else {
        0.0
    };

    let window_label = if since_minutes > 0 {
        format!("last {}m", since_minutes)
    } else {
        "all time".to_string()
    };

    // ── Host section ──
    if let Ok(hosts) = client.query("MATCH (h:Host) RETURN h.hostname, h.os, h.cpu_count, h.cpu_percent, h.memory_total_mb, h.memory_used_mb, h.memory_percent, h.load_1m, h.load_5m, h.load_15m", &[]) {
        for row in &hosts.rows {
            let hn = row[0].as_str();
            if let Some(ref filter) = host_filter {
                if hn != filter { continue; }
            }

            println!("{} {}", "#".dimmed(), format!("Host: {}", hn).bold());
            println!("  OS: {} | CPU: {}% ({} cores) | Memory: {:.1}/{:.1} GB ({:.1}%) | Load: {:.2}",
                row[1].as_str(),
                row[3].as_f64(),
                row[2].as_i64(),
                row[5].as_f64() / 1024.0,
                row[4].as_f64() / 1024.0,
                row[6].as_f64(),
                row[7].as_f64(),
            );

            // Failed systemd units
            if let Ok(units) = client.query(
                "MATCH (h:Host {hostname: $hn})-[:HAS_UNIT]->(u:SystemdUnit) WHERE u.active_state = 'failed' RETURN u.name, u.description",
                &[("hn", hn)]
            ) {
                if !units.rows.is_empty() {
                    println!();
                    println!("  {}:", "Failed units".red());
                    for u in &units.rows {
                        println!("    {} {}", "-".dimmed(), u[0].as_str());
                    }
                }
            }

            // Journal/kernel events
            let since_str = format!("{:.0}", since);
            let query = format!(
                "MATCH (h:Host {{hostname: $hn}})-[:EMITTED]->(e:HostLogEvent) \
                 WHERE e.severity IN {} {} \
                 RETURN e.unit, e.count, e.template_text \
                 ORDER BY e.count DESC LIMIT 15",
                sev_list,
                if since > 0.0 { "AND e.last_seen >= $since" } else { "" }
            );
            if let Ok(events) = client.query(&query, &[("hn", hn), ("since", &since_str)]) {
                if !events.rows.is_empty() {
                    println!();
                    println!("  Journal errors ({}):", window_label);
                    for e in &events.rows {
                        let unit = e[0].as_str();
                        let count = e[1].as_i64();
                        let tmpl = e[2].as_str();
                        let tmpl_short: String = tmpl.chars().take(100).collect();
                        println!("    {:<8}{}: {}",
                            format!("x{}", count).yellow(),
                            if unit.is_empty() { "?".dimmed().to_string() } else { unit.to_string() },
                            tmpl_short.dimmed(),
                        );
                    }
                }
            }
            println!();
        }
    }

    // ── K8s clusters ──
    // Try to find cluster graphs by listing graphs or checking K8sCluster nodes
    let cluster_names = find_clusters(&client, cluster_filter.as_deref());

    for cluster_name in &cluster_names {
        let graph_name = cluster_name.replace("-", "_");
        let cc = match GraphClient::new(&graph_name) {
            Ok(c) if c.is_connected() => c,
            _ => continue,
        };

        // Pod status
        if let Ok(pods) = cc.query("MATCH (p:K8sPod) RETURN p.status, count(p) ORDER BY count(p) DESC", &[]) {
            let status_str: String = pods.rows.iter()
                .map(|r| {
                    let status = r[0].as_str();
                    let count = r[1].as_i64();
                    let colored = match status {
                        "Running" => format!("{} {}", count, status).green().to_string(),
                        "CrashLoopBackOff" => format!("{} {}", count, status).red().to_string(),
                        "Failed" => format!("{} {}", count, status).red().to_string(),
                        _ => format!("{} {}", count, status),
                    };
                    colored
                })
                .collect::<Vec<_>>()
                .join(", ");
            println!("{} {} ({})", "#".dimmed(), format!("Cluster: {}", cluster_name).bold(), status_str);
        }

        // Log events
        let since_str = format!("{:.0}", since);
        let query = format!(
            "MATCH (e:LogEvent) WHERE e.severity IN {} {} \
             RETURN e.namespace, e.pod, e.severity, e.count, e.template_text \
             ORDER BY CASE e.severity WHEN 'FATAL' THEN 3 WHEN 'ERROR' THEN 2 WHEN 'WARN' THEN 1 ELSE 0 END DESC, e.count DESC \
             LIMIT 15",
            sev_list,
            if since > 0.0 { "AND e.last_seen >= $since" } else { "" }
        );
        if let Ok(events) = cc.query(&query, &[("since", &since_str)]) {
            if !events.rows.is_empty() {
                println!();
                println!("  Top errors ({}):", window_label);
                for e in &events.rows {
                    let ns = e[0].as_str();
                    let pod = e[1].as_str();
                    let sev = e[2].as_str();
                    let count = e[3].as_i64();
                    let tmpl: String = e[4].as_str().chars().take(100).collect();

                    let sev_colored = match sev {
                        "FATAL" => format!("[{}]", sev).red().bold().to_string(),
                        "ERROR" => format!("[{}]", sev).red().to_string(),
                        "WARN" => format!("[{}]", sev).yellow().to_string(),
                        _ => format!("[{}]", sev),
                    };
                    println!("    {} {}/{} x{}: {}",
                        sev_colored,
                        ns.dimmed(), pod,
                        count,
                        tmpl.dimmed(),
                    );
                }
            } else {
                println!();
                println!("  {}", "No log errors found.".dimmed());
            }
        }

        // Mentions
        if let Ok(mentions) = cc.query(
            &format!(
                "MATCH (e:LogEvent)-[:MENTIONS]->(x) WHERE e.severity IN {} {} \
                 RETURN labels(x)[0], x.namespace, x.name, count(DISTINCT e) \
                 ORDER BY count(DISTINCT e) DESC LIMIT 8",
                sev_list,
                if since > 0.0 { "AND e.last_seen >= $since" } else { "" }
            ),
            &[("since", &since_str)]
        ) {
            if !mentions.rows.is_empty() {
                println!();
                println!("  {}:", "Referenced entities".cyan());
                for m in &mentions.rows {
                    let label = m[0].as_str().replace("K8s", "");
                    let ns = m[1].as_str();
                    let name = m[2].as_str();
                    let n = m[3].as_i64();
                    println!("    {} {}/{} {} {} event(s)",
                        label.cyan(), ns.dimmed(), name, "←".dimmed(), n);
                }
            }
        }

        println!();
    }
}

fn severity_filter(min: &str) -> String {
    let rank = |s: &str| match s.to_uppercase().as_str() {
        "INFO" => 0, "WARN" => 1, "ERROR" => 2, "FATAL" => 3, _ => 1,
    };
    let min_rank = rank(min);
    let allowed: Vec<&str> = ["INFO", "WARN", "ERROR", "FATAL"]
        .iter()
        .filter(|s| rank(s) >= min_rank)
        .copied()
        .collect();
    format!("[{}]", allowed.iter().map(|s| format!("'{}'", s)).collect::<Vec<_>>().join(", "))
}

fn find_clusters(client: &GraphClient, filter: Option<&str>) -> Vec<String> {
    let mut clusters = vec![];

    // Check default graph for K8sCluster nodes
    if let Ok(r) = client.query("MATCH (c:K8sCluster) RETURN c.name", &[]) {
        for row in &r.rows {
            let name = row[0].as_str().to_string();
            if let Some(f) = filter {
                if name != f { continue; }
            }
            if !name.is_empty() {
                clusters.push(name);
            }
        }
    }

    // Also try to discover cluster graphs by convention
    // (graph names like "astra_k3s" for cluster "astra-k3s")
    if clusters.is_empty() {
        // Try common patterns
        for name in &["astra-k3s", "default", "production", "staging"] {
            if let Some(f) = filter {
                if *name != f { continue; }
            }
            let graph_name = name.replace("-", "_");
            if let Ok(cc) = GraphClient::new(&graph_name) {
                if let Ok(r) = cc.query("MATCH (p:K8sPod) RETURN count(p)", &[]) {
                    if let Some(row) = r.rows.first() {
                        if row[0].as_i64() > 0 {
                            clusters.push(name.to_string());
                        }
                    }
                }
            }
        }
    }

    clusters
}

use colored::*;
use crate::graph::GraphClient;

pub async fn run() {
    println!("{}", "Savants Status".bold());
    println!();

    // Graph connection
    match GraphClient::new("savants") {
        Ok(client) => {
            if client.is_connected() {
                println!("  {} Graph: {}", "●".green(), "connected".green());
                // Node/edge counts
                if let Ok(r) = client.query("MATCH (n) RETURN count(n)", &[]) {
                    if let Some(row) = r.rows.first() {
                        println!("    Nodes: {}", row[0].as_i64());
                    }
                }
                if let Ok(r) = client.query("MATCH ()-[r]->() RETURN count(r)", &[]) {
                    if let Some(row) = r.rows.first() {
                        println!("    Edges: {}", row[0].as_i64());
                    }
                }
                // Hosts
                if let Ok(r) = client.query("MATCH (h:Host) RETURN h.hostname", &[]) {
                    if !r.rows.is_empty() {
                        let hosts: Vec<&str> = r.rows.iter().map(|r| r[0].as_str()).collect();
                        println!("    Hosts: {}", hosts.join(", "));
                    }
                }
                // K8s clusters
                if let Ok(r) = client.query("MATCH (c:K8sCluster) RETURN c.name", &[]) {
                    if !r.rows.is_empty() {
                        let clusters: Vec<&str> = r.rows.iter().map(|r| r[0].as_str()).collect();
                        println!("    K8s:   {}", clusters.join(", "));
                    }
                }
            } else {
                println!("  {} Graph: {}", "●".red(), "not connected".red());
                println!("    Run 'savants up' to start the graph.");
            }
        }
        Err(_) => {
            println!("  {} Graph: {}", "●".red(), "not running".red());
            println!("    Run 'savants up' to start.");
        }
    }
}

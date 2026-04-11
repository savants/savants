use colored::*;
use std::path::Path;
use crate::find_in_path;

pub async fn run(repo: Option<String>, tail_lines: u32) {
    println!("{}", "Starting Savants...".bold());
    println!();

    // 1. Ensure graph is running
    // Try connecting; if it fails, start the embedded FalkorDB
    let connected = crate::graph::GraphClient::new("savants")
        .map(|c| c.is_connected())
        .unwrap_or(false);

    if connected {
        println!("  {} Graph: connected", "●".green());
    } else {
        println!("  {} Graph: starting...", "●".yellow());
        // Start via Python agent (it handles the embedded FalkorDB lifecycle)
        super::agent::run_python(&["up",
            "--tail-lines", &tail_lines.to_string(),
            &repo.as_deref().map(|r| format!("--repo {}", r)).unwrap_or_default(),
        ]);
        return; // Python handled the full flow
    }

    println!();
    println!("{}...", "Detecting infrastructure".bold());

    // Auto-detect K8s clusters
    let k8s_clusters = detect_k8s();
    for c in &k8s_clusters {
        println!("  Found K8s cluster: {}", c.cyan());
    }
    if k8s_clusters.is_empty() {
        println!("  {}", "No K8s clusters found".dimmed());
    }

    // Docker
    if find_in_path("docker").is_some() {
        println!("  Found Docker");
    }

    // systemd
    if find_in_path("systemctl").is_some() {
        println!("  Found systemd");
    }

    // Git repo
    let repo_path = repo.or_else(|| {
        if Path::new(".git").exists() {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        } else {
            None
        }
    });
    if let Some(ref r) = repo_path {
        println!("  Found git repo: {}", r.cyan());
    }

    println!();

    // Delegate all ingestion to Python agent
    let mut args = vec!["up".to_string()];
    args.extend(["--tail-lines".into(), tail_lines.to_string()]);
    if let Some(r) = repo_path {
        args.extend(["--repo".into(), r]);
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    super::agent::run_python(&refs);
}

fn detect_k8s() -> Vec<String> {
    // Quick check: does ~/.kube/config exist?
    let kube_config = dirs::home_dir()
        .map(|h| h.join(".kube/config"))
        .filter(|p| p.exists());

    if kube_config.is_none() {
        return vec![];
    }

    // Parse contexts from kubeconfig (simple YAML grep, no full parser needed)
    let config_path = kube_config.unwrap();
    let content = std::fs::read_to_string(&config_path).unwrap_or_default();
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
            contexts.push(name.to_string());
        }
        if in_contexts && !trimmed.is_empty() && !trimmed.starts_with('-') && !trimmed.starts_with("name:") && !trimmed.starts_with("context:") && !trimmed.starts_with("cluster:") && !trimmed.starts_with("user:") && !trimmed.starts_with("namespace:") {
            if !trimmed.starts_with('#') && !trimmed.starts_with("- ") {
                in_contexts = false;
            }
        }
    }

    contexts
}

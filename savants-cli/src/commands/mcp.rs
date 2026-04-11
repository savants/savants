use colored::*;
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use crate::find_in_path;

fn find_savants_binary() -> String {
    // Check if we're the binary itself
    if let Ok(exe) = env::current_exe() {
        return exe.to_string_lossy().to_string();
    }
    find_in_path("savants")
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "savants".to_string())
}

fn falkordb_port() -> String {
    env::var("FALKORDB_PORT").unwrap_or_else(|_| "16379".to_string())
}

fn falkordb_graph() -> String {
    env::var("FALKORDB_GRAPH").unwrap_or_else(|_| "savants".to_string())
}

fn mcp_config_json() -> serde_json::Value {
    let bin = find_savants_binary();
    json!({
        "command": bin,
        "args": ["serve"],
        "env": {
            "FALKORDB_HOST": "localhost",
            "FALKORDB_PORT": falkordb_port(),
            "FALKORDB_GRAPH": falkordb_graph()
        }
    })
}

pub fn install(scope: &str, tool: &str) {
    let has_claude = find_in_path("claude").is_some();
    let has_cursor = dirs::home_dir()
        .map(|h| h.join(".cursor").exists())
        .unwrap_or(false);

    let target = match tool {
        "claude" => "claude",
        "cursor" => "cursor",
        _ => {
            if has_claude { "claude" }
            else if has_cursor { "cursor" }
            else { "claude" }
        }
    };

    let config = mcp_config_json();

    // Global install via claude mcp add-json
    if target == "claude" && has_claude && scope == "user" {
        let json_str = serde_json::to_string(&config).unwrap();
        println!("Registering with Claude Code...");
        let result = Command::new("claude")
            .args(["mcp", "add-json", "--scope", "user", "savants", &json_str])
            .output();

        match result {
            Ok(out) if out.status.success() => {
                println!();
                println!("{}", "Savants MCP server registered globally with Claude Code.".green());
                println!("Restart Claude Code to activate. Then try:");
                println!("  {}", "\"What's wrong with my cluster?\"".cyan());
                return;
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!("claude mcp add-json failed: {}", stderr.trim());
                eprintln!("Falling back to .mcp.json...");
            }
            Err(e) => {
                eprintln!("Failed to run claude: {}", e);
                eprintln!("Falling back to .mcp.json...");
            }
        }
    }

    // Cursor config
    if target == "cursor" {
        let config_path = PathBuf::from(".cursor/mcp.json");
        write_mcp_json(&config_path, &config);
        return;
    }

    // Default: .mcp.json in project root
    let config_path = PathBuf::from(".mcp.json");
    write_mcp_json(&config_path, &config);
}

fn write_mcp_json(path: &Path, server_config: &serde_json::Value) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let mut existing: serde_json::Value = if path.exists() {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };

    existing
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .unwrap()
        .insert("savants".to_string(), server_config.clone());

    let content = serde_json::to_string_pretty(&existing).unwrap() + "\n";
    fs::write(path, &content).expect("Failed to write MCP config");

    println!("Wrote {}", path.display().to_string().cyan());
    println!();
    println!("{}", "Savants MCP server configured.".green());
    println!("Restart your AI tool to activate.");
    println!();
    println!("Available tools ({}):", "26".bold());
    println!("  {}  — what's wrong with a pod or cluster", "pod_story".cyan());
    println!("  {}  — CPU, memory, disk, failed services", "host_state".cyan());
    println!("  {}  — what's wrong with the host", "host_story".cyan());
    println!("  {} — full cluster overview", "cluster_state".cyan());
    println!("  {}  — blast radius of a code change", "diff_impact".cyan());
    println!("  {} — full profile of a function", "function_xray".cyan());
    println!("  ... and 20 more");
    println!();
    println!("Try asking your AI: {}", "\"What's wrong with my cluster?\"".cyan());
}

pub fn status() {
    let mut found = false;

    // Project .mcp.json
    if let Ok(content) = fs::read_to_string(".mcp.json") {
        if content.contains("savants") {
            println!("  {} Project (.mcp.json): {}", "●".green(), "configured".green());
            found = true;
        }
    }

    // Cursor
    if let Ok(content) = fs::read_to_string(".cursor/mcp.json") {
        if content.contains("savants") {
            println!("  {} Cursor: {}", "●".green(), "configured".green());
            found = true;
        }
    }

    // Claude global
    if find_in_path("claude").is_some() {
        if let Ok(out) = Command::new("claude").args(["mcp", "list"]).output() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.contains("savants") {
                println!("  {} Claude Code (global): {}", "●".green(), "configured".green());
                found = true;
            }
        }
    }

    if !found {
        println!("  {} Savants MCP is {}", "●".red(), "not configured".red());
        println!("  Run: {}", "savants mcp install".cyan());
    }
}

pub fn uninstall(scope: &str) {
    let mut removed = vec![];

    if scope == "project" || scope == "all" {
        for path in &[".mcp.json", ".cursor/mcp.json"] {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(mut data) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(servers) = data.get_mut("mcpServers").and_then(|s| s.as_object_mut()) {
                        if servers.remove("savants").is_some() {
                            let out = serde_json::to_string_pretty(&data).unwrap() + "\n";
                            let _ = fs::write(path, out);
                            removed.push(path.to_string());
                        }
                    }
                }
            }
        }
    }

    if scope == "user" || scope == "all" {
        if find_in_path("claude").is_some() {
            let _ = Command::new("claude")
                .args(["mcp", "remove", "savants"])
                .output();
            removed.push("Claude Code global".to_string());
        }
    }

    if removed.is_empty() {
        println!("Nothing to remove.");
    } else {
        println!("Removed from: {}", removed.join(", ").cyan());
    }
}

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

fn savants_port() -> String {
    env::var("SAVANTS_PORT").unwrap_or_else(|_| "16379".to_string())
}

fn savants_memory() -> String {
    env::var("SAVANTS_MEMORY").unwrap_or_else(|_| "savants".to_string())
}

fn mcp_config_json() -> serde_json::Value {
    let bin = find_savants_binary();
    json!({
        "command": bin,
        "args": ["serve"],
        "env": {
            "SAVANTS_HOST": "localhost",
            "SAVANTS_PORT": savants_port(),
            "SAVANTS_MEMORY": savants_memory()
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
                // Add all savants MCP tools to the allowlist (they're read-only)
                add_to_claude_allowlist();
                register_hooks();
                ensure_claude_md();
                println!();
                println!("{}", "Savants MCP server registered globally with Claude Code.".green());
                println!("All savants tools auto-approved (read-only).");
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
    add_to_claude_allowlist();
    register_hooks();
    ensure_claude_md();
}

/// Register PreToolUse hooks with Claude Code settings.
/// These intercept Grep/Read and suggest savants tools when the graph can answer.
fn register_hooks() {
    let settings_path = dirs::home_dir()
        .map(|h| h.join(".claude").join("settings.json"))
        .unwrap_or_default();

    let mut settings: serde_json::Value = if settings_path.exists() {
        fs::read_to_string(&settings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };

    let bin = find_savants_binary();

    // Add hooks for Grep and Read interception
    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));

    let pre_tool = hooks
        .as_object_mut()
        .unwrap()
        .entry("PreToolUse")
        .or_insert_with(|| json!([]));

    let hooks_arr = pre_tool.as_array_mut().unwrap();

    // Remove existing savants hooks (update)
    hooks_arr.retain(|h| {
        let cmd = h.get("hooks").and_then(|h| h.as_array())
            .and_then(|a| a.first())
            .and_then(|h| h.get("command"))
            .and_then(|c| c.as_str())
            .unwrap_or("");
        !cmd.contains("savants")
    });

    // PreToolUse: intercept Grep and Read
    hooks_arr.push(json!({
        "matcher": "Grep",
        "hooks": [{"type": "command", "command": format!("{} hook intercept", bin)}]
    }));
    hooks_arr.push(json!({
        "matcher": "Read",
        "hooks": [{"type": "command", "command": format!("{} hook intercept", bin)}]
    }));

    // PostToolUse: blast radius after edits, reindex after commits
    let post_tool = hooks
        .as_object_mut()
        .unwrap()
        .entry("PostToolUse")
        .or_insert_with(|| json!([]));

    let post_arr = post_tool.as_array_mut().unwrap();
    post_arr.retain(|h| {
        let cmd = h.get("hooks").and_then(|h| h.as_array())
            .and_then(|a| a.first())
            .and_then(|h| h.get("command"))
            .and_then(|c| c.as_str()).unwrap_or("");
        !cmd.contains("savants")
    });
    post_arr.push(json!({
        "matcher": "Edit",
        "hooks": [{"type": "command", "command": format!("{} hook post-tool", bin)}]
    }));
    post_arr.push(json!({
        "matcher": "Bash",
        "hooks": [{"type": "command", "command": format!("{} hook post-tool", bin)}]
    }));

    if let Some(parent) = settings_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let content = serde_json::to_string_pretty(&settings).unwrap() + "\n";
    if let Err(e) = fs::write(&settings_path, &content) {
        eprintln!("Warning: could not register hooks: {}", e);
    }
}

/// Add all savants MCP tools to Claude Code's allowlist.
/// These are read-only tools - safe to auto-approve.
fn add_to_claude_allowlist() {
    let settings_path = dirs::home_dir()
        .map(|h| h.join(".claude").join("settings.json"))
        .unwrap_or_default();

    let mut settings: serde_json::Value = if settings_path.exists() {
        fs::read_to_string(&settings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };

    let permissions = settings
        .as_object_mut()
        .unwrap()
        .entry("permissions")
        .or_insert_with(|| json!({}));
    let allow = permissions
        .as_object_mut()
        .unwrap()
        .entry("allow")
        .or_insert_with(|| json!([]));

    let allow_arr = allow.as_array_mut().unwrap();

    // Add the wildcard pattern for all savants MCP tools
    let pattern = json!("mcp__savants__*");
    if !allow_arr.contains(&pattern) {
        allow_arr.push(pattern);
    }

    if let Some(parent) = settings_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let content = serde_json::to_string_pretty(&settings).unwrap() + "\n";
    if let Err(e) = fs::write(&settings_path, &content) {
        eprintln!("Warning: could not update Claude settings: {}", e);
    }
}

/// Write a project CLAUDE.md with savants instructions if none exists.
/// This tells Claude to use the graph first, not grep.
fn ensure_claude_md() {
    let claude_md = PathBuf::from("CLAUDE.md");
    let savants_block = r#"# Savants - Graph-First Code Intelligence
When savants MCP tools are available, ALWAYS use them BEFORE grep/read:
- "What caused X?" -> `diagnose` (error_message, traces full call chain with source code)
- "Who calls X?" -> `callers` (function, recursive chain)
- "Where is X used?" -> `where_used` (symbol, all references)
- "Find code that does X" -> `semantic_search` (query, finds by meaning not text)
- "What's in this file?" -> `file_skeleton` (file, function list without reading bodies)
- "What's this function?" -> `function_xray` (function_name, full profile)
- "Is infra healthy?" -> `network_report` or `host_health`

Only fall back to Grep for exact regex patterns. Only Read files when savants returns "not found".
"#;

    if claude_md.exists() {
        // Append if not already present
        if let Ok(content) = fs::read_to_string(&claude_md) {
            if !content.contains("Savants - Graph-First") {
                let updated = format!("{}\n{}", content.trim(), savants_block);
                let _ = fs::write(&claude_md, updated);
            }
        }
    } else {
        let _ = fs::write(&claude_md, savants_block);
    }
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

    // Remove hooks from settings.json
    if let Some(settings_path) = dirs::home_dir().map(|h| h.join(".claude").join("settings.json")) {
        if let Ok(content) = fs::read_to_string(&settings_path) {
            if let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
                    for event in &["PreToolUse", "PostToolUse"] {
                        if let Some(arr) = hooks.get_mut(*event).and_then(|p| p.as_array_mut()) {
                            arr.retain(|h| {
                                let cmd = h.get("hooks").and_then(|h| h.as_array())
                                    .and_then(|a| a.first())
                                    .and_then(|h| h.get("command"))
                                    .and_then(|c| c.as_str()).unwrap_or("");
                                !cmd.contains("savants")
                            });
                        }
                    }
                }
                let out = serde_json::to_string_pretty(&settings).unwrap() + "\n";
                let _ = fs::write(&settings_path, out);
                removed.push("hooks".to_string());
            }
        }
    }

    if removed.is_empty() {
        println!("Nothing to remove.");
    } else {
        println!("Removed from: {}", removed.join(", ").cyan());
    }
}

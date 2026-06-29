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
    let has_windsurf = dirs::home_dir()
        .map(|h| h.join(".codeium").exists() || h.join(".windsurf").exists())
        .unwrap_or(false);
    let has_continue = dirs::home_dir()
        .map(|h| h.join(".continue").exists())
        .unwrap_or(false);
    let has_zed = dirs::config_dir()
        .map(|h| h.join("zed").exists())
        .unwrap_or(false);

    let target = match tool {
        "claude" => "claude",
        "cursor" => "cursor",
        "windsurf" => "windsurf",
        "vscode" => "vscode",
        "continue" => "continue",
        "zed" => "zed",
        _ => {
            // Auto-detect: install to all found editors
            if has_claude { "claude" }
            else if has_cursor { "cursor" }
            else if has_windsurf { "windsurf" }
            else if has_continue { "continue" }
            else if has_zed { "zed" }
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
                add_to_claude_allowlist();
                register_hooks();
                ensure_claude_md();
                println!();
                println!("{}", "Savants MCP server registered globally with Claude Code.".green());
                println!("All savants tools auto-approved (read-only).");
                println!("Restart Claude Code to activate. Then try:");
                println!("  {}", "\"What's wrong with my cluster?\"".cyan());
                println!();
                println!("Verify with: {}", "savants mcp test".cyan());
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

    // Editor-specific config paths
    match target {
        "cursor" => {
            let config_path = PathBuf::from(".cursor/mcp.json");
            write_mcp_json(&config_path, &config);
        }
        "windsurf" => {
            // Windsurf uses ~/.codeium/windsurf/mcp_config.json (global)
            // or .windsurf/mcp.json (project)
            let home = dirs::home_dir().unwrap_or_default();
            let global_path = home.join(".codeium").join("windsurf").join("mcp_config.json");
            let project_path = PathBuf::from(".windsurf/mcp.json");

            if scope == "user" && global_path.parent().map(|p| p.exists()).unwrap_or(false) {
                write_mcp_json(&global_path, &config);
            } else {
                write_mcp_json(&project_path, &config);
            }
        }
        "vscode" => {
            // VS Code uses .vscode/mcp.json
            let config_path = PathBuf::from(".vscode/mcp.json");
            write_mcp_json(&config_path, &config);
        }
        "continue" => {
            // Continue uses ~/.continue/config.json with mcpServers
            let home = dirs::home_dir().unwrap_or_default();
            let config_path = home.join(".continue").join("config.json");
            write_continue_config(&config_path, &config);
        }
        "zed" => {
            // Zed uses ~/.config/zed/settings.json with context_servers
            let config_dir = dirs::config_dir().unwrap_or_default();
            let settings_path = config_dir.join("zed").join("settings.json");
            write_zed_config(&settings_path, &config);
        }
        _ => {
            // Default: .mcp.json in project root (works for most MCP clients)
            let config_path = PathBuf::from(".mcp.json");
            write_mcp_json(&config_path, &config);
        }
    }

    // Claude-specific extras (hooks, allowlist, CLAUDE.md)
    if target == "claude" || has_claude {
        add_to_claude_allowlist();
        register_hooks();
    }
    ensure_claude_md();

    println!();
    println!("Verify with: {}", "savants mcp test".cyan());
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

    // Build the complete hooks config (replace all savants hooks)
    let hook_entry = |matcher: &str, cmd: &str| -> serde_json::Value {
        json!({"matcher": matcher, "hooks": [{"type": "command", "command": cmd}]})
    };

    let intercept_cmd = format!("{} hook intercept", bin);
    let post_cmd = format!("{} hook post-tool", bin);

    // Build fresh hook arrays, preserving non-savants hooks
    let hooks_obj = settings
        .as_object_mut().unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut().unwrap();

    // PreToolUse
    let pre = hooks_obj.entry("PreToolUse").or_insert_with(|| json!([]));
    let pre_arr = pre.as_array_mut().unwrap();
    pre_arr.retain(|h| {
        !h.get("hooks").and_then(|h| h.as_array()).and_then(|a| a.first())
            .and_then(|h| h.get("command")).and_then(|c| c.as_str())
            .unwrap_or("").contains("savants")
    });
    pre_arr.push(hook_entry("Grep", &intercept_cmd));
    pre_arr.push(hook_entry("Bash", &intercept_cmd));
    pre_arr.push(hook_entry("Read", &intercept_cmd));

    // PostToolUse
    let post = hooks_obj.entry("PostToolUse").or_insert_with(|| json!([]));
    let post_arr = post.as_array_mut().unwrap();
    post_arr.retain(|h| {
        !h.get("hooks").and_then(|h| h.as_array()).and_then(|a| a.first())
            .and_then(|h| h.get("command")).and_then(|c| c.as_str())
            .unwrap_or("").contains("savants")
    });
    post_arr.push(hook_entry("Edit", &post_cmd));
    post_arr.push(hook_entry("Bash", &post_cmd));

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

/// Write Continue config (~/.continue/config.json)
fn write_continue_config(path: &Path, server_config: &serde_json::Value) {
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

    let mcp_servers = existing
        .as_object_mut().unwrap()
        .entry("experimental")
        .or_insert_with(|| json!({}))
        .as_object_mut().unwrap()
        .entry("modelContextProtocolServers")
        .or_insert_with(|| json!([]));

    // Remove existing savants entry
    if let Some(arr) = mcp_servers.as_array_mut() {
        arr.retain(|s| s.get("name").and_then(|n| n.as_str()) != Some("savants"));
        arr.push(json!({
            "name": "savants",
            "transport": {
                "type": "stdio",
                "command": server_config["command"],
                "args": server_config["args"],
            }
        }));
    }

    let content = serde_json::to_string_pretty(&existing).unwrap() + "\n";
    fs::write(path, &content).expect("Failed to write Continue config");
    println!("Wrote {}", path.display().to_string().cyan());
    println!("{}", "Continue MCP server configured.".green());
}

/// Write Zed config (~/.config/zed/settings.json)
fn write_zed_config(path: &Path, server_config: &serde_json::Value) {
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

    let context_servers = existing
        .as_object_mut().unwrap()
        .entry("context_servers")
        .or_insert_with(|| json!({}))
        .as_object_mut().unwrap();

    context_servers.insert("savants".to_string(), json!({
        "command": {
            "path": server_config["command"],
            "args": server_config["args"],
            "env": server_config.get("env").cloned().unwrap_or(json!({})),
        },
        "settings": {}
    }));

    let content = serde_json::to_string_pretty(&existing).unwrap() + "\n";
    fs::write(path, &content).expect("Failed to write Zed config");
    println!("Wrote {}", path.display().to_string().cyan());
    println!("{}", "Zed context server configured.".green());
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
    let home = dirs::home_dir().unwrap_or_default();
    let mut found = false;

    let check = |label: &str, path: &Path| -> bool {
        if let Ok(content) = fs::read_to_string(path) {
            if content.contains("savants") {
                println!("  {} {}: {}", "●".green(), label, "configured".green());
                return true;
            }
        }
        false
    };

    // Project configs
    found |= check("Project (.mcp.json)", Path::new(".mcp.json"));
    found |= check("Cursor", Path::new(".cursor/mcp.json"));
    found |= check("Windsurf (project)", Path::new(".windsurf/mcp.json"));
    found |= check("VS Code", Path::new(".vscode/mcp.json"));

    // Global configs
    found |= check("Windsurf (global)", &home.join(".codeium/windsurf/mcp_config.json"));
    found |= check("Continue", &home.join(".continue/config.json"));
    if let Some(config_dir) = dirs::config_dir() {
        found |= check("Zed", &config_dir.join("zed/settings.json"));
    }

    // Claude global (via CLI)
    if find_in_path("claude").is_some() {
        if let Ok(out) = Command::new("claude").args(["mcp", "list"]).output() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.contains("savants") {
                println!("  {} Claude Code (global): {}", "●".green(), "configured".green());
                found = true;
            }
        }
    }

    // Hooks status
    let settings_path = home.join(".claude/settings.json");
    if let Ok(content) = fs::read_to_string(&settings_path) {
        if content.contains("savants") && content.contains("PreToolUse") {
            println!("  {} Hooks (grep/read/bash intercept): {}", "●".green(), "active".green());
        }
    }

    if !found {
        println!("  {} Savants MCP is {}", "●".red(), "not configured".red());
        println!("  Run: {}", "savants mcp install".cyan());
    }
}

/// Verify savants MCP is working end-to-end.
pub fn test() {
    use std::io::Write;

    println!();
    println!("{}", "  Savants MCP Integration Test".bold());
    println!("  {}", "─".repeat(50));
    println!();

    let mut pass = 0u32;
    let mut fail = 0u32;

    let check = |name: &str, ok: bool, detail: &str, pass: &mut u32, fail: &mut u32| {
        if ok {
            *pass += 1;
            println!("  {} {}", "PASS".green(), name);
        } else {
            *fail += 1;
            println!("  {} {} — {}", "FAIL".red(), name, detail);
        }
    };

    // 1. Binary exists
    let bin = find_savants_binary();
    let bin_exists = Path::new(&bin).exists();
    check("Binary exists", bin_exists, &format!("not found at {}", bin), &mut pass, &mut fail);

    // 2. Cloud token configured
    let state = crate::config::State::load();
    let has_token = state.is_cloud_authenticated();
    check("Cloud authenticated", has_token,
        "run 'savants connect cloud'", &mut pass, &mut fail);

    // 3. MCP config exists (any editor)
    let has_config = Path::new(".mcp.json").exists()
        || Path::new(".cursor/mcp.json").exists()
        || Path::new(".windsurf/mcp.json").exists()
        || Path::new(".vscode/mcp.json").exists();
    let home = dirs::home_dir().unwrap_or_default();
    let has_global = home.join(".codeium/windsurf/mcp_config.json").exists()
        || home.join(".continue/config.json").exists();
    let has_claude_global = if find_in_path("claude").is_some() {
        Command::new("claude").args(["mcp", "list"]).output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("savants"))
            .unwrap_or(false)
    } else { false };

    check("MCP config found", has_config || has_global || has_claude_global,
        "run 'savants mcp install'", &mut pass, &mut fail);

    // 4. Server starts and responds
    println!();
    println!("  {}", "Spawning MCP server...".dimmed());
    let server_ok = test_mcp_server(&bin);
    check("MCP server responds to initialize", server_ok,
        "server failed to start or respond", &mut pass, &mut fail);

    // 5. Tool call works
    if has_token {
        println!("  {}", "Testing cloud API...".dimmed());
        let api_ok = test_cloud_api(&state);
        check("Cloud API tool call (graph_stats)", api_ok,
            "cloud API returned error", &mut pass, &mut fail);
    }

    // 6. Hook intercept works
    println!("  {}", "Testing hooks...".dimmed());
    let hook_blocks = test_hook_blocks(&bin);
    check("Hook blocks code grep", hook_blocks,
        "hook did not block code search", &mut pass, &mut fail);

    let hook_allows = test_hook_allows(&bin);
    check("Hook allows non-code grep", hook_allows,
        "hook blocked non-code search", &mut pass, &mut fail);

    // 7. Embedding index exists
    let emb_dir = home.join(".savants/embeddings");
    let has_index = emb_dir.exists() && fs::read_dir(&emb_dir)
        .map(|d| d.count() > 0).unwrap_or(false);
    check("Embedding index exists", has_index,
        "run 'savants reindex' to index your codebase", &mut pass, &mut fail);

    // Summary
    let total = pass + fail;
    println!();
    println!("  {}", "─".repeat(50));
    println!("  {} {} {}", format!("{}/{}", pass, total).bold(),
        "passed".green(),
        if fail == 0 { "— savants is fully operational".green() }
        else { format!("— {} issues to fix", fail).red() });
    println!();

    if fail > 0 {
        std::process::exit(1);
    }
}

fn test_mcp_server(bin: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // Load API key so the server can start in cloud mode
    let state = crate::config::State::load();
    let api_key = state.cloud_token().unwrap_or_default();

    let mut child = match Command::new(bin)
        .arg("serve")
        .env("SAVANTS_API_KEY", &api_key)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    let stdin = child.stdin.as_mut().unwrap();
    let init_msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"savants-test","version":"1.0"}}}"#;
    let _ = writeln!(stdin, "{}", init_msg);
    let _ = stdin.flush();

    // Give it 5s to respond
    std::thread::sleep(std::time::Duration::from_secs(2));
    let _ = child.kill();
    let output = child.wait_with_output().unwrap_or_else(|_| std::process::Output {
        status: std::process::ExitStatus::default(),
        stdout: vec![],
        stderr: vec![],
    });

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.contains("protocolVersion") || stdout.contains("serverInfo")
}

fn test_cloud_api(state: &crate::config::State) -> bool {
    let token = state.cloud_token().unwrap_or_default();
    if token.is_empty() { return false; }

    let output = Command::new("curl")
        .args(["-s", "--max-time", "10", "-X", "POST",
            "-H", &format!("Authorization: Bearer {}", token),
            "-H", "Content-Type: application/json",
            "-d", r#"{"tool":"graph_stats","input":{"repo":"savants"}}"#,
            "https://api.savants.cloud/api/v1/tools/call"])
        .output();

    match output {
        Ok(o) => {
            let body = String::from_utf8_lossy(&o.stdout);
            body.contains("result") && !body.contains("error")
        }
        Err(_) => false,
    }
}

fn test_hook_blocks(bin: &str) -> bool {
    let output = Command::new(bin)
        .args(["hook", "intercept"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                let _ = write!(stdin, r#"{{"tool_name":"Grep","tool_input":{{"pattern":"handleAuth"}}}}"#);
            }
            child.wait_with_output()
        });

    match output {
        Ok(o) => !o.stdout.is_empty(), // blocked = prints message
        Err(_) => false,
    }
}

fn test_hook_allows(bin: &str) -> bool {
    let output = Command::new(bin)
        .args(["hook", "intercept"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                let _ = write!(stdin, r#"{{"tool_name":"Grep","tool_input":{{"pattern":"TODO"}}}}"#);
            }
            child.wait_with_output()
        });

    match output {
        Ok(o) => o.stdout.is_empty(), // allowed = no output
        Err(_) => false,
    }
}

/// Audit all configured MCP servers for security risks.
/// Scans ~/.claude/.mcp.json, .mcp.json, .cursor/mcp.json and flags:
///   - Servers with credential env vars (API keys, tokens, passwords)
///   - Servers with broad file system access
///   - Servers with network access capabilities
pub fn audit() {
    println!();
    println!("{}", "  MCP Security Audit".bold());
    println!("  {}", "─".repeat(55));
    println!("  Ref: DOD/NSA MCP Security Guidance (June 2, 2026)");
    println!("  MCP aggregates credentials — each server is a lateral");
    println!("  movement vector if compromised.");
    println!();

    let home = dirs::home_dir().unwrap_or_default();

    // Collect all MCP config paths to scan
    let config_paths: Vec<(String, PathBuf)> = vec![
        ("Claude (global)".into(), home.join(".claude").join(".mcp.json")),
        ("Claude (project)".into(), home.join(".claude").join("mcp.json")),
        ("Project (.mcp.json)".into(), PathBuf::from(".mcp.json")),
        ("Cursor".into(), PathBuf::from(".cursor/mcp.json")),
        ("Windsurf (project)".into(), PathBuf::from(".windsurf/mcp.json")),
        ("Windsurf (global)".into(), home.join(".codeium").join("windsurf").join("mcp_config.json")),
        ("VS Code".into(), PathBuf::from(".vscode/mcp.json")),
    ];

    let mut total_servers = 0u32;
    let mut credential_servers = 0u32;
    let mut filesystem_servers = 0u32;
    let mut network_servers = 0u32;
    let mut findings: Vec<String> = Vec::new();

    for (label, path) in &config_paths {
        if !path.exists() {
            continue;
        }

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let config: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let servers = config.get("mcpServers")
            .and_then(|s| s.as_object())
            .into_iter()
            .flat_map(|m| m.iter());

        for (name, server_config) in servers {
            total_servers += 1;

            let command = server_config.get("command")
                .and_then(|c| c.as_str())
                .unwrap_or("?");
            let args: Vec<&str> = server_config.get("args")
                .and_then(|a| a.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            println!("  {} {} ({})", "SERVER".cyan().bold(), name.bold(), label);
            println!("    Command: {} {}", command, args.join(" "));

            // Check env vars for credentials
            let mut has_credentials = false;
            let mut credential_names: Vec<String> = Vec::new();
            if let Some(env_obj) = server_config.get("env").and_then(|e| e.as_object()) {
                for (env_key, env_val) in env_obj {
                    let key_upper = env_key.to_uppercase();
                    let is_credential = key_upper.contains("KEY")
                        || key_upper.contains("TOKEN")
                        || key_upper.contains("SECRET")
                        || key_upper.contains("PASSWORD")
                        || key_upper.contains("CREDENTIAL")
                        || key_upper.contains("AUTH");

                    let val_str = env_val.as_str().unwrap_or("");
                    let redacted = if val_str.len() > 8 {
                        format!("{}...{}", &val_str[..4], &val_str[val_str.len()-4..])
                    } else if val_str.is_empty() {
                        "(empty)".to_string()
                    } else {
                        "***".to_string()
                    };

                    if is_credential {
                        has_credentials = true;
                        credential_names.push(env_key.clone());
                        println!("    {} {} = {}", "ENV".yellow(), env_key, redacted);
                    } else {
                        println!("    ENV {} = {}", env_key, val_str);
                    }
                }
            }

            if has_credentials {
                credential_servers += 1;
                findings.push(format!(
                    "{}: credentials in env vars ({}). If this server is compromised, attacker gets these keys.",
                    name, credential_names.join(", ")
                ));
            }

            // Check for file system access indicators
            let has_fs_access = command.contains("filesystem")
                || command.contains("file-server")
                || args.iter().any(|a| a.contains("/") || a.contains("filesystem"))
                || name.to_lowercase().contains("filesystem")
                || name.to_lowercase().contains("file");

            if has_fs_access {
                filesystem_servers += 1;
                findings.push(format!(
                    "{}: has file system access. Can read/write files on your machine.",
                    name
                ));
                println!("    {} File system access detected", "WARN".yellow().bold());
            }

            // Check for network access indicators
            let has_network = command.contains("fetch")
                || command.contains("http")
                || command.contains("curl")
                || command.contains("puppeteer")
                || command.contains("browser")
                || args.iter().any(|a| a.starts_with("http") || a.contains("url"))
                || name.to_lowercase().contains("fetch")
                || name.to_lowercase().contains("browser")
                || name.to_lowercase().contains("web");

            if has_network {
                network_servers += 1;
                findings.push(format!(
                    "{}: has network access. Can make outbound HTTP requests.",
                    name
                ));
                println!("    {} Network access detected", "WARN".yellow().bold());
            }

            // Check for overly broad commands (node, python with no restrictions)
            let is_broad_runtime = (command == "node" || command == "python" || command == "python3")
                && args.is_empty();
            if is_broad_runtime {
                findings.push(format!(
                    "{}: runs bare {} with no script specified. Could execute arbitrary code.",
                    name, command
                ));
                println!("    {} Bare runtime with no script restriction", "WARN".yellow().bold());
            }

            println!();
        }
    }

    // Summary
    println!("  {}", "─".repeat(55));
    println!("  {}", "Summary".bold());
    println!("    Servers scanned:      {}", total_servers);
    println!("    With credential access: {} {}", credential_servers,
        if credential_servers > 0 { "(!)" } else { "" });
    println!("    With file system access: {} {}", filesystem_servers,
        if filesystem_servers > 0 { "(!)" } else { "" });
    println!("    With network access:  {} {}", network_servers,
        if network_servers > 0 { "(!)" } else { "" });

    if !findings.is_empty() {
        println!();
        println!("  {}", "Security Findings".yellow().bold());
        for (i, finding) in findings.iter().enumerate() {
            println!("    {}. {}", i + 1, finding);
        }
    }

    if total_servers == 0 {
        println!();
        println!("  No MCP servers found. Checked:");
        for (label, path) in &config_paths {
            println!("    {} {}", label, path.display().to_string().dimmed());
        }
    }

    // MCP call log status
    let mcp_log = home.join(".savants").join("mcp-calls.jsonl");
    if mcp_log.exists() {
        if let Ok(content) = fs::read_to_string(&mcp_log) {
            let line_count = content.lines().count();
            println!();
            println!("  MCP call log: {} entries (run {} for details)",
                line_count, "savants mcp stats".cyan());
        }
    } else {
        println!();
        println!("  MCP call monitoring: {} (will start logging on next MCP tool use)",
            "active".green());
    }

    println!();
}

/// Show MCP tool call statistics from ~/.savants/mcp-calls.jsonl.
/// Includes: total calls, calls per server, most-used tools, anomaly detection.
pub fn mcp_stats(hours: u64) {
    let log_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("mcp-calls.jsonl");

    if !log_path.exists() {
        println!("{}", "No MCP call data yet.".yellow());
        println!("MCP calls are logged automatically when tools starting with 'mcp__' are used.");
        println!("Use your AI assistant with MCP tools and check back.");
        return;
    }

    let content = match fs::read_to_string(&log_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: could not read MCP call log: {}", "Error".red(), e);
            return;
        }
    };

    let cutoff = chrono::Utc::now() - chrono::Duration::hours(hours as i64);
    let cutoff_str = cutoff.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let mut total_calls = 0u64;
    let mut server_calls: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut tool_calls: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut hourly_counts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();

    for line in content.lines() {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let ts = entry.get("ts").and_then(|v| v.as_str()).unwrap_or("");
        if ts < cutoff_str.as_str() {
            continue;
        }

        total_calls += 1;
        let server = entry.get("mcp_server").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
        let tool = entry.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();

        *server_calls.entry(server).or_insert(0) += 1;
        *tool_calls.entry(tool).or_insert(0) += 1;

        // Extract hour for anomaly detection
        let hour_key = if ts.len() >= 13 { &ts[..13] } else { ts };
        *hourly_counts.entry(hour_key.to_string()).or_insert(0) += 1;
    }

    let period = if hours == 1 { "last hour".to_string() }
        else if hours == 24 { "last 24 hours".to_string() }
        else { format!("last {} hours", hours) };

    println!();
    println!("{}", format!("  MCP Tool Call Stats ({})", period).bold());
    println!("  {}", "─".repeat(55));

    if total_calls == 0 {
        println!("  No MCP calls in the selected period.");
        println!();
        return;
    }

    println!();
    println!("  Total MCP calls: {}", format!("{}", total_calls).green().bold());

    // Per-server breakdown
    println!();
    println!("  {}", "Calls Per Server".bold());
    let mut sorted_servers: Vec<_> = server_calls.iter().collect();
    sorted_servers.sort_by(|a, b| b.1.cmp(a.1));
    for (server, count) in &sorted_servers {
        let bar_len = (**count as f64 / total_calls as f64 * 30.0) as usize;
        let bar: String = std::iter::repeat('#').take(bar_len.max(1)).collect();
        println!("    {:24} {:>5}  {}", server.cyan(), count, bar.dimmed());
    }

    // Most-used tools
    println!();
    println!("  {}", "Top Tools".bold());
    let mut sorted_tools: Vec<_> = tool_calls.iter().collect();
    sorted_tools.sort_by(|a, b| b.1.cmp(a.1));
    for (tool, count) in sorted_tools.iter().take(10) {
        println!("    {:36} {:>5}", tool.cyan(), count);
    }

    // Anomaly detection: flag hours with >2x the average
    if hourly_counts.len() >= 2 {
        let avg = total_calls as f64 / hourly_counts.len() as f64;
        let threshold = avg * 2.0;

        let anomalies: Vec<_> = hourly_counts.iter()
            .filter(|(_, count)| **count as f64 > threshold)
            .collect();

        if !anomalies.is_empty() {
            println!();
            println!("  {}", "Anomalies (>2x average per hour)".yellow().bold());
            println!("    Average: {:.1} calls/hour, threshold: {:.0}", avg, threshold);
            for (hour, count) in &anomalies {
                println!("    {} — {} calls {}", hour, count,
                    format!("({:.1}x average)", **count as f64 / avg).yellow());
            }
        }
    }

    println!();
    println!("  {}", "─".repeat(55));
    println!("  Log file: {}", log_path.display().to_string().dimmed());
    println!();
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

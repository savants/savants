//! Savants hook for Claude Code: intercepts Grep/Read/Bash tool calls
//! and enforces graph-first code intelligence when indexed.
//!
//! GRACEFUL DEGRADATION: If savants is not available, hooks silently
//! allow native tools. The LLM never sees savants errors.
//!
//! INSTRUMENTATION: Every block/allow is logged to ~/.savants/hook-stats.jsonl
//! so `savants stats` can show time/token savings objectively.
//!
//! Exit codes: 0 = allow, 2 = block

use serde_json::{json, Value};
use std::io::Read;

/// Result of a guard rule match — determines how the hook responds.
#[derive(Debug)]
enum GuardAction {
    /// Hard block: exit 2, tool call prevented, LLM stops
    Block(String),
    /// Suggest alternative: exit 0 with deny + reason, LLM auto-recovers
    Suggest(String, String),
    /// Rewrite command: exit 0 with updatedInput, LLM never sees original
    Rewrite(String, String),
    /// Escalate to user: exit 0 with ask, user sees suggestion
    Ask(String, String),
}

/// Main hook entry point. Wrapped in catch-all for graceful degradation.
pub fn intercept() {
    let result = std::panic::catch_unwind(|| {
        intercept_inner();
    });
    if result.is_err() {
        std::process::exit(0);
    }
}

fn intercept_inner() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap_or_default();

    let hook_data: Value = serde_json::from_str(&input).unwrap_or_default();
    let tool_name = hook_data.get("tool_name")
        .or_else(|| hook_data.get("tool"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tool_input = hook_data.get("tool_input")
        .or_else(|| hook_data.get("input"))
        .cloned()
        .unwrap_or_default();

    // ─── Guard bypass: SAVANTS_GUARD=off skips all guard rules ───
    let guard_disabled = std::env::var("SAVANTS_GUARD")
        .map(|v| v == "off" || v == "0" || v == "false")
        .unwrap_or(false);

    // ─── Guard pause: check ~/.savants/guard-paused (with optional expiry) ───
    let guard_paused = is_guard_paused();

    // ─── Container build passthrough: docker/podman BUILD commands are sandboxed ───
    // Build commands (docker build, podman build) execute in isolated build contexts.
    // The Dockerfile may contain rm -rf, apt-get cleanup, etc. that trigger false positives.
    // But docker exec, docker push, kubectl exec ARE dangerous and must be guarded.
    let is_sandboxed_cmd = if tool_name == "Bash" {
        let cmd = tool_input.get("command").and_then(|v| v.as_str()).unwrap_or("");
        cmd.starts_with("docker build") || cmd.starts_with("podman build")
            || cmd.starts_with("docker run --rm") || cmd.starts_with("podman run --rm")
            || cmd.starts_with("claude ") || cmd.starts_with("timeout ") // agent/subprocess launches
    } else {
        false
    };

    // ─── Guard Rules: evaluate BEFORE any other logic ───
    // Load from ~/.savants/guard-rules.json or .savants/guard-rules.json
    if !guard_disabled && !guard_paused && !is_sandboxed_cmd {
        if let Some(action) = check_guard_rules(&tool_name, &tool_input) {
            // ─── Session memory: skip if user already approved this rule ───
            let rule_text = match &action {
                GuardAction::Block(r) | GuardAction::Suggest(r, _)
                | GuardAction::Rewrite(r, _) | GuardAction::Ask(r, _) => r.clone(),
            };
            if is_session_approved(&rule_text) {
                log_event(&tool_name, "allow", "session_approved", &rule_text);
                allow(&tool_name, "session_approved");
                return;
            }

            // ─── Cooloff: if same rule blocked 3+ times in 5 min, escalate to ask ───
            let action = match &action {
                GuardAction::Block(rule) if should_cooloff(rule) => {
                    GuardAction::Ask(
                        rule.clone(),
                        format!("This rule has blocked you multiple times. Approve to proceed this once, or run: savants guard disable '{}'",
                            rule.split("then").next().unwrap_or(rule).trim()),
                    )
                }
                _ => action,
            };

            match action {
                GuardAction::Block(rule) => {
                    block(&tool_name, "guard_rule", &rule,
                        &format!("Savants Guard BLOCKED this action.\n\nTo bypass:\n  savants guard off          # disable until re-enabled\n  savants guard off 10m      # disable for 10 minutes\n  SAVANTS_GUARD=off claude   # disable for one session"));
                    return;
                }
                GuardAction::Suggest(rule, suggestion) => {
                    // Exit 0 with deny + reason — LLM sees the suggestion and auto-recovers
                    log_event(&tool_name, "suggest", "guard_rule", &rule);
                    let reason = format!("Savants Guard: {}", suggestion);
                    let output = json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "deny",
                            "permissionDecisionReason": reason,
                            "additionalContext": suggestion
                        }
                    });
                    println!("{}", output);
                    std::process::exit(0);
                }
                GuardAction::Rewrite(rule, replacement) => {
                    // Exit 0 with updatedInput — silently swap the command
                    log_event(&tool_name, "rewrite", "guard_rule", &rule);
                    let output = json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "defer",
                            "updatedInput": {
                                "command": replacement
                            }
                        }
                    });
                    println!("{}", output);
                    std::process::exit(0);
                }
                GuardAction::Ask(rule, reason) => {
                    // Write pending ask so PostToolUse can detect approval
                    write_pending_ask(&rule);
                    // Exit 0 with ask — escalate to user with context
                    log_event(&tool_name, "ask", "guard_rule", &rule);
                    let ask_reason = format!("Savants Guard: {}", reason);
                    let output = json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "ask",
                            "permissionDecisionReason": ask_reason
                        }
                    });
                    println!("{}", output);
                    std::process::exit(0);
                }
            }
        }
    }

    // ─── MCP Call Monitoring: log all mcp__ tool calls for observability ───
    // This is separate from guard rules — pure auditing per DOD/NSA MCP guidance.
    // Logs to ~/.savants/mcp-calls.jsonl for `savants mcp stats` to analyze.
    if tool_name.starts_with("mcp__") {
        log_mcp_call(&tool_name, &tool_input);
    }

    // ─── Smart Routing: opt-in only ───
    // Redirects grep→semantic_search, read→file_skeleton, etc.
    // Only active if ~/.savants/smart-routing.enabled exists
    // Enable with: savants guard routing on
    let routing_enabled = dirs::home_dir()
        .map(|h| h.join(".savants").join("smart-routing.enabled").exists())
        .unwrap_or(false);

    if !routing_enabled {
        allow(&tool_name, "guard_only");
        return;
    }

    if !savants_is_ready() {
        allow(&tool_name, "degraded");
        return;
    }

    match tool_name.as_str() {
        "Grep" | "grep" => handle_grep_intercept(&tool_name, &tool_input),
        "Read" | "read" => handle_read_intercept(&tool_name, &tool_input),
        "Bash" | "bash" => handle_bash_intercept(&tool_name, &tool_input),
        _ => allow(&tool_name, "unmatched"),
    }
}

/// Check if a rule was already approved this session.
/// Session approvals live in ~/.savants/guard-session.json and expire after 8 hours.
fn is_session_approved(rule: &str) -> bool {
    let session_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("guard-session.json");

    if !session_path.exists() {
        return false;
    }

    let content = match std::fs::read_to_string(&session_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let session: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };

    let cutoff = chrono::Utc::now() - chrono::Duration::hours(8);
    let cutoff_str = cutoff.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    if let Some(approvals) = session.get("approvals").and_then(|v| v.as_array()) {
        for entry in approvals {
            let r = entry.get("rule").and_then(|v| v.as_str()).unwrap_or("");
            let ts = entry.get("ts").and_then(|v| v.as_str()).unwrap_or("");
            if r == rule && ts >= cutoff_str.as_str() {
                return true;
            }
        }
    }

    false
}

/// Record that the user approved a pending ask (called from post_tool).
fn record_session_approval(rule: &str) {
    let home = dirs::home_dir().unwrap_or_default();
    let session_path = home.join(".savants").join("guard-session.json");

    let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut session: Value = if session_path.exists() {
            let content = std::fs::read_to_string(&session_path)?;
            serde_json::from_str(&content).unwrap_or_else(|_| json!({"approvals": []}))
        } else {
            json!({"approvals": []})
        };

        let approvals = session
            .as_object_mut().ok_or("not object")?
            .entry("approvals")
            .or_insert_with(|| json!([]))
            .as_array_mut().ok_or("not array")?;

        // Don't duplicate
        let already = approvals.iter().any(|e|
            e.get("rule").and_then(|v| v.as_str()) == Some(rule)
        );
        if !already {
            approvals.push(json!({
                "rule": rule,
                "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            }));
        }

        // Prune expired entries (older than 8 hours)
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(8);
        let cutoff_str = cutoff.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        approvals.retain(|e| {
            e.get("ts").and_then(|v| v.as_str()).unwrap_or("") >= cutoff_str.as_str()
        });

        std::fs::write(&session_path, serde_json::to_string_pretty(&session)?)?;
        Ok(())
    })();
}

/// Write a pending approval so PostToolUse can detect user approved an ask.
fn write_pending_ask(rule: &str) {
    let pending_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("guard-pending-ask.json");

    let _ = std::fs::write(&pending_path, json!({
        "rule": rule,
        "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }).to_string());
}

/// Check and consume a pending ask (called from post_tool).
/// If there's a pending ask and the tool succeeded, the user approved.
fn consume_pending_ask() -> Option<String> {
    let pending_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("guard-pending-ask.json");

    if !pending_path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&pending_path).ok()?;
    let _ = std::fs::remove_file(&pending_path);

    let pending: Value = serde_json::from_str(&content).ok()?;
    let rule = pending.get("rule")?.as_str()?.to_string();
    let ts = pending.get("ts")?.as_str()?;

    // Only valid if pending was written in last 60 seconds
    if let Ok(pending_time) = chrono::DateTime::parse_from_rfc3339(ts) {
        if chrono::Utc::now().signed_duration_since(pending_time).num_seconds() < 60 {
            return Some(rule);
        }
    }

    None
}

/// Check if guard is paused via ~/.savants/guard-paused
/// File can contain an ISO timestamp for auto-expiry, or be empty (paused indefinitely).
fn is_guard_paused() -> bool {
    let pause_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("guard-paused");

    if !pause_path.exists() {
        return false;
    }

    // Check for expiry timestamp
    if let Ok(content) = std::fs::read_to_string(&pause_path) {
        let content = content.trim();
        if !content.is_empty() {
            // Try to parse as ISO timestamp
            if let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(content) {
                if chrono::Utc::now() > expiry {
                    // Expired — remove pause file and resume
                    let _ = std::fs::remove_file(&pause_path);
                    return false;
                }
            }
        }
    }

    true // Paused (no expiry or not yet expired)
}

/// Check if a rule has blocked too many times recently (3+ in 5 min).
/// If so, the rule should escalate from block → ask (cooloff).
fn should_cooloff(rule: &str) -> bool {
    let log_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("hook-stats.jsonl");

    if !log_path.exists() {
        return false;
    }

    // Read last 50 lines (avoid reading huge files)
    let content = match std::fs::read_to_string(&log_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let cutoff = chrono::Utc::now() - chrono::Duration::minutes(5);
    let cutoff_str = cutoff.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    // Truncate rule detail for matching (log_event truncates to 120 chars)
    let rule_prefix = if rule.len() > 120 { &rule[..120] } else { rule };

    let recent_blocks = content.lines().rev().take(50).filter(|line| {
        // Quick checks before parsing JSON
        line.contains("\"block\"") && line.contains("guard_rule") && {
            if let Ok(entry) = serde_json::from_str::<Value>(line) {
                let ts = entry.get("ts").and_then(|v| v.as_str()).unwrap_or("");
                let detail = entry.get("detail").and_then(|v| v.as_str()).unwrap_or("");
                ts >= cutoff_str.as_str() && detail == rule_prefix
            } else {
                false
            }
        }
    }).count();

    recent_blocks >= 3
}

/// Check guard rules from ~/.savants/guard-rules.json or .savants/guard-rules.json
/// Also syncs from cloud if API key is configured and cache is stale (>30s).
/// Returns Some(GuardAction) if a rule matched, None if allowed.
fn check_guard_rules(tool_name: &str, tool_input: &Value) -> Option<GuardAction> {
    // Background cloud sync: refresh local cache if stale
    maybe_sync_cloud_rules();

    // Try project-local first, then global
    let rules_paths = [
        std::path::PathBuf::from(".savants/guard-rules.json"),
        dirs::home_dir().unwrap_or_default().join(".savants").join("guard-rules.json"),
    ];

    let mut rules: Vec<String> = Vec::new();
    for path in &rules_paths {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(parsed) = serde_json::from_str::<Vec<String>>(&content) {
                    rules.extend(parsed);
                }
                // Also support object format: { "rules": ["when ...", ...] }
                if let Ok(parsed) = serde_json::from_str::<Value>(&content) {
                    if let Some(arr) = parsed.get("rules").and_then(|v| v.as_array()) {
                        for item in arr {
                            if let Some(s) = item.as_str() {
                                rules.push(s.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    if rules.is_empty() {
        return None;
    }

    // Build context from tool name + input
    let mut context = std::collections::HashMap::<String, String>::new();
    context.insert("tool".to_string(), tool_name.to_string());
    context.insert("action".to_string(), tool_name.to_lowercase());

    // Flatten tool_input into context
    if let Some(obj) = tool_input.as_object() {
        for (key, val) in obj {
            match val {
                Value::String(s) => { context.insert(key.clone(), s.clone()); }
                Value::Number(n) => { context.insert(key.clone(), n.to_string()); }
                Value::Bool(b) => { context.insert(key.clone(), b.to_string()); }
                _ => { context.insert(key.clone(), val.to_string()); }
            }
        }
    }

    // Evaluate each rule
    for rule_text in &rules {
        if let Some(action) = evaluate_dsl_rule(rule_text, &context) {
            return Some(action);
        }
    }

    None
}

/// Sync guard rules from cloud if API key is available and cache is stale.
/// Spawns a detached background process so the hook never blocks.
fn maybe_sync_cloud_rules() {
    let home = dirs::home_dir().unwrap_or_default();
    let rules_path = home.join(".savants").join("guard-rules.json");
    let meta_path = home.join(".savants").join("guard-rules.meta");

    // Need an API key to sync
    let api_key = std::env::var("SAVANTS_API_KEY")
        .or_else(|_| {
            let state_path = home.join(".savants").join("state.json");
            std::fs::read_to_string(&state_path)
                .ok()
                .and_then(|s| {
                    serde_json::from_str::<Value>(&s).ok()
                })
                .and_then(|v| v.get("cloud_token").and_then(|t| t.as_str().map(|s| s.to_string())))
                .ok_or(std::env::VarError::NotPresent)
        })
        .ok();

    let api_key = match api_key {
        Some(k) if !k.is_empty() => k,
        _ => return, // No API key, skip cloud sync
    };

    // Check if cache is stale (older than 30 seconds)
    let is_stale = meta_path.exists()
        .then(|| {
            std::fs::metadata(&meta_path)
                .and_then(|m| m.modified())
                .map(|t| t.elapsed().unwrap_or_default().as_secs() > 30)
                .unwrap_or(true)
        })
        .unwrap_or(true); // No meta file = stale

    if !is_stale {
        return; // Cache is fresh, no sync needed
    }

    // Update meta timestamp immediately (prevents concurrent syncs)
    let _ = std::fs::write(&meta_path, chrono::Utc::now().to_rfc3339());

    // Spawn detached background sync (fire and forget, never blocks hook)
    let rules_path_str = rules_path.to_string_lossy().to_string();
    let _ = std::thread::spawn(move || {
        let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()?;

            let resp = client
                .get("https://api.savants.cloud/api/v1/guard/rules")
                .header("Authorization", format!("Bearer {}", api_key))
                .send()?;

            if !resp.status().is_success() {
                return Ok(());
            }

            let data: Value = resp.json()?;
            let rules = data.get("rules").and_then(|r| r.as_array());

            if let Some(rules) = rules {
                let dsl_rules: Vec<String> = rules.iter()
                    .filter_map(|r| r.get("dsl").and_then(|d| d.as_str().map(|s| s.to_string())))
                    .collect();

                if !dsl_rules.is_empty() {
                    // Merge with existing local rules (local first, cloud appended)
                    let mut all_rules: Vec<String> = Vec::new();

                    // Read existing local rules
                    if let Ok(content) = std::fs::read_to_string(&rules_path_str) {
                        if let Ok(local) = serde_json::from_str::<Vec<String>>(&content) {
                            // Keep local rules that aren't from cloud (no way to distinguish yet,
                            // so just replace entirely with cloud rules)
                            all_rules = local;
                        }
                    }

                    // Add cloud rules that aren't already present
                    for rule in &dsl_rules {
                        if !all_rules.contains(rule) {
                            all_rules.push(rule.clone());
                        }
                    }

                    let _ = std::fs::write(
                        &rules_path_str,
                        serde_json::to_string_pretty(&all_rules).unwrap_or_default(),
                    );
                }
            }

            Ok(())
        })();
    });
}

/// Minimal DSL evaluator: "when <field> <op> <value> [and ...] then <action> ['message']"
/// Returns Some(GuardAction) if the rule matches, None if no match.
///
/// Supported actions:
///   block              — hard stop, LLM cannot continue
///   suggest 'message'  — deny with reason, LLM reads suggestion and auto-recovers
///   rewrite 'command'  — silently replace command, LLM never sees original
///   ask 'reason'       — escalate to user with context
fn evaluate_dsl_rule(rule: &str, context: &std::collections::HashMap<String, String>) -> Option<GuardAction> {
    let rule_trimmed = rule.trim();
    if rule_trimmed.is_empty() || rule_trimmed.starts_with('#') || rule_trimmed.starts_with("//") {
        return None;
    }

    // Parse: when <conditions> then <action> [optional 'message']
    // Supports: then block, then suggest 'use X instead', then rewrite 'safe-cmd', then ask 'reason'
    let re = regex::Regex::new(r#"(?i)^when\s+(.+?)\s+then\s+(\w+)(?:\s+'([^']*)')?(?:\s+"([^"]*)")?$"#).ok()?;
    let caps = re.captures(rule_trimmed)?;
    let cond_str = caps.get(1)?.as_str();
    let action = caps.get(2)?.as_str().to_lowercase();
    let message = caps.get(3).or(caps.get(4))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default();

    // Check if this is an actionable rule type
    let actionable = ["block", "block_deploy", "deny", "require_approval", "suggest", "rewrite", "ask"];
    if !actionable.contains(&action.as_str()) {
        return None;
    }

    // Split by "and" / "or"
    let parts: Vec<&str> = cond_str.split(" and ").collect();
    let mut all_match = true;

    for part in &parts {
        let part = part.trim();
        if !evaluate_condition(part, context) {
            all_match = false;
            break;
        }
    }

    if !all_match {
        return None;
    }

    // Build the appropriate action
    match action.as_str() {
        "block" | "block_deploy" | "deny" => Some(GuardAction::Block(rule_trimmed.to_string())),
        "require_approval" | "ask" => Some(GuardAction::Ask(
            rule_trimmed.to_string(),
            if message.is_empty() { "This action requires approval.".to_string() } else { message },
        )),
        "suggest" => Some(GuardAction::Suggest(
            rule_trimmed.to_string(),
            if message.is_empty() { "Consider a safer alternative.".to_string() } else { message },
        )),
        "rewrite" => {
            if message.is_empty() {
                // Can't rewrite without a replacement — fall back to block
                Some(GuardAction::Block(rule_trimmed.to_string()))
            } else {
                Some(GuardAction::Rewrite(rule_trimmed.to_string(), message))
            }
        }
        _ => None,
    }
}

fn evaluate_condition(cond: &str, context: &std::collections::HashMap<String, String>) -> bool {
    let operators = [
        "not_contains", "starts_with", "ends_with",
        "contains", "eq", "neq", "gte", "gt", "lte", "lt",
    ];

    for op in &operators {
        if let Some(idx) = cond.find(&format!(" {} ", op)) {
            let field = cond[..idx].trim();
            let value_str = cond[idx + op.len() + 2..].trim();
            let value = value_str.trim_matches('\'').trim_matches('"');
            let field_val = context.get(field).map(|s| s.as_str()).unwrap_or("");

            return match *op {
                "eq" => field_val == value,
                "neq" => field_val != value,
                "contains" => field_val.to_lowercase().contains(&value.to_lowercase()),
                "not_contains" => !field_val.to_lowercase().contains(&value.to_lowercase()),
                "starts_with" => field_val.starts_with(value),
                "ends_with" => field_val.ends_with(value),
                "gt" => field_val.parse::<f64>().unwrap_or(0.0) > value.parse::<f64>().unwrap_or(0.0),
                "gte" => field_val.parse::<f64>().unwrap_or(0.0) >= value.parse::<f64>().unwrap_or(0.0),
                "lt" => field_val.parse::<f64>().unwrap_or(0.0) < value.parse::<f64>().unwrap_or(0.0),
                "lte" => field_val.parse::<f64>().unwrap_or(0.0) <= value.parse::<f64>().unwrap_or(0.0),
                _ => false,
            };
        }
    }
    false
}

fn savants_is_ready() -> bool {
    if has_graph_index() {
        return true;
    }
    let home = dirs::home_dir().unwrap_or_default();
    let config_path = home.join(".savants").join("config.json");
    if config_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if content.contains("api_key") || content.contains("SAVANTS_API_KEY") {
                return true;
            }
        }
    }
    if std::env::var("SAVANTS_API_KEY").is_ok() {
        return true;
    }
    false
}

fn handle_grep_intercept(tool: &str, input: &Value) {
    let pattern = input.get("pattern")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if pattern.is_empty() {
        allow(tool, "empty_pattern");
        return;
    }

    let search_path = input.get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let glob_filter = input.get("glob")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let non_code_extensions = [".md", ".txt", ".log", ".json", ".yaml", ".yml",
        ".toml", ".ini", ".cfg", ".conf", ".env", ".lock", ".csv", ".xml",
        ".html", ".css", ".scss", ".svg"];
    let searching_non_code = non_code_extensions.iter()
        .any(|ext| glob_filter.ends_with(ext) || search_path.ends_with(ext));

    let upper = pattern.to_uppercase();
    let is_comment_search = upper.starts_with("TODO")
        || upper.starts_with("FIXME")
        || upper.starts_with("HACK")
        || upper.starts_with("XXX");

    if searching_non_code || is_comment_search {
        allow(tool, "non_code");
        return;
    }

    block(tool, "code_search", pattern,
        "STOP: Use mcp__savants__search_code, mcp__savants__advanced_graph_query, \
        or mcp__savants__function_xray instead of grep/find/rg. \
        Savants has the codebase indexed and can answer this query structurally. \
        Only use Bash grep/find for non-code tasks like checking process status or log files."
    );
}

fn handle_read_intercept(tool: &str, input: &Value) {
    let file_path = input.get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if file_path.is_empty() {
        allow(tool, "empty_path");
        return;
    }

    let source_extensions = [".ts", ".tsx", ".js", ".jsx", ".py", ".rs", ".go",
        ".java", ".kt", ".rb", ".php", ".swift", ".c", ".cpp", ".h", ".cs"];
    let is_source = source_extensions.iter().any(|ext| file_path.ends_with(ext));

    if !is_source {
        allow(tool, "non_code");
        return;
    }

    let has_limit = input.get("limit").is_some() || input.get("offset").is_some();
    if has_limit {
        allow(tool, "targeted_read");
        return;
    }

    let file_name = file_path.rsplit('/').next().unwrap_or(file_path);
    block(tool, "full_file_read", file_name,
        &format!(
            "STOP: Use `mcp__savants__file_skeleton` with file=\"{}\" first to see the structure \
            (function names + line numbers) without reading the entire file. \
            Then Read only the specific function you need with offset/limit.",
            file_name
        )
    );
}

fn handle_bash_intercept(tool: &str, input: &Value) {
    let command = input.get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if command.is_empty() {
        allow(tool, "empty_command");
        return;
    }

    let is_code_search = (command.starts_with("grep ") || command.starts_with("rg ")
        || command.starts_with("ag ") || command.contains("| grep")
        || command.contains("| rg"))
        && !command.contains(".log") && !command.contains(".json")
        && !command.contains("ps ") && !command.contains("docker ")
        && !command.contains("kubectl ");

    if is_code_search {
        block(tool, "bash_code_search", command,
            "STOP: Use mcp__savants__search_code, mcp__savants__advanced_graph_query, \
            or mcp__savants__function_xray instead of grep/find/rg. \
            Savants has the codebase indexed and can answer this query structurally. \
            Only use Bash grep/find for non-code tasks like checking process status or log files."
        );
        return;
    }

    // K8s secret value exposure detection — redirect to safe alternative
    let cmd_lower = command.to_lowercase();
    let is_secret_value_exposure =
        cmd_lower.contains("secret") && (
            cmd_lower.contains("-o yaml") ||
            cmd_lower.contains("-o json") ||
            cmd_lower.contains("-o jsonpath") ||
            cmd_lower.contains("-o go-template") ||
            cmd_lower.contains("describe secret")
        );

    if is_secret_value_exposure
        && !cmd_lower.contains("savants-redact")
        && !cmd_lower.contains("redacted")
        && !command.contains("sed ")
    {
        // Extract secret name for the safe alternative
        let parts: Vec<&str> = command.split_whitespace().collect();
        let secret_name = parts.iter()
            .position(|&p| p == "secret")
            .and_then(|i| parts.get(i + 1))
            .map(|s| s.to_string())
            .unwrap_or_else(|| "<name>".to_string());

        let namespace = parts.iter()
            .position(|&p| p == "-n")
            .and_then(|i| parts.get(i + 1))
            .map(|s| s.to_string());

        let ns_flag = namespace.map(|n| format!(" -n {}", n)).unwrap_or_default();

        block(tool, "secret_exposure", command,
            &format!(
                "BLOCKED: This command would expose raw secret values. \
                Use the redacted version instead:\n\
                kubectl get secret {}{} -o yaml | sed 's/^\\(  [a-zA-Z_-]*:\\) .*/\\1 ***REDACTED***/' \n\
                Or list secret names only (safe):\n\
                kubectl get secrets{}\n\
                This policy ensures secret values are never fully visible in your session.",
                secret_name, ns_flag, ns_flag
            )
        );
        return;
    }

    allow(tool, "non_code_bash");
}

fn has_graph_index() -> bool {
    let home = dirs::home_dir().unwrap_or_default();
    let cache_dir = home.join(".savants").join("embeddings");
    cache_dir.exists() && std::fs::read_dir(&cache_dir)
        .map(|d| d.count() > 0)
        .unwrap_or(false)
}

/// Log an event to ~/.savants/hook-stats.jsonl, then exit.
fn log_event(tool: &str, action: &str, reason: &str, detail: &str) {
    let log_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("hook-stats.jsonl");

    // Best-effort logging — never let this fail the hook
    let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
        let entry = serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "tool": tool,
            "action": action,
            "reason": reason,
            "detail": if detail.len() > 120 { &detail[..120] } else { detail },
        });
        let mut file = std::fs::OpenOptions::new()
            .create(true).append(true).open(&log_path)?;
        use std::io::Write;
        writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        Ok(())
    })();
}

fn allow(tool: &str, reason: &str) {
    log_event(tool, "allow", reason, "");
    std::process::exit(0);
}

fn block(tool: &str, reason: &str, detail: &str, message: &str) {
    log_event(tool, "block", reason, detail);
    eprintln!("{}", message);
    std::process::exit(2);
}

// ─── Post-tool hook ─────────────────────────────────────────────────

pub fn post_tool() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap_or_default();

    let hook_data: Value = serde_json::from_str(&input).unwrap_or_default();
    let tool_name = hook_data.get("tool_name")
        .or_else(|| hook_data.get("tool"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_input = hook_data.get("tool_input")
        .or_else(|| hook_data.get("input"))
        .cloned()
        .unwrap_or_default();

    // ─── Session memory: if there was a pending ask and the tool succeeded,
    // the user approved. Record the approval for the rest of the session. ───
    if let Some(approved_rule) = consume_pending_ask() {
        record_session_approval(&approved_rule);
        log_event(tool_name, "session_approve", "guard_rule", &approved_rule);
    }

    match tool_name {
        "Edit" | "edit" | "Write" | "write" => handle_post_edit(&tool_input),
        "Bash" | "bash" => handle_post_bash(&tool_input),
        _ => {}
    }

    std::process::exit(0);
}

fn handle_post_edit(input: &Value) {
    let file_path = input.get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if file_path.is_empty() { return; }

    let is_source = file_path.ends_with(".ts") || file_path.ends_with(".tsx")
        || file_path.ends_with(".js") || file_path.ends_with(".py")
        || file_path.ends_with(".rs") || file_path.ends_with(".go");
    if !is_source || !savants_is_ready() { return; }

    let old_string = input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
    let new_string = input.get("new_string").and_then(|v| v.as_str()).unwrap_or("");

    let mut functions_changed: Vec<String> = Vec::new();
    for text in &[old_string, new_string] {
        for word in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if word.len() > 5
                && word.chars().any(|c| c.is_lowercase())
                && word.chars().any(|c| c.is_uppercase())
                && !functions_changed.contains(&word.to_string())
            {
                functions_changed.push(word.to_string());
            }
        }
    }

    if !functions_changed.is_empty() {
        let file_name = file_path.rsplit('/').next().unwrap_or(file_path);
        println!(
            "Savants: You edited {} which contains {}. \
            Use `mcp__savants__blast_radius` to check what's affected, \
            or `mcp__savants__callers` to see what depends on these functions.",
            file_name,
            functions_changed.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
        );
    }
}

fn handle_post_bash(input: &Value) {
    let command = input.get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if command.contains("git commit") || command.contains("git push") {
        if savants_is_ready() {
            println!(
                "Savants: Code was committed. Run `mcp__savants__reindex` to update the graph \
                with the latest changes so search and callers stay accurate."
            );
        }
    }

    if command.contains("kubectl apply") || command.contains("helm upgrade")
        || command.contains("docker push") || command.contains("wrangler deploy")
        || command.contains("argocd sync")
    {
        println!(
            "Savants: Deploy detected. Use `mcp__savants__diagnose` after deploy to check \
            for new errors, or `mcp__savants__network_report` to verify connectivity."
        );
    }
}

/// Log an MCP tool call to ~/.savants/mcp-calls.jsonl for security monitoring.
/// Extracts the server name from the tool_name (mcp__<server>__<tool>) and
/// hashes the arguments to avoid logging sensitive data.
fn log_mcp_call(tool_name: &str, tool_input: &Value) {
    let mcp_log_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("mcp-calls.jsonl");

    let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
        // Ensure ~/.savants/ exists
        if let Some(parent) = mcp_log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Parse server and tool from mcp__<server>__<tool_name>
        let parts: Vec<&str> = tool_name.splitn(3, "__").collect();
        let mcp_server = if parts.len() >= 2 { parts[1] } else { "unknown" };
        let mcp_tool = if parts.len() >= 3 { parts[2] } else { tool_name };

        // Hash arguments to avoid logging sensitive values
        use sha2::{Sha256, Digest};
        let args_str = serde_json::to_string(tool_input).unwrap_or_default();
        let args_hash = format!("{:x}", Sha256::digest(args_str.as_bytes()));
        let args_hash_short = &args_hash[..16];

        let entry = serde_json::json!({
            "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "mcp_server": mcp_server,
            "tool_name": format!("mcp__{}_{}", mcp_server, mcp_tool),
            "arguments_hash": args_hash_short,
        });

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&mcp_log_path)?;
        use std::io::Write;
        writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        Ok(())
    })();
}

/// Track what tools are used and what questions are asked.
pub fn log_usage(tool: &str, query: &str) {
    let log_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("usage.jsonl");

    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        use std::io::Write;
        let entry = serde_json::json!({
            "ts": chrono::Utc::now().timestamp(),
            "tool": tool,
            "query": if query.len() > 200 { &query[..200] } else { query },
        });
        let _ = writeln!(file, "{}", serde_json::to_string(&entry).unwrap_or_default());
    }
}

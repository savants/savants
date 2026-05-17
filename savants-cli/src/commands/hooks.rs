//! Savants hook for Claude Code: intercepts Grep/Read tool calls
//! and suggests graph tools when the graph can answer the question.
//!
//! Called by Claude Code's PreToolUse hook system.
//! Reads tool name and input from stdin (JSON), writes decision to stdout.
//!
//! Exit codes:
//!   0 = allow the tool call to proceed
//!   2 = block the tool call (Claude will see the stdout message)

use serde_json::Value;
use std::io::Read;

/// Check if savants graph can answer this query better than grep/read.
/// Called by Claude Code hook system before Grep or Read tool calls.
pub fn intercept() {
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

    match tool_name {
        "Grep" | "grep" => handle_grep_intercept(&tool_input),
        "Read" | "read" => handle_read_intercept(&tool_input),
        _ => allow(),
    }
}

fn handle_grep_intercept(input: &Value) {
    let pattern = input.get("pattern")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Extract potential function/symbol names from the grep pattern
    // If it looks like searching for a function, suggest callers/where_used
    if pattern.is_empty() {
        allow();
        return;
    }

    // Check if this looks like a function name (camelCase or snake_case, not ALL_CAPS)
    let is_all_upper = pattern.chars().all(|c| c.is_uppercase() || c == '_');
    let has_mixed_case = pattern.chars().any(|c| c.is_lowercase()) && pattern.chars().any(|c| c.is_uppercase());
    let has_underscore = pattern.contains('_') && !is_all_upper;
    let is_func_search = pattern.chars().all(|c| c.is_alphanumeric() || c == '_')
        && pattern.len() > 5
        && !is_all_upper
        && (has_underscore || has_mixed_case);

    if !is_func_search {
        // Regex/exact string search - let grep handle it
        allow();
        return;
    }

    // Check if we have a graph index
    if !has_graph_index() {
        allow();
        return;
    }

    // This looks like a function search and we have a graph - suggest savants
    block(&format!(
        "Savants has this in its graph. Use `mcp__savants__where_used` with symbol=\"{}\" instead of grep. \
        If you need the full caller chain, use `mcp__savants__callers` with function=\"{}\".",
        pattern, pattern
    ));
}

fn handle_read_intercept(input: &Value) {
    let file_path = input.get("file_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if file_path.is_empty() {
        allow();
        return;
    }

    // Only intercept source code files (not configs, docs, etc)
    let is_source = file_path.ends_with(".ts") || file_path.ends_with(".tsx")
        || file_path.ends_with(".js") || file_path.ends_with(".py")
        || file_path.ends_with(".rs") || file_path.ends_with(".go")
        || file_path.ends_with(".java");

    if !is_source {
        allow();
        return;
    }

    // Check if reading a large file (no limit specified = reading full file)
    let has_limit = input.get("limit").is_some() || input.get("offset").is_some();
    if has_limit {
        // Targeted read (specific lines) - allow it
        allow();
        return;
    }

    // Full file read of source code - suggest file_skeleton first
    if has_graph_index() {
        let file_name = file_path.rsplit('/').next().unwrap_or(file_path);
        block(&format!(
            "Use `mcp__savants__file_skeleton` with file=\"{}\" first to see the structure (function names + line numbers) \
            without reading the entire file. Then Read only the specific function you need.",
            file_name
        ));
    } else {
        allow();
    }
}

fn has_graph_index() -> bool {
    // Check if any embedding cache exists (means savants has indexed something)
    let home = dirs::home_dir().unwrap_or_default();
    let cache_dir = home.join(".savants").join("embeddings");
    cache_dir.exists() && std::fs::read_dir(&cache_dir)
        .map(|d| d.count() > 0)
        .unwrap_or(false)
}

fn allow() {
    // Exit 0 = allow the tool call
    std::process::exit(0);
}

fn block(message: &str) {
    // Print the suggestion and exit 2 = block
    println!("{}", message);
    std::process::exit(2);
}

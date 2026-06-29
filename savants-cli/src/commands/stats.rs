//! `savants stats` — show how much time and tokens savants saves.
//!
//! Reads from:
//!   ~/.savants/hook-stats.jsonl  (hook block/allow events)
//!   ~/.savants/tool-stats.jsonl  (cloud tool call metrics)

use colored::*;
use serde_json::Value;
use std::collections::HashMap;

/// Model pricing per 1M input tokens (USD)
const MODEL_COSTS: &[(&str, f64)] = &[
    ("Claude Opus 4",    15.00),
    ("Claude Sonnet 4",   3.00),
    ("Claude Haiku 3.5",  0.80),
    ("GPT-4o",            2.50),
    ("GPT-4.1",           2.00),
    ("Gemini 2.5 Pro",    1.25),
];

struct HookEvent {
    tool: String,
    action: String, // "block" or "allow"
    reason: String,
}

struct ToolEvent {
    tool: String,
    duration_ms: u64,
    tokens: usize,
    ok: bool,
}

fn parse_hook_stats(days: i64) -> Vec<HookEvent> {
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("hook-stats.jsonl");

    let cutoff = chrono::Utc::now() - chrono::Duration::days(days);

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    content.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| {
            v.get("ts").and_then(|t| t.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt >= cutoff)
                .unwrap_or(false)
        })
        .map(|v| HookEvent {
            tool: v.get("tool").and_then(|t| t.as_str()).unwrap_or("").to_string(),
            action: v.get("action").and_then(|a| a.as_str()).unwrap_or("").to_string(),
            reason: v.get("reason").and_then(|r| r.as_str()).unwrap_or("").to_string(),
        })
        .collect()
}

fn parse_tool_stats(days: i64) -> Vec<ToolEvent> {
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join(".savants")
        .join("tool-stats.jsonl");

    let cutoff = chrono::Utc::now() - chrono::Duration::days(days);

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    content.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| {
            v.get("ts").and_then(|t| t.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt >= cutoff)
                .unwrap_or(false)
        })
        .map(|v| ToolEvent {
            tool: v.get("tool").and_then(|t| t.as_str()).unwrap_or("").to_string(),
            duration_ms: v.get("duration_ms").and_then(|d| d.as_u64()).unwrap_or(0),
            tokens: v.get("tokens").and_then(|t| t.as_u64()).unwrap_or(0) as usize,
            ok: v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false),
        })
        .collect()
}

pub fn run(days: i64) {
    let hook_events = parse_hook_stats(days);
    let tool_events = parse_tool_stats(days);

    if hook_events.is_empty() && tool_events.is_empty() {
        println!("{}", "No usage data yet.".yellow());
        println!("Savants collects stats as you use it via Claude Code.");
        println!("Start coding and check back later.");
        return;
    }

    let period = if days == 1 { "today".to_string() }
        else if days == 7 { "last 7 days".to_string() }
        else { format!("last {} days", days) };

    println!();
    println!("{}", format!("  Savants Usage Stats ({})", period).bold());
    println!("  {}", "─".repeat(50));

    // ─── Hook stats ─────────────────────────────────────────────
    let blocks: Vec<&HookEvent> = hook_events.iter().filter(|e| e.action == "block").collect();
    let allows: Vec<&HookEvent> = hook_events.iter().filter(|e| e.action == "allow").collect();
    let degraded: Vec<&HookEvent> = hook_events.iter().filter(|e| e.reason == "degraded").collect();

    // Count blocks by reason
    let grep_blocks = blocks.iter().filter(|e| e.reason == "code_search").count();
    let read_blocks = blocks.iter().filter(|e| e.reason == "full_file_read").count();
    let bash_blocks = blocks.iter().filter(|e| e.reason == "bash_code_search").count();

    println!();
    println!("  {}", "Interceptions".bold());
    println!("    Searches redirected:  {} {}", format!("{}", grep_blocks + bash_blocks).green().bold(),
        "(grep/rg → savants search)".dimmed());
    println!("    File reads avoided:   {} {}", format!("{}", read_blocks).green().bold(),
        "(full read → file_skeleton)".dimmed());
    println!("    Total blocks:         {}", blocks.len());
    println!("    Total allows:         {} {}", allows.len(),
        format!("(non-code, TODO, targeted reads)").dimmed());

    if !degraded.is_empty() {
        println!("    Graceful fallbacks:   {} {}", degraded.len(),
            "(savants unavailable → native tools)".dimmed());
    }

    // ─── Token savings estimate ─────────────────────────────────
    // Average source file: ~400 lines × 40 chars = ~16K chars = ~4K tokens
    // file_skeleton: ~50 lines = ~200 chars = ~50 tokens
    // Savings per read avoided: ~3,950 tokens
    let tokens_saved_from_reads = read_blocks * 3_950;

    // Average grep search: returns ~20 matches × 80 chars = ~1,600 chars = ~400 tokens
    // savants search_code: returns structured result = ~200 tokens
    // But the real savings is: grep often needs 3-5 iterations to find what you need
    // savants finds it in 1 call. So savings = ~3 grep calls × 400 tokens = ~1,200 tokens
    let tokens_saved_from_greps = (grep_blocks + bash_blocks) * 1_200;

    let total_tokens_saved = tokens_saved_from_reads + tokens_saved_from_greps;

    // ─── Tool stats ─────────────────────────────────────────────
    let successful: Vec<&ToolEvent> = tool_events.iter().filter(|e| e.ok).collect();
    let total_tool_calls = tool_events.len();
    let total_tool_tokens: usize = successful.iter().map(|e| e.tokens).sum();
    let avg_response_ms = if successful.is_empty() { 0 } else {
        successful.iter().map(|e| e.duration_ms).sum::<u64>() / successful.len() as u64
    };

    // Tool call breakdown
    let mut tool_counts: HashMap<String, usize> = HashMap::new();
    let mut tool_durations: HashMap<String, Vec<u64>> = HashMap::new();
    for e in &successful {
        *tool_counts.entry(e.tool.clone()).or_insert(0) += 1;
        tool_durations.entry(e.tool.clone()).or_insert_with(Vec::new).push(e.duration_ms);
    }

    if total_tool_calls > 0 {
        println!();
        println!("  {}", "Tool Calls".bold());
        println!("    Total calls:          {}", total_tool_calls);
        println!("    Successful:           {} ({}%)", successful.len(),
            if total_tool_calls > 0 { successful.len() * 100 / total_tool_calls } else { 0 });
        println!("    Avg response time:    {}", format!("{}ms", avg_response_ms).cyan());
        println!("    Tokens returned:      {}", format_tokens(total_tool_tokens));

        // Top tools
        let mut sorted_tools: Vec<_> = tool_counts.iter().collect();
        sorted_tools.sort_by(|a, b| b.1.cmp(a.1));

        println!();
        println!("  {}", "Top Tools".bold());
        for (tool, count) in sorted_tools.iter().take(8) {
            let avg_ms = tool_durations.get(*tool)
                .map(|d| d.iter().sum::<u64>() / d.len() as u64)
                .unwrap_or(0);
            println!("    {:24} {:>4} calls  {:>5}ms avg", tool.cyan(), count, avg_ms);
        }
    }

    // ─── Savings summary ────────────────────────────────────────
    println!();
    println!("  {}", "Estimated Savings".bold());
    println!("    Tokens saved:         {} {}", format!("~{}", format_tokens(total_tokens_saved)).green().bold(),
        "(from redirected searches + avoided reads)".dimmed());

    // Time saved: each grep iteration takes ~2-5s for the LLM to process results
    // savants returns in <500ms with structured data = saves ~3s per search
    let time_saved_secs = (grep_blocks + bash_blocks) * 3 + read_blocks * 4;
    if time_saved_secs > 0 {
        let time_str = if time_saved_secs >= 60 {
            format!("~{}m {}s", time_saved_secs / 60, time_saved_secs % 60)
        } else {
            format!("~{}s", time_saved_secs)
        };
        println!("    Est. time saved:      {} {}", time_str.green().bold(),
            "(faster structured responses vs iterative grep)".dimmed());
    }

    // Cost savings per model
    if total_tokens_saved > 0 {
        println!();
        println!("  {}", "Cost Saved Per Model".bold());
        print_cost_table(total_tokens_saved);
    }

    println!("  {}", "─".repeat(50));
    println!();
}

fn format_tokens(n: usize) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1_000_000.0) }
    else if n >= 1_000 { format!("{:.1}K", n as f64 / 1_000.0) }
    else { format!("{}", n) }
}

fn print_cost_table(tokens_saved: usize) {
    println!("    {:<24} {:>12}", "Model".dimmed(), "Saved".dimmed());
    println!("    {}", "─".repeat(38));
    for (model, cost_per_m) in MODEL_COSTS {
        let saved = tokens_saved as f64 * cost_per_m / 1_000_000.0;
        let saved_str = if saved >= 0.01 {
            format!("${:.2}", saved)
        } else {
            format!("${:.4}", saved)
        };
        println!("    {:<24} {:>12}", model, saved_str.green());
    }
}

/// Live benchmark: run the same query with grep vs savants side by side.
/// Shows speed AND token comparison.
pub fn benchmark() {
    use std::process::Command;
    use std::time::Instant;

    let repo_path = std::env::current_dir().unwrap_or_default();
    let repo_name = repo_path.file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    println!();
    println!("{}", "  Savants vs Native Tools — Live Benchmark".bold());
    println!("  {}", "─".repeat(55));
    println!("  Repo: {} ({})", repo_name.cyan(), repo_path.display());
    println!();

    // ─── Test 1: Search for a function ───────────────────────────
    let search_term = "handle";

    println!("  {}", format!("Test 1: Search for '{}'", search_term).bold());
    println!();

    // grep
    let t = Instant::now();
    let grep_out = Command::new("grep")
        .args(["-rn", search_term, "--include=*.ts", "--include=*.rs",
               "--include=*.py", "--include=*.go", "--include=*.js"])
        .current_dir(&repo_path)
        .output();
    let grep_ms = t.elapsed().as_millis() as u64;
    let grep_bytes = grep_out.as_ref().map(|o| o.stdout.len()).unwrap_or(0);
    let grep_lines = grep_out.as_ref()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count())
        .unwrap_or(0);
    let grep_tokens = grep_bytes / 4;

    // ripgrep (if available)
    let rg_result = Command::new("rg")
        .args(["-n", search_term, "--type=ts", "--type=rust", "--type=py", "--type=go", "--type=js"])
        .current_dir(&repo_path)
        .output();
    let (rg_ms, rg_tokens) = if let Ok(ref out) = rg_result {
        let t2 = Instant::now();
        let _ = Command::new("rg")
            .args(["-n", search_term, "--type=ts", "--type=rust", "--type=py", "--type=go", "--type=js"])
            .current_dir(&repo_path)
            .output();
        let ms = t2.elapsed().as_millis() as u64;
        (ms, out.stdout.len() / 4)
    } else {
        (0, 0)
    };

    // savants search_code
    let t = Instant::now();
    let savants_out = Command::new("savants")
        .args(["--version"]) // warm up
        .output();
    let _ = savants_out;

    // Use the embedding store for local search
    let savants_result = if crate::embedding_store::EmbeddingStore::exists(&repo_name) {
        let t = Instant::now();
        let store = crate::embedding_store::EmbeddingStore::load(&repo_name).ok();
        let result = store.as_ref().map(|s| {
            // Do a text-based search in the store entries
            let matches: Vec<_> = s.entries.iter()
                .filter(|e| e.name.to_lowercase().contains(&search_term.to_lowercase())
                    || e.file.to_lowercase().contains(&search_term.to_lowercase()))
                .take(20)
                .collect();
            let text: String = matches.iter()
                .map(|e| format!("{}:{} {}()", e.file, e.line, e.name))
                .collect::<Vec<_>>()
                .join("\n");
            (text.len(), matches.len(), text.len() / 4)
        }).unwrap_or((0, 0, 0));
        let ms = t.elapsed().as_millis() as u64;
        Some((ms, result.1, result.2))
    } else {
        None
    };

    // Print comparison
    println!("    {:<20} {:>8}  {:>10}  {:>8}", "Method".dimmed(), "Time".dimmed(), "Output".dimmed(), "Tokens".dimmed());
    println!("    {}", "─".repeat(50));
    println!("    {:<20} {:>6}ms  {:>7} lines  {:>6}",
        "grep", grep_ms, grep_lines, format_tokens(grep_tokens));

    if rg_ms > 0 {
        println!("    {:<20} {:>6}ms  {:>7} lines  {:>6}",
            "ripgrep", rg_ms, grep_lines, format_tokens(rg_tokens));
    }

    if let Some((ms, count, tokens)) = savants_result {
        println!("    {:<20} {:>6}ms  {:>7} results {:>6}",
            "savants".green(), ms, count, format_tokens(tokens));

        let speedup = if ms > 0 { grep_ms as f64 / ms as f64 } else { 0.0 };
        let token_savings = if grep_tokens > tokens { grep_tokens - tokens } else { 0 };
        let token_pct = if grep_tokens > 0 { token_savings * 100 / grep_tokens } else { 0 };

        println!();
        if speedup > 1.0 {
            println!("    {} {}", format!("{:.1}x faster", speedup).green().bold(),
                "than grep".dimmed());
        }
        if token_savings > 0 {
            println!("    {} {} {}", format!("{}% fewer tokens", token_pct).green().bold(),
                format!("({} saved)", format_tokens(token_savings)).dimmed(),
                "— structured results vs raw text".dimmed());
        }
    } else {
        println!("    {:<20} {:>8}  (run 'savants reindex' first)", "savants".yellow(), "—");
    }

    // ─── Test 2: Read a file ─────────────────────────────────────
    println!();

    // Find a source file to benchmark
    let test_file = find_source_file(&repo_path);
    if let Some(ref file) = test_file {
        let file_name = file.file_name().unwrap_or_default().to_string_lossy();
        let rel_path = file.strip_prefix(&repo_path).unwrap_or(file);

        println!("  {}", format!("Test 2: Read '{}'", file_name).bold());
        println!();

        // Full file read (what cat/Read does)
        let t = Instant::now();
        let file_content = std::fs::read_to_string(file).unwrap_or_default();
        let read_ms = t.elapsed().as_millis() as u64;
        let read_lines = file_content.lines().count();
        let read_tokens = file_content.len() / 4;

        // savants file_skeleton equivalent (just function names + lines)
        let skeleton = if crate::embedding_store::EmbeddingStore::exists(&repo_name) {
            let t = Instant::now();
            let store = crate::embedding_store::EmbeddingStore::load(&repo_name).ok();
            let result = store.as_ref().map(|s| {
                let rel_str = rel_path.to_string_lossy();
                let matches: Vec<_> = s.entries.iter()
                    .filter(|e| e.file == rel_str.as_ref() || e.file.ends_with(&*file_name))
                    .collect();
                let text: String = matches.iter()
                    .map(|e| format!("  L{}-{} {}()", e.line, e.line + 10, e.name))
                    .collect::<Vec<_>>()
                    .join("\n");
                (matches.len(), text.len() / 4)
            }).unwrap_or((0, 0));
            let ms = t.elapsed().as_millis() as u64;
            Some((ms, result.0, result.1))
        } else {
            None
        };

        println!("    {:<20} {:>8}  {:>10}  {:>8}", "Method".dimmed(), "Time".dimmed(), "Output".dimmed(), "Tokens".dimmed());
        println!("    {}", "─".repeat(50));
        println!("    {:<20} {:>6}ms  {:>7} lines  {:>6}",
            "Read (full file)", read_ms, read_lines, format_tokens(read_tokens));

        if let Some((ms, funcs, tokens)) = skeleton {
            println!("    {:<20} {:>6}ms  {:>7} funcs  {:>6}",
                "file_skeleton".green(), ms, funcs, format_tokens(tokens));

            let token_savings = if read_tokens > tokens { read_tokens - tokens } else { 0 };
            let token_pct = if read_tokens > 0 { token_savings * 100 / read_tokens } else { 0 };
            let ratio = if tokens > 0 { read_tokens / tokens } else { 0 };

            println!();
            if ratio > 1 {
                println!("    {} {}", format!("{}x fewer tokens", ratio).green().bold(),
                    "— function signatures only, no bodies".dimmed());
            }
            if token_savings > 0 {
                println!("    {} {}", format!("{}% reduction", token_pct).green().bold(),
                    format!("({} tokens saved per read)", format_tokens(token_savings)).dimmed());
            }
        } else {
            println!("    {:<20} {:>8}  (run 'savants reindex' first)", "file_skeleton".yellow(), "—");
        }
    }

    // ─── Cost comparison ──────────────────────────────────────────
    // Calculate total token savings from both tests
    let total_native_tokens = grep_tokens + test_file.as_ref()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|c| c.len() / 4)
        .unwrap_or(0);

    let total_savants_tokens = savants_result.map(|r| r.2).unwrap_or(grep_tokens)
        + test_file.as_ref().map(|_| {
            // skeleton tokens from test 2
            if crate::embedding_store::EmbeddingStore::exists(&repo_name) {
                50 // approximate skeleton token count
            } else {
                0
            }
        }).unwrap_or(0);

    let token_diff = if total_native_tokens > total_savants_tokens {
        total_native_tokens - total_savants_tokens
    } else { 0 };

    if token_diff > 0 {
        println!();
        println!("  {}", "Cost Per Query (native vs savants)".bold());
        println!("    {:<24} {:>10} {:>10} {:>10}", "Model".dimmed(),
            "Native".dimmed(), "Savants".dimmed(), "Saved".dimmed());
        println!("    {}", "─".repeat(56));
        for (model, cost_per_m) in MODEL_COSTS {
            let native_cost = total_native_tokens as f64 * cost_per_m / 1_000_000.0;
            let savants_cost = total_savants_tokens as f64 * cost_per_m / 1_000_000.0;
            let saved = native_cost - savants_cost;
            println!("    {:<24} {:>9} {:>9} {:>10}",
                model,
                format!("${:.4}", native_cost),
                format!("${:.4}", savants_cost),
                format!("${:.4}", saved).green());
        }
        println!();
        println!("    {} {}", "At scale:".bold(),
            format!("100 queries/day = ${:.2}-${:.2}/day saved depending on model",
                token_diff as f64 * MODEL_COSTS.last().unwrap().1 / 1_000_000.0 * 100.0,
                token_diff as f64 * MODEL_COSTS.first().unwrap().1 / 1_000_000.0 * 100.0,
            ).dimmed());
    }

    println!();
    println!("  {}", "─".repeat(55));
    println!("  Run {} to see accumulated savings over time.", "savants stats".cyan());
    println!();
}

/// Find a source file in the repo to use for benchmarking.
fn find_source_file(repo_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let extensions = ["ts", "rs", "py", "go", "js"];
    for ext in &extensions {
        if let Ok(entries) = std::fs::read_dir(repo_path.join("src")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                    // Pick a file that's at least 50 lines
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if content.lines().count() >= 50 {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }
    // Try any source file recursively
    fn find_recursive(dir: &std::path::Path, depth: usize) -> Option<std::path::PathBuf> {
        if depth > 3 { return None; }
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if ["ts", "rs", "py", "go", "js", "tsx", "jsx"].contains(&ext) {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if content.lines().count() >= 50 {
                            return Some(path);
                        }
                    }
                }
            } else if path.is_dir() && !path.to_string_lossy().contains("node_modules")
                && !path.to_string_lossy().contains("target")
                && !path.to_string_lossy().contains(".git") {
                if let Some(found) = find_recursive(&path, depth + 1) {
                    return Some(found);
                }
            }
        }
        None
    }
    find_recursive(repo_path, 0)
}

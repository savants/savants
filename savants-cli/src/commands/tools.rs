//! CLI shortcut commands that call MCP tools directly via the cloud API.
//!
//! These let developers run `savants search "query"` from the terminal
//! instead of needing an MCP client like Claude Code or Cursor.

use colored::*;
use serde_json::{json, Value};

const CLOUD_ENDPOINT: &str = "https://api.savants.cloud";

/// Resolve the API key from env or config.
fn get_api_key() -> Option<String> {
    let state = crate::config::State::load();
    state.cloud_token()
}

/// Detect the repo/project name from the current working directory.
fn detect_repo_name() -> String {
    let repo_path = std::env::current_dir().unwrap_or_default();
    if let Ok(output) = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&repo_path)
        .output()
    {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if let Some(name) = url.rsplit('/').next() {
            let name = name.trim_end_matches(".git").to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    repo_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

/// Call a cloud tool and return the result text.
fn call_tool(api_key: &str, tool_name: &str, input: Value) -> Result<String, String> {
    let cloud_url = std::env::var("SAVANTS_CLOUD_URL")
        .unwrap_or_else(|_| CLOUD_ENDPOINT.to_string());

    let url = format!("{}/api/v1/tools/call", cloud_url.trim_end_matches('/'));

    let body = json!({
        "tool": tool_name,
        "input": input,
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("Request failed: {}", e))?;

    let status = resp.status();
    if status.as_u16() == 401 {
        return Err("Session expired. Run 'savants connect' to re-authenticate.".to_string());
    }
    if status.as_u16() == 402 {
        return Err(
            "Free tier limit reached (10 calls/month).\nUpgrade: https://savants.cloud/billing"
                .to_string(),
        );
    }

    let json_resp: Value = resp
        .json()
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    if let Some(err) = json_resp.get("error").and_then(|e| e.as_str()) {
        return Err(format!("Cloud error: {}", err));
    }

    let result = json_resp.get("result");
    match result {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(v) => Ok(serde_json::to_string_pretty(v).unwrap_or_default()),
        None => Ok(json_resp.to_string()),
    }
}

/// Ensure we have an API key, printing a helpful message if not.
fn require_api_key() -> Option<String> {
    match get_api_key() {
        Some(key) if !key.is_empty() => Some(key),
        _ => {
            eprintln!(
                "{}: No API key found. Run '{}' first, or set {}.",
                "Error".red(),
                "savants connect".cyan(),
                "SAVANTS_API_KEY".cyan()
            );
            None
        }
    }
}

/// `savants search <query>` -- semantic search across the codebase
pub fn search(query: &str, limit: Option<usize>) {
    let repo = detect_repo_name();
    let limit = limit.unwrap_or(10);

    println!(
        "{} '{}' in {}...",
        "Searching".bold(),
        query.cyan(),
        repo.dimmed()
    );

    // Try local index first (fast, no network, works offline)
    if crate::embedding_store::EmbeddingStore::exists(&repo) {
        match search_local(query, &repo, limit) {
            Ok(result) => {
                println!("{}", result);
                return;
            }
            Err(e) => {
                eprintln!("{}: local search: {}. Trying cloud...", "Warning".yellow(), e);
            }
        }
    }

    // Cloud fallback: use block_in_place to allow blocking HTTP inside tokio runtime
    if let Some(api_key) = get_api_key().filter(|k| !k.is_empty()) {
        let input = json!({
            "query": query,
            "project_id": repo,
            "repo": repo,
            "limit": limit,
        });

        let result = std::thread::spawn({
            let api_key = api_key.clone();
            move || call_tool(&api_key, "semantic_search", input)
        }).join().unwrap_or_else(|_| Err("thread panic".to_string()));

        match result {
            Ok(result) => {
                println!("{}", result);
                return;
            }
            Err(e) => {
                eprintln!("{}: cloud: {}", "Warning".yellow(), e);
            }
        }
    }

    // Neither local nor cloud available
    eprintln!("{}: No local index for '{}'. Run '{}' first to index the codebase.",
        "Error".red(), repo, "savants up".cyan());
}

/// `savants skeleton <file>` -- show file structure (functions, classes, types)
pub fn skeleton(file: &str) {
    let repo = detect_repo_name();

    println!(
        "{} {}...",
        "Skeleton".bold(),
        file.cyan()
    );

    // Try local index first (fast, no network, works offline)
    match skeleton_local(file, &repo) {
        Ok(result) => {
            println!("{}", result);
            return;
        }
        Err(_) => {}
    }

    // Cloud fallback: spawn in separate thread to avoid tokio runtime conflict
    if let Some(api_key) = get_api_key().filter(|k| !k.is_empty()) {
        let input = json!({
            "file": file,
            "project_id": repo,
            "repo": repo,
        });

        let result = std::thread::spawn({
            let api_key = api_key.clone();
            move || call_tool(&api_key, "file_skeleton", input)
        }).join().unwrap_or_else(|_| Err("thread panic".to_string()));

        match result {
            Ok(result) => {
                println!("{}", result);
                return;
            }
            Err(e) => {
                eprintln!("{}: cloud: {}", "Warning".yellow(), e);
            }
        }
    }

    eprintln!("{}: No local index for '{}'. Run '{}' first to index the codebase.",
        "Error".red(), repo, "savants up".cyan());
}

/// `savants callers <function>` -- find all callers of a function
pub fn callers(function: &str) {
    let repo = detect_repo_name();

    println!(
        "{} callers of {}...",
        "Finding".bold(),
        function.cyan()
    );

    // Try local index first
    match callers_local(function, &repo) {
        Ok(result) if !result.is_empty() => {
            println!("{}", result);
            return;
        }
        _ => {}
    }

    // Cloud fallback
    let api_key = match get_api_key().filter(|k| !k.is_empty()) {
        Some(k) => k,
        None => {
            eprintln!("Function '{}' not found. Try: {} to locate it.",
                function, format!("savants search '{}'", function).cyan());
            return;
        }
    };

    let input = json!({
        "function": function,
        "project_id": repo,
        "repo": repo,
    });

    let result = std::thread::spawn({
        let api_key = api_key.clone();
        move || call_tool(&api_key, "callers", input)
    }).join().unwrap_or_else(|_| Err("thread panic".to_string()));

    match result {
        Ok(ref text) if text.is_empty() || text.contains("not found") || text.contains("Not found") => {
            eprintln!("Function '{}' not found. Try: {} to locate it.",
                function, format!("savants search '{}'", function).cyan());
        }
        Ok(result) => println!("{}", result),
        Err(e) => {
            eprintln!("Function '{}' not found. Try: {} to locate it.",
                function, format!("savants search '{}'", function).cyan());
            eprintln!("{}: {}", "Detail".dimmed(), e);
        }
    }
}

/// `savants xray <function>` -- full structural profile of a function
pub fn xray(function: &str, file: Option<&str>) {
    let repo = detect_repo_name();

    println!(
        "{} {}...",
        "X-ray".bold(),
        function.cyan()
    );

    // Try local index first
    match xray_local(function, file, &repo) {
        Ok(result) if !result.is_empty() => {
            println!("{}", result);
            return;
        }
        _ => {}
    }

    // Cloud fallback
    let api_key = match require_api_key() {
        Some(k) => k,
        None => return,
    };

    let mut input = json!({
        "function_name": function,
        "project_id": repo,
        "repo": repo,
    });

    if let Some(f) = file {
        input
            .as_object_mut()
            .unwrap()
            .insert("file_path".to_string(), json!(f));
    }

    let result = std::thread::spawn({
        let api_key = api_key.clone();
        move || call_tool(&api_key, "function_xray", input)
    }).join().unwrap_or_else(|_| Err("thread panic".to_string()));

    match result {
        Ok(result) => println!("{}", result),
        Err(e) => eprintln!("{}: {}", "Error".red(), e),
    }
}

/// `savants blast <function>` -- blast radius analysis
pub fn blast(function: &str, depth: Option<usize>) {
    let repo = detect_repo_name();
    let depth = depth.unwrap_or(3);

    println!(
        "{} blast radius of {}...",
        "Analyzing".bold(),
        function.cyan()
    );

    // Try local index first
    match blast_local(function, depth, &repo) {
        Ok(result) if !result.is_empty() => {
            println!("{}", result);
            return;
        }
        _ => {}
    }

    // Cloud fallback
    let api_key = match get_api_key().filter(|k| !k.is_empty()) {
        Some(k) => k,
        None => {
            eprintln!("Function '{}' not found. Try: {} to locate it.",
                function, format!("savants search '{}'", function).cyan());
            return;
        }
    };

    let input = json!({
        "function": function,
        "project_id": repo,
        "repo": repo,
        "depth": depth,
    });

    let result = std::thread::spawn({
        let api_key = api_key.clone();
        move || call_tool(&api_key, "blast_radius", input)
    }).join().unwrap_or_else(|_| Err("thread panic".to_string()));

    match result {
        Ok(ref text) if text.is_empty() || text.contains("not found") || text.contains("Not found") => {
            eprintln!("Function '{}' not found. Try: {} to locate it.",
                function, format!("savants search '{}'", function).cyan());
        }
        Ok(result) => println!("{}", result),
        Err(e) => {
            eprintln!("Function '{}' not found. Try: {} to locate it.",
                function, format!("savants search '{}'", function).cyan());
            eprintln!("{}: {}", "Detail".dimmed(), e);
        }
    }
}

/// `savants brief` -- what changed since you last looked
pub fn brief(since: Option<&str>) {
    let repo = detect_repo_name();
    let repo_path = std::env::current_dir().unwrap_or_default();

    // Verify we're in a git repo
    let git_check = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&repo_path)
        .output();
    if git_check.is_err() || !git_check.unwrap().status.success() {
        eprintln!("{}: not a git repository", "Error".red());
        return;
    }

    let savants_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".savants");
    std::fs::create_dir_all(&savants_dir).ok();

    let last_brief_path = savants_dir.join("last-brief");
    let agent_touched_path = savants_dir.join("agent-touched.json");

    // Determine the --since value
    // Default: last brief timestamp, or 7 days for first run (enough to show useful context)
    let since_str = if let Some(s) = since {
        s.to_string()
    } else if let Ok(ts) = std::fs::read_to_string(&last_brief_path) {
        let ts = ts.trim().to_string();
        if ts.is_empty() { "7 days ago".to_string() } else { ts }
    } else {
        "7 days ago".to_string()
    };

    // Get commits since the time window
    let log_output = std::process::Command::new("git")
        .args(["log", "--since", &since_str, "--format=%H|%an|%s", "--no-merges"])
        .current_dir(&repo_path)
        .output();

    let log_text = match log_output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(e) => {
            eprintln!("{}: git log failed: {}", "Error".red(), e);
            return;
        }
    };

    let commits: Vec<(&str, &str, &str)> = log_text
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let mut parts = l.splitn(3, '|');
            let hash = parts.next()?;
            let author = parts.next()?;
            let msg = parts.next()?;
            Some((hash, author, msg))
        })
        .collect();

    if commits.is_empty() {
        // If user specified --since explicitly, don't auto-expand
        if since.is_some() {
            println!("=== Savants Brief: {} ===", repo);
            println!("Time window: since {} (user-specified)", since_str);
            println!();
            println!("No commits found in this time window.");
            save_brief_timestamp(&last_brief_path);
            return;
        }

        // Smart fallback: try progressively wider windows
        let fallback_windows = ["7 days ago", "30 days ago", "90 days ago"];
        let mut found_window: Option<&str> = None;
        let mut fallback_log = String::new();

        for window in &fallback_windows {
            // Skip windows we already tried (since_str might be "7 days ago" from default)
            let check = std::process::Command::new("git")
                .args(["log", "--since", window, "--format=%H|%an|%s", "--no-merges"])
                .current_dir(&repo_path)
                .output();
            if let Ok(o) = check {
                let text = String::from_utf8_lossy(&o.stdout).to_string();
                if !text.trim().is_empty() {
                    found_window = Some(window);
                    fallback_log = text;
                    break;
                }
            }
        }

        if let Some(window) = found_window {
            println!("=== Savants Brief: {} ===", repo);
            println!("No commits since last brief. Showing last {} instead:", window.trim_end_matches(" ago"));
            println!();

            let fallback_commits: Vec<(&str, &str, &str)> = fallback_log
                .lines()
                .filter(|l| !l.is_empty())
                .filter_map(|l| {
                    let mut parts = l.splitn(3, '|');
                    let hash = parts.next()?;
                    let author = parts.next()?;
                    let msg = parts.next()?;
                    Some((hash, author, msg))
                })
                .collect();

            println!("Commits ({}):", fallback_commits.len());
            for (hash, author, msg) in fallback_commits.iter().take(10) {
                println!("  {} {} ({})", &hash[..7.min(hash.len())], msg, author);
            }
            if fallback_commits.len() > 10 {
                println!("  ... and {} more", fallback_commits.len() - 10);
            }

            // Delete last-brief so the next run doesn't repeat this empty window
            std::fs::remove_file(&last_brief_path).ok();
            return;
        }

        // No commits found even at 90 days
        println!("=== Savants Brief: {} ===", repo);
        println!();
        println!("No commits found in the last 90 days.");
        // Delete stale last-brief marker
        std::fs::remove_file(&last_brief_path).ok();
        return;
    }

    // Count authors
    let mut author_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (_, author, _) in &commits {
        *author_counts.entry(author).or_insert(0) += 1;
    }
    let author_summary: Vec<String> = author_counts.iter()
        .map(|(a, c)| format!("{} by {}", c, a))
        .collect();

    // Get changed files with name-status
    let first_commit = commits.last().map(|(h, _, _)| *h).unwrap_or("HEAD~1");
    let name_status_output = std::process::Command::new("git")
        .args(["diff", "--name-status", &format!("{}~1..HEAD", first_commit)])
        .current_dir(&repo_path)
        .output();

    let changed_files: Vec<(String, String)> = match name_status_output {
        Ok(o) => {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .filter_map(|l| {
                    let mut parts = l.split('\t');
                    let status = parts.next()?.trim().to_string();
                    let file = parts.next()?.trim().to_string();
                    Some((status, file))
                })
                .collect()
        }
        Err(_) => vec![],
    };

    // Try to load the code index for function-level analysis
    let parse_result = load_parse_index(&repo).ok();

    // Get the previous index snapshot to detect added/removed functions
    // We compare functions in changed files against what git shows
    let mut functions_affected: Vec<(String, String, String, usize)> = vec![]; // (name, file, status, caller_count)

    if let Some(ref pr) = parse_result {
        for (status, file) in &changed_files {
            let file_entities: Vec<_> = pr.entities.iter()
                .filter(|e| e.kind == "function" && (e.file == *file || e.file.ends_with(&format!("/{}", file))))
                .collect();

            for entity in &file_entities {
                let caller_count = pr.call_sites.iter()
                    .filter(|cs| cs.callee_name == entity.name)
                    .count();
                let func_status = match status.as_str() {
                    "A" => "NEW".to_string(),
                    "D" => "REMOVED".to_string(),
                    _ => "modified".to_string(),
                };
                functions_affected.push((
                    entity.name.clone(),
                    file.clone(),
                    func_status,
                    caller_count,
                ));
            }
        }
    }

    // Load agent-touched files for conflict detection
    let agent_touched: Vec<String> = std::fs::read_to_string(&agent_touched_path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();

    let conflicts: Vec<&str> = changed_files.iter()
        .filter(|(_, f)| agent_touched.iter().any(|t| t == f))
        .map(|(_, f)| f.as_str())
        .collect();

    // Count guard rules (look for .savants/rules/ or active profile)
    let guard_rule_count = count_guard_rules(&savants_dir);

    // ---- Output ----
    println!("=== Savants Brief: {} ===", repo);

    // Show the time window source so users know what range they're seeing
    if since.is_some() {
        println!("Time window: since {} (user-specified)", since_str);
    } else if std::fs::read_to_string(&last_brief_path).ok().filter(|s| !s.trim().is_empty()).is_some() {
        println!("Time window: since {} (last brief)", since_str);
    } else {
        println!("Time window: last 7 days (default)");
    }

    println!("Since: {} ({} commits, {})", since_str, commits.len(), author_summary.join(", "));

    // Executive summary
    let added = changed_files.iter().filter(|(s, _)| s == "A").count();
    let modified = changed_files.iter().filter(|(s, _)| s == "M").count();
    let deleted = changed_files.iter().filter(|(s, _)| s == "D").count();
    let mut summary_parts = vec![];
    if modified > 0 { summary_parts.push(format!("{} modified", modified)); }
    if added > 0 { summary_parts.push(format!("{} new", added)); }
    if deleted > 0 { summary_parts.push(format!("{} removed", deleted)); }
    if !summary_parts.is_empty() {
        println!("Summary: {} files ({})", changed_files.len(), summary_parts.join(", "));
    }

    // Focus areas summary — top directories with most changes
    if !changed_files.is_empty() {
        let mut dir_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (_, file) in &changed_files {
            let dir = file.split('/').take(2).collect::<Vec<&str>>().join("/");
            *dir_counts.entry(dir).or_insert(0) += 1;
        }
        let mut dirs: Vec<_> = dir_counts.into_iter().collect();
        dirs.sort_by(|a, b| b.1.cmp(&a.1));
        let top_dirs: Vec<String> = dirs.iter().take(3).map(|(d, c)| format!("{} ({})", d, c)).collect();
        println!("Focus areas: {}", top_dirs.join(", "));
    }
    println!();

    // Recent commits
    println!("Commits:");
    for (hash, author, msg) in commits.iter().take(10) {
        println!("  {} {} ({})", &hash[..7.min(hash.len())], msg, author);
    }
    if commits.len() > 10 {
        println!("  ... and {} more", commits.len() - 10);
    }
    println!();

    // Activity by area: group commits by top-level directory they touched
    {
        let mut area_commits: std::collections::HashMap<String, Vec<&str>> = std::collections::HashMap::new();
        for (hash, _, msg) in &commits {
            // Get files touched by this commit
            let files_output = std::process::Command::new("git")
                .args(["diff-tree", "--no-commit-id", "--name-only", "-r", hash])
                .current_dir(&repo_path)
                .output();
            if let Ok(o) = files_output {
                let files_text = String::from_utf8_lossy(&o.stdout).to_string();
                // Find the most-touched top-level directory for this commit
                let mut dir_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
                for line in files_text.lines().filter(|l| !l.is_empty()) {
                    let dir = line.split('/').next().unwrap_or(line).to_string();
                    *dir_counts.entry(dir).or_insert(0) += 1;
                }
                if let Some((top_dir, _)) = dir_counts.into_iter().max_by_key(|(_, c)| *c) {
                    area_commits.entry(top_dir).or_default().push(msg);
                }
            }
        }

        if !area_commits.is_empty() {
            let mut areas: Vec<_> = area_commits.into_iter().collect();
            areas.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
            println!("Activity by area:");
            for (area, msgs) in &areas {
                let summaries: Vec<&str> = msgs.iter().take(4).copied().collect();
                let summary_str = summaries.join(", ");
                let extra = if msgs.len() > 4 { format!(", ...") } else { String::new() };
                println!("  {} ({} commits): {}{}", area, msgs.len(), summary_str, extra);
            }
            println!();
        }
    }

    // Changed files (truncate to top 10 if >20 to reduce noise)
    if !changed_files.is_empty() {
        let max_files = if changed_files.len() > 20 { 10 } else { changed_files.len() };
        println!("Changed files:");
        for (status, file) in changed_files.iter().take(max_files) {
            let status_char = match status.as_str() {
                "A" => "A",
                "D" => "D",
                "M" => "M",
                "R" | "R100" => "R",
                _ => status.as_str(),
            };

            // Count functions in this file from index
            let func_count = functions_affected.iter()
                .filter(|(_, f, _, _)| f == file)
                .count();
            let func_note = if func_count > 0 {
                let func_names: Vec<String> = functions_affected.iter()
                    .filter(|(_, f, _, _)| f == file)
                    .take(3)
                    .map(|(n, _, s, _)| if s == "NEW" { format!("{} (new)", n) } else { n.clone() })
                    .collect();
                let extra = if func_count > 3 { format!(", +{} more", func_count - 3) } else { String::new() };
                format!(" -- {} functions ({}{})", func_count, func_names.join(", "), extra)
            } else if status.as_str() == "A" {
                " -- new file".to_string()
            } else if status.as_str() == "D" {
                " -- removed".to_string()
            } else {
                String::new()
            };

            println!("  {} {}{}", status_char, file, func_note);
        }
        if changed_files.len() > 20 {
            println!("  ... and {} more files", changed_files.len() - 10);
        }
        println!();
    }

    // Functions affected
    if !functions_affected.is_empty() {
        println!("Functions affected:");
        for (name, file, status, callers) in &functions_affected {
            let caller_note = if *callers > 0 { format!(", {} callers", callers) } else { String::new() };
            println!("  {} ({}:{}) -- {}{}", name, file,
                parse_result.as_ref()
                    .and_then(|pr| pr.entities.iter().find(|e| e.name == *name && (e.file == *file || e.file.ends_with(&format!("/{}", file)))))
                    .map(|e| e.line)
                    .unwrap_or(0),
                status, caller_note);
        }
        println!();
    }

    // Guard rules
    if guard_rule_count > 0 {
        println!("Guard rules active: {}", guard_rule_count);
    }

    // Conflicts
    if conflicts.is_empty() {
        println!("No conflicts with your previous changes.");
    } else {
        println!("{}: {} files conflict with your previous changes:", "Warning".yellow(), conflicts.len());
        for f in &conflicts {
            println!("  {}", f);
        }
    }

    // Save timestamp for next brief
    save_brief_timestamp(&last_brief_path);
}

/// Save the current UTC timestamp as the last-brief marker.
fn save_brief_timestamp(path: &std::path::Path) {
    // Use ISO 8601 format from git-compatible date
    let output = std::process::Command::new("date")
        .args(["--utc", "+%Y-%m-%dT%H:%M:%SZ"])
        .output();
    if let Ok(o) = output {
        let ts = String::from_utf8_lossy(&o.stdout).trim().to_string();
        std::fs::write(path, &ts).ok();
    }
}

/// Count the number of active guard rules from profiles.
fn count_guard_rules(savants_dir: &std::path::Path) -> usize {
    // Check for .savants-guard.json in current dir first
    let cwd_guard = std::env::current_dir()
        .unwrap_or_default()
        .join(".savants-guard.json");

    let guard_file = if cwd_guard.exists() {
        Some(cwd_guard)
    } else {
        // Check for active profile in ~/.savants/profiles/
        let standard = savants_dir.join("profiles").join("standard.json");
        if standard.exists() { Some(standard) } else { None }
    };

    if let Some(path) = guard_file {
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(json) = serde_json::from_str::<Value>(&data) {
                if let Some(rules) = json.get("rules").and_then(|r| r.as_array()) {
                    return rules.len();
                }
            }
        }
    }
    0
}

// ---- Local fallback implementations ----

/// Path to the cached parse index JSON file for a repo.
fn parse_index_path(repo: &str) -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".savants")
        .join("code-index")
        .join(format!("{}.json", repo))
}

/// Load the cached parse result from disk.
fn load_parse_index(repo: &str) -> Result<crate::code_parser::ParseResult, String> {
    let path = parse_index_path(repo);
    let data = std::fs::read_to_string(&path)
        .map_err(|_| format!("No local index for '{}'. Run 'savants up' first to index the codebase.", repo))?;
    serde_json::from_str(&data)
        .map_err(|e| format!("Corrupt index for '{}': {}", repo, e))
}

/// Tokenize a name by splitting on underscores and camelCase boundaries.
fn tokenize_name(name: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    // First split by underscores
    for part in name.split('_') {
        if part.is_empty() { continue; }
        // Then split camelCase: "evaluateRule" -> ["evaluate", "Rule"]
        let mut current = String::new();
        for ch in part.chars() {
            if ch.is_uppercase() && !current.is_empty() {
                tokens.push(current.to_lowercase());
                current = String::new();
            }
            current.push(ch);
        }
        if !current.is_empty() {
            tokens.push(current.to_lowercase());
        }
    }
    tokens
}

/// Check if query words match a function name via substring matching.
/// Returns true if ANY query word is a substring of any name token (or vice versa).
/// Check if two words are similar enough to be a match.
/// Handles: exact match, substring, and stem similarity (first N chars).
fn words_similar(a: &str, b: &str) -> bool {
    if a == b { return true; }
    if a.contains(b) || b.contains(a) { return true; }
    // Stem match: if both are 5+ chars and share the first 6 chars, treat as match
    // This catches: evaluate/evaluation, authenticate/authentication, etc.
    if a.len() >= 5 && b.len() >= 5 {
        let prefix_len = std::cmp::min(std::cmp::min(a.len(), b.len()), 6);
        if a[..prefix_len] == b[..prefix_len] { return true; }
    }
    false
}

fn name_matches_query(name_tokens: &[String], query_words: &[String]) -> f32 {
    let meaningful_words: Vec<&String> = query_words.iter().filter(|w| w.len() >= 3).collect();
    if meaningful_words.is_empty() { return 0.0; }

    let mut match_count = 0;
    for qw in &meaningful_words {
        for nt in name_tokens {
            if words_similar(nt, qw) {
                match_count += 1;
                break;
            }
        }
    }

    // Return match ratio: how many query words matched / total meaningful words
    // Require at least 50% of query words to match
    let ratio = match_count as f32 / meaningful_words.len() as f32;
    if ratio >= 0.5 { ratio } else { 0.0 }
}

/// Local semantic search using the embedding store.
fn search_local(query: &str, repo: &str, limit: usize) -> Result<String, String> {
    if !crate::embedding_store::EmbeddingStore::exists(repo) {
        return Err(format!("No local index for '{}'. Run 'savants up' first to index the codebase.", repo));
    }

    let store = crate::embedding_store::EmbeddingStore::load(repo)?;

    // Phase 1: Exact name matching against the code index
    let query_lower = query.to_lowercase();
    let query_words: Vec<String> = query_lower.split_whitespace()
        .map(|w| w.to_string())
        .collect();

    let mut name_matched_indices: Vec<usize> = Vec::new();

    // Also check the code-index JSON for DIRECT name matches
    // This catches functions not in the embedding store (especially Rust functions)
    // Simple approach: each query word (4+ chars) must appear as substring of the function name
    let mut code_index_matches: Vec<(String, String, usize, f32)> = Vec::new();
    if let Ok(parse_result) = load_parse_index(repo) {
        let long_words: Vec<&str> = query_lower.split_whitespace()
            .filter(|w| w.len() >= 4)
            .collect();
        if !long_words.is_empty() {
            for entity in &parse_result.entities {
                if entity.kind != "function" && entity.kind != "class" { continue; }
                if entity.name.len() <= 3 { continue; }
                let name_lower = entity.name.to_lowercase();
                // Count how many query words appear in the function name (substring or stem)
                let mut hits = 0;
                for word in &long_words {
                    let stem = &word[..std::cmp::min(word.len(), 6)];
                    if name_lower.contains(stem) {
                        hits += 1;
                    }
                }
                // Require at least 2 word stems to match (or 1 if query has 1 long word)
                let threshold = if long_words.len() <= 1 { 1 } else { 2 };
                if hits >= threshold {
                    let score = hits as f32 / long_words.len() as f32;
                    code_index_matches.push((entity.name.clone(), entity.file.clone(), entity.line, score));
                }
            }
            code_index_matches.sort_by(|a, b| {
                b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.0.len().cmp(&b.0.len()))
            });
            code_index_matches.truncate(5);
        }
    }
    let mut name_matched_set: std::collections::HashSet<usize> = std::collections::HashSet::new();

    let mut name_scores: Vec<(usize, f32)> = Vec::new();
    for (idx, entry) in store.entries.iter().enumerate() {
        // Skip very short names (1-2 chars) — they match everything
        if entry.name.len() <= 2 { continue; }
        let name_tokens = tokenize_name(&entry.name);
        // Skip if name tokenizes to nothing meaningful
        if name_tokens.iter().all(|t| t.len() < 3) { continue; }
        let score = name_matches_query(&name_tokens, &query_words);
        if score > 0.0 {
            name_scores.push((idx, score));
        }
    }
    // Sort by match score descending, take top 5 (tight to reduce noise)
    name_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    name_scores.truncate(5);
    for (idx, _) in &name_scores {
        name_matched_indices.push(*idx);
        name_matched_set.insert(*idx);
    }

    // Phase 2: Semantic search
    let mut engine = crate::embeddings::EmbeddingEngine::new()
        .map_err(|e| format!("Embedding engine: {}", e))?;
    let query_emb = engine.embed_one(query)
        .map_err(|e| format!("Embedding query: {}", e))?;

    let semantic_results = store.search(&query_emb, limit + name_matched_indices.len());

    // Merge: name matches first (score 1.0), then semantic (excluding duplicates)
    let mut final_results: Vec<(usize, f32)> = name_matched_indices.iter()
        .map(|&idx| (idx, 1.0f32))
        .collect();

    for (idx, score) in &semantic_results {
        // Skip low-quality semantic results (noise threshold)
        if *score < 0.45 { continue; }
        if !name_matched_set.contains(idx) {
            final_results.push((*idx, *score));
        }
    }
    final_results.truncate(limit);

    if final_results.is_empty() {
        return Ok(format!("No results for '{}'", query));
    }

    // Check if semantic results are too weak (best score < 0.45) and no name matches — likely a literal query
    let best_semantic = semantic_results.first().map(|(_, s)| *s).unwrap_or(0.0);
    if name_matched_indices.is_empty() && best_semantic < 0.45 {
        // Fall back to grep-style literal text search
        let repo_path = std::env::current_dir().unwrap_or_default();
        let grep_output = std::process::Command::new("grep")
            .args(["-rn", "--include=*.rs", "--include=*.ts", "--include=*.js",
                   "--include=*.py", "--include=*.go",
                   "--exclude-dir=node_modules", "--exclude-dir=target",
                   "--exclude-dir=dist", "--exclude-dir=.git",
                   "--exclude-dir=__pycache__", "--exclude-dir=.venv",
                   query, "."])
            .current_dir(&repo_path)
            .output();

        if let Ok(output) = grep_output {
            let grep_text = String::from_utf8_lossy(&output.stdout).to_string();
            if !grep_text.trim().is_empty() {
                let mut lines = vec![format!(
                    "Semantic matches weak (best: {:.2}) — falling back to text search:",
                    best_semantic
                )];
                for line in grep_text.lines().take(limit) {
                    lines.push(format!("  {}", line));
                }
                let total = grep_text.lines().count();
                if total > limit {
                    lines.push(format!("  ... and {} more matches", total - limit));
                }
                return Ok(lines.join("\n"));
            }
        }
    }

    let repo_path = std::env::current_dir().unwrap_or_default();
    let name_match_count = name_matched_indices.len();
    let code_idx_count = code_index_matches.len();
    let total_results = final_results.len() + code_idx_count;
    let total_name = name_match_count + code_idx_count;
    let mut lines = vec![format!("=== Semantic search: '{}' ({} results{}) ===",
        query, total_results,
        if total_name > 0 { format!(", {} by name", total_name) } else { String::new() })];

    // Show code-index-only matches FIRST (these are often the most relevant for exact queries)
    let mut shown_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (name, file, line, _score) in &code_index_matches {
        // Skip if already shown from embedding store
        let key = format!("{}:{}", file, name);
        if !shown_names.insert(key) { continue; }
        // Skip if also in embedding store results
        let in_final = final_results.iter().any(|(idx, _)| {
            store.entries.get(*idx).map(|e| e.name == *name && e.file == *file).unwrap_or(false)
        });
        if in_final { continue; }

        lines.push(format!("  {}:{} function {} [name]", file, line, name));
        let file_path = repo_path.join(file);
        if let Ok(content) = std::fs::read_to_string(&file_path) {
            if let Some(src_line) = content.lines().nth(line.saturating_sub(1)) {
                let trimmed = src_line.trim();
                let snippet: String = trimmed.chars().take(100).collect();
                if !snippet.is_empty() {
                    lines.push(format!("    {}", snippet));
                }
            }
        }
    }

    for (idx, score) in &final_results {
        if let Some(entry) = store.entries.get(*idx) {
            let kind_str = match entry.kind { 1 => "class", 2 => "interface", _ => "function" };
            let score_label = if *score >= 1.0 {
                format!("{}", "[high — name match]".green())
            } else if *score >= 0.6 {
                format!("{}", "[high]".green())
            } else if *score >= 0.5 {
                "[medium]".to_string()
            } else {
                "[low]".to_string()
            };
            lines.push(format!("  {}:{} {} {} {}",
                entry.file, entry.line, kind_str, entry.name, score_label));

            // Show 1-line source preview from disk
            let file_path = repo_path.join(&entry.file);
            if let Ok(content) = std::fs::read_to_string(&file_path) {
                if let Some(src_line) = content.lines().nth((entry.line as usize).saturating_sub(1)) {
                    let trimmed = src_line.trim();
                    let snippet: String = trimmed.chars().take(100).collect();
                    if !snippet.is_empty() {
                        lines.push(format!("    {}", snippet));
                    }
                }
            }
        }
    }

    Ok(lines.join("\n"))
}

/// Local file skeleton from the cached parse index.
fn skeleton_local(file: &str, repo: &str) -> Result<String, String> {
    // Try cached parse index first
    if let Ok(parse_result) = load_parse_index(repo) {
        return skeleton_from_parse_result(file, &parse_result);
    }

    // Fall back to parsing just this file on-demand
    let repo_path = std::env::current_dir()
        .map_err(|e| format!("Cannot get cwd: {}", e))?;

    let file_path = repo_path.join(file);
    if !file_path.exists() {
        return Err(format!("File not found: {}", file));
    }

    let mut parser = crate::code_parser::CodeParser::new(repo);
    let parse_result = parser.parse_repo(&repo_path.to_string_lossy());

    skeleton_from_parse_result(file, &parse_result)
}

/// Format a file skeleton from parsed entities.
fn skeleton_from_parse_result(file: &str, result: &crate::code_parser::ParseResult) -> Result<String, String> {
    // Match entities for the requested file (try exact and suffix match)
    let file_entities: Vec<_> = result.entities.iter()
        .filter(|e| e.file == file || e.file.ends_with(&format!("/{}", file)))
        .collect();

    if file_entities.is_empty() {
        return Err(format!("No code entities found in '{}'. Is it a supported source file?", file));
    }

    let mut lines = vec![format!("=== {} ===", file)];

    let classes: Vec<_> = file_entities.iter().filter(|e| e.kind == "class").collect();
    let interfaces: Vec<_> = file_entities.iter().filter(|e| e.kind == "interface").collect();
    let functions: Vec<_> = file_entities.iter().filter(|e| e.kind == "function").collect();

    if !classes.is_empty() {
        lines.push("Classes:".to_string());
        for e in &classes {
            lines.push(format!("  class {} (lines {}-{})", e.name, e.line, e.end_line));
        }
    }

    if !interfaces.is_empty() {
        lines.push("Types/Interfaces:".to_string());
        for e in &interfaces {
            lines.push(format!("  {} (lines {}-{})", e.name, e.line, e.end_line));
        }
    }

    if !functions.is_empty() {
        lines.push("Functions:".to_string());
        let repo_path = std::env::current_dir().unwrap_or_default();
        for e in &functions {
            let params_str = if e.params.is_empty() { String::new() }
                else { format!("({})", e.params.join(", ")) };

            // Read the source line to extract visibility and return type
            let mut visibility = String::new();
            let mut return_type = String::new();
            let file_path = repo_path.join(&e.file);
            if let Ok(content) = std::fs::read_to_string(&file_path) {
                if let Some(src_line) = content.lines().nth(e.line.saturating_sub(1)) {
                    let trimmed = src_line.trim();
                    // Detect visibility: pub, pub(crate), export, async
                    if trimmed.starts_with("pub(crate) ") || trimmed.starts_with("pub(super) ") {
                        let end = trimmed.find(')').unwrap_or(0) + 2;
                        visibility = trimmed[..end].to_string();
                    } else if trimmed.starts_with("pub ") {
                        visibility = "pub ".to_string();
                    } else if trimmed.starts_with("export ") {
                        visibility = "export ".to_string();
                    }
                    // Detect return type
                    // Rust: -> Type (before { or where)
                    if let Some(arrow_pos) = trimmed.find("->") {
                        let after_arrow = &trimmed[arrow_pos + 2..];
                        let ret = after_arrow
                            .split(|c: char| c == '{' || c == ';')
                            .next()
                            .unwrap_or("")
                            .replace("where", "")
                            .trim()
                            .to_string();
                        if !ret.is_empty() {
                            return_type = format!(" -> {}", ret);
                        }
                    }
                    // TypeScript/Python: ): ReturnType or ) -> Type
                    else if let Some(colon_pos) = trimmed.rfind("): ") {
                        let after = &trimmed[colon_pos + 3..];
                        let ret = after
                            .split(|c: char| c == '{' || c == ';' || c == ':')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !ret.is_empty() {
                            return_type = format!(": {}", ret);
                        }
                    }
                }
            }

            let line_count = e.end_line.saturating_sub(e.line) + 1;
            let line_count_str = if line_count > 100 {
                format!(" ({} lines -- consider splitting)", line_count)
            } else {
                format!(" ({} lines)", line_count)
            };
            lines.push(format!("  {}fn {}{}{}  L{}-{}{}", visibility, e.name, params_str, return_type, e.line, e.end_line, line_count_str));
        }
    }

    lines.push(format!("\n{} functions, {} classes, {} types/interfaces",
        functions.len(), classes.len(), interfaces.len()));

    Ok(lines.join("\n"))
}

/// Local callers lookup using the cached parse index (call_sites).
/// Shows the recursive caller chain (transitive callers), not just direct callers.
fn callers_local(function: &str, repo: &str) -> Result<String, String> {
    let parse_result = load_parse_index(repo)?;

    // Check if the function exists at all
    let direct_callers: Vec<_> = parse_result.call_sites.iter()
        .filter(|cs| cs.callee_name == function)
        .collect();

    if direct_callers.is_empty() {
        let exists = parse_result.entities.iter()
            .any(|e| e.kind == "function" && e.name == function);
        if exists {
            return Ok(format!("=== Callers of '{}' ===\nNo callers found — this function is not called anywhere in the indexed codebase.", function));
        }
        return Ok(String::new());
    }

    // Recursively walk callers upward (BFS with depth tracking)
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    let mut chain: Vec<(String, String, usize)> = vec![]; // (caller_name, file, depth)

    // Seed with direct callers
    for cs in &direct_callers {
        let key = format!("{}::{}", cs.caller_file, cs.caller_name);
        if visited.insert(key) {
            queue.push_back((cs.caller_name.clone(), cs.caller_file.clone(), 1usize));
            chain.push((cs.caller_name.clone(), cs.caller_file.clone(), 1));
        }
    }

    // Walk upward: find callers of callers
    while let Some((current, _current_file, depth)) = queue.pop_front() {
        if depth >= 10 { continue; } // safety limit
        for cs in &parse_result.call_sites {
            if cs.callee_name == current {
                let key = format!("{}::{}", cs.caller_file, cs.caller_name);
                if visited.insert(key) {
                    queue.push_back((cs.caller_name.clone(), cs.caller_file.clone(), depth + 1));
                    chain.push((cs.caller_name.clone(), cs.caller_file.clone(), depth + 1));
                }
            }
        }
    }

    let repo_path = std::env::current_dir().unwrap_or_default();
    let mut lines = vec![format!("=== Callers of '{}' ===", function)];

    // Build a map of (caller, callee) -> call site count for annotation
    let mut call_site_counts: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();
    for cs in &parse_result.call_sites {
        *call_site_counts.entry((cs.caller_name.clone(), cs.callee_name.clone())).or_insert(0) += 1;
    }

    // For each caller in the chain, figure out which callee it was added for
    // Depth 1 callers call `function` directly; deeper callers call the previous level
    // We track the callee relationship per chain entry
    let mut callee_for_entry: Vec<String> = Vec::new();
    for (name, _file, depth) in &chain {
        if *depth == 1 {
            callee_for_entry.push(function.to_string());
        } else {
            // Find the callee: the first chain entry at depth-1 that this caller calls
            let mut found_callee = String::new();
            for (prev_name, _, prev_depth) in &chain {
                if *prev_depth == depth - 1 {
                    let count = call_site_counts.get(&(name.clone(), prev_name.clone())).copied().unwrap_or(0);
                    if count > 0 {
                        found_callee = prev_name.clone();
                        break;
                    }
                }
            }
            callee_for_entry.push(found_callee);
        }
    }

    for (i, (name, file, depth)) in chain.iter().enumerate() {
        let indent = "  ".repeat(*depth);
        let short_file = file.rsplit('/').next().unwrap_or(file);
        // Look up line number from entities
        let line_num = parse_result.entities.iter()
            .find(|e| e.kind == "function" && e.name == *name && (e.file == *file || e.file.ends_with(&format!("/{}", file)) || file.ends_with(&format!("/{}", e.file))))
            .map(|e| e.line);
        let loc = match line_num {
            Some(ln) => format!("{}:{}", short_file, ln),
            None => short_file.to_string(),
        };
        // Count call sites for this caller -> its callee
        let callee = &callee_for_entry[i];
        let site_count = if callee.is_empty() {
            1 // fallback
        } else {
            call_site_counts.get(&(name.clone(), callee.clone())).copied().unwrap_or(1)
        };
        let site_label = if site_count == 1 {
            "1 call site".to_string()
        } else {
            format!("{} call sites", site_count)
        };
        lines.push(format!("{}{}  ({}) \u{2014} {}", indent, name, loc, site_label));

        // Show the call-site line of code where the callee is invoked
        let callee_name = &callee_for_entry[i];
        if !callee_name.is_empty() {
            let file_path = repo_path.join(file);
            if let Ok(content) = std::fs::read_to_string(&file_path) {
                // Search for the line containing the callee name within the caller's source
                for src_line in content.lines() {
                    if src_line.contains(callee_name.as_str()) {
                        let trimmed = src_line.trim();
                        if !trimmed.is_empty() {
                            let snippet: String = trimmed.chars().take(100).collect();
                            lines.push(format!("{}  {}", indent, snippet));
                            break;
                        }
                    }
                }
            }
        }
    }
    Ok(lines.join("\n"))
}

/// Local xray: show function details from the cached parse index.
fn xray_local(function: &str, file: Option<&str>, repo: &str) -> Result<String, String> {
    let parse_result = load_parse_index(repo)?;

    // Find the function
    let func = parse_result.entities.iter()
        .find(|e| {
            e.kind == "function" && e.name == function
                && file.map_or(true, |f| e.file == f || e.file.ends_with(&format!("/{}", f)))
        });

    let func = match func {
        Some(f) => f,
        None => return Ok(String::new()),
    };

    let mut lines = vec![format!("=== X-ray: {} ===", function)];
    lines.push(format!("File: {}:{}-{}", func.file, func.line, func.end_line));
    if !func.params.is_empty() {
        lines.push(format!("Params: {}", func.params.join(", ")));
    }

    // Find callers
    let callers: Vec<_> = parse_result.call_sites.iter()
        .filter(|cs| cs.callee_name == function)
        .collect();
    if !callers.is_empty() {
        lines.push(format!("Callers ({}):", callers.len()));
        let mut seen = std::collections::HashSet::new();
        for cs in &callers {
            let key = format!("{}::{}", cs.caller_file, cs.caller_name);
            if seen.insert(key) {
                lines.push(format!("  {} in {}", cs.caller_name, cs.caller_file));
            }
        }
    }

    // Find callees (what this function calls)
    let callees: Vec<_> = parse_result.call_sites.iter()
        .filter(|cs| cs.caller_name == function && cs.caller_file == func.file)
        .collect();
    if !callees.is_empty() {
        lines.push(format!("Calls ({}):", callees.len()));
        let mut seen = std::collections::HashSet::new();
        for cs in &callees {
            if seen.insert(&cs.callee_name) {
                lines.push(format!("  {}", cs.callee_name));
            }
        }
    }

    // Show body preview
    if !func.body.is_empty() {
        let preview: String = func.body.chars().take(500).collect();
        lines.push(format!("\nSource preview:\n{}", preview));
    }

    Ok(lines.join("\n"))
}

/// Local blast radius: find transitive callers from the cached parse index.
/// Differentiates from `callers` by adding risk assessment, file details, test coverage, and co-change info.
fn blast_local(function: &str, depth: usize, repo: &str) -> Result<String, String> {
    let parse_result = load_parse_index(repo)?;

    // Find the function entity for file info
    let func_entity = parse_result.entities.iter()
        .find(|e| e.kind == "function" && e.name == function);

    let mut visited = std::collections::HashSet::new();
    let mut queue = vec![(function.to_string(), 0usize)];
    let mut results: Vec<(String, String, usize)> = vec![]; // (caller_name, file, depth)

    while let Some((current, d)) = queue.pop() {
        if d >= depth { continue; }
        for cs in &parse_result.call_sites {
            if cs.callee_name == current {
                let key = format!("{}::{}", cs.caller_file, cs.caller_name);
                if visited.insert(key.clone()) {
                    results.push((cs.caller_name.clone(), cs.caller_file.clone(), d + 1));
                    queue.push((cs.caller_name.clone(), d + 1));
                }
            }
        }
    }

    if results.is_empty() {
        let exists = parse_result.entities.iter()
            .any(|e| e.kind == "function" && e.name == function);
        if exists {
            return Ok(format!("=== Blast radius of '{}' ===\nRisk: Low (0 functions, 0 files)\n\nNo other functions call this.", function));
        }
        return Ok(String::new());
    }

    // Collect unique files with per-file function counts
    let mut file_func_counts: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    for (name, file, _) in &results {
        file_func_counts.entry(file.as_str()).or_default().push(name.as_str());
    }
    // Include the function's own file
    if let Some(fe) = func_entity {
        file_func_counts.entry(fe.file.as_str()).or_default();
    }

    let total_funcs = results.len();
    let total_files = file_func_counts.len();

    // Risk assessment
    let risk = if total_funcs >= 10 || total_files >= 5 {
        "Critical"
    } else if total_funcs >= 5 || total_files >= 3 {
        "High"
    } else if total_funcs >= 2 || total_files >= 2 {
        "Medium"
    } else {
        "Low"
    };

    let mut lines = vec![format!("=== Blast radius of '{}' ===", function)];
    lines.push(format!("Risk: {} ({} functions, {} files)", risk, total_funcs, total_files));
    lines.push(String::new());

    // Call chain
    lines.push("Call chain:".to_string());
    // Show as arrow chain: function -> caller1 -> caller2
    let mut chain_parts = vec![function.to_string()];
    for (name, _, _) in results.iter().take(8) {
        if !chain_parts.contains(name) {
            chain_parts.push(name.clone());
        }
    }
    lines.push(format!("  {}", chain_parts.join(" -> ")));
    if results.len() > 8 {
        lines.push(format!("  ... and {} more in chain", results.len() - 8));
    }
    lines.push(String::new());

    // Callees: what this function calls (downstream)
    let func_file = func_entity.map(|e| e.file.as_str()).unwrap_or("");
    let callees: Vec<_> = parse_result.call_sites.iter()
        .filter(|cs| cs.caller_name == function && (func_file.is_empty() || cs.caller_file == func_file || cs.caller_file.ends_with(&format!("/{}", func_file)) || func_file.ends_with(&format!("/{}", cs.caller_file))))
        .collect();
    if !callees.is_empty() {
        // Filter out common stdlib/builtin calls that add noise
        let stdlib_skip = ["trim", "is_empty", "starts_with", "ends_with", "contains",
            "ok", "err", "map", "and_then", "unwrap", "unwrap_or", "unwrap_or_default",
            "unwrap_or_else", "expect", "clone", "to_string", "to_lowercase", "to_uppercase",
            "as_str", "as_ref", "into", "from", "new", "default", "len", "push", "pop",
            "insert", "remove", "get", "set", "iter", "collect", "filter", "find",
            "any", "all", "or", "and", "not", "join", "split", "replace", "format",
            "println", "eprintln", "write", "writeln", "read", "open", "close",
            "parse", "to_owned", "borrow", "deref", "display", "debug", "hash",
            "cmp", "eq", "ne", "lt", "gt", "le", "ge", "add", "sub", "mul", "div",
            "captures", "captures_iter", "match_indices"];
        let mut seen_callees = std::collections::HashSet::new();
        let mut callee_entries: Vec<(String, String, Option<usize>)> = Vec::new();
        for cs in &callees {
            if stdlib_skip.contains(&cs.callee_name.as_str()) { continue; }
            if seen_callees.insert(&cs.callee_name) {
                // Look up line number from entities
                let line_num = parse_result.entities.iter()
                    .find(|e| e.kind == "function" && e.name == cs.callee_name)
                    .map(|e| (e.file.clone(), e.line));
                let (callee_file, ln) = match line_num {
                    Some((f, l)) => (f, Some(l)),
                    None => (String::new(), None),
                };
                callee_entries.push((cs.callee_name.clone(), callee_file, ln));
            }
        }
        lines.push(format!("Calls (downstream, {} direct):", callee_entries.len()));
        let mut total_shown = 0;
        for (name, file, ln) in &callee_entries {
            if total_shown >= 10 { break; }
            let short_file = file.rsplit('/').next().unwrap_or(file);
            let loc = match ln {
                Some(l) => format!("{}:{}", short_file, l),
                None => short_file.to_string(),
            };
            if loc.is_empty() {
                lines.push(format!("  {}", name));
            } else {
                lines.push(format!("  {}  ({})", name, loc));
            }
            total_shown += 1;

            // Depth-2: show callees of this callee
            if total_shown < 10 {
                let sub_callees: Vec<_> = parse_result.call_sites.iter()
                    .filter(|cs| cs.caller_name == *name && !stdlib_skip.contains(&cs.callee_name.as_str()))
                    .filter(|cs| cs.callee_name != function) // avoid circular
                    .collect();
                let mut sub_seen = std::collections::HashSet::new();
                for cs in &sub_callees {
                    if total_shown >= 10 { break; }
                    if sub_seen.insert(&cs.callee_name) {
                        let sub_ln = parse_result.entities.iter()
                            .find(|e| e.kind == "function" && e.name == cs.callee_name)
                            .map(|e| format!("{}:{}", e.file.rsplit('/').next().unwrap_or(&e.file), e.line));
                        let sub_loc = sub_ln.unwrap_or_default();
                        if sub_loc.is_empty() {
                            lines.push(format!("    {}", cs.callee_name));
                        } else {
                            lines.push(format!("    {}  ({})", cs.callee_name, sub_loc));
                        }
                        total_shown += 1;
                    }
                }
            }
        }
        if callee_entries.len() > 10 {
            lines.push(format!("  ... and {} more", callee_entries.len() - 10));
        }
        lines.push(String::new());
    }

    // Files affected with per-file details and co-change commits
    lines.push("Files affected:".to_string());
    let repo_path = std::env::current_dir().unwrap_or_default();
    for (file, funcs) in &file_func_counts {
        // Count recent commits touching this file (last 30 days)
        let commit_count = std::process::Command::new("git")
            .args(["log", "--since=30 days ago", "--format=%H", "--", file])
            .current_dir(&repo_path)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.is_empty()).count())
            .unwrap_or(0);
        let commit_note = if commit_count > 0 {
            format!(", {} commits in 30d", commit_count)
        } else {
            String::new()
        };
        lines.push(format!("  {} ({} functions{})", file, funcs.len(), commit_note));
    }
    lines.push(String::new());

    // Search for test files that reference the function AND related function names in the chain
    let mut search_names: Vec<&str> = vec![function];
    for (name, _, _) in &results {
        if !search_names.contains(&name.as_str()) {
            search_names.push(name.as_str());
        }
    }

    let mut all_test_hits: Vec<String> = Vec::new();
    let mut test_files_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for search_name in &search_names {
        let grep_output = std::process::Command::new("grep")
            .args([
                "-rn",
                "--include=test_*.py", "--include=*_test.rs", "--include=*.test.ts",
                "--include=*_spec.ts", "--include=*_test.go", "--include=*.test.js",
                "--include=*_test.py", "--include=*.spec.ts", "--include=*.spec.js",
                "--exclude-dir=node_modules", "--exclude-dir=target",
                "--exclude-dir=.git", "--exclude-dir=__pycache__",
                search_name, ".",
            ])
            .current_dir(&repo_path)
            .output();

        if let Ok(output) = grep_output {
            let grep_text = String::from_utf8_lossy(&output.stdout).to_string();
            for line in grep_text.lines().filter(|l| !l.is_empty()) {
                let line = line.strip_prefix("./").unwrap_or(line);
                // Track unique test files
                if let Some((file_part, _)) = line.split_once(':') {
                    test_files_seen.insert(file_part.to_string());
                }
                if let Some((loc, content)) = line.split_once(':').and_then(|(file, rest)| {
                    rest.split_once(':').map(|(ln, content)| (format!("{}:{}", file, ln), content.trim().to_string()))
                }) {
                    let entry = format!("  {} — {}", loc, content.chars().take(80).collect::<String>());
                    if !all_test_hits.contains(&entry) {
                        all_test_hits.push(entry);
                    }
                }
            }
        }
    }

    lines.push("Tests:".to_string());
    if all_test_hits.is_empty() {
        lines.push("  No test files reference affected functions".to_string());
        // Suggest a test file name based on the source file
        if let Some(fe) = func_entity {
            let source_file = &fe.file;
            // Derive test file suggestion from source path
            let test_file = if source_file.ends_with(".rs") {
                // Rust: src/foo/bar.rs -> tests/test_bar.rs
                let stem = std::path::Path::new(source_file)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy();
                let parent = std::path::Path::new(source_file)
                    .parent()
                    .and_then(|p| p.to_str())
                    .unwrap_or("");
                if parent.starts_with("src") {
                    format!("tests/test_{}.rs", stem)
                } else {
                    format!("{}/test_{}.rs", parent, stem)
                }
            } else if source_file.ends_with(".py") {
                // Python: foo/bar.py -> tests/test_bar.py
                let stem = std::path::Path::new(source_file)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy();
                format!("tests/test_{}.py", stem)
            } else if source_file.ends_with(".ts") || source_file.ends_with(".js") {
                // TS/JS: foo/bar.ts -> foo/bar.test.ts
                let ext = if source_file.ends_with(".ts") { "ts" } else { "js" };
                let without_ext = &source_file[..source_file.len() - ext.len() - 1];
                format!("{}.test.{}", without_ext, ext)
            } else {
                // Generic fallback
                let stem = std::path::Path::new(source_file)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy();
                format!("tests/test_{}", stem)
            };
            // Collect function names for the suggestion
            let suggest_funcs: Vec<&str> = std::iter::once(function)
                .chain(results.iter().take(3).map(|(n, _, _)| n.as_str()))
                .collect();
            let unique_funcs: Vec<&str> = {
                let mut seen = std::collections::HashSet::new();
                suggest_funcs.into_iter().filter(|f| seen.insert(*f)).collect()
            };
            lines.push(format!("  Suggestion: create {} with tests for {}",
                test_file, unique_funcs.join(", ")));
        }
    } else {
        for hit in all_test_hits.iter().take(10) {
            lines.push(hit.clone());
        }
        if all_test_hits.len() > 10 {
            lines.push(format!("  ... and {} more", all_test_hits.len() - 10));
        }
        lines.push(format!("  ({} test files reference affected functions)", test_files_seen.len()));
    }
    lines.push(String::new());

    // Co-change partners: files that commonly change together with the target file
    if let Some(fe) = func_entity {
        let commit_hashes = std::process::Command::new("git")
            .args(["log", "--format=%H", "-20", "--", &fe.file])
            .current_dir(&repo_path)
            .output();
        if let Ok(o) = commit_hashes {
            let hash_output = String::from_utf8_lossy(&o.stdout).to_string();
            let hash_lines: Vec<&str> = hash_output.lines().filter(|l| !l.is_empty()).collect();

            let mut co_change_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            for hash in hash_lines.iter().take(20) {
                let files_output = std::process::Command::new("git")
                    .args(["diff-tree", "--no-commit-id", "--name-only", "-r", hash])
                    .current_dir(&repo_path)
                    .output();
                if let Ok(fo) = files_output {
                    let files_text = String::from_utf8_lossy(&fo.stdout).to_string();
                    for file_line in files_text.lines().filter(|l| !l.is_empty()) {
                        if file_line != fe.file {
                            *co_change_counts.entry(file_line.to_string()).or_insert(0) += 1;
                        }
                    }
                }
            }

            if !co_change_counts.is_empty() {
                let mut sorted: Vec<_> = co_change_counts.into_iter().collect();
                sorted.sort_by(|a, b| b.1.cmp(&a.1));
                lines.push("Co-change partners (files that change together):".to_string());
                for (file, count) in sorted.iter().take(3) {
                    let pct = (*count as f32 / hash_lines.len() as f32 * 100.0) as usize;
                    lines.push(format!("  {} ({}% of commits)", file, pct));
                }
                lines.push(String::new());
            }
        }
    }

    // Last changed info
    if let Some(fe) = func_entity {
        let last_change = std::process::Command::new("git")
            .args(["log", "-1", "--format=%ar by %an", "--", &fe.file])
            .current_dir(&repo_path)
            .output();
        if let Ok(o) = last_change {
            let info = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !info.is_empty() {
                lines.push(format!("Last changed: {}", info));
            }
        }
    }

    Ok(lines.join("\n"))
}

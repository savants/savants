//! Fusion Query Engine (Phase 4) — combines vector similarity, graph traversal,
//! and temporal filtering into a single ranked result set.
//!
//! Instead of running 3 separate searches, `fused_search` produces one merged,
//! deduplicated, scored list that includes:
//! - Embedding cosine similarity
//! - Graph context (callers/callees)
//! - Git recency boost
//! - Community membership

use crate::code_graph::CodeGraph;
use crate::code_parser::{CallSite, ParseResult};
use crate::embedding_store::EmbeddingStore;
use crate::embeddings::EmbeddingEngine;
use colored::*;
use std::collections::{HashMap, HashSet};

/// Input query for fused search.
pub struct FusionQuery {
    pub text: String,
    pub repo: String,
    pub max_results: usize,
    pub include_docs: bool,
    pub since: Option<String>, // temporal filter: "7d", "2w", "1m"
}

/// A single fused search result.
pub struct FusionResult {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub kind: String,
    pub score: f32,
    pub snippet: String,
    pub graph_context: String,
    pub recency: Option<String>,
}

/// Run a fused search combining vectors + graph + temporal signals.
pub fn fused_search(query: &FusionQuery) -> Result<Vec<FusionResult>, String> {
    let repo = &query.repo;

    // Load required data
    let store = EmbeddingStore::load(repo)
        .map_err(|e| format!("No local index for '{}': {}", repo, e))?;
    let parse_result = load_parse_index(repo)?;
    let repo_path = std::env::current_dir().unwrap_or_default();

    // -- Phase 1: Vector search --
    let mut engine = EmbeddingEngine::new()
        .map_err(|e| format!("Embedding engine: {}", e))?;
    let query_emb = engine.embed_one(&query.text)
        .map_err(|e| format!("Embedding query: {}", e))?;

    // Grab more candidates than needed so we can re-rank
    let candidate_count = query.max_results * 3;
    let semantic_hits = store.search(&query_emb, candidate_count);

    // Also do name matching against the parse index
    let name_hits = name_match_candidates(&query.text, &parse_result, 10);

    // -- Phase 2: Build git recency cache (batch for all candidate files) --
    let mut candidate_files: HashSet<String> = HashSet::new();
    for (idx, _) in &semantic_hits {
        if let Some(entry) = store.entries.get(*idx) {
            candidate_files.insert(entry.file.clone());
        }
    }
    for (_, file, _, _) in &name_hits {
        candidate_files.insert(file.clone());
    }
    let recency_cache = build_recency_cache(&candidate_files, &repo_path);

    // -- Phase 3: Build graph context --
    let caller_counts = build_caller_counts(&parse_result);
    let max_callers = caller_counts.values().copied().max().unwrap_or(1) as f32;

    // Parse the --since filter into a cutoff
    let since_cutoff_days = query.since.as_ref().and_then(|s| parse_since_to_days(s));

    // -- Phase 4: Merge and score --
    let mut seen: HashSet<String> = HashSet::new(); // dedup key: "file:name"
    let mut results: Vec<FusionResult> = Vec::new();

    // Process name-matched candidates first (they get a similarity floor of 0.8)
    for (name, file, line, name_score) in &name_hits {
        let key = format!("{}:{}", file, name);
        if !seen.insert(key) { continue; }

        let recency_info = recency_cache.get(file.as_str());
        let recency_boost = recency_info
            .map(|(days, _)| recency_score(*days))
            .unwrap_or(0.6);

        // Apply temporal filter
        if let Some(cutoff) = since_cutoff_days {
            if let Some((days, _)) = recency_info {
                if *days > cutoff { continue; }
            } else {
                continue; // no git info, skip when filtering
            }
        }

        let graph_centrality = caller_counts.get(name.as_str())
            .map(|&c| c as f32 / max_callers)
            .unwrap_or(0.0);
        let similarity = name_score.max(0.8); // name matches get at least 0.8

        let final_score = similarity * 0.6 + recency_boost * 0.2 + graph_centrality * 0.2;

        let graph_ctx = build_graph_context_for(name, file, &parse_result);
        let recency_label = recency_info.map(|(_, label)| label.clone());
        let snippet = get_snippet(name, file, *line, &parse_result, &repo_path);

        let kind = parse_result.entities.iter()
            .find(|e| e.name == *name && e.file == *file)
            .map(|e| e.kind.clone())
            .unwrap_or_else(|| "function".to_string());

        results.push(FusionResult {
            name: name.clone(),
            file: file.clone(),
            line: *line,
            kind,
            score: final_score,
            snippet,
            graph_context: graph_ctx,
            recency: recency_label,
        });
    }

    // Process semantic hits
    for (idx, sim_score) in &semantic_hits {
        if *sim_score < 0.30 { continue; } // noise floor

        let entry = match store.entries.get(*idx) {
            Some(e) => e,
            None => continue,
        };

        let key = format!("{}:{}", entry.file, entry.name);
        if !seen.insert(key) { continue; }

        let recency_info = recency_cache.get(entry.file.as_str());
        let recency_boost = recency_info
            .map(|(days, _)| recency_score(*days))
            .unwrap_or(0.6);

        // Apply temporal filter
        if let Some(cutoff) = since_cutoff_days {
            if let Some((days, _)) = recency_info {
                if *days > cutoff { continue; }
            } else {
                continue;
            }
        }

        let graph_centrality = caller_counts.get(entry.name.as_str())
            .map(|&c| c as f32 / max_callers)
            .unwrap_or(0.0);

        let final_score = sim_score * 0.6 + recency_boost * 0.2 + graph_centrality * 0.2;

        let graph_ctx = build_graph_context_for(&entry.name, &entry.file, &parse_result);
        let recency_label = recency_info.map(|(_, label)| label.clone());
        let snippet = get_snippet(&entry.name, &entry.file, entry.line as usize, &parse_result, &repo_path);

        let kind = match entry.kind {
            1 => "class".to_string(),
            2 => "interface".to_string(),
            _ => "function".to_string(),
        };

        results.push(FusionResult {
            name: entry.name.clone(),
            file: entry.file.clone(),
            line: entry.line as usize,
            kind,
            score: final_score,
            snippet,
            graph_context: graph_ctx,
            recency: recency_label,
        });
    }

    // -- Phase 5: Include doc results if requested --
    if query.include_docs {
        let doc_store_name = format!("{}-docs", repo);
        if EmbeddingStore::exists(&doc_store_name) {
            if let Ok(doc_store) = EmbeddingStore::load(&doc_store_name) {
                let doc_hits = doc_store.search(&query_emb, 3);
                for (idx, sim_score) in &doc_hits {
                    if *sim_score < 0.35 { continue; }
                    if let Some(entry) = doc_store.entries.get(*idx) {
                        let key = format!("doc:{}:{}", entry.file, entry.name);
                        if !seen.insert(key) { continue; }

                        let final_score = sim_score * 0.6 + 0.5 * 0.2; // docs get neutral recency/centrality
                        results.push(FusionResult {
                            name: entry.name.clone(),
                            file: entry.file.clone(),
                            line: entry.line as usize,
                            kind: "doc_section".to_string(),
                            score: final_score,
                            snippet: String::new(),
                            graph_context: String::new(),
                            recency: None,
                        });
                    }
                }
            }
        }
    }

    // Sort by final score descending
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(query.max_results);

    Ok(results)
}

/// Format fused results for terminal display.
pub fn format_results(query_text: &str, results: &[FusionResult]) -> String {
    if results.is_empty() {
        return format!("No results for '{}'", query_text);
    }

    let mut lines = vec![format!("=== Results for '{}' ===", query_text)];
    lines.push(String::new());

    for r in results {
        // Score + name + location + kind
        lines.push(format!(
            "  {:.2}  {:<24} {}:{}  {}",
            r.score,
            r.name.cyan().to_string(),
            r.file,
            r.line,
            r.kind.dimmed().to_string(),
        ));

        // Snippet
        if !r.snippet.is_empty() {
            let preview: String = r.snippet.chars().take(80).collect();
            lines.push(format!("        \"{}\"", preview));
        }

        // Graph context
        if !r.graph_context.is_empty() {
            lines.push(format!("        {}", r.graph_context));
        }

        // Recency
        if let Some(ref rec) = r.recency {
            lines.push(format!("        Modified: {}", rec));
        }

        lines.push(String::new());
    }

    lines.join("\n")
}

// ---- Internal helpers ----

/// Load the cached parse result from disk.
fn load_parse_index(repo: &str) -> Result<ParseResult, String> {
    let path = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".savants")
        .join("code-index")
        .join(format!("{}.json", repo));
    let data = std::fs::read_to_string(&path)
        .map_err(|_| format!("No code index for '{}'. Run 'savants up' first.", repo))?;
    serde_json::from_str(&data)
        .map_err(|e| format!("Corrupt index for '{}': {}", repo, e))
}

/// Name-match candidates: tokenize query and match against entity names.
/// Returns (name, file, line, score).
fn name_match_candidates(query: &str, pr: &ParseResult, limit: usize) -> Vec<(String, String, usize, f32)> {
    let query_lower = query.to_lowercase();
    let query_words: Vec<&str> = query_lower.split_whitespace()
        .filter(|w| w.len() >= 3)
        .collect();
    if query_words.is_empty() { return vec![]; }

    let mut hits: Vec<(String, String, usize, f32)> = Vec::new();

    for entity in &pr.entities {
        if entity.kind == "import" { continue; }
        if entity.name.len() <= 2 { continue; }

        let name_lower = entity.name.to_lowercase();
        let name_tokens = tokenize_name(&name_lower);

        let mut match_count = 0;
        for qw in &query_words {
            for nt in &name_tokens {
                if words_similar(nt, qw) {
                    match_count += 1;
                    break;
                }
            }
        }

        let ratio = match_count as f32 / query_words.len() as f32;
        if ratio >= 0.5 {
            hits.push((entity.name.clone(), entity.file.clone(), entity.line, ratio));
        }
    }

    hits.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal)
        .then(a.0.len().cmp(&b.0.len())));
    hits.truncate(limit);
    hits
}

/// Tokenize camelCase/snake_case into words.
fn tokenize_name(name: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in name.chars() {
        if ch == '_' || ch == '-' {
            if !current.is_empty() {
                tokens.push(current.to_lowercase());
                current.clear();
            }
        } else if ch.is_uppercase() && !current.is_empty() {
            tokens.push(current.to_lowercase());
            current.clear();
            current.push(ch);
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.push(current.to_lowercase());
    }
    tokens
}

/// Check if two words are similar (exact, substring, or stem).
fn words_similar(a: &str, b: &str) -> bool {
    if a == b { return true; }
    if a.contains(b) || b.contains(a) { return true; }
    if a.len() >= 5 && b.len() >= 5 {
        let prefix_len = std::cmp::min(std::cmp::min(a.len(), b.len()), 6);
        if a[..prefix_len] == b[..prefix_len] { return true; }
    }
    false
}

/// Build a map of function_name -> caller_count from the parse result.
fn build_caller_counts(pr: &ParseResult) -> HashMap<&str, usize> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for cs in &pr.call_sites {
        *counts.entry(cs.callee_name.as_str()).or_insert(0) += 1;
    }
    counts
}

/// Build graph context string for a function: "Called by: X, Y | Calls: A, B"
fn build_graph_context_for(name: &str, file: &str, pr: &ParseResult) -> String {
    let mut callers: Vec<&str> = Vec::new();
    let mut callees: Vec<&str> = Vec::new();
    let mut seen_callers: HashSet<&str> = HashSet::new();
    let mut seen_callees: HashSet<&str> = HashSet::new();

    for cs in &pr.call_sites {
        if cs.callee_name == name {
            if seen_callers.insert(cs.caller_name.as_str()) {
                callers.push(cs.caller_name.as_str());
            }
        }
        if cs.caller_name == name && (cs.caller_file == file || file.is_empty()) {
            if seen_callees.insert(cs.callee_name.as_str()) {
                callees.push(cs.callee_name.as_str());
            }
        }
    }

    // Filter out stdlib noise from callees
    let stdlib_skip = [
        "trim", "is_empty", "contains", "ok", "err", "map", "unwrap",
        "clone", "to_string", "len", "push", "insert", "get", "iter",
        "collect", "filter", "find", "join", "split", "replace", "format",
        "println", "eprintln", "write", "read", "parse", "new", "default",
    ];
    callees.retain(|c| !stdlib_skip.contains(c));

    let mut parts = Vec::new();
    if !callers.is_empty() {
        let display: Vec<&str> = callers.iter().take(3).copied().collect();
        let extra = if callers.len() > 3 { format!(", +{}", callers.len() - 3) } else { String::new() };
        parts.push(format!("Called by: {}{}", display.join(", "), extra));
    }
    if !callees.is_empty() {
        let display: Vec<&str> = callees.iter().take(3).copied().collect();
        let extra = if callees.len() > 3 { format!(", +{}", callees.len() - 3) } else { String::new() };
        parts.push(format!("Calls: {}{}", display.join(", "), extra));
    }

    if parts.is_empty() {
        String::new()
    } else {
        parts.join("  |  ")
    }
}

/// Batch git-log recency for a set of files.
/// Returns map of file -> (days_ago, human_label).
/// Uses a single git log call for efficiency.
fn build_recency_cache(
    files: &HashSet<String>,
    repo_path: &std::path::Path,
) -> HashMap<String, (u64, String)> {
    let mut cache: HashMap<String, (u64, String)> = HashMap::new();

    // Batch: get last commit date for all files at once using git log.
    // We run one git log per file but cap at 50 to stay fast.
    let files_vec: Vec<&String> = files.iter().take(50).collect();

    for file in files_vec {
        let output = std::process::Command::new("git")
            .args(["log", "-1", "--format=%ar", "--", file])
            .current_dir(repo_path)
            .output();

        if let Ok(o) = output {
            let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !text.is_empty() {
                let days = parse_relative_to_days(&text);
                cache.insert(file.clone(), (days, text));
            }
        }
    }

    cache
}

/// Parse git's relative date ("2 days ago", "3 weeks ago") into approximate days.
fn parse_relative_to_days(relative: &str) -> u64 {
    let parts: Vec<&str> = relative.split_whitespace().collect();
    if parts.len() < 3 { return 365; } // unknown -> old

    let count: u64 = parts[0].parse().unwrap_or(1);
    let unit = parts[1];

    if unit.starts_with("second") || unit.starts_with("minute") || unit.starts_with("hour") {
        0 // today
    } else if unit.starts_with("day") {
        count
    } else if unit.starts_with("week") {
        count * 7
    } else if unit.starts_with("month") {
        count * 30
    } else if unit.starts_with("year") {
        count * 365
    } else {
        365
    }
}

/// Convert days-ago into a recency score (0.0 - 1.0).
fn recency_score(days: u64) -> f32 {
    if days <= 7 {
        1.0
    } else if days <= 30 {
        0.8
    } else {
        0.6
    }
}

/// Parse a --since flag value like "7d", "2w", "1m" into a cutoff in days.
fn parse_since_to_days(since: &str) -> Option<u64> {
    let s = since.trim();
    if s.is_empty() { return None; }

    let last = s.chars().last()?;
    let num_part = &s[..s.len() - 1];
    let num: u64 = num_part.parse().ok()?;

    match last {
        'd' => Some(num),
        'w' => Some(num * 7),
        'm' => Some(num * 30),
        'y' => Some(num * 365),
        _ => None,
    }
}

/// Get a snippet for a function: first meaningful line of its body.
fn get_snippet(
    name: &str,
    file: &str,
    line: usize,
    pr: &ParseResult,
    repo_path: &std::path::Path,
) -> String {
    // Try to get from parse result body first
    if let Some(entity) = pr.entities.iter().find(|e| e.name == name && e.file == file) {
        if !entity.body.is_empty() {
            // Return a clean one-liner from the body
            let first_line = entity.body.lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim();
            let snippet: String = first_line.chars().take(100).collect();
            if !snippet.is_empty() {
                return snippet;
            }
        }
    }

    // Fall back to reading the source file
    let file_path = repo_path.join(file);
    if let Ok(content) = std::fs::read_to_string(&file_path) {
        if let Some(src_line) = content.lines().nth(line.saturating_sub(1)) {
            let trimmed = src_line.trim();
            return trimmed.chars().take(100).collect();
        }
    }

    String::new()
}

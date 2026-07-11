//! Predictive Pre-fetch Cache (Phase 5) — when a developer opens or modifies
//! a file, pre-compute the top questions about that file and cache the answers.
//! When they ask, return the cached answer with ~0ms latency.
//!
//! Answers are generated from code index + doc index + git log. No LLM needed.
//! Cache is in-memory only (lost on restart, rebuilt on watch).
//! LRU eviction: keeps max 50 files cached.

use colored::*;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use crate::code_graph;
use crate::code_parser::{ParseResult, ParsedEntity};

/// Maximum number of files to keep in cache before LRU eviction.
const MAX_CACHE_SIZE: usize = 50;

/// A pre-computed answer to a common question about a file.
pub struct PrecomputedAnswer {
    pub question: String,
    pub answer: String,
    pub confidence: f32,
}

/// Cached entry for a single file.
pub struct CacheEntry {
    pub file: PathBuf,
    pub questions: Vec<PrecomputedAnswer>,
    pub computed_at: Instant,
}

/// The predictive pre-fetch cache.
pub struct PrefetchCache {
    /// Cached answers keyed by file path (relative to repo root).
    entries: HashMap<String, CacheEntry>,
    /// LRU ordering: most recently accessed at the back.
    lru_order: VecDeque<String>,
    /// Repo name for loading indexes.
    repo_name: String,
    /// Repo root path for git commands.
    repo_path: String,
}

impl PrefetchCache {
    pub fn new(repo_name: &str, repo_path: &str) -> Self {
        Self {
            entries: HashMap::new(),
            lru_order: VecDeque::new(),
            repo_name: repo_name.to_string(),
            repo_path: repo_path.to_string(),
        }
    }

    /// Pre-compute answers for a specific file. Returns the number of answers computed
    /// and the time it took in milliseconds.
    pub fn prefetch_for_file(&mut self, file_path: &str) -> (usize, u128) {
        let start = Instant::now();

        // Normalize the path to be relative to repo root
        let rel_path = self.normalize_path(file_path);

        // Load the code index
        let parse_result = match load_parse_index(&self.repo_name) {
            Ok(pr) => pr,
            Err(_) => return (0, start.elapsed().as_millis()),
        };

        let mut answers: Vec<PrecomputedAnswer> = Vec::new();

        // 1. "What does this file do?"
        if let Some(answer) = self.compute_file_summary(&rel_path, &parse_result) {
            answers.push(answer);
        }

        // 2. "What functions are in this file?"
        if let Some(answer) = self.compute_file_skeleton(&rel_path, &parse_result) {
            answers.push(answer);
        }

        // 3. "Who calls functions in this file?"
        if let Some(answer) = self.compute_file_callers(&rel_path, &parse_result) {
            answers.push(answer);
        }

        // 4. "What has changed recently?"
        if let Some(answer) = self.compute_recent_changes(&rel_path) {
            answers.push(answer);
        }

        // 5. "What tests cover this file?"
        if let Some(answer) = self.compute_test_coverage(&rel_path, &parse_result) {
            answers.push(answer);
        }

        let count = answers.len();
        let ms = start.elapsed().as_millis();

        // Store in cache
        let entry = CacheEntry {
            file: PathBuf::from(&rel_path),
            questions: answers,
            computed_at: Instant::now(),
        };
        self.insert(rel_path, entry);

        (count, ms)
    }

    /// Check cache for a query. Returns a pre-computed answer if one matches.
    pub fn lookup(&self, query: &str, current_file: Option<&str>) -> Option<&PrecomputedAnswer> {
        let query_lower = query.to_lowercase();

        // If a current file is specified, check that file's cache first
        if let Some(file) = current_file {
            let rel_path = self.normalize_path(file);
            if let Some(entry) = self.entries.get(&rel_path) {
                return self.find_matching_answer(&query_lower, &entry.questions);
            }
        }

        // Otherwise check all cached files for a match
        for entry in self.entries.values() {
            if let Some(answer) = self.find_matching_answer(&query_lower, &entry.questions) {
                return Some(answer);
            }
        }

        None
    }

    /// Invalidate cache for a file (called by differential indexer on file change).
    pub fn invalidate(&mut self, file_path: &str) {
        let rel_path = self.normalize_path(file_path);
        self.entries.remove(&rel_path);
        self.lru_order.retain(|p| *p != rel_path);
    }

    /// Number of files currently cached.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    // ---- Internal helpers ----

    /// Normalize a file path to be relative to the repo root.
    fn normalize_path(&self, file_path: &str) -> String {
        let path = PathBuf::from(file_path);
        let repo = PathBuf::from(&self.repo_path);
        if let Ok(rel) = path.strip_prefix(&repo) {
            rel.to_string_lossy().replace('\\', "/")
        } else {
            file_path.replace('\\', "/").to_string()
        }
    }

    /// Insert an entry into the cache with LRU eviction.
    fn insert(&mut self, key: String, entry: CacheEntry) {
        // Remove from LRU order if already present (will re-add at back)
        self.lru_order.retain(|p| *p != key);

        // Evict oldest if at capacity
        while self.entries.len() >= MAX_CACHE_SIZE {
            if let Some(oldest) = self.lru_order.pop_front() {
                self.entries.remove(&oldest);
            } else {
                break;
            }
        }

        self.lru_order.push_back(key.clone());
        self.entries.insert(key, entry);
    }

    /// Find the best matching pre-computed answer for a query.
    fn find_matching_answer<'a>(
        &self,
        query_lower: &str,
        answers: &'a [PrecomputedAnswer],
    ) -> Option<&'a PrecomputedAnswer> {
        // Map query patterns to question types
        let patterns: &[(&[&str], &str)] = &[
            (&["what does", "what is", "purpose", "summary", "overview", "about"], "file_summary"),
            (&["functions", "methods", "skeleton", "structure", "what functions"], "file_skeleton"),
            (&["who calls", "callers", "called by", "references", "used by"], "file_callers"),
            (&["changed", "recent", "history", "commits", "modified", "git log"], "recent_changes"),
            (&["test", "tests", "coverage", "tested", "test files"], "test_coverage"),
        ];

        for (keywords, question_type) in patterns {
            if keywords.iter().any(|kw| query_lower.contains(kw)) {
                // Find the answer matching this question type
                for answer in answers {
                    if answer.question.contains(question_type) {
                        return Some(answer);
                    }
                }
            }
        }

        // Also try matching by file name in the query
        for answer in answers {
            let file_str = answer.question.split('|').nth(1).unwrap_or("");
            if !file_str.is_empty() && query_lower.contains(&file_str.to_lowercase()) {
                return Some(answer);
            }
        }

        None
    }

    /// Compute "What does this file do?" answer.
    fn compute_file_summary(
        &self,
        rel_path: &str,
        parse_result: &ParseResult,
    ) -> Option<PrecomputedAnswer> {
        let file_entities: Vec<&ParsedEntity> = parse_result
            .entities
            .iter()
            .filter(|e| e.file == rel_path || e.file.ends_with(&format!("/{}", rel_path)))
            .collect();

        if file_entities.is_empty() {
            return None;
        }

        let func_count = file_entities.iter().filter(|e| e.kind == "function").count();
        let class_count = file_entities
            .iter()
            .filter(|e| e.kind == "class" || e.kind == "interface")
            .count();

        // Count total lines
        let max_line = file_entities
            .iter()
            .map(|e| e.end_line)
            .max()
            .unwrap_or(0);

        // Key functions (top 5 by size)
        let mut functions: Vec<&ParsedEntity> = file_entities
            .iter()
            .filter(|e| e.kind == "function")
            .copied()
            .collect();
        functions.sort_by(|a, b| {
            (b.end_line - b.line).cmp(&(a.end_line - a.line))
        });

        let key_func_names: Vec<String> = functions
            .iter()
            .take(5)
            .map(|f| f.name.clone())
            .collect();

        // Check community membership
        let community_info = if let Ok(communities) = code_graph::load_communities(&self.repo_name)
        {
            communities
                .iter()
                .find(|c| c.files.iter().any(|f| f == rel_path || f.ends_with(&format!("/{}", rel_path))))
                .map(|c| {
                    format!(
                        "Part of community: {} ({} functions).",
                        c.name,
                        c.functions.len()
                    )
                })
                .unwrap_or_default()
        } else {
            String::new()
        };

        // Get last modified info from git
        let last_modified = self.git_last_modified(rel_path);

        let mut answer_parts = Vec::new();

        // File name as short summary
        let short_name = rel_path.rsplit('/').next().unwrap_or(rel_path);
        answer_parts.push(format!(
            "{} (~{} lines, {} functions{}).",
            short_name,
            max_line,
            func_count,
            if class_count > 0 {
                format!(", {} classes/types", class_count)
            } else {
                String::new()
            }
        ));

        if !key_func_names.is_empty() {
            answer_parts.push(format!(
                "Key functions: {}.",
                key_func_names.join(", ")
            ));
        }

        if !community_info.is_empty() {
            answer_parts.push(community_info);
        }

        if !last_modified.is_empty() {
            answer_parts.push(format!("Last modified: {}.", last_modified));
        }

        Some(PrecomputedAnswer {
            question: format!("file_summary|{}", rel_path),
            answer: answer_parts.join("\n"),
            confidence: 0.85,
        })
    }

    /// Compute "What functions are in this file?" answer.
    fn compute_file_skeleton(
        &self,
        rel_path: &str,
        parse_result: &ParseResult,
    ) -> Option<PrecomputedAnswer> {
        let file_entities: Vec<&ParsedEntity> = parse_result
            .entities
            .iter()
            .filter(|e| {
                (e.file == rel_path || e.file.ends_with(&format!("/{}", rel_path)))
                    && e.kind != "import"
            })
            .collect();

        if file_entities.is_empty() {
            return None;
        }

        let mut lines = Vec::new();
        lines.push(format!("=== {} ===", rel_path));

        let classes: Vec<_> = file_entities
            .iter()
            .filter(|e| e.kind == "class")
            .collect();
        let interfaces: Vec<_> = file_entities
            .iter()
            .filter(|e| e.kind == "interface")
            .collect();
        let functions: Vec<_> = file_entities
            .iter()
            .filter(|e| e.kind == "function")
            .collect();

        if !classes.is_empty() {
            lines.push("Classes:".to_string());
            for e in &classes {
                lines.push(format!(
                    "  class {} (lines {}-{})",
                    e.name, e.line, e.end_line
                ));
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
            for e in &functions {
                let params_str = if e.params.is_empty() {
                    String::new()
                } else {
                    format!("({})", e.params.join(", "))
                };
                let line_count = e.end_line.saturating_sub(e.line) + 1;
                lines.push(format!(
                    "  fn {}{}  L{}-{} ({} lines)",
                    e.name, params_str, e.line, e.end_line, line_count
                ));
            }
        }

        lines.push(format!(
            "\n{} functions, {} classes, {} types/interfaces",
            functions.len(),
            classes.len(),
            interfaces.len()
        ));

        Some(PrecomputedAnswer {
            question: format!("file_skeleton|{}", rel_path),
            answer: lines.join("\n"),
            confidence: 0.95,
        })
    }

    /// Compute "Who calls functions in this file?" answer.
    fn compute_file_callers(
        &self,
        rel_path: &str,
        parse_result: &ParseResult,
    ) -> Option<PrecomputedAnswer> {
        let file_functions: Vec<&str> = parse_result
            .entities
            .iter()
            .filter(|e| {
                e.kind == "function"
                    && (e.file == rel_path
                        || e.file.ends_with(&format!("/{}", rel_path)))
            })
            .map(|e| e.name.as_str())
            .collect();

        if file_functions.is_empty() {
            return None;
        }

        let mut lines = Vec::new();
        lines.push(format!("=== Callers of functions in {} ===", rel_path));

        let mut has_callers = false;
        for func_name in &file_functions {
            let callers: Vec<_> = parse_result
                .call_sites
                .iter()
                .filter(|cs| cs.callee_name == *func_name)
                .collect();

            if callers.is_empty() {
                continue;
            }

            has_callers = true;
            let mut seen = std::collections::HashSet::new();
            let unique_callers: Vec<_> = callers
                .iter()
                .filter(|cs| {
                    seen.insert(format!("{}::{}", cs.caller_file, cs.caller_name))
                })
                .collect();

            lines.push(format!(
                "  {} ({} callers):",
                func_name,
                unique_callers.len()
            ));
            for cs in unique_callers.iter().take(5) {
                let short_file = cs.caller_file.rsplit('/').next().unwrap_or(&cs.caller_file);
                lines.push(format!("    {} in {}", cs.caller_name, short_file));
            }
            if unique_callers.len() > 5 {
                lines.push(format!("    ... and {} more", unique_callers.len() - 5));
            }
        }

        if !has_callers {
            lines.push("  No external callers found.".to_string());
        }

        Some(PrecomputedAnswer {
            question: format!("file_callers|{}", rel_path),
            answer: lines.join("\n"),
            confidence: 0.9,
        })
    }

    /// Compute "What has changed recently?" answer via `git log`.
    fn compute_recent_changes(&self, rel_path: &str) -> Option<PrecomputedAnswer> {
        let output = std::process::Command::new("git")
            .args(["log", "-5", "--oneline", "--", rel_path])
            .current_dir(&self.repo_path)
            .output()
            .ok()?;

        let log_text = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if log_text.is_empty() {
            return Some(PrecomputedAnswer {
                question: format!("recent_changes|{}", rel_path),
                answer: format!("No recent git history for {}.", rel_path),
                confidence: 0.7,
            });
        }

        let mut lines = Vec::new();
        lines.push(format!("=== Recent changes to {} ===", rel_path));
        for line in log_text.lines() {
            lines.push(format!("  {}", line));
        }

        Some(PrecomputedAnswer {
            question: format!("recent_changes|{}", rel_path),
            answer: lines.join("\n"),
            confidence: 0.9,
        })
    }

    /// Compute "What tests cover this file?" answer.
    fn compute_test_coverage(
        &self,
        rel_path: &str,
        parse_result: &ParseResult,
    ) -> Option<PrecomputedAnswer> {
        // Get function names from this file to search for in test files
        let file_functions: Vec<&str> = parse_result
            .entities
            .iter()
            .filter(|e| {
                e.kind == "function"
                    && (e.file == rel_path
                        || e.file.ends_with(&format!("/{}", rel_path)))
            })
            .map(|e| e.name.as_str())
            .collect();

        if file_functions.is_empty() {
            return None;
        }

        // Also derive the file stem for import-based matching
        let file_stem = std::path::Path::new(rel_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        let mut test_hits: Vec<String> = Vec::new();
        let mut test_files_seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Search for test files referencing functions from this file
        for func_name in &file_functions {
            if func_name.len() < 4 {
                continue; // skip very short names to avoid false matches
            }

            let grep_output = std::process::Command::new("grep")
                .args([
                    "-rln",
                    "--include=test_*.py",
                    "--include=*_test.rs",
                    "--include=*.test.ts",
                    "--include=*_spec.ts",
                    "--include=*_test.go",
                    "--include=*.test.js",
                    "--include=*_test.py",
                    "--include=*.spec.ts",
                    "--include=*.spec.js",
                    "--exclude-dir=node_modules",
                    "--exclude-dir=target",
                    "--exclude-dir=.git",
                    "--exclude-dir=__pycache__",
                    func_name,
                    ".",
                ])
                .current_dir(&self.repo_path)
                .output();

            if let Ok(output) = grep_output {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines().filter(|l| !l.is_empty()) {
                    let file = line.strip_prefix("./").unwrap_or(line).to_string();
                    if test_files_seen.insert(file.clone()) {
                        test_hits.push(format!("  {} (references {})", file, func_name));
                    }
                }
            }
        }

        let mut lines = Vec::new();
        lines.push(format!("=== Test coverage for {} ===", rel_path));

        if test_hits.is_empty() {
            lines.push("  No test files found referencing functions in this file.".to_string());
            // Suggest a test file name
            let test_suggestion = if rel_path.ends_with(".rs") {
                format!("tests/test_{}.rs", file_stem)
            } else if rel_path.ends_with(".py") {
                format!("tests/test_{}.py", file_stem)
            } else if rel_path.ends_with(".ts") || rel_path.ends_with(".js") {
                let ext = rel_path.rsplit('.').next().unwrap_or("ts");
                format!(
                    "{}.test.{}",
                    &rel_path[..rel_path.len() - ext.len() - 1],
                    ext
                )
            } else {
                format!("tests/test_{}", file_stem)
            };
            lines.push(format!("  Suggestion: create {}", test_suggestion));
        } else {
            lines.push(format!(
                "  {} test files reference this file:",
                test_files_seen.len()
            ));
            for hit in test_hits.iter().take(10) {
                lines.push(hit.clone());
            }
            if test_hits.len() > 10 {
                lines.push(format!("  ... and {} more", test_hits.len() - 10));
            }
        }

        Some(PrecomputedAnswer {
            question: format!("test_coverage|{}", rel_path),
            answer: lines.join("\n"),
            confidence: 0.8,
        })
    }

    /// Get the last modified info from git for a file.
    fn git_last_modified(&self, rel_path: &str) -> String {
        let output = std::process::Command::new("git")
            .args(["log", "-1", "--format=%ar by %an", "--", rel_path])
            .current_dir(&self.repo_path)
            .output();

        match output {
            Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            Err(_) => String::new(),
        }
    }
}

/// Load the cached parse result from disk (same as in tools.rs).
fn load_parse_index(repo: &str) -> Result<ParseResult, String> {
    let path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".savants")
        .join("code-index")
        .join(format!("{}.json", repo));

    let data = std::fs::read_to_string(&path)
        .map_err(|_| format!("No local index for '{}'. Run 'savants up' first.", repo))?;
    serde_json::from_str(&data).map_err(|e| format!("Corrupt index: {}", e))
}

/// Format a prefetch result for display.
pub fn format_cached_answer(answer: &PrecomputedAnswer) -> String {
    format!(
        "{} Cached answer (0ms)\n\n{}",
        "\u{26A1}".yellow(),  // lightning bolt
        answer.answer
    )
}

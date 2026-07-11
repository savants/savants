//! Differential incremental indexer — watches a repo for file changes
//! and re-indexes only the changed files.
//!
//! Uses polling (file mtime checks) rather than OS-level watchers for
//! simplicity and cross-platform compatibility. Debounces rapid saves
//! (waits 200ms after last change before processing).
//!
//! Target: <100ms per single file change.

use colored::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};
use walkdir::WalkDir;

use crate::code_parser::{CodeParser, ParseResult, ParsedEntity};
use crate::doc_parser;
use crate::embedding_store::EmbeddingStore;
use crate::embeddings::EmbeddingEngine;
use crate::semantic_search::SemanticIndex;

/// A file that has changed since the last scan.
pub struct ChangedFile {
    pub path: PathBuf,
    pub change_type: ChangeType,
}

/// The type of change detected.
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
}

/// Persistent state for the diff indexer — tracks file mtimes.
#[derive(Serialize, Deserialize, Default)]
struct DiffState {
    file_mtimes: HashMap<String, u64>, // path -> mtime as seconds since epoch
}

/// The differential incremental indexer.
pub struct DiffIndexer {
    repo_path: PathBuf,
    repo_name: String,
    file_mtimes: HashMap<PathBuf, SystemTime>,
}

/// Source file extensions to watch.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "py", "pyi", "ts", "tsx", "js", "jsx", "mjs", "cjs",
    "go", "java", "c", "h", "cpp", "cc", "cxx", "hpp", "hxx",
    "rb", "cs", "md",
];

/// Directories to ignore.
const SKIP_DIRS: &[&str] = &[
    "target", "node_modules", ".git", "dist", "build", ".next",
    "__pycache__", ".venv", "venv", "coverage", ".turbo",
];

impl DiffIndexer {
    pub fn new(repo_path: &str, repo_name: &str) -> Self {
        let mut indexer = Self {
            repo_path: PathBuf::from(repo_path),
            repo_name: repo_name.to_string(),
            file_mtimes: HashMap::new(),
        };

        // Load persisted state if available
        if let Ok(state) = indexer.load_state() {
            for (path_str, mtime_secs) in state.file_mtimes {
                let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(mtime_secs);
                indexer.file_mtimes.insert(PathBuf::from(path_str), mtime);
            }
        }

        indexer
    }

    /// Scan the repo and populate initial mtimes without triggering changes.
    pub fn snapshot_mtimes(&mut self) {
        for entry in self.walk_source_files() {
            let path = entry.path().to_path_buf();
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(mtime) = meta.modified() {
                    self.file_mtimes.insert(path, mtime);
                }
            }
        }
        self.save_state();
    }

    /// Detect files that changed since the last check.
    pub fn detect_changes(&mut self) -> Vec<ChangedFile> {
        let mut changes = Vec::new();
        let mut current_files: HashMap<PathBuf, SystemTime> = HashMap::new();

        // Walk the repo and check mtimes
        for entry in self.walk_source_files() {
            let path = entry.path().to_path_buf();
            let meta = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime = match meta.modified() {
                Ok(t) => t,
                Err(_) => continue,
            };

            current_files.insert(path.clone(), mtime);

            match self.file_mtimes.get(&path) {
                None => {
                    // New file
                    changes.push(ChangedFile {
                        path: path.clone(),
                        change_type: ChangeType::Added,
                    });
                }
                Some(old_mtime) => {
                    if *old_mtime != mtime {
                        changes.push(ChangedFile {
                            path: path.clone(),
                            change_type: ChangeType::Modified,
                        });
                    }
                }
            }
        }

        // Check for deleted files
        for old_path in self.file_mtimes.keys() {
            if !current_files.contains_key(old_path) {
                changes.push(ChangedFile {
                    path: old_path.clone(),
                    change_type: ChangeType::Deleted,
                });
            }
        }

        // Update stored mtimes
        self.file_mtimes = current_files;

        if !changes.is_empty() {
            self.save_state();
        }

        changes
    }

    /// Incrementally update the code index and embeddings for changed files.
    pub fn update_index(&self, changes: &[ChangedFile]) {
        if changes.is_empty() {
            return;
        }

        let total_start = Instant::now();

        // Separate code files from doc files
        let mut code_changes: Vec<&ChangedFile> = Vec::new();
        let mut doc_changes: Vec<&ChangedFile> = Vec::new();

        for change in changes {
            let ext = change.path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if ext == "md" {
                doc_changes.push(change);
            } else {
                code_changes.push(change);
            }
        }

        if !code_changes.is_empty() {
            self.update_code_index(&code_changes, total_start);
        }

        if !doc_changes.is_empty() {
            self.update_doc_index(&doc_changes);
        }
    }

    /// Update the code index (entities JSON + embeddings) for changed code files.
    fn update_code_index(&self, changes: &[&ChangedFile], total_start: Instant) {
        let parse_start = Instant::now();

        // Load existing code index
        let index_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".savants")
            .join("code-index")
            .join(format!("{}.json", self.repo_name));

        let mut parse_result: ParseResult = if index_path.exists() {
            match std::fs::read_to_string(&index_path) {
                Ok(json) => serde_json::from_str(&json).unwrap_or_else(|_| ParseResult {
                    repo: self.repo_name.clone(),
                    files: 0,
                    entities: vec![],
                    call_sites: vec![],
                }),
                Err(_) => ParseResult {
                    repo: self.repo_name.clone(),
                    files: 0,
                    entities: vec![],
                    call_sites: vec![],
                },
            }
        } else {
            ParseResult {
                repo: self.repo_name.clone(),
                files: 0,
                entities: vec![],
                call_sites: vec![],
            }
        };

        let mut new_entities_for_embedding: Vec<ParsedEntity> = Vec::new();
        let mut deleted_files: Vec<String> = Vec::new();
        let mut updated_count = 0usize;

        for change in changes {
            let rel_path = change.path.strip_prefix(&self.repo_path)
                .unwrap_or(&change.path)
                .to_string_lossy()
                .replace('\\', "/");

            match change.change_type {
                ChangeType::Deleted => {
                    // Remove all entities and call sites from this file
                    parse_result.entities.retain(|e| e.file != rel_path);
                    parse_result.call_sites.retain(|c| c.caller_file != rel_path);
                    deleted_files.push(rel_path.clone());
                    updated_count += 1;
                }
                ChangeType::Added | ChangeType::Modified => {
                    // Remove old entities for this file
                    parse_result.entities.retain(|e| e.file != rel_path);
                    parse_result.call_sites.retain(|c| c.caller_file != rel_path);
                    deleted_files.push(rel_path.clone());

                    // Parse the changed file
                    let mut parser = CodeParser::new(&self.repo_name);
                    let file_result = parser.parse_single_file(
                        &change.path,
                        &self.repo_path.to_string_lossy(),
                    );

                    if let Some(result) = file_result {
                        let func_count = result.entities.iter()
                            .filter(|e| e.kind == "function")
                            .count();
                        updated_count += func_count;

                        new_entities_for_embedding.extend(result.entities.iter()
                            .filter(|e| e.kind != "import")
                            .cloned());
                        parse_result.entities.extend(result.entities);
                        parse_result.call_sites.extend(result.call_sites);
                    }
                }
            }
        }

        let parse_ms = parse_start.elapsed().as_millis();

        // Save updated code index
        let index_dir = index_path.parent().unwrap();
        std::fs::create_dir_all(index_dir).ok();
        if let Ok(json) = serde_json::to_string(&parse_result) {
            std::fs::write(&index_path, json).ok();
        }

        // Update file count
        let mut unique_files = std::collections::HashSet::new();
        for e in &parse_result.entities {
            unique_files.insert(e.file.clone());
        }
        // parse_result.files is read-only after deserialization, but we write the whole thing back

        // Update embeddings
        let embed_start = Instant::now();
        self.update_embeddings_incremental(&new_entities_for_embedding, &deleted_files);
        let embed_ms = embed_start.elapsed().as_millis();

        let total_ms = total_start.elapsed().as_millis();

        // Print summary for each changed file
        for change in changes {
            let rel_path = change.path.strip_prefix(&self.repo_path)
                .unwrap_or(&change.path)
                .to_string_lossy();
            let action = match change.change_type {
                ChangeType::Added => "added",
                ChangeType::Modified => "updated",
                ChangeType::Deleted => "removed",
            };
            let color_ms = if total_ms < 100 {
                format!("{}ms", total_ms).green()
            } else if total_ms < 500 {
                format!("{}ms", total_ms).yellow()
            } else {
                format!("{}ms", total_ms).red()
            };
            println!("{} {}: {} functions {} (parse: {}ms, embed: {}ms, total: {})",
                "\u{0394}".cyan(), // delta symbol
                rel_path,
                updated_count,
                action,
                parse_ms,
                embed_ms,
                color_ms,
            );
        }
    }

    /// Update embeddings incrementally: remove old entries for deleted files,
    /// add new entries for new/modified files.
    fn update_embeddings_incremental(&self, new_entities: &[ParsedEntity], deleted_files: &[String]) {
        // Load existing embedding store
        let mut store = match EmbeddingStore::load(&self.repo_name) {
            Ok(s) => s,
            Err(_) => {
                // No existing store, create new one if we have entities
                if new_entities.is_empty() {
                    return;
                }
                // We need the embedding engine to know the dimension
                match EmbeddingEngine::new() {
                    Ok(mut engine) => {
                        let dim = engine.embed_one("test").map(|v| v.len() as u32).unwrap_or(128);
                        EmbeddingStore::new(dim)
                    }
                    Err(_) => EmbeddingStore::new(128),
                }
            }
        };

        // Remove entries for deleted/modified files
        if !deleted_files.is_empty() {
            store.entries.retain(|e| !deleted_files.contains(&e.file));
        }

        // Add new entries
        if !new_entities.is_empty() {
            match EmbeddingEngine::new() {
                Ok(mut engine) => {
                    // Build semantic index for just the new entities
                    let temp_result = ParseResult {
                        repo: self.repo_name.clone(),
                        files: 0,
                        entities: new_entities.to_vec(),
                        call_sites: vec![],
                    };

                    match SemanticIndex::from_parse_result(&temp_result, &mut engine) {
                        Ok(index) => {
                            for (entry, emb) in index.entries_with_embeddings() {
                                let kind = match entry.kind.as_str() {
                                    "class" => 1,
                                    "interface" => 2,
                                    _ => 0,
                                };
                                store.add(&entry.name, &entry.file, entry.line as u32, kind, emb.clone());
                            }
                        }
                        Err(e) => {
                            eprintln!("  {}: building incremental index: {}", "Warning".yellow(), e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("  {}: embedding engine: {}", "Warning".yellow(), e);
                }
            }
        }

        // Save back
        if let Err(e) = store.save(&self.repo_name) {
            eprintln!("  {}: saving embeddings: {}", "Warning".yellow(), e);
        }
    }

    /// Update the doc index for changed markdown files.
    fn update_doc_index(&self, changes: &[&ChangedFile]) {
        let doc_store_name = format!("{}-docs", self.repo_name);
        let start = Instant::now();

        // Load existing doc store
        let mut store = EmbeddingStore::load(&doc_store_name).unwrap_or_else(|_| {
            match EmbeddingEngine::new() {
                Ok(mut engine) => {
                    let dim = engine.embed_one("test").map(|v| v.len() as u32).unwrap_or(128);
                    EmbeddingStore::new(dim)
                }
                Err(_) => EmbeddingStore::new(128),
            }
        });

        let mut deleted_files: Vec<String> = Vec::new();
        let mut new_sections: Vec<doc_parser::DocSection> = Vec::new();

        for change in changes {
            let rel_path = change.path.strip_prefix(&self.repo_path)
                .unwrap_or(&change.path)
                .to_string_lossy()
                .replace('\\', "/");

            match change.change_type {
                ChangeType::Deleted => {
                    deleted_files.push(rel_path.clone());
                }
                ChangeType::Added | ChangeType::Modified => {
                    deleted_files.push(rel_path.clone());
                    if let Ok(content) = std::fs::read_to_string(&change.path) {
                        let sections = doc_parser::parse_markdown(&content, &rel_path);
                        new_sections.extend(sections);
                    }
                }
            }
        }

        // Remove old entries
        store.entries.retain(|e| !deleted_files.contains(&e.file));

        // Embed and add new sections
        if !new_sections.is_empty() {
            if let Ok(mut engine) = EmbeddingEngine::new() {
                let texts: Vec<String> = new_sections.iter()
                    .map(|s| {
                        let heading_expanded = s.heading.replace('-', " ").replace('_', " ");
                        let content_preview: String = s.content.chars().take(500).collect();
                        format!("{h} {h} {f} {c}",
                            h = heading_expanded,
                            f = s.file.replace('/', " ").replace('.', " "),
                            c = content_preview,
                        )
                    })
                    .collect();

                if let Ok(embeddings) = engine.embed(&texts) {
                    for (section, emb) in new_sections.iter().zip(embeddings.into_iter()) {
                        store.add(
                            &section.heading,
                            &section.file,
                            section.line as u32,
                            3, // kind=3 for doc sections
                            emb,
                        );
                    }
                }
            }
        }

        if let Err(e) = store.save(&doc_store_name) {
            eprintln!("  {}: saving doc embeddings: {}", "Warning".yellow(), e);
        }

        let ms = start.elapsed().as_millis();
        for change in changes {
            let rel_path = change.path.strip_prefix(&self.repo_path)
                .unwrap_or(&change.path)
                .to_string_lossy();
            println!("{} {}: doc sections updated ({}ms)",
                "\u{0394}".cyan(),
                rel_path,
                ms,
            );
        }
    }

    /// Start the watch loop. Polls for changes at the given interval.
    /// Debounces: waits 200ms after last change before processing.
    /// Includes predictive pre-fetch: after index update, pre-computes
    /// answers for changed files so queries return in 0ms.
    pub fn watch(&mut self, interval_ms: u64) {
        let poll_interval = Duration::from_millis(interval_ms);
        let debounce_duration = Duration::from_millis(200);

        let mut prefetch_cache = crate::prefetch_cache::PrefetchCache::new(
            &self.repo_name,
            &self.repo_path.to_string_lossy(),
        );

        println!("  Watching for changes (poll: {}ms, debounce: {}ms, prefetch: on)...",
            interval_ms, 200);
        println!("  Press {} to stop.", "Ctrl+C".bold());
        println!();

        loop {
            std::thread::sleep(poll_interval);

            let changes = self.detect_changes();
            if changes.is_empty() {
                continue;
            }

            // Debounce: wait a bit and re-check for more changes
            std::thread::sleep(debounce_duration);
            let mut all_changes = changes;
            let more = self.detect_changes();
            all_changes.extend(more);

            // Dedup by path (keep last change type)
            let mut deduped: HashMap<PathBuf, ChangeType> = HashMap::new();
            for change in all_changes {
                deduped.insert(change.path, change.change_type);
            }
            let final_changes: Vec<ChangedFile> = deduped.into_iter()
                .map(|(path, change_type)| ChangedFile { path, change_type })
                .collect();

            // Update the code/doc index
            self.update_index(&final_changes);

            // Predictive pre-fetch: invalidate and re-compute for changed files
            for change in &final_changes {
                let rel_path = change.path.strip_prefix(&self.repo_path)
                    .unwrap_or(&change.path)
                    .to_string_lossy()
                    .replace('\\', "/");

                match change.change_type {
                    ChangeType::Deleted => {
                        prefetch_cache.invalidate(&rel_path);
                    }
                    ChangeType::Added | ChangeType::Modified => {
                        prefetch_cache.invalidate(&rel_path);
                        let (count, ms) = prefetch_cache.prefetch_for_file(&rel_path);
                        if count > 0 {
                            println!("  {} prefetched {} answers for {} ({}ms, cache: {} files)",
                                "\u{26A1}".yellow(),
                                count, rel_path, ms, prefetch_cache.len());
                        }
                    }
                }
            }
        }
    }

    /// Walk the repo and yield source file entries.
    fn walk_source_files(&self) -> Vec<walkdir::DirEntry> {
        WalkDir::new(&self.repo_path)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !SKIP_DIRS.iter().any(|d| name == *d)
            })
            .filter_map(|e| e.ok())
            .filter(|e| {
                if !e.file_type().is_file() {
                    return false;
                }
                let ext = e.path().extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("");
                SOURCE_EXTENSIONS.contains(&ext)
            })
            .collect()
    }

    /// State file path: ~/.savants/diff-index/{repo}-state.json
    fn state_path(&self) -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".savants")
            .join("diff-index")
            .join(format!("{}-state.json", self.repo_name))
    }

    /// Save mtime state to disk.
    fn save_state(&self) {
        let path = self.state_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let mut mtimes = HashMap::new();
        for (path, mtime) in &self.file_mtimes {
            if let Ok(dur) = mtime.duration_since(SystemTime::UNIX_EPOCH) {
                mtimes.insert(path.to_string_lossy().to_string(), dur.as_secs());
            }
        }

        let state = DiffState { file_mtimes: mtimes };
        if let Ok(json) = serde_json::to_string(&state) {
            std::fs::write(&path, json).ok();
        }
    }

    /// Load mtime state from disk.
    fn load_state(&self) -> Result<DiffState, String> {
        let path = self.state_path();
        let data = std::fs::read_to_string(&path).map_err(|e| format!("{}", e))?;
        serde_json::from_str(&data).map_err(|e| format!("{}", e))
    }
}

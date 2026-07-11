use colored::*;
use std::path::Path;
use crate::find_in_path;
use crate::embedded::EmbeddedEngine;

pub async fn run(repo: Option<String>, _tail_lines: u32, watch: bool) {
    println!("{}", "Starting Savants...".bold());
    println!();

    // 1. Ensure embedded context engine is running
    let embedded = EmbeddedEngine::new();
    match embedded.ensure_running() {
        Ok(true) => println!("  {} Context engine: {}", "●".green(), "ready".green()),
        Ok(false) => println!("  {} Context engine: {}", "●".green(), "ready".green()),
        Err(_e) => {
            // No local Redis available - check if cloud is configured
            if std::env::var("SAVANTS_CLOUD_URL").is_ok() {
                println!("  {} Context: {} (cloud mode)", "●".green(), "api.savants.cloud".cyan());
            } else {
                println!("  {} Context: {}", "●".yellow(), "local only".yellow());
                println!("    Run {} for cloud features.", "savants connect".cyan());
            }
        }
    }

    // Set the port for downstream graph queries
    std::env::set_var("SAVANTS_PORT", embedded.port.to_string());

    println!();
    println!("{}...", "Detecting infrastructure".bold());

    // Auto-detect K8s clusters
    let k8s_clusters = detect_k8s();
    for c in &k8s_clusters {
        println!("  Found K8s cluster: {}", c.cyan());
    }
    if k8s_clusters.is_empty() {
        println!("  {}", "No K8s clusters found".dimmed());
    }

    // Docker
    let has_docker = find_in_path("docker").is_some();
    if has_docker {
        println!("  Found Docker");
    }

    // systemd
    let has_systemd = find_in_path("systemctl").is_some();
    if has_systemd {
        println!("  Found systemd");
    }

    // Git repo
    let repo_path = repo.or_else(|| {
        if Path::new(".git").exists() {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        } else {
            None
        }
    });
    if let Some(ref r) = repo_path {
        println!("  Found git repo: {}", r.cyan());
    }

    println!();

    let mut issues: Vec<String> = vec![];

    // Index the git repo's code into a local search index
    if let Some(ref repo_dir) = repo_path {
        index_code_repo(repo_dir);
        index_docs(repo_dir);
        detect_and_save_communities(repo_dir);
    }

    // 2. Ingest host (pure Rust)
    println!("[{}] Ingesting local machine...", "host".bold());
    #[cfg(feature = "host")]
    {
        match crate::host::snapshot() {
            Ok(stats) => {
                println!("  {} disks, {} interfaces, {} systemd units ({} failed), {} journal events",
                    stats.disks, stats.interfaces, stats.systemd_units,
                    stats.failed_units, stats.journal_events);
                if stats.failed_units > 0 {
                    issues.push(format!("{} failed systemd units", stats.failed_units));
                }
                if stats.journal_events > 0 {
                    issues.push(format!("{} journal error patterns", stats.journal_events));
                }
            }
            Err(e) => println!("  {}: {}", "Error".red(), e),
        }
    }
    // Fallback: if host module isn't compiled in, try the graph for existing data
    #[cfg(not(feature = "host"))]
    {
        let graph_name = std::env::var("SAVANTS_MEMORY").unwrap_or_else(|_| "savants".into());
        if let Ok(client) = crate::graph::GraphClient::new(&graph_name) {
            // Check if host data exists from a previous run
            if let Ok(r) = client.query("MATCH (h:Host) RETURN h.hostname", &[]) {
                if r.rows.is_empty() {
                    println!("  {}", "(host module building — will be available soon)".dimmed());
                } else {
                    println!("  Host data available from previous ingest");
                }
            }
        }
    }

    // 3. Ingest K8s clusters (pure Rust when available)
    for cluster in &k8s_clusters {
        println!("\n[{}] Ingesting cluster '{}'...", "k8s".bold(), cluster.cyan());
        #[cfg(feature = "k8s")]
        {
            let graph_name = crate::config::State::cluster_graph_name(cluster);
            match crate::graph::GraphClient::new(&graph_name) {
                Ok(graph) => {
                    match crate::k8s::K8sIngestor::kube_client_from_kubeconfig(Some(cluster)).await {
                        Ok(kube_client) => {
                            let ingestor = crate::k8s::K8sIngestor::new(
                                graph, cluster.to_string(), kube_client,
                            );
                            let stats = ingestor.snapshot().await;
                            println!("{}", stats.summary());
                        }
                        Err(e) => println!("  {}: K8s client: {}", "Error".red(), e),
                    }
                }
                Err(e) => println!("  {}: context engine: {}", "Error".red(), e),
            }
        }
        #[cfg(not(feature = "k8s"))]
        {
            // Fallback: check graph for existing data
            let graph_name = cluster.replace("-", "_");
            if let Ok(client) = crate::graph::GraphClient::new(&graph_name) {
                if let Ok(r) = client.query("MATCH (p:K8sPod) RETURN p.status, count(p) ORDER BY count(p) DESC", &[]) {
                    if !r.rows.is_empty() {
                        let status_str: String = r.rows.iter()
                            .map(|r| format!("{} {}", r[1].as_i64(), r[0].as_str()))
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!("  Pods: {}", status_str);
                    }
                }
                if let Ok(r) = client.query(
                    "MATCH (e:LogEvent) WHERE e.severity IN ['ERROR','FATAL'] RETURN count(DISTINCT e.pod), count(e)", &[]) {
                    if let Some(row) = r.rows.first() {
                        let n_pods = row[0].as_i64();
                        let n_templates = row[1].as_i64();
                        if n_templates > 0 {
                            println!("  Log intelligence: {} error templates from {} pods", n_templates, n_pods);
                            issues.push(format!("{} error patterns across {} pods on {}", n_templates, n_pods, cluster));
                        }
                    }
                }
            }
        }
    }

    // 4. Auto-configure MCP if an AI tool is detected and not already configured
    let mcp_configured = Path::new(".mcp.json").exists()
        && std::fs::read_to_string(".mcp.json")
            .map(|s| s.contains("savants"))
            .unwrap_or(false);

    if !mcp_configured {
        let has_claude = find_in_path("claude").is_some();
        let has_cursor = Path::new(".cursor").exists();
        if has_claude || has_cursor {
            println!("[{}] Auto-configuring MCP for AI tools...", "mcp".bold());
            super::mcp::install("project", "auto");
        }
    }

    // 5. Summary
    println!("\n{}", "=".repeat(60));
    if issues.is_empty() {
        println!("{}", "No issues detected. Your infrastructure looks healthy.".green());
    } else {
        println!("Found {} issue(s):\n", issues.len().to_string().red());
        for (i, issue) in issues.iter().enumerate() {
            println!("  {}. {}", i + 1, issue.red());
        }
        println!("\nRun {} for full diagnosis.", "savants story".cyan());
    }
    println!("{}", "=".repeat(60));

    // 6. If --watch flag is set, start the differential file watcher
    if watch {
        if let Some(ref repo_dir) = repo_path {
            let repo_name = detect_repo_name(repo_dir);
            println!("\n[{}] Starting differential file watcher for '{}'...",
                "watch".bold(), repo_name.cyan());

            let mut indexer = crate::file_watcher::DiffIndexer::new(repo_dir, &repo_name);
            // Snapshot current mtimes so we only detect future changes
            indexer.snapshot_mtimes();
            // Poll every 500ms
            indexer.watch(500);
        } else {
            println!("\n{}: --watch requires a git repo. Run from a git directory or pass --repo.",
                "Error".red());
        }
    } else {
        println!("\nSavants is ready. Use the MCP tools from your AI assistant,");
        println!("or run {} for live monitoring.", "savants k8s watch <cluster>".cyan());
        println!("Or run {} for incremental file watching.", "savants up --watch".cyan());
    }
}

fn detect_k8s() -> Vec<String> {
    // Quick check: does ~/.kube/config exist?
    let kube_config = dirs::home_dir()
        .map(|h| h.join(".kube/config"))
        .filter(|p| p.exists());

    if kube_config.is_none() {
        return vec![];
    }

    // Parse contexts from kubeconfig (simple YAML grep, no full parser needed)
    let config_path = kube_config.unwrap();
    let content = std::fs::read_to_string(&config_path).unwrap_or_default();
    let mut contexts = vec![];
    let mut in_contexts = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "contexts:" {
            in_contexts = true;
            continue;
        }
        if in_contexts && trimmed.starts_with("- name:") {
            let name = trimmed.trim_start_matches("- name:").trim().trim_matches('"');
            contexts.push(name.to_string());
        }
        if in_contexts && !trimmed.is_empty() && !trimmed.starts_with('-') && !trimmed.starts_with("name:") && !trimmed.starts_with("context:") && !trimmed.starts_with("cluster:") && !trimmed.starts_with("user:") && !trimmed.starts_with("namespace:") {
            if !trimmed.starts_with('#') && !trimmed.starts_with("- ") {
                in_contexts = false;
            }
        }
    }

    contexts
}

/// Parse the git repo's source code and build a local embedding index
/// so that `savants search` and `savants skeleton` work without a cloud API key.
fn index_code_repo(repo_dir: &str) {
    use crate::code_parser::CodeParser;
    use crate::embedding_store::EmbeddingStore;
    use crate::embeddings::EmbeddingEngine;
    use crate::semantic_search::SemanticIndex;

    // Derive repo name from git remote or directory name
    let repo_name = detect_repo_name(repo_dir);

    println!("\n[{}] Indexing code in '{}'...", "code".bold(), repo_name.cyan());

    // Parse the codebase with tree-sitter
    let mut parser = CodeParser::new(&repo_name);
    let parse_result = parser.parse_repo(repo_dir);

    let entity_count = parse_result.entities.iter()
        .filter(|e| e.kind != "import")
        .count();
    let func_count = parse_result.entities.iter()
        .filter(|e| e.kind == "function")
        .count();
    let class_count = parse_result.entities.iter()
        .filter(|e| e.kind == "class" || e.kind == "interface")
        .count();

    if entity_count == 0 {
        println!("  {}", "No code entities found.".dimmed());
        return;
    }

    println!("  Parsed {} files: {} functions, {} classes/types, {} call sites",
        parse_result.files, func_count, class_count, parse_result.call_sites.len());

    // Save the parsed entities to a JSON sidecar for file_skeleton and callers
    if let Err(e) = save_parse_index(&repo_name, &parse_result) {
        eprintln!("  {}: saving parse index: {}", "Warning".yellow(), e);
    }

    // Build embedding index for semantic search
    match EmbeddingEngine::new() {
        Ok(mut engine) => {
            match SemanticIndex::from_parse_result(&parse_result, &mut engine) {
                Ok(index) => {
                    // Get embedding dimension
                    let dim = engine.embed_one("test").map(|v| v.len() as u32).unwrap_or(128);
                    let mut store = EmbeddingStore::new(dim);
                    for (entry, emb) in index.entries_with_embeddings() {
                        let kind = match entry.kind.as_str() { "class" => 1, "interface" => 2, _ => 0 };
                        store.add(&entry.name, &entry.file, entry.line as u32, kind, emb.clone());
                    }
                    match store.save(&repo_name) {
                        Ok(_) => println!("  {} {} entities indexed for semantic search",
                            "✓".green(), store.entries.len()),
                        Err(e) => eprintln!("  {}: saving embeddings: {}", "Warning".yellow(), e),
                    }
                }
                Err(e) => eprintln!("  {}: building search index: {}", "Warning".yellow(), e),
            }
        }
        Err(e) => eprintln!("  {}: embedding engine: {}", "Warning".yellow(), e),
    }
}

/// Detect repo name from git remote or directory name.
fn detect_repo_name(repo_dir: &str) -> String {
    if let Ok(output) = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_dir)
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
    Path::new(repo_dir)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

/// Parse and index documentation files (markdown, astro) into the embedding store.
fn index_docs(repo_dir: &str) {
    use crate::doc_parser;
    use crate::embedding_store::EmbeddingStore;
    use crate::embeddings::EmbeddingEngine;

    let repo_name = detect_repo_name(repo_dir);
    let doc_store_name = format!("{}-docs", repo_name);

    // Collect doc sections from multiple directories
    let mut all_sections: Vec<doc_parser::DocSection> = Vec::new();

    // 1. docs/ directory
    let docs_dir = Path::new(repo_dir).join("docs");
    if docs_dir.exists() {
        all_sections.extend(doc_parser::parse_docs(&docs_dir.to_string_lossy()));
    }

    // 2. Root-level .md files
    if let Ok(entries) = std::fs::read_dir(repo_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext == "md" {
                        let rel = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            let sections = parse_markdown_sections(&content, &rel);
                            all_sections.extend(sections);
                        }
                    }
                }
            }
        }
    }

    // 3. website/src/pages/docs/ (.astro files)
    let website_docs = Path::new(repo_dir).join("website").join("src").join("pages").join("docs");
    if website_docs.exists() {
        all_sections.extend(doc_parser::parse_docs(&website_docs.to_string_lossy()));
    }

    if all_sections.is_empty() {
        return;
    }

    // Count unique files
    let mut unique_files = std::collections::HashSet::new();
    for section in &all_sections {
        unique_files.insert(section.file.clone());
    }

    println!("\n[{}] Indexing documentation...", "docs".bold());

    // Build embeddings for doc sections
    match EmbeddingEngine::new() {
        Ok(mut engine) => {
            // Prepare texts for embedding — heading repeated for emphasis + content
            let texts: Vec<String> = all_sections.iter()
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

            match engine.embed(&texts) {
                Ok(embeddings) => {
                    let dim = embeddings.first().map(|v| v.len() as u32).unwrap_or(128);
                    let mut store = EmbeddingStore::new(dim);

                    for (section, emb) in all_sections.iter().zip(embeddings.into_iter()) {
                        // kind=3 for doc sections (0=function, 1=class, 2=interface)
                        store.add(
                            &section.heading,
                            &section.file,
                            section.line as u32,
                            3,
                            emb,
                        );
                    }

                    match store.save(&doc_store_name) {
                        Ok(_) => println!("  {} Indexed {} doc sections from {} files",
                            "✓".green(), all_sections.len(), unique_files.len()),
                        Err(e) => eprintln!("  {}: saving doc embeddings: {}", "Warning".yellow(), e),
                    }

                    // Also save a JSON sidecar with section details for rich display
                    if let Err(e) = save_doc_index(&doc_store_name, &all_sections) {
                        eprintln!("  {}: saving doc index: {}", "Warning".yellow(), e);
                    }
                }
                Err(e) => eprintln!("  {}: embedding docs: {}", "Warning".yellow(), e),
            }
        }
        Err(e) => eprintln!("  {}: embedding engine: {}", "Warning".yellow(), e),
    }
}

/// Wrapper around doc_parser::parse_markdown for root-level .md files.
fn parse_markdown_sections(content: &str, file: &str) -> Vec<crate::doc_parser::DocSection> {
    crate::doc_parser::parse_markdown(content, file)
}

/// Save doc section metadata to ~/.savants/doc-index/{repo}.json
fn save_doc_index(store_name: &str, sections: &[crate::doc_parser::DocSection]) -> Result<(), String> {
    let index_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".savants")
        .join("doc-index");
    std::fs::create_dir_all(&index_dir).map_err(|e| format!("mkdir: {}", e))?;

    #[derive(serde::Serialize)]
    struct DocEntry {
        file: String,
        heading: String,
        content: String,
        level: usize,
        line: usize,
    }

    let entries: Vec<DocEntry> = sections.iter().map(|s| DocEntry {
        file: s.file.clone(),
        heading: s.heading.clone(),
        content: s.content.clone(),
        level: s.level,
        line: s.line,
    }).collect();

    let path = index_dir.join(format!("{}.json", store_name));
    let json = serde_json::to_string(&entries).map_err(|e| format!("serialize: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("write: {}", e))?;
    Ok(())
}

/// Detect code communities and save summaries to disk.
fn detect_and_save_communities(repo_dir: &str) {
    use crate::code_graph;

    let repo_name = detect_repo_name(repo_dir);

    // Load the code graph
    let graph = match code_graph::load_graph(&repo_name) {
        Ok(g) => g,
        Err(_) => return, // No index yet, skip
    };

    println!("\n[{}] Detecting code communities...", "communities".bold());

    let communities = graph.detect_communities();

    if communities.is_empty() {
        println!("  {}", "No communities detected (all functions isolated).".dimmed());
        return;
    }

    // Save to disk
    if let Err(e) = code_graph::save_communities(&repo_name, &communities) {
        eprintln!("  {}: saving communities: {}", "Warning".yellow(), e);
        return;
    }

    // Print summary
    let names: Vec<String> = communities
        .iter()
        .take(8)
        .map(|c| c.name.clone())
        .collect();
    let extra = if communities.len() > 8 {
        format!(", ... +{} more", communities.len() - 8)
    } else {
        String::new()
    };
    println!(
        "  {} Detected {} communities ({}{})",
        "✓".green(),
        communities.len(),
        names.join(", "),
        extra,
    );
}

/// Save parsed entities to ~/.savants/code-index/{repo}.json for local queries.
fn save_parse_index(repo_name: &str, result: &crate::code_parser::ParseResult) -> Result<(), String> {
    let index_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".savants")
        .join("code-index");
    std::fs::create_dir_all(&index_dir).map_err(|e| format!("mkdir: {}", e))?;

    let path = index_dir.join(format!("{}.json", repo_name));
    let json = serde_json::to_string(result).map_err(|e| format!("serialize: {}", e))?;
    std::fs::write(&path, json).map_err(|e| format!("write: {}", e))?;
    Ok(())
}

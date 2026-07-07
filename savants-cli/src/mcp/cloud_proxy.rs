//! Cloud proxy MCP server: forwards all tool calls to api.savants.cloud
//! instead of querying a local FalkorDB instance.
//!
//! When SAVANTS_CLOUD_URL is set, `savants serve` uses this instead of
//! the local McpServer. The developer doesn't need FalkorDB installed.

use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashSet;
use std::io::{self, BufRead, Write};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

pub struct CloudProxyServer {
    cloud_url: String,
    api_key: RefCell<String>,
    /// Repos we've confirmed are indexed (avoids re-checking every call)
    indexed_repos: RefCell<HashSet<String>>,
}

impl CloudProxyServer {
    pub fn new(cloud_url: &str, api_key: &str) -> Self {
        Self {
            cloud_url: cloud_url.trim_end_matches('/').to_string(),
            api_key: RefCell::new(api_key.to_string()),
            indexed_repos: RefCell::new(HashSet::new()),
        }
    }

    /// Re-read the API key from state.json if it has changed.
    /// This ensures token refreshes are picked up without restarting.
    fn current_api_key(&self) -> String {
        let state = crate::config::State::load();
        if let Some(fresh_token) = state.cloud_token() {
            let current = self.api_key.borrow().clone();
            if fresh_token != current {
                eprintln!("[savants] Token refreshed from state.json");
                *self.api_key.borrow_mut() = fresh_token.clone();
            }
            fresh_token
        } else {
            self.api_key.borrow().clone()
        }
    }

    pub fn run(&self) {
        eprintln!("Savants MCP server started (cloud proxy -> {})", self.cloud_url);
        let stdin = io::stdin();
        let stdout = io::stdout();
        let reader = stdin.lock();
        let mut writer = stdout.lock();

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            let trimmed = line.trim();
            if trimmed.is_empty() { continue; }

            let message: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if let Some(response) = self.handle_message(&message) {
                let body = serde_json::to_string(&response).unwrap();
                let _ = writeln!(writer, "{}", body);
                let _ = writer.flush();
            }
        }
    }

    fn handle_message(&self, message: &Value) -> Option<Value> {
        let method = message.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or(json!({}));
        let req_id = message.get("id");

        if req_id.is_none() || req_id == Some(&Value::Null) {
            return None;
        }
        let req_id = req_id.unwrap().clone();

        match method {
            "initialize" => Some(self.response(&req_id, json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {"listChanged": false},
                    "resources": {},
                    "prompts": {}
                },
                "serverInfo": {"name": "savants", "version": "0.1.0-cloud"}
            }))),

            "ping" => Some(self.response(&req_id, json!({}))),

            "tools/list" => {
                // Fetch tool list from cloud API
                match self.cloud_get("/api/v1/tools") {
                    Ok(tools_response) => {
                        let tools = tools_response.get("tools").cloned().unwrap_or(json!([]));
                        // Convert cloud format to MCP format
                        let mcp_tools: Vec<Value> = tools.as_array()
                            .unwrap_or(&vec![])
                            .iter()
                            .map(|t| {
                                json!({
                                    "name": t.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                                    "description": t.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                                    "inputSchema": {
                                        "type": "object",
                                        "properties": {}
                                    }
                                })
                            })
                            .collect();
                        Some(self.response(&req_id, json!({"tools": mcp_tools})))
                    }
                    Err(e) => Some(self.error(&req_id, -32000, &format!("Cloud error: {}", e))),
                }
            }

            "tools/call" => {
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

                // Only git tools run locally (they need the actual repo on disk)
                let local_only_tools = ["git_blame", "git_log"];
                if local_only_tools.contains(&tool_name) {
                    let result = self.run_local_tool(tool_name, &arguments);
                    return Some(self.response(&req_id, json!({
                        "content": [{"type": "text", "text": result}]
                    })));
                }

                // Everything else goes through the cloud (D1 + Vectorize)
                // MCP uses hyphens (pr-risk), cloud API uses underscores (pr_risk)
                let cloud_tool_name = tool_name.replace('-', "_");

                // Auto-detect project from current working directory
                let mut input = arguments.clone();
                if input.get("project_id").is_none() && input.get("repo").is_none() {
                    let repo_name = Self::detect_repo_name();
                    if !repo_name.is_empty() {
                        input.as_object_mut().map(|m| m.insert("project_id".to_string(), json!(repo_name)));
                    }
                }

                let body = json!({
                    "tool": cloud_tool_name,
                    "input": input,
                });

                // Check if this project has been indexed (cache per session)
                let repo_name = Self::detect_repo_name();
                if !repo_name.is_empty() && !self.is_indexed(&repo_name) {
                    eprintln!("[savants] Repo '{}' not indexed. Auto-indexing...", repo_name);
                    if let Some(result) = self.auto_index_and_retry(&repo_name, &body) {
                        return Some(self.response(&req_id, json!({
                            "content": [{"type": "text", "text": result}]
                        })));
                    }
                }

                let call_start = std::time::Instant::now();
                match self.cloud_post("/api/v1/tools/call", &body) {
                    Ok(cloud_response) => {
                        // Check if the cloud returned an indexing-related error in the response body
                        let is_index_error = cloud_response.get("error")
                            .and_then(|e| e.as_str())
                            .map(|e| e == "not_available" || e == "no_project" || e == "needs_graph")
                            .unwrap_or(false);

                        if is_index_error {
                            eprintln!("[savants] Cloud reports repo not indexed. Auto-indexing '{}'...", repo_name);
                            if let Some(result) = self.auto_index_and_retry(&repo_name, &body) {
                                return Some(self.response(&req_id, json!({
                                    "content": [{"type": "text", "text": format!(
                                        "[savants] Auto-indexed '{}' before running {}.\n\n{}",
                                        repo_name, tool_name, result
                                    )}]
                                })));
                            }
                            return Some(self.response(&req_id, json!({
                                "content": [{"type": "text", "text": format!(
                                    "[savants] Could not auto-index '{}'. Please run 'savants reindex' manually in your repo, then retry.",
                                    repo_name
                                )}],
                                "isError": true
                            })));
                        }

                        let result_text = cloud_response.get("result")
                            .and_then(|v| v.as_str())
                            .unwrap_or_else(|| {
                                cloud_response.get("result")
                                    .map(|v| v.to_string())
                                    .unwrap_or_default()
                                    .leak()
                            });

                        let elapsed_ms = call_start.elapsed().as_millis() as u64;
                        let tokens_approx = result_text.len() / 4;
                        Self::log_tool_call(tool_name, elapsed_ms, tokens_approx, true);

                        Some(self.response(&req_id, json!({
                            "content": [{
                                "type": "text",
                                "text": result_text
                            }]
                        })))
                    }
                    Err(e) if e.contains("402") || e.contains("Payment") => {
                        Some(self.response(&req_id, json!({
                            "content": [{
                                "type": "text",
                                "text": "Free tier limit reached (10 calls/month).\n\nUpgrade to pay-as-you-go: https://savants.cloud/billing\nOr run: savants usage"
                            }],
                            "isError": true
                        })))
                    }
                    // Catch indexing-related HTTP errors (503 not_available, 404 no_project)
                    Err(e) if e.contains("not_available") || e.contains("not available") ||
                             e.contains("no_project") || e.contains("reindex") => {
                        eprintln!("[savants] Cloud returned indexing error. Auto-indexing '{}'...", repo_name);
                        if let Some(result) = self.auto_index_and_retry(&repo_name, &body) {
                            Some(self.response(&req_id, json!({
                                "content": [{"type": "text", "text": format!(
                                    "[savants] Auto-indexed '{}' before running {}.\n\n{}",
                                    repo_name, tool_name, result
                                )}]
                            })))
                        } else {
                            Some(self.response(&req_id, json!({
                                "content": [{"type": "text", "text": format!(
                                    "[savants] Could not auto-index '{}'. Please run 'savants reindex' manually in your repo, then retry.",
                                    repo_name
                                )}],
                                "isError": true
                            })))
                        }
                    }
                    Err(e) => Some(self.error(&req_id, -32000, &format!("Cloud error: {}", e))),
                }
            }

            "resources/list" => Some(self.response(&req_id, json!({"resources": []}))),
            "prompts/list" => Some(self.response(&req_id, json!({"prompts": []}))),
            _ => Some(self.error(&req_id, -32601, &format!("Unknown method: {}", method))),
        }
    }

    fn cloud_get(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.cloud_url, path);
        let output = std::process::Command::new("curl")
            .args(["-s", "--max-time", "60", "-w", "\n%{http_code}", "-H", &format!("Authorization: Bearer {}", self.current_api_key()), &url])
            .output()
            .map_err(|e| format!("curl failed: {}", e))?;
        let raw = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = raw.rsplitn(2, '\n').collect();
        let status_code = parts.first().unwrap_or(&"0").trim();
        let body_text = if parts.len() > 1 { parts[1] } else { &raw };
        if !output.status.success() || status_code.starts_with('4') || status_code.starts_with('5') {
            let preview = if body_text.len() > 200 { &body_text[..200] } else { body_text };
            return Err(format!("HTTP {} from {}: {}", status_code, url, preview));
        }
        serde_json::from_str(body_text)
            .map_err(|e| format!("parse failed: {}", e))
    }

    fn cloud_post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let url = format!("{}{}", self.cloud_url, path);
        let body_str = serde_json::to_string(body).unwrap();
        let output = std::process::Command::new("curl")
            .args([
                "-s", "--max-time", "60",
                "-w", "\n%{http_code}",
                "-X", "POST",
                "-H", &format!("Authorization: Bearer {}", self.current_api_key()),
                "-H", "Content-Type: application/json",
                "-d", &body_str,
                &url,
            ])
            .output()
            .map_err(|e| format!("curl failed: {}", e))?;
        let raw = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = raw.rsplitn(2, '\n').collect();
        let status_code = parts.first().unwrap_or(&"0").trim();
        let body_text = if parts.len() > 1 { parts[1] } else { &raw };
        if !output.status.success() || status_code.starts_with('4') || status_code.starts_with('5') {
            let preview = if body_text.len() > 200 { &body_text[..200] } else { body_text };
            return Err(format!("HTTP {} from {}: {}", status_code, url, preview));
        }
        serde_json::from_str(body_text)
            .map_err(|e| format!("parse failed: {}", e))
    }

    /// Check if a repo has been indexed (has nodes in the cloud graph).
    /// Caches result per session so we only check once per repo.
    fn is_indexed(&self, repo_name: &str) -> bool {
        // Check session cache first
        if self.indexed_repos.borrow().contains(repo_name) {
            return true;
        }

        // Check cloud for existing nodes
        let body = json!({
            "tool": "graph_stats",
            "input": {"project_id": repo_name},
        });
        match self.cloud_post("/api/v1/tools/call", &body) {
            Ok(resp) => {
                let total = resp.get("result")
                    .and_then(|r| r.get("total_nodes"))
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0);
                if total > 0 {
                    eprintln!("[savants] '{}' has {} nodes in cloud graph", repo_name, total);
                    self.indexed_repos.borrow_mut().insert(repo_name.to_string());
                    true
                } else {
                    false
                }
            }
            Err(_) => false,
        }
    }

    /// Auto-index the current repo and retry the tool call.
    fn auto_index_and_retry(&self, repo_name: &str, original_body: &Value) -> Option<String> {
        let repo_path = std::env::current_dir().ok()?;

        // Parse the repo
        eprintln!("[savants] Parsing {}...", repo_path.display());
        let mut parser = crate::code_parser::CodeParser::new(repo_name);
        let result = parser.parse_repo(repo_path.to_str()?);
        eprintln!("[savants] Parsed {} files, {} functions, {} calls",
            result.files, result.entities.len(), result.call_sites.len());

        if result.entities.is_empty() {
            eprintln!("[savants] No code entities found, skipping upload");
            return None;
        }

        // Convert to cloud ingest format
        let mut nodes = vec![];
        let mut edges = vec![];

        for entity in &result.entities {
            let node_id = format!("{}:{}", entity.file, entity.name);
            nodes.push(json!({
                "id": node_id,
                "type": entity.kind,
                "name": entity.name,
                "file_path": entity.file,
                "line_start": entity.line,
                "language": "",
                "content_summary": entity.body.chars().take(300).collect::<String>(),
            }));
        }

        for call in &result.call_sites {
            let source_id = format!("{}:{}", call.caller_file, call.caller_name);
            let target_id = call.callee_name.clone();
            edges.push(json!({
                "source": source_id,
                "target": target_id,
                "type": "calls",
            }));
        }

        // Upload to cloud - first ensure project exists
        let project_body = json!({
            "name": repo_name,
            "slug": repo_name,
        });
        let _ = self.cloud_post("/api/v1/projects", &project_body);

        // Get project ID
        let project_id = match self.cloud_get("/api/v1/projects") {
            Ok(resp) => {
                resp.get("projects")
                    .and_then(|p| p.as_array())
                    .and_then(|arr| arr.iter().find(|p|
                        p.get("slug").and_then(|s| s.as_str()) == Some(repo_name) ||
                        p.get("name").and_then(|s| s.as_str()) == Some(repo_name)
                    ))
                    .and_then(|p| p.get("id"))
                    .and_then(|id| id.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| repo_name.to_string())
            }
            Err(_) => repo_name.to_string(),
        };

        // Upload in batches of 500
        let batch_size = 500;
        let total_nodes = nodes.len();
        for (i, chunk) in nodes.chunks(batch_size).enumerate() {
            let chunk_edges: Vec<_> = if i == 0 { edges.clone() } else { vec![] };
            let ingest_body = json!({
                "project_id": project_id,
                "source_type": "cli",
                "source_id": repo_name,
                "repo": repo_name,
                "entities": chunk.iter().map(|n| json!({
                    "name": n["name"],
                    "kind": n["type"],
                    "file": n["file_path"],
                    "line": n["line_start"],
                    "language": n["language"],
                    "body": n["content_summary"],
                    "params": [],
                })).collect::<Vec<_>>(),
                "calls": chunk_edges.iter().map(|e| json!({
                    "caller": e["source"],
                    "callee": e["target"],
                })).collect::<Vec<_>>(),
            });

            match self.cloud_post("/api/v1/graph/ingest", &ingest_body) {
                Ok(_) => eprintln!("[savants] Uploaded batch {}/{}", i + 1, (total_nodes + batch_size - 1) / batch_size),
                Err(e) => {
                    eprintln!("[savants] Upload failed: {}", e);
                    return None;
                }
            }
        }

        eprintln!("[savants] Index complete. Retrying tool call...");
        self.indexed_repos.borrow_mut().insert(repo_name.to_string());

        // Retry the original tool call
        match self.cloud_post("/api/v1/tools/call", original_body) {
            Ok(retry_response) => {
                let text = retry_response.get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| {
                        retry_response.get("result")
                            .map(|v| v.to_string())
                            .unwrap_or_default()
                            .leak()
                    });
                Some(text.to_string())
            }
            Err(e) => {
                eprintln!("[savants] Retry failed: {}", e);
                None
            }
        }
    }

    /// Detect the repo/project name from the current working directory.
    fn detect_repo_name() -> String {
        let repo_path = std::env::current_dir().unwrap_or_default();
        // Try git remote first
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
        // Fall back to directory name
        repo_path.file_name().unwrap_or_default().to_string_lossy().to_string()
    }

    /// Run a code tool locally using the cached index.
    fn run_local_tool(&self, tool_name: &str, args: &Value) -> String {
        let repo_path = std::env::current_dir().unwrap_or_default().to_string_lossy().to_string();

        // Detect repo name from git remote or directory name
        let repo_name = args.get("repo")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                if let Ok(output) = std::process::Command::new("git")
                    .args(["remote", "get-url", "origin"])
                    .current_dir(&repo_path)
                    .output()
                {
                    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if let Some(name) = url.rsplit('/').next() {
                        return name.trim_end_matches(".git").to_string();
                    }
                }
                std::path::Path::new(&repo_path)
                    .file_name().unwrap_or_default()
                    .to_string_lossy().to_string()
            });

        match tool_name {
            "semantic_search" => {
                let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

                if !crate::embedding_store::EmbeddingStore::exists(&repo_name) {
                    return format!("No index for '{}'. Run: savants reindex", repo_name);
                }

                let store = match crate::embedding_store::EmbeddingStore::load(&repo_name) {
                    Ok(s) => s,
                    Err(e) => return format!("Failed to load index: {}", e),
                };

                // Generate query embedding
                let query_emb = match crate::embeddings::EmbeddingEngine::new() {
                    Ok(mut engine) => match engine.embed(&[query.to_string()]) {
                        Ok(embs) if !embs.is_empty() => embs[0].clone(),
                        Ok(_) => return format!("Embedding returned empty for '{}'", query),
                        Err(e) => return format!("Embedding failed: {}", e),
                    },
                    Err(e) => return format!("Embedding engine failed: {}", e),
                };

                let results = store.search(&query_emb, limit);
                if results.is_empty() {
                    return format!("No results for '{}'", query);
                }
                let mut lines = vec![format!("=== Semantic search: '{}' ({} results) ===", query, results.len())];
                for (idx, score) in &results {
                    if let Some(entry) = store.entries.get(*idx) {
                        lines.push(format!("  {}:{} {}() [{:.3}]", entry.file, entry.line, entry.name, score));
                    }
                }
                lines.join("\n")
            }
            "git_blame" => {
                let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
                let start = args.get("line_start").and_then(|v| v.as_u64()).unwrap_or(1);
                let end = args.get("line_end").and_then(|v| v.as_u64()).unwrap_or(start + 20);
                let output = std::process::Command::new("git")
                    .args(["blame", "-L", &format!("{},{}", start, end), "--porcelain", file])
                    .current_dir(&repo_path)
                    .output();
                match output {
                    Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
                    Err(e) => format!("git blame failed: {}", e),
                }
            }
            "git_log" => {
                let file = args.get("file").and_then(|v| v.as_str());
                let n = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10);
                let mut cmd_args = vec!["log", "--oneline", "-n"];
                let n_str = n.to_string();
                cmd_args.push(&n_str);
                if let Some(f) = file {
                    cmd_args.push("--");
                    cmd_args.push(f);
                }
                let output = std::process::Command::new("git")
                    .args(&cmd_args)
                    .current_dir(&repo_path)
                    .output();
                match output {
                    Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
                    Err(e) => format!("git log failed: {}", e),
                }
            }
            "session_stats" => {
                format!("Repo: {}\nPath: {}\nIndex: {}", repo_name, repo_path,
                    if crate::embedding_store::EmbeddingStore::exists(&repo_name) { "cached" } else { "not indexed" })
            }
            _ => format!("Tool '{}' not available locally", tool_name),
        }
    }

    /// Log tool call metrics to ~/.savants/tool-stats.jsonl
    fn log_tool_call(tool: &str, duration_ms: u64, tokens: usize, success: bool) {
        let _ = (|| -> Result<(), Box<dyn std::error::Error>> {
            let log_path = dirs::home_dir()
                .unwrap_or_default()
                .join(".savants")
                .join("tool-stats.jsonl");
            let entry = json!({
                "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                "tool": tool,
                "duration_ms": duration_ms,
                "tokens": tokens,
                "ok": success,
            });
            let mut file = std::fs::OpenOptions::new()
                .create(true).append(true).open(&log_path)?;
            use std::io::Write;
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
            Ok(())
        })();
    }

    fn response(&self, id: &Value, result: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    }

    fn error(&self, id: &Value, code: i32, message: &str) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
    }
}

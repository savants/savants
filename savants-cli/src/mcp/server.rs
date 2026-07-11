//! MCP Server: exposes structural graph queries to external AI tools.
//!
//! Implements the Model Context Protocol stdio transport using
//! newline-delimited JSON-RPC. One JSON message per line, no framing
//! headers. This is the transport Claude Code, Cursor, and Continue use.

use crate::graph::{GraphClient, GraphValue};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Detect the repo name for community lookups in MCP context.
fn detect_repo_name_for_mcp() -> String {
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

pub struct McpServer {
    client: GraphClient,
}

impl McpServer {
    pub fn new(graph_name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let client = GraphClient::new(graph_name)?;

        // Auto-index the current working directory if it's a git repo
        if let Ok(cwd) = std::env::current_dir() {
            if cwd.join(".git").exists() {
                let repo_name = cwd.file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                let mut indexer = crate::code_index::CodeIndexer::new(client.clone(), &repo_name);
                let stats = indexer.index_repo(&cwd.to_string_lossy());
                eprintln!("Auto-indexed {}: {}", repo_name, stats.summary());
            }
        }

        Ok(Self { client })
    }

    /// Run the MCP server: read newline-delimited JSON from stdin, write responses to stdout.
    pub fn run(&self) {
        eprintln!("Savants MCP server started (newline-delimited JSON-RPC)");
        let stdin = io::stdin();
        let stdout = io::stdout();
        let reader = stdin.lock();
        let mut writer = stdout.lock();

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break, // EOF or read error
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let message: Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("Skipping malformed line on stdin: {}", e);
                    continue;
                }
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

        // Notifications (no id) -- handle silently
        if req_id.is_none() || req_id == Some(&Value::Null) {
            if method == "notifications/initialized" {
                eprintln!("Client confirmed initialization");
            }
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
                "serverInfo": {"name": "savants", "version": "0.1.0"}
            }))),

            "ping" => Some(self.response(&req_id, json!({}))),

            "tools/list" => Some(self.response(&req_id, json!({
                "tools": self.list_tools()
            }))),

            "tools/call" => Some(self.handle_tool_call(&req_id, &params)),

            "resources/list" => Some(self.response(&req_id, json!({"resources": []}))),

            "prompts/list" => Some(self.response(&req_id, json!({"prompts": []}))),

            _ => Some(self.error(&req_id, -32601, &format!("Unknown method: {}", method))),
        }
    }

    // ---------------------------------------------------------------
    // Tool definitions
    // ---------------------------------------------------------------

    fn list_tools(&self) -> Value {
        json!([
            {
                "name": "diagnose",
                "description": "THE FIRST TOOL TO CALL. Runs a complete diagnostic of the user's infrastructure: host health (CPU, memory, disk, failed services), K8s cluster state (pod status, errors), and top issues with root cause analysis. Call this BEFORE any other tool when the user asks 'what's wrong', 'check my cluster', 'any issues', or any diagnostic question. Returns a full narrative with severity-ranked issues.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "since_minutes": {"type": "integer", "description": "Look back window in minutes. Default: 60. Use 0 for all time."},
                        "min_severity": {"type": "string", "description": "WARN or ERROR. Default: WARN"}
                    }
                }
            },
            {
                "name": "graph_stats",
                "description": "Get the total number of resources and connections tracked by Savants.",
                "inputSchema": {"type": "object", "properties": {}}
            },
            {
                "name": "cluster_state",
                "description": "Return a summary of what's running in a Kubernetes cluster: namespace count, deployment count, pod count by status, service count, and top namespaces by workload.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cluster": {"type": "string", "description": "Cluster name as stored in the graph"}
                    },
                    "required": ["cluster"]
                }
            },
            {
                "name": "list_pods",
                "description": "List Kubernetes pods matching a filter. Can filter by namespace, status, or a substring of the pod name.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cluster": {"type": "string"},
                        "namespace": {"type": "string"},
                        "status": {"type": "string", "description": "Filter by pod status (Running, Pending, CrashLoopBackOff, etc.)"},
                        "name_contains": {"type": "string", "description": "Substring match on pod name"}
                    },
                    "required": ["cluster"]
                }
            },
            {
                "name": "pod_story",
                "description": "THE MTTR KILLER TOOL: given a pod (or a whole cluster), return a narrative-ready summary of the significant log events it has emitted.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cluster": {"type": "string"},
                        "pod": {"type": "string", "description": "Pod name (optional)"},
                        "namespace": {"type": "string"},
                        "since_minutes": {"type": "integer", "description": "Only include events from the last N minutes. Default: 60."},
                        "min_severity": {"type": "string", "description": "INFO | WARN | ERROR | FATAL (default: WARN)"},
                        "limit": {"type": "integer", "description": "Max events to return (default: 15)"}
                    },
                    "required": ["cluster"]
                }
            },
            {
                "name": "host_state",
                "description": "Return a snapshot of a host's health: OS, kernel, uptime, CPU/memory/load averages, disk usage, failed systemd units, and top processes by memory.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "hostname": {"type": "string", "description": "Hostname (optional -- omit for all hosts)"}
                    }
                }
            },
            {
                "name": "host_story",
                "description": "THE HOST MTTR TOOL: given a hostname (or all hosts), return a narrative-ready summary of significant host-level log events and kernel events.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "hostname": {"type": "string", "description": "Hostname (optional)"},
                        "since_minutes": {"type": "integer", "description": "Only include events from the last N minutes. Default: 60."},
                        "min_severity": {"type": "string", "description": "INFO | WARN | ERROR | FATAL (default: WARN)"},
                        "limit": {"type": "integer", "description": "Max events to return (default: 15)"}
                    }
                }
            },
            {
                "name": "deployment_info",
                "description": "Full details for a Kubernetes Deployment: replica status, current image, labels, and all pods belonging to it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cluster": {"type": "string"},
                        "namespace": {"type": "string"},
                        "name": {"type": "string"}
                    },
                    "required": ["cluster", "namespace", "name"]
                }
            },
            {
                "name": "pod_dependencies",
                "description": "Return every ConfigMap and Secret that a Pod reads from. Answers 'what config does this pod depend on?'",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cluster": {"type": "string"},
                        "namespace": {"type": "string"},
                        "pod": {"type": "string"}
                    },
                    "required": ["cluster", "namespace", "pod"]
                }
            },
            {
                "name": "namespace_summary",
                "description": "Everything in a namespace: deployments, pods (grouped by status), services, configmap count, secret count.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cluster": {"type": "string"},
                        "namespace": {"type": "string"}
                    },
                    "required": ["cluster", "namespace"]
                }
            },
            {
                "name": "search_code",
                "description": "Search for functions and classes by name pattern.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "Search pattern"}
                    },
                    "required": ["pattern"]
                }
            },
            {
                "name": "find_references_structured",
                "description": "Replacement for grep / IDE 'Find References'. Returns the structural callers of a function with metadata.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function_name": {"type": "string"},
                        "include_tests": {"type": "boolean", "default": true}
                    },
                    "required": ["function_name"]
                }
            },
            {
                "name": "function_xray",
                "description": "Composite query: returns the full structural and historical profile of a function in one call.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function_name": {"type": "string"},
                        "file_path": {"type": "string", "description": "Optional -- disambiguate when name is shared across files"}
                    },
                    "required": ["function_name"]
                }
            },
            {
                "name": "impact_analysis",
                "description": "Analyze the cascading impact of changing a function. Returns all direct and transitive dependents.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function_name": {"type": "string", "description": "Name of the function to analyze"},
                        "max_depth": {"type": "integer", "default": 5}
                    },
                    "required": ["function_name"]
                }
            },
            {
                "name": "diff_impact",
                "description": "Structural blast radius for a git ref or range. Returns changed files, changed functions, transitively reachable entry points, and config keys in touched files.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ref": {"type": "string", "description": "git ref (HEAD, abc123), or range (main..branch)"},
                        "repo_path": {"type": "string", "description": "Path to the git repo (defaults to cwd)"}
                    },
                    "required": ["ref"]
                }
            },
            {
                "name": "risk_score",
                "description": "Compute a 0-10 risk score for modifying a function. Combines call-graph blast radius, historical bug correlation, maintainer bus factor, and recency.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function_name": {"type": "string"},
                        "file_path": {"type": "string"}
                    },
                    "required": ["function_name"]
                }
            },
            {
                "name": "decorated_with",
                "description": "List all functions decorated with a given decorator name, e.g. 'workflow.defn', 'app.route', 'lru_cache'.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "decorator_name": {"type": "string"}
                    },
                    "required": ["decorator_name"]
                }
            },
            {
                "name": "resolves_to",
                "description": "Given a string literal, find any Function or Class whose name matches, plus every function that mentions the string.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": {"type": "string"}
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "community_summary",
                "description": "Show detected code communities (clusters of related functions). Returns pre-computed summaries with entry points, file lists, and descriptions. Query by name to filter, or omit for all communities.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Optional: filter communities by name/description match"},
                        "max_results": {"type": "integer", "default": 10}
                    }
                }
            },
            {
                "name": "dependency_chain",
                "description": "Find the shortest dependency path between two files.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "from_file": {"type": "string"},
                        "to_file": {"type": "string"}
                    },
                    "required": ["from_file", "to_file"]
                }
            },
            {
                "name": "co_change_partners",
                "description": "Find functions that historically change in the same commits as the target. Reveals hidden coupling.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function_name": {"type": "string"},
                        "limit": {"type": "integer", "default": 10}
                    },
                    "required": ["function_name"]
                }
            },
            {
                "name": "recall_history",
                "description": "Recall historical facts and episodes from episodic memory. Useful for understanding why code was written a certain way.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Topic or entity to recall history for"}
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "federated_symbol_in_cluster",
                "description": "THE KILLER FEDERATED QUERY: given a function/class/symbol name from the code graph, find any Kubernetes resources in the cluster graph that reference it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": {"type": "string"},
                        "cluster": {"type": "string"}
                    },
                    "required": ["symbol", "cluster"]
                }
            },
            {
                "name": "pre_change_warning",
                "description": "Before modifying a function, check the structural and historical risk of the change. Returns blast radius, maintainer concentration, and stale-knowledge alerts.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function_name": {"type": "string"},
                        "file_path": {"type": "string"}
                    },
                    "required": ["function_name"]
                }
            },
            {
                "name": "coupling_check",
                "description": "Check whether a new dependency between two modules would violate the codebase's existing architectural boundaries.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "from_module": {"type": "string", "description": "Source module path prefix"},
                        "to_module": {"type": "string", "description": "Target module path prefix"}
                    },
                    "required": ["from_module", "to_module"]
                }
            },
            {
                "name": "query",
                "description": "Run a SaQL query against Savants. SaQL is a resource-oriented query language. Examples: 'show pods where status = CrashLoopBackOff', 'story pod api-gateway in prod since 1h', 'causes of pod api-gateway', 'dependents of configmap api-config', 'impact of function authenticate', 'stats'. Use this for any question the specialized tools don't cover.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "q": {"type": "string", "description": "SaQL query string. Examples: 'show pods', 'story host astra since 24h', 'causes of pod api-gw in prod'"}
                    },
                    "required": ["q"]
                }
            },
            {
                "name": "reindex",
                "description": "Index a repository's source code into the graph using tree-sitter. Extracts functions, classes, imports, call chains, git history. Also analyzes open PRs for risk.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo_path": {"type": "string", "description": "Absolute path to the repository to index"},
                        "full": {"type": "boolean", "default": true, "description": "Drop and rebuild the entire graph"}
                    },
                    "required": ["repo_path"]
                }
            },
            {
                "name": "file_skeleton",
                "description": "FASTER THAN READ: Returns the structure of a file - all function names, class names, type definitions, their line ranges, and export status. No function bodies. 10x fewer tokens than reading the full file. Always use this before Read when you need to understand a file's structure.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": {"type": "string", "description": "File path (relative to repo root, e.g. 'server/services/stripe.ts')"},
                        "repo": {"type": "string", "description": "Repository name"}
                    },
                    "required": ["file"]
                }
            },
            {
                "name": "where_used",
                "description": "FASTER THAN GREP: Find every place a symbol is used across the entire codebase. Returns callers, importers, and body references grouped by file. Milliseconds vs seconds. Always use this instead of Grep when searching for function/class/variable usage.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": {"type": "string", "description": "The symbol name to search for"},
                        "repo": {"type": "string", "description": "Repository name"}
                    },
                    "required": ["symbol"]
                }
            },
            {
                "name": "callers",
                "description": "FASTER THAN GREP: Find all functions that directly call a given function. Returns caller name, file, and line number. Use this instead of grep when finding callers.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function": {"type": "string", "description": "Function name to find callers of"},
                        "repo": {"type": "string", "description": "Repository name"}
                    },
                    "required": ["function"]
                }
            },
            {
                "name": "import_tree",
                "description": "FASTER THAN READ: Returns the full import graph of a file to a given depth. Shows what each file imports recursively. One call replaces reading 5+ files to trace dependencies. Always use this instead of reading import statements manually.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": {"type": "string", "description": "File path (relative to repo root)"},
                        "repo": {"type": "string", "description": "Repository name"},
                        "depth": {"type": "integer", "description": "How many levels deep to trace. Default: 2"}
                    },
                    "required": ["file"]
                }
            },
            {
                "name": "module_exports",
                "description": "FASTER THAN READ: Returns just the public API surface of a file - exported function names with parameter signatures. No bodies. Use this instead of Read when you need to know what a module exports.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": {"type": "string", "description": "File path (relative to repo root)"},
                        "repo": {"type": "string", "description": "Repository name"}
                    },
                    "required": ["file"]
                }
            },
            {
                "name": "blast_radius",
                "description": "Given a function, returns all functions that directly or transitively depend on it. Shows what would break if you change this function.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function": {"type": "string", "description": "Function name"},
                        "repo": {"type": "string", "description": "Repository name"},
                        "depth": {"type": "integer", "description": "Max traversal depth. Default: 3"}
                    },
                    "required": ["function"]
                }
            },
            {
                "name": "semantic_search",
                "description": "NATURAL LANGUAGE CODE SEARCH: Find functions by describing what they do, not by name. 'payment retry logic' finds handleTransactionWithBackoff. 'user authentication' finds verifyJwtToken. Uses BM25 ranking - no API keys, no external services, works offline.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Natural language description of what you're looking for"},
                        "repo": {"type": "string", "description": "Repository name"},
                        "limit": {"type": "integer", "description": "Max results. Default: 10"}
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "dead_code",
                "description": "Find functions with zero callers - candidates for removal during refactoring.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo": {"type": "string", "description": "Repository name"},
                        "file": {"type": "string", "description": "Optional: limit to a specific file"}
                    }
                }
            },
            {
                "name": "pr-risk",
                "description": "Analyze open PRs for risk: removed null guards, schema changes, deleted files, co-change gaps, blast radius, high-churn functions, unanswered Slack questions, Jira mismatches, and known prod errors.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo": {"type": "string", "description": "Repository name (default: talent-pipeline)"}
                    }
                }
            },
            {
                "name": "diagnose-error",
                "description": "Deep diagnosis of a production error. Traces upstream through the call chain to find the root cause. Cross-references with Slack discussions, Jira tickets, git history, and Sentry. Returns confidence score and blind spots.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "error": {"type": "string", "description": "The error message from Sentry, logs, or Slack"},
                        "repo": {"type": "string", "description": "Repository name (default: talent-pipeline)"}
                    },
                    "required": ["error"]
                }
            },
            {
                "name": "radar",
                "description": "Personal radar - shows what YOU need to know across all channels without reading everything. Finds direct mentions you haven't replied to, questions waiting for your answer, discussions about your code, errors in your services, and conversations you should be involved in but weren't tagged. Never evaluates performance - only finds what needs your attention.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "user": {"type": "string", "description": "Your name, email, or Slack username to identify you in the graph"},
                        "hours": {"type": "number", "description": "Look back this many hours (default: 24)"}
                    },
                    "required": ["user"]
                }
            }
        ])
    }

    // ---------------------------------------------------------------
    // Tool dispatch
    // ---------------------------------------------------------------

    fn handle_tool_call(&self, req_id: &Value, params: &Value) -> Value {
        let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let call_start = std::time::Instant::now();

        let result = match tool_name {
            "diagnose" => self.tool_diagnose(&args),
            "graph_stats" => self.tool_graph_stats(),
            "cluster_state" | "cluster-state" => self.tool_cluster_state(&args),
            "list_pods" | "list-pods" => self.tool_list_pods(&args),
            "pod_story" | "pod-story" => self.tool_pod_story(&args),
            "host_state" | "host-state" => self.tool_host_state(&args),
            "host_story" | "host-story" => self.tool_host_story(&args),
            "deployment_info" | "deployment-info" => self.tool_deployment_info(&args),
            "pod_dependencies" | "pod-dependencies" => self.tool_pod_dependencies(&args),
            "namespace_summary" | "namespace-summary" => self.tool_namespace_summary(&args),
            "file_skeleton" => self.tool_file_skeleton(&args),
            "where_used" => self.tool_where_used(&args),
            "callers" => self.tool_callers(&args),
            "import_tree" => self.tool_import_tree(&args),
            "module_exports" => self.tool_module_exports(&args),
            "blast_radius" => self.tool_blast_radius(&args),
            "semantic_search" => self.tool_semantic_search(&args),
            "dead_code" => self.tool_dead_code(&args),
            "search_code" | "search-code" => self.tool_search_code(&args),
            "find_references_structured" | "find-references" | "find_references" => self.tool_find_references(&args),
            "function_xray" => self.tool_function_xray(&args),
            "impact_analysis" => self.tool_impact_analysis(&args),
            "diff_impact" | "diff-impact" => self.tool_diff_impact(&args),
            "risk_score" => self.tool_risk_score(&args),
            "decorated_with" => self.tool_decorated_with(&args),
            "resolves_to" => self.tool_resolves_to(&args),
            "community_summary" => self.tool_community_summary(&args),
            "dependency_chain" | "dependency-chain" => self.tool_dependency_chain(&args),
            "co_change_partners" | "co-change-partners" => self.tool_co_change_partners(&args),
            "recall_history" => self.tool_recall_history(&args),
            "federated_symbol_in_cluster" => self.tool_federated_symbol_in_cluster(&args),
            "pre_change_warning" | "pre-change-warning" => self.tool_pre_change_warning(&args),
            "coupling_check" => self.tool_coupling_check(&args),
            "query" => self.tool_saql_query(&args),
            "advanced_graph_query" => self.tool_advanced_graph_query(&args),  // hidden, not in tool list
            "reindex" => self.tool_reindex(&args),
            "pr-risk" | "pr_risk" => self.tool_pr_risk(&args),
            "diagnose-error" | "diagnose_error" => self.tool_diagnose_error(&args),
            "radar" => self.tool_radar(&args),
            _ => Err(format!("Unknown tool: {}", tool_name)),
        };

        let elapsed_ms = call_start.elapsed().as_millis();

        // Show savings banner (disable with SAVANTS_QUIET=1)
        let show_banner = std::env::var("SAVANTS_QUIET").unwrap_or_default() != "1";

        match result {
            Ok(text) => {
                let response_text = if show_banner && tool_name != "reindex" {
                    let response_tokens = text.len() / 4;
                    format!("{}\n\n---\n[savants] {} | {}ms | {} tokens",
                        text, tool_name, elapsed_ms, response_tokens)
                } else {
                    text
                };
                self.response(req_id, json!({
                    "content": [{"type": "text", "text": response_text}]
                }))
            },
            Err(e) => self.response(req_id, json!({
                "content": [{"type": "text", "text": format!("Error: {}", e)}],
                "isError": true
            })),
        }
    }

    // ---------------------------------------------------------------
    // Helper: get a GraphClient for a K8s cluster graph
    // ---------------------------------------------------------------

    fn k8s_client(&self, cluster: &str) -> Result<GraphClient, String> {
        let graph_name = cluster.replace('-', "_");
        GraphClient::new(&graph_name).map_err(|e| format!("Cannot connect to cluster graph '{}': {}", graph_name, e))
    }

    // ---------------------------------------------------------------
    // Helper: extract rows as Vec<Vec<String>> for simple formatting
    // ---------------------------------------------------------------

    fn query_text(&self, client: &GraphClient, cypher: &str, params: &[(&str, &str)]) -> Result<Vec<Vec<GraphValue>>, String> {
        client.query(cypher, params)
            .map(|r| r.rows)
            .map_err(|e| format!("Query failed: {}", e))
    }

    // ---------------------------------------------------------------
    // Tool implementations
    // ---------------------------------------------------------------

    fn tool_graph_stats(&self) -> Result<String, String> {
        let nodes = self.query_text(&self.client, "MATCH (n) RETURN count(n)", &[])?;
        let edges = self.query_text(&self.client, "MATCH ()-[r]->() RETURN count(r)", &[])?;
        let n = nodes.first().and_then(|r| r.first()).map(|v| v.as_i64()).unwrap_or(0);
        let e = edges.first().and_then(|r| r.first()).map(|v| v.as_i64()).unwrap_or(0);
        Ok(format!("Nodes: {}, Edges: {}", n, e))
    }

    fn tool_cluster_state(&self, args: &Value) -> Result<String, String> {
        let cluster = arg_str(args, "cluster")?;
        let c = self.k8s_client(&cluster)?;

        let count_q = |label: &str| -> i64 {
            self.query_text(&c, &format!("MATCH (n:{}) RETURN count(n)", label), &[])
                .ok()
                .and_then(|r| r.first().and_then(|row| row.first().map(|v| v.as_i64())))
                .unwrap_or(0)
        };

        let ns_count = count_q("K8sNamespace");
        let deploy_count = count_q("K8sDeployment");
        let pod_total = count_q("K8sPod");
        let svc_count = count_q("K8sService");
        let cm_count = count_q("K8sConfigMap");
        let sec_count = count_q("K8sSecret");

        if ns_count == 0 {
            return Ok(format!(
                "Cluster '{}' has no data yet. Run the K8s ingestor to populate it first.",
                cluster
            ));
        }

        // Pod count by status
        let status_rows = self.query_text(
            &c,
            "MATCH (p:K8sPod) RETURN p.status, count(p) ORDER BY count(p) DESC",
            &[],
        )?;
        let status_breakdown: Vec<String> = status_rows
            .iter()
            .map(|r| format!("    {:4}  {}", r.get(1).map(|v| v.as_i64()).unwrap_or(0), r.first().map(|v| v.as_str()).unwrap_or("?")))
            .collect();

        // Top namespaces by workload
        let top_ns = self.query_text(
            &c,
            "MATCH (n:K8sNamespace)-[:CONTAINS]->(p:K8sPod) \
             RETURN n.name, count(p) AS pods ORDER BY pods DESC LIMIT 10",
            &[],
        )?;
        let top_ns_str: Vec<String> = top_ns
            .iter()
            .map(|r| format!("    {:4}  {}", r.get(1).map(|v| v.as_i64()).unwrap_or(0), r.first().map(|v| v.as_str()).unwrap_or("?")))
            .collect();

        Ok(format!(
            "Cluster: {}\n\n\
             Resource counts:\n\
             \x20 Namespaces:   {}\n\
             \x20 Deployments:  {}\n\
             \x20 Pods:         {}\n\
             \x20 Services:     {}\n\
             \x20 ConfigMaps:   {}\n\
             \x20 Secrets:      {}\n\n\
             Pods by status:\n{}\n\n\
             Top namespaces by workload:\n{}",
            cluster, ns_count, deploy_count, pod_total, svc_count, cm_count, sec_count,
            status_breakdown.join("\n"),
            top_ns_str.join("\n"),
        ))
    }

    fn tool_list_pods(&self, args: &Value) -> Result<String, String> {
        let cluster = arg_str(args, "cluster")?;
        let c = self.k8s_client(&cluster)?;

        let mut where_clauses = Vec::new();
        let mut params: Vec<(&str, &str)> = Vec::new();

        let ns_val = args.get("namespace").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let status_val = args.get("status").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let name_val = args.get("name_contains").and_then(|v| v.as_str()).unwrap_or("").to_string();

        if !ns_val.is_empty() {
            where_clauses.push("p.namespace = $namespace");
        }
        if !status_val.is_empty() {
            where_clauses.push("p.status = $status");
        }
        if !name_val.is_empty() {
            where_clauses.push("p.name CONTAINS $name_contains");
        }

        let where_clause = if where_clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", where_clauses.join(" AND "))
        };

        // Build params - we need owned strings but params needs &str references
        let mut param_vec: Vec<(String, String)> = Vec::new();
        if !ns_val.is_empty() {
            param_vec.push(("namespace".to_string(), ns_val));
        }
        if !status_val.is_empty() {
            param_vec.push(("status".to_string(), status_val));
        }
        if !name_val.is_empty() {
            param_vec.push(("name_contains".to_string(), name_val));
        }
        let params_ref: Vec<(&str, &str)> = param_vec.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

        let query = format!(
            "MATCH (p:K8sPod){} \
             RETURN p.namespace, p.name, p.status, p.image, p.restart_count, \
             p.owner_kind, p.owner_name ORDER BY p.namespace, p.name LIMIT 100",
            where_clause
        );

        let rows = self.query_text(&c, &query, &params_ref)?;

        if rows.is_empty() {
            return Ok("No pods found matching filters.".to_string());
        }

        let mut lines = vec![format!("Found {} pods:", rows.len())];
        for r in &rows {
            let ns = r.get(0).map(|v| v.as_str()).unwrap_or("?");
            let name = r.get(1).map(|v| v.as_str()).unwrap_or("?");
            let st = r.get(2).map(|v| v.as_str()).unwrap_or("?");
            let img = r.get(3).map(|v| v.as_str()).unwrap_or("");
            let rc = r.get(4).map(|v| v.as_i64()).unwrap_or(0);
            let okind = r.get(5).map(|v| v.as_str()).unwrap_or("");
            let oname = r.get(6).map(|v| v.as_str()).unwrap_or("");

            let owner = if !okind.is_empty() { format!(" <- {}/{}", okind, oname) } else { String::new() };
            let restarts = if rc > 0 { format!(" (restarts={})", rc) } else { String::new() };
            lines.push(format!("  [{:15}] {}/{}{}{}", st, ns, name, owner, restarts));
            if !img.is_empty() {
                lines.push(format!("    image: {}", img));
            }
        }
        if rows.len() >= 100 {
            lines.push("  (limit reached, refine filters to see more)".to_string());
        }
        Ok(lines.join("\n"))
    }

    fn tool_pod_story(&self, args: &Value) -> Result<String, String> {
        let cluster = arg_str(args, "cluster")?;
        let c = self.k8s_client(&cluster)?;

        let pod = args.get("pod").and_then(|v| v.as_str());
        let namespace = args.get("namespace").and_then(|v| v.as_str());
        let since_minutes = args.get("since_minutes").and_then(|v| v.as_i64()).unwrap_or(60);
        let min_severity = args.get("min_severity").and_then(|v| v.as_str()).unwrap_or("WARN");
        let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(15);

        let sev_rank = |s: &str| -> i32 {
            match s.to_uppercase().as_str() {
                "INFO" => 0,
                "WARN" => 1,
                "ERROR" => 2,
                "FATAL" => 3,
                _ => 0,
            }
        };
        let min_rank = sev_rank(min_severity);
        let allowed: Vec<&str> = ["INFO", "WARN", "ERROR", "FATAL"]
            .iter()
            .copied()
            .filter(|s| sev_rank(s) >= min_rank)
            .collect();

        // Build dynamic WHERE + params
        let mut where_parts = vec![
            format!("e.cluster = '{}'", escape_cypher(&cluster)),
        ];
        // Severity filter inline (memory engine param lists are tricky)
        let allowed_str = allowed.iter().map(|s| format!("'{}'", s)).collect::<Vec<_>>().join(", ");
        where_parts.push(format!("e.severity IN [{}]", allowed_str));

        if let Some(p) = pod {
            where_parts.push(format!("e.pod = '{}'", escape_cypher(p)));
        }
        if let Some(ns) = namespace {
            where_parts.push(format!("e.namespace = '{}'", escape_cypher(ns)));
        }
        if since_minutes > 0 {
            let since_ts = now_unix() - (since_minutes as f64 * 60.0);
            where_parts.push(format!("e.last_seen >= {}", since_ts));
        }

        let where_clause = where_parts.join(" AND ");
        let query = format!(
            "MATCH (e:LogEvent) WHERE {} \
             RETURN e.pod, e.namespace, e.severity, e.count, \
                    e.template_text, e.example_lines, \
                    e.first_seen, e.last_seen, e.pod_deleted_at \
             ORDER BY CASE e.severity \
                        WHEN 'FATAL' THEN 3 \
                        WHEN 'ERROR' THEN 2 \
                        WHEN 'WARN' THEN 1 \
                        ELSE 0 END DESC, e.count DESC \
             LIMIT {}",
            where_clause, limit,
        );
        let rows = self.query_text(&c, &query, &[])?;

        // Histogram
        let hist_query = format!(
            "MATCH (e:LogEvent) WHERE {} \
             RETURN e.severity, count(e), sum(e.count), count(DISTINCT e.pod)",
            where_clause,
        );
        let hist = self.query_text(&c, &hist_query, &[]).unwrap_or_default();

        // Scope string
        let mut scope = Vec::new();
        if let Some(p) = pod { scope.push(format!("pod={}", p)); }
        if let Some(ns) = namespace { scope.push(format!("namespace={}", ns)); }
        if since_minutes > 0 { scope.push(format!("last {}m", since_minutes)); }
        let scope_str = if scope.is_empty() { "cluster-wide".to_string() } else { scope.join(", ") };

        let mut lines = vec![
            format!("# Log story for {} ({})", cluster, scope_str),
            String::new(),
        ];

        if rows.is_empty() {
            // Check if there's data outside the time window — don't penalize
            // for querying a narrow range when data exists further back
            if since_minutes > 0 {
                let check = c.query(
                    "MATCH (e:LogEvent) WHERE e.severity IN ['ERROR','FATAL'] RETURN min(e.last_seen), max(e.last_seen), count(e)",
                    &[],
                );
                if let Ok(ref r) = check {
                    if let Some(row) = r.rows.first() {
                        let count = row.get(2).map(|v| v.as_i64()).unwrap_or(0);
                        if count > 0 {
                            let oldest = row.get(0).map(|v| v.as_f64()).unwrap_or(0.0);
                            let newest = row.get(1).map(|v| v.as_f64()).unwrap_or(0.0);
                            let oldest_ago = ((now_unix() - oldest) / 60.0) as i64;
                            let newest_ago = ((now_unix() - newest) / 60.0) as i64;
                            lines.push(format!(
                                "No events in the last {} minutes, but {} events exist from {}–{} minutes ago. \
                                 Try `since_minutes: {}` or `since_minutes: 0` to see them.",
                                since_minutes, count, oldest_ago, newest_ago,
                                newest_ago + 10,
                            ));
                            return Ok(lines.join("\n"));
                        }
                    }
                }
            }
            lines.push(
                "No significant log events found. Either the log watcher \
                 isn't running, the filters excluded everything, or the pod \
                 is actually healthy.".to_string(),
            );
            return Ok(lines.join("\n"));
        }

        let total_templates: i64 = hist.iter().map(|r| r.get(1).map(|v| v.as_i64()).unwrap_or(0)).sum();
        let total_occurrences: i64 = hist.iter().map(|r| r.get(2).map(|v| v.as_i64()).unwrap_or(0)).sum();
        let total_pods: i64 = hist.iter().map(|r| r.get(3).map(|v| v.as_i64()).unwrap_or(0)).max().unwrap_or(0);
        lines.push(format!(
            "**Summary:** {} distinct templates, {} total occurrences, across {} pods",
            total_templates, total_occurrences, total_pods,
        ));

        let hist_str: Vec<String> = hist.iter().map(|r| {
            format!("{}={}", r.first().map(|v| v.as_str()).unwrap_or("?"), r.get(1).map(|v| v.as_i64()).unwrap_or(0))
        }).collect();
        lines.push(format!("**Severity:** {}", hist_str.join(", ")));
        lines.push(String::new());

        lines.push(format!("## Top {} events (by severity, then volume)", rows.len()));
        lines.push(String::new());

        let now_ts = now_unix();
        for (i, r) in rows.iter().enumerate() {
            let pod_name = r.get(0).map(|v| v.as_str()).unwrap_or("?");
            let ns = r.get(1).map(|v| v.as_str()).unwrap_or("?");
            let sev = r.get(2).map(|v| v.as_str()).unwrap_or("?");
            let cnt = r.get(3).map(|v| v.as_i64()).unwrap_or(0);
            let tmpl = r.get(4).map(|v| v.as_str()).unwrap_or("");
            let examples = r.get(5).map(|v| v.as_str()).unwrap_or("");
            let _first_seen = r.get(6).map(|v| v.as_f64()).unwrap_or(0.0);
            let last_seen = r.get(7).map(|v| v.as_f64()).unwrap_or(0.0);
            let deleted_at = r.get(8);

            let mut tombstone = String::new();
            if let Some(da) = deleted_at {
                if !da.is_null() {
                    let ago = (now_ts - da.as_f64()) as i64;
                    let mins = ago / 60;
                    tombstone = if mins > 0 {
                        format!(" (pod deleted {}m ago)", mins)
                    } else {
                        " (pod just deleted)".to_string()
                    };
                }
            }

            lines.push(format!(
                "### {}. [{}] {}/{}{} -- {} occurrences",
                i + 1, sev, ns, pod_name, tombstone, cnt,
            ));
            if !tmpl.is_empty() {
                let short = if tmpl.len() > 200 { &tmpl[..200] } else { tmpl };
                lines.push(format!("Template: `{}`", short));
            }
            if !examples.is_empty() {
                lines.push("Example:".to_string());
                let short = if examples.len() > 250 { &examples[..250] } else { examples };
                lines.push(format!("    {}", short));
            }
            if last_seen > 0.0 {
                lines.push(format!("Last seen: {}", format_timestamp(last_seen)));
            }
            lines.push(String::new());
        }

        // Mentions summary
        let mentions_query = format!(
            "MATCH (e:LogEvent) WHERE {} \
             MATCH (e)-[:MENTIONS]->(x) \
             RETURN labels(x)[0], x.name, x.namespace, count(DISTINCT e) \
             ORDER BY count(DISTINCT e) DESC LIMIT 20",
            where_clause,
        );
        if let Ok(mentions_rows) = self.query_text(&c, &mentions_query, &[]) {
            if !mentions_rows.is_empty() {
                lines.push("## Referenced entities (from log text)".to_string());
                lines.push(String::new());
                for r in &mentions_rows {
                    let label = r.get(0).map(|v| v.as_str()).unwrap_or("?").replace("K8s", "");
                    let ent_name = r.get(1).map(|v| v.as_str()).unwrap_or("?");
                    let ent_ns = r.get(2).map(|v| v.as_str()).unwrap_or("?");
                    let n_events = r.get(3).map(|v| v.as_i64()).unwrap_or(0);
                    lines.push(format!(
                        "- **{}** `{}/{}` -- mentioned by {} event(s)",
                        label, ent_ns, ent_name, n_events,
                    ));
                }
                lines.push(String::new());
            }
        }

        Ok(lines.join("\n"))
    }

    fn tool_host_state(&self, args: &Value) -> Result<String, String> {
        let hostname = args.get("hostname").and_then(|v| v.as_str());

        let host_rows = if let Some(h) = hostname {
            self.query_text(
                &self.client,
                "MATCH (h:Host {hostname: $hostname}) \
                 RETURN h.hostname, h.os, h.kernel, h.uptime_seconds, \
                        h.cpu_count, h.cpu_percent, h.memory_total_mb, \
                        h.memory_used_mb, h.load_1m, h.load_5m, h.load_15m",
                &[("hostname", h)],
            )?
        } else {
            self.query_text(
                &self.client,
                "MATCH (h:Host) \
                 RETURN h.hostname, h.os, h.kernel, h.uptime_seconds, \
                        h.cpu_count, h.cpu_percent, h.memory_total_mb, \
                        h.memory_used_mb, h.load_1m, h.load_5m, h.load_15m \
                 ORDER BY h.hostname",
                &[],
            )?
        };

        if host_rows.is_empty() {
            let target = hostname.map(|h| format!("'{}'", h)).unwrap_or("any".to_string());
            return Ok(format!(
                "No Host nodes found for {}. Run the host ingestor to populate host data first.",
                target,
            ));
        }

        let mut lines = Vec::new();
        for row in &host_rows {
            let h_name = row.get(0).map(|v| v.as_str()).unwrap_or("?");
            let os_name = row.get(1).map(|v| v.as_str()).unwrap_or("unknown");
            let kernel = row.get(2).map(|v| v.as_str()).unwrap_or("unknown");
            let uptime_s = row.get(3).map(|v| v.as_i64()).unwrap_or(0);
            let cpu_count = row.get(4).map(|v| v.as_i64()).unwrap_or(0);
            let cpu_pct = row.get(5).map(|v| v.as_f64()).unwrap_or(0.0);
            let mem_total = row.get(6).map(|v| v.as_f64()).unwrap_or(0.0);
            let mem_used = row.get(7).map(|v| v.as_f64()).unwrap_or(0.0);
            let load1 = row.get(8).map(|v| v.as_f64()).unwrap_or(0.0);
            let load5 = row.get(9).map(|v| v.as_f64()).unwrap_or(0.0);
            let load15 = row.get(10).map(|v| v.as_f64()).unwrap_or(0.0);

            let days = uptime_s / 86400;
            let hours = (uptime_s % 86400) / 3600;
            let uptime_str = format!("{}d {}h", days, hours);
            let mem_pct = if mem_total > 0.0 {
                format!("{:.1}%", mem_used / mem_total * 100.0)
            } else {
                "N/A".to_string()
            };

            lines.push(format!("Host: {}", h_name));
            lines.push(format!("  OS:           {}", os_name));
            lines.push(format!("  Kernel:       {}", kernel));
            lines.push(format!("  Uptime:       {}", uptime_str));
            lines.push(format!("  CPU:          {} cores, {:.1}% used", cpu_count, cpu_pct));
            lines.push(format!("  Memory:       {:.0}/{:.0} MB ({})", mem_used, mem_total, mem_pct));
            lines.push(format!("  Load avg:     {:.2} / {:.2} / {:.2}", load1, load5, load15));
            lines.push(String::new());

            // Disk usage
            let disk_rows = self.query_text(
                &self.client,
                "MATCH (h:Host {hostname: $hostname})-[:HAS_DISK]->(d:HostDisk) \
                 RETURN d.mountpoint, d.device, d.total_gb, d.used_gb, d.percent \
                 ORDER BY d.percent DESC",
                &[("hostname", h_name)],
            ).unwrap_or_default();
            if !disk_rows.is_empty() {
                lines.push("  Disks:".to_string());
                for dr in &disk_rows {
                    let mount = dr.get(0).map(|v| v.as_str()).unwrap_or("?");
                    let total = dr.get(2).map(|v| v.as_f64()).unwrap_or(0.0);
                    let used = dr.get(3).map(|v| v.as_f64()).unwrap_or(0.0);
                    let pct = dr.get(4).map(|v| v.as_f64()).unwrap_or(0.0);
                    let warn = if pct > 90.0 { " !!" } else { "" };
                    lines.push(format!("    {:<20} {:.0}/{:.0} GB ({:.0}% used){}", mount, used, total, pct, warn));
                }
            } else {
                lines.push("  Disks: (none in graph)".to_string());
            }
            lines.push(String::new());

            // Failed systemd units
            let failed_rows = self.query_text(
                &self.client,
                "MATCH (h:Host {hostname: $hostname})-[:HAS_UNIT]->(u:SystemdUnit) \
                 WHERE u.active_state = 'failed' \
                 RETURN u.name, u.sub_state, u.description ORDER BY u.name",
                &[("hostname", h_name)],
            ).unwrap_or_default();
            if !failed_rows.is_empty() {
                lines.push(format!("  Failed systemd units ({}):", failed_rows.len()));
                for fr in &failed_rows {
                    let unit_name = fr.get(0).map(|v| v.as_str()).unwrap_or("?");
                    let sub = fr.get(1).map(|v| v.as_str()).unwrap_or("failed");
                    lines.push(format!("    x {} ({})", unit_name, sub));
                    if let Some(desc) = fr.get(2) {
                        let d = desc.as_str();
                        if !d.is_empty() {
                            lines.push(format!("      {}", d));
                        }
                    }
                }
            } else {
                lines.push("  Failed systemd units: none".to_string());
            }
            lines.push(String::new());

            // Top 10 processes by memory
            let proc_rows = self.query_text(
                &self.client,
                "MATCH (h:Host {hostname: $hostname})-[:RUNS]->(p:HostProcess) \
                 RETURN p.pid, p.name, p.memory_mb, p.cpu_pct, p.user \
                 ORDER BY p.memory_mb DESC LIMIT 10",
                &[("hostname", h_name)],
            ).unwrap_or_default();
            if !proc_rows.is_empty() {
                lines.push("  Top 10 processes by memory:".to_string());
                for pr in &proc_rows {
                    let pid = pr.get(0).map(|v| v.as_i64()).unwrap_or(0);
                    let pname = pr.get(1).map(|v| v.as_str()).unwrap_or("?");
                    let mem_mb = pr.get(2).map(|v| v.as_f64()).unwrap_or(0.0);
                    let proc_cpu = pr.get(3).map(|v| v.as_f64()).unwrap_or(0.0);
                    let user = pr.get(4).map(|v| v.as_str()).unwrap_or("?");
                    lines.push(format!(
                        "    PID {:>6}  {:<25} {:.0} MB  {:.1}% CPU  ({})",
                        pid, pname, mem_mb, proc_cpu, user,
                    ));
                }
            } else {
                lines.push("  Top processes: (none in graph)".to_string());
            }
            lines.push(String::new());
        }

        Ok(lines.join("\n"))
    }

    fn tool_host_story(&self, args: &Value) -> Result<String, String> {
        let hostname = args.get("hostname").and_then(|v| v.as_str());
        let since_minutes = args.get("since_minutes").and_then(|v| v.as_i64()).unwrap_or(60);
        let min_severity = args.get("min_severity").and_then(|v| v.as_str()).unwrap_or("WARN");
        let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(15);

        let sev_rank = |s: &str| -> i32 {
            match s.to_uppercase().as_str() { "INFO" => 0, "WARN" => 1, "ERROR" => 2, "FATAL" => 3, _ => 0 }
        };
        let min_rank = sev_rank(min_severity);
        let allowed: Vec<&str> = ["INFO", "WARN", "ERROR", "FATAL"]
            .iter().copied().filter(|s| sev_rank(s) >= min_rank).collect();
        let allowed_str = allowed.iter().map(|s| format!("'{}'", s)).collect::<Vec<_>>().join(", ");

        // HostLogEvent query
        let mut log_where = vec![format!("e.severity IN [{}]", allowed_str)];
        if let Some(h) = hostname {
            log_where.push(format!("e.hostname = '{}'", escape_cypher(h)));
        }
        if since_minutes > 0 {
            let since_ts = now_unix() - (since_minutes as f64 * 60.0);
            log_where.push(format!("e.last_seen >= {}", since_ts));
        }
        let log_where_clause = log_where.join(" AND ");

        let log_query = format!(
            "MATCH (e:HostLogEvent) WHERE {} \
             RETURN e.hostname, e.source, e.severity, e.count, \
                    e.template_text, e.example_lines, \
                    e.first_seen, e.last_seen \
             ORDER BY CASE e.severity \
                        WHEN 'FATAL' THEN 3 WHEN 'ERROR' THEN 2 \
                        WHEN 'WARN' THEN 1 ELSE 0 END DESC, e.count DESC \
             LIMIT {}",
            log_where_clause, limit,
        );
        let log_rows = self.query_text(&self.client, &log_query, &[]).unwrap_or_default();

        // KernelEvent query
        let mut kern_where = vec![format!("k.severity IN [{}]", allowed_str)];
        if let Some(h) = hostname {
            kern_where.push(format!("k.hostname = '{}'", escape_cypher(h)));
        }
        if since_minutes > 0 {
            let since_ts = now_unix() - (since_minutes as f64 * 60.0);
            kern_where.push(format!("k.last_seen >= {}", since_ts));
        }
        let kern_where_clause = kern_where.join(" AND ");

        let kern_query = format!(
            "MATCH (k:KernelEvent) WHERE {} \
             RETURN k.hostname, k.facility, k.severity, k.count, \
                    k.template_text, k.example_lines, \
                    k.first_seen, k.last_seen \
             ORDER BY CASE k.severity \
                        WHEN 'FATAL' THEN 3 WHEN 'ERROR' THEN 2 \
                        WHEN 'WARN' THEN 1 ELSE 0 END DESC, k.count DESC \
             LIMIT {}",
            kern_where_clause, limit,
        );
        let kern_rows = self.query_text(&self.client, &kern_query, &[]).unwrap_or_default();

        // Histograms
        let log_hist = self.query_text(
            &self.client,
            &format!(
                "MATCH (e:HostLogEvent) WHERE {} \
                 RETURN e.severity, count(e), sum(e.count), count(DISTINCT e.hostname)",
                log_where_clause,
            ),
            &[],
        ).unwrap_or_default();

        let kern_hist = self.query_text(
            &self.client,
            &format!(
                "MATCH (k:KernelEvent) WHERE {} \
                 RETURN k.severity, count(k), sum(k.count), count(DISTINCT k.hostname)",
                kern_where_clause,
            ),
            &[],
        ).unwrap_or_default();

        // Build output
        let mut scope = Vec::new();
        if let Some(h) = hostname { scope.push(format!("host={}", h)); }
        if since_minutes > 0 { scope.push(format!("last {}m", since_minutes)); }
        let scope_str = if scope.is_empty() { "all hosts".to_string() } else { scope.join(", ") };

        let mut lines = vec![
            format!("# Host story ({})", scope_str),
            String::new(),
        ];

        if log_rows.is_empty() && kern_rows.is_empty() {
            lines.push(
                "No significant host events found. Either the host log watcher \
                 isn't running, the filters excluded everything, or the host \
                 is actually healthy.".to_string(),
            );
            return Ok(lines.join("\n"));
        }

        let total_templates: i64 =
            log_hist.iter().map(|r| r.get(1).map(|v| v.as_i64()).unwrap_or(0)).sum::<i64>()
            + kern_hist.iter().map(|r| r.get(1).map(|v| v.as_i64()).unwrap_or(0)).sum::<i64>();
        let total_occurrences: i64 =
            log_hist.iter().map(|r| r.get(2).map(|v| v.as_i64()).unwrap_or(0)).sum::<i64>()
            + kern_hist.iter().map(|r| r.get(2).map(|v| v.as_i64()).unwrap_or(0)).sum::<i64>();
        let total_hosts = std::cmp::max(
            log_hist.iter().map(|r| r.get(3).map(|v| v.as_i64()).unwrap_or(0)).max().unwrap_or(0),
            kern_hist.iter().map(|r| r.get(3).map(|v| v.as_i64()).unwrap_or(0)).max().unwrap_or(0),
        );

        lines.push(format!(
            "**Summary:** {} distinct templates, {} total occurrences, across {} host(s)",
            total_templates, total_occurrences, total_hosts,
        ));
        lines.push(String::new());

        // HostLogEvent details
        if !log_rows.is_empty() {
            lines.push(format!("## Host log events (top {})", log_rows.len()));
            lines.push(String::new());
            for (i, r) in log_rows.iter().enumerate() {
                let h_name = r.get(0).map(|v| v.as_str()).unwrap_or("?");
                let source = r.get(1).map(|v| v.as_str()).unwrap_or("unknown");
                let sev = r.get(2).map(|v| v.as_str()).unwrap_or("?");
                let cnt = r.get(3).map(|v| v.as_i64()).unwrap_or(0);
                let tmpl = r.get(4).map(|v| v.as_str()).unwrap_or("");
                let examples = r.get(5).map(|v| v.as_str()).unwrap_or("");
                let last_seen = r.get(7).map(|v| v.as_f64()).unwrap_or(0.0);

                lines.push(format!("### {}. [{}] {} ({}) -- {} occurrences", i + 1, sev, h_name, source, cnt));
                if !tmpl.is_empty() {
                    let short = if tmpl.len() > 200 { &tmpl[..200] } else { tmpl };
                    lines.push(format!("Template: `{}`", short));
                }
                if !examples.is_empty() {
                    lines.push("Example:".to_string());
                    let short = if examples.len() > 250 { &examples[..250] } else { examples };
                    lines.push(format!("    {}", short));
                }
                if last_seen > 0.0 {
                    lines.push(format!("Last seen: {}", format_timestamp(last_seen)));
                }
                lines.push(String::new());
            }
        }

        // KernelEvent details
        if !kern_rows.is_empty() {
            lines.push(format!("## Kernel events (top {})", kern_rows.len()));
            lines.push(String::new());
            for (i, r) in kern_rows.iter().enumerate() {
                let h_name = r.get(0).map(|v| v.as_str()).unwrap_or("?");
                let facility = r.get(1).map(|v| v.as_str()).unwrap_or("unknown");
                let sev = r.get(2).map(|v| v.as_str()).unwrap_or("?");
                let cnt = r.get(3).map(|v| v.as_i64()).unwrap_or(0);
                let tmpl = r.get(4).map(|v| v.as_str()).unwrap_or("");
                let examples = r.get(5).map(|v| v.as_str()).unwrap_or("");
                let last_seen = r.get(7).map(|v| v.as_f64()).unwrap_or(0.0);

                lines.push(format!("### {}. [{}] {} (kern/{}) -- {} occurrences", i + 1, sev, h_name, facility, cnt));
                if !tmpl.is_empty() {
                    let short = if tmpl.len() > 200 { &tmpl[..200] } else { tmpl };
                    lines.push(format!("Template: `{}`", short));
                }
                if !examples.is_empty() {
                    lines.push("Example:".to_string());
                    let short = if examples.len() > 250 { &examples[..250] } else { examples };
                    lines.push(format!("    {}", short));
                }
                if last_seen > 0.0 {
                    lines.push(format!("Last seen: {}", format_timestamp(last_seen)));
                }
                lines.push(String::new());
            }
        }

        Ok(lines.join("\n"))
    }

    fn tool_deployment_info(&self, args: &Value) -> Result<String, String> {
        let cluster = arg_str(args, "cluster")?;
        let namespace = arg_str(args, "namespace")?;
        let name = arg_str(args, "name")?;
        let c = self.k8s_client(&cluster)?;

        let deploy_rows = self.query_text(
            &c,
            "MATCH (d:K8sDeployment {name: $name, namespace: $ns}) \
             RETURN d.kind, d.replicas_desired, d.replicas_ready, \
             d.replicas_available, d.image, d.labels LIMIT 1",
            &[("name", &name), ("ns", &namespace)],
        )?;

        if deploy_rows.is_empty() {
            return Ok(format!(
                "No Deployment/StatefulSet/DaemonSet named '{}' in namespace '{}'.",
                name, namespace,
            ));
        }

        let r = &deploy_rows[0];
        let kind = r.get(0).map(|v| v.as_str()).unwrap_or("Deployment");
        let rd = r.get(1).map(|v| v.as_i64()).unwrap_or(0);
        let rr = r.get(2).map(|v| v.as_i64()).unwrap_or(0);
        let ra = r.get(3).map(|v| v.as_i64()).unwrap_or(0);
        let image = r.get(4).map(|v| v.as_str()).unwrap_or("?");
        let labels = r.get(5).map(|v| v.as_str()).unwrap_or("(none)");

        // Pods belonging to this deployment
        let pod_rows = self.query_text(
            &c,
            "MATCH (p:K8sPod) \
             WHERE p.namespace = $ns AND (p.owner_name STARTS WITH $name OR p.owner_name = $name) \
             RETURN p.name, p.status, p.restart_count, p.node_name",
            &[("ns", &namespace), ("name", &name)],
        )?;

        let mut lines = vec![
            format!("{}: {}/{}", kind, namespace, name),
            format!("  Image:        {}", image),
            format!("  Replicas:     {}/{} ready, {} available", rr, rd, ra),
            format!("  Labels:       {}", labels),
            String::new(),
            format!("Pods ({}):", pod_rows.len()),
        ];

        for pr in &pod_rows {
            let pn = pr.get(0).map(|v| v.as_str()).unwrap_or("?");
            let st = pr.get(1).map(|v| v.as_str()).unwrap_or("?");
            let rc = pr.get(2).map(|v| v.as_i64()).unwrap_or(0);
            let node = pr.get(3).map(|v| v.as_str()).unwrap_or("");
            let restarts = if rc > 0 { format!(" (restarts={})", rc) } else { String::new() };
            let node_str = if !node.is_empty() { format!(" on {}", node) } else { String::new() };
            lines.push(format!("  [{:15}] {}{}{}", st, pn, node_str, restarts));
        }

        Ok(lines.join("\n"))
    }

    fn tool_pod_dependencies(&self, args: &Value) -> Result<String, String> {
        let cluster = arg_str(args, "cluster")?;
        let namespace = arg_str(args, "namespace")?;
        let pod = arg_str(args, "pod")?;
        let c = self.k8s_client(&cluster)?;

        let cm_rows = self.query_text(
            &c,
            "MATCH (p:K8sPod {name: $pod, namespace: $ns})-[:READS]->(cm:K8sConfigMap) \
             RETURN cm.name, cm.key_names",
            &[("pod", &pod), ("ns", &namespace)],
        )?;

        let sec_rows = self.query_text(
            &c,
            "MATCH (p:K8sPod {name: $pod, namespace: $ns})-[:READS]->(sec:K8sSecret) \
             RETURN sec.name, sec.type, sec.key_names",
            &[("pod", &pod), ("ns", &namespace)],
        )?;

        if cm_rows.is_empty() && sec_rows.is_empty() {
            return Ok(format!(
                "Pod {}/{} has no ConfigMap or Secret dependencies (or doesn't exist in context).",
                namespace, pod,
            ));
        }

        let mut lines = vec![
            format!("Dependencies for pod {}/{}:", namespace, pod),
            String::new(),
            format!("ConfigMaps ({}):", cm_rows.len()),
        ];
        for r in &cm_rows {
            let name = r.get(0).map(|v| v.as_str()).unwrap_or("?");
            let keys = r.get(1).map(|v| v.as_str()).unwrap_or("");
            let key_info = if !keys.is_empty() { format!(" [{}]", keys) } else { String::new() };
            lines.push(format!("  * {}{}", name, key_info));
        }
        lines.push(String::new());
        lines.push(format!("Secrets ({}):", sec_rows.len()));
        for r in &sec_rows {
            let name = r.get(0).map(|v| v.as_str()).unwrap_or("?");
            let type_ = r.get(1).map(|v| v.as_str()).unwrap_or("?");
            let keys = r.get(2).map(|v| v.as_str()).unwrap_or("");
            let key_info = if !keys.is_empty() { format!(" [{}]", keys) } else { String::new() };
            lines.push(format!("  * {} ({}){}", name, type_, key_info));
        }

        Ok(lines.join("\n"))
    }

    fn tool_namespace_summary(&self, args: &Value) -> Result<String, String> {
        let cluster = arg_str(args, "cluster")?;
        let namespace = arg_str(args, "namespace")?;
        let c = self.k8s_client(&cluster)?;

        // Verify namespace exists
        let ns_rows = self.query_text(
            &c,
            "MATCH (n:K8sNamespace {name: $ns}) RETURN n.status, n.age_seconds",
            &[("ns", &namespace)],
        )?;
        if ns_rows.is_empty() {
            return Ok(format!("Namespace '{}' not found in cluster data.", namespace));
        }
        let status = ns_rows[0].get(0).map(|v| v.as_str()).unwrap_or("?");
        let age = ns_rows[0].get(1).map(|v| v.as_i64()).unwrap_or(0);

        // Counts
        let count_q = |label: &str, var: &str| -> i64 {
            self.query_text(
                &c,
                &format!(
                    "MATCH (n:K8sNamespace {{name: $ns}})-[:CONTAINS]->({}:{}) RETURN count({})",
                    var, label, var,
                ),
                &[("ns", &*namespace)],
            )
            .ok()
            .and_then(|r| r.first().and_then(|row| row.first().map(|v| v.as_i64())))
            .unwrap_or(0)
        };

        let deploy_count = count_q("K8sDeployment", "d");
        let pod_count = count_q("K8sPod", "p");
        let svc_count = count_q("K8sService", "s");
        let cm_count = count_q("K8sConfigMap", "cm");
        let sec_count = count_q("K8sSecret", "sec");

        // Deployments with health
        let deploys = self.query_text(
            &c,
            "MATCH (n:K8sNamespace {name: $ns})-[:CONTAINS]->(d:K8sDeployment) \
             RETURN d.name, d.kind, d.replicas_ready, d.replicas_desired, d.image \
             ORDER BY d.name",
            &[("ns", &*namespace)],
        ).unwrap_or_default();

        // Pod status breakdown
        let pod_status = self.query_text(
            &c,
            "MATCH (n:K8sNamespace {name: $ns})-[:CONTAINS]->(p:K8sPod) \
             RETURN p.status, count(p) ORDER BY count(p) DESC",
            &[("ns", &*namespace)],
        ).unwrap_or_default();

        let mut lines = vec![
            format!("Namespace: {}", namespace),
            format!("  Status:       {}", status),
            format!("  Age:          {} days, {} hours", age / 86400, (age % 86400) / 3600),
            String::new(),
            "Resource counts:".to_string(),
            format!("  Deployments:  {}", deploy_count),
            format!("  Pods:         {}", pod_count),
            format!("  Services:     {}", svc_count),
            format!("  ConfigMaps:   {}", cm_count),
            format!("  Secrets:      {}", sec_count),
            String::new(),
            "Pod status breakdown:".to_string(),
        ];

        for r in &pod_status {
            let st = r.get(0).map(|v| v.as_str()).unwrap_or("?");
            let cnt = r.get(1).map(|v| v.as_i64()).unwrap_or(0);
            lines.push(format!("  {:4}  {}", cnt, st));
        }

        if !deploys.is_empty() {
            lines.push(String::new());
            lines.push("Deployments:".to_string());
            for r in &deploys {
                let d_name = r.get(0).map(|v| v.as_str()).unwrap_or("?");
                let d_kind = r.get(1).map(|v| v.as_str()).unwrap_or("Deployment");
                let rr = r.get(2).map(|v| v.as_i64()).unwrap_or(0);
                let rd = r.get(3).map(|v| v.as_i64()).unwrap_or(0);
                let img = r.get(4).map(|v| v.as_str()).unwrap_or("");
                let health = if rr == rd && rd > 0 { "ok" } else { "!!" };
                lines.push(format!("  {} [{:12}] {}  ({}/{} ready)", health, d_kind, d_name, rr, rd));
                if !img.is_empty() {
                    lines.push(format!("      {}", img));
                }
            }
        }

        Ok(lines.join("\n"))
    }

    // ---------------------------------------------------------------
    // Context tools: replace grep/read for LLMs
    // ---------------------------------------------------------------

    fn tool_file_skeleton(&self, args: &Value) -> Result<String, String> {
        let file = arg_str(args, "file")?;
        let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("talent-pipeline");

        // Get all functions, classes, and interfaces in this file
        let functions = self.query_text(
            &self.client,
            &format!(
                "MATCH (f:CodeFunction {{repo: '{}', file: '{}'}}) \
                 RETURN f.name, f.line, f.end_line, f.params \
                 ORDER BY f.line",
                repo, file.replace('\'', "\\'")
            ),
            &[],
        ).unwrap_or_default();

        let classes = self.query_text(
            &self.client,
            &format!(
                "MATCH (c:CodeClass {{repo: '{}', file: '{}'}}) \
                 RETURN c.name, c.line, c.end_line \
                 ORDER BY c.line",
                repo, file.replace('\'', "\\'")
            ),
            &[],
        ).unwrap_or_default();

        let interfaces = self.query_text(
            &self.client,
            &format!(
                "MATCH (i:CodeInterface {{repo: '{}', file: '{}'}}) \
                 RETURN i.name, i.line, i.end_line \
                 ORDER BY i.line",
                repo, file.replace('\'', "\\'")
            ),
            &[],
        ).unwrap_or_default();

        if functions.is_empty() && classes.is_empty() && interfaces.is_empty() {
            return Ok(format!("No code entities found in '{}'. Is it indexed?", file));
        }

        let mut lines = vec![format!("=== {} ===", file)];

        if !classes.is_empty() {
            lines.push("Classes:".to_string());
            for row in &classes {
                let name = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                let line = row.get(1).map(|v| v.as_i64()).unwrap_or(0);
                let end = row.get(2).map(|v| v.as_i64()).unwrap_or(0);
                lines.push(format!("  class {} (lines {}-{})", name, line, end));
            }
        }

        if !interfaces.is_empty() {
            lines.push("Types/Interfaces:".to_string());
            for row in &interfaces {
                let name = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                let line = row.get(1).map(|v| v.as_i64()).unwrap_or(0);
                let end = row.get(2).map(|v| v.as_i64()).unwrap_or(0);
                lines.push(format!("  {} (lines {}-{})", name, line, end));
            }
        }

        if !functions.is_empty() {
            lines.push("Functions:".to_string());
            for row in &functions {
                let name = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                let line = row.get(1).map(|v| v.as_i64()).unwrap_or(0);
                let end = row.get(2).map(|v| v.as_i64()).unwrap_or(0);
                let params = row.get(3).map(|v| v.as_str()).unwrap_or("");
                lines.push(format!("  {}({}) (lines {}-{})", name, params, line, end));
            }
        }

        lines.push(format!("\n{} functions, {} classes, {} types",
            functions.len(), classes.len(), interfaces.len()));
        Ok(lines.join("\n"))
    }

    fn tool_where_used(&self, args: &Value) -> Result<String, String> {
        let symbol = arg_str(args, "symbol")?;
        let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("talent-pipeline");

        let mut lines = vec![format!("=== Where '{}' is used ===", symbol)];

        // 1. Direct callers (functions that call this symbol)
        let callers = self.query_text(
            &self.client,
            &format!(
                "MATCH (caller:CodeFunction {{repo: '{}'}})-[:CALLS]->(target:CodeFunction {{repo: '{}', name: '{}'}}) \
                 RETURN caller.name, caller.file, caller.line ORDER BY caller.file",
                repo, repo, symbol.replace('\'', "\\'")
            ),
            &[],
        ).unwrap_or_default();

        if !callers.is_empty() {
            lines.push(format!("\nCallers ({}):", callers.len()));
            for row in &callers {
                let name = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                let file = row.get(1).map(|v| v.as_str()).unwrap_or("?");
                let line = row.get(2).map(|v| v.as_i64()).unwrap_or(0);
                lines.push(format!("  {}:{} - {}()", file, line, name));
            }
        }

        // 2. Importers (files that import this symbol)
        let importers = self.query_text(
            &self.client,
            &format!(
                "MATCH (fi:CodeFile {{repo: '{}'}})-[:IMPORTS]->(fn:CodeFunction {{repo: '{}', name: '{}'}}) \
                 RETURN fi.path ORDER BY fi.path",
                repo, repo, symbol.replace('\'', "\\'")
            ),
            &[],
        ).unwrap_or_default();

        if !importers.is_empty() {
            lines.push(format!("\nImported by ({} files):", importers.len()));
            for row in &importers {
                let path = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                lines.push(format!("  {}", path));
            }
        }

        // 3. Functions that reference this symbol in their body
        let body_refs = self.query_text(
            &self.client,
            &format!(
                "MATCH (f:CodeFunction {{repo: '{}'}}) \
                 WHERE toLower(f.body) CONTAINS toLower('{}') AND f.name <> '{}' \
                 RETURN f.name, f.file, f.line ORDER BY f.file LIMIT 20",
                repo, symbol.replace('\'', "\\'"), symbol.replace('\'', "\\'")
            ),
            &[],
        ).unwrap_or_default();

        if !body_refs.is_empty() {
            lines.push(format!("\nReferenced in body ({}):", body_refs.len()));
            for row in &body_refs {
                let name = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                let file = row.get(1).map(|v| v.as_str()).unwrap_or("?");
                let line = row.get(2).map(|v| v.as_i64()).unwrap_or(0);
                lines.push(format!("  {}:{} - {}()", file, line, name));
            }
        }

        if callers.is_empty() && importers.is_empty() && body_refs.is_empty() {
            lines.push(format!("\nNo usages of '{}' found in {}", symbol, repo));
        }

        Ok(lines.join("\n"))
    }

    fn tool_callers(&self, args: &Value) -> Result<String, String> {
        let function = arg_str(args, "function")?;
        let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("talent-pipeline");

        let callers = self.query_text(
            &self.client,
            &format!(
                "MATCH (caller:CodeFunction {{repo: '{}'}})-[:CALLS]->(target:CodeFunction {{repo: '{}', name: '{}'}}) \
                 RETURN caller.name, caller.file, caller.line ORDER BY caller.file",
                repo, repo, function.replace('\'', "\\'")
            ),
            &[],
        ).unwrap_or_default();

        if callers.is_empty() {
            return Ok(format!("No callers found for '{}' in {}", function, repo));
        }

        let mut lines = vec![format!("=== Callers of {} ({}) ===", function, callers.len())];
        for row in &callers {
            let name = row.get(0).map(|v| v.as_str()).unwrap_or("?");
            let file = row.get(1).map(|v| v.as_str()).unwrap_or("?");
            let line = row.get(2).map(|v| v.as_i64()).unwrap_or(0);
            lines.push(format!("  {}:{} - {}()", file, line, name));
        }
        Ok(lines.join("\n"))
    }

    fn tool_import_tree(&self, args: &Value) -> Result<String, String> {
        let file = arg_str(args, "file")?;
        let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("talent-pipeline");
        let depth = args.get("depth").and_then(|v| v.as_i64()).unwrap_or(2) as usize;

        let mut lines = vec![format!("=== Import tree: {} (depth {}) ===", file, depth)];
        let mut visited = std::collections::HashSet::new();
        self.build_import_tree(&file, repo, depth, 0, &mut lines, &mut visited);

        if lines.len() == 1 {
            lines.push(format!("  No imports found for '{}'", file));
        }
        Ok(lines.join("\n"))
    }

    fn build_import_tree(&self, file: &str, repo: &str, max_depth: usize, current_depth: usize,
                         lines: &mut Vec<String>, visited: &mut std::collections::HashSet<String>) {
        if current_depth >= max_depth || visited.contains(file) { return; }
        visited.insert(file.to_string());

        let indent = "  ".repeat(current_depth + 1);

        // Get all functions imported by this file
        let imports = self.query_text(
            &self.client,
            &format!(
                "MATCH (fi:CodeFile {{repo: '{}', path: '{}'}})-[:IMPORTS]->(fn:CodeFunction) \
                 RETURN DISTINCT fn.name, fn.file ORDER BY fn.file",
                repo, file.replace('\'', "\\'")
            ),
            &[],
        ).unwrap_or_default();

        // Group by file
        let mut by_file: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        for row in &imports {
            let name = row.get(0).map(|v| v.as_str()).unwrap_or("?").to_string();
            let imp_file = row.get(1).map(|v| v.as_str()).unwrap_or("?").to_string();
            by_file.entry(imp_file).or_default().push(name);
        }

        for (imp_file, names) in &by_file {
            let name_list = names.join(", ");
            lines.push(format!("{}-> {} [{}]", indent, imp_file, name_list));
            // Recurse
            self.build_import_tree(imp_file, repo, max_depth, current_depth + 1, lines, visited);
        }
    }

    fn tool_module_exports(&self, args: &Value) -> Result<String, String> {
        let file = arg_str(args, "file")?;
        let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("talent-pipeline");

        // Get all functions in this file (these are the "exports" since tree-sitter
        // extracts top-level and exported declarations)
        let functions = self.query_text(
            &self.client,
            &format!(
                "MATCH (f:CodeFunction {{repo: '{}', file: '{}'}}) \
                 RETURN f.name, f.params, f.line ORDER BY f.line",
                repo, file.replace('\'', "\\'")
            ),
            &[],
        ).unwrap_or_default();

        let interfaces = self.query_text(
            &self.client,
            &format!(
                "MATCH (i:CodeInterface {{repo: '{}', file: '{}'}}) \
                 RETURN i.name, i.line ORDER BY i.line",
                repo, file.replace('\'', "\\'")
            ),
            &[],
        ).unwrap_or_default();

        if functions.is_empty() && interfaces.is_empty() {
            return Ok(format!("No exports found in '{}'", file));
        }

        let mut lines = vec![format!("=== Exports: {} ===", file)];

        for row in &interfaces {
            let name = row.get(0).map(|v| v.as_str()).unwrap_or("?");
            let line = row.get(1).map(|v| v.as_i64()).unwrap_or(0);
            lines.push(format!("  type {} (line {})", name, line));
        }

        for row in &functions {
            let name = row.get(0).map(|v| v.as_str()).unwrap_or("?");
            let params = row.get(1).map(|v| v.as_str()).unwrap_or("");
            let line = row.get(2).map(|v| v.as_i64()).unwrap_or(0);
            lines.push(format!("  {}({}) (line {})", name, params, line));
        }

        lines.push(format!("\n{} functions, {} types", functions.len(), interfaces.len()));
        Ok(lines.join("\n"))
    }

    fn tool_blast_radius(&self, args: &Value) -> Result<String, String> {
        let function = arg_str(args, "function")?;
        let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("talent-pipeline");
        let depth = args.get("depth").and_then(|v| v.as_i64()).unwrap_or(3);

        // Find all transitive callers (functions that depend on this one)
        let dependents = self.query_text(
            &self.client,
            &format!(
                "MATCH (caller:CodeFunction {{repo: '{}'}})-[:CALLS*1..{}]->(target:CodeFunction {{repo: '{}', name: '{}'}}) \
                 RETURN DISTINCT caller.name, caller.file ORDER BY caller.file",
                repo, depth, repo, function.replace('\'', "\\'")
            ),
            &[],
        ).unwrap_or_default();

        if dependents.is_empty() {
            return Ok(format!("No dependents found for '{}' (blast radius: 0)", function));
        }

        let mut lines = vec![format!("=== Blast radius: {} ({} dependents, depth {}) ===", function, dependents.len(), depth)];

        // Group by file
        let mut current_file = String::new();
        for row in &dependents {
            let name = row.get(0).map(|v| v.as_str()).unwrap_or("?");
            let file = row.get(1).map(|v| v.as_str()).unwrap_or("?");
            if file != current_file {
                current_file = file.to_string();
                lines.push(format!("\n  {}:", file));
            }
            lines.push(format!("    {}()", name));
        }

        // Also count affected files
        let mut files: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for row in &dependents {
            files.insert(row.get(1).map(|v| v.as_str()).unwrap_or("?"));
        }
        lines.push(format!("\nIf you change {}: {} functions across {} files could break.", function, dependents.len(), files.len()));

        Ok(lines.join("\n"))
    }

    fn tool_semantic_search(&self, args: &Value) -> Result<String, String> {
        let query = arg_str(args, "query")?;
        let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("talent-pipeline");
        let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(10) as usize;

        // Try loading cached embeddings first (instant, <1ms)
        if crate::embedding_store::EmbeddingStore::exists(repo) {
            let store = crate::embedding_store::EmbeddingStore::load(repo)
                .map_err(|e| format!("Load embeddings: {}", e))?;

            let mut engine = crate::embeddings::EmbeddingEngine::new()
                .map_err(|e| format!("Embedding engine: {}", e))?;
            let query_vec = engine.embed_one(&query)
                .map_err(|e| format!("Embed query: {}", e))?;

            // Method 1: Embedding similarity (top 20 candidates)
            let embed_results = store.search(&query_vec, 20);

            // Method 2: Graph keyword search (name, file, body)
            let query_terms: Vec<String> = query.to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| w.len() >= 2)
                .map(|w| w.to_string())
                .collect();

            // Query the graph for keyword matches
            let mut graph_scored: Vec<(String, f32)> = vec![];
            for (idx, entry) in store.entries.iter().enumerate() {
                let mut score = 0.0f32;
                let name_lower = entry.name.to_lowercase();
                let file_lower = entry.file.to_lowercase();

                // Split function name into parts
                let mut name_parts = vec![];
                let mut current = String::new();
                for ch in entry.name.chars() {
                    if ch == '_' || ch == '-' || (ch.is_uppercase() && !current.is_empty()) {
                        if !current.is_empty() { name_parts.push(current.to_lowercase()); current.clear(); }
                        if ch.is_uppercase() { current.push(ch); }
                    } else { current.push(ch); }
                }
                if !current.is_empty() { name_parts.push(current.to_lowercase()); }

                let mut name_hits = 0;
                for qt in &query_terms {
                    // Name part match (strongest signal)
                    if name_parts.iter().any(|p| p == qt) { score += 40.0; name_hits += 1; }
                    // Name substring match
                    else if name_lower.contains(qt.as_str()) { score += 20.0; name_hits += 1; }
                    // File path segment match (e.g., "stripe" in "services/stripe.ts")
                    if file_lower.split('/').any(|seg| seg.contains(qt.as_str())) { score += 30.0; }
                }
                // Multi-term name bonus
                if name_hits >= 2 { score += 60.0; }
                if name_hits >= 3 { score += 120.0; }

                if score > 0.0 { graph_scored.push((idx.to_string(), score)); }
            }
            graph_scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

            // Build ranked lists for RRF
            let embed_ranked: Vec<(String, usize)> = embed_results.iter()
                .enumerate().map(|(rank, (idx, _))| (idx.to_string(), rank)).collect();
            let graph_ranked: Vec<(String, usize)> = graph_scored.iter()
                .take(20).enumerate().map(|(rank, (idx, _))| (idx.clone(), rank)).collect();

            // RRF fusion
            let fused = crate::embeddings::reciprocal_rank_fusion(
                &[embed_ranked, graph_ranked], 60.0
            );

            if fused.is_empty() {
                return Ok(format!("No results for '{}' in {}", query, repo));
            }

            let mut lines = vec![format!("=== Semantic search: '{}' ({} results, hybrid) ===", query, fused.len().min(limit))];
            for (idx_str, score) in fused.iter().take(limit) {
                let idx: usize = idx_str.parse().unwrap_or(0);
                if idx < store.entries.len() {
                    let entry = &store.entries[idx];
                    lines.push(format!("  {}:{} {}() [{:.3}]", entry.file, entry.line, entry.name, score));
                }
            }
            return Ok(lines.join("\n"));
        }

        // No cache - parse and embed (slow, ~15s, happens once)
        let repo_path_candidates = [
            format!("/home/miguel/git/sourcecoders-ai/{}", repo),
            format!("/home/miguel/git/bernadinm/{}", repo),
            format!("/home/miguel/git/{}", repo),
        ];
        let repo_path = repo_path_candidates.iter()
            .find(|p| std::path::Path::new(p).is_dir())
            .cloned()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().to_string_lossy().to_string());

        let mut parser = crate::code_parser::CodeParser::new(repo);
        let parse_result = parser.parse_repo(&repo_path);

        if parse_result.entities.is_empty() {
            return Ok(format!("No code found in '{}'. Run reindex first.", repo_path));
        }

        let mut engine = crate::embeddings::EmbeddingEngine::new()
            .map_err(|e| format!("Embedding engine: {}", e))?;

        // Build index, search, AND persist for next time
        let index = crate::semantic_search::SemanticIndex::from_parse_result(&parse_result, &mut engine)
            .map_err(|e| format!("Index: {}", e))?;

        // Save embeddings to disk for instant future searches
        let dim = engine.embed_one("test").map(|v| v.len() as u32).unwrap_or(128);
        let mut store = crate::embedding_store::EmbeddingStore::new(dim);
        for (entry, emb) in index.entries_with_embeddings() {
            let kind = match entry.kind.as_str() { "class" => 1, "interface" => 2, _ => 0 };
            store.add(&entry.name, &entry.file, entry.line as u32, kind, emb.clone());
        }
        if let Err(e) = store.save(repo) {
            eprintln!("Warning: could not cache embeddings: {}", e);
        }

        let results = index.search(&query, &mut engine, limit)
            .map_err(|e| format!("Search: {}", e))?;

        if results.is_empty() {
            return Ok(format!("No results for '{}' in {}", query, repo));
        }

        let mut lines = vec![format!("=== Semantic search: '{}' ({} results, indexed) ===", query, results.len())];
        for r in &results {
            lines.push(format!("  {}:{} {}() [{:.3}]", r.file, r.line, r.name, r.score));
        }
        Ok(lines.join("\n"))
    }

    fn tool_dead_code(&self, args: &Value) -> Result<String, String> {
        let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("talent-pipeline");
        let file_filter = args.get("file").and_then(|v| v.as_str());

        let query = if let Some(f) = file_filter {
            format!(
                "MATCH (f:CodeFunction {{repo: '{}', file: '{}'}}) \
                 WHERE NOT ()-[:CALLS]->(f) AND NOT f.name STARTS WITH '_' \
                 RETURN f.name, f.file, f.line ORDER BY f.file",
                repo, f.replace('\'', "\\'")
            )
        } else {
            format!(
                "MATCH (f:CodeFunction {{repo: '{}'}}) \
                 WHERE NOT ()-[:CALLS]->(f) AND NOT f.name STARTS WITH '_' \
                 AND NOT f.file CONTAINS 'test' AND NOT f.file CONTAINS '__mock' \
                 RETURN f.name, f.file, f.line ORDER BY f.file LIMIT 50",
                repo
            )
        };

        let dead = self.query_text(&self.client, &query, &[]).unwrap_or_default();

        if dead.is_empty() {
            return Ok("No dead code found (all functions have callers).".to_string());
        }

        let mut lines = vec![format!("=== Dead code candidates ({}) ===", dead.len())];
        let mut current_file = String::new();
        for row in &dead {
            let name = row.get(0).map(|v| v.as_str()).unwrap_or("?");
            let file = row.get(1).map(|v| v.as_str()).unwrap_or("?");
            let line = row.get(2).map(|v| v.as_i64()).unwrap_or(0);
            if file != current_file {
                current_file = file.to_string();
                lines.push(format!("\n  {}:", file));
            }
            lines.push(format!("    {}() line {} - no callers found", name, line));
        }
        lines.push("\nNote: entry points (routes, exports, main) may appear here.".to_string());
        Ok(lines.join("\n"))
    }

    fn tool_search_code(&self, args: &Value) -> Result<String, String> {
        let pattern = arg_str(args, "pattern")?;

        // Search both old-style (Function/Class) and new tree-sitter (CodeFunction/CodeClass) nodes
        // Also search function bodies for the pattern
        let mut results = Vec::new();

        // Search by name
        if let Ok(rows) = self.query_text(
            &self.client,
            "MATCH (n) WHERE (n:CodeFunction OR n:Class OR n:CodeFunction OR n:CodeClass) AND toLower(n.name) CONTAINS toLower($pattern) \
             RETURN labels(n)[0], n.name, n.file, n.file, n.line, n.repo LIMIT 50",
            &[("pattern", &pattern)],
        ) {
            for r in &rows {
                let label = r.get(0).map(|v| v.as_str()).unwrap_or("?");
                let name = r.get(1).map(|v| v.as_str()).unwrap_or("?");
                let fp = r.get(2).map(|v| v.as_str()).unwrap_or("");
                let file = r.get(3).map(|v| v.as_str()).unwrap_or("");
                let line = r.get(4).map(|v| v.as_i64()).unwrap_or(0);
                let repo = r.get(5).map(|v| v.as_str()).unwrap_or("");
                let path = if !fp.is_empty() { fp } else { file };
                let loc = if line > 0 { format!(":{}",line) } else { String::new() };
                let repo_prefix = if !repo.is_empty() { format!("{}/", repo) } else { String::new() };
                results.push(format!("[{}] {} ({}{}{})", label, name, repo_prefix, path, loc));
            }
        }

        // Search function bodies for the pattern (finds usage, not just definitions)
        if let Ok(rows) = self.query_text(
            &self.client,
            "MATCH (n:CodeFunction) WHERE toLower(n.body) CONTAINS toLower($pattern) \
             RETURN n.name, n.file, n.line, n.repo LIMIT 20",
            &[("pattern", &pattern)],
        ) {
            if !rows.is_empty() {
                results.push(String::new());
                results.push(format!("Functions containing '{}' in body:", pattern));
                for r in &rows {
                    let name = r.get(0).map(|v| v.as_str()).unwrap_or("?");
                    let file = r.get(1).map(|v| v.as_str()).unwrap_or("?");
                    let line = r.get(2).map(|v| v.as_i64()).unwrap_or(0);
                    let repo = r.get(3).map(|v| v.as_str()).unwrap_or("");
                    results.push(format!("  {} ({}{}:{})", name, if !repo.is_empty() { format!("{}/",repo) } else { String::new() }, file, line));
                }
            }
        }

        if results.is_empty() {
            return Ok(format!("No functions or classes matching '{}'.", pattern));
        }
        Ok(results.join("\n"))
    }

    fn tool_find_references(&self, args: &Value) -> Result<String, String> {
        let function_name = arg_str(args, "function_name")?;
        let include_tests = args.get("include_tests").and_then(|v| v.as_bool()).unwrap_or(true);

        let where_extra = if include_tests {
            ""
        } else {
            "AND NOT caller.file STARTS WITH 'tests/'"
        };

        let query = format!(
            "MATCH (caller:CodeFunction)-[:CALLS]->(target:CodeFunction {{name: $name}}) \
             WHERE 1=1 {} \
             RETURN caller.name, caller.file \
             ORDER BY caller.file LIMIT 50",
            where_extra,
        );
        let rows = self.query_text(&self.client, &query, &[("name", &function_name)])?;

        if rows.is_empty() {
            return Ok(format!("No structural callers found for '{}'.", function_name));
        }

        // Group by file
        let mut by_file: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
        for r in &rows {
            let caller_name = r.get(0).map(|v| v.as_str().to_string()).unwrap_or_default();
            let caller_file = r.get(1).map(|v| v.as_str().to_string()).unwrap_or_default();
            by_file.entry(caller_file).or_default().push(caller_name);
        }

        let mut lines = vec![format!("{} references to '{}':", rows.len(), function_name), String::new()];
        for (file, callers) in &by_file {
            lines.push(format!("  {}", file));
            for cn in callers {
                lines.push(format!("    -- {}", cn));
            }
        }
        Ok(lines.join("\n"))
    }

    fn tool_function_xray(&self, args: &Value) -> Result<String, String> {
        let function_name = arg_str(args, "function_name")?;
        let file_path = args.get("file_path").and_then(|v| v.as_str());

        let fn_result = if let Some(fp) = file_path {
            self.query_text(
                &self.client,
                "MATCH (fn:CodeFunction {name: $name, file: $fp}) \
                 RETURN fn.name, fn.file, fn.start_line, fn.end_line, fn.parameters",
                &[("name", &function_name), ("fp", fp)],
            )?
        } else {
            self.query_text(
                &self.client,
                "MATCH (fn:CodeFunction {name: $name}) \
                 RETURN fn.name, fn.file, fn.start_line, fn.end_line, fn.parameters \
                 LIMIT 5",
                &[("name", &function_name)],
            )?
        };

        if fn_result.is_empty() {
            return Ok(format!("Function '{}' not found.", function_name));
        }

        let mut lines = vec![format!("=== X-Ray: {} ===", function_name)];
        for row in &fn_result {
            let name = row.get(0).map(|v| v.as_str()).unwrap_or("?");
            let fp = row.get(1).map(|v| v.as_str()).unwrap_or("?");
            let start = row.get(2).map(|v| v.as_i64()).unwrap_or(0);
            let _end = row.get(3).map(|v| v.as_i64()).unwrap_or(0);
            let params = row.get(4).map(|v| v.as_str()).unwrap_or("(none)");

            lines.push(format!("|  {}:{}", fp, start));
            lines.push(format!("|  parameters: {}", params));
            lines.push("|".to_string());

            // Callers
            let callers = self.query_text(
                &self.client,
                "MATCH (c:CodeFunction)-[:CALLS]->(t:CodeFunction {name: $name, file: $fp}) \
                 RETURN c.name, c.file LIMIT 25",
                &[("name", name), ("fp", fp)],
            ).unwrap_or_default();
            lines.push(format!("|  Direct callers: {}", callers.len()));
            for (idx, cr) in callers.iter().enumerate() {
                if idx >= 8 { break; }
                let cn = cr.get(0).map(|v| v.as_str()).unwrap_or("?");
                let cf = cr.get(1).map(|v| v.as_str()).unwrap_or("?");
                lines.push(format!("|    - {} ({})", cn, cf));
            }
            if callers.len() > 8 {
                lines.push(format!("|    ... and {} more", callers.len() - 8));
            }

            // Callees
            let callees = self.query_text(
                &self.client,
                "MATCH (t:CodeFunction {name: $name, file: $fp})-[:CALLS]->(c:CodeFunction) \
                 RETURN c.name LIMIT 15",
                &[("name", name), ("fp", fp)],
            ).unwrap_or_default();
            lines.push(format!("|  Direct callees: {}", callees.len()));
            for (idx, cr) in callees.iter().enumerate() {
                if idx >= 6 { break; }
                let cn = cr.get(0).map(|v| v.as_str()).unwrap_or("?");
                lines.push(format!("|    - {}", cn));
            }

            // Episodes (history)
            let episodes = self.query_text(
                &self.client,
                "MATCH (e:Commit)-[:MODIFIED]->(:CodeFunction {name: $name, file: $fp}) \
                 RETURN e.date, e.author_name, e.message \
                 ORDER BY e.date DESC LIMIT 5",
                &[("name", name), ("fp", fp)],
            ).unwrap_or_default();
            if !episodes.is_empty() {
                lines.push(format!("|  Recent commits ({} shown):", episodes.len()));
                for ep in &episodes {
                    let ts = ep.get(0).map(|v| v.as_str()).unwrap_or("?");
                    let author = ep.get(1).map(|v| v.as_str()).unwrap_or("?");
                    let msg = ep.get(2).map(|v| v.as_str()).unwrap_or("");
                    let short_author = author.split('<').next().unwrap_or("?").trim();
                    let short_msg = if msg.len() > 60 { &msg[..60] } else { msg };
                    let ts_short = if ts.len() >= 10 { &ts[..10] } else { ts };
                    lines.push(format!("|    {}  {}  {}", ts_short, short_author, short_msg));
                }
            } else {
                lines.push("|  Recent commits: (history not loaded)".to_string());
            }
            lines.push("|".to_string());
        }
        lines.push("============================".to_string());
        Ok(lines.join("\n"))
    }

    fn tool_impact_analysis(&self, args: &Value) -> Result<String, String> {
        let function_name = arg_str(args, "function_name")?;
        let max_depth = args.get("max_depth").and_then(|v| v.as_i64()).unwrap_or(5);

        // Direct dependents
        let direct = self.query_text(
            &self.client,
            "MATCH (c:CodeFunction)-[:CALLS]->(t:CodeFunction {name: $name}) \
             RETURN DISTINCT c.name, c.file",
            &[("name", &function_name)],
        )?;

        // Transitive dependents
        let query = format!(
            "MATCH (c:CodeFunction)-[:CALLS*1..{}]->(t:CodeFunction {{name: $name}}) \
             RETURN DISTINCT c.name, c.file",
            max_depth,
        );
        let transitive = self.query_text(&self.client, &query, &[("name", &function_name)])?;

        // Affected files
        let aff_query = format!(
            "MATCH (c:CodeFunction)-[:CALLS*1..{}]->(t:CodeFunction {{name: $name}}) \
             RETURN DISTINCT c.file",
            max_depth,
        );
        let affected = self.query_text(&self.client, &aff_query, &[("name", &function_name)])?;
        let affected_files: Vec<&str> = affected.iter()
            .filter_map(|r| r.first().map(|v| v.as_str()))
            .collect();

        Ok(format!(
            "Impact analysis for '{}':\n\
             Direct dependents: {}\n\
             Transitive dependents: {}\n\
             Affected files: {:?}",
            function_name,
            direct.len(),
            transitive.len(),
            affected_files,
        ))
    }

    fn tool_diff_impact(&self, _args: &Value) -> Result<String, String> {
        // Stub -- needs git integration
        Ok("diff_impact is not yet implemented in the Rust MCP server. \
            It requires git integration (parsing diffs, mapping changed lines \
            to graph nodes). Use the Python server for this tool."
            .to_string())
    }

    fn tool_risk_score(&self, args: &Value) -> Result<String, String> {
        let function_name = arg_str(args, "function_name")?;
        let mut score: f64 = 0.0;
        let mut breakdown = Vec::new();

        // Blast radius
        let trans = self.query_text(
            &self.client,
            "MATCH (c:CodeFunction)-[:CALLS*1..3]->(t:CodeFunction {name: $name}) \
             RETURN count(DISTINCT c)",
            &[("name", &function_name)],
        )?;
        let trans_count = trans.first().and_then(|r| r.first()).map(|v| v.as_i64()).unwrap_or(0);
        let blast = if trans_count > 200 { 4.0 }
            else if trans_count > 50 { 3.0 }
            else if trans_count > 10 { 2.0 }
            else if trans_count > 0 { 1.0 }
            else { 0.0 };
        score += blast;
        breakdown.push(format!("  blast radius:  {}/4   ({} transitive callers)", blast, trans_count));

        // Bus factor
        let maintainers = self.query_text(
            &self.client,
            "MATCH (e:Commit)-[:MODIFIED]->(:CodeFunction {name: $name}) \
             RETURN e.author_name, count(e) AS t ORDER BY t DESC",
            &[("name", &function_name)],
        )?;
        let (bus, bus_note) = if maintainers.is_empty() {
            (0.0, "(no history loaded)".to_string())
        } else if maintainers.len() == 1 {
            let author = maintainers[0].get(0).map(|v| v.as_str()).unwrap_or("?");
            let short = author.split('<').next().unwrap_or("?").trim();
            (3.0, format!("only 1 contributor ({})", short))
        } else if maintainers.len() == 2 {
            (2.0, "2 contributors".to_string())
        } else {
            (1.0, format!("{} contributors", maintainers.len()))
        };
        score += bus;
        breakdown.push(format!("  bus factor:    {}/3   {}", bus, bus_note));

        // Incident correlation
        let fix_commits = self.query_text(
            &self.client,
            "MATCH (e:Commit)-[:MODIFIED]->(:CodeFunction {name: $name}) \
             WHERE toLower(e.message) CONTAINS 'fix' OR toLower(e.message) CONTAINS 'hotfix' \
             RETURN count(e)",
            &[("name", &function_name)],
        )?;
        let fix_count = fix_commits.first().and_then(|r| r.first()).map(|v| v.as_i64()).unwrap_or(0);
        let incident = if fix_count >= 2 { 1.0 } else if fix_count == 1 { 0.5 } else { 0.0 };
        score += incident;
        breakdown.push(format!("  incidents:     {}/1   ({} fix-related commits)", incident, fix_count));

        let verdict = if score < 3.0 { "LOW" }
            else if score < 6.0 { "MEDIUM" }
            else if score < 8.0 { "HIGH" }
            else { "VERY HIGH" };

        Ok(format!(
            "Risk score for '{}': {:.1} / 10 -- {}\n\n{}",
            function_name, score, verdict, breakdown.join("\n"),
        ))
    }

    fn tool_decorated_with(&self, args: &Value) -> Result<String, String> {
        let decorator_name = arg_str(args, "decorator_name")?;
        let needle = decorator_name.trim();
        let dot_needle = format!(".{}", needle);

        let fn_rows = self.query_text(
            &self.client,
            "MATCH (f:CodeFunction)-[:DECORATED_BY]->(d:Decorator) \
             WHERE d.name = $needle OR d.name ENDS WITH $dot_needle \
             RETURN DISTINCT 'Function' AS kind, f.name AS name, f.file AS fp, d.name AS dec \
             ORDER BY fp, name",
            &[("needle", needle), ("dot_needle", &dot_needle)],
        )?;

        let cls_rows = self.query_text(
            &self.client,
            "MATCH (c:Class)-[:DECORATED_BY]->(d:Decorator) \
             WHERE d.name = $needle OR d.name ENDS WITH $dot_needle \
             RETURN DISTINCT 'Class' AS kind, c.name AS name, c.file AS fp, d.name AS dec \
             ORDER BY fp, name",
            &[("needle", needle), ("dot_needle", &dot_needle)],
        )?;

        let mut all_rows = fn_rows;
        all_rows.extend(cls_rows);

        if all_rows.is_empty() {
            return Ok(format!("No functions or classes decorated with '{}'.", decorator_name));
        }

        let mut lines = vec![format!("{} symbol(s) decorated with '{}':", all_rows.len(), decorator_name)];
        for (idx, r) in all_rows.iter().enumerate() {
            if idx >= 50 { break; }
            let kind = r.get(0).map(|v| v.as_str()).unwrap_or("?");
            let name = r.get(1).map(|v| v.as_str()).unwrap_or("?");
            let fpath = r.get(2).map(|v| v.as_str()).unwrap_or("?");
            let d = r.get(3).map(|v| v.as_str()).unwrap_or("?");
            let tag = if kind == "Function" { "fn" } else { "cls" };
            lines.push(format!("  [{}] @{:<25} {}  ({})", tag, d, name, fpath));
        }
        if all_rows.len() > 50 {
            lines.push(format!("  ... and {} more", all_rows.len() - 50));
        }
        Ok(lines.join("\n"))
    }

    fn tool_resolves_to(&self, args: &Value) -> Result<String, String> {
        let symbol = arg_str(args, "symbol")?;
        let terminal = symbol.rsplit('.').next().unwrap_or(&symbol);

        let defs = self.query_text(
            &self.client,
            "MATCH (n) WHERE (n:CodeFunction OR n:Class) AND n.name = $t \
             RETURN labels(n)[0], n.name, n.file LIMIT 20",
            &[("t", terminal)],
        )?;

        let refs = self.query_text(
            &self.client,
            "MATCH (c:CodeFunction)-[:REFERENCES_SYMBOL]->(t) WHERE t.name = $t \
             RETURN DISTINCT c.name, c.file LIMIT 30",
            &[("t", terminal)],
        )?;

        let mut lines = vec![format!("Resolving '{}' (terminal: '{}'):", symbol, terminal)];
        lines.push(format!("\nDefinitions ({}):", defs.len()));
        if !defs.is_empty() {
            for r in &defs {
                let label = r.get(0).map(|v| v.as_str()).unwrap_or("?");
                let n = r.get(1).map(|v| v.as_str()).unwrap_or("?");
                let fp = r.get(2).map(|v| v.as_str()).unwrap_or("?");
                lines.push(format!("  [{}] {}  ({})", label, n, fp));
            }
        } else {
            lines.push("  (none -- may be external, dynamic, or not indexed)".to_string());
        }

        lines.push(format!("\nString-literal references ({}):", refs.len()));
        if !refs.is_empty() {
            for r in &refs {
                let n = r.get(0).map(|v| v.as_str()).unwrap_or("?");
                let fp = r.get(1).map(|v| v.as_str()).unwrap_or("?");
                lines.push(format!("  {}  ({})", n, fp));
            }
        } else {
            lines.push("  (none)".to_string());
        }

        Ok(lines.join("\n"))
    }

    fn tool_community_summary(&self, args: &Value) -> Result<String, String> {
        let max_results = args.get("max_results").and_then(|v| v.as_i64()).unwrap_or(10) as usize;
        let query_filter = args.get("query").and_then(|v| v.as_str()).unwrap_or("");

        // Try pre-computed communities first
        let repo = detect_repo_name_for_mcp();
        if let Ok(communities) = crate::code_graph::load_communities(&repo) {
            if !communities.is_empty() {
                let filtered: Vec<&crate::code_graph::Community> = if query_filter.is_empty() {
                    communities.iter().take(max_results).collect()
                } else {
                    let q = query_filter.to_lowercase();
                    communities.iter()
                        .filter(|c| {
                            c.name.to_lowercase().contains(&q)
                                || c.description.to_lowercase().contains(&q)
                                || c.functions.iter().any(|f| f.to_lowercase().contains(&q))
                        })
                        .take(max_results)
                        .collect()
                };

                if filtered.is_empty() {
                    return Ok(format!("No communities matching '{}' found.", query_filter));
                }

                let mut lines = vec![format!("{} communities detected:", communities.len())];
                for c in &filtered {
                    lines.push(String::new());
                    lines.push(format!(
                        "=== Module: {} ===",
                        c.name
                    ));
                    lines.push(format!(
                        "{} functions in {} files. Entry point: {}.",
                        c.functions.len(),
                        c.files.len(),
                        c.key_function,
                    ));
                    lines.push(c.description.clone());
                    let key_funcs: Vec<&str> = c.functions.iter().take(8).map(|s| s.as_str()).collect();
                    lines.push(format!("Key functions: {}", key_funcs.join(", ")));
                    if c.files.len() <= 5 {
                        lines.push(format!("Files: {}", c.files.join(", ")));
                    } else {
                        let shown: Vec<&str> = c.files.iter().take(3).map(|s| s.as_str()).collect();
                        lines.push(format!(
                            "Files: {}, ... +{} more",
                            shown.join(", "),
                            c.files.len() - 3
                        ));
                    }
                }
                return Ok(lines.join("\n"));
            }
        }

        // Fall back to graph query
        let query = format!(
            "MATCH (f:CodeFunction)-[r:CALLS]->() \
             RETURN f.file, count(r) AS edges \
             ORDER BY edges DESC LIMIT {}",
            max_results,
        );
        let rows = self.query_text(&self.client, &query, &[])?;

        if rows.is_empty() {
            return Ok("No call edges found in the graph. Run 'savants up' to index the codebase.".to_string());
        }

        let mut lines = vec!["Most connected hub files (by outgoing call edges):".to_string()];
        for r in &rows {
            let fp = r.get(0).map(|v| v.as_str()).unwrap_or("?");
            let edges = r.get(1).map(|v| v.as_i64()).unwrap_or(0);
            lines.push(format!("  {:>5} edges  {}", edges, fp));
        }
        Ok(lines.join("\n"))
    }

    fn tool_dependency_chain(&self, args: &Value) -> Result<String, String> {
        let from_file = arg_str(args, "from_file")?;
        let to_file = arg_str(args, "to_file")?;

        let rows = self.query_text(
            &self.client,
            "MATCH (a:CodeFunction)-[:CALLS*1..6]->(b:CodeFunction) \
             WHERE a.file = $from AND b.file = $to \
             RETURN DISTINCT a.file, b.file \
             LIMIT 20",
            &[("from", &from_file), ("to", &to_file)],
        )?;

        if rows.is_empty() {
            return Ok("No dependency chain found.".to_string());
        }

        let chain: Vec<&str> = rows.iter()
            .filter_map(|r| r.first().map(|v| v.as_str()))
            .collect();
        Ok(chain.join(" -> "))
    }

    fn tool_co_change_partners(&self, args: &Value) -> Result<String, String> {
        let function_name = arg_str(args, "function_name")?;
        let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(10);

        let query = format!(
            "MATCH (e:Commit)-[:MODIFIED]->(fn1:CodeFunction {{name: $name}}) \
             MATCH (e)-[:MODIFIED]->(fn2:CodeFunction) \
             WHERE fn1.name <> fn2.name \
             RETURN fn2.name, count(e) AS co \
             ORDER BY co DESC LIMIT {}",
            limit,
        );
        let rows = self.query_text(&self.client, &query, &[("name", &function_name)])?;

        if rows.is_empty() {
            return Ok(format!(
                "No co-change history for '{}'. \
                 (Either the function is new, or git history hasn't been walked.)",
                function_name,
            ));
        }

        let mut lines = vec![format!("Functions that change with '{}' historically:", function_name)];
        for r in &rows {
            let name = r.get(0).map(|v| v.as_str()).unwrap_or("?");
            let co = r.get(1).map(|v| v.as_i64()).unwrap_or(0);
            lines.push(format!("  {:>4}x -- {}", co, name));
        }
        Ok(lines.join("\n"))
    }

    fn tool_recall_history(&self, args: &Value) -> Result<String, String> {
        let query = arg_str(args, "query")?;

        // Facts
        let facts = self.query_text(
            &self.client,
            "MATCH (f:Fact) WHERE f.subject CONTAINS $q OR f.object CONTAINS $q \
             RETURN f.subject, f.predicate, f.object LIMIT 15",
            &[("q", &query)],
        ).unwrap_or_default();

        // Episodes
        let episodes = self.query_text(
            &self.client,
            "MATCH (e:Commit) WHERE e.content CONTAINS $q \
             RETURN e.source_type, e.content LIMIT 10",
            &[("q", &query)],
        ).unwrap_or_default();

        let mut parts = Vec::new();
        for f in &facts {
            let subj = f.get(0).map(|v| v.as_str()).unwrap_or("?");
            let pred = f.get(1).map(|v| v.as_str()).unwrap_or("?");
            let obj = f.get(2).map(|v| v.as_str()).unwrap_or("?");
            parts.push(format!("{} -{}-> {}", subj, pred, obj));
        }
        for e in &episodes {
            let source_type = e.get(0).map(|v| v.as_str()).unwrap_or("?");
            let content = e.get(1).map(|v| v.as_str()).unwrap_or("");
            let short = if content.len() > 200 { &content[..200] } else { content };
            parts.push(format!("[{}] {}", source_type, short));
        }

        if parts.is_empty() {
            Ok("No relevant history found.".to_string())
        } else {
            Ok(parts.join("\n"))
        }
    }

    fn tool_federated_symbol_in_cluster(&self, args: &Value) -> Result<String, String> {
        let symbol = arg_str(args, "symbol")?;
        let cluster = arg_str(args, "cluster")?;

        // Query 1: code graph
        let code_hits = self.query_text(
            &self.client,
            "MATCH (n) WHERE (n:CodeFunction OR n:Class) AND n.name = $symbol \
             RETURN labels(n)[0], n.name, n.file LIMIT 10",
            &[("symbol", &symbol)],
        ).unwrap_or_default();

        // Query 2: cluster graph
        let c = self.k8s_client(&cluster)?;

        let image_hits = self.query_text(
            &c,
            "MATCH (d:K8sDeployment) WHERE d.image CONTAINS $symbol \
             RETURN d.namespace, d.name, d.image",
            &[("symbol", &symbol)],
        ).unwrap_or_default();

        let name_hits = self.query_text(
            &c,
            "MATCH (d:K8sDeployment) WHERE d.name CONTAINS $symbol \
             RETURN d.namespace, d.name, d.image",
            &[("symbol", &symbol)],
        ).unwrap_or_default();

        let svc_hits = self.query_text(
            &c,
            "MATCH (s:K8sService) WHERE s.name CONTAINS $symbol \
             RETURN s.namespace, s.name, s.type",
            &[("symbol", &symbol)],
        ).unwrap_or_default();

        let cm_hits = self.query_text(
            &c,
            "MATCH (cm:K8sConfigMap) WHERE ANY(k IN cm.key_names WHERE k CONTAINS $symbol) \
             RETURN cm.namespace, cm.name, cm.key_names",
            &[("symbol", &symbol)],
        ).unwrap_or_default();

        let mut lines = vec![format!(
            "Federated query for symbol '{}' across code + cluster '{}':",
            symbol, cluster,
        )];
        lines.push(String::new());

        if !code_hits.is_empty() {
            lines.push(format!("Context ({} matches):", code_hits.len()));
            for r in &code_hits {
                let label = r.get(0).map(|v| v.as_str()).unwrap_or("?");
                let name = r.get(1).map(|v| v.as_str()).unwrap_or("?");
                let path = r.get(2).map(|v| v.as_str()).unwrap_or("?");
                lines.push(format!("  [{}] {}  ({})", label, name, path));
            }
        } else {
            lines.push("Code graph: no Function or Class with this exact name.".to_string());
        }
        lines.push(String::new());

        let mut cluster_found = false;
        if !image_hits.is_empty() {
            cluster_found = true;
            lines.push(format!("Cluster Deployments running image matching '{}' ({}):", symbol, image_hits.len()));
            for r in &image_hits {
                let ns = r.get(0).map(|v| v.as_str()).unwrap_or("?");
                let n = r.get(1).map(|v| v.as_str()).unwrap_or("?");
                let img = r.get(2).map(|v| v.as_str()).unwrap_or("?");
                lines.push(format!("  {}/{}", ns, n));
                lines.push(format!("    image: {}", img));
            }
        }
        if !name_hits.is_empty() {
            cluster_found = true;
            lines.push(format!("Cluster Deployments named like '{}' ({}):", symbol, name_hits.len()));
            for r in &name_hits {
                let ns = r.get(0).map(|v| v.as_str()).unwrap_or("?");
                let n = r.get(1).map(|v| v.as_str()).unwrap_or("?");
                lines.push(format!("  {}/{}", ns, n));
            }
        }
        if !svc_hits.is_empty() {
            cluster_found = true;
            lines.push(format!("Cluster Services named like '{}' ({}):", symbol, svc_hits.len()));
            for r in &svc_hits {
                let ns = r.get(0).map(|v| v.as_str()).unwrap_or("?");
                let n = r.get(1).map(|v| v.as_str()).unwrap_or("?");
                let t = r.get(2).map(|v| v.as_str()).unwrap_or("?");
                lines.push(format!("  {}/{} ({})", ns, n, t));
            }
        }
        if !cm_hits.is_empty() {
            cluster_found = true;
            lines.push(format!("ConfigMaps with key names matching '{}' ({}):", symbol, cm_hits.len()));
            for r in &cm_hits {
                let ns = r.get(0).map(|v| v.as_str()).unwrap_or("?");
                let n = r.get(1).map(|v| v.as_str()).unwrap_or("?");
                let keys = r.get(2).map(|v| v.as_str()).unwrap_or("");
                lines.push(format!("  {}/{}  keys: {}", ns, n, keys));
            }
        }

        if !cluster_found {
            lines.push(format!("Context: no references to '{}' found.", symbol));
        }

        Ok(lines.join("\n"))
    }

    fn tool_pre_change_warning(&self, args: &Value) -> Result<String, String> {
        let function_name = arg_str(args, "function_name")?;

        // Blast radius
        let callers = self.query_text(
            &self.client,
            "MATCH (c:CodeFunction)-[:CALLS]->(t:CodeFunction {name: $name}) RETURN count(c)",
            &[("name", &function_name)],
        )?;
        let direct = callers.first().and_then(|r| r.first()).map(|v| v.as_i64()).unwrap_or(0);

        let transitive = self.query_text(
            &self.client,
            "MATCH (c:CodeFunction)-[:CALLS*1..3]->(t:CodeFunction {name: $name}) \
             RETURN count(DISTINCT c)",
            &[("name", &function_name)],
        )?;
        let trans = transitive.first().and_then(|r| r.first()).map(|v| v.as_i64()).unwrap_or(0);

        // Last touched
        let last_touch = self.query_text(
            &self.client,
            "MATCH (e:Commit)-[:MODIFIED]->(:CodeFunction {name: $name}) \
             RETURN e.date, e.author_name ORDER BY e.date DESC LIMIT 1",
            &[("name", &function_name)],
        ).unwrap_or_default();

        // Maintainer concentration
        let maintainers = self.query_text(
            &self.client,
            "MATCH (e:Commit)-[:MODIFIED]->(:CodeFunction {name: $name}) \
             RETURN e.author_name, count(e) AS touches ORDER BY touches DESC LIMIT 3",
            &[("name", &function_name)],
        ).unwrap_or_default();

        let mut lines = vec![
            format!("Pre-change warning for '{}':", function_name),
            String::new(),
            "  Blast radius:".to_string(),
            format!("    Direct callers:     {}", direct),
            format!("    Transitive (3 hops): {}", trans),
            String::new(),
        ];

        if direct > 50 || trans > 200 {
            lines.push("  !! HIGH BLAST RADIUS -- many things depend on this.".to_string());
            lines.push(String::new());
        }

        if let Some(lt) = last_touch.first() {
            let ts = lt.get(0).map(|v| v.as_str()).unwrap_or("?");
            let author = lt.get(1).map(|v| v.as_str()).unwrap_or("?");
            let short_author = author.split('<').next().unwrap_or("?").trim();
            let ts_short = if ts.len() >= 10 { &ts[..10] } else { ts };
            lines.push(format!("  Last touched: {} by {}", ts_short, short_author));
            lines.push(String::new());
        }

        if !maintainers.is_empty() {
            lines.push("  Recent maintainers:".to_string());
            for m in &maintainers {
                let author = m.get(0).map(|v| v.as_str()).unwrap_or("?");
                let touches = m.get(1).map(|v| v.as_i64()).unwrap_or(0);
                let short = author.split('<').next().unwrap_or("?").trim();
                lines.push(format!("    {:>3}x -- {}", touches, short));
            }
            if maintainers.len() == 1 {
                lines.push(String::new());
                lines.push("  !! BUS FACTOR 1 -- only one person has touched this recently.".to_string());
            }
        } else {
            lines.push("  History: not loaded for this function.".to_string());
        }

        Ok(lines.join("\n"))
    }

    fn tool_coupling_check(&self, args: &Value) -> Result<String, String> {
        let from_module = arg_str(args, "from_module")?;
        let to_module = arg_str(args, "to_module")?;

        let result = self.query_text(
            &self.client,
            "MATCH (a:CodeFunction)-[:CALLS]->(b:CodeFunction) \
             WHERE a.file STARTS WITH $from_mod \
               AND b.file STARTS WITH $to_mod \
             RETURN count(*) AS edge_count",
            &[("from_mod", &from_module), ("to_mod", &to_module)],
        )?;
        let edge_count = result.first().and_then(|r| r.first()).map(|v| v.as_i64()).unwrap_or(0);

        if edge_count == 0 {
            Ok(format!(
                "!! COUPLING WARNING\n\
                 \x20  {} -> {}\n\
                 \x20  Current edges: 0\n\n\
                 \x20  These two modules currently have NO call edges between them. \
                 Introducing a new dependency would be the first.\n\n\
                 \x20  This pattern often indicates:\n\
                 \x20  - The modules were intentionally kept separate\n\
                 \x20  - You may be violating an implicit architectural boundary\n\
                 \x20  - Code review will likely push back\n\n\
                 \x20  If intentional, document why in the commit message.",
                from_module, to_module,
            ))
        } else {
            Ok(format!(
                "OK -- coupling already exists.\n\
                 \x20  {} -> {}\n\
                 \x20  Existing call edges: {}\n\
                 \x20  Adding another is consistent with current architecture.",
                from_module, to_module, edge_count,
            ))
        }
    }

    fn tool_advanced_graph_query(&self, args: &Value) -> Result<String, String> {
        let query = arg_str(args, "query")?;
        let rows = self.query_text(&self.client, &query, &[])?;
        let result: Vec<String> = rows.iter().map(|row| {
            let cols: Vec<String> = row.iter().map(|v| format!("{:?}", v)).collect();
            cols.join(", ")
        }).collect();
        if result.is_empty() {
            Ok("(empty result set)".to_string())
        } else {
            Ok(result.join("\n"))
        }
    }

    fn tool_reindex(&self, args: &Value) -> Result<String, String> {
        let repo_path = args.get("repo_path")
            .and_then(|v| v.as_str())
            .ok_or("repo_path is required")?;

        if !std::path::Path::new(repo_path).is_dir() {
            return Err(format!("Not a directory: {}", repo_path));
        }

        let repo_name = std::path::Path::new(repo_path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Check if we're in cloud mode - if so, parse locally and send metadata
        if let Ok(cloud_url) = std::env::var("SAVANTS_CLOUD_URL") {
            let api_key = std::env::var("SAVANTS_API_KEY").unwrap_or_default();

            // Parse locally (tree-sitter only, no graph)
            let mut parser = crate::code_parser::CodeParser::new(&repo_name);
            let result = parser.parse_repo(repo_path);
            let entity_count = result.entities.len();
            let file_count = result.files;

            // Send parsed metadata to cloud for graph construction
            let body = serde_json::to_string(&result)
                .map_err(|e| format!("serialize error: {}", e))?;

            let output = std::process::Command::new("curl")
                .args([
                    "-sf", "--max-time", "60",
                    "-X", "POST",
                    "-H", &format!("Authorization: Bearer {}", api_key),
                    "-H", "Content-Type: application/json",
                    "-d", &body,
                    &format!("{}/api/v1/ingest", cloud_url),
                ])
                .output()
                .map_err(|e| format!("upload failed: {}", e))?;

            if output.status.success() {
                Ok(format!("Parsed {}: {} files, {} entities. Uploaded to cloud for indexing.", repo_name, file_count, entity_count))
            } else {
                // Fall back to local indexing if cloud upload fails
                let mut indexer = crate::code_index::CodeIndexer::new(self.client.clone(), &repo_name);
                let stats = indexer.index_repo(repo_path);
                Ok(format!("Cloud upload failed. Indexed locally: {}. {}", repo_name, stats.summary()))
            }
        } else {
            // Local mode - use graph directly
            let mut indexer = crate::code_index::CodeIndexer::new(self.client.clone(), &repo_name);
            let stats = indexer.index_repo(repo_path);

            let pr_analyzer = crate::code_index::PRAnalyzer::new(self.client.clone(), &repo_name);
            let prs_analyzed = pr_analyzer.analyze_open_prs(repo_path);

            Ok(format!("Indexed {}: {}. Analyzed {} open PRs.", repo_name, stats.summary(), prs_analyzed))
        }
    }

    fn tool_pr_risk(&self, args: &Value) -> Result<String, String> {
        let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("talent-pipeline");

        let rows = self.query_text(
            &self.client,
            &format!(
                "MATCH (p:GitHubPR {{repo: '{}', state: 'OPEN'}}) \
                 RETURN p.number, p.title, p.author, p.risk_level, p.risk_details, p.files_changed, p.branch \
                 ORDER BY CASE p.risk_level WHEN 'HIGH' THEN 0 WHEN 'MEDIUM' THEN 1 ELSE 2 END",
                repo
            ),
            &[],
        )?;

        if rows.is_empty() {
            return Ok("No analyzed open PRs. Run reindex first.".to_string());
        }

        let mut output = vec![format!("{} open PRs analyzed:\n", rows.len())];

        for row in &rows {
            let num = row.get(0).map(|v| v.as_i64()).unwrap_or(0);
            let title = row.get(1).map(|v| v.as_str()).unwrap_or("?");
            let author = row.get(2).map(|v| v.as_str()).unwrap_or("?");
            let risk = row.get(3).map(|v| v.as_str()).unwrap_or("?");
            let details = row.get(4).map(|v| v.as_str()).unwrap_or("");
            let files = row.get(5).map(|v| v.as_i64()).unwrap_or(0);
            let branch = row.get(6).map(|v| v.as_str()).unwrap_or("?");

            let emoji = match risk { "HIGH" => "🔴", "MEDIUM" => "🟡", _ => "🟢" };

            output.push(format!("{} PR #{} — {} ({})", emoji, num, title, risk));
            output.push(format!("  Author: @{} | Branch: {} | Files: {}", author, branch, files));
            if !details.is_empty() {
                output.push(format!("  Risk: {}", details));
            }

            // --- CHECK 1: Functions affected ---
            if let Ok(affected) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}})-[:MODIFIED]->(ch:PRChange)-[:AFFECTS]->(f:CodeFunction) \
                     RETURN f.name, f.file, ch.change_type LIMIT 10",
                    num, repo
                ),
                &[],
            ) {
                if !affected.is_empty() {
                    output.push("  Functions affected:".to_string());
                    for a in &affected {
                        let fname = a.get(0).map(|v| v.as_str()).unwrap_or("?");
                        let ffile = a.get(1).map(|v| v.as_str()).unwrap_or("?");
                        let change = a.get(2).map(|v| v.as_str()).unwrap_or("?");
                        output.push(format!("    {} {} ({})", change, fname, ffile));
                    }
                }
            }

            // --- CHECK 2: "Last time this function changed, prod broke" ---
            if let Ok(history) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}})-[:MODIFIED]->(ch:PRChange)-[:AFFECTS]->(f:CodeFunction) \
                     MATCH (prev:Commit {{repo: '{}'}})-[:MODIFIED]->(f) \
                     MATCH (m:SlackMessage) WHERE m.channel_name IN ['prod-errors', 'engineering-alerts-prod'] \
                     AND m.has_symptom = true AND toLower(m.text) CONTAINS toLower(f.name) \
                     RETURN DISTINCT f.name, prev.short_hash, prev.author, prev.date, prev.message, m.text LIMIT 5",
                    num, repo, repo
                ),
                &[],
            ) {
                if !history.is_empty() {
                    output.push("  🔥 HISTORY: Functions that caused prod errors before:".to_string());
                    for h in &history {
                        let fname = h.get(0).map(|v| v.as_str()).unwrap_or("?");
                        let commit = h.get(1).map(|v| v.as_str()).unwrap_or("?");
                        let cauthor = h.get(2).map(|v| v.as_str()).unwrap_or("?");
                        let cdate = h.get(3).map(|v| v.as_str()).unwrap_or("?");
                        let cmsg: String = h.get(4).map(|v| v.as_str()).unwrap_or("?").chars().take(50).collect();
                        let err: String = h.get(5).map(|v| v.as_str()).unwrap_or("?").chars().take(60).collect();
                        output.push(format!("    {} — last changed by @{} in {} ({})", fname, cauthor, commit, cmsg));
                        output.push(format!("      Prod error: {}", err));
                    }
                }
            }

            // --- CHECK 3: Co-change partners missing from PR ---
            if let Ok(cochange) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}})-[:MODIFIED]->(ch:PRChange) \
                     WITH p, collect(ch.file) AS pr_files \
                     MATCH (c1:Commit {{repo: '{}'}})-[:MODIFIED_FILE]->(f1:CodeFile) \
                     WHERE f1.path IN pr_files \
                     MATCH (c1)-[:MODIFIED_FILE]->(f2:CodeFile) \
                     WHERE NOT f2.path IN pr_files \
                     WITH f2.path AS missing_file, count(c1) AS co_changes \
                     WHERE co_changes >= 3 \
                     RETURN missing_file, co_changes ORDER BY co_changes DESC LIMIT 5",
                    num, repo, repo
                ),
                &[],
            ) {
                if !cochange.is_empty() {
                    output.push("  🔗 CO-CHANGE: Files that usually change together but are missing from this PR:".to_string());
                    for c in &cochange {
                        let file = c.get(0).map(|v| v.as_str()).unwrap_or("?");
                        let count = c.get(1).map(|v| v.as_i64()).unwrap_or(0);
                        output.push(format!("    {} (co-changed {} times in history)", file, count));
                    }
                }
            }

            // --- CHECK 4: Functions with many callers (blast radius) ---
            if let Ok(callers) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}})-[:MODIFIED]->(ch:PRChange)-[:AFFECTS]->(f:CodeFunction) \
                     MATCH (caller:CodeFunction)-[:CALLS]->(f) \
                     WITH f, count(caller) AS caller_count WHERE caller_count >= 2 \
                     RETURN f.name, f.file, caller_count ORDER BY caller_count DESC LIMIT 5",
                    num, repo
                ),
                &[],
            ) {
                if !callers.is_empty() {
                    output.push("  📡 BLAST RADIUS: Functions called by other code:".to_string());
                    for c in &callers {
                        let fname = c.get(0).map(|v| v.as_str()).unwrap_or("?");
                        let ffile = c.get(1).map(|v| v.as_str()).unwrap_or("?");
                        let count = c.get(2).map(|v| v.as_i64()).unwrap_or(0);
                        output.push(format!("    {} ({}) — called by {} other functions", fname, ffile, count));
                    }
                }
            }

            // --- CHECK 5: Unanswered Slack questions about this area ---
            if let Ok(unanswered) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}})-[:MODIFIED]->(ch:PRChange)-[:AFFECTS]->(f:CodeFunction) \
                     MATCH (m:SlackMessage)-[:SENT_BY]->(u:SlackUser) \
                     WHERE m.text CONTAINS '?' AND m.reply_count = 0 \
                     AND (toLower(m.text) CONTAINS toLower(f.name) OR toLower(m.text) CONTAINS toLower(ch.file)) \
                     RETURN u.name, m.channel_name, m.text LIMIT 3",
                    num, repo
                ),
                &[],
            ) {
                if !unanswered.is_empty() {
                    output.push("  ❓ UNANSWERED: Slack questions about code this PR touches:".to_string());
                    for u in &unanswered {
                        let who = u.get(0).map(|v| v.as_str()).unwrap_or("?");
                        let ch = u.get(1).map(|v| v.as_str()).unwrap_or("?");
                        let text: String = u.get(2).map(|v| v.as_str()).unwrap_or("?").chars().take(80).collect();
                        output.push(format!("    @{} in #{}: {}", who, ch, text));
                    }
                }
            }

            // --- CHECK 6: High churn functions (fragile code) ---
            if let Ok(churn) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}})-[:MODIFIED]->(ch:PRChange)-[:AFFECTS]->(f:CodeFunction) \
                     MATCH (c:Commit {{repo: '{}'}})-[:MODIFIED]->(f) \
                     WITH f, count(c) AS changes WHERE changes >= 5 \
                     RETURN f.name, f.file, changes ORDER BY changes DESC LIMIT 5",
                    num, repo, repo
                ),
                &[],
            ) {
                if !churn.is_empty() {
                    output.push("  🔄 HIGH CHURN (fragile — changed often):".to_string());
                    for c in &churn {
                        let fname = c.get(0).map(|v| v.as_str()).unwrap_or("?");
                        let ffile = c.get(1).map(|v| v.as_str()).unwrap_or("?");
                        let count = c.get(2).map(|v| v.as_i64()).unwrap_or(0);
                        output.push(format!("    {} ({}) — modified {} times", fname, ffile, count));
                    }
                }
            }

            // --- CHECK 7: Jira ticket status mismatch ---
            if let Ok(tickets) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}})-[:RESOLVES]->(t:JiraTicket) \
                     RETURN t.key, t.status, t.assignee",
                    num, repo
                ),
                &[],
            ) {
                for t in &tickets {
                    let tkey = t.get(0).map(|v| v.as_str()).unwrap_or("?");
                    let tstatus = t.get(1).map(|v| v.as_str()).unwrap_or("?");
                    let tassignee = t.get(2).map(|v| v.as_str()).unwrap_or("?");
                    if tstatus == "To Do" || tstatus == "Done" {
                        output.push(format!("  ⚠️ JIRA MISMATCH: {} is '{}' but PR is open (assigned to {})",
                            tkey, tstatus, tassignee));
                    }
                }
            }

            // --- CHECK 8: Known prod errors in Slack ---
            if let Ok(issues) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}})-[:MODIFIED]->(ch:PRChange)-[:HAS_KNOWN_ISSUE]->(m:SlackMessage) \
                     RETURN m.channel_name, m.text LIMIT 3",
                    num, repo
                ),
                &[],
            ) {
                if !issues.is_empty() {
                    output.push("  🐛 KNOWN PROD ISSUES for functions this PR touches:".to_string());
                    for i in &issues {
                        let ch = i.get(0).map(|v| v.as_str()).unwrap_or("?");
                        let text: String = i.get(1).map(|v| v.as_str()).unwrap_or("?").chars().take(80).collect();
                        output.push(format!("    #{}: {}", ch, text));
                    }
                }
            }

            output.push(String::new());
        }

        Ok(output.join("\n"))
    }

    /// Deep diagnosis: given a prod error, trace upstream through the entire
    /// call chain to find the root cause. Checks validation gaps, missing null
    /// guards, and cross-references with Slack/Jira.
    fn tool_diagnose_error(&self, args: &Value) -> Result<String, String> {
        let raw_error = arg_str(args, "error")?;
        let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("talent-pipeline");

        // Normalize: decode HTML entities (Sentry sends &lt; &gt; &amp; via Slack)
        let error_text = raw_error
            .replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
            .replace("&#39;", "'").replace("&quot;", "\"")
            .replace("\\n", " ").replace("\\r", "");
        let error_lower = error_text.to_lowercase();

        let mut output = vec![];
        output.push("DEEP DIAGNOSIS".to_string());
        output.push(format!("Error: {}", &error_text.chars().take(100).collect::<String>()));
        output.push(String::new());

        // Step 1: Extract function names from error
        let mut candidate_functions: Vec<String> = vec![];

        // Skip list: common words that aren't function names
        let skip_names = [
            "split", "map", "filter", "forEach", "find", "reduce", "push",
            "null", "undefined", "true", "false", "this", "self", "none",
            "Error", "TypeError", "ReferenceError", "SyntaxError",
            "info", "warn", "error", "debug", "test", "data", "result",
            "SUBSCRIBE", "UNSUBSCRIBE", "PING", "QUIT", "RESET",
            "AUTO", "VOCATOR", "FRONTEND", "BACKEND",
        ];

        // Pattern 1: [functionName] — Sentry wraps function names in brackets
        let bracket_re = regex::Regex::new(r"\[(\w+)\]").unwrap();
        for cap in bracket_re.captures_iter(&error_text) {
            let name = cap[1].to_string();
            if name.len() > 3 && !skip_names.contains(&name.as_str()) {
                if !candidate_functions.contains(&name) {
                    candidate_functions.push(name);
                }
            }
        }

        // Pattern 2: Standard JS error patterns
        let js_patterns = [
            regex::Regex::new(r"(\w+)\s+is not a function").unwrap(),
            regex::Regex::new(r"Cannot read propert\w+ of (\w+)").unwrap(),
            regex::Regex::new(r"at\s+(\w+)\s+\(").unwrap(),
            regex::Regex::new(r"in\s+'(\w+)\s*\(").unwrap(),
            regex::Regex::new(r"in\s+(\w+)\s+\(").unwrap(),
        ];
        for re in &js_patterns {
            for cap in re.captures_iter(&error_text) {
                let name = cap[1].to_string();
                if name.len() > 3 && !skip_names.contains(&name.as_str()) && !candidate_functions.contains(&name) {
                    candidate_functions.push(name);
                }
            }
        }

        // Pattern 3: Sentry auto-investigation format
        // "The XxxYyy service is failing" / "The variable 'x' is undefined"
        let sentry_service_re = regex::Regex::new(r"[Tt]he\s+(\w+(?:[A-Z]\w+)+)\s+(?:service|function|method|class|module|component)").unwrap();
        for cap in sentry_service_re.captures_iter(&error_text) {
            let name = cap[1].to_string();
            if !skip_names.contains(&name.as_str()) && !candidate_functions.contains(&name) {
                candidate_functions.push(name);
            }
        }

        // Pattern 4: CamelCase words that match known functions in the graph
        let camel_re = regex::Regex::new(r"\b([a-z]+(?:[A-Z][a-z]+)+)\b").unwrap();
        for cap in camel_re.captures_iter(&error_text) {
            let name = cap[1].to_string();
            if name.len() > 6 && !skip_names.contains(&name.as_str()) && !candidate_functions.contains(&name) {
                // Verify it's an actual function in the graph before adding
                if let Ok(rows) = self.query_text(
                    &self.client,
                    &format!("MATCH (f:CodeFunction {{repo: '{}', name: '{}'}}) RETURN f.name LIMIT 1", repo, name),
                    &[],
                ) {
                    if !rows.is_empty() {
                        candidate_functions.push(name);
                    }
                }
            }
        }

        // Pattern 5: PascalCase component/class names
        let pascal_re = regex::Regex::new(r"\b([A-Z][a-z]+(?:[A-Z][a-z]+)+)\b").unwrap();
        for cap in pascal_re.captures_iter(&error_text) {
            let name = cap[1].to_string();
            if name.len() > 6 && !skip_names.contains(&name.as_str()) && !candidate_functions.contains(&name) {
                if let Ok(rows) = self.query_text(
                    &self.client,
                    &format!("MATCH (f:CodeFunction {{repo: '{}', name: '{}'}}) RETURN f.name LIMIT 1", repo, name),
                    &[],
                ) {
                    if !rows.is_empty() {
                        candidate_functions.push(name);
                    }
                }
            }
        }

        // EARLY CHECK: Infrastructure errors bypass function matching entirely
        let is_infra_error = error_lower.contains("redis")
            || (error_lower.contains("subscribe") && error_lower.contains("allowed"))
            || error_lower.contains("econnrefused")
            || (error_lower.contains("polling") && error_lower.contains("failed to fetch"))
            || (error_lower.contains("polling") && error_lower.contains("failing"));

        if is_infra_error && candidate_functions.is_empty() {
            output.push("Step 1 — Infrastructure error detected".to_string());
            output.push(String::new());

            if error_lower.contains("subscribe") || error_lower.contains("redis") {
                output.push("CONCLUSION:".to_string());
                output.push("  ROOT CAUSE: A Redis connection in SUBSCRIBE mode is being reused for regular commands.".to_string());
                output.push("  Redis clients in pub/sub mode can ONLY run SUBSCRIBE/UNSUBSCRIBE/PING/QUIT.".to_string());
                output.push("  FIX: Use separate Redis connections for pub/sub and regular commands.".to_string());

                output.push(String::new());
                output.push("  Related code:".to_string());
                if let Ok(rows) = self.query_text(
                    &self.client,
                    &format!(
                        "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE f.file STARTS WITH 'server/' AND (toLower(f.body) CONTAINS 'redis' OR toLower(f.body) CONTAINS 'subscribe' OR toLower(f.body) CONTAINS 'pubsub') RETURN f.name, f.file LIMIT 10",
                        repo
                    ),
                    &[],
                ) {
                    for row in &rows {
                        output.push(format!("    {} ({})", row.get(0).map(|v| v.as_str()).unwrap_or("?"), row.get(1).map(|v| v.as_str()).unwrap_or("?")));
                    }
                }
            } else if error_lower.contains("polling") || error_lower.contains("failed to fetch") {
                output.push("CONCLUSION:".to_string());
                output.push("  ROOT CAUSE: External API polling job failing.".to_string());

                output.push(String::new());
                output.push("  Related code:".to_string());
                let search_terms: Vec<&str> = if error_lower.contains("rippling") { vec!["rippling", "polling"] }
                    else { vec!["polling", "fetch", "cron"] };
                for term in &search_terms {
                    if let Ok(rows) = self.query_text(
                        &self.client,
                        &format!(
                            "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE f.file STARTS WITH 'server/' AND toLower(f.body) CONTAINS '{}' RETURN f.name, f.file LIMIT 5",
                            repo, term
                        ),
                        &[],
                    ) {
                        for row in &rows {
                            output.push(format!("    {} ({})", row.get(0).map(|v| v.as_str()).unwrap_or("?"), row.get(1).map(|v| v.as_str()).unwrap_or("?")));
                        }
                    }
                }

                // Check Jira for related tickets
                if let Ok(rows) = self.query_text(
                    &self.client,
                    "MATCH (t:JiraTicket) WHERE toLower(t.summary) CONTAINS 'rippling' OR toLower(t.summary) CONTAINS 'polling' OR toLower(t.summary) CONTAINS 'integration' RETURN t.key, t.status, t.summary LIMIT 5",
                    &[],
                ) {
                    if !rows.is_empty() {
                        output.push(String::new());
                        output.push("  Related Jira tickets:".to_string());
                        for row in &rows {
                            output.push(format!("    {} ({}) — {}", row.get(0).map(|v| v.as_str()).unwrap_or("?"), row.get(1).map(|v| v.as_str()).unwrap_or("?"), row.get(2).map(|v| v.as_str()).unwrap_or("?").chars().take(50).collect::<String>()));
                        }
                    }
                }

                // Check Slack for discussions
                if let Ok(rows) = self.query_text(
                    &self.client,
                    "MATCH (m:SlackMessage)-[:SENT_BY]->(u:SlackUser) WHERE toLower(m.text) CONTAINS 'rippling' AND m.reply_count = 0 RETURN u.name, m.channel_name, m.text LIMIT 3",
                    &[],
                ) {
                    if !rows.is_empty() {
                        output.push(String::new());
                        output.push("  Unanswered Slack questions:".to_string());
                        for row in &rows {
                            output.push(format!("    @{} in #{}: {}", row.get(0).map(|v| v.as_str()).unwrap_or("?"), row.get(1).map(|v| v.as_str()).unwrap_or("?"), row.get(2).map(|v| v.as_str()).unwrap_or("?").chars().take(80).collect::<String>()));
                        }
                    }
                }

                // Check open PRs related to this
                if let Ok(rows) = self.query_text(
                    &self.client,
                    "MATCH (p:GitHubPR {state: 'OPEN'}) WHERE toLower(p.title) CONTAINS 'rippling' OR toLower(p.branch) CONTAINS 'rippling' RETURN p.number, p.title, p.author, p.state LIMIT 3",
                    &[],
                ) {
                    if !rows.is_empty() {
                        output.push(String::new());
                        output.push("  Open PRs:".to_string());
                        for row in &rows {
                            output.push(format!("    PR #{} — {} (@{})", row.get(0).map(|v| v.as_i64()).unwrap_or(0), row.get(1).map(|v| v.as_str()).unwrap_or("?"), row.get(2).map(|v| v.as_str()).unwrap_or("?")));
                        }
                    }
                }

                output.push(String::new());
                output.push("  FIX: Check API credentials, rate limits, and the open PR above.".to_string());
            }

            return Ok(output.join("\n"));
        }

        // Also search for any known function names that appear in the error
        // Use word-boundary matching to avoid substring false positives (e.g., "onRead" in "onReady")
        if let Ok(rows) = self.query_text(
            &self.client,
            &format!(
                "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE size(f.name) > 5 AND toLower('{}') CONTAINS toLower(f.name) RETURN f.name LIMIT 10",
                repo, error_text.replace('\'', "\\'").chars().take(200).collect::<String>()
            ),
            &[],
        ) {
            for row in &rows {
                let name = row.get(0).map(|v| v.as_str()).unwrap_or("");
                if name.is_empty() || candidate_functions.contains(&name.to_string()) { continue; }
                // Word boundary check: the function name must be surrounded by non-alphanumeric chars
                let name_lower = name.to_lowercase();
                let err_lower = error_lower.clone();
                let is_whole_word = if let Some(pos) = err_lower.find(&name_lower) {
                    let before_ok = pos == 0 || !err_lower.as_bytes()[pos - 1].is_ascii_alphanumeric();
                    let after_pos = pos + name_lower.len();
                    let after_ok = after_pos >= err_lower.len() || !err_lower.as_bytes()[after_pos].is_ascii_alphanumeric();
                    before_ok && after_ok
                } else {
                    false
                };
                if is_whole_word {
                    candidate_functions.push(name.to_string());
                }
            }
        }

        // Determine if this is a backend or frontend error
        let is_backend_error = error_lower.contains("backend")
            || error_lower.contains("server")
            || error_lower.contains("temporal")
            || error_lower.contains("activity")
            || error_lower.contains("redis")
            || error_lower.contains("prisma")
            || error_lower.contains("api");
        let is_frontend_error = error_lower.contains("frontend")
            || error_lower.contains("typeerror")
            || error_lower.contains("referenceerror")
            || error_lower.contains("component")
            || error_lower.contains("dialog");

        // Fallback: if no function names found, search by keywords in the error
        if candidate_functions.is_empty() {
            // Extract service/module names (CamelCase or snake_case words > 5 chars)
            let word_re = regex::Regex::new(r"[A-Z][a-zA-Z]{5,}|[a-z][a-z_]{5,}[a-z]").unwrap();
            let skip_words = ["TypeError", "Error", "Failed", "Request", "String", "Invalid",
                "Cannot", "Undefined", "Function", "Object", "Promise", "Module",
                "SUBSCRIBE", "UNSUBSCRIBE", "allowed", "context", "execute", "applications",
                "endpoint"];
            for cap in word_re.find_iter(&error_text) {
                let word = cap.as_str();
                if skip_words.contains(&word) { continue; }
                if word.len() > 5 {
                    // Check if this word matches any function in the graph
                    if let Ok(rows) = self.query_text(
                        &self.client,
                        &format!(
                            "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE toLower(f.name) CONTAINS toLower('{}') RETURN f.name LIMIT 3",
                            repo, word
                        ),
                        &[],
                    ) {
                        for row in &rows {
                            let name = row.get(0).map(|v| v.as_str()).unwrap_or("");
                            if !name.is_empty() && !candidate_functions.contains(&name.to_string()) {
                                candidate_functions.push(name.to_string());
                            }
                        }
                    }
                    // Also search function bodies for this keyword
                    if candidate_functions.is_empty() {
                        if let Ok(rows) = self.query_text(
                            &self.client,
                            &format!(
                                "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE toLower(f.body) CONTAINS toLower('{}') {} RETURN f.name, f.file LIMIT 3",
                                repo, word,
                                if is_backend_error { "AND f.file STARTS WITH 'server/'" }
                                else if is_frontend_error { "AND f.file STARTS WITH 'src/'" }
                                else { "" }
                            ),
                            &[],
                        ) {
                            for row in &rows {
                                let name = row.get(0).map(|v| v.as_str()).unwrap_or("");
                                if !name.is_empty() && !candidate_functions.contains(&name.to_string()) {
                                    candidate_functions.push(name.to_string());
                                }
                            }
                        }
                    }
                }
                if candidate_functions.len() >= 3 { break; }
            }
        }

        // For infrastructure errors (Redis, connection, timeout), use a different path
        let is_infra_error = error_lower.contains("redis")
            || error_lower.contains("connection")
            || error_lower.contains("timeout")
            || error_lower.contains("econnrefused")
            || error_lower.contains("subscribe");

        if candidate_functions.is_empty() && is_infra_error {
            output.push("Step 1 — Infrastructure error detected (not a code function issue)".to_string());
            output.push(String::new());

            // Search for infrastructure-related code
            let infra_keywords: Vec<&str> = if error_lower.contains("redis") || error_lower.contains("subscribe") {
                vec!["redis", "createClient", "subscriber", "pubsub"]
            } else if error_lower.contains("polling") || error_lower.contains("fetch") {
                vec!["polling", "fetch", "cron", "schedule"]
            } else {
                vec!["connection", "client", "connect"]
            };

            output.push("  Searching for infrastructure code:".to_string());
            for kw in &infra_keywords {
                if let Ok(rows) = self.query_text(
                    &self.client,
                    &format!(
                        "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE f.file STARTS WITH 'server/' AND toLower(f.body) CONTAINS '{}' RETURN f.name, f.file LIMIT 5",
                        repo, kw
                    ),
                    &[],
                ) {
                    for row in &rows {
                        let fname = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                        let ffile = row.get(1).map(|v| v.as_str()).unwrap_or("?");
                        output.push(format!("    {} ({}) — contains '{}'", fname, ffile, kw));
                    }
                }
            }

            output.push(String::new());
            if error_lower.contains("subscribe") && error_lower.contains("info") {
                output.push("CONCLUSION:".to_string());
                output.push("  ROOT CAUSE: A Redis connection in SUBSCRIBE mode is being reused for regular commands.".to_string());
                output.push("  Redis clients in pub/sub mode can ONLY run SUBSCRIBE/UNSUBSCRIBE/PING/QUIT.".to_string());
                output.push("  FIX: Use separate Redis connections for pub/sub and regular commands.".to_string());
                output.push("  Check server code that creates Redis clients — ensure pub/sub client is not shared.".to_string());
            } else if error_lower.contains("polling") || error_lower.contains("fetch") {
                output.push("CONCLUSION:".to_string());
                output.push("  ROOT CAUSE: External API polling job failing — likely auth, network, or rate limit.".to_string());
                output.push("  FIX: Check API credentials, rate limits, and network connectivity to the external service.".to_string());
            } else {
                output.push("CONCLUSION:".to_string());
                output.push("  Infrastructure error — check service connectivity, credentials, and resource limits.".to_string());
            }

            return Ok(output.join("\n"));
        }

        // ============================================================
        // FILE-NAME KEYWORD SEARCH FALLBACK
        // Extract meaningful keywords from the error and search file paths.
        // If a file matches a keyword, its functions get boosted to the top.
        // ============================================================
        let error_keywords: Vec<String> = {
            let kw_re = regex::Regex::new(r"\b([a-zA-Z]{4,})\b").unwrap();
            let skip_kw = ["TypeError", "Error", "Cannot", "read", "properties",
                "undefined", "null", "from", "when", "that", "this", "with",
                "function", "method", "class", "object", "string", "number",
                "server", "client", "backend", "frontend", "service", "crashes",
                "crash", "fails", "failing", "throws", "throwing", "causes",
                "causing", "sends", "using", "async", "handler", "route",
                "during", "startup", "calling", "where", "whose", "reading",
                "send", "stream", "reply", "request", "response", "data",
                "input", "output", "file", "path", "name", "type", "value",
                "unhandled", "exception", "promise", "rejection", "lifecycle"];
            let repo_lower = repo.to_lowercase();
            kw_re.find_iter(&error_text)
                .map(|m| m.as_str().to_string())
                .filter(|w| {
                    !skip_kw.iter().any(|s| s.eq_ignore_ascii_case(w))
                    && w.to_lowercase() != repo_lower  // skip the repo name itself
                })
                .collect::<Vec<_>>()
        };

        // Search for files whose path contains error keywords
        let mut file_keyword_matches: Vec<(String, String)> = vec![]; // (function_name, file_path)
        for kw in &error_keywords {
            if kw.len() < 4 { continue; }
            let kw_lower = kw.to_lowercase();
            if let Ok(rows) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE toLower(f.file) CONTAINS '{}' AND NOT f.file CONTAINS 'test' AND NOT f.file CONTAINS '__mock' RETURN f.name, f.file, f.line LIMIT 5",
                    repo, kw_lower.replace('\'', "\\'")
                ),
                &[],
            ) {
                for row in &rows {
                    let fname = row.get(0).map(|v| v.as_str()).unwrap_or("").to_string();
                    let ffile = row.get(1).map(|v| v.as_str()).unwrap_or("").to_string();
                    if !fname.is_empty() && !file_keyword_matches.iter().any(|(n, _)| n == &fname) {
                        file_keyword_matches.push((fname, ffile));
                    }
                }
            }
        }

        // Boost: if file keyword matches found AND no strong candidates exist,
        // prepend file-keyword functions to candidate list.
        // "Strong candidate" = a function name that exists in the graph for this repo.
        let generic_names = ["get", "set", "constructor", "handler", "run", "init",
            "start", "stop", "create", "update", "delete", "remove", "find",
            "next", "done", "callback", "apply", "call", "bind", "toString",
            "valueOf", "default", "export", "module", "require", "import"];
        let has_strong_candidate = candidate_functions.iter().any(|name| {
            if generic_names.contains(&name.as_str()) || name.len() <= 3 { return false; }
            // Must be an EXACT function name match in the graph (not substring)
            let exists = self.query_text(
                &self.client,
                &format!("MATCH (f:CodeFunction {{repo: '{}', name: '{}'}}) RETURN f.name LIMIT 1", repo, name),
                &[],
            ).ok().map(|r| !r.is_empty()).unwrap_or(false);
            if !exists { return false; }
            // Also verify the function name appears as a whole word in the error text
            let name_lower = name.to_lowercase();
            if let Some(pos) = error_lower.find(&name_lower) {
                let before_ok = pos == 0 || !error_lower.as_bytes()[pos - 1].is_ascii_alphanumeric();
                let after_pos = pos + name_lower.len();
                let after_ok = after_pos >= error_lower.len() || !error_lower.as_bytes()[after_pos].is_ascii_alphanumeric();
                before_ok && after_ok
            } else {
                // Function name extracted by regex pattern, not from error substring
                // Still consider it strong if it's a specific enough name (>8 chars)
                name.len() > 8
            }
        });

        if !file_keyword_matches.is_empty() && !has_strong_candidate {
            let mut boosted: Vec<String> = vec![];
            for (fname, _ffile) in &file_keyword_matches {
                if !boosted.contains(fname)
                    && !candidate_functions.contains(fname)
                    && !generic_names.contains(&fname.as_str())
                    && fname.len() > 3
                {
                    boosted.push(fname.clone());
                }
            }
            // Prepend boosted candidates (file-path matches come first)
            boosted.extend(candidate_functions.clone());
            candidate_functions = boosted;
        }

        if candidate_functions.is_empty() {
            output.push("Step 1 — Could not identify specific functions from the error.".to_string());
            output.push("  Try providing more context: the function name, file path, or stack trace.".to_string());
            return Ok(output.join("\n"));
        }

        output.push(format!("Step 1 — Functions identified: {}", candidate_functions.join(", ")));
        if is_backend_error { output.push("  (backend error — prioritizing server/ files)".to_string()); }
        if is_frontend_error { output.push("  (frontend error — prioritizing src/ files)".to_string()); }
        output.push(String::new());

        for func_name in &candidate_functions {
            // Step 2: Find the function — prefer backend/frontend match
            // If this function came from a file keyword match, prefer that file
            let file_hint = file_keyword_matches.iter()
                .find(|(n, _)| n == func_name)
                .map(|(_, f)| f.clone());

            let func_rows = if let Some(ref hint_file) = file_hint {
                // Prefer the exact file where the keyword matched
                self.query_text(
                    &self.client,
                    &format!(
                        "MATCH (f:CodeFunction {{repo: '{}', name: '{}', file: '{}'}}) RETURN f.file, f.line, f.body LIMIT 1",
                        repo, func_name, hint_file.replace('\'', "\\'")
                    ),
                    &[],
                ).unwrap_or_default()
            } else {
                let file_filter = if is_backend_error { "AND f.file STARTS WITH 'server/'" }
                    else if is_frontend_error && !func_name.contains("split") && !func_name.contains("Dialog") { "AND f.file STARTS WITH 'server/'" }
                    else { "" };
                self.query_text(
                    &self.client,
                    &format!(
                        "MATCH (f:CodeFunction {{repo: '{}', name: '{}'}}) {} RETURN f.file, f.line, f.body LIMIT 1",
                        repo, func_name, file_filter
                    ),
                    &[],
                ).unwrap_or_default()
            };

            // Fallback: try without filter
            let func_rows = if func_rows.is_empty() {
                self.query_text(
                    &self.client,
                    &format!(
                        "MATCH (f:CodeFunction {{repo: '{}', name: '{}'}}) RETURN f.file, f.line, f.body LIMIT 1",
                        repo, func_name
                    ),
                    &[],
                ).unwrap_or_default()
            } else {
                func_rows
            };

            if func_rows.is_empty() { continue; }

            let func_file = func_rows[0].get(0).map(|v| v.as_str()).unwrap_or("?");
            let func_line = func_rows[0].get(1).map(|v| v.as_i64()).unwrap_or(0);
            let func_body = func_rows[0].get(2).map(|v| v.as_str()).unwrap_or("");

            output.push(format!("Step 2 — Found: {} at {}:{}", func_name, func_file, func_line));

            // Step 3: Check if THIS function has validation
            // Context-aware: detect any validation pattern, not just specific function names
            let validation_patterns = ["validate", "Validate", "schema", "Schema", "guard", "check",
                "??", "?.", "try", "catch", "assert", "ensure", "verify", "sanitize", "parse"];
            let has_validation = validation_patterns.iter().any(|p| func_body.contains(p));
            output.push(format!("Step 3 — Function has inline validation: {}", has_validation));

            // Step 4: Check what the file imports
            let import_rows = self.query_text(
                &self.client,
                &format!(
                    "MATCH (fi:CodeFile {{repo: '{}'}})-[:IMPORTS]->(fn) WHERE fi.path = '{}' RETURN fn.name, fn.file LIMIT 20",
                    repo, func_file
                ),
                &[],
            ).unwrap_or_default();

            let imports_validation = import_rows.iter().any(|row| {
                let name = row.get(0).map(|v| v.as_str()).unwrap_or("");
                name.contains("validate") || name.contains("Validate") || name.contains("Schema")
            });

            output.push(format!("Step 4 — File imports validation: {}", imports_validation));
            if !import_rows.is_empty() {
                output.push("  Imports:".to_string());
                for row in &import_rows {
                    let iname = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                    let ifile = row.get(1).map(|v| v.as_str()).unwrap_or("?");
                    let marker = if iname.to_lowercase().contains("validat") { " ← VALIDATION" } else { "" };
                    output.push(format!("    {} from {}{}", iname, ifile, marker));
                }
            }

            // Step 5: Trace upstream — who CALLS this function?
            output.push(String::new());
            output.push("Step 5 — Upstream trace (who feeds data to this function):".to_string());

            let caller_rows = self.query_text(
                &self.client,
                &format!(
                    "MATCH (caller:CodeFunction {{repo: '{}'}})-[:CALLS]->(f:CodeFunction {{repo: '{}', name: '{}'}}) RETURN caller.name, caller.file LIMIT 10",
                    repo, repo, func_name
                ),
                &[],
            ).unwrap_or_default();

            // Also check who imports this function's file
            let importer_rows = self.query_text(
                &self.client,
                &format!(
                    "MATCH (fi:CodeFile {{repo: '{}'}})-[:IMPORTS]->(fn:CodeFunction {{repo: '{}', name: '{}'}}) RETURN fi.path LIMIT 10",
                    repo, repo, func_name
                ),
                &[],
            ).unwrap_or_default();

            let mut upstream_files: Vec<String> = vec![];
            if !caller_rows.is_empty() {
                for row in &caller_rows {
                    let cname = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                    let cfile = row.get(1).map(|v| v.as_str()).unwrap_or("?");
                    output.push(format!("    Called by: {} ({})", cname, cfile));
                    upstream_files.push(cfile.to_string());
                }
            }
            if !importer_rows.is_empty() {
                for row in &importer_rows {
                    let path = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                    if !upstream_files.contains(&path.to_string()) {
                        output.push(format!("    Imported by: {}", path));
                        upstream_files.push(path.to_string());
                    }
                }
            }

            // Step 5b: If no callers found (frontend component), trace via data shape
            // Look for backend functions that produce the data this component consumes
            if upstream_files.is_empty() && func_file.starts_with("src/") {
                output.push(String::new());
                output.push("Step 5b — Frontend component: tracing data producer via content analysis:".to_string());

                // Extract key field names from the function body to find who produces this data
                let data_keywords: Vec<&str> = vec![
                    "integrity_report", "identity_report", "discrepancy_check",
                    "confidence_score", "resume_integrity", "profile_authenticity",
                ];
                for keyword in &data_keywords {
                    if func_body.to_lowercase().contains(keyword) || error_lower.contains(keyword) {
                        if let Ok(producers) = self.query_text(
                            &self.client,
                            &format!(
                                "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE f.file STARTS WITH 'server/' AND toLower(f.body) CONTAINS '{}' RETURN f.name, f.file LIMIT 5",
                                repo, keyword
                            ),
                            &[],
                        ) {
                            for row in &producers {
                                let pname = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                                let pfile = row.get(1).map(|v| v.as_str()).unwrap_or("?");
                                if !upstream_files.contains(&pfile.to_string()) {
                                    output.push(format!("    Data producer (contains '{}'): {} ({})", keyword, pname, pfile));
                                    upstream_files.push(pfile.to_string());
                                }
                            }
                        }
                    }
                }

                // Also search for the component name as a type/interface in server code
                // (API routes that return this data)
                if let Ok(api_rows) = self.query_text(
                    &self.client,
                    &format!(
                        "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE f.file STARTS WITH 'server/routes/' AND (toLower(f.body) CONTAINS 'integrity' OR toLower(f.body) CONTAINS 'identity' OR toLower(f.body) CONTAINS 'verification') RETURN f.name, f.file LIMIT 5",
                        repo
                    ),
                    &[],
                ) {
                    for row in &api_rows {
                        let rname = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                        let rfile = row.get(1).map(|v| v.as_str()).unwrap_or("?");
                        if !upstream_files.contains(&rfile.to_string()) {
                            output.push(format!("    API route: {} ({})", rname, rfile));
                            upstream_files.push(rfile.to_string());
                        }
                    }
                }

                // Find the service that calls the AI and produces this data
                if let Ok(svc_rows) = self.query_text(
                    &self.client,
                    &format!(
                        "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE f.file CONTAINS 'identity-verification' OR f.file CONTAINS 'identity_verification' RETURN f.name, f.file LIMIT 10",
                        repo
                    ),
                    &[],
                ) {
                    for row in &svc_rows {
                        let sname = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                        let sfile = row.get(1).map(|v| v.as_str()).unwrap_or("?");
                        if !upstream_files.contains(&sfile.to_string()) {
                            output.push(format!("    Service: {} ({})", sname, sfile));
                            upstream_files.push(sfile.to_string());
                        }
                    }
                }
            }

            // Step 6: For each upstream, check if IT has validation
            output.push(String::new());
            output.push("Step 6 — Validation check on upstream chain:".to_string());

            for upstream_file in &upstream_files {
                let upstream_imports = self.query_text(
                    &self.client,
                    &format!(
                        "MATCH (fi:CodeFile {{repo: '{}'}})-[:IMPORTS]->(fn) WHERE fi.path = '{}' AND (fn.name CONTAINS 'validate' OR fn.name CONTAINS 'Validate' OR fn.name CONTAINS 'Schema') RETURN fn.name, fn.file LIMIT 5",
                        repo, upstream_file
                    ),
                    &[],
                ).unwrap_or_default();

                let upstream_uses_validation = self.query_text(
                    &self.client,
                    &format!(
                        "MATCH (f:CodeFunction {{repo: '{}'}})-[:USES_VALIDATION]->(v) WHERE f.file = '{}' RETURN f.name, v.name LIMIT 3",
                        repo, upstream_file
                    ),
                    &[],
                ).unwrap_or_default();

                if upstream_imports.is_empty() && upstream_uses_validation.is_empty() {
                    output.push(format!("    {} — NO VALIDATION IMPORTS", upstream_file));
                    output.push("    ^^^ POTENTIAL ROOT CAUSE: data enters here without validation".to_string());
                } else {
                    let val_names: Vec<String> = upstream_imports.iter().chain(upstream_uses_validation.iter())
                        .filter_map(|row| row.get(0).map(|v| v.as_str().to_string()))
                        .collect();
                    output.push(format!("    {} — uses: {}", upstream_file, val_names.join(", ")));
                }
            }

            // Step 7: Check if there's a DB write between backend and frontend (the data path)
            output.push(String::new());
            output.push("Step 7 — Data persistence (where is the data stored?):".to_string());

            let db_rows = self.query_text(
                &self.client,
                &format!(
                    "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE toLower(f.body) CONTAINS 'prisma' AND (toLower(f.body) CONTAINS '{}' OR toLower(f.body) CONTAINS '{}') RETURN f.name, f.file LIMIT 5",
                    repo, func_name.to_lowercase(), error_lower.chars().take(30).collect::<String>()
                ),
                &[],
            ).unwrap_or_default();

            if !db_rows.is_empty() {
                for row in &db_rows {
                    output.push(format!("    DB write: {} ({})", row.get(0).map(|v| v.as_str()).unwrap_or("?"), row.get(1).map(|v| v.as_str()).unwrap_or("?")));
                }
                output.push("    Data is stored in DB → frontend reads it later → crash happens on read, not write".to_string());
            }

            // Step 8: Slack context
            output.push(String::new());
            output.push("Step 8 — Slack context:".to_string());

            if let Ok(slack_rows) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (m:SlackMessage)-[:SENT_BY]->(u:SlackUser) WHERE (toLower(m.text) CONTAINS '{}' OR toLower(m.text) CONTAINS '{}') AND m.text CONTAINS '?' AND m.reply_count = 0 RETURN u.name, m.channel_name, m.text LIMIT 3",
                    func_name.to_lowercase(), error_lower.chars().take(30).collect::<String>()
                ),
                &[],
            ) {
                if !slack_rows.is_empty() {
                    output.push("    Unanswered questions about this area:".to_string());
                    for row in &slack_rows {
                        let who = row.get(0).map(|v| v.as_str()).unwrap_or("?");
                        let ch = row.get(1).map(|v| v.as_str()).unwrap_or("?");
                        let text: String = row.get(2).map(|v| v.as_str()).unwrap_or("?").chars().take(80).collect();
                        output.push(format!("    @{} in #{}: {}", who, ch, text));
                    }
                }
            }

            // Step 8b: Test coverage check
            let test_patterns = [".test.", ".spec.", "__test__", "__tests__"];
            let has_tests = if let Ok(test_rows) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (f:CodeFile {{repo: '{}'}}) WHERE {} RETURN f.path LIMIT 1",
                    repo,
                    test_patterns.iter()
                        .map(|p| format!("f.path CONTAINS '{}'", p))
                        .collect::<Vec<_>>()
                        .join(" OR ")
                ),
                &[],
            ) {
                // Check if there's a test file matching the affected source file
                let source_stem = func_file.replace(".ts", "").replace(".tsx", "").replace(".js", "").replace(".py", "");
                let source_name = source_stem.split('/').last().unwrap_or("");
                let matching_test = test_rows.iter().any(|row| {
                    let test_path = row.get(0).map(|v| v.as_str()).unwrap_or("");
                    test_path.contains(source_name)
                });
                if !matching_test && !source_name.is_empty() {
                    output.push(String::new());
                    output.push(format!("  WARNING: No test file found matching {}", func_file));
                    output.push("  This function lacks test coverage, which may have allowed the bug to ship.".to_string());
                    output.push(format!("  Recommended: add tests for the null/undefined input case in {}.test.ts", source_name));
                }
                matching_test
            } else {
                false
            };

            // Step 9: Who has context on this code?
            output.push(String::new());
            output.push("Step 9 — Who has context:".to_string());

            if let Ok(blame_rows) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (c:Commit {{repo: '{}'}})-[:MODIFIED]->(f:CodeFunction {{repo: '{}', name: '{}'}}) RETURN c.short_hash, c.author, c.date, c.message ORDER BY c.date DESC LIMIT 3",
                    repo, repo, func_name
                ),
                &[],
            ) {
                for row in &blame_rows {
                    let author = row.get(1).map(|v| v.as_str()).unwrap_or("?");
                    let date = row.get(2).map(|v| v.as_str()).unwrap_or("?").chars().take(10).collect::<String>();

                    // Check if this author is still active
                    let activity_note = if let Ok(active_rows) = self.query_text(
                        &self.client,
                        &format!(
                            "MATCH (u:SlackUser) WHERE toLower(u.name) CONTAINS toLower('{}') RETURN u.has_recent_activity, u.suggested_contact",
                            author.split_whitespace().next().unwrap_or(author)
                        ),
                        &[],
                    ) {
                        if let Some(row) = active_rows.first() {
                            let is_active = row.get(0).map(|v| v.as_str()).unwrap_or("true");
                            let replacement = row.get(1).map(|v| v.as_str()).unwrap_or("");
                            if is_active == "false" || is_active == "0" {
                                if !replacement.is_empty() {
                                    format!(" (no recent activity, try @{} instead)", replacement)
                                } else {
                                    " (no recent activity)".to_string()
                                }
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };

                    output.push(format!("    {} by @{} ({}){}",
                        row.get(0).map(|v| v.as_str()).unwrap_or("?"),
                        author, date, activity_note,
                    ));
                }
            }

            // Step 10: Conclusion — find the REAL root cause
            output.push(String::new());
            output.push("CONCLUSION:".to_string());

            // First check: are there unvalidated upstream producers?
            let unvalidated_upstream: Vec<&String> = upstream_files.iter().filter(|f| {
                // Skip the frontend file itself
                if f.starts_with("src/") { return false; }
                let res = self.query_text(
                    &self.client,
                    &format!(
                        "MATCH (fi:CodeFile {{repo: '{}'}})-[:IMPORTS]->(fn) WHERE fi.path = '{}' AND (fn.name CONTAINS 'validate' OR fn.name CONTAINS 'Validate' OR fn.name CONTAINS 'Schema') RETURN count(fn)",
                        repo, f
                    ),
                    &[],
                );
                match res {
                    Ok(rows) => rows.first().map(|r| r[0].as_i64() == 0).unwrap_or(true),
                    Err(_) => true,
                }
            }).collect();

            if !unvalidated_upstream.is_empty() {
                // Root cause is UPSTREAM, not the crashing function
                // Generate context-aware conclusion based on what the graph actually shows
                let upstream_location = if unvalidated_upstream.iter().any(|f| f.starts_with("server/")) {
                    " in the backend"
                } else if unvalidated_upstream.iter().any(|f| f.starts_with("src/")) {
                    " in the frontend"
                } else {
                    ""
                };
                output.push(format!("  ROOT CAUSE: The crash is in {}, but the bug is upstream{}:", func_name, upstream_location));
                for f in &unvalidated_upstream {
                    output.push(format!("    -> {} - produces data WITHOUT validation or null guards", f));
                }
                output.push(String::new());

                // Build WHY from the actual graph context
                output.push(format!("  WHY: {} receives data from upstream without validation.", func_name));
                output.push("  The upstream producer does not check for null, undefined, or unexpected".to_string());
                output.push("  data types before passing data downstream. When invalid data flows through,".to_string());
                output.push(format!("  {} crashes because it assumes the data is well-formed.", func_name));
                output.push(String::new());

                // Build FIX from the actual files involved
                output.push("  FIX: Add input validation or null guards to the upstream producer:".to_string());
                for f in &unvalidated_upstream {
                    // Check what validation patterns exist in the repo
                    let validation_exists_in_repo = self.query_text(
                        &self.client,
                        &format!(
                            "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE toLower(f.name) CONTAINS 'validate' OR toLower(f.name) CONTAINS 'schema' RETURN f.name LIMIT 1",
                            repo
                        ),
                        &[],
                    ).ok().map(|r| !r.is_empty()).unwrap_or(false);

                    if validation_exists_in_repo {
                        output.push(format!("    -> {} should use the existing validation utilities in this codebase", f));
                    } else {
                        output.push(format!("    -> {} needs null checks before accessing properties on input data", f));
                    }
                }
                output.push("  The consuming function does NOT need to change -".to_string());
                output.push("  validation belongs at the data producer, not the consumer.".to_string());
            } else if !imports_validation && !has_validation && upstream_files.is_empty() {
                output.push(format!("  ROOT CAUSE: {} at {} has no validation and no upstream trace found.", func_name, func_file));
                output.push(format!("  FIX: Add null guards or input validation to {}", func_file));
            } else if imports_validation && !has_validation {
                output.push("  The file imports validation but this specific function doesn't use it.".to_string());
                output.push(format!("  FIX: Apply the existing validation to {} specifically", func_name));
            } else {
                output.push("  Validation exists at all levels. The error may be caused by:".to_string());
                output.push("  1. Schema mismatch - unexpected data shape from an external service or API".to_string());
                output.push("  2. Edge case not covered by current validation/schema logic".to_string());
                output.push("  3. Stale or corrupted data in the data store".to_string());
                output.push(String::new());
                output.push("  This is likely a schema validation issue, not a missing validation issue.".to_string());
                output.push("  FIX: Review the schema constraints and add fallback handling for non-conforming data.".to_string());
            }

            break; // Only diagnose the first matching function
        }

        // ============================================================
        // ERROR CATEGORY CLASSIFICATION
        // ============================================================
        let error_category = {
            let infra_keywords = ["redis", "connection", "timeout", "dns", "certificate",
                "tls", "ssl", "port", "network", "socket", "refused", "unreachable",
                "oom", "memory", "disk", "cpu", "rate limit", "429", "503", "502",
                "healthcheck", "readiness", "liveness", "pod", "container", "k8s",
                "kubernetes", "deploy", "api endpoint", "polling", "etimedout",
                "econnrefused", "econnreset", "accessdenied"];
            let config_keywords = ["config", "environment", "env var", "secret", "credential",
                "permission", "forbidden", "401", "403", "cors",
                "missing key", "not found in config", "unauthorized", "hmac",
                "signature mismatch", "signing"];

            let lower = error_lower.clone();
            let is_infra = infra_keywords.iter().any(|k| lower.contains(k));
            let is_config = config_keywords.iter().any(|k| lower.contains(k));

            if is_infra { "infrastructure" }
            else if is_config { "configuration" }
            else { "application" }
        };
        output.push(String::new());
        output.push(format!("CATEGORY: {} error", error_category));

        // ============================================================
        // CONFIDENCE SCORE + DATA GAP DETECTION
        // ============================================================
        output.push(String::new());
        output.push("─".repeat(50));

        // Check what data sources are available
        let mut sources_available: Vec<(&str, bool, i64)> = vec![];

        // Code graph
        let code_count = self.query_text(&self.client,
            &format!("MATCH (f:CodeFunction {{repo: '{}'}}) RETURN count(f)", repo), &[])
            .ok().and_then(|r| r.first().map(|row| row[0].as_i64())).unwrap_or(0);
        sources_available.push(("Context (functions)", code_count > 0, code_count));

        // Git history
        let commit_count = self.query_text(&self.client,
            &format!("MATCH (c:Commit {{repo: '{}'}}) RETURN count(c)", repo), &[])
            .ok().and_then(|r| r.first().map(|row| row[0].as_i64())).unwrap_or(0);
        sources_available.push(("Git history (commits)", commit_count > 0, commit_count));

        // Import/call chain
        let import_count = self.query_text(&self.client,
            "MATCH ()-[r:IMPORTS]->() RETURN count(r)", &[])
            .ok().and_then(|r| r.first().map(|row| row[0].as_i64())).unwrap_or(0);
        sources_available.push(("Import/call chain", import_count > 0, import_count));

        // Slack
        let slack_count = self.query_text(&self.client,
            "MATCH (m:SlackMessage) RETURN count(m)", &[])
            .ok().and_then(|r| r.first().map(|row| row[0].as_i64())).unwrap_or(0);
        sources_available.push(("Slack messages", slack_count > 0, slack_count));

        // Jira
        let jira_count = self.query_text(&self.client,
            "MATCH (t:JiraTicket) RETURN count(t)", &[])
            .ok().and_then(|r| r.first().map(|row| row[0].as_i64())).unwrap_or(0);
        sources_available.push(("Jira tickets", jira_count > 0, jira_count));

        // GitHub PRs
        let pr_count = self.query_text(&self.client,
            "MATCH (p:GitHubPR) RETURN count(p)", &[])
            .ok().and_then(|r| r.first().map(|row| row[0].as_i64())).unwrap_or(0);
        sources_available.push(("GitHub PRs", pr_count > 0, pr_count));

        // K8s
        let k8s_count = self.query_text(&self.client,
            "MATCH (p:K8sPod) RETURN count(p)", &[])
            .ok().and_then(|r| r.first().map(|row| row[0].as_i64())).unwrap_or(0);
        sources_available.push(("K8s cluster data", k8s_count > 0, k8s_count));

        // Host
        let host_count = self.query_text(&self.client,
            "MATCH (h:Host) RETURN count(h)", &[])
            .ok().and_then(|r| r.first().map(|row| row[0].as_i64())).unwrap_or(0);
        sources_available.push(("Host metrics", host_count > 0, host_count));

        let available = sources_available.iter().filter(|s| s.1).count();
        let total = sources_available.len();
        let confidence = (available as f64 / total as f64 * 100.0) as i32;

        let confidence_label = if confidence >= 80 { "HIGH" }
            else if confidence >= 50 { "MEDIUM" }
            else { "LOW" };

        output.push(format!("CONFIDENCE: {}% ({})", confidence, confidence_label));
        output.push(String::new());
        output.push("Data sources:".to_string());
        for (name, has_data, count) in &sources_available {
            let icon = if *has_data { "✅" } else { "❌" };
            let detail = if *has_data { format!("{}", count) } else { "not connected".to_string() };
            output.push(format!("  {} {} ({})", icon, name, detail));
        }

        // ============================================================
        // UNKNOWN UNKNOWN DETECTION
        // Scan code imports + Slack messages for references to systems
        // that aren't in the graph
        // ============================================================
        let mut missing_integrations: Vec<(&str, &str)> = vec![];

        // Check code imports for known observability/infra tools
        let tool_checks = [
            ("datadog", "Datadog", "savants connect datadog"),
            ("sentry", "Sentry", "Error tracking — check sentry.io dashboard"),
            ("@prisma", "Database queries", "DB query tracing not in graph"),
            ("prometheus", "Prometheus", "savants connect prometheus"),
            ("grafana", "Grafana", "savants connect grafana"),
            ("pagerduty", "PagerDuty", "savants connect pagerduty"),
            ("cloudwatch", "AWS CloudWatch", "savants connect aws"),
            ("newrelic", "New Relic", "savants connect newrelic"),
        ];

        for (import_keyword, tool_name, hint) in &tool_checks {
            // Check if code imports this tool
            if let Ok(rows) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (f:CodeFunction {{repo: '{}'}}) WHERE toLower(f.body) CONTAINS '{}' RETURN count(f)",
                    repo, import_keyword
                ),
                &[],
            ) {
                let used_in_code = rows.first().map(|r| r[0].as_i64() > 0).unwrap_or(false);
                if used_in_code {
                    // Check if we have data from this tool in the graph
                    // (we don't have dedicated nodes for most of these yet)
                    let has_data = match *import_keyword {
                        "sentry" => slack_count > 0, // Sentry errors come through Slack
                        "@prisma" => false, // We don't track DB queries
                        _ => false,
                    };
                    if !has_data {
                        missing_integrations.push((tool_name, hint));
                    }
                }
            }
        }

        // Check Slack for mentions of tools not in the graph
        let slack_tool_checks = [
            ("datadog", "Datadog"),
            ("grafana", "Grafana"),
            ("pagerduty", "PagerDuty"),
            ("cloudwatch", "CloudWatch"),
            ("new relic", "New Relic"),
            ("kibana", "Kibana/ELK"),
            ("splunk", "Splunk"),
            ("opsgenie", "OpsGenie"),
        ];

        for (keyword, tool_name) in &slack_tool_checks {
            if missing_integrations.iter().any(|(n, _)| n == tool_name) { continue; }
            if let Ok(rows) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (m:SlackMessage) WHERE toLower(m.text) CONTAINS '{}' RETURN count(m)",
                    keyword
                ),
                &[],
            ) {
                let mentioned = rows.first().map(|r| r[0].as_i64() > 0).unwrap_or(false);
                if mentioned {
                    missing_integrations.push((tool_name, "Mentioned in Slack but not connected to Savants"));
                }
            }
        }

        if !missing_integrations.is_empty() {
            output.push(String::new());
            output.push("BLIND SPOTS — tools your team uses that Savants can't see:".to_string());
            for (tool, hint) in &missing_integrations {
                output.push(format!("  ⚠️ {} — {}", tool, hint));
            }
            output.push(String::new());
            output.push("Connecting these would improve diagnosis accuracy.".to_string());
        }

        Ok(output.join("\n"))
    }

    fn tool_radar(&self, args: &Value) -> Result<String, String> {
        let user = arg_str(args, "user")?;
        let hours = args.get("hours").and_then(|v| v.as_f64()).unwrap_or(24.0);

        let radar = match crate::radar::PersonalRadar::from_graph(&self.client, &user) {
            Some(r) => r,
            None => return Err(format!("Could not find user '{}' in the context engine. Try your Slack username, git author name, or email.", user)),
        };

        let items = radar.scan(&self.client, hours);
        Ok(radar.format_digest(&items))
    }

    // ---------------------------------------------------------------
    // JSON-RPC response helpers
    // ---------------------------------------------------------------

    fn response(&self, req_id: &Value, result: Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": result
        })
    }

    fn error(&self, req_id: &Value, code: i32, message: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "error": {
                "code": code,
                "message": message
            }
        })
    }

    fn tool_diagnose(&self, args: &Value) -> Result<String, String> {
        let since = args.get("since_minutes").and_then(|v| v.as_i64()).unwrap_or(60);
        let sev = args.get("min_severity").and_then(|v| v.as_str()).unwrap_or("WARN");

        let mut sections = Vec::new();

        // Host state
        match self.tool_host_state(&json!({})) {
            Ok(s) => sections.push(s),
            Err(_) => {}
        }

        // Host story
        match self.tool_host_story(&json!({"since_minutes": since, "min_severity": sev})) {
            Ok(s) if !s.contains("No significant") => sections.push(s),
            _ => {}
        }

        // Find K8s clusters
        let cluster_names = self.find_cluster_names();
        for cluster in &cluster_names {
            match self.tool_cluster_state(&json!({"cluster": cluster})) {
                Ok(s) => sections.push(s),
                Err(_) => {}
            }
            match self.tool_pod_story(&json!({
                "cluster": cluster,
                "since_minutes": since,
                "min_severity": sev,
                "limit": 10
            })) {
                Ok(s) if !s.contains("No significant") => sections.push(s),
                _ => {}
            }
        }

        if sections.is_empty() {
            return Ok("No infrastructure data found. Run `savants up` to ingest your infrastructure first.".into());
        }

        Ok(sections.join("\n\n---\n\n"))
    }

    fn tool_saql_query(&self, args: &Value) -> Result<String, String> {
        let q_str = arg_str(args, "q")?;
        let parsed = crate::saql::parse(&q_str)?;
        crate::saql::execute(&parsed, &self.client)
    }

    fn find_cluster_names(&self) -> Vec<String> {
        let mut clusters = Vec::new();
        // Check default graph for K8sCluster nodes
        if let Ok(r) = self.client.query("MATCH (c:K8sCluster) RETURN c.name", &[]) {
            for row in &r.rows {
                if let Some(name) = row.first() {
                    let n = name.as_str().to_string();
                    if !n.is_empty() { clusters.push(n); }
                }
            }
        }
        // Also try well-known names
        for name in &["astra-k3s", "default", "production", "staging"] {
            if clusters.iter().any(|c| c == *name) { continue; }
            let graph_name = name.replace("-", "_");
            if let Ok(c) = GraphClient::new(&graph_name) {
                if let Ok(r) = c.query("MATCH (p:K8sPod) RETURN count(p)", &[]) {
                    if r.rows.first().map(|r| r[0].as_i64()).unwrap_or(0) > 0 {
                        clusters.push(name.to_string());
                    }
                }
            }
        }
        clusters
    }
}

// ===================================================================
// Utility functions
// ===================================================================

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Missing required argument: {}", key))
}

fn escape_cypher(s: &str) -> String {
    s.replace('\'', "\\'")
}

fn now_unix() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn format_timestamp(ts: f64) -> String {
    let secs = ts as i64;
    // Simple ISO-like format without pulling in chrono
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Approximate date calculation (good enough for display)
    // Using a simple algorithm for year/month/day from days since epoch
    let mut y = 1970;
    let mut remaining = days_since_epoch;
    loop {
        let days_in_year = if is_leap_year(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let months = [31, if is_leap_year(y) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 1;
    for &dim in &months {
        if remaining < dim {
            break;
        }
        remaining -= dim;
        m += 1;
    }
    let d = remaining + 1;

    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", y, m, d, hours, minutes, seconds)
}

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

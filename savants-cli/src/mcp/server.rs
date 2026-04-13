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

pub struct McpServer {
    client: GraphClient,
}

impl McpServer {
    pub fn new(graph_name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let client = GraphClient::new(graph_name)?;
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
                "description": "Identify the most connected hub files in the codebase.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
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
                "description": "Rebuild the graph for a repository. Stub -- needs tree-sitter integration in Rust.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo_path": {"type": "string", "description": "Absolute path to the repository to index"},
                        "full": {"type": "boolean", "default": true, "description": "Drop and rebuild the entire graph"}
                    },
                    "required": ["repo_path"]
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

        let result = match tool_name {
            "diagnose" => self.tool_diagnose(&args),
            "graph_stats" => self.tool_graph_stats(),
            "cluster_state" => self.tool_cluster_state(&args),
            "list_pods" => self.tool_list_pods(&args),
            "pod_story" => self.tool_pod_story(&args),
            "host_state" => self.tool_host_state(&args),
            "host_story" => self.tool_host_story(&args),
            "deployment_info" => self.tool_deployment_info(&args),
            "pod_dependencies" => self.tool_pod_dependencies(&args),
            "namespace_summary" => self.tool_namespace_summary(&args),
            "search_code" => self.tool_search_code(&args),
            "find_references_structured" => self.tool_find_references(&args),
            "function_xray" => self.tool_function_xray(&args),
            "impact_analysis" => self.tool_impact_analysis(&args),
            "diff_impact" => self.tool_diff_impact(&args),
            "risk_score" => self.tool_risk_score(&args),
            "decorated_with" => self.tool_decorated_with(&args),
            "resolves_to" => self.tool_resolves_to(&args),
            "community_summary" => self.tool_community_summary(&args),
            "dependency_chain" => self.tool_dependency_chain(&args),
            "co_change_partners" => self.tool_co_change_partners(&args),
            "recall_history" => self.tool_recall_history(&args),
            "federated_symbol_in_cluster" => self.tool_federated_symbol_in_cluster(&args),
            "pre_change_warning" => self.tool_pre_change_warning(&args),
            "coupling_check" => self.tool_coupling_check(&args),
            "query" => self.tool_saql_query(&args),
            "advanced_graph_query" => self.tool_advanced_graph_query(&args),  // hidden, not in tool list
            "reindex" => self.tool_reindex(&args),
            "pr_risk" => self.tool_pr_risk(&args),
            _ => Err(format!("Unknown tool: {}", tool_name)),
        };

        match result {
            Ok(text) => self.response(req_id, json!({
                "content": [{"type": "text", "text": text}]
            })),
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
                "Pod {}/{} has no ConfigMap or Secret dependencies (or doesn't exist in graph).",
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

    fn tool_search_code(&self, args: &Value) -> Result<String, String> {
        let pattern = arg_str(args, "pattern")?;

        // Search both old-style (Function/Class) and new tree-sitter (CodeFunction/CodeClass) nodes
        // Also search function bodies for the pattern
        let mut results = Vec::new();

        // Search by name
        if let Ok(rows) = self.query_text(
            &self.client,
            "MATCH (n) WHERE (n:Function OR n:Class OR n:CodeFunction OR n:CodeClass) AND toLower(n.name) CONTAINS toLower($pattern) \
             RETURN labels(n)[0], n.name, n.file_path, n.file, n.line, n.repo LIMIT 50",
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
            "AND NOT caller.file_path STARTS WITH 'tests/'"
        };

        let query = format!(
            "MATCH (caller:Function)-[:CALLS]->(target:Function {{name: $name}}) \
             WHERE 1=1 {} \
             RETURN caller.name, caller.file_path \
             ORDER BY caller.file_path LIMIT 50",
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
                "MATCH (fn:Function {name: $name, file_path: $fp}) \
                 RETURN fn.name, fn.file_path, fn.start_line, fn.end_line, fn.parameters",
                &[("name", &function_name), ("fp", fp)],
            )?
        } else {
            self.query_text(
                &self.client,
                "MATCH (fn:Function {name: $name}) \
                 RETURN fn.name, fn.file_path, fn.start_line, fn.end_line, fn.parameters \
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
                "MATCH (c:Function)-[:CALLS]->(t:Function {name: $name, file_path: $fp}) \
                 RETURN c.name, c.file_path LIMIT 25",
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
                "MATCH (t:Function {name: $name, file_path: $fp})-[:CALLS]->(c:Function) \
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
                "MATCH (e:Episode)-[:CHANGES]->(:Function {name: $name, file_path: $fp}) \
                 RETURN e.timestamp, e.author, e.message \
                 ORDER BY e.timestamp DESC LIMIT 5",
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
            "MATCH (c:Function)-[:CALLS]->(t:Function {name: $name}) \
             RETURN DISTINCT c.name, c.file_path",
            &[("name", &function_name)],
        )?;

        // Transitive dependents
        let query = format!(
            "MATCH (c:Function)-[:CALLS*1..{}]->(t:Function {{name: $name}}) \
             RETURN DISTINCT c.name, c.file_path",
            max_depth,
        );
        let transitive = self.query_text(&self.client, &query, &[("name", &function_name)])?;

        // Affected files
        let aff_query = format!(
            "MATCH (c:Function)-[:CALLS*1..{}]->(t:Function {{name: $name}}) \
             RETURN DISTINCT c.file_path",
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
            "MATCH (c:Function)-[:CALLS*1..3]->(t:Function {name: $name}) \
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
            "MATCH (e:Episode)-[:CHANGES]->(:Function {name: $name}) \
             RETURN e.author, count(e) AS t ORDER BY t DESC",
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
            "MATCH (e:Episode)-[:CHANGES]->(:Function {name: $name}) \
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
            "MATCH (f:Function)-[:DECORATED_BY]->(d:Decorator) \
             WHERE d.name = $needle OR d.name ENDS WITH $dot_needle \
             RETURN DISTINCT 'Function' AS kind, f.name AS name, f.file_path AS fp, d.name AS dec \
             ORDER BY fp, name",
            &[("needle", needle), ("dot_needle", &dot_needle)],
        )?;

        let cls_rows = self.query_text(
            &self.client,
            "MATCH (c:Class)-[:DECORATED_BY]->(d:Decorator) \
             WHERE d.name = $needle OR d.name ENDS WITH $dot_needle \
             RETURN DISTINCT 'Class' AS kind, c.name AS name, c.file_path AS fp, d.name AS dec \
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
            "MATCH (n) WHERE (n:Function OR n:Class) AND n.name = $t \
             RETURN labels(n)[0], n.name, n.file_path LIMIT 20",
            &[("t", terminal)],
        )?;

        let refs = self.query_text(
            &self.client,
            "MATCH (c:Function)-[:REFERENCES_SYMBOL]->(t) WHERE t.name = $t \
             RETURN DISTINCT c.name, c.file_path LIMIT 30",
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
        let max_results = args.get("max_results").and_then(|v| v.as_i64()).unwrap_or(10);

        let query = format!(
            "MATCH (f:Function)-[r:CALLS]->() \
             RETURN f.file_path, count(r) AS edges \
             ORDER BY edges DESC LIMIT {}",
            max_results,
        );
        let rows = self.query_text(&self.client, &query, &[])?;

        if rows.is_empty() {
            return Ok("No call edges found in the graph.".to_string());
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
            "MATCH (a:Function)-[:CALLS*1..6]->(b:Function) \
             WHERE a.file_path = $from AND b.file_path = $to \
             RETURN DISTINCT a.file_path, b.file_path \
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
            "MATCH (e:Episode)-[:CHANGES]->(fn1:Function {{name: $name}}) \
             MATCH (e)-[:CHANGES]->(fn2:Function) \
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
            "MATCH (e:Episode) WHERE e.content CONTAINS $q \
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
            "MATCH (n) WHERE (n:Function OR n:Class) AND n.name = $symbol \
             RETURN labels(n)[0], n.name, n.file_path LIMIT 10",
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
            lines.push(format!("Code graph ({} matches):", code_hits.len()));
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
            lines.push(format!("Cluster graph: no references to '{}' found.", symbol));
        }

        Ok(lines.join("\n"))
    }

    fn tool_pre_change_warning(&self, args: &Value) -> Result<String, String> {
        let function_name = arg_str(args, "function_name")?;

        // Blast radius
        let callers = self.query_text(
            &self.client,
            "MATCH (c:Function)-[:CALLS]->(t:Function {name: $name}) RETURN count(c)",
            &[("name", &function_name)],
        )?;
        let direct = callers.first().and_then(|r| r.first()).map(|v| v.as_i64()).unwrap_or(0);

        let transitive = self.query_text(
            &self.client,
            "MATCH (c:Function)-[:CALLS*1..3]->(t:Function {name: $name}) \
             RETURN count(DISTINCT c)",
            &[("name", &function_name)],
        )?;
        let trans = transitive.first().and_then(|r| r.first()).map(|v| v.as_i64()).unwrap_or(0);

        // Last touched
        let last_touch = self.query_text(
            &self.client,
            "MATCH (e:Episode)-[:CHANGES]->(:Function {name: $name}) \
             RETURN e.timestamp, e.author ORDER BY e.timestamp DESC LIMIT 1",
            &[("name", &function_name)],
        ).unwrap_or_default();

        // Maintainer concentration
        let maintainers = self.query_text(
            &self.client,
            "MATCH (e:Episode)-[:CHANGES]->(:Function {name: $name}) \
             RETURN e.author, count(e) AS touches ORDER BY touches DESC LIMIT 3",
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
            "MATCH (a:Function)-[:CALLS]->(b:Function) \
             WHERE a.file_path STARTS WITH $from_mod \
               AND b.file_path STARTS WITH $to_mod \
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

        // Derive repo name from path
        let repo_name = std::path::Path::new(repo_path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let indexer = crate::code_index::CodeIndexer::new(self.client.clone(), &repo_name);
        let stats = indexer.index_repo(repo_path);

        // Also analyze open PRs if they exist in the graph
        let pr_analyzer = crate::code_index::PRAnalyzer::new(self.client.clone(), &repo_name);
        let prs_analyzed = pr_analyzer.analyze_open_prs(repo_path);

        Ok(format!("Indexed {}: {}. Analyzed {} open PRs.", repo_name, stats.summary(), prs_analyzed))
    }

    fn tool_pr_risk(&self, args: &Value) -> Result<String, String> {
        let repo = args.get("repo").and_then(|v| v.as_str()).unwrap_or("talent-pipeline");

        // Query all analyzed PRs with risk info
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

            let emoji = match risk {
                "HIGH" => "🔴",
                "MEDIUM" => "🟡",
                _ => "🟢",
            };

            output.push(format!("{} PR #{} — {} ({})", emoji, num, title, risk));
            output.push(format!("  Author: @{} | Branch: {} | Files: {}", author, branch, files));
            if !details.is_empty() {
                output.push(format!("  Risk: {}", details));
            }

            // Show affected functions with known issues
            if let Ok(affected) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}})-[:CHANGES]->(ch:PRChange)-[:AFFECTS]->(f:CodeFunction) \
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

            // Show known Slack issues for affected functions
            if let Ok(issues) = self.query_text(
                &self.client,
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}})-[:CHANGES]->(ch:PRChange)-[:HAS_KNOWN_ISSUE]->(m:SlackMessage) \
                     RETURN m.channel_name, m.text LIMIT 3",
                    num, repo
                ),
                &[],
            ) {
                if !issues.is_empty() {
                    output.push("  ⚠️ Known issues in Slack for functions this PR touches:".to_string());
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

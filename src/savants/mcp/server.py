"""MCP Server: exposes structural graph queries to external AI tools.

Implements the Model Context Protocol stdio transport using
newline-delimited JSON-RPC, per the MCP specification. One JSON
message per line, no framing headers. This is the transport Claude
Code, Cursor, and Continue actually use — not the LSP-style
Content-Length framing that an earlier version of this server (and
this docstring) used incorrectly.

Usage:
    claude mcp add-json savants --scope user '{
      "command": "python",
      "args": ["-m", "savants.mcp"],
      "env": {"FALKORDB_HOST": "localhost", "FALKORDB_PORT": "6379"}
    }'
"""

from __future__ import annotations

import json
import logging
import os
import sys
from typing import IO

from savants.graph.client import GraphClient
from savants.graph.query import GraphQueryEngine

logger = logging.getLogger(__name__)

MCP_PROTOCOL_VERSION = "2024-11-05"


def read_message(stream: IO[bytes]) -> dict | None:
    """Read one newline-delimited JSON-RPC message from a byte stream.

    MCP stdio transport uses line-delimited JSON: each message is a
    single line, terminated by `\\n`. Blank lines are skipped. Returns
    None on EOF.
    """
    while True:
        line = stream.readline()
        if not line:
            return None  # EOF
        stripped = line.strip()
        if not stripped:
            continue  # blank line, skip
        try:
            return json.loads(stripped.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as e:
            logger.warning("Skipping malformed line on stdin: %s", e)
            continue


def write_message(stream: IO[bytes], msg: dict) -> None:
    """Write one JSON-RPC message followed by a newline.

    MCP stdio transport: one JSON object per line, `\\n` terminator,
    no embedded newlines in the body. json.dumps with default settings
    never produces embedded newlines so we're safe.
    """
    body = json.dumps(msg, separators=(",", ":")).encode("utf-8")
    stream.write(body + b"\n")
    stream.flush()


class SynapCodeMCPServer:
    """MCP-compliant server using newline-delimited JSON-RPC stdio transport."""

    def __init__(self, client: GraphClient | None = None):
        self.client = client or GraphClient()
        self.engine = GraphQueryEngine(self.client)
        self._initialized = False

    def handle_message(self, message: dict) -> dict | None:
        """Handle a JSON-RPC message. Returns None for notifications."""
        method = message.get("method", "")
        params = message.get("params", {})
        req_id = message.get("id")  # None for notifications

        # Notifications (no id) — handle silently
        if req_id is None:
            if method == "notifications/initialized":
                self._initialized = True
                logger.info("Client confirmed initialization")
            elif method == "notifications/cancelled":
                logger.info("Client cancelled request")
            return None  # No response for notifications

        # Requests (have id) — must respond
        if method == "initialize":
            return self._response(req_id, {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {"listChanged": False},
                    "resources": {},
                    "prompts": {},
                },
                "serverInfo": {"name": "savants", "version": "0.1.0"},
            })

        elif method == "ping":
            return self._response(req_id, {})

        elif method == "tools/list":
            return self._response(req_id, {"tools": self._list_tools()})

        elif method == "tools/call":
            return self._handle_tool_call(req_id, params)

        elif method == "resources/list":
            return self._response(req_id, {"resources": []})

        elif method == "prompts/list":
            return self._response(req_id, {"prompts": []})

        return self._error(req_id, -32601, f"Unknown method: {method}")

    def _list_tools(self) -> list[dict]:
        return [
            {
                "name": "impact_analysis",
                "description": (
                    "Analyze the cascading impact of changing a function. "
                    "Returns all direct and transitive dependents."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function_name": {
                            "type": "string",
                            "description": "Name of the function to analyze",
                        },
                        "max_depth": {"type": "integer", "default": 5},
                    },
                    "required": ["function_name"],
                },
            },
            {
                "name": "search_code",
                "description": "Search for functions and classes by name pattern.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Search pattern",
                        },
                    },
                    "required": ["pattern"],
                },
            },
            {
                "name": "dependency_chain",
                "description": "Find the shortest dependency path between two files.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "from_file": {"type": "string"},
                        "to_file": {"type": "string"},
                    },
                    "required": ["from_file", "to_file"],
                },
            },
            {
                "name": "community_summary",
                "description": "Identify the most connected hub files in the codebase.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "max_results": {"type": "integer", "default": 10},
                    },
                },
            },
            {
                "name": "graph_stats",
                "description": "Get node and edge counts for the Code Property Graph.",
                "inputSchema": {"type": "object", "properties": {}},
            },
            {
                "name": "advanced_graph_query",
                "description": (
                    "Power-user escape hatch: run a raw graph query against "
                    "the underlying engine. Most agents should prefer the "
                    "higher-level tools (function_xray, find_references_structured, "
                    "etc.) which return structured results without requiring "
                    "knowledge of the query language."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Graph query string",
                        },
                    },
                    "required": ["query"],
                },
            },
            {
                "name": "recall_history",
                "description": (
                    "Recall historical facts and episodes from episodic memory. "
                    "Useful for understanding why code was written a certain way."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Topic or entity to recall history for",
                        },
                    },
                    "required": ["query"],
                },
            },
            # --- Grounding tools (for AI agent use) -------------------------
            {
                "name": "find_references_structured",
                "description": (
                    "Replacement for grep / IDE 'Find References'. Returns the "
                    "structural callers of a function with metadata: who, where, "
                    "co-change partners, and the file each lives in. Use this "
                    "instead of text search whenever you need to find where a "
                    "function is actually called from."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function_name": {"type": "string"},
                        "include_tests": {"type": "boolean", "default": True},
                    },
                    "required": ["function_name"],
                },
            },
            {
                "name": "function_xray",
                "description": (
                    "Composite query: returns the full structural and historical "
                    "profile of a function in one call. Includes definition site, "
                    "current callers, callees, classes that contain it, and "
                    "(if history is loaded) recent contributors and last touch."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function_name": {"type": "string"},
                        "file_path": {
                            "type": "string",
                            "description": "Optional — disambiguate when name is shared across files",
                        },
                    },
                    "required": ["function_name"],
                },
            },
            {
                "name": "co_change_partners",
                "description": (
                    "Find functions that historically change in the same commits "
                    "as the target. Reveals hidden coupling that the static call "
                    "graph alone cannot show. Requires history to be loaded."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function_name": {"type": "string"},
                        "limit": {"type": "integer", "default": 10},
                    },
                    "required": ["function_name"],
                },
            },
            {
                "name": "coupling_check",
                "description": (
                    "Check whether a new dependency between two modules would "
                    "violate the codebase's existing architectural boundaries. "
                    "Returns the historical edge count between the two modules "
                    "and warns if it has been zero. Use before introducing a "
                    "new import or call across module boundaries."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "from_module": {
                            "type": "string",
                            "description": "Source module path prefix (e.g., 'src/payments/')",
                        },
                        "to_module": {
                            "type": "string",
                            "description": "Target module path prefix (e.g., 'src/admin/')",
                        },
                    },
                    "required": ["from_module", "to_module"],
                },
            },
            {
                "name": "pre_change_warning",
                "description": (
                    "Before modifying a function, check the structural and "
                    "historical risk of the change. Returns a warning text that "
                    "an AI agent should consider before suggesting edits. "
                    "Includes blast radius, maintainer concentration, and "
                    "stale-knowledge alerts."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function_name": {"type": "string"},
                        "file_path": {"type": "string"},
                    },
                    "required": ["function_name"],
                },
            },
            {
                "name": "risk_score",
                "description": (
                    "Compute a 0-10 risk score for modifying a function. "
                    "Combines call-graph blast radius, historical bug correlation, "
                    "maintainer bus factor, and recency-of-last-touch into a single "
                    "number suitable for PR review heuristics."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "function_name": {"type": "string"},
                        "file_path": {"type": "string"},
                    },
                    "required": ["function_name"],
                },
            },
            {
                "name": "decorated_with",
                "description": (
                    "List all functions decorated with a given decorator name, "
                    "e.g. 'workflow.defn', 'app.route', 'lru_cache'. "
                    "Matches on the decorator's callable expression "
                    "(trailing segment or full dotted path)."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "decorator_name": {"type": "string"},
                    },
                    "required": ["decorator_name"],
                },
            },
            {
                "name": "reindex",
                "description": (
                    "Rebuild the graph for a repository. By default does a "
                    "full rebuild (drops the existing graph and re-parses "
                    "every file); pass incremental=true to only re-parse "
                    "changed files since the last bookmark. This is the "
                    "self-heal tool: if an agent detects the graph is stale "
                    "or missing data, it can call this directly via MCP "
                    "without dropping to the CLI."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "repo_path": {
                            "type": "string",
                            "description": "Absolute path to the repository to index",
                        },
                        "full": {
                            "type": "boolean",
                            "default": True,
                            "description": "Drop and rebuild the entire graph (default true)",
                        },
                    },
                    "required": ["repo_path"],
                },
            },
            {
                "name": "cluster_state",
                "description": (
                    "Return a summary of what's running in a Kubernetes cluster: "
                    "namespace count, deployment count, pod count by status, "
                    "service count, and top namespaces by workload. Requires "
                    "the cluster to have been indexed via the K8s ingestor "
                    "(see savants.k8s.ingestor) — this tool queries the "
                    "stored graph, not the live K8s API, so it's sub-second."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cluster": {
                            "type": "string",
                            "description": "Cluster name as stored in the graph (e.g. 'astra-k3s')",
                        },
                    },
                    "required": ["cluster"],
                },
            },
            {
                "name": "list_pods",
                "description": (
                    "List Kubernetes pods matching a filter. Can filter by "
                    "namespace, status (Running, CrashLoopBackOff, etc.), or "
                    "a substring of the pod name. Returns pod name, namespace, "
                    "status, image, restart count, and the Deployment/StatefulSet "
                    "that owns it. Use to triage 'what's broken' questions."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cluster": {"type": "string"},
                        "namespace": {"type": "string"},
                        "status": {
                            "type": "string",
                            "description": "Filter by pod status (Running, Pending, CrashLoopBackOff, Failed, etc.)",
                        },
                        "name_contains": {
                            "type": "string",
                            "description": "Substring match on pod name",
                        },
                    },
                    "required": ["cluster"],
                },
            },
            {
                "name": "deployment_info",
                "description": (
                    "Full details for a Kubernetes Deployment: replica status, "
                    "current image, labels, and all pods belonging to it. Used "
                    "when an engineer needs to know 'is this service healthy?'"
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cluster": {"type": "string"},
                        "namespace": {"type": "string"},
                        "name": {"type": "string"},
                    },
                    "required": ["cluster", "namespace", "name"],
                },
            },
            {
                "name": "pod_dependencies",
                "description": (
                    "Return every ConfigMap and Secret that a Pod reads from "
                    "(via volumes, envFrom, or env.valueFrom references). "
                    "Answers 'what config does this pod depend on?' and helps "
                    "with impact analysis when a ConfigMap or Secret is changed."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cluster": {"type": "string"},
                        "namespace": {"type": "string"},
                        "pod": {"type": "string"},
                    },
                    "required": ["cluster", "namespace", "pod"],
                },
            },
            {
                "name": "namespace_summary",
                "description": (
                    "Everything in a namespace: deployments, pods (grouped by "
                    "status), services, configmap count, secret count. Useful "
                    "for 'show me the state of the payments namespace' queries."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cluster": {"type": "string"},
                        "namespace": {"type": "string"},
                    },
                    "required": ["cluster", "namespace"],
                },
            },
            {
                "name": "pod_story",
                "description": (
                    "THE MTTR KILLER TOOL: given a pod (or a whole cluster), "
                    "return a narrative-ready summary of the significant log "
                    "events it has emitted — deduplicated by drain3 template, "
                    "ranked by severity and count, with up to 3 example lines "
                    "per template. Answers 'what's wrong with this pod?' in "
                    "one call by reading LogEvent nodes produced by the log "
                    "watcher. If `pod` is omitted, returns the top events "
                    "across the whole cluster. Use `since_minutes` to scope "
                    "to a recent incident window."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cluster": {"type": "string"},
                        "pod": {
                            "type": "string",
                            "description": "Pod name (optional — omit for cluster-wide view)",
                        },
                        "namespace": {"type": "string"},
                        "since_minutes": {
                            "type": "integer",
                            "description": (
                                "Only include events whose last_seen is within "
                                "the last N minutes. Default: 60. Pass 0 to "
                                "disable the time filter entirely (returns all "
                                "retained events, useful for historical review)."
                            ),
                        },
                        "min_severity": {
                            "type": "string",
                            "description": "INFO | WARN | ERROR | FATAL (default: WARN)",
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Max events to return (default: 15)",
                        },
                    },
                    "required": ["cluster"],
                },
            },
            {
                "name": "federated_symbol_in_cluster",
                "description": (
                    "THE KILLER FEDERATED QUERY: given a function/class/symbol "
                    "name from the code graph, find any Kubernetes resources "
                    "in the cluster graph that reference it (as container "
                    "image names, ConfigMap keys, labels, or env values). "
                    "This demonstrates the cross-graph join: code graph → "
                    "cluster graph via symbol matching."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": {"type": "string"},
                        "cluster": {"type": "string"},
                    },
                    "required": ["symbol", "cluster"],
                },
            },
            {
                "name": "diff_impact",
                "description": (
                    "Structural blast radius for a git ref or range. "
                    "Returns: changed files, changed functions, transitively "
                    "reachable entry points (routes, workflows, CLI commands, "
                    "tasks), and config keys in touched files. This is the "
                    "PR-review killer query — the one humans actually want "
                    "answered when deciding whether a change is safe to merge."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "ref": {
                            "type": "string",
                            "description": "git ref (HEAD, abc123), or range (main..branch, a...b)",
                        },
                        "repo_path": {
                            "type": "string",
                            "description": "Path to the git repo (defaults to cwd)",
                        },
                    },
                    "required": ["ref"],
                },
            },
            {
                "name": "resolves_to",
                "description": (
                    "Given a string literal (e.g. a registry key, Temporal "
                    "activity name, or config value), find any Function or "
                    "Class in the graph whose name matches — plus every "
                    "function that mentions the string. Closes the "
                    "registry-dispatch blind spot that grep handles poorly."
                ),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "symbol": {"type": "string"},
                    },
                    "required": ["symbol"],
                },
            },
        ]

    def _handle_tool_call(self, req_id: int | str, params: dict) -> dict:
        tool_name = params.get("name", "")
        args = params.get("arguments", {})

        try:
            if tool_name == "impact_analysis":
                result = self.engine.impact_analysis(
                    args["function_name"],
                    args.get("max_depth", 5),
                )
                text = (
                    f"Impact analysis for '{result.target}':\n"
                    f"Direct dependents: {result.direct_dependents}\n"
                    f"Transitive dependents: {result.transitive_dependents}\n"
                    f"Affected files: {result.affected_files}"
                )

            elif tool_name == "search_code":
                results = self.engine.search_by_pattern(args["pattern"])
                text = json.dumps(results, indent=2)

            elif tool_name == "dependency_chain":
                chain = self.engine.find_dependency_chain(
                    args["from_file"], args["to_file"]
                )
                text = " -> ".join(chain) if chain else "No dependency chain found."

            elif tool_name == "community_summary":
                summary = self.engine.community_summary(args.get("max_results", 10))
                text = json.dumps(summary, indent=2)

            elif tool_name == "graph_stats":
                nodes = self.client.node_count()
                edges = self.client.edge_count()
                text = f"Nodes: {nodes}, Edges: {edges}"

            elif tool_name == "advanced_graph_query":
                result = self.client.query(args["query"])
                text = str(result.result_set)

            elif tool_name == "recall_history":
                from savants.graph.episodic import EpisodicMemory

                memory = EpisodicMemory(self.client)
                recall = memory.recall(args["query"])
                parts = []
                for f in recall.facts[:15]:
                    parts.append(f"{f.subject} -{f.predicate}-> {f.object}")
                for e in recall.episodes[:10]:
                    parts.append(f"[{e.source_type}] {e.content[:200]}")
                text = "\n".join(parts) if parts else "No relevant history found."

            # --- Grounding tools ---------------------------------------

            elif tool_name == "find_references_structured":
                text = self._tool_find_references(
                    args["function_name"],
                    args.get("include_tests", True),
                )

            elif tool_name == "function_xray":
                text = self._tool_function_xray(
                    args["function_name"],
                    args.get("file_path"),
                )

            elif tool_name == "co_change_partners":
                text = self._tool_co_change(
                    args["function_name"],
                    args.get("limit", 10),
                )

            elif tool_name == "coupling_check":
                text = self._tool_coupling_check(
                    args["from_module"],
                    args["to_module"],
                )

            elif tool_name == "pre_change_warning":
                text = self._tool_pre_change_warning(
                    args["function_name"],
                    args.get("file_path"),
                )

            elif tool_name == "risk_score":
                text = self._tool_risk_score(
                    args["function_name"],
                    args.get("file_path"),
                )

            elif tool_name == "decorated_with":
                text = self._tool_decorated_with(args["decorator_name"])

            elif tool_name == "resolves_to":
                text = self._tool_resolves_to(args["symbol"])

            elif tool_name == "cluster_state":
                text = self._tool_cluster_state(args["cluster"])

            elif tool_name == "list_pods":
                text = self._tool_list_pods(
                    args["cluster"],
                    args.get("namespace"),
                    args.get("status"),
                    args.get("name_contains"),
                )

            elif tool_name == "deployment_info":
                text = self._tool_deployment_info(
                    args["cluster"], args["namespace"], args["name"]
                )

            elif tool_name == "pod_dependencies":
                text = self._tool_pod_dependencies(
                    args["cluster"], args["namespace"], args["pod"]
                )

            elif tool_name == "namespace_summary":
                text = self._tool_namespace_summary(
                    args["cluster"], args["namespace"]
                )

            elif tool_name == "pod_story":
                text = self._tool_pod_story(
                    args["cluster"],
                    args.get("pod"),
                    args.get("namespace"),
                    args.get("since_minutes", 60),
                    args.get("min_severity", "WARN"),
                    args.get("limit", 15),
                )

            elif tool_name == "federated_symbol_in_cluster":
                text = self._tool_federated_symbol_in_cluster(
                    args["symbol"], args["cluster"]
                )

            elif tool_name == "diff_impact":
                text = self._tool_diff_impact(
                    args["ref"],
                    args.get("repo_path", "."),
                )

            elif tool_name == "reindex":
                text = self._tool_reindex(
                    args["repo_path"],
                    args.get("full", True),
                )

            else:
                return self._error(req_id, -32602, f"Unknown tool: {tool_name}")

            return self._response(req_id, {
                "content": [{"type": "text", "text": text}],
            })

        except Exception as e:
            return self._response(req_id, {
                "content": [{"type": "text", "text": f"Error: {e}"}],
                "isError": True,
            })

    # --- Grounding tool implementations ----------------------------------

    def _tool_find_references(self, function_name: str, include_tests: bool) -> str:
        """Smart find-references: structural callers with metadata."""
        where = "" if include_tests else "AND NOT caller.file_path STARTS WITH 'tests/'"
        result = self.client.query(
            f"MATCH (caller:Function)-[:CALLS]->(target:Function {{name: $name}}) "
            f"WHERE 1=1 {where} "
            "RETURN caller.name, caller.file_path "
            "ORDER BY caller.file_path LIMIT 50",
            {"name": function_name},
        )
        rows = result.result_set
        if not rows:
            return f"No structural callers found for '{function_name}'."

        # Group by file
        by_file: dict[str, list[str]] = {}
        for caller_name, caller_file in rows:
            by_file.setdefault(caller_file, []).append(caller_name)

        lines = [f"{len(rows)} references to '{function_name}':"]
        lines.append("")
        for file in sorted(by_file):
            lines.append(f"  {file}")
            for cn in sorted(by_file[file]):
                lines.append(f"    └─ {cn}")
        return "\n".join(lines)

    def _tool_function_xray(self, function_name: str, file_path: str | None) -> str:
        """Composite query: structural + historical profile of a function."""
        # Find the function (disambiguate by file_path if given)
        if file_path:
            fn_result = self.client.query(
                "MATCH (fn:Function {name: $name, file_path: $fp}) "
                "RETURN fn.name, fn.file_path, fn.start_line, fn.end_line, fn.parameters",
                {"name": function_name, "fp": file_path},
            )
        else:
            fn_result = self.client.query(
                "MATCH (fn:Function {name: $name}) "
                "RETURN fn.name, fn.file_path, fn.start_line, fn.end_line, fn.parameters "
                "LIMIT 5",
                {"name": function_name},
            )

        if not fn_result.result_set:
            return f"Function '{function_name}' not found in graph."

        lines = [f"╔═══ X-Ray: {function_name} ═══"]
        for row in fn_result.result_set:
            name, fp, start, end, params = row
            lines.append(f"║  {fp}:{start}")
            lines.append(f"║  parameters: {params or '(none)'}")
            lines.append("║")

            # Callers
            callers = self.client.query(
                "MATCH (c:Function)-[:CALLS]->(t:Function {name: $name, file_path: $fp}) "
                "RETURN c.name, c.file_path LIMIT 25",
                {"name": name, "fp": fp},
            ).result_set
            lines.append(f"║  Direct callers: {len(callers)}")
            for cn, cf in callers[:8]:
                lines.append(f"║    - {cn} ({cf})")
            if len(callers) > 8:
                lines.append(f"║    ... and {len(callers) - 8} more")

            # Callees
            callees = self.client.query(
                "MATCH (t:Function {name: $name, file_path: $fp})-[:CALLS]->(c:Function) "
                "RETURN c.name LIMIT 15",
                {"name": name, "fp": fp},
            ).result_set
            lines.append(f"║  Direct callees: {len(callees)}")
            for (cn,) in callees[:6]:
                lines.append(f"║    - {cn}")

            # Episodes (history)
            episodes = self.client.query(
                "MATCH (e:Episode)-[:CHANGES]->(:Function {name: $name, file_path: $fp}) "
                "RETURN e.timestamp, e.author, e.message "
                "ORDER BY e.timestamp DESC LIMIT 5",
                {"name": name, "fp": fp},
            ).result_set
            if episodes:
                lines.append(f"║  Recent commits ({len(episodes)} shown):")
                for ts, author, msg in episodes:
                    short_author = author.split("<")[0].strip() if author else "?"
                    short_msg = (msg[:60] + "...") if msg and len(msg) > 60 else (msg or "")
                    lines.append(f"║    {ts[:10]}  {short_author}  {short_msg}")
            else:
                lines.append("║  Recent commits: (history not loaded)")

            lines.append("║")
        lines.append("╚════════════════════════════")
        return "\n".join(lines)

    def _tool_co_change(self, function_name: str, limit: int) -> str:
        """Functions that historically change in the same commits."""
        result = self.client.query(
            "MATCH (e:Episode)-[:CHANGES]->(fn1:Function {name: $name}) "
            "MATCH (e)-[:CHANGES]->(fn2:Function) "
            "WHERE fn1.name <> fn2.name "
            "RETURN fn2.name, count(e) AS co "
            "ORDER BY co DESC "
            f"LIMIT {limit}",
            {"name": function_name},
        )
        rows = result.result_set
        if not rows:
            return (
                f"No co-change history for '{function_name}'. "
                f"(Either the function is new, or git history hasn't been walked.)"
            )

        lines = [f"Functions that change with '{function_name}' historically:"]
        for name, co in rows:
            lines.append(f"  {co:>4}× — {name}")
        return "\n".join(lines)

    def _tool_coupling_check(self, from_module: str, to_module: str) -> str:
        """Check whether a new dependency would violate existing boundaries."""
        # How many CALLS edges currently cross from from_module → to_module?
        result = self.client.query(
            "MATCH (a:Function)-[:CALLS]->(b:Function) "
            "WHERE a.file_path STARTS WITH $from_mod "
            "  AND b.file_path STARTS WITH $to_mod "
            "RETURN count(*) AS edge_count",
            {"from_mod": from_module, "to_mod": to_module},
        )
        edge_count = result.result_set[0][0] if result.result_set else 0

        if edge_count == 0:
            return (
                f"⚠️  COUPLING WARNING\n"
                f"   {from_module} -> {to_module}\n"
                f"   Current edges: 0\n\n"
                f"   These two modules currently have NO call edges between them. "
                f"Introducing a new dependency would be the first.\n\n"
                f"   This pattern often indicates:\n"
                f"   - The modules were intentionally kept separate\n"
                f"   - You may be violating an implicit architectural boundary\n"
                f"   - Code review will likely push back\n\n"
                f"   If intentional, document why in the commit message."
            )
        else:
            return (
                f"OK — coupling already exists.\n"
                f"   {from_module} -> {to_module}\n"
                f"   Existing call edges: {edge_count}\n"
                f"   Adding another is consistent with current architecture."
            )

    def _tool_pre_change_warning(self, function_name: str, file_path: str | None) -> str:
        """Generate a warning about modifying a function."""
        # Get blast radius
        callers = self.client.query(
            "MATCH (c:Function)-[:CALLS]->(t:Function {name: $name}) "
            "RETURN count(c)",
            {"name": function_name},
        ).result_set
        direct = callers[0][0] if callers else 0

        transitive = self.client.query(
            "MATCH (c:Function)-[:CALLS*1..3]->(t:Function {name: $name}) "
            "RETURN count(DISTINCT c)",
            {"name": function_name},
        ).result_set
        trans = transitive[0][0] if transitive else 0

        # Last touched in history
        last_touch = self.client.query(
            "MATCH (e:Episode)-[:CHANGES]->(:Function {name: $name}) "
            "RETURN e.timestamp, e.author "
            "ORDER BY e.timestamp DESC LIMIT 1",
            {"name": function_name},
        ).result_set

        # Maintainer concentration
        maintainers = self.client.query(
            "MATCH (e:Episode)-[:CHANGES]->(:Function {name: $name}) "
            "RETURN e.author, count(e) AS touches "
            "ORDER BY touches DESC LIMIT 3",
            {"name": function_name},
        ).result_set

        lines = [f"Pre-change warning for '{function_name}':"]
        lines.append("")
        lines.append(f"  Blast radius:")
        lines.append(f"    Direct callers:     {direct}")
        lines.append(f"    Transitive (3 hops): {trans}")
        lines.append("")

        if direct > 50 or trans > 200:
            lines.append("  ⚠️  HIGH BLAST RADIUS — many things depend on this.")
            lines.append("")

        if last_touch:
            ts, author = last_touch[0]
            short_author = author.split("<")[0].strip() if author else "?"
            lines.append(f"  Last touched: {ts[:10]} by {short_author}")
            lines.append("")

        if maintainers:
            lines.append("  Recent maintainers:")
            for author, touches in maintainers:
                short = author.split("<")[0].strip() if author else "?"
                lines.append(f"    {touches:>3}× — {short}")
            if len(maintainers) == 1:
                lines.append("")
                lines.append("  ⚠️  BUS FACTOR 1 — only one person has touched this recently.")
        else:
            lines.append("  History: not loaded for this function.")

        return "\n".join(lines)

    def _tool_risk_score(self, function_name: str, file_path: str | None) -> str:
        """Compute a 0-10 risk score for modifying a function."""
        # Components:
        #   blast (0-4)         based on transitive caller count
        #   bus_factor (0-3)    based on maintainer concentration
        #   recency (0-2)       based on staleness
        #   incident (0-1)      based on history co-occurrence with "fix"/"hotfix" commits
        score = 0.0
        breakdown = []

        # Blast
        trans = self.client.query(
            "MATCH (c:Function)-[:CALLS*1..3]->(t:Function {name: $name}) "
            "RETURN count(DISTINCT c)",
            {"name": function_name},
        ).result_set
        trans_count = trans[0][0] if trans else 0
        if trans_count > 200:
            blast = 4.0
        elif trans_count > 50:
            blast = 3.0
        elif trans_count > 10:
            blast = 2.0
        elif trans_count > 0:
            blast = 1.0
        else:
            blast = 0.0
        score += blast
        breakdown.append(f"  blast radius:  {blast}/4   ({trans_count} transitive callers)")

        # Bus factor
        maintainers = self.client.query(
            "MATCH (e:Episode)-[:CHANGES]->(:Function {name: $name}) "
            "RETURN e.author, count(e) AS t "
            "ORDER BY t DESC",
            {"name": function_name},
        ).result_set
        if not maintainers:
            bus = 0.0
            bus_note = "(no history loaded)"
        elif len(maintainers) == 1:
            bus = 3.0
            bus_note = f"only 1 contributor ({maintainers[0][0].split('<')[0].strip()})"
        elif len(maintainers) == 2:
            bus = 2.0
            bus_note = "2 contributors"
        else:
            bus = 1.0
            bus_note = f"{len(maintainers)} contributors"
        score += bus
        breakdown.append(f"  bus factor:    {bus}/3   {bus_note}")

        # Incident correlation: did any commit touching this function have "fix" in the message?
        fix_commits = self.client.query(
            "MATCH (e:Episode)-[:CHANGES]->(:Function {name: $name}) "
            "WHERE toLower(e.message) CONTAINS 'fix' OR toLower(e.message) CONTAINS 'hotfix' "
            "RETURN count(e)",
            {"name": function_name},
        ).result_set
        fix_count = fix_commits[0][0] if fix_commits else 0
        incident = 1.0 if fix_count >= 2 else 0.5 if fix_count == 1 else 0.0
        score += incident
        breakdown.append(f"  incidents:     {incident}/1   ({fix_count} fix-related commits)")

        verdict = "LOW" if score < 3 else "MEDIUM" if score < 6 else "HIGH" if score < 8 else "VERY HIGH"
        emoji = "🟢" if score < 3 else "🟡" if score < 6 else "🟠" if score < 8 else "🔴"

        return (
            f"{emoji} Risk score for '{function_name}': {score:.1f} / 10 — {verdict}\n\n"
            + "\n".join(breakdown)
        )

    def _response(self, req_id: int | str | None, result: dict) -> dict:
        return {"jsonrpc": "2.0", "id": req_id, "result": result}

    def _tool_decorated_with(self, decorator_name: str) -> str:
        """List functions decorated with `decorator_name`.

        Uses the indexed Decorator node + DECORATED_BY edge added in the
        decorator-perf fix. The Decorator.name property is indexed, so
        the exact-equality clause is an O(1) index lookup, and the
        suffix-match clause scans only the (typically small) set of
        distinct decorator names — never the full Function table.

        Matching semantics: caller passes either a dotted name
        ('workflow.defn') or a bare name ('command'). We match against
        decorators that are either exactly that name OR end with
        '.{name}'. So:

          - decorator_name='workflow.defn' matches '@workflow.defn' AND
            '@temporalio.workflow.defn' but NOT '@activity.defn'
          - decorator_name='command' matches '@command' AND '@cli.command'
            AND '@click.command' but NOT '@cli.commands'

        We deliberately do NOT do tail-segment broadening (no
        'workflow.defn' → 'defn' fallback). Earlier versions did, and it
        caused workflow.defn and activity.defn to both return the same
        ~73 results because both end in '.defn'. The current rule:
        what the caller asks for is what they get.
        """
        needle = decorator_name.strip()
        # Match BOTH Function-level decorators (e.g. @app.route, @cli.command)
        # AND Class-level decorators (e.g. @workflow.defn, @dataclass,
        # @strawberry.type). Class-level decorators on Temporal workflow
        # classes were invisible until the parser was extended to walk
        # decorated_definition wrappers around class_definition nodes.
        params = {"needle": needle, "dot_needle": "." + needle}
        fn_rows = self.client.query(
            "MATCH (f:Function)-[:DECORATED_BY]->(d:Decorator) "
            "WHERE d.name = $needle OR d.name ENDS WITH $dot_needle "
            "RETURN DISTINCT 'Function' AS kind, f.name AS name, f.file_path AS fp, d.name AS dec "
            "ORDER BY fp, name",
            params,
        ).result_set
        cls_rows = self.client.query(
            "MATCH (c:Class)-[:DECORATED_BY]->(d:Decorator) "
            "WHERE d.name = $needle OR d.name ENDS WITH $dot_needle "
            "RETURN DISTINCT 'Class' AS kind, c.name AS name, c.file_path AS fp, d.name AS dec "
            "ORDER BY fp, name",
            params,
        ).result_set
        rows = fn_rows + cls_rows

        if not rows:
            return f"No functions or classes decorated with '{decorator_name}'."

        lines = [f"{len(rows)} symbol(s) decorated with '{decorator_name}':"]
        for kind, name, fpath, d in rows[:50]:
            tag = "fn" if kind == "Function" else "cls"
            lines.append(f"  [{tag}] @{d:<25} {name}  ({fpath})")
        if len(rows) > 50:
            lines.append(f"  ... and {len(rows) - 50} more")
        return "\n".join(lines)

    def _tool_resolves_to(self, symbol: str) -> str:
        """Resolve a string literal to any matching Function/Class + callers."""
        terminal = symbol.rsplit(".", 1)[-1]

        defs = self.client.query(
            "MATCH (n) WHERE (n:Function OR n:Class) AND n.name = $t "
            "RETURN labels(n)[0], n.name, n.file_path LIMIT 20",
            {"t": terminal},
        ).result_set

        refs = self.client.query(
            "MATCH (c:Function)-[:REFERENCES_SYMBOL]->(t) WHERE t.name = $t "
            "RETURN DISTINCT c.name, c.file_path LIMIT 30",
            {"t": terminal},
        ).result_set

        lines = [f"Resolving '{symbol}' (terminal: '{terminal}'):"]
        lines.append(f"\nDefinitions ({len(defs)}):")
        if defs:
            for label, n, fp in defs:
                lines.append(f"  [{label}] {n}  ({fp})")
        else:
            lines.append("  (none — may be external, dynamic, or not indexed)")

        lines.append(f"\nString-literal references ({len(refs)}):")
        if refs:
            for n, fp in refs:
                lines.append(f"  {n}  ({fp})")
        else:
            lines.append("  (none)")

        return "\n".join(lines)

    # ------------------------------------------------------------------
    # K8s runtime layer tools
    #
    # These tools query the cluster graph (populated by K8sIngestor) rather
    # than the code graph. They are the first concrete delivery of the Live
    # Infrastructure Layer from docs/live-infrastructure-layer.md.
    #
    # All of these assume `self.client` is connected to the cluster graph.
    # In production, the federation server would route cluster queries to
    # the appropriate cluster graph automatically — for the MVP we rely on
    # the env var FALKORDB_GRAPH to pick the right graph per-server.
    # ------------------------------------------------------------------

    def _k8s_client(self, cluster: str) -> GraphClient:
        """Return a GraphClient pointed at the cluster graph for `cluster`.

        The graph name convention is `{cluster_name}` with hyphens
        replaced by underscores (e.g. 'astra-k3s' → 'astra_k3s'). This
        is temporary until the full federation server is built — at
        that point the routing will happen at the server level, not
        per-tool.
        """
        graph_name = cluster.replace("-", "_")
        if self.client._config.graph_name == graph_name:
            return self.client
        from savants.config import FalkorDBConfig
        new_cfg = FalkorDBConfig(
            host=self.client._config.host,
            port=self.client._config.port,
            graph_name=graph_name,
        )
        return GraphClient(new_cfg)

    def _tool_cluster_state(self, cluster: str) -> str:
        c = self._k8s_client(cluster)
        try:
            ns_count = c.query("MATCH (n:K8sNamespace) RETURN count(n)").result_set[0][0]
            deploy_count = c.query("MATCH (d:K8sDeployment) RETURN count(d)").result_set[0][0]
            pod_total = c.query("MATCH (p:K8sPod) RETURN count(p)").result_set[0][0]
            svc_count = c.query("MATCH (s:K8sService) RETURN count(s)").result_set[0][0]
            cm_count = c.query("MATCH (cm:K8sConfigMap) RETURN count(cm)").result_set[0][0]
            sec_count = c.query("MATCH (sec:K8sSecret) RETURN count(sec)").result_set[0][0]
        except Exception as e:
            return f"Cluster '{cluster}' not found in graph or empty. Error: {e}"

        if ns_count == 0:
            return (
                f"Cluster '{cluster}' has no data in the graph. "
                f"Run savants.k8s.ingestor.K8sIngestor to populate it first."
            )

        # Pod count by status
        status_rows = c.query(
            "MATCH (p:K8sPod) RETURN p.status, count(p) ORDER BY count(p) DESC"
        ).result_set
        status_breakdown = "\n".join(f"    {row[1]:4}  {row[0]}" for row in status_rows)

        # Top namespaces by workload (pods)
        top_ns = c.query(
            "MATCH (n:K8sNamespace)-[:CONTAINS]->(p:K8sPod) "
            "RETURN n.name, count(p) AS pods ORDER BY pods DESC LIMIT 10"
        ).result_set
        top_ns_str = "\n".join(f"    {row[1]:4}  {row[0]}" for row in top_ns)

        lines = [
            f"Cluster: {cluster}",
            f"",
            f"Resource counts:",
            f"  Namespaces:   {ns_count}",
            f"  Deployments:  {deploy_count}",
            f"  Pods:         {pod_total}",
            f"  Services:     {svc_count}",
            f"  ConfigMaps:   {cm_count}",
            f"  Secrets:      {sec_count}",
            f"",
            f"Pods by status:",
            status_breakdown,
            f"",
            f"Top namespaces by workload:",
            top_ns_str,
        ]
        return "\n".join(lines)

    def _tool_list_pods(
        self,
        cluster: str,
        namespace: str | None,
        status: str | None,
        name_contains: str | None,
    ) -> str:
        c = self._k8s_client(cluster)

        where = []
        params: dict = {}
        if namespace:
            where.append("p.namespace = $namespace")
            params["namespace"] = namespace
        if status:
            where.append("p.status = $status")
            params["status"] = status
        if name_contains:
            where.append("p.name CONTAINS $name_contains")
            params["name_contains"] = name_contains

        where_clause = (" WHERE " + " AND ".join(where)) if where else ""
        query = (
            "MATCH (p:K8sPod)" + where_clause +
            " RETURN p.namespace, p.name, p.status, p.image, p.restart_count, "
            "p.owner_kind, p.owner_name ORDER BY p.namespace, p.name LIMIT 100"
        )
        rows = c.query(query, params).result_set

        if not rows:
            return "No pods found matching filters."

        lines = [f"Found {len(rows)} pods:"]
        for ns, name, st, img, rc, okind, oname in rows:
            owner = f" ← {okind}/{oname}" if okind else ""
            restarts = f" (restarts={rc})" if rc > 0 else ""
            lines.append(f"  [{st:15}] {ns}/{name}{owner}{restarts}")
            if img:
                lines.append(f"    image: {img}")
        if len(rows) >= 100:
            lines.append("  (limit reached, refine filters to see more)")
        return "\n".join(lines)

    def _tool_deployment_info(self, cluster: str, namespace: str, name: str) -> str:
        c = self._k8s_client(cluster)

        deploy_rows = c.query(
            "MATCH (d:K8sDeployment {name: $name, namespace: $ns}) "
            "RETURN d.kind, d.replicas_desired, d.replicas_ready, "
            "d.replicas_available, d.image, d.labels LIMIT 1",
            {"name": name, "ns": namespace},
        ).result_set
        if not deploy_rows:
            return f"No Deployment/StatefulSet/DaemonSet named '{name}' in namespace '{namespace}'."
        kind, rd, rr, ra, image, labels = deploy_rows[0]

        # Pods belonging to this deployment (via owner_name match)
        pod_rows = c.query(
            "MATCH (p:K8sPod) "
            "WHERE p.namespace = $ns AND (p.owner_name STARTS WITH $name OR p.owner_name = $name) "
            "RETURN p.name, p.status, p.restart_count, p.node_name",
            {"ns": namespace, "name": name},
        ).result_set

        lines = [
            f"{kind}: {namespace}/{name}",
            f"  Image:        {image}",
            f"  Replicas:     {rr}/{rd} ready, {ra} available",
            f"  Labels:       {', '.join(labels) if labels else '(none)'}",
            f"",
            f"Pods ({len(pod_rows)}):",
        ]
        for pn, st, rc, node in pod_rows:
            restarts = f" (restarts={rc})" if rc > 0 else ""
            node_str = f" on {node}" if node else ""
            lines.append(f"  [{st:15}] {pn}{node_str}{restarts}")
        return "\n".join(lines)

    def _tool_pod_dependencies(
        self, cluster: str, namespace: str, pod: str
    ) -> str:
        c = self._k8s_client(cluster)

        cm_rows = c.query(
            "MATCH (p:K8sPod {name: $pod, namespace: $ns})-[:READS]->(cm:K8sConfigMap) "
            "RETURN cm.name, cm.key_names",
            {"pod": pod, "ns": namespace},
        ).result_set

        sec_rows = c.query(
            "MATCH (p:K8sPod {name: $pod, namespace: $ns})-[:READS]->(sec:K8sSecret) "
            "RETURN sec.name, sec.type, sec.key_names",
            {"pod": pod, "ns": namespace},
        ).result_set

        if not cm_rows and not sec_rows:
            return f"Pod {namespace}/{pod} has no ConfigMap or Secret dependencies (or doesn't exist in graph)."

        lines = [f"Dependencies for pod {namespace}/{pod}:"]
        lines.append(f"")
        lines.append(f"ConfigMaps ({len(cm_rows)}):")
        for name, keys in cm_rows:
            key_str = f" [{len(keys)} keys]" if keys else ""
            lines.append(f"  • {name}{key_str}")
        lines.append(f"")
        lines.append(f"Secrets ({len(sec_rows)}):")
        for name, type_, keys in sec_rows:
            key_str = f" [{len(keys)} keys]" if keys else ""
            lines.append(f"  • {name} ({type_}){key_str}")
        return "\n".join(lines)

    def _tool_namespace_summary(self, cluster: str, namespace: str) -> str:
        c = self._k8s_client(cluster)

        # Verify namespace exists
        ns_rows = c.query(
            "MATCH (n:K8sNamespace {name: $ns}) RETURN n.status, n.age_seconds",
            {"ns": namespace},
        ).result_set
        if not ns_rows:
            return f"Namespace '{namespace}' not found in cluster graph."
        status, age = ns_rows[0]

        # Counts
        counts = {}
        for label, var in [
            ("K8sDeployment", "d"),
            ("K8sPod", "p"),
            ("K8sService", "s"),
            ("K8sConfigMap", "cm"),
            ("K8sSecret", "sec"),
        ]:
            r = c.query(
                f"MATCH (n:K8sNamespace {{name: $ns}})-[:CONTAINS]->({var}:{label}) "
                f"RETURN count({var})",
                {"ns": namespace},
            ).result_set
            counts[label] = r[0][0] if r else 0

        # Deployments list with health
        deploys = c.query(
            "MATCH (n:K8sNamespace {name: $ns})-[:CONTAINS]->(d:K8sDeployment) "
            "RETURN d.name, d.kind, d.replicas_ready, d.replicas_desired, d.image "
            "ORDER BY d.name",
            {"ns": namespace},
        ).result_set

        # Pod status breakdown
        pod_status = c.query(
            "MATCH (n:K8sNamespace {name: $ns})-[:CONTAINS]->(p:K8sPod) "
            "RETURN p.status, count(p) ORDER BY count(p) DESC",
            {"ns": namespace},
        ).result_set

        lines = [
            f"Namespace: {namespace}",
            f"  Status:       {status}",
            f"  Age:          {age // 86400} days, {(age % 86400) // 3600} hours",
            f"",
            f"Resource counts:",
            f"  Deployments:  {counts.get('K8sDeployment', 0)}",
            f"  Pods:         {counts.get('K8sPod', 0)}",
            f"  Services:     {counts.get('K8sService', 0)}",
            f"  ConfigMaps:   {counts.get('K8sConfigMap', 0)}",
            f"  Secrets:      {counts.get('K8sSecret', 0)}",
            f"",
            f"Pod status breakdown:",
        ]
        for st, cnt in pod_status:
            lines.append(f"  {cnt:4}  {st}")

        if deploys:
            lines.append(f"")
            lines.append(f"Deployments:")
            for d_name, d_kind, rr, rd, img in deploys:
                health = "✓" if rr == rd and rd > 0 else "⚠"
                lines.append(f"  {health} [{d_kind:12}] {d_name}  ({rr}/{rd} ready)")
                if img:
                    lines.append(f"      {img}")

        return "\n".join(lines)

    def _tool_pod_story(
        self,
        cluster: str,
        pod: str | None,
        namespace: str | None,
        since_minutes: int | None,
        min_severity: str,
        limit: int,
    ) -> str:
        """MTTR tool: summarize significant LogEvents for a pod or whole cluster.

        Reads from the log intelligence layer (LogEvent nodes + EMITTED
        edges) populated by `savants.k8s.log_watcher.LogWatcher`. No
        live K8s calls. No raw logs. Just the pre-digested story.
        """
        c = self._k8s_client(cluster)

        sev_rank = {"INFO": 0, "WARN": 1, "ERROR": 2, "FATAL": 3}
        min_rank = sev_rank.get(min_severity.upper(), 1)
        allowed = [s for s, r in sev_rank.items() if r >= min_rank]

        where = ["e.cluster = $cluster", "e.severity IN $allowed"]
        params: dict = {"cluster": cluster, "allowed": allowed, "limit": limit}
        if pod:
            where.append("e.pod = $pod")
            params["pod"] = pod
        if namespace:
            where.append("e.namespace = $ns")
            params["ns"] = namespace
        # since_minutes=0 is the explicit opt-out ("give me everything").
        # None would mean "use the default" — but dispatch already applied
        # the default before calling, so None here means no filter.
        if since_minutes and since_minutes > 0:
            import time as _t
            params["since"] = _t.time() - (since_minutes * 60)
            where.append("e.last_seen >= $since")

        cy = (
            "MATCH (e:LogEvent) WHERE " + " AND ".join(where) + " "
            "RETURN e.pod, e.namespace, e.severity, e.count, "
            "       e.template_text, e.example_lines, "
            "       e.first_seen, e.last_seen, e.pod_deleted_at "
            "ORDER BY CASE e.severity "
            "           WHEN 'FATAL' THEN 3 "
            "           WHEN 'ERROR' THEN 2 "
            "           WHEN 'WARN' THEN 1 "
            "           ELSE 0 END DESC, e.count DESC "
            "LIMIT $limit"
        )
        rows = c.query(cy, params).result_set

        # Header: totals + severity histogram + pod count
        hist_cy = (
            "MATCH (e:LogEvent) WHERE " + " AND ".join(where[:-0] if False else where) + " "
            "RETURN e.severity, count(e), sum(e.count), count(DISTINCT e.pod)"
        )
        # Drop the limit from params for histogram
        hist_params = {k: v for k, v in params.items() if k != "limit"}
        hist_cy = hist_cy.replace("$limit", "15")  # placeholder, not used
        hist = c.query(
            "MATCH (e:LogEvent) WHERE " + " AND ".join(where) + " "
            "RETURN e.severity, count(e), sum(e.count), count(DISTINCT e.pod)",
            hist_params,
        ).result_set

        scope = []
        if pod:
            scope.append(f"pod={pod}")
        if namespace:
            scope.append(f"namespace={namespace}")
        if since_minutes:
            scope.append(f"last {since_minutes}m")
        scope_str = ", ".join(scope) if scope else "cluster-wide"

        lines: list[str] = []
        lines.append(f"# Log story for {cluster} ({scope_str})")
        lines.append("")

        if not rows:
            lines.append(
                "No significant log events found. Either the log watcher "
                "isn't running, the filters excluded everything, or the pod "
                "is actually healthy."
            )
            return "\n".join(lines)

        total_templates = sum(int(r[1]) for r in hist)
        total_occurrences = sum(int(r[2] or 0) for r in hist)
        total_pods = max((int(r[3]) for r in hist), default=0)
        lines.append(
            f"**Summary:** {total_templates} distinct templates, "
            f"{total_occurrences} total occurrences, across "
            f"{total_pods} pods"
        )
        hist_str = ", ".join(
            f"{r[0]}={int(r[1])}" for r in sorted(hist, key=lambda x: -int(x[1]))
        )
        lines.append(f"**Severity:** {hist_str}")
        lines.append("")

        lines.append(f"## Top {len(rows)} events (by severity, then volume)")
        lines.append("")
        import time as _now_mod
        now_ts = _now_mod.time()
        for i, row in enumerate(rows, 1):
            pod_name, ns, sev, cnt, tmpl, examples, first_seen, last_seen, deleted_at = row
            tombstone = ""
            if deleted_at:
                try:
                    ago = int(now_ts - float(deleted_at))
                    mins = ago // 60
                    tombstone = f" ⚰ (pod deleted {mins}m ago)" if mins else " ⚰ (pod just deleted)"
                except Exception:
                    tombstone = " ⚰ (pod deleted)"
            lines.append(
                f"### {i}. [{sev}] {ns}/{pod_name}{tombstone} — {int(cnt)} occurrences"
            )
            if tmpl:
                lines.append(f"Template: `{tmpl[:200]}`")
            if examples:
                lines.append("Example:")
                lines.append(f"    {examples[0][:250]}")
            if last_seen:
                import datetime as _dt
                try:
                    ts = _dt.datetime.fromtimestamp(float(last_seen)).isoformat(
                        timespec="seconds"
                    )
                    lines.append(f"Last seen: {ts}")
                except Exception:
                    pass
            # Mentions: show the graph entities this event refers to.
            mentions_r = c.query(
                "MATCH (e:LogEvent {cluster: $cluster, namespace: $ns, "
                "pod: $pod, template_hash: $th})-[:MENTIONS]->(x) "
                "RETURN labels(x)[0], x.name",
                {"cluster": cluster, "ns": ns, "pod": pod_name, "th": None},
            )
            # The template_hash isn't in the row — refetch via ordering is
            # expensive. Simpler: query by (pod, count) match — but safest
            # is to skip mentions here and show them in a dedicated pass
            # below. Drop this inline attempt.
            lines.append("")

        # Mentions summary: entities referenced across all returned events.
        mentions_rows = c.query(
            "MATCH (e:LogEvent) WHERE " + " AND ".join(where) + " "
            "MATCH (e)-[:MENTIONS]->(x) "
            "RETURN labels(x)[0], x.name, x.namespace, count(DISTINCT e) "
            "ORDER BY count(DISTINCT e) DESC LIMIT 20",
            hist_params,
        ).result_set
        if mentions_rows:
            lines.append("## Referenced entities (from log text)")
            lines.append("")
            for label, ent_name, ent_ns, n_events in mentions_rows:
                short_label = label.replace("K8s", "")
                lines.append(
                    f"- **{short_label}** `{ent_ns}/{ent_name}` "
                    f"— mentioned by {int(n_events)} event(s)"
                )
            lines.append("")

        return "\n".join(lines)

    def _tool_federated_symbol_in_cluster(self, symbol: str, cluster: str) -> str:
        """The killer federated query: find a code symbol across both graphs.

        This is the first working demonstration of cross-graph federation.
        It queries the cluster graph for any K8s resource whose name,
        image, labels, or other properties reference the given symbol,
        then reports what was found alongside whatever the code graph
        knows about that symbol.
        """
        # Query 1: what does the code graph know about this symbol?
        # (Uses the current client's graph — assumed to be the code graph.)
        code_hits = self.client.query(
            "MATCH (n) WHERE (n:Function OR n:Class) AND n.name = $symbol "
            "RETURN labels(n)[0], n.name, n.file_path LIMIT 10",
            {"symbol": symbol},
        ).result_set

        # Query 2: what does the cluster graph know?
        c = self._k8s_client(cluster)

        # Match images containing the symbol as a substring
        image_hits = c.query(
            "MATCH (d:K8sDeployment) WHERE d.image CONTAINS $symbol "
            "RETURN d.namespace, d.name, d.image",
            {"symbol": symbol},
        ).result_set

        # Match deployment/service names containing the symbol
        name_hits = c.query(
            "MATCH (d:K8sDeployment) WHERE d.name CONTAINS $symbol "
            "RETURN d.namespace, d.name, d.image",
            {"symbol": symbol},
        ).result_set
        svc_hits = c.query(
            "MATCH (s:K8sService) WHERE s.name CONTAINS $symbol "
            "RETURN s.namespace, s.name, s.type",
            {"symbol": symbol},
        ).result_set

        # Match ConfigMap key names (which often contain application symbols)
        cm_hits = c.query(
            "MATCH (cm:K8sConfigMap) WHERE ANY(k IN cm.key_names WHERE k CONTAINS $symbol) "
            "RETURN cm.namespace, cm.name, cm.key_names",
            {"symbol": symbol},
        ).result_set

        lines = [f"Federated query for symbol '{symbol}' across code + cluster '{cluster}':"]
        lines.append("")

        if code_hits:
            lines.append(f"Code graph ({len(code_hits)} matches):")
            for label, name, path in code_hits:
                lines.append(f"  [{label}] {name}  ({path})")
        else:
            lines.append("Code graph: no Function or Class with this exact name.")
        lines.append("")

        cluster_found = False
        if image_hits:
            cluster_found = True
            lines.append(f"Cluster Deployments running image matching '{symbol}' ({len(image_hits)}):")
            for ns, n, img in image_hits:
                lines.append(f"  {ns}/{n}")
                lines.append(f"    image: {img}")
        if name_hits:
            cluster_found = True
            lines.append(f"Cluster Deployments named like '{symbol}' ({len(name_hits)}):")
            for ns, n, img in name_hits:
                lines.append(f"  {ns}/{n}")
        if svc_hits:
            cluster_found = True
            lines.append(f"Cluster Services named like '{symbol}' ({len(svc_hits)}):")
            for ns, n, t in svc_hits:
                lines.append(f"  {ns}/{n} ({t})")
        if cm_hits:
            cluster_found = True
            lines.append(f"ConfigMaps with key names matching '{symbol}' ({len(cm_hits)}):")
            for ns, n, keys in cm_hits:
                matched = [k for k in keys if symbol in k]
                lines.append(f"  {ns}/{n}  keys: {', '.join(matched[:3])}")

        if not cluster_found:
            lines.append(f"Cluster graph: no references to '{symbol}' found.")

        return "\n".join(lines)

    def _tool_diff_impact(self, ref: str, repo_path: str) -> str:
        from savants.analysis.diff_impact import diff_impact, format_report

        report = diff_impact(repo_path=repo_path, ref=ref, client=self.client)
        return format_report(report)

    def _tool_reindex(self, repo_path: str, full: bool) -> str:
        """Self-heal tool: rebuild the graph from a repo without leaving MCP.

        Agents call this when they detect stale or missing data. It runs
        the same builder the CLI uses, against the same graph the MCP
        server is currently connected to. Returns a summary of what was
        indexed so the caller can verify success.
        """
        import time
        from savants.graph.cpg import CodePropertyGraphBuilder

        if not os.path.isdir(repo_path):
            return f"Error: repo_path '{repo_path}' is not a directory."

        t0 = time.time()
        try:
            if full:
                try:
                    self.client.delete_graph()
                except Exception:
                    pass
                self.client.ensure_schema()
                builder = CodePropertyGraphBuilder(repo_path=repo_path, client=self.client)
                stats = builder.build()
            else:
                # Incremental reindex via the existing CLI helper paths
                from savants.sync.diff import compute_diff
                from savants.sync.git_hooks import get_current_head, get_last_indexed_sha
                bookmark = get_last_indexed_sha(repo_path)
                head = get_current_head(repo_path)
                if bookmark and bookmark != head:
                    diff = compute_diff(repo_path, bookmark, head)
                    builder = CodePropertyGraphBuilder(repo_path=repo_path, client=self.client)
                    stats = builder.build_incremental(
                        changed_files=diff.changed_files + diff.added_files,
                        deleted_files=diff.deleted_files,
                    )
                else:
                    return f"Incremental reindex skipped: no diff between bookmark and HEAD."
        except Exception as e:
            return f"Reindex failed: {e}"

        elapsed = time.time() - t0
        n_nodes = self.client.node_count()
        n_edges = self.client.edge_count()

        lines = [
            f"Reindex of {repo_path} complete in {elapsed:.1f}s ({'full' if full else 'incremental'})",
            f"  Files:        {stats.get('files', stats.get('updated', 0))}",
            f"  Functions:    {stats.get('functions', 0)}",
            f"  Classes:      {stats.get('classes', 0)}",
            f"  Edges:        {stats.get('edges', 0)}",
            f"  Config keys:  {stats.get('config_keys', 0)}",
            f"  Env vars:     {stats.get('env_vars', 0)}",
            f"  Symbol refs:  {stats.get('references_symbol', 0)}",
            f"",
            f"  Graph total:  {n_nodes} nodes / {n_edges} edges",
        ]
        return "\n".join(lines)

    def _error(self, req_id: int | str | None, code: int, message: str) -> dict:
        return {"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": message}}

    def run(self) -> None:
        """Run the MCP server using newline-delimited JSON-RPC stdio transport."""
        logger.info("SynapCode MCP server started (newline-delimited JSON-RPC)")
        stdin = sys.stdin.buffer
        stdout = sys.stdout.buffer

        while True:
            try:
                message = read_message(stdin)
                if message is None:
                    break  # EOF

                response = self.handle_message(message)
                if response is not None:
                    write_message(stdout, response)

            except json.JSONDecodeError:
                error = self._error(None, -32700, "Parse error")
                write_message(stdout, error)
            except Exception as e:
                logger.error("Unexpected error: %s", e)
                error = self._error(None, -32603, f"Internal error: {e}")
                write_message(stdout, error)


def main():
    logging.basicConfig(level=logging.INFO, stream=sys.stderr)
    server = SynapCodeMCPServer()
    server.run()


if __name__ == "__main__":
    main()

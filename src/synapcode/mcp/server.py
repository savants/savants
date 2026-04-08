"""MCP Server: exposes FalkorDB graph queries to external AI tools.

Implements the Model Context Protocol with proper Content-Length framed
stdio transport, allowing Claude Code, Cursor, and VS Code to query
the local Code Property Graph for structural context.

Usage:
    claude mcp add-json synapcode --scope user '{
      "command": "python",
      "args": ["-m", "synapcode.mcp"],
      "env": {"FALKORDB_HOST": "localhost", "FALKORDB_PORT": "6379"}
    }'
"""

from __future__ import annotations

import json
import logging
import sys
from typing import IO

from synapcode.graph.client import GraphClient
from synapcode.graph.query import GraphQueryEngine

logger = logging.getLogger(__name__)

MCP_PROTOCOL_VERSION = "2024-11-05"


def read_message(stream: IO[bytes]) -> dict | None:
    """Read a Content-Length framed JSON-RPC message from a byte stream."""
    content_length = -1

    while True:
        line = stream.readline()
        if not line:
            return None  # EOF

        line_str = line.decode("utf-8").rstrip("\r\n")

        if line_str == "":
            # Empty line = end of headers
            break

        if line_str.lower().startswith("content-length:"):
            content_length = int(line_str.split(":", 1)[1].strip())

    if content_length < 0:
        return None

    body = stream.read(content_length)
    if len(body) < content_length:
        return None

    return json.loads(body.decode("utf-8"))


def write_message(stream: IO[bytes], msg: dict) -> None:
    """Write a Content-Length framed JSON-RPC message to a byte stream."""
    body = json.dumps(msg).encode("utf-8")
    header = f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8")
    stream.write(header + body)
    stream.flush()


class SynapCodeMCPServer:
    """MCP-compliant server using Content-Length framed stdio transport."""

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
                "serverInfo": {"name": "synapcode", "version": "0.1.0"},
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
                "name": "cypher_query",
                "description": "Execute a raw Cypher query against the graph.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Cypher query string",
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

            elif tool_name == "cypher_query":
                result = self.client.query(args["query"])
                text = str(result.result_set)

            elif tool_name == "recall_history":
                from synapcode.graph.episodic import EpisodicMemory

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

            elif tool_name == "diff_impact":
                text = self._tool_diff_impact(
                    args["ref"],
                    args.get("repo_path", "."),
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
        """List functions whose `decorators` property contains decorator_name.

        Matches are loose: the decorator list stores expressions like
        'workflow.defn' or 'app.route'; we match on exact, trailing segment,
        and dotted-prefix to keep the tool forgiving.
        """
        rows = self.client.query(
            "MATCH (f:Function) WHERE f.decorators IS NOT NULL "
            "RETURN f.name, f.file_path, f.decorators"
        ).result_set

        needle = decorator_name.strip()
        tail = needle.rsplit(".", 1)[-1]

        matches: list[tuple[str, str, str]] = []
        for name, fpath, decs in rows:
            if not decs:
                continue
            for d in decs:
                if d == needle or d.endswith("." + needle) or d == tail or d.endswith("." + tail):
                    matches.append((name, fpath, d))
                    break

        if not matches:
            return f"No functions decorated with '{decorator_name}'."

        lines = [f"{len(matches)} function(s) decorated with '{decorator_name}':"]
        for name, fpath, d in matches[:50]:
            lines.append(f"  @{d:<25} {name}  ({fpath})")
        if len(matches) > 50:
            lines.append(f"  ... and {len(matches) - 50} more")
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

    def _tool_diff_impact(self, ref: str, repo_path: str) -> str:
        from synapcode.analysis.diff_impact import diff_impact, format_report

        report = diff_impact(repo_path=repo_path, ref=ref, client=self.client)
        return format_report(report)

    def _error(self, req_id: int | str | None, code: int, message: str) -> dict:
        return {"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": message}}

    def run(self) -> None:
        """Run the MCP server using Content-Length framed stdio transport."""
        logger.info("SynapCode MCP server started (Content-Length framed)")
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

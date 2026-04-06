"""MCP Server: exposes FalkorDB graph queries to external AI tools.

This server implements the Model Context Protocol, allowing tools like
Claude Code, Cursor, and VS Code Copilot to query the local Code Property
Graph for structural context.

Usage:
    claude mcp add-json synapcode --scope user '{
      "command": "python",
      "args": ["-m", "synapcode.mcp.server"],
      "env": {"FALKORDB_HOST": "localhost", "FALKORDB_PORT": "6379"}
    }'
"""

from __future__ import annotations

import json
import logging
import sys

from synapcode.graph.client import GraphClient
from synapcode.graph.query import GraphQueryEngine

logger = logging.getLogger(__name__)


class SynapCodeMCPServer:
    """Minimal MCP server that reads JSON-RPC from stdin and writes to stdout."""

    def __init__(self):
        self.client = GraphClient()
        self.engine = GraphQueryEngine(self.client)

    def handle_request(self, request: dict) -> dict:
        method = request.get("method", "")
        params = request.get("params", {})
        req_id = request.get("id")

        if method == "initialize":
            return self._response(req_id, {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": {"name": "synapcode", "version": "0.1.0"},
            })

        elif method == "tools/list":
            return self._response(req_id, {"tools": self._list_tools()})

        elif method == "tools/call":
            return self._handle_tool_call(req_id, params)

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
                        "function_name": {"type": "string", "description": "Name of the function to analyze"},
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
                        "pattern": {"type": "string", "description": "Search pattern"},
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
                        "query": {"type": "string", "description": "Cypher query string"},
                    },
                    "required": ["query"],
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
                chain = self.engine.find_dependency_chain(args["from_file"], args["to_file"])
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

    def _response(self, req_id: int | str | None, result: dict) -> dict:
        return {"jsonrpc": "2.0", "id": req_id, "result": result}

    def _error(self, req_id: int | str | None, code: int, message: str) -> dict:
        return {"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": message}}

    def run(self) -> None:
        """Run the MCP server, reading JSON-RPC from stdin."""
        logger.info("SynapCode MCP server started")
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                request = json.loads(line)
                response = self.handle_request(request)
                sys.stdout.write(json.dumps(response) + "\n")
                sys.stdout.flush()
            except json.JSONDecodeError:
                error = self._error(None, -32700, "Parse error")
                sys.stdout.write(json.dumps(error) + "\n")
                sys.stdout.flush()


def main():
    logging.basicConfig(level=logging.INFO, stream=sys.stderr)
    server = SynapCodeMCPServer()
    server.run()


if __name__ == "__main__":
    main()

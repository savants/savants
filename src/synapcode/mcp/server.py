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

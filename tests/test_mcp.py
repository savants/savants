"""Acceptance tests for the MCP server protocol compliance.

Tests the full JSON-RPC lifecycle with Content-Length framed transport.
"""

from __future__ import annotations

import io
import json
from unittest.mock import MagicMock, patch

from synapcode.mcp.server import (
    SynapCodeMCPServer,
    read_message,
    write_message,
)


def _frame(msg: dict) -> bytes:
    """Create a Content-Length framed message."""
    body = json.dumps(msg).encode("utf-8")
    return f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8") + body


def _make_request(method: str, params: dict | None = None, req_id: int = 1) -> dict:
    msg = {"jsonrpc": "2.0", "method": method, "id": req_id}
    if params is not None:
        msg["params"] = params
    return msg


def _make_notification(method: str, params: dict | None = None) -> dict:
    msg = {"jsonrpc": "2.0", "method": method}
    if params is not None:
        msg["params"] = params
    return msg


class TestContentLengthFraming:
    def test_read_message(self):
        msg = {"jsonrpc": "2.0", "method": "ping", "id": 1}
        stream = io.BytesIO(_frame(msg))
        result = read_message(stream)
        assert result == msg

    def test_write_message(self):
        msg = {"jsonrpc": "2.0", "id": 1, "result": {}}
        stream = io.BytesIO()
        write_message(stream, msg)

        stream.seek(0)
        output = stream.read().decode("utf-8")
        assert output.startswith("Content-Length:")
        assert '"result"' in output

    def test_roundtrip(self):
        msg = {"jsonrpc": "2.0", "method": "test", "id": 42, "params": {"key": "val"}}
        stream = io.BytesIO()
        write_message(stream, msg)
        stream.seek(0)
        result = read_message(stream)
        assert result == msg

    def test_eof_returns_none(self):
        stream = io.BytesIO(b"")
        assert read_message(stream) is None


@patch("synapcode.mcp.server.GraphClient")
@patch("synapcode.mcp.server.GraphQueryEngine")
class TestMCPLifecycle:
    def test_initialize(self, mock_engine_cls, mock_client_cls):
        server = SynapCodeMCPServer()
        resp = server.handle_message(_make_request("initialize"))

        assert resp["id"] == 1
        assert resp["result"]["protocolVersion"] == "2024-11-05"
        assert "tools" in resp["result"]["capabilities"]
        assert resp["result"]["serverInfo"]["name"] == "synapcode"

    def test_notifications_return_none(self, mock_engine_cls, mock_client_cls):
        server = SynapCodeMCPServer()
        resp = server.handle_message(_make_notification("notifications/initialized"))
        assert resp is None
        assert server._initialized is True

    def test_ping(self, mock_engine_cls, mock_client_cls):
        server = SynapCodeMCPServer()
        resp = server.handle_message(_make_request("ping"))
        assert resp["result"] == {}

    def test_tools_list(self, mock_engine_cls, mock_client_cls):
        server = SynapCodeMCPServer()
        resp = server.handle_message(_make_request("tools/list"))
        tools = resp["result"]["tools"]
        assert len(tools) >= 6
        names = {t["name"] for t in tools}
        assert "impact_analysis" in names
        assert "search_code" in names
        assert "recall_history" in names

    def test_tools_list_has_input_schemas(self, mock_engine_cls, mock_client_cls):
        server = SynapCodeMCPServer()
        resp = server.handle_message(_make_request("tools/list"))
        for tool in resp["result"]["tools"]:
            assert "inputSchema" in tool
            assert tool["inputSchema"]["type"] == "object"

    def test_resources_list_empty(self, mock_engine_cls, mock_client_cls):
        server = SynapCodeMCPServer()
        resp = server.handle_message(_make_request("resources/list"))
        assert resp["result"] == {"resources": []}

    def test_prompts_list_empty(self, mock_engine_cls, mock_client_cls):
        server = SynapCodeMCPServer()
        resp = server.handle_message(_make_request("prompts/list"))
        assert resp["result"] == {"prompts": []}

    def test_unknown_method(self, mock_engine_cls, mock_client_cls):
        server = SynapCodeMCPServer()
        resp = server.handle_message(_make_request("nonexistent/method"))
        assert "error" in resp
        assert resp["error"]["code"] == -32601

    def test_unknown_tool(self, mock_engine_cls, mock_client_cls):
        server = SynapCodeMCPServer()
        resp = server.handle_message(_make_request(
            "tools/call",
            {"name": "nonexistent_tool", "arguments": {}},
        ))
        assert "error" in resp
        assert resp["error"]["code"] == -32602


@patch("synapcode.mcp.server.GraphClient")
@patch("synapcode.mcp.server.GraphQueryEngine")
class TestToolCalls:
    def test_graph_stats(self, mock_engine_cls, mock_client_cls):
        server = SynapCodeMCPServer()
        server.client.node_count.return_value = 42
        server.client.edge_count.return_value = 100

        resp = server.handle_message(_make_request(
            "tools/call",
            {"name": "graph_stats", "arguments": {}},
        ))
        content = resp["result"]["content"][0]["text"]
        assert "42" in content
        assert "100" in content

    def test_search_code(self, mock_engine_cls, mock_client_cls):
        server = SynapCodeMCPServer()
        server.engine.search_by_pattern.return_value = [
            {"type": "Function", "name": "test_fn", "file": "test.py"}
        ]

        resp = server.handle_message(_make_request(
            "tools/call",
            {"name": "search_code", "arguments": {"pattern": "test"}},
        ))
        content = resp["result"]["content"][0]["text"]
        assert "test_fn" in content

    def test_tool_error_returns_isError(self, mock_engine_cls, mock_client_cls):
        server = SynapCodeMCPServer()
        server.engine.impact_analysis.side_effect = RuntimeError("DB down")

        resp = server.handle_message(_make_request(
            "tools/call",
            {"name": "impact_analysis", "arguments": {"function_name": "foo"}},
        ))
        assert resp["result"]["isError"] is True
        assert "DB down" in resp["result"]["content"][0]["text"]

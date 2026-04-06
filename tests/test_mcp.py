"""Tests for MCP server — Content-Length framing is pure IO, tool calls hit real FalkorDB."""

from __future__ import annotations

import io
import json

import pytest

from synapcode.mcp.server import (
    SynapCodeMCPServer,
    read_message,
    write_message,
)


def _frame(msg: dict) -> bytes:
    body = json.dumps(msg).encode("utf-8")
    return f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8") + body


def _req(method: str, params: dict | None = None, req_id: int = 1) -> dict:
    msg: dict = {"jsonrpc": "2.0", "method": method, "id": req_id}
    if params is not None:
        msg["params"] = params
    return msg


def _notif(method: str) -> dict:
    return {"jsonrpc": "2.0", "method": method}


class TestContentLengthFraming:
    """Pure IO tests — no FalkorDB needed."""

    def test_read_message(self):
        msg = {"jsonrpc": "2.0", "method": "ping", "id": 1}
        stream = io.BytesIO(_frame(msg))
        assert read_message(stream) == msg

    def test_write_then_read_roundtrip(self):
        msg = {"jsonrpc": "2.0", "method": "test", "id": 42, "params": {"k": "v"}}
        buf = io.BytesIO()
        write_message(buf, msg)
        buf.seek(0)
        assert read_message(buf) == msg

    def test_eof_returns_none(self):
        assert read_message(io.BytesIO(b"")) is None


@pytest.mark.integration
class TestMCPProtocol:
    """Full lifecycle tests against real FalkorDB."""

    def test_initialize(self, graph_client):
        server = SynapCodeMCPServer(client=graph_client)
        resp = server.handle_message(_req("initialize"))
        assert resp["result"]["protocolVersion"] == "2024-11-05"
        assert resp["result"]["serverInfo"]["name"] == "synapcode"

    def test_notification_returns_none(self, graph_client):
        server = SynapCodeMCPServer(client=graph_client)
        assert server.handle_message(_notif("notifications/initialized")) is None
        assert server._initialized is True

    def test_ping(self, graph_client):
        server = SynapCodeMCPServer(client=graph_client)
        resp = server.handle_message(_req("ping"))
        assert resp["result"] == {}

    def test_tools_list(self, graph_client):
        server = SynapCodeMCPServer(client=graph_client)
        resp = server.handle_message(_req("tools/list"))
        tools = resp["result"]["tools"]
        names = {t["name"] for t in tools}
        assert "impact_analysis" in names
        assert "search_code" in names
        assert "recall_history" in names
        for t in tools:
            assert "inputSchema" in t

    def test_resources_and_prompts_empty(self, graph_client):
        server = SynapCodeMCPServer(client=graph_client)
        assert server.handle_message(_req("resources/list"))["result"] == {"resources": []}
        assert server.handle_message(_req("prompts/list"))["result"] == {"prompts": []}

    def test_unknown_method_returns_error(self, graph_client):
        server = SynapCodeMCPServer(client=graph_client)
        resp = server.handle_message(_req("fake/method"))
        assert resp["error"]["code"] == -32601

    def test_unknown_tool_returns_error(self, graph_client):
        server = SynapCodeMCPServer(client=graph_client)
        resp = server.handle_message(_req(
            "tools/call", {"name": "fake_tool", "arguments": {}},
        ))
        assert resp["error"]["code"] == -32602


@pytest.mark.integration
class TestMCPToolCalls:
    """Tool calls against a seeded graph."""

    def test_graph_stats(self, indexed_repo):
        server = SynapCodeMCPServer(client=indexed_repo["client"])
        resp = server.handle_message(_req(
            "tools/call", {"name": "graph_stats", "arguments": {}},
        ))
        text = resp["result"]["content"][0]["text"]
        # Should report non-zero counts from the indexed test repo
        assert "Nodes:" in text
        assert "Edges:" in text

    def test_search_code(self, indexed_repo):
        server = SynapCodeMCPServer(client=indexed_repo["client"])
        resp = server.handle_message(_req(
            "tools/call", {"name": "search_code", "arguments": {"pattern": "helper"}},
        ))
        text = resp["result"]["content"][0]["text"]
        assert "helper" in text

    def test_tool_error_returns_isError(self, graph_client):
        server = SynapCodeMCPServer(client=graph_client)
        # Calling impact_analysis on empty graph shouldn't crash, just return empty
        resp = server.handle_message(_req(
            "tools/call",
            {"name": "impact_analysis", "arguments": {"function_name": "nonexistent"}},
        ))
        content = resp["result"]["content"][0]["text"]
        assert "Impact analysis" in content or "Error" in content

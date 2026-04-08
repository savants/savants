"""Tests for MCP server — newline-delimited JSON-RPC stdio, real FalkorDB for tool calls."""

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
    """Encode a single MCP stdio message: one JSON object per line."""
    return json.dumps(msg).encode("utf-8") + b"\n"


def test_transport_roundtrip_uses_newline_delimited_json():
    """Regression test: MCP stdio is newline-delimited JSON, NOT Content-Length framed.

    This bug took an entire night to find — the original implementation used
    LSP-style Content-Length headers, which Claude Code does not send. The
    server would block on stdin waiting for a Content-Length header that
    never arrived, then time out after 30s. This test ensures we never
    regress to the wrong framing.
    """
    msg = {"jsonrpc": "2.0", "method": "initialize", "id": 1}
    encoded = _frame(msg)
    # Must end in a newline, must NOT contain "Content-Length"
    assert encoded.endswith(b"\n"), "messages must be newline-terminated"
    assert b"Content-Length" not in encoded, "MCP stdio is NOT Content-Length framed"
    # Round-trip through read_message
    stream = io.BytesIO(encoded)
    decoded = read_message(stream)
    assert decoded == msg

    # write_message also uses newline-delimited format
    out = io.BytesIO()
    write_message(out, msg)
    out_bytes = out.getvalue()
    assert out_bytes.endswith(b"\n")
    assert b"Content-Length" not in out_bytes
    assert json.loads(out_bytes.strip()) == msg


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


@pytest.mark.integration
class TestGroundingTools:
    """Tests for the AI agent grounding tools."""

    def test_find_references_structured(self, indexed_repo):
        server = SynapCodeMCPServer(client=indexed_repo["client"])
        resp = server.handle_message(_req(
            "tools/call",
            {"name": "find_references_structured",
             "arguments": {"function_name": "helper"}},
        ))
        text = resp["result"]["content"][0]["text"]
        # Should either find structured callers or report none
        assert "references" in text.lower() or "no structural callers" in text.lower()

    def test_function_xray(self, indexed_repo):
        server = SynapCodeMCPServer(client=indexed_repo["client"])
        resp = server.handle_message(_req(
            "tools/call",
            {"name": "function_xray", "arguments": {"function_name": "helper"}},
        ))
        text = resp["result"]["content"][0]["text"]
        assert "X-Ray" in text or "not found" in text.lower()

    def test_co_change_partners_no_history(self, indexed_repo):
        server = SynapCodeMCPServer(client=indexed_repo["client"])
        resp = server.handle_message(_req(
            "tools/call",
            {"name": "co_change_partners",
             "arguments": {"function_name": "helper"}},
        ))
        text = resp["result"]["content"][0]["text"]
        # No history walked → should report missing
        assert "co-change" in text.lower() or "history" in text.lower()

    def test_coupling_check_existing(self, indexed_repo):
        server = SynapCodeMCPServer(client=indexed_repo["client"])
        resp = server.handle_message(_req(
            "tools/call",
            {"name": "coupling_check",
             "arguments": {"from_module": "src/", "to_module": "src/"}},
        ))
        text = resp["result"]["content"][0]["text"]
        # src/ -> src/ should have edges (the test repo's intra-module calls)
        assert "OK" in text or "WARNING" in text

    def test_coupling_check_zero_edges(self, indexed_repo):
        server = SynapCodeMCPServer(client=indexed_repo["client"])
        resp = server.handle_message(_req(
            "tools/call",
            {"name": "coupling_check",
             "arguments": {
                 "from_module": "non_existent_module/",
                 "to_module": "another_imaginary/",
             }},
        ))
        text = resp["result"]["content"][0]["text"]
        assert "WARNING" in text or "0" in text

    def test_pre_change_warning(self, indexed_repo):
        server = SynapCodeMCPServer(client=indexed_repo["client"])
        resp = server.handle_message(_req(
            "tools/call",
            {"name": "pre_change_warning",
             "arguments": {"function_name": "helper"}},
        ))
        text = resp["result"]["content"][0]["text"]
        assert "Pre-change warning" in text
        assert "Blast radius" in text or "blast" in text.lower()

    def test_risk_score_returns_score(self, indexed_repo):
        server = SynapCodeMCPServer(client=indexed_repo["client"])
        resp = server.handle_message(_req(
            "tools/call",
            {"name": "risk_score",
             "arguments": {"function_name": "helper"}},
        ))
        text = resp["result"]["content"][0]["text"]
        assert "Risk score" in text
        # Should produce a numeric score
        assert "/ 10" in text

    def test_grounding_tools_listed_in_tools_list(self, graph_client):
        server = SynapCodeMCPServer(client=graph_client)
        resp = server.handle_message(_req("tools/list"))
        names = {t["name"] for t in resp["result"]["tools"]}
        # All 6 grounding tools should be advertised
        for tool in [
            "find_references_structured",
            "function_xray",
            "co_change_partners",
            "coupling_check",
            "pre_change_warning",
            "risk_score",
        ]:
            assert tool in names, f"missing tool: {tool}"

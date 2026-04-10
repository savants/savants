"""Tests for pipeline routing and GraphRAG context injection — no mocks."""

from __future__ import annotations

import pytest

from savants.pipelines.graphrag_inlet import (
    build_graph_context,
    extract_code_references,
    inject_context,
)
from savants.pipelines.router import RouteDecision, estimate_complexity, route_request


class TestCodeReferenceExtraction:
    def test_extracts_function_calls(self):
        refs = extract_code_references("Call process_data() and validate_input()")
        assert "process_data" in refs["functions"]
        assert "validate_input" in refs["functions"]

    def test_extracts_pascal_case_classes(self):
        refs = extract_code_references("The DataProcessor and BaseHandler classes")
        assert "DataProcessor" in refs["classes"]
        assert "BaseHandler" in refs["classes"]

    def test_extracts_file_paths(self):
        refs = extract_code_references("Look at src/main.py and lib/utils.ts")
        assert "src/main.py" in refs["files"]
        assert "lib/utils.ts" in refs["files"]

    def test_empty_message(self):
        refs = extract_code_references("")
        assert refs == {"functions": [], "classes": [], "files": []}


class TestComplexityEstimation:
    def test_simple_scores_low(self):
        assert estimate_complexity("format this code") < 0.5

    def test_complex_scores_high(self):
        score = estimate_complexity(
            "refactor the architecture to handle cascading impact of dependency changes"
        )
        assert score > 0.7

    def test_bounds(self):
        score = estimate_complexity("x " * 2000 + " refactor impact cascading")
        assert 0.0 <= score <= 1.0


class TestRoutingDecisions:
    def test_low_complexity_routes_local(self):
        result = route_request("summarize this", local_ram_pct=30.0)
        assert result.decision == RouteDecision.LOCAL

    def test_high_complexity_routes_frontier(self):
        result = route_request(
            "analyze the cascading impact of this refactoring on the architecture",
            local_ram_pct=30.0,
        )
        assert result.decision == RouteDecision.FRONTIER

    def test_ram_overload_forces_frontier(self):
        result = route_request("simple question", local_ram_pct=80.0, ram_threshold=60.0)
        assert result.decision == RouteDecision.FRONTIER
        assert "RAM" in result.reason

    def test_returns_correct_model_names(self):
        result = route_request(
            "hello", local_model="my-local", frontier_model="my-frontier", local_ram_pct=30.0,
        )
        assert result.model == "my-local"


@pytest.mark.integration
class TestGraphContextInjection:
    def test_builds_context_from_real_graph(self, indexed_repo):
        """build_graph_context should return structural context from an indexed repo."""
        context = build_graph_context("What does helper() do?", indexed_repo["client"])
        # Should find the helper function in the graph
        assert context  # Non-empty means graph context was found
        assert "helper" in context.lower() or "Function" in context

    def test_inject_modifies_user_message(self, indexed_repo):
        messages = [{"role": "user", "content": "Tell me about helper()"}]
        result = inject_context(messages, indexed_repo["client"])
        # The user message should now contain graph context
        assert len(result[0]["content"]) > len("Tell me about helper()")

    def test_empty_graph_returns_empty_context(self, graph_client):
        context = build_graph_context("random question about nothing", graph_client)
        assert context == ""

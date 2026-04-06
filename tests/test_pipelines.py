"""Acceptance tests for pipeline routing and GraphRAG context injection."""

from __future__ import annotations

from synapcode.pipelines.graphrag_inlet import extract_code_references, inject_context
from synapcode.pipelines.router import RouteDecision, estimate_complexity, route_request


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
        assert refs["functions"] == []
        assert refs["classes"] == []
        assert refs["files"] == []


class TestContextInjection:
    def test_no_messages_returns_empty(self):
        result = inject_context([])
        assert result == []

    def test_preserves_system_messages(self):
        messages = [
            {"role": "system", "content": "You are helpful."},
            {"role": "user", "content": "hello"},
        ]
        # With no graph client, context injection will fail silently
        result = inject_context(messages)
        assert result[0]["role"] == "system"


class TestComplexityEstimation:
    def test_simple_scores_low(self):
        assert estimate_complexity("format this code") < 0.5

    def test_complex_scores_high(self):
        score = estimate_complexity(
            "refactor the architecture to handle cascading impact of dependency changes"
        )
        assert score > 0.7

    def test_long_messages_increase_score(self):
        short = estimate_complexity("fix bug")
        long = estimate_complexity("x " * 1500 + "fix bug")
        assert long >= short

    def test_code_blocks_increase_score(self):
        without = estimate_complexity("explain this function")
        with_code = estimate_complexity("explain this function ```def foo(): pass```")
        assert with_code >= without


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
        result = route_request(
            "simple question",
            local_ram_pct=80.0,
            ram_threshold=60.0,
        )
        assert result.decision == RouteDecision.FRONTIER
        assert "RAM" in result.reason

    def test_custom_threshold(self):
        # With a very high threshold, complex queries still route locally
        result = route_request(
            "refactor the architecture",
            complexity_threshold=0.99,
            local_ram_pct=30.0,
        )
        assert result.decision == RouteDecision.LOCAL

    def test_returns_correct_model_names(self):
        result = route_request(
            "hello",
            local_model="my-local-model",
            frontier_model="my-frontier-model",
            local_ram_pct=30.0,
        )
        assert result.model == "my-local-model"

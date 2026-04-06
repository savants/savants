"""Acceptance tests for the GraphRAG query engine."""

from __future__ import annotations

from unittest.mock import MagicMock

from synapcode.graph.query import GraphQueryEngine


def _mock_result(rows):
    result = MagicMock()
    result.result_set = rows
    return result


class TestImpactAnalysis:
    def test_returns_direct_and_transitive(self, mock_graph_client):
        mock_graph_client.query.side_effect = [
            _mock_result([["caller_a", "file_a.py"], ["caller_b", "file_b.py"]]),
            _mock_result([["transitive_c", "file_c.py", 2]]),
            _mock_result([["file_a.py"], ["file_b.py"], ["file_c.py"]]),
        ]
        engine = GraphQueryEngine(mock_graph_client)
        result = engine.impact_analysis("target_fn")

        assert result.target == "target_fn"
        assert "caller_a" in result.direct_dependents
        assert "transitive_c" in result.transitive_dependents
        assert len(result.affected_files) == 3

    def test_empty_graph_returns_empty(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([])
        engine = GraphQueryEngine(mock_graph_client)
        result = engine.impact_analysis("nonexistent")

        assert result.direct_dependents == []
        assert result.transitive_dependents == []
        assert result.affected_files == []


class TestDependencyChain:
    def test_returns_chain(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result(
            [["a.py", "b.py", "c.py"]]  # shortestPath returns a list
        )
        engine = GraphQueryEngine(mock_graph_client)
        chain = engine.find_dependency_chain("a.py", "c.py")
        # The query extracts the first row, first column (the list)
        assert chain == ["a.py", "b.py", "c.py"]

    def test_no_path_returns_empty(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([])
        engine = GraphQueryEngine(mock_graph_client)
        chain = engine.find_dependency_chain("a.py", "z.py")
        assert chain == []


class TestCommunity:
    def test_returns_sorted_hubs(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([
            ["core.py", 50],
            ["utils.py", 30],
            ["config.py", 20],
        ])
        engine = GraphQueryEngine(mock_graph_client)
        summary = engine.community_summary(3)

        assert len(summary) == 3
        assert summary[0]["file"] == "core.py"
        assert summary[0]["connections"] == 50


class TestSearch:
    def test_pattern_search(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([
            ["Function", "test_fn", "test.py"],
            ["Class", "TestClass", "test.py"],
        ])
        engine = GraphQueryEngine(mock_graph_client)
        results = engine.search_by_pattern("test")

        assert len(results) == 2
        assert results[0]["type"] == "Function"
        assert results[1]["name"] == "TestClass"

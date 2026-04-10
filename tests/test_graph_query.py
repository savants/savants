"""Tests for GraphRAG query engine — against real FalkorDB with seeded data."""

from __future__ import annotations

import pytest

from savants.graph.query import GraphQueryEngine


@pytest.mark.integration
class TestImpactAnalysis:
    def test_direct_dependents(self, indexed_repo):
        """Functions that call helper() should appear as dependents."""
        engine = GraphQueryEngine(indexed_repo["client"])
        result = engine.impact_analysis("helper")
        # process() calls helper() in the test repo
        assert any("process" in d for d in result.direct_dependents) or len(result.affected_files) > 0

    def test_no_dependents_for_leaf(self, indexed_repo):
        engine = GraphQueryEngine(indexed_repo["client"])
        result = engine.impact_analysis("entry_point")
        # entry_point is a leaf — nothing calls it
        assert result.direct_dependents == []

    def test_nonexistent_function(self, indexed_repo):
        engine = GraphQueryEngine(indexed_repo["client"])
        result = engine.impact_analysis("does_not_exist")
        assert result.direct_dependents == []
        assert result.affected_files == []


@pytest.mark.integration
class TestSearch:
    def test_finds_functions(self, indexed_repo):
        engine = GraphQueryEngine(indexed_repo["client"])
        results = engine.search_by_pattern("helper")
        names = [r["name"] for r in results]
        assert "helper" in names

    def test_finds_classes(self, indexed_repo):
        engine = GraphQueryEngine(indexed_repo["client"])
        results = engine.search_by_pattern("DataModel")
        names = [r["name"] for r in results]
        assert "DataModel" in names

    def test_no_results(self, indexed_repo):
        engine = GraphQueryEngine(indexed_repo["client"])
        results = engine.search_by_pattern("zzz_nonexistent_zzz")
        assert results == []


@pytest.mark.integration
class TestCommunity:
    def test_returns_hubs(self, indexed_repo):
        engine = GraphQueryEngine(indexed_repo["client"])
        summary = engine.community_summary(5)
        # Should return files that have the most connections
        assert len(summary) > 0
        assert "file" in summary[0]
        assert "connections" in summary[0]


@pytest.mark.integration
class TestFunctionContext:
    def test_returns_subgraph(self, indexed_repo):
        engine = GraphQueryEngine(indexed_repo["client"])
        ctx = engine.get_function_context("process")
        assert ctx.nodes  # Should have at least the function node itself
        assert "process" in ctx.summary

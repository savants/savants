"""Tests for graph garbage collection — real FalkorDB, no mocks."""

from __future__ import annotations

from datetime import datetime, timezone

import pytest

from synapcode.graph.gc import GraphGarbageCollector


@pytest.mark.integration
class TestOrphanCollection:
    def test_removes_orphan_nodes(self, graph_client):
        # Create an orphan Function node (no edges)
        graph_client.query("CREATE (:Function {name: 'orphan_fn', file_path: 'gone.py'})")
        # Create a connected pair (should NOT be removed)
        graph_client.query(
            "CREATE (:File {path: 'kept.py'})-[:CONTAINS]->(:Function {name: 'kept_fn'})"
        )

        gc = GraphGarbageCollector(graph_client)
        removed = gc.collect_orphan_nodes()

        assert removed >= 1

        # Verify orphan is gone
        result = graph_client.query(
            "MATCH (fn:Function {name: 'orphan_fn'}) RETURN count(fn)"
        )
        assert result.result_set[0][0] == 0

        # Verify connected function is still there
        result = graph_client.query(
            "MATCH (fn:Function {name: 'kept_fn'}) RETURN count(fn)"
        )
        assert result.result_set[0][0] == 1


@pytest.mark.integration
class TestStaleFileCollection:
    def test_removes_files_not_on_disk(self, graph_client, tmp_path):
        repo = tmp_path / "gc_repo"
        repo.mkdir()
        (repo / "exists.py").write_text("pass")

        # Add both to graph
        graph_client.query("CREATE (:File {path: 'exists.py'})")
        graph_client.query("CREATE (:File {path: 'deleted.py'})")

        gc = GraphGarbageCollector(graph_client)
        removed = gc.collect_stale_files(str(repo))

        assert removed == 1

        # exists.py still in graph
        result = graph_client.query("MATCH (f:File {path: 'exists.py'}) RETURN count(f)")
        assert result.result_set[0][0] == 1

        # deleted.py removed
        result = graph_client.query("MATCH (f:File {path: 'deleted.py'}) RETURN count(f)")
        assert result.result_set[0][0] == 0


@pytest.mark.integration
class TestExpiredFacts:
    def test_removes_expired_facts(self, graph_client):
        from synapcode.graph.episodic import EpisodicMemory, TemporalFact

        memory = EpisodicMemory(graph_client)
        memory.ensure_schema()

        # Create a fact that expired in the past
        fact = TemporalFact(
            subject="OldLib",
            predicate="version",
            object="1.0",
            valid_from=datetime(2024, 1, 1, tzinfo=timezone.utc),
        )
        memory.add_fact(fact)
        # Manually set valid_to to a past date
        memory.invalidate_fact(
            "OldLib", "version", "1.0",
            invalidated_at=datetime(2025, 1, 1, tzinfo=timezone.utc),
        )

        gc = GraphGarbageCollector(graph_client)
        removed = gc.collect_expired_facts()
        assert removed >= 1


@pytest.mark.integration
class TestFullGC:
    def test_runs_all_passes_and_returns_report(self, graph_client, tmp_path):
        repo = tmp_path / "gc_full"
        repo.mkdir()

        # Create some orphan data
        graph_client.query("CREATE (:Function {name: 'junk'})")

        gc = GraphGarbageCollector(graph_client)
        report = gc.run_full_gc(str(repo))

        assert report.duration_ms >= 0
        assert report.orphan_nodes_removed >= 1

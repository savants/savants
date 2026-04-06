"""Tests for graph garbage collection."""

from __future__ import annotations

from unittest.mock import MagicMock

from synapcode.graph.gc import GraphGarbageCollector


def _mock_result(rows):
    result = MagicMock()
    result.result_set = rows
    return result


class TestOrphanCollection:
    def test_removes_orphan_nodes(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([[5]])
        gc = GraphGarbageCollector(mock_graph_client)
        count = gc.collect_orphan_nodes()

        assert count == 5
        cypher = mock_graph_client.query.call_args[0][0]
        assert "DELETE" in cypher
        assert "NOT (n)--()" in cypher

    def test_zero_orphans(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([[0]])
        gc = GraphGarbageCollector(mock_graph_client)
        assert gc.collect_orphan_nodes() == 0


class TestStaleFileCollection:
    def test_removes_files_not_on_disk(self, mock_graph_client, tmp_path):
        # Create one real file
        (tmp_path / "exists.py").write_text("pass")

        mock_graph_client.query.side_effect = [
            _mock_result([["exists.py"], ["deleted.py"]]),  # list files
            _mock_result([]),  # delete edges for deleted.py
            _mock_result([]),  # delete node for deleted.py
        ]
        gc = GraphGarbageCollector(mock_graph_client)
        count = gc.collect_stale_files(str(tmp_path))

        assert count == 1  # only deleted.py was stale


class TestExpiredFacts:
    def test_removes_expired(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([[3]])
        gc = GraphGarbageCollector(mock_graph_client)
        count = gc.collect_expired_facts()

        assert count == 3
        cypher = mock_graph_client.query.call_args[0][0]
        assert "DELETE" in cypher
        assert "valid_to" in cypher


class TestContradictionResolution:
    def test_resolves_contradictions(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([[2]])
        gc = GraphGarbageCollector(mock_graph_client)
        count = gc.collect_contradictions()

        assert count == 2
        cypher = mock_graph_client.query.call_args[0][0]
        assert "SET" in cypher  # Should invalidate, not delete


class TestFullGC:
    def test_runs_all_passes(self, mock_graph_client, tmp_path):
        mock_graph_client.query.return_value = _mock_result([[0]])
        gc = GraphGarbageCollector(mock_graph_client)
        report = gc.run_full_gc(str(tmp_path))

        assert report.duration_ms >= 0
        assert mock_graph_client.query.call_count >= 3  # At least orphan + stale + expired + contradictions

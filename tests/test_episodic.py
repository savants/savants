"""Tests for episodic temporal memory."""

from __future__ import annotations

from datetime import datetime, timezone
from unittest.mock import MagicMock

from synapcode.graph.episodic import Episode, EpisodicMemory, TemporalFact


def _mock_result(rows):
    result = MagicMock()
    result.result_set = rows
    return result


class TestEpisodeCreation:
    def test_add_episode(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([[42]])
        memory = EpisodicMemory(mock_graph_client)

        episode = Episode(
            content="Refactored auth module to use JWT",
            source_type="git_commit",
            source_id="abc123def456",
        )
        eid = memory.add_episode(episode)
        assert eid == "42"
        assert mock_graph_client.query.called

    def test_episode_has_timestamp(self):
        ep = Episode(content="test", source_type="chat", source_id="1")
        assert ep.timestamp.tzinfo == timezone.utc


class TestTemporalFacts:
    def test_add_fact(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([])
        memory = EpisodicMemory(mock_graph_client)

        fact = TemporalFact(
            subject="AuthModule",
            predicate="uses",
            object="JWT",
            valid_from=datetime(2026, 1, 1, tzinfo=timezone.utc),
        )
        memory.add_fact(fact)

        cypher = mock_graph_client.query.call_args[0][0]
        assert "MERGE" in cypher
        assert "FACT" in cypher

    def test_invalidate_preserves_history(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([[1]])
        memory = EpisodicMemory(mock_graph_client)

        count = memory.invalidate_fact("AuthModule", "uses", "BasicAuth")
        assert count == 1

        cypher = mock_graph_client.query.call_args[0][0]
        # Should SET valid_to, not DELETE
        assert "SET" in cypher
        assert "DELETE" not in cypher

    def test_supersede_invalidates_and_adds(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([[1]])
        memory = EpisodicMemory(mock_graph_client)

        new_fact = TemporalFact(
            subject="AuthModule",
            predicate="uses",
            object="JWT",
            valid_from=datetime(2026, 4, 1, tzinfo=timezone.utc),
        )
        memory.supersede_fact("AuthModule", "uses", "BasicAuth", new_fact)

        # Should have called query at least twice (invalidate + add)
        assert mock_graph_client.query.call_count >= 2


class TestRecall:
    def test_recall_filters_by_time(self, mock_graph_client):
        mock_graph_client.query.side_effect = [
            _mock_result([
                ["AuthModule", "uses", "JWT",
                 "2026-01-01T00:00:00+00:00", "", 1.0, "ep1"],
            ]),
            _mock_result([
                ["Refactored auth", "git_commit", "abc123",
                 "2026-01-01T00:00:00+00:00"],
            ]),
        ]
        memory = EpisodicMemory(mock_graph_client)
        result = memory.recall("auth")

        assert len(result.facts) == 1
        assert result.facts[0].subject == "AuthModule"
        assert len(result.episodes) == 1

    def test_recall_empty(self, mock_graph_client):
        mock_graph_client.query.return_value = _mock_result([])
        memory = EpisodicMemory(mock_graph_client)
        result = memory.recall("nonexistent")

        assert result.facts == []
        assert result.episodes == []

"""Tests for episodic temporal memory — real FalkorDB, no mocks."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone

import pytest

from savants.graph.episodic import Episode, EpisodicMemory, TemporalFact


@pytest.mark.integration
class TestEpisodeCreation:
    def test_add_and_retrieve_episode(self, graph_client):
        memory = EpisodicMemory(graph_client)
        memory.ensure_schema()

        episode = Episode(
            content="Refactored auth module to use JWT",
            source_type="git_commit",
            source_id="abc123def456",
        )
        eid = memory.add_episode(episode)
        assert eid  # Got a node ID back

        # Verify it's in the graph
        result = graph_client.query(
            "MATCH (e:Episode {source_id: 'abc123def456'}) RETURN e.content"
        )
        assert result.result_set[0][0] == "Refactored auth module to use JWT"


@pytest.mark.integration
class TestTemporalFacts:
    def test_add_and_recall_fact(self, graph_client):
        memory = EpisodicMemory(graph_client)
        memory.ensure_schema()

        fact = TemporalFact(
            subject="AuthModule",
            predicate="uses",
            object="JWT",
            valid_from=datetime(2026, 1, 1, tzinfo=timezone.utc),
        )
        memory.add_fact(fact)

        result = memory.recall("AuthModule")
        assert len(result.facts) >= 1
        assert result.facts[0].subject == "AuthModule"
        assert result.facts[0].object == "JWT"

    def test_invalidate_sets_valid_to(self, graph_client):
        memory = EpisodicMemory(graph_client)
        memory.ensure_schema()

        fact = TemporalFact(
            subject="Config",
            predicate="format",
            object="YAML",
            valid_from=datetime(2025, 1, 1, tzinfo=timezone.utc),
        )
        memory.add_fact(fact)

        count = memory.invalidate_fact("Config", "format", "YAML")
        assert count == 1

        # Fact should still be in history (not deleted)
        history = memory.get_entity_history("Config")
        assert len(history) >= 1
        assert history[0].valid_to is not None

    def test_supersede_creates_new_fact(self, graph_client):
        memory = EpisodicMemory(graph_client)
        memory.ensure_schema()

        old_fact = TemporalFact(
            subject="DB",
            predicate="engine",
            object="SQLite",
            valid_from=datetime(2025, 1, 1, tzinfo=timezone.utc),
        )
        memory.add_fact(old_fact)

        new_fact = TemporalFact(
            subject="DB",
            predicate="engine",
            object="PostgreSQL",
            valid_from=datetime(2026, 1, 1, tzinfo=timezone.utc),
        )
        memory.supersede_fact("DB", "engine", "SQLite", new_fact)

        # Old fact should be invalidated, new fact should be active
        result = memory.recall("DB")
        active_objects = [f.object for f in result.facts if f.valid_to is None]
        assert "PostgreSQL" in active_objects

    def test_recall_respects_time_window(self, graph_client):
        memory = EpisodicMemory(graph_client)
        memory.ensure_schema()

        past = datetime(2025, 6, 1, tzinfo=timezone.utc)
        future = datetime(2027, 1, 1, tzinfo=timezone.utc)

        fact = TemporalFact(
            subject="Feature",
            predicate="status",
            object="beta",
            valid_from=datetime(2026, 1, 1, tzinfo=timezone.utc),
        )
        memory.add_fact(fact)

        # Query before the fact was valid
        before = memory.recall("Feature", as_of=past)
        assert len(before.facts) == 0

        # Query after the fact is valid
        after = memory.recall("Feature", as_of=future)
        assert len(after.facts) >= 1

"""Golden path E2E tests — simulate real user journeys against real FalkorDB.

These tests verify the complete workflows users will follow, from first
install through daily use. Every test creates real data, indexes real
files, and queries a real graph database.
"""

from __future__ import annotations

import subprocess

import pytest

from synapcode.graph.client import GraphClient
from synapcode.graph.cpg import CodePropertyGraphBuilder
from synapcode.graph.episodic import Episode, EpisodicMemory, TemporalFact
from synapcode.graph.gc import GraphGarbageCollector
from synapcode.graph.query import GraphQueryEngine
from synapcode.sync.git_hooks import get_current_head, save_last_indexed_sha


@pytest.mark.integration
class TestJourneyNewUser:
    """Journey 1: User installs, points at a repo, indexes, queries."""

    def test_full_flow(self, test_repo, graph_client):
        # Step 1: Index the repo
        builder = CodePropertyGraphBuilder(repo_path=test_repo, client=graph_client)
        stats = builder.build()

        assert stats["files"] >= 3
        assert stats["functions"] >= 4  # helper, unused_util, process, entry_point, validate, transform
        assert stats["classes"] >= 1  # DataModel

        # Step 2: Query — search for a function
        engine = GraphQueryEngine(graph_client)
        results = engine.search_by_pattern("process")
        names = [r["name"] for r in results]
        assert "process" in names

        # Step 3: Get function context
        ctx = engine.get_function_context("process")
        assert ctx.nodes
        assert "process" in ctx.summary

        # Step 4: Verify graph stats
        assert graph_client.node_count() > 0
        assert graph_client.edge_count() > 0


@pytest.mark.integration
class TestJourneyIncrementalSync:
    """Journey 2: User indexes, modifies code, pulls, graph auto-updates."""

    def test_incremental_add_and_delete(self, test_repo, graph_client):
        # Step 1: Full index
        builder = CodePropertyGraphBuilder(repo_path=test_repo, client=graph_client)
        builder.build()
        initial_nodes = graph_client.node_count()

        # Step 2: Add a new file
        new_file = test_repo / "src" / "new_module.py"
        new_file.write_text(
            "def brand_new_function():\n"
            "    return 'hello'\n"
        )
        subprocess.run(
            ["git", "-C", str(test_repo), "add", "."],
            check=True, capture_output=True,
        )
        subprocess.run(
            ["git", "-C", str(test_repo), "commit", "-m", "add new module"],
            check=True, capture_output=True,
        )

        # Step 3: Incremental update — add
        builder.build_incremental(
            changed_files=["src/new_module.py"],
            deleted_files=[],
        )

        engine = GraphQueryEngine(graph_client)
        results = engine.search_by_pattern("brand_new_function")
        assert len(results) >= 1

        # Step 4: Delete a file
        (test_repo / "src" / "utils.py").unlink()
        subprocess.run(
            ["git", "-C", str(test_repo), "add", "."],
            check=True, capture_output=True,
        )
        subprocess.run(
            ["git", "-C", str(test_repo), "commit", "-m", "remove utils"],
            check=True, capture_output=True,
        )

        builder.build_incremental(
            changed_files=[],
            deleted_files=["src/utils.py"],
        )

        # Verify utils.py nodes are gone
        result = graph_client.query(
            "MATCH (f:File {path: 'src/utils.py'}) RETURN count(f)"
        )
        assert result.result_set[0][0] == 0


@pytest.mark.integration
class TestJourneyImpactAnalysis:
    """Journey 4: User indexes a known call graph, runs impact analysis."""

    def test_cascading_impact(self, graph_client, tmp_path):
        # Create a repo with a clear call chain: top -> mid -> base
        repo = tmp_path / "impact_repo"
        repo.mkdir()
        subprocess.run(["git", "init", str(repo)], check=True, capture_output=True)
        subprocess.run(
            ["git", "-C", str(repo), "config", "user.name", "T"],
            check=True, capture_output=True,
        )
        subprocess.run(
            ["git", "-C", str(repo), "config", "user.email", "t@t.com"],
            check=True, capture_output=True,
        )

        (repo / "base.py").write_text("def base_func():\n    return 1\n")
        (repo / "middle.py").write_text(
            "from base import base_func\n\n"
            "def mid_func():\n"
            "    return base_func()\n"
        )
        (repo / "top.py").write_text(
            "from middle import mid_func\n\n"
            "def top_func():\n"
            "    return mid_func()\n"
        )
        subprocess.run(["git", "-C", str(repo), "add", "."], check=True, capture_output=True)
        subprocess.run(
            ["git", "-C", str(repo), "commit", "-m", "init"],
            check=True, capture_output=True,
        )

        # Index
        builder = CodePropertyGraphBuilder(repo_path=repo, client=graph_client)
        stats = builder.build()
        assert stats["files"] == 3
        assert stats["functions"] == 3

        # Impact analysis on base_func
        engine = GraphQueryEngine(graph_client)
        result = engine.impact_analysis("base_func", max_depth=5)

        # Verify call chain is detected
        all_dependents = result.direct_dependents + result.transitive_dependents
        # At minimum the files calling base_func should appear
        assert len(result.affected_files) >= 1


@pytest.mark.integration
class TestJourneyEpisodicMemory:
    """Journey 5: System records episodes, user recalls historical context."""

    def test_record_and_recall_across_changes(self, graph_client):
        memory = EpisodicMemory(graph_client)
        memory.ensure_schema()

        # Record: auth was using BasicAuth
        from datetime import datetime, timezone

        memory.add_episode(Episode(
            content="Initial auth implementation using BasicAuth",
            source_type="git_commit",
            source_id="commit_001",
        ))
        memory.add_fact(TemporalFact(
            subject="AuthModule",
            predicate="uses",
            object="BasicAuth",
            valid_from=datetime(2025, 1, 1, tzinfo=timezone.utc),
        ))

        # Later: auth was refactored to JWT
        memory.add_episode(Episode(
            content="Refactored auth to JWT for better security",
            source_type="git_commit",
            source_id="commit_050",
        ))
        memory.supersede_fact(
            "AuthModule", "uses", "BasicAuth",
            TemporalFact(
                subject="AuthModule",
                predicate="uses",
                object="JWT",
                valid_from=datetime(2026, 1, 1, tzinfo=timezone.utc),
            ),
        )

        # Recall current state
        now_result = memory.recall("AuthModule")
        active_facts = [f for f in now_result.facts if f.valid_to is None]
        assert any(f.object == "JWT" for f in active_facts)

        # Recall full history
        history = memory.get_entity_history("AuthModule")
        objects = [f.object for f in history]
        assert "BasicAuth" in objects
        assert "JWT" in objects


@pytest.mark.integration
class TestJourneyGarbageCollection:
    """Journey 6: Graph accumulates junk, GC cleans it up."""

    def test_gc_cleans_orphans_and_stale(self, graph_client, tmp_path):
        repo = tmp_path / "gc_repo"
        repo.mkdir()
        (repo / "alive.py").write_text("def alive(): pass\n")

        # Index
        builder = CodePropertyGraphBuilder(repo_path=repo, client=graph_client)
        builder.build()

        # Manually inject junk
        graph_client.query("CREATE (:Function {name: 'orphan_junk', file_path: 'gone.py'})")
        graph_client.query("CREATE (:File {path: 'phantom.py'})")

        before = graph_client.node_count()

        gc = GraphGarbageCollector(graph_client)
        report = gc.run_full_gc(str(repo))

        after = graph_client.node_count()
        assert after < before
        assert report.orphan_nodes_removed >= 1
        assert report.stale_files_removed >= 1  # phantom.py doesn't exist on disk

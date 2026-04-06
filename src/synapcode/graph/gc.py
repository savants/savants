"""Graph garbage collection: prune stale, orphan, and contradictory nodes.

Without GC, graphs rot — deleted files linger as phantom nodes, expired
episodic facts accumulate, and users lose trust in query results.
Designed to run as a scheduled Temporal workflow (daily at 3 AM).
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from synapcode.graph.client import GraphClient

logger = logging.getLogger(__name__)


@dataclass
class GCReport:
    orphan_nodes_removed: int
    stale_files_removed: int
    expired_facts_removed: int
    contradictions_resolved: int
    duration_ms: float


class GraphGarbageCollector:

    def __init__(self, client: GraphClient | None = None):
        self.client = client or GraphClient()

    def collect_orphan_nodes(self) -> int:
        """Remove nodes with no edges (except File nodes which are roots)."""
        result = self.client.query(
            "MATCH (n) "
            "WHERE NOT (n)--() AND NOT n:File AND NOT n:Episode "
            "DELETE n "
            "RETURN count(n) AS removed"
        )
        count = result.result_set[0][0] if result.result_set else 0
        logger.info("Removed %d orphan nodes", count)
        return count

    def collect_stale_files(self, repo_path: str | Path) -> int:
        """Remove File nodes for files that no longer exist on disk."""
        repo = Path(repo_path).resolve()

        result = self.client.query("MATCH (f:File) RETURN f.path")
        paths_in_graph = [row[0] for row in result.result_set]

        stale = [p for p in paths_in_graph if not (repo / p).exists()]

        for rel_path in stale:
            # Delete the file and all its children
            self.client.query(
                "MATCH (f:File {path: $path})-[r]->(n) DELETE r, n",
                {"path": rel_path},
            )
            self.client.query(
                "MATCH (f:File {path: $path}) DELETE f",
                {"path": rel_path},
            )

        logger.info("Removed %d stale file nodes", len(stale))
        return len(stale)

    def collect_expired_facts(self, as_of: datetime | None = None) -> int:
        """Remove episodic facts whose valid_to has passed.

        Facts past their validity window are noise — they've already been
        superseded and their history was preserved at invalidation time.
        """
        ts = (as_of or datetime.now(timezone.utc)).isoformat()
        result = self.client.query(
            "MATCH (s:Entity)-[r:FACT]->(o:Entity) "
            "WHERE r.valid_to <> '' AND r.valid_to < $ts "
            "DELETE r "
            "RETURN count(r) AS removed",
            {"ts": ts},
        )
        count = result.result_set[0][0] if result.result_set else 0
        logger.info("Removed %d expired facts", count)
        return count

    def collect_contradictions(self) -> int:
        """Find facts where a newer fact exists for the same subject+predicate.

        If entity A has two active FACT edges with the same predicate,
        the older one is invalidated (not deleted).
        """
        ts = datetime.now(timezone.utc).isoformat()
        result = self.client.query(
            "MATCH (s:Entity)-[r1:FACT]->(o1:Entity), "
            "      (s)-[r2:FACT]->(o2:Entity) "
            "WHERE r1.predicate = r2.predicate "
            "AND r1.valid_to = '' AND r2.valid_to = '' "
            "AND r1.valid_from < r2.valid_from "
            "AND ID(r1) <> ID(r2) "
            "SET r1.valid_to = $ts "
            "RETURN count(r1) AS resolved",
            {"ts": ts},
        )
        count = result.result_set[0][0] if result.result_set else 0
        logger.info("Resolved %d contradictory facts", count)
        return count

    def run_full_gc(self, repo_path: str | Path) -> GCReport:
        """Run all garbage collection passes and return a report."""
        import time

        start = time.monotonic()

        orphans = self.collect_orphan_nodes()
        stale = self.collect_stale_files(repo_path)
        expired = self.collect_expired_facts()
        contradictions = self.collect_contradictions()

        duration = (time.monotonic() - start) * 1000

        report = GCReport(
            orphan_nodes_removed=orphans,
            stale_files_removed=stale,
            expired_facts_removed=expired,
            contradictions_resolved=contradictions,
            duration_ms=duration,
        )
        logger.info("GC complete in %.1fms: %s", duration, report)
        return report

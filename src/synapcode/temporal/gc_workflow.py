"""Graph Garbage Collection Temporal workflow.

Designed to run on a schedule (e.g., daily at 3 AM) to keep the graph
lean and accurate. Without this, graphs rot and users lose trust.

Schedule with:
    temporal schedule create --schedule-id gc-daily \
        --cron "0 3 * * *" \
        --workflow-type GraphGCWorkflow \
        --task-queue synapcode-tasks \
        --input '{"repo_path": "/path/to/repo"}'
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import timedelta

from temporalio import activity, workflow

with workflow.unsafe.imports_passed_through():
    from synapcode.graph.gc import GCReport, GraphGarbageCollector


@dataclass
class GCInput:
    repo_path: str


@activity.defn
async def run_gc_activity(input: GCInput) -> dict:
    """Temporal activity: run full garbage collection pass."""
    gc = GraphGarbageCollector()
    report = gc.run_full_gc(input.repo_path)
    return {
        "orphan_nodes_removed": report.orphan_nodes_removed,
        "stale_files_removed": report.stale_files_removed,
        "expired_facts_removed": report.expired_facts_removed,
        "contradictions_resolved": report.contradictions_resolved,
        "duration_ms": report.duration_ms,
    }


@workflow.defn
class GraphGCWorkflow:
    """Scheduled workflow that cleans up the Code Property Graph."""

    @workflow.run
    async def run(self, input: GCInput) -> dict:
        return await workflow.execute_activity(
            run_gc_activity,
            input,
            start_to_close_timeout=timedelta(minutes=10),
            retry_policy=workflow.RetryPolicy(maximum_attempts=2),
        )

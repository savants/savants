"""Temporal workflows: deterministic orchestration logic.

Workflows coordinate activities into durable, crash-proof sequences.
All state is event-sourced — if the worker crashes, Temporal replays
the event history to reconstruct the workflow's progress.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from datetime import timedelta

from temporalio import workflow

with workflow.unsafe.imports_passed_through():
    from savants.temporal.activities import (
        GitDiffInput,
        GitDiffOutput,
        GraphSnapshotInput,
        IndexFileInput,
        IndexFileOutput,
        call_llm,
        compute_git_diff,
        index_file,
        remove_file_from_graph,
        snapshot_graph,
        LLMRequestInput,
    )

logger = logging.getLogger(__name__)


@dataclass
class FullIndexInput:
    repo_path: str
    commit_sha: str
    author: str
    file_paths: list[str]


@dataclass
class FullIndexOutput:
    total_files: int
    successful: int
    failed: int
    functions_total: int
    classes_total: int


@dataclass
class IncrementalSyncInput:
    repo_path: str
    from_sha: str  # last indexed commit (the "bookmark")
    to_sha: str  # new HEAD after git pull
    author: str


@dataclass
class IncrementalSyncOutput:
    changed: int
    deleted: int
    added: int
    errors: int


@dataclass
class AgentTaskInput:
    query: str
    graph_context: str
    model: str
    api_url: str
    api_key: str = ""


@workflow.defn
class FullIndexWorkflow:
    """Index an entire repository into the Code Property Graph.

    Processes files one at a time with durable progress tracking.
    If the worker crashes at file 500/1000, it resumes at file 501.
    """

    @workflow.run
    async def run(self, input: FullIndexInput) -> FullIndexOutput:
        stats = {"successful": 0, "failed": 0, "functions": 0, "classes": 0}

        for file_path in input.file_paths:
            result: IndexFileOutput = await workflow.execute_activity(
                index_file,
                IndexFileInput(
                    repo_path=input.repo_path,
                    file_path=file_path,
                    commit_sha=input.commit_sha,
                    author=input.author,
                ),
                start_to_close_timeout=timedelta(seconds=30),
                retry_policy=workflow.RetryPolicy(
                    maximum_attempts=3,
                    initial_interval=timedelta(seconds=1),
                ),
            )
            if result.success:
                stats["successful"] += 1
                stats["functions"] += result.functions_count
                stats["classes"] += result.classes_count
            else:
                stats["failed"] += 1

        return FullIndexOutput(
            total_files=len(input.file_paths),
            successful=stats["successful"],
            failed=stats["failed"],
            functions_total=stats["functions"],
            classes_total=stats["classes"],
        )


@workflow.defn
class IncrementalSyncWorkflow:
    """Sync the graph after a git pull using diff-based incremental updates.

    Triggered by the post-merge git hook. Calculates the file diff,
    removes deleted file nodes, and re-parses changed/added files.
    """

    @workflow.run
    async def run(self, input: IncrementalSyncInput) -> IncrementalSyncOutput:
        # Step 1: Compute git diff between bookmarked SHA and new HEAD
        diff: GitDiffOutput = await workflow.execute_activity(
            compute_git_diff,
            GitDiffInput(
                repo_path=input.repo_path,
                from_sha=input.from_sha,
                to_sha=input.to_sha,
            ),
            start_to_close_timeout=timedelta(seconds=30),
        )

        errors = 0

        # Step 2: Remove deleted files from graph
        for file_path in diff.deleted_files:
            success = await workflow.execute_activity(
                remove_file_from_graph,
                file_path,
                start_to_close_timeout=timedelta(seconds=10),
            )
            if not success:
                errors += 1

        # Step 3: Re-index changed and added files
        files_to_index = diff.changed_files + diff.added_files
        for file_path in files_to_index:
            # Remove old version first
            await workflow.execute_activity(
                remove_file_from_graph,
                file_path,
                start_to_close_timeout=timedelta(seconds=10),
            )

            # Re-index
            result: IndexFileOutput = await workflow.execute_activity(
                index_file,
                IndexFileInput(
                    repo_path=input.repo_path,
                    file_path=file_path,
                    commit_sha=input.to_sha,
                    author=input.author,
                ),
                start_to_close_timeout=timedelta(seconds=30),
                retry_policy=workflow.RetryPolicy(maximum_attempts=3),
            )
            if not result.success:
                errors += 1

        return IncrementalSyncOutput(
            changed=len(diff.changed_files),
            deleted=len(diff.deleted_files),
            added=len(diff.added_files),
            errors=errors,
        )


@workflow.defn
class SnapshotWorkflow:
    """Create a durable graph snapshot for Git LFS bootstrap or backup."""

    @workflow.run
    async def run(self, graph_name: str, output_path: str) -> str:
        return await workflow.execute_activity(
            snapshot_graph,
            GraphSnapshotInput(
                graph_name=graph_name,
                output_path=output_path,
            ),
            start_to_close_timeout=timedelta(minutes=5),
        )


@workflow.defn
class AgentReasoningWorkflow:
    """Durable agent workflow: query the graph, then call an LLM with context.

    Combines GraphRAG retrieval with frontier model reasoning.
    Survives API timeouts and retries automatically.
    """

    @workflow.run
    async def run(self, input: AgentTaskInput) -> str:
        # Build a prompt with graph context injected
        augmented_prompt = (
            f"You have access to the following code architecture context:\n\n"
            f"{input.graph_context}\n\n"
            f"User query: {input.query}\n\n"
            f"Provide a detailed, accurate answer based on the structural context above."
        )

        result = await workflow.execute_activity(
            call_llm,
            LLMRequestInput(
                prompt=augmented_prompt,
                model=input.model,
                api_url=input.api_url,
                api_key=input.api_key,
            ),
            start_to_close_timeout=timedelta(minutes=2),
            retry_policy=workflow.RetryPolicy(
                maximum_attempts=3,
                initial_interval=timedelta(seconds=5),
                backoff_coefficient=2.0,
            ),
        )

        if result.success:
            return result.content
        else:
            return f"Agent reasoning failed: {result.error}"

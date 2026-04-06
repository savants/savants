"""Post-merge git hook integration.

After a successful `git pull`, this triggers an incremental sync workflow
in Temporal to update the Code Property Graph with only the changed files.
"""

from __future__ import annotations

import asyncio
import logging
import subprocess
from pathlib import Path

from temporalio.client import Client

from synapcode.config import load_config
from synapcode.temporal.workflows import IncrementalSyncInput, IncrementalSyncWorkflow

logger = logging.getLogger(__name__)


def get_last_indexed_sha(repo_path: str) -> str:
    """Read the last indexed commit SHA from the bookmark file."""
    bookmark = Path(repo_path) / ".synapcode" / "last_indexed_sha"
    if bookmark.exists():
        return bookmark.read_text().strip()
    return ""


def save_last_indexed_sha(repo_path: str, sha: str) -> None:
    """Persist the last indexed commit SHA."""
    bookmark_dir = Path(repo_path) / ".synapcode"
    bookmark_dir.mkdir(exist_ok=True)
    (bookmark_dir / "last_indexed_sha").write_text(sha)


def get_current_head(repo_path: str) -> str:
    """Get the current HEAD commit SHA."""
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=repo_path,
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


def get_current_author(repo_path: str) -> str:
    """Get the current git user."""
    result = subprocess.run(
        ["git", "config", "user.name"],
        cwd=repo_path,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip() or "unknown"


async def trigger_incremental_sync(repo_path: str) -> str:
    """Trigger an incremental sync workflow in Temporal.

    Called by the post-merge git hook. Returns the workflow run ID.
    """
    config = load_config()

    from_sha = get_last_indexed_sha(repo_path)
    to_sha = get_current_head(repo_path)
    author = get_current_author(repo_path)

    if not from_sha:
        logger.warning(
            "No bookmark found. Run a full index first, or set "
            ".synapcode/last_indexed_sha to the initial commit."
        )
        return ""

    if from_sha == to_sha:
        logger.info("Graph is already up to date at %s", to_sha[:8])
        return ""

    client = await Client.connect(config.temporal.host)

    handle = await client.start_workflow(
        IncrementalSyncWorkflow.run,
        IncrementalSyncInput(
            repo_path=repo_path,
            from_sha=from_sha,
            to_sha=to_sha,
            author=author,
        ),
        id=f"sync-{to_sha[:8]}",
        task_queue=config.temporal.task_queue,
    )

    logger.info("Started incremental sync workflow: %s", handle.id)

    # Update the bookmark
    save_last_indexed_sha(repo_path, to_sha)

    return handle.id


def post_merge_hook_main() -> None:
    """Entry point for the post-merge git hook script."""
    import os

    repo_path = os.getcwd()
    logging.basicConfig(level=logging.INFO)

    logger.info("Post-merge hook triggered in %s", repo_path)
    run_id = asyncio.run(trigger_incremental_sync(repo_path))

    if run_id:
        logger.info("Incremental sync started: %s", run_id)
    else:
        logger.info("No sync needed")

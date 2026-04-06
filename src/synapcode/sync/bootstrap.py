"""Git LFS Bootstrap: serialize/restore graph snapshots for team onboarding.

Instead of re-indexing from scratch, new developers can restore a graph
snapshot checked into Git LFS and be instantly productive.
"""

from __future__ import annotations

import logging
import subprocess
from pathlib import Path

from synapcode.graph.client import GraphClient
from synapcode.config import FalkorDBConfig

logger = logging.getLogger(__name__)

SNAPSHOT_FILENAME = ".synapcode/graph.dump"
LFS_TRACK_PATTERN = ".synapcode/*.dump"


def setup_lfs_tracking(repo_path: str) -> None:
    """Configure Git LFS to track graph snapshot files."""
    subprocess.run(
        ["git", "lfs", "track", LFS_TRACK_PATTERN],
        cwd=repo_path,
        check=True,
    )
    logger.info("Git LFS tracking configured for %s", LFS_TRACK_PATTERN)


def create_snapshot(repo_path: str, graph_name: str = "synapcode") -> Path:
    """Create a graph snapshot and save it to the repo for Git LFS.

    The snapshot is a serialized FalkorDB dump that can be restored
    on any machine to instantly bootstrap the Code Property Graph.
    """
    config = FalkorDBConfig(graph_name=graph_name)
    client = GraphClient(config)

    output_path = Path(repo_path) / SNAPSHOT_FILENAME
    output_path.parent.mkdir(exist_ok=True)

    data = client.dump_graph()
    output_path.write_bytes(data)

    logger.info(
        "Graph snapshot created: %s (%d bytes)",
        output_path,
        len(data),
    )
    return output_path


def restore_snapshot(repo_path: str, graph_name: str = "synapcode") -> bool:
    """Restore a graph from a Git LFS snapshot.

    Called when a new developer clones the repo and wants instant
    access to the Code Property Graph without re-indexing.
    """
    snapshot_path = Path(repo_path) / SNAPSHOT_FILENAME

    if not snapshot_path.exists():
        logger.warning("No snapshot found at %s", snapshot_path)
        return False

    config = FalkorDBConfig(graph_name=graph_name)
    client = GraphClient(config)

    data = snapshot_path.read_bytes()
    client.restore_graph(data)

    node_count = client.node_count()
    logger.info(
        "Graph restored from snapshot: %d nodes loaded",
        node_count,
    )
    return True


def snapshot_exists(repo_path: str) -> bool:
    """Check if a graph snapshot exists in the repo."""
    return (Path(repo_path) / SNAPSHOT_FILENAME).exists()

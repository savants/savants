"""Git diff utilities shared by CLI and Temporal activities."""

from __future__ import annotations

import subprocess
from dataclasses import dataclass


@dataclass
class DiffResult:
    changed_files: list[str]
    deleted_files: list[str]
    added_files: list[str]


def compute_diff(repo_path: str, from_sha: str, to_sha: str) -> DiffResult:
    """Compute file-level diff between two git commits."""
    result = subprocess.run(
        ["git", "diff", "--name-status", from_sha, to_sha],
        cwd=repo_path,
        capture_output=True,
        text=True,
        check=True,
    )

    changed: list[str] = []
    deleted: list[str] = []
    added: list[str] = []

    for line in result.stdout.strip().split("\n"):
        if not line:
            continue
        parts = line.split("\t", 1)
        status = parts[0]
        filepath = parts[1] if len(parts) > 1 else ""

        if status.startswith("D"):
            deleted.append(filepath)
        elif status.startswith("A"):
            added.append(filepath)
        elif status.startswith("M") or status.startswith("R"):
            changed.append(filepath)

    return DiffResult(changed_files=changed, deleted_files=deleted, added_files=added)

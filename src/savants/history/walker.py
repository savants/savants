"""GitHistoryWalker — turn git log into Episode nodes + CHANGES edges.

This is the input pipeline that populates the Layer 2 history overlay
described in docs/architecture-layered-graphs.md.

Workflow:
    1. Walk git log oldest-to-newest (or filtered by --since)
    2. For each commit, compute the file diff against its parent
    3. For each changed file:
       a. Get the file content at parent SHA and at this SHA
       b. Run the local delta computer to produce a Delta
    4. For each commit, create:
       - One Episode node (sha, author, timestamp, message, branch)
       - One CHANGES edge per affected Function/Class/File node

Usage:
    walker = GitHistoryWalker(
        repo_path="/path/to/repo",
        client=GraphClient(),
        branch="main",
        since="6 months ago",
    )
    result = walker.walk()
    print(f"Indexed {result.commits_processed} commits")
"""

from __future__ import annotations

import logging
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

from savants.delta.computer import compute_file_delta
from savants.delta.schema import AddEdge, AddNode, RemoveEdge, RemoveNode, UpdateNode
from savants.graph.client import GraphClient
from savants.graph.schema import (
    ClassNode,
    EpisodeNode,
    FileNode,
    FunctionNode,
    create_changes_edge_query,
    create_class_query,
    create_episode_query,
    create_file_query,
    create_function_query,
)

logger = logging.getLogger(__name__)


@dataclass
class CommitInfo:
    sha: str
    parent_sha: str | None
    author_name: str
    author_email: str
    timestamp: str  # ISO8601
    subject: str
    changed_files: list[str] = field(default_factory=list)


@dataclass
class HistoryWalkResult:
    commits_processed: int = 0
    commits_failed: int = 0
    episodes_created: int = 0
    changes_edges_created: int = 0
    files_diffed: int = 0
    duration_s: float = 0.0


class GitHistoryWalker:
    """Walks git history and populates the Episode/CHANGES history overlay."""

    def __init__(
        self,
        repo_path: str | Path,
        client: GraphClient | None = None,
        branch: str = "main",
        since: str | None = None,
        max_commits: int | None = None,
    ):
        self.repo_path = Path(repo_path).resolve()
        self.client = client or GraphClient()
        self.branch = branch
        self.since = since
        self.max_commits = max_commits

    # --- Git plumbing ----------------------------------------------------

    def _git(self, *args: str) -> str:
        """Run a git command in the repo and return stdout."""
        result = subprocess.run(
            ["git", *args],
            cwd=self.repo_path,
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            raise RuntimeError(
                f"git {' '.join(args)} failed: {result.stderr.strip()}"
            )
        return result.stdout

    def list_commits(self) -> list[CommitInfo]:
        """List commits in chronological (oldest-first) order."""
        # Format: sha|parent|author|email|iso|subject
        sep = "\x1f"  # ASCII unit separator (won't appear in normal text)
        rec = "\x1e"  # ASCII record separator
        fmt = sep.join(["%H", "%P", "%an", "%ae", "%aI", "%s"]) + rec
        args = ["log", "--reverse", f"--format={fmt}", self.branch]
        if self.since:
            args.append(f"--since={self.since}")
        if self.max_commits:
            args.append(f"-n{self.max_commits}")

        out = self._git(*args)
        commits: list[CommitInfo] = []
        for record in out.split(rec):
            record = record.strip()
            if not record:
                continue
            parts = record.split(sep)
            if len(parts) < 6:
                continue
            sha, parent, author, email, ts, subject = parts[:6]
            parent_sha = parent.split()[0] if parent.strip() else None
            commits.append(
                CommitInfo(
                    sha=sha,
                    parent_sha=parent_sha,
                    author_name=author,
                    author_email=email,
                    timestamp=ts,
                    subject=subject,
                )
            )
        return commits

    def files_changed_in(self, commit: CommitInfo) -> list[str]:
        """Return the list of file paths changed by a commit."""
        if commit.parent_sha is None:
            # Root commit — every file is "added"
            out = self._git("ls-tree", "-r", "--name-only", commit.sha)
        else:
            out = self._git(
                "diff", "--name-only", commit.parent_sha, commit.sha
            )
        return [line for line in out.splitlines() if line.strip()]

    def file_content_at(self, sha: str, file_path: str) -> str | None:
        """Return file contents at a given commit, or None if missing/binary."""
        try:
            result = subprocess.run(
                ["git", "show", f"{sha}:{file_path}"],
                cwd=self.repo_path,
                capture_output=True,
                check=False,
            )
            if result.returncode != 0:
                return None
            try:
                return result.stdout.decode("utf-8")
            except UnicodeDecodeError:
                return None  # binary or non-utf-8
        except Exception:
            return None

    # --- Graph writes ---------------------------------------------------

    def write_episode(self, commit: CommitInfo) -> None:
        """Insert an Episode node for a commit."""
        episode = EpisodeNode(
            sha=commit.sha,
            source_type="git_commit",
            timestamp=commit.timestamp,
            author=f"{commit.author_name} <{commit.author_email}>",
            message=commit.subject,
            branch=self.branch,
        )
        cypher, params = create_episode_query(episode)
        self.client.query(cypher, params)

    def apply_delta_to_layer1(self, delta_ops: list) -> None:
        """Apply add/remove ops to the current-state graph (Layer 1).

        Layer 1 must exist for CHANGES edges to attach to. We materialize
        each touched File/Function/Class as we walk so the CHANGES edges
        in write_changes_for_delta can MATCH a real target.
        """
        for op in delta_ops:
            try:
                if isinstance(op, AddNode):
                    label = op.label
                    props = op.properties
                    if label == "File":
                        node = FileNode(
                            path=props.get("file_path") or props.get("path", ""),
                            language=props.get("language", ""),
                            line_count=props.get("line_count", 0),
                            sha256=props.get("sha256", ""),
                        )
                        cypher, p = create_file_query(node)
                        self.client.query(cypher, p)
                    elif label == "Function":
                        node = FunctionNode(
                            name=props.get("name", ""),
                            file_path=props.get("file_path", ""),
                            start_line=props.get("start_line", 0),
                            end_line=props.get("end_line", 0),
                            parameters=props.get("parameters", []),
                            return_type=props.get("return_type", ""),
                        )
                        cypher, p = create_function_query(node)
                        self.client.query(cypher, p)
                    elif label == "Class":
                        node = ClassNode(
                            name=props.get("name", ""),
                            file_path=props.get("file_path", ""),
                            start_line=props.get("start_line", 0),
                            end_line=props.get("end_line", 0),
                            bases=props.get("bases", []),
                        )
                        cypher, p = create_class_query(node)
                        self.client.query(cypher, p)

                # Note: we deliberately do NOT process RemoveNode here.
                # Removing from Layer 1 would also DETACH DELETE the historical
                # CHANGES edges we just attached. Instead, Layer 1 accumulates
                # everything that ever existed. Queries that want only current
                # HEAD state should filter by the latest CHANGES op per node.
            except Exception as e:
                logger.debug("Layer1 apply failed for %r: %s", op, e)
                continue

    def write_changes_for_delta(
        self,
        episode_sha: str,
        delta_ops: list,
    ) -> int:
        """Convert a delta's operations into CHANGES edges. Returns count created."""
        edges_created = 0
        for op in delta_ops:
            try:
                # Map delta operations to CHANGES edges
                if isinstance(op, AddNode):
                    label = op.label
                    name = op.properties.get("name") or op.id.split(":")[-1]
                    file_path = op.properties.get("file_path")
                    cypher, params = create_changes_edge_query(
                        episode_sha=episode_sha,
                        target_label=label,
                        target_key="name" if label != "File" else "path",
                        target_val=name if label != "File" else (file_path or name),
                        op="add",
                        after_props=op.properties,
                        file_path=file_path,
                    )
                    self.client.query(cypher, params)
                    edges_created += 1

                elif isinstance(op, RemoveNode):
                    parts = op.id.split(":")
                    label_short = parts[0]
                    label = {
                        "f": "File",
                        "fn": "Function",
                        "c": "Class",
                        "m": "Module",
                        "v": "Variable",
                    }.get(label_short)
                    if label is None:
                        continue
                    if label == "File":
                        target_key = "path"
                        target_val = ":".join(parts[1:])
                        file_path = None
                    else:
                        target_key = "name"
                        target_val = parts[-1]
                        file_path = ":".join(parts[1:-1]) if len(parts) > 2 else None

                    cypher, params = create_changes_edge_query(
                        episode_sha=episode_sha,
                        target_label=label,
                        target_key=target_key,
                        target_val=target_val,
                        op="remove",
                        file_path=file_path,
                    )
                    self.client.query(cypher, params)
                    edges_created += 1

                elif isinstance(op, UpdateNode):
                    # UpdateNode → CHANGES with op="modify"
                    parts = op.id.split(":")
                    if len(parts) < 2:
                        continue
                    label_short = parts[0]
                    label = {
                        "f": "File",
                        "fn": "Function",
                        "c": "Class",
                    }.get(label_short)
                    if label is None:
                        continue
                    if label == "File":
                        target_key = "path"
                        target_val = ":".join(parts[1:])
                        file_path = None
                    else:
                        target_key = "name"
                        target_val = parts[-1]
                        file_path = ":".join(parts[1:-1]) if len(parts) > 2 else None

                    cypher, params = create_changes_edge_query(
                        episode_sha=episode_sha,
                        target_label=label,
                        target_key=target_key,
                        target_val=target_val,
                        op="modify",
                        after_props=op.set,
                        file_path=file_path,
                    )
                    self.client.query(cypher, params)
                    edges_created += 1

                # AddEdge / RemoveEdge are not currently emitted as CHANGES
                # because the historical caller relationship is implicit
                # (callers exist when their function exists). Could be added
                # later if needed.

            except Exception as e:
                logger.debug("Skipping CHANGES op %r: %s", op, e)
                continue

        return edges_created

    # --- Main loop ------------------------------------------------------

    def walk(self) -> HistoryWalkResult:
        """Walk the configured commit range and populate the history layer."""
        import time

        self.client.ensure_schema()
        result = HistoryWalkResult()
        start = time.monotonic()

        commits = self.list_commits()
        logger.info(
            "Walking %d commits on branch %s%s",
            len(commits),
            self.branch,
            f" since {self.since}" if self.since else "",
        )

        for commit in commits:
            try:
                changed = self.files_changed_in(commit)
                commit.changed_files = changed
                self.write_episode(commit)
                result.episodes_created += 1

                for fp in changed:
                    # Skip non-source / excluded files
                    suffix = Path(fp).suffix
                    if suffix not in (".py", ".js", ".ts", ".tsx", ".jsx"):
                        continue
                    if any(p in (".git", "node_modules", "__pycache__", ".venv")
                           for p in Path(fp).parts):
                        continue

                    before = (
                        self.file_content_at(commit.parent_sha, fp)
                        if commit.parent_sha
                        else None
                    )
                    after = self.file_content_at(commit.sha, fp)

                    delta = compute_file_delta(
                        file_path=fp,
                        before_content=before,
                        after_content=after,
                        org="local",
                        repo=self.repo_path.name,
                        branch=self.branch,
                        base_sha=commit.parent_sha,
                        head_sha=commit.sha,
                        author=commit.author_name,
                    )

                    # Step 1: materialize the delta in Layer 1 (adds only).
                    # Layer 1 accumulates union of everything ever seen so
                    # CHANGES edges have a target to MATCH against.
                    self.apply_delta_to_layer1(delta.operations)

                    # Step 2: write CHANGES edges with MATCH targeting the
                    # nodes we just materialized in Layer 1.
                    edges = self.write_changes_for_delta(commit.sha, delta.operations)
                    result.changes_edges_created += edges
                    result.files_diffed += 1

                result.commits_processed += 1
                if result.commits_processed % 50 == 0:
                    logger.info(
                        "  ... %d / %d commits processed",
                        result.commits_processed,
                        len(commits),
                    )

            except Exception as e:
                logger.warning("Commit %s failed: %s", commit.sha[:8], e)
                result.commits_failed += 1
                continue

        result.duration_s = round(time.monotonic() - start, 2)
        logger.info(
            "History walk complete: %d commits, %d episodes, %d edges, %.1fs",
            result.commits_processed,
            result.episodes_created,
            result.changes_edges_created,
            result.duration_s,
        )
        return result

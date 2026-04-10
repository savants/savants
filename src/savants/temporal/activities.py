"""Temporal activities: non-deterministic I/O operations.

Activities handle all side effects: FalkorDB queries, LLM API calls,
file system operations, and git commands. They are automatically retried
on failure.
"""

from __future__ import annotations

import logging
import subprocess
from dataclasses import dataclass
from pathlib import Path

from temporalio import activity

logger = logging.getLogger(__name__)


@dataclass
class IndexFileInput:
    repo_path: str
    file_path: str  # relative to repo_path
    commit_sha: str
    author: str


@dataclass
class IndexFileOutput:
    file_path: str
    functions_count: int
    classes_count: int
    success: bool
    error: str = ""


@dataclass
class GitDiffInput:
    repo_path: str
    from_sha: str
    to_sha: str


@dataclass
class GitDiffOutput:
    changed_files: list[str]
    deleted_files: list[str]
    added_files: list[str]


@dataclass
class LLMRequestInput:
    prompt: str
    model: str
    api_url: str
    api_key: str = ""
    max_tokens: int = 4096


@dataclass
class LLMRequestOutput:
    content: str
    model: str
    tokens_used: int
    success: bool
    error: str = ""


@dataclass
class GraphQueryInput:
    cypher: str
    params: dict


@dataclass
class GraphSnapshotInput:
    graph_name: str
    output_path: str


@activity.defn
async def index_file(input: IndexFileInput) -> IndexFileOutput:
    """Parse a single file and update its nodes in FalkorDB."""
    from savants.graph.client import GraphClient
    from savants.graph.cpg import CodePropertyGraphBuilder

    try:
        client = GraphClient()
        builder = CodePropertyGraphBuilder(
            repo_path=input.repo_path,
            client=client,
            commit_sha=input.commit_sha,
            author=input.author,
        )
        full_path = Path(input.repo_path) / input.file_path
        parsed = builder.parse_file(full_path)

        fn_count = len(parsed.get("functions", []))
        cls_count = len(parsed.get("classes", []))

        # Upsert into graph
        from savants.graph.schema import (
            create_file_query,
            create_function_query,
            create_class_query,
            create_edge_query,
        )

        cypher, params = create_file_query(parsed["file"])
        client.query(cypher, params)

        for fn in parsed["functions"]:
            cypher, params = create_function_query(fn)
            client.query(cypher, params)
            cypher, params = create_edge_query(
                "File", "path", fn.file_path,
                "Function", "name", fn.name,
                "CONTAINS",
            )
            client.query(cypher, params)

        for cls in parsed["classes"]:
            cypher, params = create_class_query(cls)
            client.query(cypher, params)
            cypher, params = create_edge_query(
                "File", "path", cls.file_path,
                "Class", "name", cls.name,
                "CONTAINS",
            )
            client.query(cypher, params)

        return IndexFileOutput(
            file_path=input.file_path,
            functions_count=fn_count,
            classes_count=cls_count,
            success=True,
        )
    except Exception as e:
        logger.error("Failed to index %s: %s", input.file_path, e)
        return IndexFileOutput(
            file_path=input.file_path,
            functions_count=0,
            classes_count=0,
            success=False,
            error=str(e),
        )


@activity.defn
async def compute_git_diff(input: GitDiffInput) -> GitDiffOutput:
    """Compute the file diff between two git commits."""
    try:
        result = subprocess.run(
            ["git", "diff", "--name-status", input.from_sha, input.to_sha],
            cwd=input.repo_path,
            capture_output=True,
            text=True,
            check=True,
        )

        changed = []
        deleted = []
        added = []

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

        return GitDiffOutput(
            changed_files=changed,
            deleted_files=deleted,
            added_files=added,
        )
    except subprocess.CalledProcessError as e:
        logger.error("Git diff failed: %s", e.stderr)
        return GitDiffOutput(changed_files=[], deleted_files=[], added_files=[])


@activity.defn
async def call_llm(input: LLMRequestInput) -> LLMRequestOutput:
    """Call an LLM API (local or frontier) and return the response."""
    import httpx

    try:
        headers = {"Content-Type": "application/json"}
        if input.api_key:
            headers["Authorization"] = f"Bearer {input.api_key}"

        payload = {
            "model": input.model,
            "messages": [{"role": "user", "content": input.prompt}],
            "max_tokens": input.max_tokens,
        }

        async with httpx.AsyncClient(timeout=120.0) as http:
            response = await http.post(
                f"{input.api_url}/v1/chat/completions",
                json=payload,
                headers=headers,
            )
            response.raise_for_status()
            data = response.json()

        content = data["choices"][0]["message"]["content"]
        usage = data.get("usage", {})
        return LLMRequestOutput(
            content=content,
            model=input.model,
            tokens_used=usage.get("total_tokens", 0),
            success=True,
        )
    except Exception as e:
        logger.error("LLM call failed: %s", e)
        return LLMRequestOutput(
            content="",
            model=input.model,
            tokens_used=0,
            success=False,
            error=str(e),
        )


@activity.defn
async def snapshot_graph(input: GraphSnapshotInput) -> str:
    """Create a serialized snapshot of the graph for Git LFS or backup."""
    from savants.graph.client import GraphClient
    from savants.config import FalkorDBConfig

    try:
        config = FalkorDBConfig(graph_name=input.graph_name)
        client = GraphClient(config)
        data = client.dump_graph()

        output = Path(input.output_path)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(data)

        logger.info("Graph snapshot saved to %s (%d bytes)", output, len(data))
        return str(output)
    except Exception as e:
        logger.error("Snapshot failed: %s", e)
        raise


@activity.defn
async def remove_file_from_graph(file_path: str) -> bool:
    """Remove a file and all its children from the graph."""
    from savants.graph.client import GraphClient

    try:
        client = GraphClient()
        client.query(
            "MATCH (f:File {path: $path})-[r]->(n) DELETE r, n",
            {"path": file_path},
        )
        client.query("MATCH (f:File {path: $path}) DELETE f", {"path": file_path})
        return True
    except Exception as e:
        logger.error("Failed to remove %s from graph: %s", file_path, e)
        return False

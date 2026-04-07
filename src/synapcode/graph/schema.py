"""Code Property Graph schema definitions for FalkorDB.

Layer 1 — current state (the live code property graph):

  Node types:
    - File: source file in the repository
    - Function: function/method definition
    - Class: class definition
    - Module: logical module or package
    - Variable: global/module-level variable or constant

  Edge types:
    - CONTAINS: File -> Function/Class/Variable
    - CALLS: Function -> Function
    - INHERITS_FROM: Class -> Class
    - IMPORTS: File -> File/Module
    - DEPENDS_ON: Module -> Module
    - DEFINES: Class -> Function (methods)
    - REFERENCES: Function -> Variable

Layer 2 — history (the time-travel overlay, see docs/architecture-layered-graphs.md):

  Node types:
    - Episode: a discrete event (git commit, chat message, agent action)

  Edge types:
    - CHANGES: Episode -> File/Function/Class
        properties: op ('add'|'remove'|'modify'|'rename'),
                    before_props, after_props
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime

# Cypher schema creation queries — both layers
SCHEMA_INDICES = [
    # Layer 1
    "CREATE INDEX FOR (f:File) ON (f.path)",
    "CREATE INDEX FOR (fn:Function) ON (fn.name)",
    "CREATE INDEX FOR (c:Class) ON (c.name)",
    "CREATE INDEX FOR (m:Module) ON (m.name)",
    # Layer 2 — history
    "CREATE INDEX FOR (e:Episode) ON (e.sha)",
    "CREATE INDEX FOR (e:Episode) ON (e.timestamp)",
    "CREATE INDEX FOR (e:Episode) ON (e.author)",
    "CREATE INDEX FOR (e:Episode) ON (e.branch)",
    "CREATE INDEX FOR (e:Episode) ON (e.source_type)",
]


@dataclass
class FileNode:
    path: str
    language: str
    line_count: int
    sha256: str  # provenance hash
    last_commit: str = ""


@dataclass
class FunctionNode:
    name: str
    file_path: str
    start_line: int
    end_line: int
    parameters: list[str] = field(default_factory=list)
    return_type: str = ""


@dataclass
class ClassNode:
    name: str
    file_path: str
    start_line: int
    end_line: int
    bases: list[str] = field(default_factory=list)


@dataclass
class ProvenanceStamp:
    """SHA-256 provenance attached to every graph entry."""

    source_commit: str
    author: str
    timestamp: str
    content_hash: str


def create_file_query(node: FileNode) -> tuple[str, dict]:
    return (
        "MERGE (f:File {path: $path}) "
        "SET f.language = $language, f.line_count = $line_count, "
        "f.sha256 = $sha256, f.last_commit = $last_commit",
        {
            "path": node.path,
            "language": node.language,
            "line_count": node.line_count,
            "sha256": node.sha256,
            "last_commit": node.last_commit,
        },
    )


def create_function_query(node: FunctionNode) -> tuple[str, dict]:
    return (
        "MERGE (fn:Function {name: $name, file_path: $file_path}) "
        "SET fn.start_line = $start_line, fn.end_line = $end_line, "
        "fn.parameters = $parameters, fn.return_type = $return_type",
        {
            "name": node.name,
            "file_path": node.file_path,
            "start_line": node.start_line,
            "end_line": node.end_line,
            "parameters": node.parameters,
            "return_type": node.return_type,
        },
    )


def create_class_query(node: ClassNode) -> tuple[str, dict]:
    return (
        "MERGE (c:Class {name: $name, file_path: $file_path}) "
        "SET c.start_line = $start_line, c.end_line = $end_line, "
        "c.bases = $bases",
        {
            "name": node.name,
            "file_path": node.file_path,
            "start_line": node.start_line,
            "end_line": node.end_line,
            "bases": node.bases,
        },
    )


def create_edge_query(
    from_label: str, from_key: str, from_val: str,
    to_label: str, to_key: str, to_val: str,
    edge_type: str,
) -> tuple[str, dict]:
    return (
        f"MATCH (a:{from_label} {{{from_key}: $from_val}}) "
        f"MATCH (b:{to_label} {{{to_key}: $to_val}}) "
        f"MERGE (a)-[:{edge_type}]->(b)",
        {"from_val": from_val, "to_val": to_val},
    )


# --- Layer 2: History (Episode + CHANGES) ------------------------------------


@dataclass
class EpisodeNode:
    """A discrete event in the history layer (commit, chat, agent action)."""

    sha: str  # commit SHA, message ID, or other unique identifier
    source_type: str = "git_commit"  # "git_commit" | "chat" | "agent_action" | ...
    timestamp: str = ""  # ISO8601 datetime string
    author: str = ""
    message: str = ""
    branch: str = "main"


@dataclass
class ChangeProps:
    """Properties on a CHANGES edge from an Episode to a Layer 1 node."""

    op: str  # "add" | "remove" | "modify" | "rename"
    before_props: dict | None = None  # state before this commit
    after_props: dict | None = None  # state after this commit


def create_episode_query(node: EpisodeNode) -> tuple[str, dict]:
    """MERGE an Episode node by SHA. SHA is the natural key."""
    return (
        "MERGE (e:Episode {sha: $sha}) "
        "SET e.source_type = $source_type, "
        "    e.timestamp = $timestamp, "
        "    e.author = $author, "
        "    e.message = $message, "
        "    e.branch = $branch",
        {
            "sha": node.sha,
            "source_type": node.source_type,
            "timestamp": node.timestamp,
            "author": node.author,
            "message": node.message,
            "branch": node.branch,
        },
    )


def create_changes_edge_query(
    episode_sha: str,
    target_label: str,
    target_key: str,
    target_val: str,
    op: str,
    before_props: dict | None = None,
    after_props: dict | None = None,
    file_path: str | None = None,
) -> tuple[str, dict]:
    """Create a CHANGES edge from an Episode to a Layer 1 node.

    For Function/Class targets, file_path is needed because their canonical
    identity is (name, file_path), not name alone.
    """
    import json

    if file_path and target_label in ("Function", "Class"):
        match_target = (
            f"MATCH (b:{target_label} {{{target_key}: $target_val, "
            f"file_path: $file_path}}) "
        )
    else:
        match_target = f"MATCH (b:{target_label} {{{target_key}: $target_val}}) "

    cypher = (
        "MATCH (e:Episode {sha: $episode_sha}) "
        + match_target
        + "MERGE (e)-[c:CHANGES]->(b) "
        "SET c.op = $op, "
        "    c.before_props = $before_props_json, "
        "    c.after_props = $after_props_json"
    )
    return (
        cypher,
        {
            "episode_sha": episode_sha,
            "target_val": target_val,
            "file_path": file_path or "",
            "op": op,
            "before_props_json": json.dumps(before_props or {}),
            "after_props_json": json.dumps(after_props or {}),
        },
    )

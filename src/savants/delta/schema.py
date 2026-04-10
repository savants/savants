"""Pydantic schema for the Graph Delta Protocol.

This is the canonical wire format for representing graph mutations sent
between the local client and the cloud, or between cloud layers (base,
overlay, working delta).

See docs/delta-protocol.md for the full specification.
"""

from __future__ import annotations

import json
from datetime import datetime
from typing import Any, Literal

from pydantic import BaseModel, Field

PROTOCOL_VERSION = "0.1"
SCHEMA_ID = "savants/delta/v0.1"

# Node label short codes used in canonical IDs
LABEL_SHORT = {
    "File": "f",
    "Function": "fn",
    "Class": "c",
    "Module": "m",
    "Variable": "v",
    "Episode": "ep",
    "Entity": "e",
}


def canonical_node_id(label: str, file_path: str | None = None, name: str | None = None) -> str:
    """Compute a deterministic ID for a node from its label and key properties.

    Examples:
        canonical_node_id("File", "src/main.py")
            -> "f:src/main.py"
        canonical_node_id("Function", "src/main.py", "process")
            -> "fn:src/main.py:process"
        canonical_node_id("Entity", name="JWT")
            -> "e:JWT"
    """
    short = LABEL_SHORT.get(label, label.lower())
    parts: list[str] = [short]
    if file_path:
        parts.append(file_path)
    if name:
        parts.append(name)
    return ":".join(parts)


def canonical_edge_id(from_id: str, to_id: str, edge_type: str) -> str:
    """Compute a deterministic ID for an edge from its endpoints and type."""
    return f"edge:{from_id}\u2192{to_id}:{edge_type}"


# --- Operation types ---


class AddNode(BaseModel):
    op: Literal["add_node"] = "add_node"
    id: str
    label: str
    properties: dict[str, Any] = Field(default_factory=dict)


class RemoveNode(BaseModel):
    op: Literal["remove_node"] = "remove_node"
    id: str


class UpdateNode(BaseModel):
    op: Literal["update_node"] = "update_node"
    id: str
    set: dict[str, Any] = Field(default_factory=dict)
    unset: list[str] = Field(default_factory=list)


class AddEdge(BaseModel):
    op: Literal["add_edge"] = "add_edge"
    id: str
    type: str
    from_id: str
    to_id: str
    properties: dict[str, Any] = Field(default_factory=dict)


class RemoveEdge(BaseModel):
    op: Literal["remove_edge"] = "remove_edge"
    id: str


Operation = AddNode | RemoveNode | UpdateNode | AddEdge | RemoveEdge


# --- Top-level structures ---


class DeltaScope(BaseModel):
    """Identifies which org/repo/branch/SHAs the delta applies to."""

    org: str
    repo: str
    branch: str = "main"
    base_sha: str | None = None
    head_sha: str | None = None


class Provenance(BaseModel):
    """Audit metadata for a delta."""

    author: str | None = None
    timestamp: datetime = Field(default_factory=datetime.utcnow)
    session_id: str | None = None


class Delta(BaseModel):
    """A graph delta describing a set of mutations to apply."""

    version: str = PROTOCOL_VERSION
    schema_id: str = SCHEMA_ID
    scope: DeltaScope
    provenance: Provenance | None = None
    operations: list[Operation] = Field(default_factory=list)

    def to_json(self) -> str:
        return self.model_dump_json(exclude_none=True)

    @classmethod
    def from_json(cls, data: str | bytes) -> "Delta":
        return cls.model_validate_json(data)

    def to_dict(self) -> dict[str, Any]:
        return self.model_dump(mode="json", exclude_none=True)

    def add_node(
        self,
        label: str,
        file_path: str | None = None,
        name: str | None = None,
        **properties: Any,
    ) -> "Delta":
        """Convenience: add an AddNode operation with computed canonical ID."""
        node_id = canonical_node_id(label, file_path, name)
        props: dict[str, Any] = {}
        if file_path:
            props["file_path"] = file_path
        if name:
            props["name"] = name
        props.update(properties)
        self.operations.append(AddNode(id=node_id, label=label, properties=props))
        return self

    def remove_node(
        self,
        label: str,
        file_path: str | None = None,
        name: str | None = None,
    ) -> "Delta":
        """Convenience: add a RemoveNode operation with computed canonical ID."""
        node_id = canonical_node_id(label, file_path, name)
        self.operations.append(RemoveNode(id=node_id))
        return self

    def add_edge(
        self,
        edge_type: str,
        from_label: str,
        from_file: str | None,
        from_name: str | None,
        to_label: str,
        to_file: str | None,
        to_name: str | None,
        **properties: Any,
    ) -> "Delta":
        """Convenience: add an AddEdge operation with computed canonical IDs."""
        from_id = canonical_node_id(from_label, from_file, from_name)
        to_id = canonical_node_id(to_label, to_file, to_name)
        edge_id = canonical_edge_id(from_id, to_id, edge_type)
        self.operations.append(
            AddEdge(
                id=edge_id,
                type=edge_type,
                from_id=from_id,
                to_id=to_id,
                properties=properties or {},
            )
        )
        return self

    def remove_edge(
        self,
        edge_type: str,
        from_label: str,
        from_file: str | None,
        from_name: str | None,
        to_label: str,
        to_file: str | None,
        to_name: str | None,
    ) -> "Delta":
        """Convenience: add a RemoveEdge operation with computed canonical IDs."""
        from_id = canonical_node_id(from_label, from_file, from_name)
        to_id = canonical_node_id(to_label, to_file, to_name)
        edge_id = canonical_edge_id(from_id, to_id, edge_type)
        self.operations.append(RemoveEdge(id=edge_id))
        return self

    def stats(self) -> dict[str, int]:
        """Summary of operations in this delta."""
        counts = {"add_node": 0, "remove_node": 0, "update_node": 0, "add_edge": 0, "remove_edge": 0}
        for op in self.operations:
            counts[op.op] = counts.get(op.op, 0) + 1
        counts["total"] = len(self.operations)
        return counts


__all__ = [
    "PROTOCOL_VERSION",
    "SCHEMA_ID",
    "Delta",
    "DeltaScope",
    "Provenance",
    "Operation",
    "AddNode",
    "RemoveNode",
    "UpdateNode",
    "AddEdge",
    "RemoveEdge",
    "canonical_node_id",
    "canonical_edge_id",
]

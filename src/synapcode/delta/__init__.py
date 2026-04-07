"""Graph delta protocol: wire format and computation for layered graphs.

See docs/delta-protocol.md and docs/architecture-layered-graphs.md for the
canonical specification.
"""

from synapcode.delta.schema import (
    AddEdge,
    AddNode,
    Delta,
    DeltaScope,
    Operation,
    Provenance,
    RemoveEdge,
    RemoveNode,
    UpdateNode,
    canonical_node_id,
    canonical_edge_id,
)

__all__ = [
    "AddEdge",
    "AddNode",
    "Delta",
    "DeltaScope",
    "Operation",
    "Provenance",
    "RemoveEdge",
    "RemoveNode",
    "UpdateNode",
    "canonical_node_id",
    "canonical_edge_id",
]

"""High-level graph query utilities for GraphRAG retrieval.

Provides multi-hop reasoning queries for the Code Property Graph.
"""

from __future__ import annotations

from dataclasses import dataclass

from synapcode.graph.client import GraphClient


@dataclass
class ImpactAnalysis:
    """Result of a cascading impact analysis."""

    target: str
    direct_dependents: list[str]
    transitive_dependents: list[str]
    affected_files: list[str]
    depth: int


@dataclass
class SubgraphContext:
    """A subgraph extracted for LLM context injection."""

    nodes: list[dict]
    edges: list[dict]
    summary: str


class GraphQueryEngine:
    """Executes multi-hop GraphRAG queries against the Code Property Graph."""

    def __init__(self, client: GraphClient | None = None):
        self.client = client or GraphClient()

    def impact_analysis(self, function_name: str, max_depth: int = 5) -> ImpactAnalysis:
        """Find all functions and files affected by changing a given function.

        Uses variable-length path traversal to find transitive dependencies.
        """
        # Direct callers
        direct_result = self.client.query(
            "MATCH (caller:Function)-[:CALLS]->(target:Function {name: $name}) "
            "RETURN caller.name, caller.file_path",
            {"name": function_name},
        )
        direct = [row[0] for row in direct_result.result_set]

        # Transitive callers (multi-hop)
        transitive_result = self.client.query(
            "MATCH path = (caller:Function)-[:CALLS*2..]->(target:Function {name: $name}) "
            f"WHERE length(path) <= {max_depth} "
            "RETURN DISTINCT caller.name, caller.file_path, length(path) AS depth "
            "ORDER BY depth",
            {"name": function_name},
        )
        transitive = [row[0] for row in transitive_result.result_set]

        # Affected files
        files_result = self.client.query(
            "MATCH (caller:Function)-[:CALLS*1..]->(target:Function {name: $name}) "
            "RETURN DISTINCT caller.file_path",
            {"name": function_name},
        )
        affected_files = [row[0] for row in files_result.result_set]

        return ImpactAnalysis(
            target=function_name,
            direct_dependents=direct,
            transitive_dependents=transitive,
            affected_files=affected_files,
            depth=max_depth,
        )

    def get_function_context(self, function_name: str) -> SubgraphContext:
        """Extract the local subgraph around a function for LLM context."""
        result = self.client.query(
            "MATCH (f:Function {name: $name}) "
            "OPTIONAL MATCH (f)-[r1]->(n1) "
            "OPTIONAL MATCH (n2)-[r2]->(f) "
            "RETURN f, type(r1), n1, type(r2), n2",
            {"name": function_name},
        )

        nodes = []
        edges = []
        seen = set()

        for row in result.result_set:
            fn_node = row[0]
            if fn_node and id(fn_node) not in seen:
                nodes.append({"type": "Function", "properties": fn_node.properties})
                seen.add(id(fn_node))

            if row[2] and id(row[2]) not in seen:
                nodes.append({"properties": row[2].properties})
                seen.add(id(row[2]))
            if row[1]:
                edges.append({"type": row[1], "direction": "outgoing"})

            if row[4] and id(row[4]) not in seen:
                nodes.append({"properties": row[4].properties})
                seen.add(id(row[4]))
            if row[3]:
                edges.append({"type": row[3], "direction": "incoming"})

        summary = (
            f"Function '{function_name}' has {len(nodes)} connected nodes "
            f"and {len(edges)} relationships."
        )
        return SubgraphContext(nodes=nodes, edges=edges, summary=summary)

    def find_dependency_chain(self, from_file: str, to_file: str) -> list[str]:
        """Find the shortest dependency path between two files."""
        result = self.client.query(
            "MATCH path = shortestPath("
            "(a:File {path: $from})-[:IMPORTS*]->(b:File {path: $to})"
            ") RETURN [n IN nodes(path) | n.path] AS chain",
            {"from": from_file, "to": to_file},
        )
        if result.result_set:
            return result.result_set[0][0]
        return []

    def community_summary(self, max_communities: int = 10) -> list[dict]:
        """Identify clusters of tightly connected modules (community detection).

        Uses degree centrality as a proxy for community hubs.
        """
        result = self.client.query(
            "MATCH (f:File)-[r]->() "
            "WITH f, count(r) AS degree "
            "ORDER BY degree DESC "
            f"LIMIT {max_communities} "
            "RETURN f.path, degree",
        )
        return [
            {"file": row[0], "connections": row[1]}
            for row in result.result_set
        ]

    def search_by_pattern(self, pattern: str) -> list[dict]:
        """Search for functions/classes/config keys matching a name pattern.

        Matches on the `name` property, which for ConfigKey is the dotted
        path (e.g. `operationProfiling.mode`). ConfigKey hits also return
        their stringified value so callers can see what's set without
        re-reading the file.
        """
        result = self.client.query(
            "MATCH (n) WHERE n.name CONTAINS $pattern "
            "RETURN labels(n)[0] AS type, n.name AS name, "
            "n.file_path AS file, "
            "CASE WHEN n:ConfigKey THEN n.value ELSE NULL END AS value "
            "ORDER BY type, name",
            {"pattern": pattern},
        )
        out: list[dict] = []
        for row in result.result_set:
            entry = {"type": row[0], "name": row[1], "file": row[2]}
            if row[3] is not None:
                entry["value"] = row[3]
            out.append(entry)
        return out

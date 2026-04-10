"""Reusable codebase analysis queries.

Each function takes a GraphClient and returns a list of typed dicts.
These are pure read queries — they never modify the graph.

The queries here produced the findings in docs/fastapi-analysis.md.
They are intentionally small and composable so the CLI, the MCP server,
and future agent tools can all share the same primitives.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from savants.graph.client import GraphClient


# --- Layer 1 (current state) queries ----------------------------------------


@dataclass
class CalleeRow:
    name: str
    file_path: str
    callers: int


def most_called(
    client: GraphClient,
    *,
    limit: int = 15,
    production_only: bool = False,
    src_prefix: str = "fastapi/",
    test_prefix: str = "tests/",
) -> list[CalleeRow]:
    """Find the most-called functions in the graph.

    When production_only=True, restrict to functions in src_prefix and
    exclude callers in test_prefix. Otherwise count all callers.
    """
    if production_only:
        cypher = (
            "MATCH (caller:Function)-[:CALLS]->(callee:Function) "
            f"WHERE NOT caller.file_path STARTS WITH '{test_prefix}' "
            f"  AND NOT callee.file_path STARTS WITH '{test_prefix}' "
            f"  AND callee.file_path STARTS WITH '{src_prefix}' "
            "RETURN callee.name, callee.file_path, count(caller) AS callers "
            "ORDER BY callers DESC "
            f"LIMIT {limit}"
        )
    else:
        cypher = (
            "MATCH (caller:Function)-[:CALLS]->(callee:Function) "
            "RETURN callee.name, callee.file_path, count(caller) AS callers "
            "ORDER BY callers DESC "
            f"LIMIT {limit}"
        )
    result = client.query(cypher)
    return [
        CalleeRow(name=row[0], file_path=row[1], callers=row[2])
        for row in result.result_set
    ]


@dataclass
class HubFile:
    path: str
    contained_count: int


def top_hubs(client: GraphClient, *, limit: int = 10) -> list[HubFile]:
    """Files containing the most functions/classes (size hubs)."""
    result = client.query(
        "MATCH (f:File)-[r:CONTAINS]->() "
        "RETURN f.path, count(r) AS contained "
        "ORDER BY contained DESC "
        f"LIMIT {limit}"
    )
    return [HubFile(path=row[0], contained_count=row[1]) for row in result.result_set]


@dataclass
class GodClass:
    name: str
    file_path: str
    methods: int


def god_classes(client: GraphClient, *, limit: int = 10) -> list[GodClass]:
    """Classes containing the most methods (potential god class candidates)."""
    result = client.query(
        "MATCH (c:Class) "
        "OPTIONAL MATCH (f:File {path: c.file_path})-[:CONTAINS]->(fn:Function) "
        "WHERE fn.start_line >= c.start_line AND fn.end_line <= c.end_line "
        "RETURN c.name, c.file_path, count(fn) AS methods "
        "ORDER BY methods DESC "
        f"LIMIT {limit}"
    )
    return [
        GodClass(name=row[0], file_path=row[1], methods=row[2])
        for row in result.result_set
    ]


@dataclass
class NameCollision:
    name: str
    file_count: int


def name_collisions(
    client: GraphClient,
    *,
    min_files: int = 5,
    limit: int = 10,
) -> list[NameCollision]:
    """Function names defined in min_files+ different files.

    High counts often indicate test scaffolding duplication or naming-convention
    issues that should be parameterized.
    """
    result = client.query(
        "MATCH (fn:Function) "
        "WITH fn.name AS name, count(DISTINCT fn.file_path) AS file_count "
        f"WHERE file_count >= {min_files} "
        "RETURN name, file_count "
        "ORDER BY file_count DESC "
        f"LIMIT {limit}"
    )
    return [NameCollision(name=row[0], file_count=row[1]) for row in result.result_set]


@dataclass
class FanOut:
    function_name: str
    file_path: str
    fan_out: int


def top_callers_per_file(
    client: GraphClient,
    *,
    src_prefix: str = "fastapi/",
    limit: int = 10,
) -> list[FanOut]:
    """Functions with the highest fan-out (call the most other functions)."""
    result = client.query(
        "MATCH (caller:Function)-[:CALLS]->(callee:Function) "
        f"WHERE caller.file_path STARTS WITH '{src_prefix}' "
        "RETURN caller.name, caller.file_path, count(callee) AS fan_out "
        "ORDER BY fan_out DESC "
        f"LIMIT {limit}"
    )
    return [
        FanOut(function_name=row[0], file_path=row[1], fan_out=row[2])
        for row in result.result_set
    ]


@dataclass
class LocationCount:
    location: str
    function_count: int


def test_to_source_ratio(
    client: GraphClient,
    *,
    src_prefix: str = "fastapi/",
    test_prefix: str = "tests/",
    docs_prefix: str = "docs/",
) -> list[LocationCount]:
    """Categorize functions into src/tests/docs/other and count them.

    The ratio of tests to source is a hidden quality signal.
    """
    cypher = f"""
    MATCH (fn:Function)
    WITH fn,
         CASE
           WHEN fn.file_path STARTS WITH '{test_prefix}' THEN 'tests'
           WHEN fn.file_path STARTS WITH '{src_prefix}' THEN 'src'
           WHEN fn.file_path STARTS WITH '{docs_prefix}' THEN 'docs'
           ELSE 'other'
         END AS location
    RETURN location, count(fn) AS function_count
    ORDER BY function_count DESC
    """
    result = client.query(cypher)
    return [
        LocationCount(location=row[0], function_count=row[1])
        for row in result.result_set
    ]


# --- Layer 2 (history) queries ----------------------------------------------


@dataclass
class ContributorRow:
    author: str
    commits: int


def top_contributors(client: GraphClient, *, limit: int = 5) -> list[ContributorRow]:
    """Top contributors by commit count across all walked history."""
    result = client.query(
        "MATCH (e:Episode) "
        "RETURN e.author, count(e) AS commits "
        "ORDER BY commits DESC "
        f"LIMIT {limit}"
    )
    return [ContributorRow(author=row[0], commits=row[1]) for row in result.result_set]


@dataclass
class BusFactorRow:
    author: str
    touches: int


def bus_factor(
    client: GraphClient,
    *,
    file_path: str,
    limit: int = 5,
) -> list[BusFactorRow]:
    """Who has actually touched a specific file recently? (bus factor signal)

    A file with one dominant author and zero substantive backups is in
    the red zone — losing that author creates a knowledge gap.
    """
    result = client.query(
        "MATCH (e:Episode)-[:CHANGES]->(f:File {path: $path}) "
        "RETURN e.author, count(e) AS touches "
        "ORDER BY touches DESC "
        f"LIMIT {limit}",
        {"path": file_path},
    )
    return [
        BusFactorRow(author=row[0], touches=row[1]) for row in result.result_set
    ]


@dataclass
class HotFile:
    path: str
    changes: int


def hot_files(
    client: GraphClient,
    *,
    src_prefix: str = "fastapi/",
    exclude_prefix: str | None = "fastapi/_compat/",
    limit: int = 15,
) -> list[HotFile]:
    """Most-changed files in the walked history (refactoring hotspots)."""
    where_clauses = [f"f.path STARTS WITH '{src_prefix}'"]
    if exclude_prefix:
        where_clauses.append(f"NOT f.path STARTS WITH '{exclude_prefix}'")
    where = " AND ".join(where_clauses)

    cypher = (
        "MATCH (f:File) "
        f"WHERE {where} "
        "OPTIONAL MATCH (e:Episode)-[c:CHANGES]->(f) "
        "WITH f.path AS path, count(c) AS changes "
        "RETURN path, changes "
        "ORDER BY changes DESC "
        f"LIMIT {limit}"
    )
    result = client.query(cypher)
    return [HotFile(path=row[0], changes=row[1]) for row in result.result_set]


@dataclass
class CoChangeRow:
    other_function: str
    co_changes: int


def co_change(
    client: GraphClient,
    *,
    function_name: str,
    limit: int = 10,
) -> list[CoChangeRow]:
    """Functions that historically change in the same commits as the target.

    Reveals hidden coupling that doesn't show up in the call graph alone.
    """
    cypher = (
        "MATCH (e:Episode)-[:CHANGES]->(fn1:Function {name: $name}) "
        "MATCH (e)-[:CHANGES]->(fn2:Function) "
        "WHERE fn1.name <> fn2.name "
        "RETURN fn2.name, count(e) AS co_changes "
        "ORDER BY co_changes DESC "
        f"LIMIT {limit}"
    )
    result = client.query(cypher, {"name": function_name})
    return [
        CoChangeRow(other_function=row[0], co_changes=row[1])
        for row in result.result_set
    ]


# --- Composite reports ------------------------------------------------------


def architectural_summary(
    client: GraphClient,
    *,
    src_prefix: str = "fastapi/",
) -> dict[str, Any]:
    """Run all the canonical analytical queries and return a summary dict.

    This is the function the CLI's `savants audit` command would call.
    """
    return {
        "most_called_overall": [vars(r) for r in most_called(client, limit=10)],
        "most_called_production": [
            vars(r) for r in most_called(client, limit=10, production_only=True, src_prefix=src_prefix)
        ],
        "test_source_ratio": [vars(r) for r in test_to_source_ratio(client, src_prefix=src_prefix)],
        "top_hubs": [vars(r) for r in top_hubs(client, limit=10)],
        "god_classes": [vars(r) for r in god_classes(client, limit=10)],
        "name_collisions": [vars(r) for r in name_collisions(client, limit=10)],
        "top_fan_out": [vars(r) for r in top_callers_per_file(client, src_prefix=src_prefix, limit=10)],
        "top_contributors": [vars(r) for r in top_contributors(client, limit=10)],
        "hot_files": [vars(r) for r in hot_files(client, src_prefix=src_prefix, limit=15)],
    }

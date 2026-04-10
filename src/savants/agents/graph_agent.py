"""Graph-aware PydanticAI agent with FalkorDB tool access.

Pre-configured with tools that query the Code Property Graph — impact
analysis, dependency chains, code search — so the LLM can reason
about codebase architecture, not just individual files.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from typing import Any

from pydantic import BaseModel
from pydantic_ai import Agent, RunContext

from savants.graph.client import GraphClient
from savants.graph.episodic import EpisodicMemory
from savants.graph.query import GraphQueryEngine

logger = logging.getLogger(__name__)


@dataclass
class GraphAgentDeps:
    """Dependencies injected into the graph agent's tools."""

    query_engine: GraphQueryEngine
    episodic_memory: EpisodicMemory
    repo_path: str = ""


class ImpactResult(BaseModel):
    target: str
    direct_dependents: list[str]
    transitive_dependents: list[str]
    affected_files: list[str]


class SearchResult(BaseModel):
    results: list[dict[str, str]]


graph_agent = Agent(
    "openai:gpt-4o",
    deps_type=GraphAgentDeps,
    system_prompt=(
        "You are SynapCode, a code intelligence agent with access to a "
        "knowledge graph of the user's codebase. You can analyze impact, "
        "trace dependencies, search code patterns, and recall historical "
        "context. Always use your tools before making claims about the code."
    ),
)


@graph_agent.tool
async def impact_analysis(
    ctx: RunContext[GraphAgentDeps],
    function_name: str,
    max_depth: int = 5,
) -> str:
    """Analyze cascading impact of changing a function."""
    result = ctx.deps.query_engine.impact_analysis(function_name, max_depth)
    return (
        f"Impact for '{result.target}':\n"
        f"  Direct dependents ({len(result.direct_dependents)}): "
        f"{', '.join(result.direct_dependents[:20])}\n"
        f"  Transitive ({len(result.transitive_dependents)}): "
        f"{', '.join(result.transitive_dependents[:20])}\n"
        f"  Affected files ({len(result.affected_files)}): "
        f"{', '.join(result.affected_files[:20])}"
    )


@graph_agent.tool
async def search_code(
    ctx: RunContext[GraphAgentDeps],
    pattern: str,
) -> str:
    """Search for functions and classes by name pattern."""
    results = ctx.deps.query_engine.search_by_pattern(pattern)
    if not results:
        return f"No results found for '{pattern}'"
    lines = [f"  {r['type']}: {r['name']} ({r['file']})" for r in results[:25]]
    return f"Found {len(results)} matches:\n" + "\n".join(lines)


@graph_agent.tool
async def dependency_chain(
    ctx: RunContext[GraphAgentDeps],
    from_file: str,
    to_file: str,
) -> str:
    """Find the shortest dependency path between two files."""
    chain = ctx.deps.query_engine.find_dependency_chain(from_file, to_file)
    if chain:
        return f"Dependency chain: {' -> '.join(chain)}"
    return f"No dependency path found between {from_file} and {to_file}"


@graph_agent.tool
async def architecture_overview(
    ctx: RunContext[GraphAgentDeps],
    max_hubs: int = 10,
) -> str:
    """Get the most connected hub files (architectural overview)."""
    summary = ctx.deps.query_engine.community_summary(max_hubs)
    if not summary:
        return "No community data available yet."
    lines = [f"  {s['file']}: {s['connections']} connections" for s in summary]
    return "Top hub files:\n" + "\n".join(lines)


@graph_agent.tool
async def recall_history(
    ctx: RunContext[GraphAgentDeps],
    query: str,
) -> str:
    """Recall historical facts and episodes from episodic memory."""
    result = ctx.deps.episodic_memory.recall(query)
    parts = []
    if result.facts:
        parts.append(f"Facts ({len(result.facts)}):")
        for f in result.facts[:10]:
            validity = f"since {f.valid_from.date()}"
            if f.valid_to:
                validity += f" until {f.valid_to.date()}"
            parts.append(f"  {f.subject} -{f.predicate}-> {f.object} ({validity})")
    if result.episodes:
        parts.append(f"\nEpisodes ({len(result.episodes)}):")
        for e in result.episodes[:10]:
            parts.append(f"  [{e.source_type}] {e.content[:100]}")
    return "\n".join(parts) if parts else "No relevant history found."


def create_graph_agent(
    client: GraphClient | None = None,
    model: str = "openai:gpt-4o",
) -> tuple[Agent, GraphAgentDeps]:
    """Factory: create a graph agent with its dependencies."""
    c = client or GraphClient()
    deps = GraphAgentDeps(
        query_engine=GraphQueryEngine(c),
        episodic_memory=EpisodicMemory(c),
    )
    return graph_agent, deps

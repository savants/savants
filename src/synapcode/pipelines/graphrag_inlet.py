"""GraphRAG Inlet Filter for Open WebUI Pipelines.

This inlet filter intercepts user messages before they reach the LLM,
queries FalkorDB for relevant structural context, and injects it into
the prompt. This transforms a generic chat into a graph-aware conversation.
"""

from __future__ import annotations

import logging
import re

from synapcode.graph.client import GraphClient
from synapcode.graph.query import GraphQueryEngine

logger = logging.getLogger(__name__)


def extract_code_references(message: str) -> dict:
    """Extract function names, class names, and file paths from a message."""
    refs: dict = {"functions": [], "classes": [], "files": []}

    # Match function-like references: function_name(), ClassName.method()
    fn_pattern = r"\b([a-zA-Z_]\w*)\s*\("
    refs["functions"] = list(set(re.findall(fn_pattern, message)))

    # Match class-like references: PascalCase words
    class_pattern = r"\b([A-Z][a-zA-Z0-9]*(?:[A-Z][a-zA-Z0-9]*)+)\b"
    refs["classes"] = list(set(re.findall(class_pattern, message)))

    # Match file paths
    file_pattern = r"[\w./\\-]+\.(?:py|js|ts|tsx|go|rs|java)\b"
    refs["files"] = list(set(re.findall(file_pattern, message)))

    return refs


def build_graph_context(
    message: str,
    client: GraphClient | None = None,
    max_context_tokens: int = 2000,
) -> str:
    """Query FalkorDB for structural context relevant to the user's message.

    Returns a formatted string suitable for injection into the LLM prompt.
    """
    engine = GraphQueryEngine(client or GraphClient())
    refs = extract_code_references(message)
    context_parts = []

    # Fetch context for referenced functions
    for fn_name in refs["functions"][:5]:  # limit to avoid context overflow
        try:
            subgraph = engine.get_function_context(fn_name)
            if subgraph.nodes:
                context_parts.append(f"### Function: {fn_name}")
                context_parts.append(subgraph.summary)
                for node in subgraph.nodes[:10]:
                    props = node.get("properties", {})
                    context_parts.append(f"  - {props}")
        except Exception:
            pass

    # Fetch impact analysis if the message suggests refactoring
    impact_keywords = ["change", "modify", "refactor", "rename", "impact", "affect"]
    if any(kw in message.lower() for kw in impact_keywords):
        for fn_name in refs["functions"][:3]:
            try:
                impact = engine.impact_analysis(fn_name, max_depth=3)
                if impact.direct_dependents:
                    context_parts.append(f"\n### Impact Analysis: {fn_name}")
                    context_parts.append(
                        f"Direct dependents: {', '.join(impact.direct_dependents[:10])}"
                    )
                    context_parts.append(
                        f"Affected files: {', '.join(impact.affected_files[:10])}"
                    )
            except Exception:
                pass

    # Fetch dependency chains between referenced files
    if len(refs["files"]) >= 2:
        try:
            chain = engine.find_dependency_chain(refs["files"][0], refs["files"][1])
            if chain:
                context_parts.append(
                    f"\n### Dependency Chain: {refs['files'][0]} -> {refs['files'][1]}"
                )
                context_parts.append(" -> ".join(chain))
        except Exception:
            pass

    if not context_parts:
        return ""

    context = "\n".join(context_parts)
    # Rough token estimation (4 chars per token)
    if len(context) > max_context_tokens * 4:
        context = context[: max_context_tokens * 4] + "\n... (truncated)"

    return f"<graph_context>\n{context}\n</graph_context>"


def inject_context(messages: list[dict], client: GraphClient | None = None) -> list[dict]:
    """Open WebUI inlet: inject graph context into the conversation.

    Modifies the last user message to include structural context
    from FalkorDB before it reaches the LLM.
    """
    if not messages:
        return messages

    # Find last user message
    for i in range(len(messages) - 1, -1, -1):
        if messages[i].get("role") == "user":
            user_msg = messages[i]["content"]
            context = build_graph_context(user_msg, client)

            if context:
                messages[i]["content"] = f"{context}\n\n{user_msg}"
                logger.info("Injected %d chars of graph context", len(context))
            break

    return messages

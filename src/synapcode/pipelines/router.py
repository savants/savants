"""Selective Deferred Routing pipeline.

Evaluates request complexity and routes to local SLMs for simple tasks
or frontier APIs for complex reasoning. Cuts compute costs by up to 70%.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from enum import Enum

logger = logging.getLogger(__name__)


class RouteDecision(Enum):
    LOCAL = "local"
    FRONTIER = "frontier"


@dataclass
class RoutingResult:
    decision: RouteDecision
    model: str
    api_url: str
    reason: str


# Heuristic signals that suggest a complex query
COMPLEXITY_SIGNALS = [
    "refactor",
    "impact",
    "cascading",
    "architecture",
    "dependency",
    "migrate",
    "redesign",
    "security audit",
    "explain why",
    "trade-off",
    "multi-step",
]

# Signals for simple/routine tasks
SIMPLE_SIGNALS = [
    "summarize",
    "format",
    "rename",
    "unit test",
    "docstring",
    "type hint",
    "lint",
    "translate",
]


def estimate_complexity(query: str) -> float:
    """Score query complexity from 0.0 (trivial) to 1.0 (highly complex).

    Uses keyword heuristics. In production, this would be replaced by
    a lightweight classifier or the local SLM's self-assessment.
    """
    query_lower = query.lower()
    score = 0.5  # baseline

    for signal in COMPLEXITY_SIGNALS:
        if signal in query_lower:
            score += 0.15

    for signal in SIMPLE_SIGNALS:
        if signal in query_lower:
            score -= 0.15

    # Long queries with code blocks tend to be more complex
    if len(query) > 2000:
        score += 0.1
    if "```" in query:
        score += 0.05

    return max(0.0, min(1.0, score))


def route_request(
    query: str,
    local_model: str = "qwen2.5-coder:14b",
    local_url: str = "http://localhost:11434",
    frontier_model: str = "claude-sonnet-4-20250514",
    frontier_url: str = "https://api.anthropic.com",
    complexity_threshold: float = 0.7,
    local_ram_pct: float = 40.0,
    ram_threshold: float = 60.0,
) -> RoutingResult:
    """Decide whether to route a request to a local SLM or frontier API.

    Args:
        query: The user's request text.
        local_model: Ollama model identifier for local inference.
        local_url: Local inference server URL.
        frontier_model: Cloud model identifier.
        frontier_url: Cloud API URL.
        complexity_threshold: Score above which we route to frontier.
        local_ram_pct: Current RAM usage percentage.
        ram_threshold: RAM % above which we force frontier routing.
    """
    complexity = estimate_complexity(query)

    # Force frontier if local hardware is overloaded (60% rule)
    if local_ram_pct >= ram_threshold:
        return RoutingResult(
            decision=RouteDecision.FRONTIER,
            model=frontier_model,
            api_url=frontier_url,
            reason=f"Local RAM at {local_ram_pct:.0f}% (threshold: {ram_threshold:.0f}%)",
        )

    if complexity >= complexity_threshold:
        return RoutingResult(
            decision=RouteDecision.FRONTIER,
            model=frontier_model,
            api_url=frontier_url,
            reason=f"High complexity score: {complexity:.2f}",
        )

    return RoutingResult(
        decision=RouteDecision.LOCAL,
        model=local_model,
        api_url=local_url,
        reason=f"Low complexity score: {complexity:.2f}",
    )

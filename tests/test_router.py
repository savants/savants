"""Tests for the selective deferred routing pipeline."""

from savants.pipelines.router import (
    RouteDecision,
    estimate_complexity,
    route_request,
)


def test_simple_query_routes_locally():
    result = route_request(
        query="summarize this function",
        local_ram_pct=30.0,
    )
    assert result.decision == RouteDecision.LOCAL


def test_complex_query_routes_to_frontier():
    result = route_request(
        query="analyze the cascading impact of refactoring the authentication architecture",
        complexity_threshold=0.7,
        local_ram_pct=30.0,
    )
    assert result.decision == RouteDecision.FRONTIER


def test_high_ram_forces_frontier():
    result = route_request(
        query="summarize this function",
        local_ram_pct=75.0,
        ram_threshold=60.0,
    )
    assert result.decision == RouteDecision.FRONTIER
    assert "RAM" in result.reason


def test_complexity_scoring():
    simple = estimate_complexity("format this code")
    complex_ = estimate_complexity("refactor the architecture to handle cascading dependency changes")
    assert complex_ > simple


def test_complexity_bounds():
    score = estimate_complexity("a" * 5000 + " refactor impact cascading")
    assert 0.0 <= score <= 1.0

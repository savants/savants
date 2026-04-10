"""Tests for hardware monitoring."""

from savants.hardware.monitor import (
    estimate_graph_memory,
    get_system_stats,
)


def test_get_system_stats():
    stats = get_system_stats()
    assert stats.ram_total_gb > 0
    assert 0 <= stats.ram_percent <= 100
    assert stats.cpu_count > 0


def test_estimate_graph_memory():
    # 10k nodes, 50k edges
    mem_gb = estimate_graph_memory(10_000, 50_000)
    assert mem_gb > 0
    assert mem_gb < 0.01  # Should be a few MB

    # 1M nodes, 5M edges
    mem_gb = estimate_graph_memory(1_000_000, 5_000_000)
    assert mem_gb < 1.0  # Should be under 1 GB

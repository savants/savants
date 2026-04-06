"""System resource monitor for hardware-aware routing decisions.

Tracks CPU, RAM, and disk usage to enforce the "60% Rule":
if model weights + graph indexes exceed 60% of RAM, trigger cloud bursting.
"""

from __future__ import annotations

import logging
import platform
from dataclasses import dataclass

import psutil

logger = logging.getLogger(__name__)


@dataclass
class SystemStats:
    ram_total_gb: float
    ram_used_gb: float
    ram_available_gb: float
    ram_percent: float
    cpu_percent: float
    cpu_count: int
    platform: str
    architecture: str
    is_apple_silicon: bool


def get_system_stats() -> SystemStats:
    """Collect current system resource usage."""
    mem = psutil.virtual_memory()
    cpu_pct = psutil.cpu_percent(interval=0.1)

    arch = platform.machine().lower()
    is_apple = arch in ("arm64", "aarch64") and platform.system() == "Darwin"

    return SystemStats(
        ram_total_gb=mem.total / (1024**3),
        ram_used_gb=mem.used / (1024**3),
        ram_available_gb=mem.available / (1024**3),
        ram_percent=mem.percent,
        cpu_percent=cpu_pct,
        cpu_count=psutil.cpu_count() or 1,
        platform=platform.system(),
        architecture=arch,
        is_apple_silicon=is_apple,
    )


def should_cloud_burst(threshold_pct: float = 60.0) -> tuple[bool, str]:
    """Check if local resources are exhausted and cloud bursting is needed.

    Returns (should_burst, reason).
    """
    stats = get_system_stats()

    if stats.ram_percent >= threshold_pct:
        return True, (
            f"RAM at {stats.ram_percent:.1f}% "
            f"({stats.ram_used_gb:.1f}/{stats.ram_total_gb:.1f} GB). "
            f"Threshold: {threshold_pct}%"
        )

    if stats.cpu_percent >= 90.0:
        return True, f"CPU at {stats.cpu_percent:.1f}%. System under heavy load."

    return False, f"Resources OK: RAM {stats.ram_percent:.1f}%, CPU {stats.cpu_percent:.1f}%"


def estimate_graph_memory(node_count: int, edge_count: int) -> float:
    """Estimate memory usage of the graph in GB.

    FalkorDB uses sparse matrices, so memory scales roughly linearly
    with the number of non-zero entries (edges).
    """
    # Rough estimate: ~100 bytes per node, ~50 bytes per edge
    bytes_estimate = (node_count * 100) + (edge_count * 50)
    return bytes_estimate / (1024**3)


def can_fit_locally(
    node_count: int,
    edge_count: int,
    model_size_gb: float = 8.0,
) -> tuple[bool, str]:
    """Check if the graph + model can fit within the 60% RAM rule."""
    stats = get_system_stats()
    graph_gb = estimate_graph_memory(node_count, edge_count)
    total_needed = graph_gb + model_size_gb
    threshold_gb = stats.ram_total_gb * 0.6

    if total_needed > threshold_gb:
        return False, (
            f"Estimated need: {total_needed:.1f} GB "
            f"(graph: {graph_gb:.2f} GB + model: {model_size_gb:.1f} GB). "
            f"60% threshold: {threshold_gb:.1f} GB of {stats.ram_total_gb:.1f} GB total."
        )

    return True, (
        f"Fits locally: {total_needed:.1f} GB needed, "
        f"{threshold_gb:.1f} GB available within 60% rule."
    )

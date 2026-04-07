"""Reusable analytical queries for codebase intelligence.

These are the canonical Cypher queries that produced the findings in
docs/fastapi-analysis.md. Each one is a small named function that takes
a GraphClient and returns structured results — designed to be both
called from the CLI (synapcode bus-factor, synapcode hubs, etc.) and
composed into higher-level reports.
"""

from synapcode.analysis.queries import (
    bus_factor,
    co_change,
    god_classes,
    hot_files,
    most_called,
    name_collisions,
    test_to_source_ratio,
    top_callers_per_file,
    top_contributors,
    top_hubs,
)

__all__ = [
    "bus_factor",
    "co_change",
    "god_classes",
    "hot_files",
    "most_called",
    "name_collisions",
    "test_to_source_ratio",
    "top_callers_per_file",
    "top_contributors",
    "top_hubs",
]

"""Central configuration for SynapCode."""

from __future__ import annotations

import os
from dataclasses import dataclass, field


@dataclass
class FalkorDBConfig:
    host: str = os.getenv("FALKORDB_HOST", "localhost")
    port: int = int(os.getenv("FALKORDB_PORT", "6379"))
    graph_name: str = os.getenv("FALKORDB_GRAPH", "savants")


@dataclass
class TemporalConfig:
    host: str = os.getenv("TEMPORAL_HOST", "localhost:7233")
    namespace: str = os.getenv("TEMPORAL_NAMESPACE", "default")
    task_queue: str = os.getenv("TEMPORAL_TASK_QUEUE", "savants-tasks")


@dataclass
class RoutingConfig:
    """Selective Deferred Routing thresholds."""

    local_model: str = os.getenv("LOCAL_MODEL", "qwen2.5-coder:14b")
    local_model_url: str = os.getenv("LOCAL_MODEL_URL", "http://localhost:11434")
    frontier_api_key: str = os.getenv("FRONTIER_API_KEY", "")
    frontier_model: str = os.getenv("FRONTIER_MODEL", "claude-sonnet-4-20250514")
    frontier_url: str = os.getenv("FRONTIER_URL", "https://api.anthropic.com")
    complexity_threshold: float = float(os.getenv("COMPLEXITY_THRESHOLD", "0.7"))
    ram_threshold_pct: float = float(os.getenv("RAM_THRESHOLD_PCT", "60.0"))


@dataclass
class EpisodicConfig:
    """Graphiti episodic memory settings."""

    embedding_model: str = os.getenv("EMBEDDING_MODEL", "all-MiniLM-L6-v2")
    embedding_dim: int = int(os.getenv("EMBEDDING_DIM", "384"))
    max_episodes_per_recall: int = int(os.getenv("MAX_EPISODES_PER_RECALL", "20"))


@dataclass
class SynapCodeConfig:
    falkordb: FalkorDBConfig = field(default_factory=FalkorDBConfig)
    temporal: TemporalConfig = field(default_factory=TemporalConfig)
    routing: RoutingConfig = field(default_factory=RoutingConfig)
    episodic: EpisodicConfig = field(default_factory=EpisodicConfig)


def load_config() -> SynapCodeConfig:
    return SynapCodeConfig()

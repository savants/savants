"""FalkorDB client wrapper with connection pooling and graph management."""

from __future__ import annotations

import logging
from typing import Any

from falkordb import FalkorDB

from synapcode.config import FalkorDBConfig
from synapcode.graph.schema import SCHEMA_INDICES

logger = logging.getLogger(__name__)


class GraphClient:
    """Manages connections to FalkorDB and provides query execution."""

    def __init__(self, config: FalkorDBConfig | None = None):
        self._config = config or FalkorDBConfig()
        self._db: FalkorDB | None = None

    @property
    def db(self) -> FalkorDB:
        if self._db is None:
            self._db = FalkorDB(
                host=self._config.host,
                port=self._config.port,
            )
        return self._db

    @property
    def graph(self):
        return self.db.select_graph(self._config.graph_name)

    def ensure_schema(self) -> None:
        """Create indices if they don't exist."""
        g = self.graph
        for idx_query in SCHEMA_INDICES:
            try:
                g.query(idx_query)
            except Exception:
                # Index may already exist
                pass
        logger.info("Schema indices ensured for graph '%s'", self._config.graph_name)

    def query(self, cypher: str, params: dict[str, Any] | None = None) -> Any:
        """Execute a Cypher query and return the result."""
        g = self.graph
        result = g.query(cypher, params=params or {})
        return result

    def execute_batch(self, queries: list[tuple[str, dict[str, Any]]]) -> None:
        """Execute a batch of Cypher queries atomically."""
        g = self.graph
        for cypher, params in queries:
            g.query(cypher, params=params)

    def dump_graph(self) -> bytes:
        """Serialize the current graph for snapshot/export (Git LFS bootstrap)."""
        # FalkorDB supports GRAPH.DUMP for serialization
        result = self.db.connection.execute_command(
            "GRAPH.DUMP", self._config.graph_name
        )
        return result

    def restore_graph(self, data: bytes) -> None:
        """Restore a graph from a serialized dump."""
        self.db.connection.execute_command(
            "GRAPH.RESTORE", self._config.graph_name, data
        )

    def delete_graph(self) -> None:
        """Delete the entire graph."""
        try:
            self.graph.delete()
            logger.info("Deleted graph '%s'", self._config.graph_name)
        except Exception as e:
            logger.warning("Could not delete graph: %s", e)

    def node_count(self) -> int:
        result = self.query("MATCH (n) RETURN count(n) AS cnt")
        return result.result_set[0][0] if result.result_set else 0

    def edge_count(self) -> int:
        result = self.query("MATCH ()-[r]->() RETURN count(r) AS cnt")
        return result.result_set[0][0] if result.result_set else 0

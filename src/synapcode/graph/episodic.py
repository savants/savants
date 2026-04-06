"""Episodic temporal memory layer built on FalkorDB.

Transforms the static Code Property Graph into a living memory that tracks
*when* facts were true — not just *what* was true. Enables queries like
"What was the architecture before the refactor last Tuesday?"

Uses validity windows (valid_from, valid_to) on every fact so the agent
can reason across historical versions of the codebase.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any

from synapcode.graph.client import GraphClient

logger = logging.getLogger(__name__)


@dataclass
class Episode:
    """A discrete event that introduces new knowledge."""

    content: str
    source_type: str  # "git_commit", "chat", "file_upload", "agent_reasoning"
    source_id: str  # commit SHA, message ID, etc.
    timestamp: datetime = field(default_factory=lambda: datetime.now(timezone.utc))
    metadata: dict[str, Any] = field(default_factory=dict)


@dataclass
class TemporalFact:
    """A fact with a validity window."""

    subject: str
    predicate: str
    object: str
    valid_from: datetime
    valid_to: datetime | None = None  # None = still valid
    source_episode: str = ""
    confidence: float = 1.0


@dataclass
class RecallResult:
    """Results from querying episodic memory."""

    facts: list[TemporalFact]
    episodes: list[Episode]
    as_of: datetime


EPISODIC_INDICES = [
    "CREATE INDEX FOR (e:Episode) ON (e.timestamp)",
    "CREATE INDEX FOR (e:Episode) ON (e.source_type)",
    "CREATE INDEX FOR (f:Fact) ON (f.valid_from)",
    "CREATE INDEX FOR (ent:Entity) ON (ent.name)",
]


class EpisodicMemory:
    """Manages temporal knowledge: episodes, facts with validity windows, entities."""

    def __init__(self, client: GraphClient | None = None):
        self.client = client or GraphClient()

    def ensure_schema(self) -> None:
        for idx in EPISODIC_INDICES:
            try:
                self.client.query(idx)
            except Exception:
                pass

    def add_episode(self, episode: Episode) -> str:
        """Record an episode (discrete knowledge event) in the graph.

        Returns the episode node ID.
        """
        ts = episode.timestamp.isoformat()
        result = self.client.query(
            "CREATE (e:Episode {"
            "  content: $content,"
            "  source_type: $source_type,"
            "  source_id: $source_id,"
            "  timestamp: $timestamp"
            "}) RETURN ID(e)",
            {
                "content": episode.content,
                "source_type": episode.source_type,
                "source_id": episode.source_id,
                "timestamp": ts,
            },
        )
        episode_id = result.result_set[0][0] if result.result_set else ""
        logger.info("Recorded episode [%s] %s", episode.source_type, episode.source_id[:12])
        return str(episode_id)

    def add_fact(self, fact: TemporalFact, episode_id: str = "") -> None:
        """Add a temporal fact to the graph.

        Creates Entity nodes for subject/object and a typed Fact edge
        between them with validity window properties.
        """
        valid_from = fact.valid_from.isoformat()
        valid_to = fact.valid_to.isoformat() if fact.valid_to else ""

        self.client.query(
            "MERGE (s:Entity {name: $subject}) "
            "MERGE (o:Entity {name: $object}) "
            "CREATE (s)-[r:FACT {"
            "  predicate: $predicate,"
            "  valid_from: $valid_from,"
            "  valid_to: $valid_to,"
            "  confidence: $confidence,"
            "  source_episode: $source_episode"
            "}]->(o)",
            {
                "subject": fact.subject,
                "predicate": fact.predicate,
                "object": fact.object,
                "valid_from": valid_from,
                "valid_to": valid_to,
                "confidence": fact.confidence,
                "source_episode": episode_id or fact.source_episode,
            },
        )

    def invalidate_fact(
        self,
        subject: str,
        predicate: str,
        object_: str,
        invalidated_at: datetime | None = None,
    ) -> int:
        """Mark a fact as no longer valid by setting its valid_to timestamp.

        Does NOT delete the fact — preserves historical reasoning.
        """
        ts = (invalidated_at or datetime.now(timezone.utc)).isoformat()
        result = self.client.query(
            "MATCH (s:Entity {name: $subject})-[r:FACT {predicate: $predicate}]->"
            "(o:Entity {name: $object}) "
            "WHERE r.valid_to = '' "
            "SET r.valid_to = $ts "
            "RETURN count(r)",
            {
                "subject": subject,
                "predicate": predicate,
                "object": object_,
                "ts": ts,
            },
        )
        count = result.result_set[0][0] if result.result_set else 0
        logger.info("Invalidated %d facts: %s -[%s]-> %s", count, subject, predicate, object_)
        return count

    def supersede_fact(
        self,
        old_subject: str,
        old_predicate: str,
        old_object: str,
        new_fact: TemporalFact,
        episode_id: str = "",
    ) -> None:
        """Replace an old fact with a new one, maintaining the audit trail."""
        now = datetime.now(timezone.utc)
        self.invalidate_fact(old_subject, old_predicate, old_object, now)
        self.add_fact(new_fact, episode_id)

    def recall(
        self,
        query: str,
        as_of: datetime | None = None,
        max_results: int = 20,
    ) -> RecallResult:
        """Recall facts and episodes relevant to a query, filtered by time.

        Returns only facts that were valid at the given point in time.
        """
        as_of = as_of or datetime.now(timezone.utc)
        ts = as_of.isoformat()

        # Fetch valid facts containing the query terms
        fact_result = self.client.query(
            "MATCH (s:Entity)-[r:FACT]->(o:Entity) "
            "WHERE r.valid_from <= $ts "
            "AND (r.valid_to = '' OR r.valid_to > $ts) "
            "AND (s.name CONTAINS $query OR o.name CONTAINS $query "
            "     OR r.predicate CONTAINS $query) "
            "RETURN s.name, r.predicate, o.name, r.valid_from, "
            "       r.valid_to, r.confidence, r.source_episode "
            f"ORDER BY r.valid_from DESC LIMIT {max_results}",
            {"ts": ts, "query": query},
        )

        facts = []
        for row in fact_result.result_set:
            facts.append(TemporalFact(
                subject=row[0],
                predicate=row[1],
                object=row[2],
                valid_from=datetime.fromisoformat(row[3]),
                valid_to=datetime.fromisoformat(row[4]) if row[4] else None,
                confidence=row[5],
                source_episode=row[6],
            ))

        # Fetch related episodes
        episode_result = self.client.query(
            "MATCH (e:Episode) "
            "WHERE e.content CONTAINS $query AND e.timestamp <= $ts "
            f"RETURN e.content, e.source_type, e.source_id, e.timestamp "
            f"ORDER BY e.timestamp DESC LIMIT {max_results}",
            {"query": query, "ts": ts},
        )

        episodes = []
        for row in episode_result.result_set:
            episodes.append(Episode(
                content=row[0],
                source_type=row[1],
                source_id=row[2],
                timestamp=datetime.fromisoformat(row[3]),
            ))

        return RecallResult(facts=facts, episodes=episodes, as_of=as_of)

    def get_entity_history(self, entity_name: str) -> list[TemporalFact]:
        """Get the full history of facts about an entity, including expired ones."""
        result = self.client.query(
            "MATCH (s:Entity)-[r:FACT]->(o:Entity) "
            "WHERE s.name = $name OR o.name = $name "
            "RETURN s.name, r.predicate, o.name, r.valid_from, "
            "       r.valid_to, r.confidence "
            "ORDER BY r.valid_from",
            {"name": entity_name},
        )
        return [
            TemporalFact(
                subject=row[0],
                predicate=row[1],
                object=row[2],
                valid_from=datetime.fromisoformat(row[3]),
                valid_to=datetime.fromisoformat(row[4]) if row[4] else None,
                confidence=row[5],
            )
            for row in result.result_set
        ]

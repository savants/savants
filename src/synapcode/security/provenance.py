"""Provenance stamping: SHA-256 content hashing for graph integrity.

Every node and edge in the shared graph is stamped with:
  - Source commit SHA
  - Author identity
  - Timestamp
  - Content hash (SHA-256 of the source code)

This prevents graph poisoning from experimental local code and provides
an auditable trail for multi-developer environments.
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from datetime import datetime, timezone


@dataclass
class ProvenanceStamp:
    source_commit: str
    author: str
    timestamp: str
    content_hash: str

    def to_dict(self) -> dict:
        return {
            "source_commit": self.source_commit,
            "author": self.author,
            "timestamp": self.timestamp,
            "content_hash": self.content_hash,
        }


def compute_content_hash(content: str | bytes) -> str:
    """Compute SHA-256 hash of content."""
    if isinstance(content, str):
        content = content.encode()
    return hashlib.sha256(content).hexdigest()


def create_stamp(
    content: str | bytes,
    commit_sha: str,
    author: str,
) -> ProvenanceStamp:
    """Create a provenance stamp for a piece of content."""
    return ProvenanceStamp(
        source_commit=commit_sha,
        author=author,
        timestamp=datetime.now(timezone.utc).isoformat(),
        content_hash=compute_content_hash(content),
    )


def verify_stamp(stamp: ProvenanceStamp, content: str | bytes) -> bool:
    """Verify that content matches its provenance stamp."""
    return compute_content_hash(content) == stamp.content_hash


def stamp_cypher_properties(stamp: ProvenanceStamp) -> str:
    """Generate Cypher SET clause for attaching provenance to a node."""
    return (
        f"n.prov_commit = '{stamp.source_commit}', "
        f"n.prov_author = '{stamp.author}', "
        f"n.prov_timestamp = '{stamp.timestamp}', "
        f"n.prov_hash = '{stamp.content_hash}'"
    )

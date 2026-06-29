"""
Duplicate Detection — find existing functions before writing new code.

Searches the Savants code index for functions with similar names or
purposes using keyword-based fuzzy matching. Helps AI agents avoid
re-implementing functionality that already exists.

Usage:
    from savants_guard.duplicates import find_similar

    matches = find_similar("retry with exponential backoff", repo="savants")
    # Returns list of dicts with: function, file, score
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any


# ============================================================
# Code index loading
# ============================================================

def _load_code_index(repo: str) -> dict[str, Any] | None:
    """Load the code index for a repo from ~/.savants/code-index/{repo}.json."""
    index_dir = Path.home() / ".savants" / "code-index"
    index_path = index_dir / f"{repo}.json"
    if not index_path.exists():
        return None
    with open(index_path) as f:
        return json.load(f)


# ============================================================
# Tokenization and scoring
# ============================================================

_SPLIT_RE = re.compile(r"[^a-zA-Z0-9]+")
_CAMEL_SPLIT_RE = re.compile(r"(?<=[a-z])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])")


def _tokenize(text: str) -> set[str]:
    """
    Split text into lowercase tokens, handling camelCase, snake_case,
    and natural language.
    """
    # First split on non-alphanumeric
    parts = _SPLIT_RE.split(text)
    tokens: set[str] = set()
    for part in parts:
        if not part:
            continue
        # Split camelCase
        sub_parts = _CAMEL_SPLIT_RE.split(part)
        for sp in sub_parts:
            if sp:
                tokens.add(sp.lower())
    return tokens


def _score_match(query_tokens: set[str], target_tokens: set[str]) -> float:
    """
    Compute a similarity score between two token sets.

    Uses Jaccard-like overlap weighted toward query coverage.
    Score is in [0.0, 1.0].
    """
    if not query_tokens or not target_tokens:
        return 0.0

    intersection = query_tokens & target_tokens
    if not intersection:
        # Try substring matching as fallback
        sub_matches = 0
        for qt in query_tokens:
            for tt in target_tokens:
                if qt in tt or tt in qt:
                    sub_matches += 1
                    break
        if sub_matches == 0:
            return 0.0
        return round(sub_matches / len(query_tokens) * 0.5, 2)

    # Weight toward query coverage (how much of the query is matched)
    query_coverage = len(intersection) / len(query_tokens)
    # Also factor in target specificity
    target_coverage = len(intersection) / len(target_tokens)
    # Weighted average favoring query coverage
    score = 0.7 * query_coverage + 0.3 * target_coverage
    return round(score, 2)


# ============================================================
# Public API
# ============================================================

def find_similar(
    description: str,
    *,
    repo: str,
    threshold: float = 0.3,
    max_results: int = 10,
) -> list[dict[str, Any]]:
    """
    Find existing functions similar to a description.

    Args:
        description: Natural language description of the functionality
                     (e.g. "retry with exponential backoff").
        repo: Repository name (must have a code index at
              ~/.savants/code-index/{repo}.json).
        threshold: Minimum similarity score (0.0 to 1.0). Default 0.3.
        max_results: Maximum number of results to return. Default 10.

    Returns:
        List of dicts sorted by score (descending), each with:
          - function: function name
          - file: "filepath:line"
          - score: similarity score (0.0 to 1.0)
    """
    index = _load_code_index(repo)
    if index is None:
        return []

    entities = index.get("entities", [])
    query_tokens = _tokenize(description)
    if not query_tokens:
        return []

    results: list[dict[str, Any]] = []

    for entity in entities:
        if entity.get("kind") not in ("function", "class"):
            continue

        name = entity.get("name", "")
        file_path = entity.get("file", "")
        line = entity.get("line", 0)
        body = entity.get("body", "")

        if not name:
            continue

        # Build target tokens from name + body
        name_tokens = _tokenize(name)
        body_tokens = _tokenize(body) if body else set()

        # Score against name (primary) and body (secondary)
        name_score = _score_match(query_tokens, name_tokens)
        body_score = _score_match(query_tokens, body_tokens) if body_tokens else 0.0

        # Combined score: name match is more important
        score = round(0.6 * name_score + 0.4 * body_score, 2)

        if score >= threshold:
            results.append({
                "function": name,
                "file": f"{file_path}:{line}",
                "score": score,
            })

    # Sort by score descending, then by function name for stability
    results.sort(key=lambda r: (-r["score"], r["function"]))
    return results[:max_results]

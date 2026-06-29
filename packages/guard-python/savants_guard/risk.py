"""
Risk Context — graph-powered risk scoring for guard rules.

Enriches the guard context with risk variables (blast_radius, caller_count,
test_coverage, risk_score, file_risk, change_size) by calling registered
data sources lazily — only when a rule actually references the variable.

Usage:
    from savants_guard.risk import RiskContext, file_risk, change_size

    rc = RiskContext()
    rc.register_risk_source("blast_radius", lambda ctx: api.blast_radius(ctx["file"]))

    guard = create_guard(rules, risk_context=rc)
    guard.check({"file": "src/payments/charge.py", "action": "edit"})
    # blast_radius is resolved lazily when the rule references it
"""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any, Callable

from .types import ASTNode


# ============================================================
# Built-in risk sources
# ============================================================

# Patterns that indicate high-risk files
HIGH_RISK_PATTERNS: list[str] = [
    r"payment",
    r"billing",
    r"auth",
    r"migration",
    r"secret",
    r"credential",
    r"password",
    r"token",
    r"crypt",
]

_HIGH_RISK_RE = re.compile(
    "|".join(HIGH_RISK_PATTERNS), re.IGNORECASE
)


def file_risk(context: dict[str, Any]) -> str:
    """
    Built-in risk source: returns 'high' if the file path matches
    known high-risk patterns, 'low' otherwise.

    Looks for 'file', 'file_path', or 'path' in context.
    """
    path = context.get("file") or context.get("file_path") or context.get("path") or ""
    if _HIGH_RISK_RE.search(str(path)):
        return "high"
    return "low"


def change_size(context: dict[str, Any]) -> int:
    """
    Built-in risk source: returns the line count of a diff.

    Looks for 'diff' or 'change' in context.
    """
    diff = context.get("diff") or context.get("change") or ""
    text = str(diff)
    if not text:
        return 0
    return len(text.splitlines())


# ============================================================
# AST variable extraction
# ============================================================

def _extract_var_names(node: ASTNode | None) -> set[str]:
    """
    Walk an AST node tree and collect all variable names referenced.
    """
    if node is None:
        return set()

    names: set[str] = set()
    node_type = node.get("type")

    if node_type == "var":
        names.add(node["name"])
    elif node_type == "compare":
        names |= _extract_var_names(node.get("left"))
        names |= _extract_var_names(node.get("right"))
    elif node_type in ("and", "or"):
        for child in node.get("children", []):
            names |= _extract_var_names(child)
    elif node_type == "not":
        names |= _extract_var_names(node.get("child"))
    elif node_type == "if":
        names |= _extract_var_names(node.get("condition"))
        names |= _extract_var_names(node.get("then"))
        names |= _extract_var_names(node.get("else"))

    return names


# ============================================================
# RiskContext
# ============================================================

RiskSourceFn = Callable[[dict[str, Any]], Any]


# ============================================================
# Codebase-aware suggestion source
# ============================================================

def _load_code_index(repo_path: str) -> dict[str, Any] | None:
    """Load the code index for a repo from ~/.savants/code-index/{name}.json."""
    repo_name = Path(repo_path).name
    index_path = Path.home() / ".savants" / "code-index" / f"{repo_name}.json"
    if not index_path.exists():
        return None
    with open(index_path) as f:
        return json.load(f)


def codebase_suggestion(repo_path: str) -> RiskSourceFn:
    """
    Risk source factory: returns a function that searches the code index
    for functions matching the action being blocked, and returns a
    codebase-specific suggestion.

    Usage:
        rc = RiskContext()
        rc.register_risk_source("suggestion", codebase_suggestion("/path/to/repo"))

        guard = create_guard([
            "when action eq 'delete_user' then suggest suggestion",
        ], risk_context=rc)

    When the guard fires, the suggestion variable is populated from the
    code index (e.g. "This codebase uses soft deletes. See
    UserRepository.softDelete() at user-repo.ts:89").
    """
    # Pre-load the index once
    index = _load_code_index(repo_path)

    def _source(context: dict[str, Any]) -> str:
        if index is None:
            return ""

        action = str(context.get("action", ""))
        if not action:
            return ""

        # Tokenize the action for matching
        action_tokens = set(re.split(r"[_\-\s]+", action.lower()))
        action_tokens.discard("")

        entities = index.get("entities", [])
        best_match: dict[str, Any] | None = None
        best_score = 0

        for entity in entities:
            if entity.get("kind") != "function":
                continue
            name = entity.get("name", "")
            if not name:
                continue

            # Split name into tokens (camelCase + snake_case)
            name_lower = name.lower()
            name_tokens = set(re.split(r"[_\-\s]+", name_lower))
            # Also split camelCase
            camel_parts = re.sub(r"([a-z])([A-Z])", r"\1_\2", name).lower()
            name_tokens |= set(re.split(r"[_\-\s]+", camel_parts))
            name_tokens.discard("")

            overlap = action_tokens & name_tokens
            if len(overlap) > best_score:
                best_score = len(overlap)
                best_match = entity

        if best_match and best_score > 0:
            func_name = best_match["name"]
            file_path = best_match.get("file", "unknown")
            line = best_match.get("line", 0)
            return (
                f"This codebase already has {func_name}() "
                f"at {file_path}:{line}. Consider using it instead."
            )

        return ""

    return _source


class RiskContext:
    """
    Manages risk data sources and enriches guard context lazily.

    Register named sources that map to DSL variable names. When
    guard.check() runs, only sources referenced by the current rule
    are invoked.

    Usage:
        rc = RiskContext()
        rc.register_risk_source("blast_radius", my_blast_radius_fn)
        guard = create_guard(rules, risk_context=rc)
    """

    def __init__(self) -> None:
        self._sources: dict[str, RiskSourceFn] = {}
        # Register built-ins
        self._sources["file_risk"] = file_risk
        self._sources["change_size"] = change_size

    def register_risk_source(self, name: str, source: RiskSourceFn) -> None:
        """
        Register a risk data source.

        Args:
            name: Variable name in DSL rules (e.g. 'blast_radius').
            source: Callable that takes a context dict and returns a value.
        """
        self._sources[name] = source

    def unregister_risk_source(self, name: str) -> None:
        """Remove a registered risk source."""
        self._sources.pop(name, None)

    @property
    def source_names(self) -> list[str]:
        """List all registered source names."""
        return list(self._sources.keys())

    def enrich(self, context: dict[str, Any], rule_condition: ASTNode) -> dict[str, Any]:
        """
        Enrich a context dict with risk variables, but only for variables
        that are actually referenced in the rule condition (lazy evaluation).

        Returns a new dict — does not mutate the original.
        """
        referenced = _extract_var_names(rule_condition)
        needed = referenced & set(self._sources.keys())

        if not needed:
            return context

        # Only copy if we actually need to add something
        enriched = dict(context)
        for name in needed:
            # Don't overwrite if already provided in context
            if name not in enriched:
                enriched[name] = self._sources[name](context)

        return enriched

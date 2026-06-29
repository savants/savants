"""
Convention Detection — analyze a codebase's patterns from the code index.

Reads the Savants code index at ~/.savants/code-index/{repo}.json
and detects naming conventions, error handling patterns, test patterns,
common architectural patterns, and file structure organization.

Usage:
    from savants_guard.conventions import detect_conventions

    conventions = detect_conventions("/path/to/repo")
    # Returns dict with: naming, error_handling, test_pattern,
    #                     common_patterns, file_structure
"""

from __future__ import annotations

import json
import os
import re
from pathlib import Path
from typing import Any


# ============================================================
# Helpers
# ============================================================

def _load_code_index(repo: str) -> dict[str, Any] | None:
    """Load the code index for a repo from ~/.savants/code-index/{repo}.json."""
    index_dir = Path.home() / ".savants" / "code-index"
    index_path = index_dir / f"{repo}.json"
    if not index_path.exists():
        return None
    with open(index_path) as f:
        return json.load(f)


def _repo_name_from_path(repo_path: str) -> str:
    """Extract repo name from a filesystem path."""
    return Path(repo_path).name


_CAMEL_RE = re.compile(r"^[a-z][a-zA-Z0-9]*$")
_SNAKE_RE = re.compile(r"^[a-z][a-z0-9]*(?:_[a-z0-9]+)+$")
_PASCAL_RE = re.compile(r"^[A-Z][a-zA-Z0-9]*$")


def _classify_name(name: str) -> str | None:
    """Classify a single identifier as camelCase, snake_case, or PascalCase."""
    if _SNAKE_RE.match(name):
        return "snake_case"
    if _CAMEL_RE.match(name):
        return "camelCase"
    if _PASCAL_RE.match(name):
        return "PascalCase"
    return None


# ============================================================
# Detection functions
# ============================================================

def _detect_naming(entities: list[dict[str, Any]]) -> str:
    """Detect the dominant naming convention from function names."""
    counts: dict[str, int] = {"camelCase": 0, "snake_case": 0, "PascalCase": 0}

    for entity in entities:
        if entity.get("kind") != "function":
            continue
        name = entity.get("name", "")
        if not name or name.startswith("_"):
            continue
        style = _classify_name(name)
        if style:
            counts[style] += 1

    if not any(counts.values()):
        return "mixed"

    return max(counts, key=lambda k: counts[k])


def _detect_error_handling(entities: list[dict[str, Any]]) -> str:
    """Detect error handling pattern from function bodies."""
    patterns: dict[str, int] = {
        "try/except": 0,
        "if err != nil": 0,
        "Result<T, Error>": 0,
        ".catch": 0,
        "try/catch": 0,
    }

    for entity in entities:
        body = entity.get("body", "")
        if not body:
            continue
        if "try:" in body or "except " in body:
            patterns["try/except"] += 1
        if "if err != nil" in body or "if err != nil {" in body:
            patterns["if err != nil"] += 1
        if "Result<" in body or "-> Result" in body:
            patterns["Result<T, Error>"] += 1
        if ".catch(" in body or ".catch " in body:
            patterns[".catch"] += 1
        if "try {" in body or "catch (" in body or "catch(" in body:
            patterns["try/catch"] += 1

    if not any(patterns.values()):
        return "unknown"

    return max(patterns, key=lambda k: patterns[k])


def _detect_test_pattern(entities: list[dict[str, Any]]) -> str:
    """Detect test file naming pattern."""
    test_files: set[str] = set()
    for entity in entities:
        file_path = entity.get("file", "")
        if not file_path:
            continue
        basename = os.path.basename(file_path)
        if "test" in basename.lower() or "spec" in basename.lower():
            test_files.add(basename)

    if not test_files:
        return "unknown"

    # Count patterns
    patterns: dict[str, int] = {}
    for name in test_files:
        if name.startswith("test_") and name.endswith(".py"):
            patterns["test_*.py"] = patterns.get("test_*.py", 0) + 1
        elif name.endswith(".test.ts"):
            patterns["*.test.ts"] = patterns.get("*.test.ts", 0) + 1
        elif name.endswith(".test.js"):
            patterns["*.test.js"] = patterns.get("*.test.js", 0) + 1
        elif name.endswith(".spec.ts"):
            patterns["*.spec.ts"] = patterns.get("*.spec.ts", 0) + 1
        elif name.endswith(".spec.js"):
            patterns["*.spec.js"] = patterns.get("*.spec.js", 0) + 1
        elif name.endswith("_test.go"):
            patterns["*_test.go"] = patterns.get("*_test.go", 0) + 1
        elif name.endswith("_test.py"):
            patterns["*_test.py"] = patterns.get("*_test.py", 0) + 1
        elif name.startswith("test_"):
            patterns["test_*"] = patterns.get("test_*", 0) + 1

    if not patterns:
        return "unknown"

    return max(patterns, key=lambda k: patterns[k])


def _detect_common_patterns(entities: list[dict[str, Any]]) -> list[str]:
    """Detect common architectural patterns from function/class names and bodies."""
    detected: list[str] = []
    all_names = {e.get("name", "").lower() for e in entities}
    all_bodies = " ".join(e.get("body", "") for e in entities)

    # Dependency injection
    di_signals = ["inject", "provider", "container", "@inject", "dependency"]
    if any(s in all_bodies.lower() for s in di_signals) or any(
        s in n for n in all_names for s in ["inject", "provider", "container"]
    ):
        detected.append("dependency injection")

    # Repository pattern
    if any("repository" in n or "repo" == n for n in all_names):
        detected.append("repository pattern")

    # Middleware
    if any("middleware" in n for n in all_names):
        detected.append("middleware chain")

    # Factory pattern
    if any("factory" in n or n.startswith("create_") or n.startswith("make_") for n in all_names):
        detected.append("factory pattern")

    # Observer/event pattern
    if any(s in n for n in all_names for s in ["observer", "listener", "emitter", "on_event", "emit"]):
        detected.append("observer pattern")

    # Singleton
    if any("singleton" in n or "instance" == n or "get_instance" in n for n in all_names):
        detected.append("singleton pattern")

    # Builder pattern
    if any("builder" in n for n in all_names):
        detected.append("builder pattern")

    return detected


def _detect_file_structure(entities: list[dict[str, Any]]) -> str:
    """
    Detect file organization: feature-based vs layer-based.

    Layer-based: files grouped by type (controllers/, models/, services/)
    Feature-based: files grouped by feature (user/, payments/, auth/)
    """
    dirs: set[str] = set()
    for entity in entities:
        file_path = entity.get("file", "")
        if not file_path:
            continue
        parts = file_path.split("/")
        if len(parts) > 1:
            dirs.add(parts[0].lower())

    layer_signals = {"controllers", "models", "services", "views", "routes", "handlers", "middleware", "utils"}
    feature_overlap = len(dirs - layer_signals)
    layer_overlap = len(dirs & layer_signals)

    if layer_overlap >= 2:
        return "layer-based"
    if feature_overlap > layer_overlap:
        return "feature-based"
    return "mixed"


# ============================================================
# Public API
# ============================================================

def detect_conventions(repo_path: str) -> dict[str, Any]:
    """
    Analyze a codebase and detect its conventions.

    Args:
        repo_path: Filesystem path to the repo, or just the repo name.

    Returns:
        Dict with keys: naming, error_handling, test_pattern,
        common_patterns, file_structure.
    """
    repo_name = _repo_name_from_path(repo_path)
    index = _load_code_index(repo_name)

    if index is None:
        return {
            "error_handling": "unknown",
            "naming": "unknown",
            "test_pattern": "unknown",
            "common_patterns": [],
            "file_structure": "unknown",
        }

    entities = index.get("entities", [])

    return {
        "error_handling": _detect_error_handling(entities),
        "naming": _detect_naming(entities),
        "test_pattern": _detect_test_pattern(entities),
        "common_patterns": _detect_common_patterns(entities),
        "file_structure": _detect_file_structure(entities),
    }

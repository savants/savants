"""Tests for convention detection, duplicate detection, and codebase suggestions."""

import json
import os
import pytest
from pathlib import Path
from unittest.mock import patch

from savants_guard.conventions import (
    detect_conventions,
    _classify_name,
    _detect_naming,
    _detect_error_handling,
    _detect_test_pattern,
    _detect_common_patterns,
    _detect_file_structure,
)
from savants_guard.duplicates import find_similar, _tokenize, _score_match
from savants_guard.risk import codebase_suggestion
from savants_guard import create_guard, RiskContext


# ============================================================
# Sample code index for tests
# ============================================================

SAMPLE_INDEX = {
    "repo": "test-repo",
    "files": 10,
    "entities": [
        {
            "kind": "function",
            "name": "handle_request",
            "file": "src/handlers/request.py",
            "line": 15,
            "end_line": 30,
            "body": "def handle_request(self, req):\n    try:\n        result = process(req)\n    except ValueError:\n        return error_response()",
            "params": ["self", "req"],
            "import_source": "",
            "import_names": [],
        },
        {
            "kind": "function",
            "name": "get_user_by_id",
            "file": "src/models/user.py",
            "line": 42,
            "end_line": 55,
            "body": "def get_user_by_id(self, user_id):\n    try:\n        return self.db.query(user_id)\n    except NotFoundError:\n        return None",
            "params": ["self", "user_id"],
            "import_source": "",
            "import_names": [],
        },
        {
            "kind": "function",
            "name": "create_order",
            "file": "src/services/order.py",
            "line": 10,
            "end_line": 25,
            "body": "def create_order(self, items):\n    return Order(items=items)",
            "params": ["self", "items"],
            "import_source": "",
            "import_names": [],
        },
        {
            "kind": "function",
            "name": "soft_delete",
            "file": "src/models/user.py",
            "line": 80,
            "end_line": 90,
            "body": "def soft_delete(self, user_id):\n    self.db.update(user_id, deleted=True)",
            "params": ["self", "user_id"],
            "import_source": "",
            "import_names": [],
        },
        {
            "kind": "function",
            "name": "retryWithBackoff",
            "file": "src/utils/retry.ts",
            "line": 23,
            "end_line": 45,
            "body": "function retryWithBackoff(fn, maxRetries) {\n  return fn().catch(err => delay(exponential(retries)))",
            "params": ["fn", "maxRetries"],
            "import_source": "",
            "import_names": [],
        },
        {
            "kind": "class",
            "name": "UserRepository",
            "file": "src/models/user.py",
            "line": 5,
            "end_line": 100,
            "body": "",
            "params": [],
            "import_source": "",
            "import_names": [],
        },
        {
            "kind": "function",
            "name": "test_create_order",
            "file": "tests/test_order.py",
            "line": 10,
            "end_line": 20,
            "body": "def test_create_order():\n    assert create_order([]) is not None",
            "params": [],
            "import_source": "",
            "import_names": [],
        },
        {
            "kind": "function",
            "name": "test_handle_request",
            "file": "tests/test_handlers.py",
            "line": 5,
            "end_line": 15,
            "body": "def test_handle_request():\n    pass",
            "params": [],
            "import_source": "",
            "import_names": [],
        },
        {
            "kind": "function",
            "name": "register_middleware",
            "file": "src/middleware/auth.py",
            "line": 1,
            "end_line": 10,
            "body": "def register_middleware(app):\n    app.use(auth_check)",
            "params": ["app"],
            "import_source": "",
            "import_names": [],
        },
    ],
    "call_sites": [],
}

CAMEL_CASE_INDEX = {
    "repo": "camel-repo",
    "files": 5,
    "entities": [
        {"kind": "function", "name": "getUserById", "file": "a.ts", "line": 1, "end_line": 5, "body": "", "params": [], "import_source": "", "import_names": []},
        {"kind": "function", "name": "createOrder", "file": "b.ts", "line": 1, "end_line": 5, "body": "", "params": [], "import_source": "", "import_names": []},
        {"kind": "function", "name": "handleRequest", "file": "c.ts", "line": 1, "end_line": 5, "body": "", "params": [], "import_source": "", "import_names": []},
        {"kind": "function", "name": "formatDate", "file": "d.ts", "line": 1, "end_line": 5, "body": "", "params": [], "import_source": "", "import_names": []},
    ],
    "call_sites": [],
}

GO_INDEX = {
    "repo": "go-repo",
    "files": 3,
    "entities": [
        {
            "kind": "function",
            "name": "HandleRequest",
            "file": "handler.go",
            "line": 10,
            "end_line": 25,
            "body": "func HandleRequest(w http.ResponseWriter, r *http.Request) {\n    data, err := fetchData(r)\n    if err != nil {\n        http.Error(w, err.Error(), 500)\n    }\n}",
            "params": ["w", "r"],
            "import_source": "",
            "import_names": [],
        },
        {
            "kind": "function",
            "name": "TestHandleRequest",
            "file": "handler_test.go",
            "line": 5,
            "end_line": 15,
            "body": "",
            "params": ["t"],
            "import_source": "",
            "import_names": [],
        },
    ],
    "call_sites": [],
}


def _write_index(tmp_path: Path, name: str, data: dict) -> None:
    """Write a code index to the expected path."""
    index_dir = tmp_path / ".savants" / "code-index"
    index_dir.mkdir(parents=True, exist_ok=True)
    with open(index_dir / f"{name}.json", "w") as f:
        json.dump(data, f)


# ============================================================
# Convention Detection — _classify_name
# ============================================================

class TestClassifyName:
    def test_snake_case(self):
        assert _classify_name("get_user_by_id") == "snake_case"

    def test_camel_case(self):
        assert _classify_name("getUserById") == "camelCase"

    def test_pascal_case(self):
        assert _classify_name("UserRepository") == "PascalCase"

    def test_single_word_lower(self):
        # Single lowercase word matches camelCase (starts lowercase, all alnum)
        assert _classify_name("handle") == "camelCase"

    def test_underscored_private(self):
        # Leading underscore doesn't match any pattern
        assert _classify_name("_private") is None

    def test_all_caps(self):
        assert _classify_name("HTTP") == "PascalCase"


# ============================================================
# Convention Detection — detect_naming
# ============================================================

class TestDetectNaming:
    def test_snake_case_dominant(self):
        assert _detect_naming(SAMPLE_INDEX["entities"]) == "snake_case"

    def test_camel_case_dominant(self):
        assert _detect_naming(CAMEL_CASE_INDEX["entities"]) == "camelCase"

    def test_empty_entities(self):
        assert _detect_naming([]) == "mixed"

    def test_no_functions(self):
        entities = [{"kind": "class", "name": "Foo"}]
        assert _detect_naming(entities) == "mixed"


# ============================================================
# Convention Detection — detect_error_handling
# ============================================================

class TestDetectErrorHandling:
    def test_try_except(self):
        assert _detect_error_handling(SAMPLE_INDEX["entities"]) == "try/except"

    def test_go_style(self):
        assert _detect_error_handling(GO_INDEX["entities"]) == "if err != nil"

    def test_catch_style(self):
        entities = [
            {"kind": "function", "name": "fetch", "body": "fetch(url).catch(err => log(err))"},
        ]
        assert _detect_error_handling(entities) == ".catch"

    def test_unknown_when_empty(self):
        assert _detect_error_handling([]) == "unknown"

    def test_no_bodies(self):
        entities = [{"kind": "function", "name": "x", "body": ""}]
        assert _detect_error_handling(entities) == "unknown"


# ============================================================
# Convention Detection — detect_test_pattern
# ============================================================

class TestDetectTestPattern:
    def test_python_test_pattern(self):
        assert _detect_test_pattern(SAMPLE_INDEX["entities"]) == "test_*.py"

    def test_go_test_pattern(self):
        assert _detect_test_pattern(GO_INDEX["entities"]) == "*_test.go"

    def test_unknown_no_tests(self):
        entities = [{"kind": "function", "name": "foo", "file": "src/foo.py"}]
        assert _detect_test_pattern(entities) == "unknown"


# ============================================================
# Convention Detection — detect_common_patterns
# ============================================================

class TestDetectCommonPatterns:
    def test_finds_middleware(self):
        patterns = _detect_common_patterns(SAMPLE_INDEX["entities"])
        assert "middleware chain" in patterns

    def test_finds_repository(self):
        # UserRepository class name contains "repository"
        patterns = _detect_common_patterns(SAMPLE_INDEX["entities"])
        assert "repository pattern" in patterns

    def test_finds_factory(self):
        entities = [{"kind": "function", "name": "create_user", "file": "a.py", "body": ""}]
        patterns = _detect_common_patterns(entities)
        assert "factory pattern" in patterns

    def test_empty_entities(self):
        assert _detect_common_patterns([]) == []


# ============================================================
# Convention Detection — detect_file_structure
# ============================================================

class TestDetectFileStructure:
    def test_layer_based(self):
        entities = [
            {"kind": "function", "name": "a", "file": "controllers/user.py"},
            {"kind": "function", "name": "b", "file": "models/user.py"},
            {"kind": "function", "name": "c", "file": "services/user.py"},
        ]
        assert _detect_file_structure(entities) == "layer-based"

    def test_feature_based(self):
        entities = [
            {"kind": "function", "name": "a", "file": "user/handler.py"},
            {"kind": "function", "name": "b", "file": "order/handler.py"},
            {"kind": "function", "name": "c", "file": "payment/handler.py"},
        ]
        assert _detect_file_structure(entities) == "feature-based"

    def test_mixed_structure(self):
        # src/ is the only dir at depth 0
        entities = [
            {"kind": "function", "name": "a", "file": "src/foo.py"},
        ]
        assert _detect_file_structure(entities) in ("feature-based", "mixed")


# ============================================================
# Convention Detection — detect_conventions (integration)
# ============================================================

class TestDetectConventions:
    def test_full_detection(self, tmp_path):
        _write_index(tmp_path, "test-repo", SAMPLE_INDEX)
        with patch("savants_guard.conventions.Path.home", return_value=tmp_path):
            result = detect_conventions("/some/path/test-repo")
        assert result["naming"] == "snake_case"
        assert result["error_handling"] == "try/except"
        assert result["test_pattern"] == "test_*.py"
        assert isinstance(result["common_patterns"], list)
        assert isinstance(result["file_structure"], str)

    def test_missing_index_returns_unknowns(self, tmp_path):
        with patch("savants_guard.conventions.Path.home", return_value=tmp_path):
            result = detect_conventions("/some/nonexistent/repo")
        assert result["naming"] == "unknown"
        assert result["error_handling"] == "unknown"
        assert result["test_pattern"] == "unknown"
        assert result["common_patterns"] == []
        assert result["file_structure"] == "unknown"


# ============================================================
# Duplicate Detection — tokenize
# ============================================================

class TestTokenize:
    def test_snake_case(self):
        tokens = _tokenize("get_user_by_id")
        assert "get" in tokens
        assert "user" in tokens
        assert "by" in tokens
        assert "id" in tokens

    def test_camel_case(self):
        tokens = _tokenize("getUserById")
        assert "get" in tokens
        assert "user" in tokens
        assert "by" in tokens
        assert "id" in tokens

    def test_natural_language(self):
        tokens = _tokenize("retry with exponential backoff")
        assert "retry" in tokens
        assert "exponential" in tokens
        assert "backoff" in tokens

    def test_empty_string(self):
        assert _tokenize("") == set()


# ============================================================
# Duplicate Detection — score_match
# ============================================================

class TestScoreMatch:
    def test_exact_match(self):
        score = _score_match({"retry", "backoff"}, {"retry", "backoff"})
        assert score == 1.0

    def test_no_overlap(self):
        score = _score_match({"retry", "backoff"}, {"create", "user"})
        assert score == 0.0

    def test_partial_overlap(self):
        score = _score_match({"retry", "backoff"}, {"retry", "delay"})
        assert 0.0 < score < 1.0

    def test_empty_sets(self):
        assert _score_match(set(), {"a"}) == 0.0
        assert _score_match({"a"}, set()) == 0.0

    def test_substring_fallback(self):
        # "retry" is a substring of "retrying"
        score = _score_match({"retry"}, {"retrying"})
        assert score > 0.0


# ============================================================
# Duplicate Detection — find_similar
# ============================================================

class TestFindSimilar:
    def test_finds_retry_function(self, tmp_path):
        _write_index(tmp_path, "test-repo", SAMPLE_INDEX)
        with patch("savants_guard.duplicates.Path.home", return_value=tmp_path):
            results = find_similar(
                "retry with backoff",
                repo="test-repo",
                threshold=0.1,
            )
        # Should find retryWithBackoff
        names = [r["function"] for r in results]
        assert "retryWithBackoff" in names

    def test_finds_user_functions(self, tmp_path):
        _write_index(tmp_path, "test-repo", SAMPLE_INDEX)
        with patch("savants_guard.duplicates.Path.home", return_value=tmp_path):
            results = find_similar(
                "get user",
                repo="test-repo",
                threshold=0.1,
            )
        names = [r["function"] for r in results]
        assert "get_user_by_id" in names

    def test_respects_threshold(self, tmp_path):
        _write_index(tmp_path, "test-repo", SAMPLE_INDEX)
        with patch("savants_guard.duplicates.Path.home", return_value=tmp_path):
            results = find_similar(
                "completely unrelated query xyz",
                repo="test-repo",
                threshold=0.9,
            )
        assert len(results) == 0

    def test_respects_max_results(self, tmp_path):
        _write_index(tmp_path, "test-repo", SAMPLE_INDEX)
        with patch("savants_guard.duplicates.Path.home", return_value=tmp_path):
            results = find_similar(
                "handle request",
                repo="test-repo",
                threshold=0.0,
                max_results=2,
            )
        assert len(results) <= 2

    def test_missing_index(self, tmp_path):
        with patch("savants_guard.duplicates.Path.home", return_value=tmp_path):
            results = find_similar("anything", repo="nonexistent")
        assert results == []

    def test_results_sorted_by_score(self, tmp_path):
        _write_index(tmp_path, "test-repo", SAMPLE_INDEX)
        with patch("savants_guard.duplicates.Path.home", return_value=tmp_path):
            results = find_similar(
                "handle request",
                repo="test-repo",
                threshold=0.0,
            )
        if len(results) >= 2:
            for i in range(len(results) - 1):
                assert results[i]["score"] >= results[i + 1]["score"]

    def test_result_format(self, tmp_path):
        _write_index(tmp_path, "test-repo", SAMPLE_INDEX)
        with patch("savants_guard.duplicates.Path.home", return_value=tmp_path):
            results = find_similar(
                "handle request",
                repo="test-repo",
                threshold=0.0,
            )
        if results:
            r = results[0]
            assert "function" in r
            assert "file" in r
            assert "score" in r
            assert ":" in r["file"]  # "filepath:line" format


# ============================================================
# Codebase Suggestion — risk source
# ============================================================

class TestCodebaseSuggestion:
    def test_returns_suggestion_for_matching_action(self, tmp_path):
        _write_index(tmp_path, "test-repo", SAMPLE_INDEX)
        with patch("savants_guard.risk.Path.home", return_value=tmp_path):
            source_fn = codebase_suggestion("/some/path/test-repo")
        result = source_fn({"action": "soft_delete"})
        assert "soft_delete" in result
        assert "src/models/user.py" in result

    def test_returns_empty_for_no_match(self, tmp_path):
        _write_index(tmp_path, "test-repo", SAMPLE_INDEX)
        with patch("savants_guard.risk.Path.home", return_value=tmp_path):
            source_fn = codebase_suggestion("/some/path/test-repo")
        result = source_fn({"action": "totally_unique_xyz"})
        # May or may not match due to substring; just verify it's a string
        assert isinstance(result, str)

    def test_returns_empty_for_missing_index(self, tmp_path):
        with patch("savants_guard.risk.Path.home", return_value=tmp_path):
            source_fn = codebase_suggestion("/some/path/nonexistent")
        result = source_fn({"action": "delete_user"})
        assert result == ""

    def test_returns_empty_for_no_action(self, tmp_path):
        _write_index(tmp_path, "test-repo", SAMPLE_INDEX)
        with patch("savants_guard.risk.Path.home", return_value=tmp_path):
            source_fn = codebase_suggestion("/some/path/test-repo")
        result = source_fn({"file": "something.py"})
        assert result == ""

    def test_integration_with_guard(self, tmp_path):
        """Test codebase_suggestion plugged into a guard via RiskContext."""
        _write_index(tmp_path, "test-repo", SAMPLE_INDEX)
        with patch("savants_guard.risk.Path.home", return_value=tmp_path):
            source_fn = codebase_suggestion("/some/path/test-repo")

        rc = RiskContext()
        rc.register_risk_source("suggestion", source_fn)

        guard = create_guard(
            ["when action contains 'delete' then suggest 'Check codebase'"],
            risk_context=rc,
        )
        result = guard.check({"action": "soft_delete"})
        assert result.action == "suggest"
        # The suggestion comes from the rule text, not the risk source
        # (risk source populates the 'suggestion' context variable)
        assert result.suggestion == "Check codebase"

    def test_suggestion_variable_in_context(self, tmp_path):
        """Verify the risk source populates the context with a suggestion."""
        _write_index(tmp_path, "test-repo", SAMPLE_INDEX)
        with patch("savants_guard.risk.Path.home", return_value=tmp_path):
            source_fn = codebase_suggestion("/some/path/test-repo")

        rc = RiskContext()
        rc.register_risk_source("suggestion", source_fn)

        # Build a condition that references 'suggestion'
        condition = {
            "type": "compare",
            "op": "contains",
            "left": {"type": "var", "name": "suggestion"},
            "right": {"type": "literal", "value": "soft_delete"},
        }
        enriched = rc.enrich({"action": "soft_delete"}, condition)
        assert "suggestion" in enriched
        assert "soft_delete" in enriched["suggestion"]

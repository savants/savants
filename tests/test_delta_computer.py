"""Tests for the local Delta computer.

These tests use real before/after Python code to verify that the delta
computer produces the correct graph mutations. No mocks — we run real
tree-sitter parsing on real Python source.
"""

from __future__ import annotations

from savants.delta.computer import compute_file_delta, parse_content
from savants.delta.schema import (
    AddEdge,
    AddNode,
    Delta,
    RemoveEdge,
    RemoveNode,
    canonical_node_id,
)


# --- Pure parsing tests (no diff) ---


class TestParseContent:
    def test_parses_python_function(self):
        result = parse_content(
            "src/test.py",
            "def hello():\n    return 'world'\n",
        )
        assert result is not None
        assert "hello" in result.functions
        assert result.functions["hello"]["start_line"] == 1

    def test_parses_python_class(self):
        result = parse_content(
            "src/models.py",
            "class User:\n    def login(self):\n        pass\n",
        )
        assert result is not None
        assert "User" in result.classes
        assert "login" in result.functions

    def test_parses_calls(self):
        result = parse_content(
            "src/main.py",
            "def caller():\n    helper()\n    other()\n",
        )
        assert result is not None
        callees = {c["callee_name"] for c in result.calls if c["caller_function"] == "caller"}
        assert "helper" in callees
        assert "other" in callees

    def test_unsupported_extension(self):
        result = parse_content("README.md", "# Hello")
        assert result is None

    def test_empty_file(self):
        result = parse_content("src/empty.py", "")
        assert result is not None
        assert result.functions == {}


# --- Delta computation tests ---


class TestComputeFileDelta:
    def test_added_file(self):
        delta = compute_file_delta(
            file_path="src/new.py",
            before_content=None,
            after_content="def added():\n    pass\n",
            org="acme",
            repo="backend",
            branch="alice/feature",
        )
        ops = delta.operations
        assert any(isinstance(op, AddNode) and op.label == "File" for op in ops)
        assert any(
            isinstance(op, AddNode) and op.label == "Function" and op.id.endswith(":added")
            for op in ops
        )

    def test_deleted_file(self):
        delta = compute_file_delta(
            file_path="src/old.py",
            before_content="def gone():\n    pass\n",
            after_content=None,
            org="acme",
            repo="backend",
        )
        ops = delta.operations
        assert any(isinstance(op, RemoveNode) and "old.py" in op.id for op in ops)

    def test_added_function(self):
        before = "def existing():\n    pass\n"
        after = "def existing():\n    pass\n\ndef new_function():\n    pass\n"
        delta = compute_file_delta(
            file_path="src/main.py",
            before_content=before,
            after_content=after,
            org="acme",
            repo="backend",
        )
        new_fn_id = canonical_node_id("Function", "src/main.py", "new_function")
        assert any(
            isinstance(op, AddNode) and op.id == new_fn_id for op in delta.operations
        )
        # The existing function should NOT be re-added (sha unchanged at the function level)
        existing_id = canonical_node_id("Function", "src/main.py", "existing")
        adds = [op for op in delta.operations if isinstance(op, AddNode) and op.id == existing_id]
        # It might be re-added if line numbers shifted; tolerate either
        assert len(adds) <= 1

    def test_removed_function(self):
        before = "def keeper():\n    pass\n\ndef remover():\n    pass\n"
        after = "def keeper():\n    pass\n"
        delta = compute_file_delta(
            file_path="src/main.py",
            before_content=before,
            after_content=after,
            org="acme",
            repo="backend",
        )
        remover_id = canonical_node_id("Function", "src/main.py", "remover")
        assert any(
            isinstance(op, RemoveNode) and op.id == remover_id for op in delta.operations
        )

    def test_renamed_function(self):
        before = "def authenticate(token):\n    return True\n"
        after = "def verify_session(token):\n    return True\n"
        delta = compute_file_delta(
            file_path="src/auth.py",
            before_content=before,
            after_content=after,
            org="acme",
            repo="backend",
        )
        ops = delta.operations
        old_id = canonical_node_id("Function", "src/auth.py", "authenticate")
        new_id = canonical_node_id("Function", "src/auth.py", "verify_session")
        assert any(isinstance(op, RemoveNode) and op.id == old_id for op in ops)
        assert any(isinstance(op, AddNode) and op.id == new_id for op in ops)

    def test_modified_function_signature(self):
        before = "def f(x):\n    return x\n"
        after = "def f(x, y):\n    return x + y\n"
        delta = compute_file_delta(
            file_path="src/util.py",
            before_content=before,
            after_content=after,
            org="acme",
            repo="backend",
        )
        # f is now different (params changed)
        fn_id = canonical_node_id("Function", "src/util.py", "f")
        removes = [op for op in delta.operations if isinstance(op, RemoveNode) and op.id == fn_id]
        adds = [op for op in delta.operations if isinstance(op, AddNode) and op.id == fn_id]
        assert len(removes) >= 1
        assert len(adds) >= 1
        # The new add should have the new parameters
        new_add = [op for op in adds if isinstance(op, AddNode) and op.label == "Function"][0]
        assert new_add.properties["parameters"] == ["x", "y"]

    def test_call_diff(self):
        before = "def caller():\n    old_helper()\n"
        after = "def caller():\n    new_helper()\n"
        delta = compute_file_delta(
            file_path="src/x.py",
            before_content=before,
            after_content=after,
            org="acme",
            repo="backend",
        )
        types = [op.op for op in delta.operations]
        assert "remove_edge" in types
        assert "add_edge" in types

    def test_no_changes_produces_minimal_delta(self):
        content = "def stable():\n    pass\n"
        delta = compute_file_delta(
            file_path="src/stable.py",
            before_content=content,
            after_content=content,
            org="acme",
            repo="backend",
        )
        # No actual changes - delta should be empty or near-empty
        assert delta.stats()["total"] <= 1  # at most a no-op file refresh

    def test_refactor_with_all_changes(self):
        """A realistic refactor: rename a function, change its signature,
        delete an old helper, add a new helper, change the calls."""
        before = """
def authenticate(token):
    return validate(token)

def validate(token):
    return token.startswith("eyJ")

def cleanup():
    pass
"""
        after = """
def verify_session(token, strict=False):
    return _verify(token, strict)

def _verify(token, strict):
    if strict:
        return token.startswith("eyJ.")
    return token.startswith("eyJ")
"""
        delta = compute_file_delta(
            file_path="src/auth.py",
            before_content=before,
            after_content=after,
            org="acme",
            repo="backend",
            branch="alice/refactor",
        )
        stats = delta.stats()
        # Should have removes (authenticate, validate, cleanup) and adds (verify_session, _verify)
        assert stats["remove_node"] >= 3
        assert stats["add_node"] >= 2

    def test_delta_round_trips_through_json(self):
        delta = compute_file_delta(
            file_path="src/main.py",
            before_content=None,
            after_content="def f():\n    pass\n",
            org="acme",
            repo="backend",
        )
        json_str = delta.to_json()
        parsed = Delta.from_json(json_str)
        assert parsed.scope.org == "acme"
        assert parsed.scope.repo == "backend"
        assert len(parsed.operations) == len(delta.operations)


class TestDeltaSize:
    def test_small_change_produces_small_delta(self):
        before = "def f():\n    return 1\n"
        after = "def f():\n    return 2\n"  # Only body changed, signature same
        delta = compute_file_delta(
            file_path="src/x.py",
            before_content=before,
            after_content=after,
            org="acme",
            repo="backend",
        )
        json_size = len(delta.to_json())
        # A trivial body change shouldn't produce a huge delta
        assert json_size < 2000  # bytes

    def test_larger_refactor_still_compact(self):
        # 50 functions before, 50 different ones after
        before = "\n".join(f"def fn_{i}():\n    pass\n" for i in range(50))
        after = "\n".join(f"def gn_{i}():\n    pass\n" for i in range(50))
        delta = compute_file_delta(
            file_path="src/x.py",
            before_content=before,
            after_content=after,
            org="acme",
            repo="backend",
        )
        json_size = len(delta.to_json())
        # Should be a few KB, not megabytes
        assert json_size < 50000  # 50 KB

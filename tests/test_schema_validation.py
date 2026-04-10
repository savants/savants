"""Schema contract tests: validate Cypher query generation is safe and correct."""

from __future__ import annotations

from savants.graph.schema import (
    SCHEMA_INDICES,
    ClassNode,
    FileNode,
    FunctionNode,
    create_class_query,
    create_edge_query,
    create_file_query,
    create_function_query,
)


class TestSchemaIndices:
    def test_all_indices_are_valid_cypher(self):
        for idx in SCHEMA_INDICES:
            assert idx.startswith("CREATE INDEX"), f"Invalid index: {idx}"
            assert "ON" in idx

    def test_index_covers_file_path(self):
        assert any("File" in idx and "path" in idx for idx in SCHEMA_INDICES)

    def test_index_covers_function_name(self):
        assert any("Function" in idx and "name" in idx for idx in SCHEMA_INDICES)

    def test_index_covers_class_name(self):
        assert any("Class" in idx and "name" in idx for idx in SCHEMA_INDICES)


class TestParameterizedQueries:
    """Ensure all queries use parameterized inputs (no injection risk)."""

    def test_file_query_parameterized(self):
        node = FileNode(
            path="'; DROP GRAPH --",
            language="python",
            line_count=1,
            sha256="x",
        )
        cypher, params = create_file_query(node)
        # The malicious path should be in params, not interpolated into cypher
        assert "'; DROP" not in cypher
        assert params["path"] == "'; DROP GRAPH --"
        assert "$path" in cypher

    def test_function_query_parameterized(self):
        node = FunctionNode(
            name="evil'); DELETE *; --",
            file_path="test.py",
            start_line=1,
            end_line=2,
        )
        cypher, params = create_function_query(node)
        assert "DELETE" not in cypher
        assert "$name" in cypher

    def test_class_query_parameterized(self):
        node = ClassNode(
            name="EvilClass",
            file_path="test.py",
            start_line=1,
            end_line=10,
        )
        cypher, params = create_class_query(node)
        assert "$name" in cypher
        assert "$file_path" in cypher


class TestEdgeQueries:
    EDGE_TYPES = ["CONTAINS", "CALLS", "INHERITS_FROM", "IMPORTS", "DEPENDS_ON", "DEFINES"]

    def test_all_edge_types_generate_valid_cypher(self):
        for edge_type in self.EDGE_TYPES:
            cypher, params = create_edge_query(
                "File", "path", "a.py",
                "Function", "name", "fn_a",
                edge_type,
            )
            assert f":{edge_type}" in cypher
            assert "MATCH" in cypher
            assert "MERGE" in cypher

    def test_edge_query_uses_params(self):
        cypher, params = create_edge_query(
            "File", "path", "src/main.py",
            "Function", "name", "main",
            "CONTAINS",
        )
        assert params["from_val"] == "src/main.py"
        assert params["to_val"] == "main"
        assert "$from_val" in cypher
        assert "$to_val" in cypher

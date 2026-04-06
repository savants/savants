"""Tests for graph schema queries — executed against real FalkorDB."""

from __future__ import annotations

import pytest

from synapcode.graph.schema import (
    ClassNode,
    FileNode,
    FunctionNode,
    create_class_query,
    create_edge_query,
    create_file_query,
    create_function_query,
)


class TestCypherGeneration:
    """Pure function tests — these don't need FalkorDB."""

    def test_create_file_query(self):
        node = FileNode(path="src/main.py", language="python", line_count=100, sha256="abc123")
        cypher, params = create_file_query(node)
        assert "MERGE" in cypher
        assert params["path"] == "src/main.py"

    def test_create_function_query(self):
        node = FunctionNode(
            name="process_data", file_path="src/main.py",
            start_line=10, end_line=25, parameters=["data", "config"],
        )
        cypher, params = create_function_query(node)
        assert "MERGE" in cypher
        assert params["parameters"] == ["data", "config"]

    def test_create_class_query(self):
        node = ClassNode(
            name="DataProcessor", file_path="src/processor.py",
            start_line=5, end_line=50, bases=["BaseProcessor"],
        )
        cypher, params = create_class_query(node)
        assert params["name"] == "DataProcessor"

    def test_create_edge_query(self):
        cypher, params = create_edge_query(
            "File", "path", "src/main.py",
            "Function", "name", "process_data",
            "CONTAINS",
        )
        assert "CONTAINS" in cypher
        assert params["from_val"] == "src/main.py"


@pytest.mark.integration
class TestGraphRoundtrip:
    """Verify nodes survive a write-then-read cycle in real FalkorDB."""

    def test_file_node_roundtrip(self, graph_client):
        node = FileNode(path="test/round.py", language="python", line_count=50, sha256="x")
        cypher, params = create_file_query(node)
        graph_client.query(cypher, params)

        result = graph_client.query(
            "MATCH (f:File {path: $p}) RETURN f.language, f.line_count",
            {"p": "test/round.py"},
        )
        assert result.result_set[0][0] == "python"
        assert result.result_set[0][1] == 50

    def test_function_node_roundtrip(self, graph_client):
        node = FunctionNode(name="my_func", file_path="a.py", start_line=1, end_line=5)
        cypher, params = create_function_query(node)
        graph_client.query(cypher, params)

        result = graph_client.query(
            "MATCH (fn:Function {name: 'my_func'}) RETURN fn.start_line, fn.end_line"
        )
        assert result.result_set[0] == [1, 5]

    def test_edge_roundtrip(self, graph_client):
        # Create two nodes and an edge
        graph_client.query(
            "CREATE (:File {path: 'a.py'}), (:Function {name: 'fn_a', file_path: 'a.py'})"
        )
        cypher, params = create_edge_query(
            "File", "path", "a.py", "Function", "name", "fn_a", "CONTAINS",
        )
        graph_client.query(cypher, params)

        result = graph_client.query(
            "MATCH (f:File)-[:CONTAINS]->(fn:Function) RETURN f.path, fn.name"
        )
        assert result.result_set[0] == ["a.py", "fn_a"]

    def test_node_count_after_insert(self, graph_client):
        assert graph_client.node_count() == 0
        graph_client.query("CREATE (:File {path: 'x.py'})")
        assert graph_client.node_count() == 1

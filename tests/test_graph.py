"""Tests for the graph schema and Code Property Graph builder."""

from __future__ import annotations

from pathlib import Path
from unittest.mock import MagicMock

from synapcode.graph.schema import (
    ClassNode,
    FileNode,
    FunctionNode,
    create_class_query,
    create_edge_query,
    create_file_query,
    create_function_query,
)


def test_create_file_query():
    node = FileNode(
        path="src/main.py",
        language="python",
        line_count=100,
        sha256="abc123",
        last_commit="def456",
    )
    cypher, params = create_file_query(node)
    assert "MERGE" in cypher
    assert params["path"] == "src/main.py"
    assert params["language"] == "python"


def test_create_function_query():
    node = FunctionNode(
        name="process_data",
        file_path="src/main.py",
        start_line=10,
        end_line=25,
        parameters=["data", "config"],
    )
    cypher, params = create_function_query(node)
    assert "MERGE" in cypher
    assert params["name"] == "process_data"
    assert params["parameters"] == ["data", "config"]


def test_create_class_query():
    node = ClassNode(
        name="DataProcessor",
        file_path="src/processor.py",
        start_line=5,
        end_line=50,
        bases=["BaseProcessor"],
    )
    cypher, params = create_class_query(node)
    assert "MERGE" in cypher
    assert params["name"] == "DataProcessor"


def test_create_edge_query():
    cypher, params = create_edge_query(
        "File", "path", "src/main.py",
        "Function", "name", "process_data",
        "CONTAINS",
    )
    assert "MATCH" in cypher
    assert "CONTAINS" in cypher
    assert params["from_val"] == "src/main.py"
    assert params["to_val"] == "process_data"

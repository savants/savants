"""Shared test fixtures."""

from __future__ import annotations

from unittest.mock import MagicMock

import pytest

from synapcode.graph.schema import ClassNode, FileNode, FunctionNode


@pytest.fixture
def mock_graph_client():
    """A mocked GraphClient that returns empty result sets by default."""
    client = MagicMock()
    mock_result = MagicMock()
    mock_result.result_set = []
    client.query.return_value = mock_result
    client.node_count.return_value = 42
    client.edge_count.return_value = 100
    return client


@pytest.fixture
def sample_file_node():
    return FileNode(
        path="src/main.py",
        language="python",
        line_count=150,
        sha256="a1b2c3d4e5f6",
        last_commit="abc123",
    )


@pytest.fixture
def sample_function_node():
    return FunctionNode(
        name="process_data",
        file_path="src/main.py",
        start_line=10,
        end_line=30,
        parameters=["data", "config"],
        return_type="dict",
    )


@pytest.fixture
def sample_class_node():
    return ClassNode(
        name="DataProcessor",
        file_path="src/processor.py",
        start_line=5,
        end_line=80,
        bases=["BaseProcessor"],
    )


# Marker for tests requiring a live FalkorDB instance
def pytest_configure(config):
    config.addinivalue_line(
        "markers",
        "integration: tests requiring live FalkorDB (deselect with -m 'not integration')",
    )

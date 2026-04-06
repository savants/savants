"""Shared test fixtures — ALL tests hit real FalkorDB, no mocks."""

from __future__ import annotations

import os
import subprocess
import uuid

import pytest
from falkordb import FalkorDB

from synapcode.config import FalkorDBConfig
from synapcode.graph.client import GraphClient


@pytest.fixture(scope="session")
def falkordb_connection():
    """Session-scoped: verify FalkorDB is reachable, return the connection."""
    host = os.environ.get("FALKORDB_HOST", "localhost")
    port = int(os.environ.get("FALKORDB_PORT", "6379"))
    try:
        db = FalkorDB(host=host, port=port)
        db.connection.ping()
        return db
    except Exception:
        pytest.skip(
            f"FalkorDB not reachable at {host}:{port}. "
            "Start it with: docker compose up -d falkordb"
        )


@pytest.fixture
def graph_client(falkordb_connection):
    """Per-test graph client with a unique graph name. Cleaned up after test."""
    graph_name = f"test_{uuid.uuid4().hex[:8]}"
    host = os.environ.get("FALKORDB_HOST", "localhost")
    port = int(os.environ.get("FALKORDB_PORT", "6379"))
    config = FalkorDBConfig(host=host, port=port, graph_name=graph_name)
    client = GraphClient(config)
    client.ensure_schema()
    yield client
    client.delete_graph()


@pytest.fixture
def test_repo(tmp_path):
    """Create a minimal git repo with known Python files for testing.

    Structure:
        src/utils.py     -> def helper(): return 42
        src/main.py      -> from utils import helper; def process(): helper()
        src/models.py    -> class DataModel: def validate(self): ...
    """
    repo = tmp_path / "test_repo"
    repo.mkdir()

    subprocess.run(["git", "init", str(repo)], check=True, capture_output=True)
    subprocess.run(
        ["git", "-C", str(repo), "config", "user.name", "Test"],
        check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "-C", str(repo), "config", "user.email", "test@test.com"],
        check=True, capture_output=True,
    )

    src = repo / "src"
    src.mkdir()

    (src / "utils.py").write_text(
        "def helper():\n"
        "    return 42\n"
        "\n"
        "def unused_util():\n"
        "    pass\n"
    )
    (src / "main.py").write_text(
        "from utils import helper\n"
        "\n"
        "def process():\n"
        "    return helper()\n"
        "\n"
        "def entry_point():\n"
        "    result = process()\n"
        "    return result\n"
    )
    (src / "models.py").write_text(
        "class DataModel:\n"
        "    def validate(self):\n"
        "        return True\n"
        "\n"
        "    def transform(self):\n"
        "        self.validate()\n"
        "        return {}\n"
    )

    subprocess.run(["git", "-C", str(repo), "add", "."], check=True, capture_output=True)
    subprocess.run(
        ["git", "-C", str(repo), "commit", "-m", "initial"],
        check=True, capture_output=True,
    )

    return repo


@pytest.fixture
def indexed_repo(test_repo, graph_client):
    """A test repo that has already been indexed into the graph."""
    from synapcode.graph.cpg import CodePropertyGraphBuilder

    builder = CodePropertyGraphBuilder(repo_path=test_repo, client=graph_client)
    stats = builder.build()
    return {"repo": test_repo, "client": graph_client, "stats": stats}

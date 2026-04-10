"""CLI tests using click.testing.CliRunner against real FalkorDB."""

from __future__ import annotations

import os
import subprocess

import pytest
from click.testing import CliRunner

from savants.cli import cli


@pytest.fixture
def runner():
    return CliRunner()


@pytest.fixture
def cli_repo(tmp_path):
    """A git repo for CLI tests."""
    repo = tmp_path / "cli_repo"
    repo.mkdir()
    subprocess.run(["git", "init", str(repo)], check=True, capture_output=True)
    subprocess.run(
        ["git", "-C", str(repo), "config", "user.name", "T"],
        check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "-C", str(repo), "config", "user.email", "t@t.com"],
        check=True, capture_output=True,
    )

    (repo / "main.py").write_text(
        "def greet(name):\n"
        "    return f'Hello {name}'\n\n"
        "def farewell(name):\n"
        "    return f'Bye {name}'\n"
    )
    (repo / "app.py").write_text(
        "from main import greet\n\n"
        "def run():\n"
        "    print(greet('world'))\n"
    )
    subprocess.run(["git", "-C", str(repo), "add", "."], check=True, capture_output=True)
    subprocess.run(
        ["git", "-C", str(repo), "commit", "-m", "init"],
        check=True, capture_output=True,
    )
    return repo


class TestCLIHelp:
    """Basic CLI wiring tests — no FalkorDB needed."""

    def test_help(self, runner):
        result = runner.invoke(cli, ["--help"])
        assert result.exit_code == 0
        assert "SynapCode" in result.output

    def test_init_help(self, runner):
        result = runner.invoke(cli, ["init", "--help"])
        assert result.exit_code == 0
        assert "First-time setup" in result.output

    def test_subcommands_listed(self, runner):
        result = runner.invoke(cli, ["--help"])
        for cmd in ["init", "index", "query", "impact", "search", "status", "gc", "serve", "worker"]:
            assert cmd in result.output


@pytest.mark.integration
class TestCLIInit:
    def test_init_indexes_repo(self, runner, cli_repo):
        result = runner.invoke(cli, ["init", str(cli_repo)])
        assert result.exit_code == 0
        assert "files" in result.output
        assert "functions" in result.output
        assert "Bookmark saved" in result.output

    def test_init_nonexistent_path(self, runner, tmp_path):
        result = runner.invoke(cli, ["init", str(tmp_path / "nope")])
        assert result.exit_code != 0


@pytest.mark.integration
class TestCLIIndex:
    def test_full_reindex(self, runner, cli_repo):
        # Init first
        runner.invoke(cli, ["init", str(cli_repo)])
        # Then full re-index
        result = runner.invoke(cli, ["index", "--full", str(cli_repo)])
        assert result.exit_code == 0
        assert "Full index" in result.output

    def test_incremental_no_changes(self, runner, cli_repo):
        runner.invoke(cli, ["init", str(cli_repo)])
        result = runner.invoke(cli, ["index", str(cli_repo)])
        assert result.exit_code == 0
        assert "up to date" in result.output

    def test_incremental_after_change(self, runner, cli_repo):
        runner.invoke(cli, ["init", str(cli_repo)])

        # Add a new file and commit
        (cli_repo / "extra.py").write_text("def extra(): pass\n")
        subprocess.run(["git", "-C", str(cli_repo), "add", "."], check=True, capture_output=True)
        subprocess.run(
            ["git", "-C", str(cli_repo), "commit", "-m", "add extra"],
            check=True, capture_output=True,
        )

        result = runner.invoke(cli, ["index", str(cli_repo)])
        assert result.exit_code == 0
        assert "Incremental" in result.output or "Updated" in result.output


@pytest.mark.integration
class TestCLISearch:
    def test_search_finds_function(self, runner, cli_repo):
        runner.invoke(cli, ["init", str(cli_repo)])
        result = runner.invoke(cli, ["search", "greet"])
        assert result.exit_code == 0
        assert "greet" in result.output

    def test_search_no_results(self, runner, cli_repo):
        runner.invoke(cli, ["init", str(cli_repo)])
        result = runner.invoke(cli, ["search", "zzz_nonexistent"])
        assert result.exit_code == 0
        assert "No matches" in result.output


@pytest.mark.integration
class TestCLIImpact:
    def test_impact_analysis(self, runner, cli_repo):
        runner.invoke(cli, ["init", str(cli_repo)])
        result = runner.invoke(cli, ["impact", "greet"])
        assert result.exit_code == 0
        assert "Impact analysis" in result.output


@pytest.mark.integration
class TestCLIStatus:
    def test_status_shows_connection(self, runner):
        result = runner.invoke(cli, ["status"])
        assert result.exit_code == 0
        assert "Index backend:" in result.output
        assert "connected" in result.output
        assert "RAM:" in result.output


@pytest.mark.integration
class TestCLIGC:
    def test_gc_runs(self, runner, cli_repo):
        runner.invoke(cli, ["init", str(cli_repo)])
        result = runner.invoke(cli, ["gc", str(cli_repo)])
        assert result.exit_code == 0
        assert "GC complete" in result.output

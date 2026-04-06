"""Tests for PydanticAI agent wrappers."""

from __future__ import annotations

from synapcode.agents.base import AgentInput, AgentOutput


class TestAgentModels:
    def test_agent_input_defaults(self):
        inp = AgentInput(prompt="What does this function do?")
        assert inp.model == "openai:gpt-4o"
        assert inp.max_retries == 3
        assert inp.timeout_seconds == 120

    def test_agent_input_custom(self):
        inp = AgentInput(
            prompt="Refactor this",
            model="anthropic:claude-sonnet-4-20250514",
            context={"graph": "42 nodes, 100 edges"},
            max_retries=5,
        )
        assert inp.model == "anthropic:claude-sonnet-4-20250514"
        assert "graph" in inp.context

    def test_agent_output_success(self):
        out = AgentOutput(response="Here's the analysis...", model_used="gpt-4o")
        assert out.success is True
        assert out.error == ""

    def test_agent_output_failure(self):
        out = AgentOutput(
            response="",
            model_used="gpt-4o",
            success=False,
            error="API timeout",
        )
        assert not out.success
        assert "timeout" in out.error

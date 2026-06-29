"""
Tests for framework integrations.

These test the guard logic without requiring the actual frameworks installed.
They mock the framework imports and verify the integration functions produce
correct behavior.
"""

import sys
import types
import pytest
from unittest.mock import MagicMock, patch
from savants_guard import create_guard, GuardError


class TestLangChainCallback:
    """Test LangChain callback handler via mock."""

    def setup_method(self):
        # Mock langchain_core so import doesn't fail
        mock_module = types.ModuleType("langchain_core")
        mock_callbacks = types.ModuleType("langchain_core.callbacks")
        mock_base = types.ModuleType("langchain_core.callbacks.base")

        class MockBaseCallbackHandler:
            pass

        mock_base.BaseCallbackHandler = MockBaseCallbackHandler
        mock_module.callbacks = mock_callbacks
        mock_callbacks.base = mock_base

        sys.modules["langchain_core"] = mock_module
        sys.modules["langchain_core.callbacks"] = mock_callbacks
        sys.modules["langchain_core.callbacks.base"] = mock_base

    def teardown_method(self):
        for k in list(sys.modules.keys()):
            if k.startswith("langchain"):
                del sys.modules[k]

    def test_callback_blocks_on_match(self):
        from savants_guard.integrations import langchain_callback
        guard = create_guard(["when action contains 'delete' then block"])
        handler = langchain_callback(guard)

        with pytest.raises(ValueError, match="Guard blocked"):
            handler.on_tool_start(
                {"name": "delete_database"},
                "delete all records",
            )

    def test_callback_allows_safe_action(self):
        from savants_guard.integrations import langchain_callback
        guard = create_guard(["when action contains 'delete' then block"])
        handler = langchain_callback(guard)

        # Should not raise
        handler.on_tool_start({"name": "read_logs"}, "read recent logs")

    def test_callback_warns_on_suggest(self):
        from savants_guard.integrations import langchain_callback
        guard = create_guard(["when action contains 'chmod' then suggest 'Use 755'"])
        handler = langchain_callback(guard)

        import warnings
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            handler.on_tool_start({"name": "chmod_file"}, "chmod 777")
            assert len(w) == 1
            assert "suggests" in str(w[0].message)


class TestCrewAIHook:
    """Test CrewAI before_tool_call hook via mock."""

    def setup_method(self):
        # Mock crewai.hooks
        mock_crewai = types.ModuleType("crewai")
        mock_hooks = types.ModuleType("crewai.hooks")

        self.registered_hooks = []

        def mock_before_tool_call(fn):
            self.registered_hooks.append(fn)
            return fn

        mock_hooks.before_tool_call = mock_before_tool_call
        mock_crewai.hooks = mock_hooks

        sys.modules["crewai"] = mock_crewai
        sys.modules["crewai.hooks"] = mock_hooks

    def teardown_method(self):
        for k in list(sys.modules.keys()):
            if k.startswith("crewai"):
                del sys.modules[k]

    def test_hook_blocks_on_match(self):
        from savants_guard.integrations import crewai_hook
        guard = create_guard(["when action contains 'delete' then block"])
        crewai_hook(guard)

        assert len(self.registered_hooks) == 1
        hook = self.registered_hooks[0]

        context = MagicMock()
        context.tool_name = "delete_database"
        context.tool_input = {"table": "users"}

        result = hook(context)
        assert result is False  # blocked

    def test_hook_allows_safe_action(self):
        from savants_guard.integrations import crewai_hook
        guard = create_guard(["when action contains 'delete' then block"])
        crewai_hook(guard)

        hook = self.registered_hooks[0]

        context = MagicMock()
        context.tool_name = "read_logs"
        context.tool_input = {}

        result = hook(context)
        assert result is None  # allowed

    def test_hook_rewrites_command(self):
        from savants_guard.integrations import crewai_hook
        guard = create_guard(["when action contains 'push' then rewrite 'git push --force-with-lease'"])
        crewai_hook(guard)

        hook = self.registered_hooks[0]

        context = MagicMock()
        context.tool_name = "push_code"
        context.tool_input = {"command": "git push --force"}

        result = hook(context)
        assert result is None  # allowed (rewrite, not block)
        assert context.tool_input["command"] == "git push --force-with-lease"


class TestOpenAIToolGuardrail:
    """Test OpenAI Agents SDK tool_input_guardrail via mock."""

    def setup_method(self):
        # Mock agents module
        mock_agents = types.ModuleType("agents")

        class MockToolGuardrailFunctionOutput:
            def __init__(self, allowed=True, message=None):
                self._allowed = allowed
                self.message = message

            @classmethod
            def allow(cls, output_info=None):
                return cls(allowed=True)

            @classmethod
            def reject_content(cls, message, output_info=None):
                return cls(allowed=False, message=message)

            @classmethod
            def raise_exception(cls, output_info=None):
                raise Exception("Guardrail exception")

        def mock_tool_input_guardrail(fn):
            fn._is_guardrail = True
            return fn

        mock_agents.ToolGuardrailFunctionOutput = MockToolGuardrailFunctionOutput
        mock_agents.tool_input_guardrail = mock_tool_input_guardrail

        sys.modules["agents"] = mock_agents
        self.MockOutput = MockToolGuardrailFunctionOutput

    def teardown_method(self):
        if "agents" in sys.modules:
            del sys.modules["agents"]

    def test_guardrail_blocks_on_match(self):
        from savants_guard.integrations import openai_tool_guardrail
        guard = create_guard(["when action contains 'delete' then block"])
        guardrail = openai_tool_guardrail(guard)

        data = MagicMock()
        data.context.tool_name = "delete_user"
        data.context.tool_arguments = '{"user_id": "123"}'

        result = guardrail(data)
        assert result._allowed is False
        assert "Blocked" in result.message

    def test_guardrail_allows_safe_action(self):
        from savants_guard.integrations import openai_tool_guardrail
        guard = create_guard(["when action contains 'delete' then block"])
        guardrail = openai_tool_guardrail(guard)

        data = MagicMock()
        data.context.tool_name = "read_data"
        data.context.tool_arguments = '{}'

        result = guardrail(data)
        assert result._allowed is True

    def test_guardrail_suggests_alternative(self):
        from savants_guard.integrations import openai_tool_guardrail
        guard = create_guard(["when action contains 'chmod' then suggest 'Use 755'"])
        guardrail = openai_tool_guardrail(guard)

        data = MagicMock()
        data.context.tool_name = "chmod_file"
        data.context.tool_arguments = '{"mode": "777"}'

        result = guardrail(data)
        assert result._allowed is False
        assert "suggests" in result.message


class TestIntegrationImportErrors:
    """Verify clean error messages when frameworks aren't installed."""

    def test_langchain_import_error(self):
        # Remove any mock
        for k in list(sys.modules.keys()):
            if k.startswith("langchain"):
                del sys.modules[k]

        from savants_guard.integrations import langchain_callback
        guard = create_guard([])

        with pytest.raises(ImportError, match="langchain-core"):
            langchain_callback(guard)

    def test_crewai_import_error(self):
        for k in list(sys.modules.keys()):
            if k.startswith("crewai"):
                del sys.modules[k]

        from savants_guard.integrations import crewai_hook
        guard = create_guard([])

        with pytest.raises(ImportError, match="crewai"):
            crewai_hook(guard)

    def test_openai_agents_import_error(self):
        if "agents" in sys.modules:
            del sys.modules["agents"]

        from savants_guard.integrations import openai_tool_guardrail
        guard = create_guard([])

        with pytest.raises(ImportError, match="openai-agents"):
            openai_tool_guardrail(guard)

"""
Framework integrations for savants-guard.

Provides drop-in guardrails for LangChain, CrewAI, and OpenAI Agents SDK.
All integrations use the same Guard instance and DSL rules.

Usage:
    from savants_guard import create_guard
    from savants_guard.integrations import langchain_callback, crewai_hook, openai_tool_guardrail

    guard = create_guard(["when action contains 'delete' then block"])

    # LangChain: add as callback
    handler = langchain_callback(guard)
    agent.invoke(input, config={"callbacks": [handler]})

    # CrewAI: register as before_tool_call hook
    crewai_hook(guard)

    # OpenAI Agents SDK: use as tool_input_guardrail
    guardrail = openai_tool_guardrail(guard)
"""

from __future__ import annotations

from typing import Any

from .guard import Guard
from .types import GuardResult


# ============================================================
# LangChain Integration
# ============================================================

def langchain_callback(guard: Guard):
    """
    Create a LangChain BaseCallbackHandler that logs guard evaluations.

    NOTE: LangChain callbacks are observer-only and cannot block tool execution.
    For blocking, use langchain_tool_wrapper() to wrap individual tools.

    Usage:
        from savants_guard.integrations import langchain_callback
        handler = langchain_callback(guard)
        agent.invoke(input, config={"callbacks": [handler]})
    """
    try:
        from langchain_core.callbacks.base import BaseCallbackHandler
    except ImportError:
        raise ImportError("Install langchain-core: pip install langchain-core")

    class SavantsGuardCallback(BaseCallbackHandler):
        def on_tool_start(self, serialized, input_str, *, run_id=None, **kwargs):
            tool_name = serialized.get("name", "unknown")
            inputs = kwargs.get("inputs", {})
            context = {"action": tool_name, "tool": tool_name, **inputs}
            result = guard.check(context)
            if result.blocked:
                raise ValueError(
                    f"Savants Guard blocked: {result.rule}"
                )
            if result.action == "suggest" and result.suggestion:
                import warnings
                warnings.warn(f"Savants Guard suggests: {result.suggestion}")

    return SavantsGuardCallback()


def langchain_tool_wrapper(guard: Guard):
    """
    Wrap a LangChain tool so guard.check() runs before execution.

    Usage:
        from savants_guard.integrations import langchain_tool_wrapper
        wrapper = langchain_tool_wrapper(guard)
        safe_tool = wrapper(my_tool)
    """
    def wrapper(tool):
        original_run = tool._run if hasattr(tool, '_run') else tool.run

        def guarded_run(*args, **kwargs):
            tool_name = getattr(tool, 'name', 'unknown')
            context = {"action": tool_name, "tool": tool_name, **kwargs}
            result = guard.check(context)
            if result.blocked:
                return f"BLOCKED by guard rule: {result.rule}"
            if result.action == "rewrite" and result.suggestion:
                kwargs["command"] = result.suggestion
            return original_run(*args, **kwargs)

        if hasattr(tool, '_run'):
            tool._run = guarded_run
        else:
            tool.run = guarded_run
        return tool

    return wrapper


# ============================================================
# CrewAI Integration
# ============================================================

def crewai_hook(guard: Guard):
    """
    Register a CrewAI before_tool_call hook that enforces guard rules.

    Usage:
        from savants_guard.integrations import crewai_hook
        crewai_hook(guard)  # registers globally, no return value needed
    """
    try:
        from crewai.hooks import before_tool_call
    except ImportError:
        raise ImportError("Install crewai: pip install crewai")

    @before_tool_call
    def savants_guard_hook(context):
        tool_name = context.tool_name
        tool_input = context.tool_input if isinstance(context.tool_input, dict) else {}
        check_context = {"action": tool_name, "tool": tool_name, **tool_input}
        result = guard.check(check_context)

        if result.blocked:
            return False  # block execution

        if result.action == "rewrite" and result.suggestion:
            if "command" in tool_input:
                context.tool_input["command"] = result.suggestion

        return None  # allow execution


# ============================================================
# OpenAI Agents SDK Integration
# ============================================================

def openai_tool_guardrail(guard: Guard):
    """
    Create an OpenAI Agents SDK tool_input_guardrail.

    Usage:
        from savants_guard.integrations import openai_tool_guardrail
        guardrail = openai_tool_guardrail(guard)

        @function_tool(tool_input_guardrails=[guardrail])
        def my_tool(query: str) -> str:
            ...
    """
    try:
        from agents import (
            ToolGuardrailFunctionOutput,
            tool_input_guardrail,
        )
    except ImportError:
        raise ImportError("Install openai-agents: pip install openai-agents")

    @tool_input_guardrail
    def savants_guard_check(data):
        import json
        tool_name = data.context.tool_name if hasattr(data.context, 'tool_name') else "unknown"
        args = {}
        if hasattr(data.context, 'tool_arguments') and data.context.tool_arguments:
            try:
                args = json.loads(data.context.tool_arguments)
            except (json.JSONDecodeError, TypeError):
                args = {}

        context = {"action": tool_name, "tool": tool_name, **args}
        result = guard.check(context)

        if result.blocked:
            return ToolGuardrailFunctionOutput.reject_content(
                f"Blocked by guard rule: {result.rule}"
            )

        if result.action == "suggest" and result.suggestion:
            return ToolGuardrailFunctionOutput.reject_content(
                f"Guard suggests: {result.suggestion}"
            )

        return ToolGuardrailFunctionOutput.allow()

    return savants_guard_check


def openai_input_guardrail(guard: Guard):
    """
    Create an OpenAI Agents SDK input_guardrail for the Agent.

    Usage:
        from savants_guard.integrations import openai_input_guardrail
        guardrail = openai_input_guardrail(guard)

        agent = Agent(
            name="My agent",
            input_guardrails=[guardrail],
        )
    """
    try:
        from agents import (
            GuardrailFunctionOutput,
            input_guardrail,
        )
    except ImportError:
        raise ImportError("Install openai-agents: pip install openai-agents")

    @input_guardrail
    async def savants_guard_input(ctx, agent, input):
        input_text = input if isinstance(input, str) else str(input)
        context = {"action": "user_input", "content": input_text, "input": input_text}
        result = guard.check(context)

        return GuardrailFunctionOutput(
            output_info={"rule": result.rule, "action": result.action},
            tripwire_triggered=result.blocked,
        )

    return savants_guard_input

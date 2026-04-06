"""TemporalAgent: wraps a PydanticAI Agent in Temporal durable execution.

A PydanticAI Agent normally runs in-process. If the process crashes mid-
reasoning, all context is lost. TemporalAgent offloads the agent's run()
to a Temporal activity — giving it automatic retry, timeout management,
and crash recovery for free.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass
from datetime import timedelta
from typing import Any

from pydantic import BaseModel
from temporalio import activity, workflow

logger = logging.getLogger(__name__)


class AgentInput(BaseModel):
    """Input to a durable agent execution."""

    prompt: str
    model: str = "openai:gpt-4o"
    context: dict[str, Any] = {}
    max_retries: int = 3
    timeout_seconds: int = 120


class AgentOutput(BaseModel):
    """Output from a durable agent execution."""

    response: str
    model_used: str
    tool_calls: list[str] = []
    success: bool = True
    error: str = ""


@activity.defn
async def run_pydantic_agent(input: AgentInput) -> AgentOutput:
    """Temporal activity: execute a PydanticAI agent run.

    This is where the actual LLM call happens. Temporal manages retries
    and timeouts around this activity.
    """
    from pydantic_ai import Agent

    try:
        agent = Agent(
            input.model,
            system_prompt=(
                "You are SynapCode, an AI assistant with deep structural "
                "understanding of codebases via a knowledge graph. "
                "Use the provided graph context to give accurate, "
                "architecture-aware answers."
            ),
        )

        # Build the prompt with injected context
        full_prompt = input.prompt
        if input.context:
            context_str = "\n".join(f"- {k}: {v}" for k, v in input.context.items())
            full_prompt = f"Context:\n{context_str}\n\nQuery: {input.prompt}"

        result = await agent.run(full_prompt)

        return AgentOutput(
            response=result.data,
            model_used=input.model,
            success=True,
        )
    except Exception as e:
        logger.error("Agent execution failed: %s", e)
        return AgentOutput(
            response="",
            model_used=input.model,
            success=False,
            error=str(e),
        )


@workflow.defn
class DurableAgentWorkflow:
    """Temporal workflow: orchestrates a PydanticAI agent with crash recovery.

    If the LLM API times out or the worker crashes, Temporal replays
    the event history and retries the activity automatically.
    """

    @workflow.run
    async def run(self, input: AgentInput) -> AgentOutput:
        return await workflow.execute_activity(
            run_pydantic_agent,
            input,
            start_to_close_timeout=timedelta(seconds=input.timeout_seconds),
            retry_policy=workflow.RetryPolicy(
                maximum_attempts=input.max_retries,
                initial_interval=timedelta(seconds=2),
                backoff_coefficient=2.0,
                maximum_interval=timedelta(seconds=30),
            ),
        )

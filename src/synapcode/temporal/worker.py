"""Temporal worker entry point.

Starts a worker that polls the SynapCode task queue and executes
workflows and activities. Run with: python -m synapcode.temporal.worker
"""

from __future__ import annotations

import asyncio
import logging

from temporalio.client import Client
from temporalio.worker import Worker

from synapcode.config import load_config
from synapcode.agents.base import DurableAgentWorkflow, run_pydantic_agent
from synapcode.temporal.activities import (
    call_llm,
    compute_git_diff,
    index_file,
    remove_file_from_graph,
    snapshot_graph,
)
from synapcode.temporal.gc_workflow import GraphGCWorkflow, run_gc_activity
from synapcode.temporal.workflows import (
    AgentReasoningWorkflow,
    FullIndexWorkflow,
    IncrementalSyncWorkflow,
    SnapshotWorkflow,
)

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


async def main() -> None:
    config = load_config()

    logger.info("Connecting to Temporal at %s", config.temporal.host)
    client = await Client.connect(config.temporal.host)

    logger.info("Starting worker on task queue '%s'", config.temporal.task_queue)
    worker = Worker(
        client,
        task_queue=config.temporal.task_queue,
        workflows=[
            FullIndexWorkflow,
            IncrementalSyncWorkflow,
            SnapshotWorkflow,
            AgentReasoningWorkflow,
            GraphGCWorkflow,
            DurableAgentWorkflow,
        ],
        activities=[
            index_file,
            compute_git_diff,
            call_llm,
            snapshot_graph,
            remove_file_from_graph,
            run_gc_activity,
            run_pydantic_agent,
        ],
    )

    logger.info("Worker started. Listening for tasks...")
    await worker.run()


if __name__ == "__main__":
    asyncio.run(main())

"""Manifold Pipe for Open WebUI: exposes SynapCode as a model provider.

This pipeline registers with Open WebUI as a "model" that users can select.
It combines GraphRAG context injection with selective deferred routing
to transparently choose the best backend for each query.
"""

from __future__ import annotations

import logging
from typing import Any, Generator

from savants.graph.client import GraphClient
from savants.hardware.monitor import get_system_stats
from savants.pipelines.graphrag_inlet import build_graph_context
from savants.pipelines.router import RouteDecision, route_request

logger = logging.getLogger(__name__)


class Pipeline:
    """Open WebUI Manifold Pipeline for SynapCode.

    Appears as a selectable model in the Open WebUI interface.
    Handles routing, context injection, and streaming responses.
    """

    class Valves:
        """Configurable parameters exposed in the Open WebUI admin panel."""

        def __init__(self):
            self.local_model: str = "qwen2.5-coder:14b"
            self.local_url: str = "http://localhost:11434"
            self.frontier_model: str = "claude-sonnet-4-20250514"
            self.frontier_url: str = "https://api.anthropic.com"
            self.frontier_api_key: str = ""
            self.complexity_threshold: float = 0.7
            self.ram_threshold: float = 60.0
            self.max_context_tokens: int = 2000
            self.falkordb_host: str = "localhost"
            self.falkordb_port: int = 6379
            self.graph_name: str = "savants"

    def __init__(self):
        self.name = "SynapCode GraphRAG"
        self.valves = self.Valves()
        self._client: GraphClient | None = None

    def _get_client(self) -> GraphClient:
        if self._client is None:
            from savants.config import FalkorDBConfig

            config = FalkorDBConfig(
                host=self.valves.falkordb_host,
                port=self.valves.falkordb_port,
                graph_name=self.valves.graph_name,
            )
            self._client = GraphClient(config)
        return self._client

    def pipelines(self) -> list[dict]:
        """Register available model endpoints."""
        return [
            {
                "id": "savants-auto",
                "name": "SynapCode (Auto-Route)",
            },
            {
                "id": "savants-local",
                "name": "SynapCode (Local Only)",
            },
            {
                "id": "savants-frontier",
                "name": "SynapCode (Frontier Only)",
            },
        ]

    def pipe(
        self,
        body: dict[str, Any],
    ) -> str | Generator:
        """Process a chat request through the SynapCode pipeline.

        1. Extract graph context from FalkorDB
        2. Route to local SLM or frontier API
        3. Return the response
        """
        messages = body.get("messages", [])
        model_id = body.get("model", "savants-auto")

        # Step 1: Inject graph context into the conversation
        if messages:
            last_user_msg = ""
            for msg in reversed(messages):
                if msg.get("role") == "user":
                    last_user_msg = msg["content"]
                    break

            if last_user_msg:
                try:
                    context = build_graph_context(
                        last_user_msg,
                        self._get_client(),
                        self.valves.max_context_tokens,
                    )
                    if context:
                        # Prepend context as a system message
                        messages.insert(0, {
                            "role": "system",
                            "content": (
                                "You are SynapCode, an AI assistant with deep structural "
                                "understanding of the user's codebase via a knowledge graph. "
                                "Use the following graph context to provide accurate answers.\n\n"
                                f"{context}"
                            ),
                        })
                except Exception as e:
                    logger.warning("Failed to build graph context: %s", e)

        # Step 2: Route the request
        if model_id == "savants-local":
            target_model = self.valves.local_model
            target_url = self.valves.local_url
        elif model_id == "savants-frontier":
            target_model = self.valves.frontier_model
            target_url = self.valves.frontier_url
        else:
            # Auto-route based on complexity and hardware
            stats = get_system_stats()
            routing = route_request(
                query=last_user_msg if messages else "",
                local_model=self.valves.local_model,
                local_url=self.valves.local_url,
                frontier_model=self.valves.frontier_model,
                frontier_url=self.valves.frontier_url,
                complexity_threshold=self.valves.complexity_threshold,
                local_ram_pct=stats.ram_percent,
                ram_threshold=self.valves.ram_threshold,
            )
            target_model = routing.model
            target_url = routing.api_url
            logger.info(
                "Routed to %s (%s): %s",
                routing.decision.value,
                target_model,
                routing.reason,
            )

        # Step 3: Forward to the target model
        import httpx

        headers = {"Content-Type": "application/json"}
        if target_url != self.valves.local_url and self.valves.frontier_api_key:
            headers["Authorization"] = f"Bearer {self.valves.frontier_api_key}"

        payload = {
            "model": target_model,
            "messages": messages,
            "stream": body.get("stream", False),
        }

        with httpx.Client(timeout=120.0) as http:
            response = http.post(
                f"{target_url}/v1/chat/completions",
                json=payload,
                headers=headers,
            )
            response.raise_for_status()
            data = response.json()

        return data["choices"][0]["message"]["content"]

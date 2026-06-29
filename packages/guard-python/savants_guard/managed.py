"""
Managed Guard Client — syncs rules from Savants cloud.

Fetches rule bundles, polls for changes, batches events.
Uses httpx for async HTTP. Only imported when managed=True.
"""

from __future__ import annotations

import asyncio
import json
from datetime import datetime, timezone
from typing import Any, Callable

from .types import ASTNode, ParsedRule


class ManagedGuardClient:
    """Async client for Savants cloud rule management."""

    def __init__(
        self,
        api_key: str,
        api_url: str = "https://api.savants.cloud",
        poll_interval: int = 30,
        batch_size: int = 50,
        batch_interval: int = 30,
    ):
        self._api_key = api_key
        self._api_url = api_url
        self._poll_interval = poll_interval
        self._batch_size = batch_size
        self._batch_interval = batch_interval
        self._current_hash = ""
        self._bundle_version = 0
        self._event_queue: list[dict[str, Any]] = []
        self._poll_task: asyncio.Task[None] | None = None
        self._flush_task: asyncio.Task[None] | None = None

    async def fetch_bundle(self) -> list[ParsedRule]:
        """Fetch rule bundle from cloud. Returns parsed rules or empty list if unchanged."""
        import httpx

        headers = {"Authorization": f"Bearer {self._api_key}"}
        if self._current_hash:
            headers["If-None-Match"] = self._current_hash

        async with httpx.AsyncClient() as client:
            resp = await client.get(
                f"{self._api_url}/api/v1/guard/bundle",
                headers=headers,
                timeout=10,
            )

        if resp.status_code == 304:
            return []

        if resp.status_code != 200:
            raise RuntimeError(f"Failed to fetch bundle: {resp.status_code}")

        data = resp.json()
        self._current_hash = data.get("hash", "")
        self._bundle_version = data.get("version", 0)

        rules: list[ParsedRule] = []
        for r in data.get("rules", []):
            parsed = json.loads(r["ast_json"])
            rules.append(ParsedRule(
                dsl=r["dsl"],
                condition=parsed["condition"],
                action=parsed["action"],
            ))
        return rules

    def start_polling(self, on_update: Callable[[list[ParsedRule]], None]) -> None:
        """Start background polling for rule changes."""
        if self._poll_task is not None:
            return

        async def _poll_loop() -> None:
            while True:
                await asyncio.sleep(self._poll_interval)
                try:
                    rules = await self.fetch_bundle()
                    if rules:
                        on_update(rules)
                except Exception:
                    pass  # Silent fail, keep using cached rules

        self._poll_task = asyncio.get_event_loop().create_task(_poll_loop())

    def stop_polling(self) -> None:
        """Stop polling."""
        if self._poll_task:
            self._poll_task.cancel()
            self._poll_task = None

    def report_event(self, event: dict[str, Any]) -> None:
        """Queue a guard evaluation event. Auto-flushes when batch is full."""
        event["bundle_version"] = self._bundle_version
        self._event_queue.append(event)
        if len(self._event_queue) >= self._batch_size:
            asyncio.get_event_loop().create_task(self.flush())

    def start_flushing(self) -> None:
        """Start periodic flush timer."""
        if self._flush_task is not None:
            return

        async def _flush_loop() -> None:
            while True:
                await asyncio.sleep(self._batch_interval)
                if self._event_queue:
                    await self.flush()

        self._flush_task = asyncio.get_event_loop().create_task(_flush_loop())

    async def flush(self) -> None:
        """Flush queued events to cloud."""
        if not self._event_queue:
            return

        events = self._event_queue[:]
        self._event_queue.clear()

        try:
            import httpx
            async with httpx.AsyncClient() as client:
                await client.post(
                    f"{self._api_url}/api/v1/guard/events",
                    json={"events": events},
                    headers={"Authorization": f"Bearer {self._api_key}",
                             "Content-Type": "application/json"},
                    timeout=5,
                )
        except Exception:
            # Re-queue on failure
            self._event_queue = events + self._event_queue

    async def close(self) -> None:
        """Stop polling and flush remaining events."""
        self.stop_polling()
        if self._flush_task:
            self._flush_task.cancel()
            self._flush_task = None
        await self.flush()

"""
Guard — deterministic guardrails for AI agents.

create_guard() is the main entry point. Returns a Guard object
that evaluates rules locally with zero latency.
"""

from __future__ import annotations

import functools
from datetime import datetime, timezone
from typing import Any, Callable, TypeVar

from .types import GuardResult, GuardError, ParsedRule
from .parser import parse_rule
from .ast_eval import evaluate
from .risk import RiskContext

F = TypeVar("F", bound=Callable[..., Any])

BLOCKING_ACTIONS = frozenset({"block", "block_deploy", "deny", "require_approval"})
SOFT_ACTIONS = frozenset({"suggest", "rewrite", "ask"})


class Guard:
    """
    Deterministic guard that evaluates DSL rules against context.

    Usage:
        guard = create_guard(["when action contains 'delete' then block"])
        result = guard.check({"action": "delete_db"})
        # result.blocked == True
    """

    def __init__(
        self,
        rules: list[ParsedRule],
        *,
        managed: bool = False,
        risk_context: RiskContext | None = None,
    ):
        self._rules = list(rules)
        self._log: list[dict[str, Any]] = []
        self._managed = managed
        self._client: Any = None  # ManagedGuardClient, set externally
        self._risk_context = risk_context

    @property
    def managed(self) -> bool:
        return self._managed

    def check(self, context: dict[str, Any]) -> GuardResult:
        """
        Check if an action is allowed. Always synchronous, never throws.

        Returns GuardResult with blocked/allowed/rule/action fields.
        """
        for rule in self._rules:
            # Lazy risk enrichment: only resolve sources referenced by this rule
            eval_context = context
            if self._risk_context is not None:
                eval_context = self._risk_context.enrich(context, rule.condition)

            result = evaluate(rule.condition, eval_context)
            if result and (rule.action in BLOCKING_ACTIONS or rule.action in SOFT_ACTIONS):
                is_hard_block = rule.action in BLOCKING_ACTIONS
                guard_result = GuardResult(
                    blocked=is_hard_block,
                    allowed=False,
                    rule=rule.dsl,
                    action=rule.action,
                    context=context,
                    suggestion=rule.suggestion or None,
                )
                self._log.append({
                    "timestamp": datetime.now(timezone.utc).isoformat(),
                    "context": context,
                    "result": guard_result,
                })
                # Report to cloud if managed
                if self._client:
                    self._client.report_event({
                        "context_hash": str(len(str(context))),
                        "action": str(context.get("action", "")),
                        "tool": str(context.get("tool", "")),
                        "result": "blocked",
                        "matched_rule": rule.dsl,
                        "timestamp": datetime.now(timezone.utc).isoformat(),
                    })
                return guard_result

        guard_result = GuardResult(
            blocked=False,
            allowed=True,
            rule=None,
            action=None,
            context=context,
        )
        self._log.append({
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "context": context,
            "result": guard_result,
        })
        if self._client:
            self._client.report_event({
                "context_hash": str(len(str(context))),
                "action": str(context.get("action", "")),
                "tool": str(context.get("tool", "")),
                "result": "allowed",
                "timestamp": datetime.now(timezone.utc).isoformat(),
            })
        return guard_result

    def wrap(self, fn: F) -> F:
        """
        Wrap a function — raises GuardError if any rule blocks.

        Can be used as a decorator:
            @guard.wrap
            def delete_user(user_id, env="production"):
                ...
        """
        @functools.wraps(fn)
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            # Build context from first arg (if dict) or kwargs
            if args and isinstance(args[0], dict):
                context = args[0]
            else:
                context = kwargs
            result = self.check(context)
            if result.blocked:
                raise GuardError(result.rule or "", result.action or "", context)
            return fn(*args, **kwargs)
        return wrapper  # type: ignore

    def add_rule(self, dsl: str) -> None:
        """Add a rule at runtime."""
        parsed = parse_rule(dsl)
        if parsed:
            self._rules.append(parsed)

    def list_rules(self) -> list[str]:
        """List all active rules as DSL strings."""
        return [r.dsl for r in self._rules]

    def get_log(self) -> list[dict[str, Any]]:
        """Get the evaluation log."""
        return list(self._log)

    async def close(self) -> None:
        """Stop polling and flush events (managed mode)."""
        if self._client:
            await self._client.close()


def create_guard(
    rules: list[str],
    *,
    managed: bool = False,
    api_key: str | None = None,
    api_url: str | None = None,
    poll_interval: int = 30,
    risk_context: RiskContext | None = None,
) -> Guard:
    """
    Create a guard from human-readable DSL rules.

    Args:
        rules: List of DSL rule strings.
        managed: If True, sync rules from Savants cloud.
        api_key: API key for managed mode (sk_live_...).
        api_url: Cloud API URL (default: https://api.savants.cloud).
        poll_interval: Seconds between rule polls (default: 30).
        risk_context: Optional RiskContext for graph-powered risk scoring.

    Returns:
        Guard object. If managed=True, call this with await.

    Examples:
        # Local mode (synchronous)
        guard = create_guard(["when action contains 'delete' then block"])

        # With risk context
        rc = RiskContext()
        guard = create_guard(rules, risk_context=rc)

        # Managed mode (async)
        guard = await create_guard(rules, managed=True, api_key="sk_live_...")
    """
    parsed_rules = [r for dsl in rules if (r := parse_rule(dsl)) is not None]
    guard = Guard(parsed_rules, managed=managed, risk_context=risk_context)

    if managed and api_key:
        # Import managed client only when needed
        from .managed import ManagedGuardClient

        async def _init_managed() -> Guard:
            client = ManagedGuardClient(
                api_key=api_key,
                api_url=api_url or "https://api.savants.cloud",
                poll_interval=poll_interval,
            )
            guard._client = client
            guard._managed = True

            try:
                managed_rules = await client.fetch_bundle()
                for r in managed_rules:
                    guard._rules.append(r)
            except Exception:
                import sys
                print(
                    "[@savants/guard] Failed to fetch managed rules, using local rules only",
                    file=sys.stderr,
                )

            client.start_polling(
                lambda new_rules: _update_rules(guard, len(parsed_rules), new_rules)
            )
            client.start_flushing()
            return guard

        # Return coroutine for await
        return _init_managed()  # type: ignore

    return guard


def _update_rules(guard: Guard, local_count: int, new_rules: list[ParsedRule]) -> None:
    """Replace managed rules while keeping local rules."""
    guard._rules = guard._rules[:local_count] + list(new_rules)


# ============================================================
# PRESETS
# ============================================================

def production_safety() -> Guard:
    """Block destructive actions (delete, terminate, drop, remove) in production."""
    return create_guard([
        "when action contains 'delete' and environment eq 'production' then block",
        "when action contains 'terminate' and environment eq 'production' then block",
        "when action contains 'drop' and environment eq 'production' then block",
        "when action contains 'remove' and environment eq 'production' then block",
    ])


def spend_limit(max_amount: int | float = 100) -> Guard:
    """Block spending over a threshold."""
    return create_guard([
        f"when amount gt {max_amount} then require_approval",
        f"when spend gt {max_amount} then require_approval",
        f"when cost gt {max_amount} then require_approval",
    ])


def business_hours() -> Guard:
    """Block actions on weekends."""
    return create_guard([
        "when day_of_week eq 'Saturday' then block",
        "when day_of_week eq 'Sunday' then block",
    ])


def deploy_safety() -> Guard:
    """Block risky deployments."""
    return create_guard([
        "when action contains 'deploy' and risk_score gt 0.7 and day_of_week eq 'Friday' then block",
        "when action contains 'deploy' and test_pass_rate lt 100 then block",
        "when skip_final_snapshot is_true then block",
    ])


def risk_aware_safety(risk_context: RiskContext | None = None) -> Guard:
    """
    Guard preset that uses code-graph risk variables.

    If no risk_context is provided, a default one is created with
    built-in file_risk and change_size sources. Register additional
    sources (blast_radius, caller_count, test_coverage, risk_score)
    on the returned guard's risk context or pass your own.

    Usage:
        rc = RiskContext()
        rc.register_risk_source("blast_radius", lambda ctx: graph.blast_radius(ctx["file"]))
        guard = risk_aware_safety(rc)
    """
    rc = risk_context or RiskContext()
    return create_guard(
        [
            "when blast_radius gt 30 then require_approval",
            "when file_risk eq 'high' and action contains 'edit' then ask 'This file is high-risk'",
            "when caller_count gt 20 then suggest 'This function has many callers -- test carefully'",
            "when change_size gt 500 then ask 'Large change -- review before proceeding'",
        ],
        risk_context=rc,
    )

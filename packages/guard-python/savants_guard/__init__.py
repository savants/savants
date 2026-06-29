"""
savants-guard — Deterministic guardrails for AI agents.

Your agent follows your rules. Always.

Usage:
    from savants_guard import create_guard, GuardError

    guard = create_guard([
        "when action contains 'delete' and env eq 'production' then block",
        "when spend gt 100 then require_approval",
        "when command contains 'git push --force' then rewrite 'git push --force-with-lease'",
        "when command contains 'chmod 777' then suggest 'Use chmod 755 instead'",
        "when command contains 'npm publish' then ask 'Publishing is permanent'",
    ])

    result = guard.check({"action": "delete_db", "env": "production"})
    # result.blocked    == True
    # result.allowed    == False
    # result.action     == "block"
    # result.rule       == "when action contains 'delete'..."
    # result.suggestion == None (set for suggest/rewrite/ask actions)

Actions: block (hard stop), suggest (alternative), rewrite (silent swap), ask (user approval)
Rules evaluate in order — first match wins.

Framework integrations:
    from savants_guard.integrations import langchain_callback, crewai_hook, openai_tool_guardrail
"""

from .types import GuardResult, GuardError, ParsedRule
from .guard import (
    create_guard,
    Guard,
    production_safety,
    spend_limit,
    business_hours,
    deploy_safety,
    risk_aware_safety,
)
from .risk import RiskContext, file_risk, change_size, codebase_suggestion
from .parser import parse_rule
from .ast_eval import evaluate, var_, lit, compare, and_, or_, not_
from .conventions import detect_conventions
from .duplicates import find_similar

__all__ = [
    "create_guard",
    "Guard",
    "GuardResult",
    "GuardError",
    "ParsedRule",
    "parse_rule",
    "evaluate",
    "production_safety",
    "spend_limit",
    "business_hours",
    "deploy_safety",
    "risk_aware_safety",
    "RiskContext",
    "file_risk",
    "change_size",
    "codebase_suggestion",
    "detect_conventions",
    "find_similar",
    "var_",
    "lit",
    "compare",
    "and_",
    "or_",
    "not_",
]

__version__ = "0.4.0"

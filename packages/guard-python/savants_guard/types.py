"""
Core types for savants-guard.

GuardResult, GuardError, ASTNode, ParsedRule.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


# ============================================================
# AST Node Types
# ============================================================

ASTNode = dict[str, Any]
"""
AST nodes are plain dicts with a 'type' discriminator:
  {"type": "literal", "value": ...}
  {"type": "var", "name": "field.nested"}
  {"type": "compare", "op": "eq", "left": ASTNode, "right": ASTNode}
  {"type": "and", "children": [ASTNode, ...]}
  {"type": "or", "children": [ASTNode, ...]}
  {"type": "not", "child": ASTNode}
"""


# ============================================================
# Parsed Rule
# ============================================================

@dataclass(frozen=True)
class ParsedRule:
    """A parsed DSL rule ready for evaluation."""
    dsl: str
    condition: ASTNode
    action: str
    suggestion: str = ""


# ============================================================
# Guard Result
# ============================================================

@dataclass(frozen=True)
class GuardResult:
    """Result of evaluating a guard check.

    Actions:
        block    — hard stop, action must not proceed
        suggest  — action denied, suggestion provided for recovery
        rewrite  — action should be replaced with the suggestion
        ask      — action needs user approval, suggestion has the reason
        None     — allowed, no rule matched
    """
    blocked: bool
    allowed: bool
    rule: str | None
    action: str | None
    context: dict[str, Any]
    suggestion: str | None = None


# ============================================================
# Guard Error
# ============================================================

class GuardError(Exception):
    """Raised when guard.wrap() blocks an action."""

    def __init__(self, rule: str, guard_action: str, context: dict[str, Any]):
        self.rule = rule
        self.guard_action = guard_action
        self.context = context
        super().__init__(f"Guard blocked: {rule} → {guard_action}")

"""
DSL Parser — human-readable guard rules → AST.

Format: "when <field> <op> <value> [and|or ...] then <action>"
"""

from __future__ import annotations

import re
from typing import Any

from .types import ASTNode, ParsedRule


OPERATORS = [
    "not_contains", "starts_with", "ends_with", "not_in",
    "is_true", "is_false", "is_empty", "is_not_empty",
    "contains", "matches", "eq", "neq", "gte", "gt", "lte", "lt", "in",
]


def parse_rule(text: str) -> ParsedRule | None:
    """
    Parse a human-readable guard rule into a ParsedRule.

    Examples:
        "when action contains 'delete' then block"
        "when spend gt 100 then require_approval"
        "when env eq 'production' and action contains 'delete' then block"

    Returns None if the text is not a valid rule.
    """
    text = text.strip()
    if not text or text.startswith("//") or text.startswith("#"):
        return None

    # Match: when <conditions> then <action> [optional 'message']
    match = re.match(
        r"^(?:when|if)\s+(.+?)\s+then\s+(\S+)(?:\s+'([^']*)')?(?:\s+\"([^\"]*)\")?",
        text,
        re.IGNORECASE,
    )
    if not match:
        return None

    cond_str = match.group(1).strip()
    action = match.group(2).strip()
    suggestion = match.group(3) or match.group(4) or ""

    # Split by and/or
    parts: list[str] = []
    combinators: list[str] = []
    remaining = cond_str

    while remaining:
        combo = re.match(r"^(.+?)\s+(and|or)\s+(.+)$", remaining, re.IGNORECASE)
        if combo:
            parts.append(combo.group(1).strip())
            combinators.append(combo.group(2).lower())
            remaining = combo.group(3).strip()
        else:
            parts.append(remaining.strip())
            remaining = ""

    # Parse each condition part
    conditions: list[ASTNode] = []
    for part in parts:
        negate = part.lower().startswith("not ")
        clean = part[4:].strip() if negate else part

        node: ASTNode | None = None
        for op in OPERATORS:
            idx = clean.find(f" {op}")
            if idx != -1:
                field = clean[:idx].strip()
                value_str = clean[idx + len(op) + 1:].strip()
                # Strip quotes
                stripped = re.sub(r"^['\"]|['\"]$", "", value_str)
                # Try numeric conversion
                try:
                    value: Any = float(stripped) if "." in stripped else int(stripped)
                    if value_str.startswith("'") or value_str.startswith('"'):
                        value = stripped  # Keep as string if quoted
                except ValueError:
                    value = stripped

                node = {
                    "type": "compare",
                    "op": op,
                    "left": {"type": "var", "name": field},
                    "right": {"type": "literal", "value": value},
                }
                break

        if node is None:
            # Unary operator or bare field
            node = {"type": "var", "name": clean}

        if negate:
            node = {"type": "not", "child": node}

        conditions.append(node)

    # Combine with and/or
    condition = conditions[0]
    for i, combinator in enumerate(combinators):
        if combinator == "and":
            condition = {"type": "and", "children": [condition, conditions[i + 1]]}
        else:
            condition = {"type": "or", "children": [condition, conditions[i + 1]]}

    return ParsedRule(dsl=text, condition=condition, action=action, suggestion=suggestion)

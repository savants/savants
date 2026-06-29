"""
AST Evaluator — safe expression evaluator for guard conditions.

Evaluates AST nodes against a context dictionary.
NO eval(), NO exec(), NO code injection.
Pure function, deterministic, zero dependencies.
"""

from __future__ import annotations

import re
from typing import Any

from .types import ASTNode


# ============================================================
# OPERATORS
# ============================================================

def _resolve_var(name: str, context: dict[str, Any]) -> Any:
    """Resolve a dotted variable name from context. e.g. 'contact.email'"""
    parts = name.split(".")
    val: Any = context
    for part in parts:
        if isinstance(val, dict):
            val = val.get(part)
        else:
            return None
    return val


def _op_eq(a: Any, b: Any) -> bool:
    return str(a) == str(b)

def _op_neq(a: Any, b: Any) -> bool:
    return str(a) != str(b)

def _op_gt(a: Any, b: Any) -> bool:
    try: return float(a) > float(b)
    except (TypeError, ValueError): return False

def _op_gte(a: Any, b: Any) -> bool:
    try: return float(a) >= float(b)
    except (TypeError, ValueError): return False

def _op_lt(a: Any, b: Any) -> bool:
    try: return float(a) < float(b)
    except (TypeError, ValueError): return False

def _op_lte(a: Any, b: Any) -> bool:
    try: return float(a) <= float(b)
    except (TypeError, ValueError): return False

def _op_contains(a: Any, b: Any) -> bool:
    return str(b).lower() in str(a).lower()

def _op_not_contains(a: Any, b: Any) -> bool:
    return str(b).lower() not in str(a).lower()

def _op_starts_with(a: Any, b: Any) -> bool:
    return str(a).startswith(str(b))

def _op_ends_with(a: Any, b: Any) -> bool:
    return str(a).endswith(str(b))

def _op_matches(a: Any, b: Any) -> bool:
    try: return bool(re.search(str(b), str(a), re.IGNORECASE))
    except re.error: return False

def _op_in(a: Any, b: Any) -> bool:
    if isinstance(b, (list, tuple, set)):
        return a in b
    return str(a) in str(b)

def _op_not_in(a: Any, b: Any) -> bool:
    if isinstance(b, (list, tuple, set)):
        return a not in b
    return str(a) not in str(b)

def _op_is_true(a: Any, _: Any) -> bool:
    return a is True or a == "true"

def _op_is_false(a: Any, _: Any) -> bool:
    return a is False or a == "false" or not a

def _op_is_empty(a: Any, _: Any) -> bool:
    return a is None or a == ""

def _op_is_not_empty(a: Any, _: Any) -> bool:
    return a is not None and a != ""


OPERATORS: dict[str, Any] = {
    "eq": _op_eq, "neq": _op_neq,
    "gt": _op_gt, "gte": _op_gte, "lt": _op_lt, "lte": _op_lte,
    "contains": _op_contains, "not_contains": _op_not_contains,
    "starts_with": _op_starts_with, "ends_with": _op_ends_with,
    "matches": _op_matches,
    "in": _op_in, "not_in": _op_not_in,
    "is_true": _op_is_true, "is_false": _op_is_false,
    "is_empty": _op_is_empty, "is_not_empty": _op_is_not_empty,
}


# ============================================================
# EVALUATOR
# ============================================================

def evaluate(node: ASTNode | None, context: dict[str, Any]) -> Any:
    """
    Recursively evaluate an AST node against a context dictionary.
    Returns boolean for conditions, raw value for literals/vars.
    """
    if node is None:
        return None

    node_type = node.get("type")

    if node_type == "literal":
        return node.get("value")

    if node_type == "var":
        return _resolve_var(node["name"], context)

    if node_type == "compare":
        op_fn = OPERATORS.get(node["op"])
        if op_fn is None:
            return False
        left = evaluate(node["left"], context)
        right = evaluate(node["right"], context)
        return op_fn(left, right)

    if node_type == "and":
        return all(evaluate(child, context) for child in node["children"])

    if node_type == "or":
        return any(evaluate(child, context) for child in node["children"])

    if node_type == "not":
        return not evaluate(node["child"], context)

    if node_type == "if":
        cond = evaluate(node["condition"], context)
        if cond:
            return evaluate(node["then"], context)
        elif "else" in node:
            return evaluate(node["else"], context)
        return None

    return None


# ============================================================
# AST BUILDER HELPERS
# ============================================================

def var_(name: str) -> ASTNode:
    return {"type": "var", "name": name}

def lit(value: Any) -> ASTNode:
    return {"type": "literal", "value": value}

def compare(field: str, op: str, value: Any) -> ASTNode:
    return {"type": "compare", "op": op, "left": var_(field), "right": lit(value)}

def and_(*conditions: ASTNode) -> ASTNode:
    return {"type": "and", "children": list(conditions)}

def or_(*conditions: ASTNode) -> ASTNode:
    return {"type": "or", "children": list(conditions)}

def not_(condition: ASTNode) -> ASTNode:
    return {"type": "not", "child": condition}

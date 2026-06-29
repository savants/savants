/**
 * AST Engine — safe expression evaluator for FSM guard conditions.
 *
 * Evaluates JSON AST nodes against a context dictionary.
 * NO eval(), NO Function constructor, NO code injection.
 * All evaluation is done by walking the AST tree.
 *
 * Ported from Python ast_engine.py — same semantics, same operators.
 */
const OPERATORS = {
    eq: (a, b) => a === b,
    neq: (a, b) => a !== b,
    gt: (a, b) => {
        if (a == null || b == null)
            return false;
        return Number(a) > Number(b);
    },
    gte: (a, b) => {
        if (a == null || b == null)
            return false;
        return Number(a) >= Number(b);
    },
    lt: (a, b) => {
        if (a == null || b == null)
            return false;
        return Number(a) < Number(b);
    },
    lte: (a, b) => {
        if (a == null || b == null)
            return false;
        return Number(a) <= Number(b);
    },
    contains: (a, b) => {
        if (!a || !b)
            return false;
        return String(a).toLowerCase().includes(String(b).toLowerCase());
    },
    not_contains: (a, b) => {
        if (!a)
            return true;
        if (!b)
            return true;
        return !String(a).toLowerCase().includes(String(b).toLowerCase());
    },
    starts_with: (a, b) => {
        if (!a || !b)
            return false;
        return String(a).toLowerCase().startsWith(String(b).toLowerCase());
    },
    ends_with: (a, b) => {
        if (!a || !b)
            return false;
        return String(a).toLowerCase().endsWith(String(b).toLowerCase());
    },
    in: (a, b) => {
        if (Array.isArray(b))
            return b.includes(a);
        return String(b).includes(String(a));
    },
    not_in: (a, b) => {
        if (Array.isArray(b))
            return !b.includes(a);
        return !String(b).includes(String(a));
    },
    matches: (a, b) => {
        if (!a || !b)
            return false;
        try {
            return new RegExp(String(b), "i").test(String(a));
        }
        catch {
            return false;
        }
    },
    is_empty: (a) => !a || (typeof a === "string" && !a.trim()),
    is_not_empty: (a) => !!a && (typeof a !== "string" || !!a.trim()),
    is_true: (a) => !!a,
    is_false: (a) => !a,
};
// ============================================================
// AST EVALUATOR
// ============================================================
/**
 * Evaluate an AST node against a context dictionary.
 * Returns the result of the evaluation (boolean for conditions,
 * any value for literals/variables, action dicts for actions).
 */
export function evaluate(node, context) {
    if (!node || typeof node !== "object" || !("type" in node)) {
        return null;
    }
    const n = node;
    switch (n.type) {
        // --- Leaf nodes ---
        case "literal":
            return n.value;
        case "var": {
            // Support nested access: "contact.name" → context.contact.name
            const parts = n.name.split(".");
            let val = context;
            for (const part of parts) {
                if (val && typeof val === "object" && part in val) {
                    val = val[part];
                }
                else {
                    return null;
                }
            }
            return val;
        }
        // --- Comparison ---
        case "compare": {
            const opName = n.op || "eq";
            const left = evaluate(n.left, context);
            const right = evaluate(n.right, context);
            const opFn = OPERATORS[opName];
            if (!opFn)
                return false;
            try {
                return opFn(left, right);
            }
            catch {
                return false;
            }
        }
        // --- Logical operators ---
        case "and": {
            const children = n.children || [];
            if (children.length === 0)
                return true;
            return children.every((child) => evaluate(child, context));
        }
        case "or": {
            const children = n.children || [];
            if (children.length === 0)
                return false;
            return children.some((child) => evaluate(child, context));
        }
        case "not":
            return !evaluate(n.child, context);
        // --- Control flow ---
        case "if": {
            const condition = evaluate(n.condition, context);
            if (condition) {
                return evaluate(n.then, context);
            }
            return n.else ? evaluate(n.else, context) : null;
        }
        case "sequence": {
            let result = null;
            for (const step of n.steps || []) {
                result = evaluate(step, context);
            }
            return result;
        }
        // --- Actions ---
        case "action":
            return { action: n.name, params: n.params || {} };
        // --- Rules ---
        case "rule": {
            const condition = evaluate(n.when, context);
            if (condition) {
                return evaluate(n.then, context);
            }
            return null;
        }
        case "ruleset": {
            for (const rule of n.rules || []) {
                const result = evaluate(rule, context);
                if (result != null)
                    return result;
            }
            return null;
        }
        default:
            return null;
    }
}
// ============================================================
// AST BUILDER HELPERS
// ============================================================
export const var_ = (name) => ({ type: "var", name });
export const lit = (value) => ({ type: "literal", value });
export const compare = (field, op, value) => ({
    type: "compare",
    op,
    left: var_(field),
    right: lit(value),
});
export const and_ = (...conditions) => ({
    type: "and",
    children: conditions,
});
export const or_ = (...conditions) => ({
    type: "or",
    children: conditions,
});
export const not_ = (condition) => ({
    type: "not",
    child: condition,
});
export const action = (name, params = {}) => ({
    type: "action",
    name,
    params,
});

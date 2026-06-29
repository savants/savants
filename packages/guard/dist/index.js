/**
 * @savants/guard — Deterministic guardrails for AI agents.
 *
 * Your agent follows your rules. Always.
 *
 * Usage:
 *   import { createGuard } from '@savants/guard';
 *
 *   const guard = createGuard([
 *     "when action contains 'delete' and env eq 'production' then block",
 *     "when spend gt 100 then require_approval",
 *   ]);
 *
 *   // Wrap any tool call
 *   const result = guard.check({ action: 'delete_db', env: 'production' });
 *   if (result.blocked) {
 *     console.log('Blocked by:', result.rule);
 *   }
 *
 *   // Or wrap an async function
 *   const safeTool = guard.wrap(dangerousTool);
 *   await safeTool({ action: 'delete_db', env: 'production' }); // throws GuardError
 *
 *   // Works with Vercel AI SDK
 *   const tools = guard.wrapTools({
 *     deleteThing: tool({ ... }),
 *   });
 */
// Re-export the engine for advanced usage
export { FSMSchema, GuardRegistry, transition, transitionSimple, checkTimeouts, loadSchema } from './engine.js';
export { evaluate, compare, and_, or_, not_, var_, lit, action } from './ast.js';
import { evaluate } from './ast.js';
// ============================================================
// DSL PARSER — human-readable rules → AST
// ============================================================
/**
 * Parse a human-readable guard rule into an AST node.
 *
 * Format: "when <field> <op> <value> then <action>"
 * Supports: and, or (single level)
 *
 * Examples:
 *   "when action contains 'delete' then block"
 *   "when spend gt 100 then require_approval"
 *   "when env eq 'production' and action contains 'delete' then block"
 */
export function parseRule(text) {
    text = text.trim();
    const match = text.match(/^(?:when|if)\s+(.+?)\s+then\s+(\S+)/i);
    if (!match)
        return null;
    const conditionStr = match[1].trim();
    const actionName = match[2].trim();
    const condition = parseCondition(conditionStr);
    return { condition, action: actionName };
}
function parseCondition(text) {
    // Handle AND
    if (/\s+and\s+/i.test(text)) {
        const parts = text.split(/\s+and\s+/i);
        return { type: "and", children: parts.map(p => parseSingleCondition(p.trim())) };
    }
    // Handle OR
    if (/\s+or\s+/i.test(text)) {
        const parts = text.split(/\s+or\s+/i);
        return { type: "or", children: parts.map(p => parseSingleCondition(p.trim())) };
    }
    return parseSingleCondition(text);
}
function parseSingleCondition(text) {
    // Unary operators
    const unaryOps = ["is_not_empty", "is_empty", "is_false", "is_true"];
    for (const op of unaryOps) {
        const m = text.match(new RegExp(`^(\\S+)\\s+${op}\\s*$`, "i"));
        if (m) {
            return { type: "compare", op, left: { type: "var", name: m[1] }, right: { type: "literal", value: null } };
        }
    }
    // Binary operators
    const binaryOps = ["not_contains", "starts_with", "ends_with", "contains", "matches",
        "not_in", "neq", "gte", "lte", "gt", "lt", "eq", "in"];
    for (const op of binaryOps) {
        const m = text.match(new RegExp(`^(\\S+)\\s+${op}\\s+(.+)$`, "i"));
        if (m) {
            let value = m[2].trim();
            // Strip quotes
            if (typeof value === 'string' && ((value.startsWith("'") && value.endsWith("'")) || (value.startsWith('"') && value.endsWith('"')))) {
                value = value.slice(1, -1);
            }
            // Try number
            if (typeof value === 'string' && !isNaN(Number(value))) {
                value = Number(value);
            }
            return { type: "compare", op, left: { type: "var", name: m[1] }, right: { type: "literal", value } };
        }
    }
    // Fallback: treat as truthy check
    return { type: "compare", op: "is_true", left: { type: "var", name: text }, right: { type: "literal", value: null } };
}
export class GuardError extends Error {
    rule;
    guardAction;
    context;
    constructor(rule, guardAction, context) {
        super(`Guard blocked: ${rule} → ${guardAction}`);
        this.name = "GuardError";
        this.rule = rule;
        this.guardAction = guardAction;
        this.context = context;
    }
}
/**
 * Create a guard from human-readable rules.
 *
 * @example
 * const guard = createGuard([
 *   "when action contains 'delete' and env eq 'production' then block",
 *   "when spend gt 100 then require_approval",
 * ]);
 */
export function createGuard(rules) {
    const parsedRules = [];
    const log = [];
    for (const rule of rules) {
        const parsed = parseRule(rule);
        if (parsed) {
            parsedRules.push({ dsl: rule, ...parsed });
        }
    }
    function check(context) {
        for (const rule of parsedRules) {
            const result = evaluate(rule.condition, context);
            if (result) {
                const guardResult = {
                    allowed: false,
                    blocked: true,
                    rule: rule.dsl,
                    action: rule.action,
                    context,
                };
                log.push({ timestamp: new Date().toISOString(), context, result: guardResult });
                return guardResult;
            }
        }
        const guardResult = {
            allowed: true,
            blocked: false,
            rule: null,
            action: null,
            context,
        };
        log.push({ timestamp: new Date().toISOString(), context, result: guardResult });
        return guardResult;
    }
    function wrap(fn) {
        return ((...args) => {
            const context = typeof args[0] === 'object' && args[0] !== null ? args[0] : {};
            const result = check(context);
            if (result.blocked) {
                throw new GuardError(result.rule, result.action, context);
            }
            return fn(...args);
        });
    }
    function wrapTool(tool) {
        const originalExecute = tool.execute;
        return {
            ...tool,
            execute: (...args) => {
                const context = typeof args[0] === 'object' && args[0] !== null ? args[0] : {};
                const result = check(context);
                if (result.blocked) {
                    throw new GuardError(result.rule, result.action, context);
                }
                return originalExecute(...args);
            },
        };
    }
    function wrapTools(tools) {
        const wrapped = {};
        for (const [name, tool] of Object.entries(tools)) {
            wrapped[name] = wrapTool(tool);
        }
        return wrapped;
    }
    return {
        check,
        wrap,
        wrapTool,
        wrapTools,
        addRule(dsl) {
            const parsed = parseRule(dsl);
            if (parsed)
                parsedRules.push({ dsl, ...parsed });
        },
        listRules() {
            return parsedRules.map(r => r.dsl);
        },
        getLog() {
            return [...log];
        },
    };
}
// ============================================================
// PRESETS — one-liner guardrails for common scenarios
// ============================================================
/** Block destructive actions (delete, terminate, drop, remove) in production. */
export const productionSafety = () => createGuard([
    "when action contains 'delete' and environment eq 'production' then block",
    "when action contains 'terminate' and environment eq 'production' then block",
    "when action contains 'drop' and environment eq 'production' then block",
    "when action contains 'remove' and environment eq 'production' then block",
]);
/** Block spending over a threshold without approval. */
export const spendLimit = (maxAmount = 100) => createGuard([
    `when amount gt ${maxAmount} then require_approval`,
    `when spend gt ${maxAmount} then require_approval`,
    `when cost gt ${maxAmount} then require_approval`,
]);
/** Block actions outside business hours and on weekends. */
export const businessHours = () => createGuard([
    "when day_of_week eq 'Saturday' then block",
    "when day_of_week eq 'Sunday' then block",
]);
/** Block high-risk deployments. */
export const deploySafety = () => createGuard([
    "when action contains 'deploy' and risk_score gt 0.7 and day_of_week eq 'Friday' then block",
    "when action contains 'deploy' and test_pass_rate lt 100 then block",
    "when skip_final_snapshot is_true then block",
]);

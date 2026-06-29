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
export { FSMSchema, GuardRegistry, transition, transitionSimple, checkTimeouts, loadSchema } from './engine.js';
export { evaluate, compare, and_, or_, not_, var_, lit, action } from './ast.js';
import type { ASTNode } from './types.js';
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
export declare function parseRule(text: string): {
    condition: ASTNode;
    action: string;
} | null;
export interface GuardResult {
    allowed: boolean;
    blocked: boolean;
    rule: string | null;
    action: string | null;
    context: Record<string, unknown>;
}
export declare class GuardError extends Error {
    rule: string;
    guardAction: string;
    context: Record<string, unknown>;
    constructor(rule: string, guardAction: string, context: Record<string, unknown>);
}
export interface Guard {
    /** Check if an action is allowed. Returns result without throwing. */
    check(context: Record<string, unknown>): GuardResult;
    /** Wrap a function — throws GuardError if any rule blocks. */
    wrap<T extends (...args: any[]) => any>(fn: T): T;
    /** Wrap a Vercel AI SDK tool — adds guard before execute(). */
    wrapTool<T extends {
        execute: (...args: any[]) => any;
    }>(tool: T): T;
    /** Wrap multiple Vercel AI SDK tools. */
    wrapTools<T extends Record<string, {
        execute: (...args: any[]) => any;
    }>>(tools: T): T;
    /** Add a rule at runtime. */
    addRule(dsl: string): void;
    /** List all rules. */
    listRules(): string[];
    /** Get evaluation log. */
    getLog(): Array<{
        timestamp: string;
        context: Record<string, unknown>;
        result: GuardResult;
    }>;
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
export declare function createGuard(rules: string[]): Guard;
/** Block destructive actions (delete, terminate, drop, remove) in production. */
export declare const productionSafety: () => Guard;
/** Block spending over a threshold without approval. */
export declare const spendLimit: (maxAmount?: number) => Guard;
/** Block actions outside business hours and on weekends. */
export declare const businessHours: () => Guard;
/** Block high-risk deployments. */
export declare const deploySafety: () => Guard;

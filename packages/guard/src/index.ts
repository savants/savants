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

import { FSMSchema, GuardRegistry, transition, loadSchema } from './engine.js';
import { evaluate, compare, and_, or_, not_ } from './ast.js';
import type { ASTNode, FSMSchemaDefinition } from './types.js';
import { ManagedGuardClient, type ParsedManagedRule } from './managed.js';

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
export function parseRule(text: string): { condition: ASTNode; action: string } | null {
  text = text.trim();
  const match = text.match(/^(?:when|if)\s+(.+?)\s+then\s+(\S+)/i);
  if (!match) return null;

  const conditionStr = match[1].trim();
  const actionName = match[2].trim();

  const condition = parseCondition(conditionStr);
  return { condition, action: actionName };
}

function parseCondition(text: string): ASTNode {
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

function parseSingleCondition(text: string): ASTNode {
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
      let value: unknown = m[2].trim();
      // Strip quotes
      if (typeof value === 'string' && ((value.startsWith("'") && value.endsWith("'")) || (value.startsWith('"') && value.endsWith('"')))) {
        value = (value as string).slice(1, -1);
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

// ============================================================
// GUARD — the main API
// ============================================================

export interface GuardResult {
  allowed: boolean;
  blocked: boolean;
  rule: string | null;
  action: string | null;
  context: Record<string, unknown>;
}

export interface GuardOptions {
  /** Fetch and sync rules from Savants cloud. */
  managed?: boolean;
  /** API key for managed mode (sk_live_... or svt_agent_...). */
  apiKey?: string;
  /** API base URL. Default: "https://api.savants.cloud". */
  apiUrl?: string;
  /** Poll interval for rule changes in ms. Default: 30000. */
  pollInterval?: number;
}

export class GuardError extends Error {
  rule: string;
  guardAction: string;
  context: Record<string, unknown>;

  constructor(rule: string, guardAction: string, context: Record<string, unknown>) {
    super(`Guard blocked: ${rule} → ${guardAction}`);
    this.name = "GuardError";
    this.rule = rule;
    this.guardAction = guardAction;
    this.context = context;
  }
}

export interface Guard {
  /** Check if an action is allowed. Always synchronous (local evaluation). */
  check(context: Record<string, unknown>): GuardResult;

  /** Wrap a function — throws GuardError if any rule blocks. */
  wrap<T extends (...args: any[]) => any>(fn: T): T;

  /** Wrap a Vercel AI SDK tool — adds guard before execute(). */
  wrapTool<T extends { execute: (...args: any[]) => any }>(tool: T): T;

  /** Wrap multiple Vercel AI SDK tools. */
  wrapTools<T extends Record<string, { execute: (...args: any[]) => any }>>(tools: T): T;

  /** Add a rule at runtime. */
  addRule(dsl: string): void;

  /** List all rules. */
  listRules(): string[];

  /** Get evaluation log. */
  getLog(): Array<{ timestamp: string; context: Record<string, unknown>; result: GuardResult }>;

  /** Whether managed mode is active (rules synced from cloud). */
  readonly managed: boolean;

  /** Stop polling + flush events. Call when shutting down. */
  close(): Promise<void>;
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
export function createGuard(rules: string[], options?: GuardOptions): Guard | Promise<Guard> {
  const parsedRules: Array<{ dsl: string; condition: ASTNode; action: string }> = [];
  const log: Array<{ timestamp: string; context: Record<string, unknown>; result: GuardResult }> = [];
  let managedClient: ManagedGuardClient | null = null;

  for (const rule of rules) {
    const parsed = parseRule(rule);
    if (parsed) {
      parsedRules.push({ dsl: rule, ...parsed });
    }
  }

  const BLOCKING = new Set(["block", "block_deploy", "deny", "require_approval"]);

  function check(context: Record<string, unknown>): GuardResult {
    for (const rule of parsedRules) {
      const result = evaluate(rule.condition, context);
      if (result && BLOCKING.has(rule.action)) {
        const guardResult: GuardResult = {
          allowed: false,
          blocked: true,
          rule: rule.dsl,
          action: rule.action,
          context,
        };
        log.push({ timestamp: new Date().toISOString(), context, result: guardResult });
        // Report to cloud if managed
        managedClient?.reportEvent({
          context_hash: JSON.stringify(context).length.toString(),
          action: String(context.action || ""),
          tool: String(context.tool || ""),
          result: "blocked",
          matched_rule: rule.dsl,
          timestamp: new Date().toISOString(),
        });
        return guardResult;
      }
    }
    const guardResult: GuardResult = {
      allowed: true,
      blocked: false,
      rule: null,
      action: null,
      context,
    };
    log.push({ timestamp: new Date().toISOString(), context, result: guardResult });
    managedClient?.reportEvent({
      context_hash: JSON.stringify(context).length.toString(),
      action: String(context.action || ""),
      tool: String(context.tool || ""),
      result: "allowed",
      timestamp: new Date().toISOString(),
    });
    return guardResult;
  }

  function wrap<T extends (...args: any[]) => any>(fn: T): T {
    return ((...args: any[]) => {
      const context = typeof args[0] === 'object' && args[0] !== null ? args[0] : {};
      const result = check(context);
      if (result.blocked) {
        throw new GuardError(result.rule!, result.action!, context);
      }
      return fn(...args);
    }) as T;
  }

  function wrapTool<T extends { execute: (...args: any[]) => any }>(tool: T): T {
    const originalExecute = tool.execute;
    return {
      ...tool,
      execute: (...args: any[]) => {
        const context = typeof args[0] === 'object' && args[0] !== null ? args[0] : {};
        const result = check(context);
        if (result.blocked) {
          throw new GuardError(result.rule!, result.action!, context);
        }
        return originalExecute(...args);
      },
    } as T;
  }

  function wrapTools<T extends Record<string, { execute: (...args: any[]) => any }>>(tools: T): T {
    const wrapped: Record<string, any> = {};
    for (const [name, tool] of Object.entries(tools)) {
      wrapped[name] = wrapTool(tool);
    }
    return wrapped as T;
  }

  function buildGuard(isManaged: boolean): Guard {
    return {
      check,
      wrap,
      wrapTool,
      wrapTools,
      managed: isManaged,
      addRule(dsl: string) {
        const parsed = parseRule(dsl);
        if (parsed) parsedRules.push({ dsl, ...parsed });
      },
      listRules() {
        return parsedRules.map(r => r.dsl);
      },
      getLog() {
        return [...log];
      },
      async close() {
        if (managedClient) await managedClient.close();
      },
    };
  }

  // Managed mode: fetch rules from cloud, poll for updates, batch events
  if (options?.managed && options?.apiKey) {
    managedClient = new ManagedGuardClient({
      apiKey: options.apiKey,
      apiUrl: options.apiUrl,
      pollInterval: options.pollInterval,
    });

    return (async () => {
      try {
        const managedRules = await managedClient!.fetchBundle();
        for (const r of managedRules) {
          parsedRules.push({ dsl: r.dsl, condition: r.condition, action: r.action });
        }
      } catch {
        // If fetch fails, continue with local rules only
        if (typeof console !== "undefined") {
          console.warn("[@savants/guard] Failed to fetch managed rules, using local rules only");
        }
      }

      // Start background polling for rule updates
      managedClient!.startPolling((newRules) => {
        // Replace managed rules (keep local rules at the front)
        const localCount = rules.length;
        parsedRules.splice(localCount);
        for (const r of newRules) {
          parsedRules.push({ dsl: r.dsl, condition: r.condition, action: r.action });
        }
      });

      // Start background event flushing
      managedClient!.startFlushing();

      return buildGuard(true);
    })();
  }

  // Local-only mode: synchronous, no network
  return buildGuard(false);
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
export const spendLimit = (maxAmount: number = 100) => createGuard([
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

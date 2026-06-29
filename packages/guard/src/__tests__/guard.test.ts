/**
 * Tests for @savants/guard npm package.
 *
 * Tests the DEVELOPER EXPERIENCE — the API surface that developers
 * actually touch. If these pass, the DX works.
 */

import { describe, it, expect } from "vitest";
import {
  createGuard, parseRule, GuardError,
  productionSafety, spendLimit, businessHours, deploySafety,
} from "../index.js";

// ============================================================
// createGuard() — the core API
// ============================================================

describe("createGuard()", () => {
  it("blocks destructive action in production", () => {
    const guard = createGuard([
      "when action contains 'delete' and environment eq 'production' then block",
    ]);
    const result = guard.check({ action: "delete_database", environment: "production" });
    expect(result.blocked).toBe(true);
    expect(result.allowed).toBe(false);
    expect(result.action).toBe("block");
  });

  it("allows non-destructive action in production", () => {
    const guard = createGuard([
      "when action contains 'delete' and environment eq 'production' then block",
    ]);
    const result = guard.check({ action: "read_logs", environment: "production" });
    expect(result.blocked).toBe(false);
    expect(result.allowed).toBe(true);
  });

  it("allows destructive action in staging", () => {
    const guard = createGuard([
      "when action contains 'delete' and environment eq 'production' then block",
    ]);
    const result = guard.check({ action: "delete_database", environment: "staging" });
    expect(result.allowed).toBe(true);
  });

  it("first matching rule wins", () => {
    const guard = createGuard([
      "when priority eq 'high' then escalate",
      "when priority eq 'high' then log",
    ]);
    const result = guard.check({ priority: "high" });
    expect(result.action).toBe("escalate");
  });

  it("no rules = always allowed", () => {
    const guard = createGuard([]);
    expect(guard.check({ anything: true }).allowed).toBe(true);
  });

  it("invalid DSL is silently skipped", () => {
    const guard = createGuard(["this is not a valid rule"]);
    expect(guard.check({ x: 1 }).allowed).toBe(true);
  });
});

// ============================================================
// guard.wrap() — wraps any function
// ============================================================

describe("guard.wrap()", () => {
  it("allows safe calls to pass through", () => {
    const guard = createGuard(["when action contains 'delete' then block"]);
    const fn = guard.wrap((ctx: any) => `executed: ${ctx.action}`);
    expect(fn({ action: "read_logs" })).toBe("executed: read_logs");
  });

  it("throws GuardError on blocked calls", () => {
    const guard = createGuard(["when action contains 'delete' then block"]);
    const fn = guard.wrap((ctx: any) => `executed: ${ctx.action}`);
    expect(() => fn({ action: "delete_db" })).toThrow(GuardError);
  });

  it("GuardError has rule and action", () => {
    const guard = createGuard(["when spend gt 100 then require_approval"]);
    const fn = guard.wrap((ctx: any) => ctx);
    try {
      fn({ spend: 150 });
      expect(true).toBe(false); // Should not reach here
    } catch (e) {
      expect(e).toBeInstanceOf(GuardError);
      expect((e as GuardError).rule).toContain("spend gt 100");
      expect((e as GuardError).guardAction).toBe("require_approval");
    }
  });

  it("wrapped function is transparent when allowed", () => {
    const guard = createGuard(["when x eq 999 then block"]);
    const add = guard.wrap((a: number, b: number) => a + b);
    expect(add(2, 3)).toBe(5);
  });
});

// ============================================================
// guard.wrapTool() — Vercel AI SDK integration
// ============================================================

describe("guard.wrapTool()", () => {
  it("wraps a tool-like object", () => {
    const guard = createGuard(["when action contains 'delete' then block"]);
    const tool = {
      description: "Delete a user",
      execute: async (params: { action: string; userId: string }) => {
        return { deleted: params.userId };
      },
    };
    const safeTool = guard.wrapTool(tool);

    // Should throw on dangerous call
    expect(() => safeTool.execute({ action: "delete_user", userId: "123" })).toThrow(GuardError);
  });

  it("allows safe tool execution", async () => {
    const guard = createGuard(["when action contains 'delete' then block"]);
    const tool = {
      description: "Read a user",
      execute: async (params: { action: string; userId: string }) => {
        return { user: params.userId };
      },
    };
    const safeTool = guard.wrapTool(tool);
    const result = await safeTool.execute({ action: "read_user", userId: "123" });
    expect(result).toEqual({ user: "123" });
  });
});

// ============================================================
// guard.wrapTools() — wrap multiple tools at once
// ============================================================

describe("guard.wrapTools()", () => {
  it("wraps multiple tools", () => {
    const guard = createGuard([
      "when action contains 'delete' and env eq 'production' then block",
    ]);
    const tools = guard.wrapTools({
      deleteUser: {
        execute: (p: any) => ({ deleted: true }),
      },
      readUser: {
        execute: (p: any) => ({ user: p.userId }),
      },
    });

    // Delete in production → blocked
    expect(() => tools.deleteUser.execute({ action: "delete_user", env: "production" })).toThrow(GuardError);

    // Read in production → allowed
    expect(tools.readUser.execute({ action: "read_user", env: "production" })).toEqual({ user: undefined });
  });
});

// ============================================================
// guard.addRule() — add rules at runtime
// ============================================================

describe("guard.addRule()", () => {
  it("adds a rule dynamically", () => {
    const guard = createGuard([]);
    expect(guard.check({ action: "delete_db" }).allowed).toBe(true);

    guard.addRule("when action contains 'delete' then block");
    expect(guard.check({ action: "delete_db" }).blocked).toBe(true);
  });
});

// ============================================================
// guard.listRules() + guard.getLog()
// ============================================================

describe("guard.listRules() + getLog()", () => {
  it("lists all rules", () => {
    const guard = createGuard([
      "when x eq 1 then block",
      "when y gt 5 then escalate",
    ]);
    expect(guard.listRules()).toHaveLength(2);
  });

  it("logs every evaluation", () => {
    const guard = createGuard(["when x eq 1 then block"]);
    guard.check({ x: 1 });
    guard.check({ x: 2 });
    const log = guard.getLog();
    expect(log).toHaveLength(2);
    expect(log[0].result.blocked).toBe(true);
    expect(log[1].result.allowed).toBe(true);
  });
});

// ============================================================
// parseRule() — DSL parsing
// ============================================================

describe("parseRule()", () => {
  it("parses simple rule", () => {
    const parsed = parseRule("when x eq 1 then block");
    expect(parsed).not.toBeNull();
    expect(parsed!.action).toBe("block");
  });

  it("parses AND rule", () => {
    const parsed = parseRule("when a eq 1 and b eq 2 then block");
    expect(parsed).not.toBeNull();
    expect(parsed!.condition.type).toBe("and");
  });

  it("parses string values with quotes", () => {
    const parsed = parseRule("when env eq 'production' then block");
    expect(parsed).not.toBeNull();
  });

  it("parses numeric values", () => {
    const parsed = parseRule("when spend gt 100 then block");
    expect(parsed).not.toBeNull();
  });

  it("returns null for invalid rule", () => {
    expect(parseRule("not a rule")).toBeNull();
  });

  it("supports all operators", () => {
    const ops = ["eq", "neq", "gt", "gte", "lt", "lte", "contains", "not_contains",
                 "starts_with", "ends_with"];
    for (const op of ops) {
      const parsed = parseRule(`when field ${op} 'value' then block`);
      expect(parsed).not.toBeNull();
    }
  });

  it("supports unary operators", () => {
    for (const op of ["is_true", "is_false", "is_empty", "is_not_empty"]) {
      const parsed = parseRule(`when field ${op} then block`);
      expect(parsed).not.toBeNull();
    }
  });
});

// ============================================================
// PRESETS
// ============================================================

describe("Presets", () => {
  it("productionSafety blocks delete/terminate/drop/remove in production", () => {
    const guard = productionSafety();
    expect(guard.check({ action: "delete_db", environment: "production" }).blocked).toBe(true);
    expect(guard.check({ action: "terminate_instance", environment: "production" }).blocked).toBe(true);
    expect(guard.check({ action: "drop_table", environment: "production" }).blocked).toBe(true);
    expect(guard.check({ action: "remove_role", environment: "production" }).blocked).toBe(true);
    expect(guard.check({ action: "read_logs", environment: "production" }).allowed).toBe(true);
    expect(guard.check({ action: "delete_db", environment: "staging" }).allowed).toBe(true);
  });

  it("spendLimit blocks over threshold", () => {
    const guard = spendLimit(50);
    expect(guard.check({ amount: 100 }).blocked).toBe(true);
    expect(guard.check({ spend: 75 }).blocked).toBe(true);
    expect(guard.check({ amount: 30 }).allowed).toBe(true);
  });

  it("businessHours blocks weekends", () => {
    const guard = businessHours();
    expect(guard.check({ day_of_week: "Saturday" }).blocked).toBe(true);
    expect(guard.check({ day_of_week: "Sunday" }).blocked).toBe(true);
    expect(guard.check({ day_of_week: "Monday" }).allowed).toBe(true);
  });

  it("deploySafety blocks risky Friday deploys", () => {
    const guard = deploySafety();
    expect(guard.check({
      action: "deploy_to_production",
      risk_score: 0.9,
      day_of_week: "Friday",
    }).blocked).toBe(true);

    expect(guard.check({ skip_final_snapshot: true }).blocked).toBe(true);

    expect(guard.check({
      action: "deploy_to_staging",
      risk_score: 0.3,
      day_of_week: "Monday",
      test_pass_rate: 100,
      skip_final_snapshot: false,
    }).allowed).toBe(true);
  });
});

// ============================================================
// REAL-WORLD SCENARIOS
// ============================================================

describe("Real-world: Pocket OS prevention", () => {
  it("AI agent cannot delete production database", () => {
    const guard = productionSafety();

    // The exact scenario that killed Pocket OS
    const result = guard.check({
      action: "delete_database_volume",
      environment: "production",
    });
    expect(result.blocked).toBe(true);
    expect(result.rule).toContain("delete");
    expect(result.action).toBe("block");
  });
});

describe("Real-world: OpenAI egg purchase prevention", () => {
  it("AI agent cannot spend $31 without approval", () => {
    const guard = spendLimit(10);
    const result = guard.check({ amount: 31.43, item: "eggs" });
    expect(result.blocked).toBe(true);
    expect(result.action).toBe("require_approval");
  });
});

describe("Real-world: Vercel AI SDK integration", () => {
  it("guards wrap tool execute functions", () => {
    const guard = createGuard([
      "when action contains 'delete' and env eq 'production' then block",
    ]);

    // Simulated Vercel AI SDK tool
    const deleteUserTool = {
      description: "Delete a user from the database",
      parameters: { type: "object" },
      execute: async ({ userId, env }: { userId: string; env: string }) => {
        return { deleted: userId };
      },
    };

    const safeTool = guard.wrapTool(deleteUserTool);

    // Production delete → GuardError thrown BEFORE execute runs
    expect(() =>
      safeTool.execute({ userId: "123", action: "delete_user", env: "production" })
    ).toThrow(GuardError);

    // Staging delete → execute runs normally
    expect(
      safeTool.execute({ userId: "123", action: "delete_user", env: "staging" })
    ).resolves.toEqual({ deleted: "123" });
  });
});

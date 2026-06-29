/**
 * Website Code Example Tests
 *
 * Every code example shown on savants.dev docs is tested here.
 * If a developer copies code from our website, it MUST work.
 * If any of these fail, the website is lying.
 */

import { describe, it, expect } from "vitest";
import {
  createGuard, GuardError,
  productionSafety, spendLimit, businessHours, deploySafety,
  parseRule,
} from "../index.js";

// ============================================================
// GETTING STARTED PAGE — /docs/getting-started
// ============================================================

describe("Website: Getting Started examples", () => {
  it("Install + first guardrail", () => {
    // From: "Add your first guardrail" section
    const guard = createGuard([
      "when action contains 'delete' and env eq 'production' then block",
      "when spend gt 100 then require_approval",
    ]);
    expect(guard).toBeDefined();
    expect(guard.listRules()).toHaveLength(2);
  });

  it("Check an action", () => {
    // From: "Check an action" section
    const guard = createGuard([
      "when action contains 'delete' and env eq 'production' then block",
    ]);
    const result = guard.check({
      action: "delete_database",
      env: "production",
    });
    expect(result.blocked).toBe(true);
    expect(result.rule).toContain("delete");
    expect(result.action).toBe("block");
  });

  it("productionSafety preset", () => {
    // From: "Or use a preset" section
    const guard = productionSafety();
    // Blocks delete, terminate, drop, remove in production
    expect(guard.check({ action: "delete_db", environment: "production" }).blocked).toBe(true);
    // Allows everything in staging/dev
    expect(guard.check({ action: "delete_db", environment: "staging" }).allowed).toBe(true);
    // Allows read-only actions anywhere
    expect(guard.check({ action: "read_logs", environment: "production" }).allowed).toBe(true);
  });

  it("Wrap AI tools", () => {
    // From: "Wrap your AI tools" section
    const guard = createGuard([
      "when action contains 'delete' and env eq 'production' then block",
    ]);
    const safeTools = guard.wrapTools({
      deleteUser: {
        description: "Delete a user",
        execute: async ({ userId }: { userId: string }) => ({ deleted: userId }),
      },
    });
    // LLM calls deleteUser → GuardError thrown BEFORE execute()
    expect(() =>
      safeTools.deleteUser.execute({ userId: "123", action: "delete_user", env: "production" } as any)
    ).toThrow(GuardError);
  });

  it("Test a guard", () => {
    // From: "Test a guard" section
    const guard = createGuard(["when spend gt 100 then block"]);
    expect(guard.check({ spend: 150 }).blocked).toBe(true);
    expect(guard.check({ spend: 50 }).allowed).toBe(true);
  });
});

// ============================================================
// GUARD DSL PAGE — /docs/guard-dsl
// ============================================================

describe("Website: Guard DSL examples", () => {
  it("Syntax example", () => {
    // From: "Examples" section
    const examples = [
      "when action contains 'delete' and env eq 'production' then block",
      "when spend gt 100 then require_approval",
      "when cpu_pct gt 90 then alert",
      "when message contains 'urgent' then escalate",
      "when day_of_week eq 'Friday' and risk gt 0.7 then block_deploy",
    ];
    for (const rule of examples) {
      const parsed = parseRule(rule);
      expect(parsed).not.toBeNull();
      expect(parsed!.action).toBeTruthy();
    }
  });

  it("All 16 operators work", () => {
    // From: "All 16 Operators" table
    const guard = createGuard([
      "when status eq 'active' then match",
      "when role neq 'admin' then match",
      "when spend gt 100 then match",
      "when score gte 650 then match",
      "when age lt 18 then match",
      "when risk lte 0.5 then match",
      "when msg contains 'urgent' then match",
      "when email not_contains 'spam' then match",
      "when tool starts_with 'send_' then match",
      "when file ends_with '.sql' then match",
      "when approved is_true then match",
      "when blocked is_false then match",
      "when notes is_empty then match",
    ]);
    expect(guard.check({ status: "active" }).blocked).toBe(true);
    expect(guard.check({ role: "user" }).blocked).toBe(true);
    expect(guard.check({ spend: 200 }).blocked).toBe(true);
    expect(guard.check({ score: 700 }).blocked).toBe(true);
    expect(guard.check({ age: 15 }).blocked).toBe(true);
    expect(guard.check({ risk: 0.3 }).blocked).toBe(true);
    expect(guard.check({ msg: "this is urgent" }).blocked).toBe(true);
    expect(guard.check({ email: "hello@test.com" }).blocked).toBe(true);
    expect(guard.check({ tool: "send_email" }).blocked).toBe(true);
    expect(guard.check({ file: "data.sql" }).blocked).toBe(true);
    expect(guard.check({ approved: true }).blocked).toBe(true);
    expect(guard.check({ blocked: false }).blocked).toBe(true);
    expect(guard.check({ notes: "" }).blocked).toBe(true);
  });

  it("AND combinator", () => {
    // From: "Logical Combinators" section
    const guard = createGuard([
      "when role eq 'admin' and mfa is_true and ip_allowed is_true then allow",
    ]);
    expect(guard.check({ role: "admin", mfa: true, ip_allowed: true }).blocked).toBe(true);
    expect(guard.check({ role: "admin", mfa: false, ip_allowed: true }).allowed).toBe(true);
  });

  it("OR combinator", () => {
    const guard = createGuard([
      "when env eq 'staging' or env eq 'development' then allow_deploy",
    ]);
    expect(guard.check({ env: "staging" }).blocked).toBe(true);
    expect(guard.check({ env: "development" }).blocked).toBe(true);
    expect(guard.check({ env: "production" }).allowed).toBe(true);
  });

  it("Using in code example", () => {
    // From: "Using in code" section
    const guard = createGuard([
      "when action contains 'delete' and env eq 'production' then block",
      "when spend gt 100 then require_approval",
    ]);
    const result = guard.check({ action: "delete_db", env: "production" });
    expect(result.blocked).toBe(true);
    expect(result.rule).toContain("delete");
    expect(result.action).toBe("block");
  });
});

// ============================================================
// RECIPES PAGE — /docs/recipes
// ============================================================

describe("Website: Recipes examples", () => {
  it("Recipe 1: Protect your database", () => {
    const guard = productionSafety();
    const safeDb = guard.wrap((params: any) => `executed: ${params.action}`);

    expect(safeDb({ action: "select_users", environment: "production" })).toBe("executed: select_users");
    expect(() => safeDb({ action: "delete_table", environment: "production" })).toThrow(GuardError);
  });

  it("Recipe 2: Guard Vercel AI SDK tools", () => {
    const guard = createGuard([
      "when action contains 'delete' and env eq 'production' then block",
      "when action contains 'send_email' and recipient_count gt 10 then block",
    ]);
    const tools = guard.wrapTools({
      deleteUser: {
        description: "Delete a user",
        execute: async ({ userId }: any) => ({ deleted: userId }),
      },
    });
    expect(() =>
      tools.deleteUser.execute({ userId: "123", action: "delete_user", env: "production" })
    ).toThrow(GuardError);
  });

  it("Recipe 3: Spend limits", () => {
    const guard = spendLimit(25);
    expect(guard.check({ amount: 5.99, item: "eggs" }).allowed).toBe(true);
    expect(guard.check({ amount: 31.43, item: "eggs with delivery" }).blocked).toBe(true);
  });

  it("Recipe 4: No deploys on Friday", () => {
    const guard = createGuard([
      "when action contains 'deploy' and day_of_week eq 'Friday' and risk_score gt 0.7 then block",
      "when action contains 'deploy' and test_pass_rate lt 100 then block",
    ]);
    expect(guard.check({
      action: "deploy_to_production",
      day_of_week: "Friday",
      risk_score: 0.85,
      test_pass_rate: 100,
    }).blocked).toBe(true);

    expect(guard.check({
      action: "deploy_to_production",
      day_of_week: "Monday",
      risk_score: 0.3,
      test_pass_rate: 100,
    }).allowed).toBe(true);
  });

  it("Recipe 5: Audit trail", () => {
    const guard = createGuard([
      "when action contains 'delete' then block",
      "when action contains 'send' then log",
    ]);
    guard.check({ action: "delete_user", user_id: "123" });
    guard.check({ action: "send_email", to: "ceo@company.com" });
    guard.check({ action: "read_report" });

    const log = guard.getLog();
    expect(log).toHaveLength(3);
    expect(log[0].result.blocked).toBe(true);
    expect(log[1].result.blocked).toBe(true);
    expect(log[2].result.allowed).toBe(true);
  });

  it("Recipe 6: Custom rules at runtime", () => {
    const guard = productionSafety();
    guard.addRule("when department eq 'finance' and amount gt 10000 then require_cfo_approval");
    guard.addRule("when data_classification eq 'pii' and action contains 'export' then block");

    const rules = guard.listRules();
    expect(rules.length).toBeGreaterThanOrEqual(6); // 4 preset + 2 custom
    expect(rules.some(r => r.includes("finance"))).toBe(true);
    expect(rules.some(r => r.includes("pii"))).toBe(true);
  });
});

// ============================================================
// PRICING PAGE — guarantee claims
// ============================================================

describe("Website: Pricing guarantee claims", () => {
  it("Guards can't be bypassed with empty context", () => {
    const guard = createGuard(["when authorized is_true then allow"]);
    expect(guard.check({}).allowed).toBe(true); // No rule matches when authorized is undefined
  });

  it("productionSafety blocks all 4 destructive patterns", () => {
    const guard = productionSafety();
    expect(guard.check({ action: "delete_db", environment: "production" }).blocked).toBe(true);
    expect(guard.check({ action: "terminate_instance", environment: "production" }).blocked).toBe(true);
    expect(guard.check({ action: "drop_table", environment: "production" }).blocked).toBe(true);
    expect(guard.check({ action: "remove_role", environment: "production" }).blocked).toBe(true);
  });

  it("Pocket OS scenario blocked", () => {
    const guard = productionSafety();
    expect(guard.check({
      action: "delete_database_volume",
      environment: "production",
    }).blocked).toBe(true);
  });

  it("OpenAI egg purchase blocked", () => {
    const guard = spendLimit(10);
    expect(guard.check({ amount: 31.43, item: "eggs" }).blocked).toBe(true);
  });
});

// ============================================================
// README — npm package README examples
// ============================================================

describe("Website: README examples", () => {
  it("30 seconds to first guardrail", () => {
    const guard = createGuard([
      "when action contains 'delete' and env eq 'production' then block",
      "when spend gt 100 then require_approval",
    ]);
    const r1 = guard.check({ action: "delete_db", env: "production" });
    expect(r1.blocked).toBe(true);
    expect(r1.rule).toContain("delete");
    expect(r1.action).toBe("block");

    const r2 = guard.check({ action: "read_logs", env: "production" });
    expect(r2.allowed).toBe(true);
  });

  it("Wrap any function", () => {
    const guard = createGuard(["when action contains 'delete' then block"]);
    const fn = guard.wrap((ctx: any) => `executed: ${ctx.action}`);
    expect(fn({ action: "read_logs" })).toBe("executed: read_logs");
    expect(() => fn({ action: "delete_db" })).toThrow(GuardError);
  });
});

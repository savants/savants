import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";
import { getMonthlyUsage, getMonthlyToolCallCount, getOrgById } from "../db/queries";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const usage = new Hono<HonoEnv>();

const FREE_MONTHLY_CALLS = 10;

// GET /api/v1/usage - Aggregate current month usage
usage.get("/", async (c) => {
  const auth = c.get("auth");

  const [org, breakdown, totalCalls] = await Promise.all([
    getOrgById(c.env.DB, auth.orgId),
    getMonthlyUsage(c.env.DB, auth.orgId),
    getMonthlyToolCallCount(c.env.DB, auth.orgId),
  ]);

  const isPaid = org?.plan === "cloud" || org?.plan === "enterprise";
  const now = new Date();
  const month = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}`;

  const totalTokensOut = breakdown.reduce((sum, r) => sum + (r.total_tokens_out ?? 0), 0);
  const totalDurationMs = breakdown.reduce((sum, r) => sum + (r.total_duration_ms ?? 0), 0);
  const estimatedTokensSaved = totalTokensOut * 11; // savants returns ~12x less noise than grep
  const estimatedTimeSavedMs = totalCalls * 8000; // ~8s saved per call vs manual grep+read

  return c.json({
    org_id: auth.orgId,
    plan: org?.plan ?? "free",
    month,
    total_calls: totalCalls,
    limit: isPaid ? null : FREE_MONTHLY_CALLS,
    remaining: isPaid ? null : Math.max(0, FREE_MONTHLY_CALLS - totalCalls),
    value: {
      tokens_saved: estimatedTokensSaved,
      time_saved_minutes: Math.round(estimatedTimeSavedMs / 60000),
      avg_response_ms: totalCalls > 0 ? Math.round(totalDurationMs / totalCalls) : 0,
    },
    breakdown: breakdown.map((row) => ({
      tool: row.tool_name,
      calls: row.call_count,
      tokens_in: row.total_tokens_in,
      tokens_out: row.total_tokens_out,
      duration_ms: row.total_duration_ms,
    })),
  });
});

export default usage;

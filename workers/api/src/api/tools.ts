import { Hono } from "hono";
import type { Env, AuthContext, ToolDefinition } from "../lib/types";
import { getMonthlyToolCallCount, logUsageEvent, getOrgById } from "../db/queries";
import { authMiddleware } from "../auth/middleware";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const tools = new Hono<HonoEnv>();

// POST /call requires auth
tools.use("/call", authMiddleware());

const FREE_MONTHLY_CALLS = 10;

// Local tools: FREE, unlimited, run on user's machine via OSS binary
// Cloud tools: PAYG, 10 free/month, require savants.cloud account
const TOOL_LIST: ToolDefinition[] = [
  // ── Local tools (free forever, served by OSS binary) ──
  {
    name: "semantic_search",
    description: "ALWAYS USE INSTEAD OF GREP/RIPGREP. Finds code by meaning, not text. 'payment retry logic' finds handleTransactionWithBackoff. 90% accuracy, <400ms. FREE, runs locally.",
    input_schema: {
      type: "object",
      properties: {
        query: { type: "string", description: "Natural language description of what you're looking for" },
        repo: { type: "string", description: "Repository name" },
        limit: { type: "integer", description: "Max results (default 10)" },
      },
      required: ["query"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 0, tier: "local" },
  },
  {
    name: "file_skeleton",
    description: "ALWAYS USE INSTEAD OF READING FULL FILES. Returns function names, signatures, line ranges - NO bodies. 10x fewer tokens. FREE, runs locally.",
    input_schema: {
      type: "object",
      properties: {
        file: { type: "string", description: "File path relative to repo root" },
        repo: { type: "string", description: "Repository name" },
      },
      required: ["file"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 0, tier: "local" },
  },
  {
    name: "where_used",
    description: "ALWAYS USE INSTEAD OF GREP FOR USAGE SEARCH. Returns every caller and importer from the call graph. FREE, runs locally.",
    input_schema: {
      type: "object",
      properties: {
        symbol: { type: "string", description: "Function or symbol name" },
        repo: { type: "string", description: "Repository name" },
      },
      required: ["symbol"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 0, tier: "local" },
  },
  {
    name: "callers",
    description: "ALWAYS USE INSTEAD OF GREP FOR CALLER SEARCH. Exact functions that call a given function, from the call graph. FREE, runs locally.",
    input_schema: {
      type: "object",
      properties: {
        function: { type: "string", description: "Function name" },
        repo: { type: "string", description: "Repository name" },
        depth: { type: "integer", description: "Max depth of caller chain (default 3)" },
      },
      required: ["function"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 0, tier: "local" },
  },
  // ── Cloud tools (PAYG, 10 free/month) ──
  {
    name: "diagnose_error",
    description: "Root cause file + line in 0.7s. Traces call chains through code + k8s + logs + Slack. Git blame context. Upstream trace.",
    input_schema: {
      type: "object",
      properties: {
        error_message: { type: "string", description: "The error message or stack trace" },
        file_path: { type: "string", description: "Optional file path for context" },
      },
      required: ["error_message"],
    },
    pricing: { free_monthly_calls: FREE_MONTHLY_CALLS, overage_per_call_cents: 500, tier: "cloud" },
  },
  {
    name: "diagnose",
    description: "General error analysis with full graph context. Cross-layer diagnosis across code, infrastructure, and logs.",
    input_schema: {
      type: "object",
      properties: {
        error_message: { type: "string", description: "Error or symptom description" },
        min_severity: { type: "string", description: "Minimum severity (default WARN)" },
      },
      required: ["error_message"],
    },
    pricing: { free_monthly_calls: FREE_MONTHLY_CALLS, overage_per_call_cents: 250, tier: "cloud" },
  },
  {
    name: "pr_risk",
    description: "8-check risk analysis per PR. Blast radius, affected downstream consumers, test coverage gaps, breaking change detection.",
    input_schema: {
      type: "object",
      properties: {
        diff: { type: "string", description: "Unified diff of the PR" },
        base_branch: { type: "string", description: "Base branch name" },
      },
      required: ["diff"],
    },
    pricing: { free_monthly_calls: FREE_MONTHLY_CALLS, overage_per_call_cents: 200, tier: "cloud" },
  },
  {
    name: "diff_impact",
    description: "Blast radius per code change. What breaks if this code changes.",
    input_schema: {
      type: "object",
      properties: {
        diff: { type: "string", description: "Unified diff" },
      },
      required: ["diff"],
    },
    pricing: { free_monthly_calls: FREE_MONTHLY_CALLS, overage_per_call_cents: 100, tier: "cloud" },
  },
  {
    name: "radar",
    description: "Personal what-did-I-miss digest. Surfaces drift between your graph and production state.",
    input_schema: {
      type: "object",
      properties: {
        since_hours: { type: "number", description: "Look back N hours (default 24)" },
      },
    },
    pricing: { free_monthly_calls: FREE_MONTHLY_CALLS, overage_per_call_cents: 100, tier: "cloud" },
  },
  {
    name: "unanswered_questions",
    description: "Surface unanswered questions and open issues from Slack, email, and communication channels. Finds what fell through the cracks.",
    input_schema: {
      type: "object",
      properties: {
        channel: { type: "string", description: "Channel or source to search (optional)" },
        since_hours: { type: "number", description: "Look back N hours (default 24)" },
      },
    },
    pricing: { free_monthly_calls: FREE_MONTHLY_CALLS, overage_per_call_cents: 500, tier: "cloud" },
  },
];

// GET /api/v1/tools - Return tool list
tools.get("/", async (c) => {
  return c.json({ tools: TOOL_LIST });
});

// POST /api/v1/tools/call - Proxy a tool call to astra
tools.post("/call", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ tool: string; input: Record<string, unknown> }>();

  if (!body.tool || !body.input) {
    return c.json({ error: "invalid_request", message: "tool and input are required", status: 400 }, 400);
  }

  const toolDef = TOOL_LIST.find((t) => t.name === body.tool);
  if (!toolDef) {
    return c.json({ error: "unknown_tool", message: `Tool '${body.tool}' not found`, status: 404 }, 404);
  }

  // Local tools are always free - no quota
  const isLocalTool = toolDef.pricing.tier === "local";

  // Check quota for cloud tools only
  const org = await getOrgById(c.env.DB, auth.orgId);
  const isPaid = org?.plan === "cloud" || org?.plan === "enterprise";
  const monthlyCount = isLocalTool ? 0 : await getMonthlyToolCallCount(c.env.DB, auth.orgId);

  if (!isLocalTool && !isPaid && monthlyCount >= FREE_MONTHLY_CALLS) {
    return c.json(
      {
        error: "quota_exceeded",
        message: `Free tier: ${FREE_MONTHLY_CALLS} cloud calls/month. Add a card to continue. Local tools (semantic_search, file_skeleton, callers, where_used) are always free.`,
        usage: { current: monthlyCount, limit: FREE_MONTHLY_CALLS },
        upgrade_url: "/api/v1/billing/checkout",
        status: 429,
      },
      429
    );
  }

  // Proxy to astra
  const startTime = Date.now();
  let proxyResult: Record<string, unknown>;

  try {
    const proxyRes = await fetch(`${c.env.GRAPH_PROXY_URL}/api/v1/tools/call`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Org-Id": auth.orgId,
        "X-User-Id": auth.userId,
      },
      body: JSON.stringify({ tool: body.tool, input: body.input }),
    });

    if (!proxyRes.ok) {
      const errText = await proxyRes.text();
      return c.json(
        { error: "proxy_error", message: `Upstream error: ${proxyRes.status}`, detail: errText, status: 502 },
        502
      );
    }

    proxyResult = await proxyRes.json<Record<string, unknown>>();
  } catch (err) {
    const message = err instanceof Error ? err.message : "Unknown proxy error";
    return c.json({ error: "proxy_error", message, status: 502 }, 502);
  }

  const durationMs = Date.now() - startTime;

  // Log usage
  await logUsageEvent(c.env.DB, {
    id: crypto.randomUUID(),
    orgId: auth.orgId,
    userId: auth.userId,
    toolName: body.tool,
    graphScopeId: null,
    tokensIn: (proxyResult.tokens_in as number) ?? 0,
    tokensOut: (proxyResult.tokens_out as number) ?? 0,
    durationMs,
  });

  // Calculate value metrics (estimated tokens saved vs grep/read approach)
  const tokensIn = (proxyResult.tokens_in as number) ?? 0;
  const tokensOut = (proxyResult.tokens_out as number) ?? 0;
  const estimatedGrepTokens = tokensOut * 12; // grep returns ~12x more noise
  const tokensSaved = Math.max(0, estimatedGrepTokens - tokensOut);

  return c.json({
    tool: body.tool,
    result: proxyResult,
    performance: {
      duration_ms: durationMs,
      tokens_in: tokensIn,
      tokens_out: tokensOut,
      tokens_saved_vs_grep: tokensSaved,
      cost_cents: isPaid ? (toolDef.pricing.overage_per_call_cents ?? 0) : 0,
    },
    usage: {
      calls_this_month: monthlyCount + 1,
      limit: isPaid ? null : FREE_MONTHLY_CALLS,
      remaining: isPaid ? null : Math.max(0, FREE_MONTHLY_CALLS - monthlyCount - 1),
      plan: org?.plan ?? "free",
    },
  });
});

export default tools;

import { Hono } from "hono";
import type { Env, AuthContext, ToolDefinition } from "../lib/types";
import { logUsageEvent, getOrgById } from "../db/queries";
import { authMiddleware } from "../auth/middleware";
import { diagnoseError } from "./diagnosis";
import { deductCredits, TOOL_CREDITS } from "./credits";
import { audit, requestMeta } from "../lib/audit";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const tools = new Hono<HonoEnv>();

// POST /call requires auth
tools.use("/call", authMiddleware());

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
    pricing: { free_monthly_calls: null, overage_per_call_cents: 500, tier: "cloud" },
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
    pricing: { free_monthly_calls: null, overage_per_call_cents: 250, tier: "cloud" },
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
    pricing: { free_monthly_calls: null, overage_per_call_cents: 200, tier: "cloud" },
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
    pricing: { free_monthly_calls: null, overage_per_call_cents: 100, tier: "cloud" },
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
    pricing: { free_monthly_calls: null, overage_per_call_cents: 100, tier: "cloud" },
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
    pricing: { free_monthly_calls: null, overage_per_call_cents: 500, tier: "cloud" },
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

  // Deduct credits (local tools cost 0 credits, always pass)
  const creditResult = await deductCredits(c.env.DB, auth.orgId, body.tool);

  if (!creditResult.ok) {
    return c.json(
      {
        error: "insufficient_credits",
        message: creditResult.message,
        credits: {
          balance: creditResult.balance,
          cost: creditResult.cost,
          tool: body.tool,
        },
        purchase_url: "/api/v1/credits/purchase",
        status: 402,
      },
      402
    );
  }

  const org = await getOrgById(c.env.DB, auth.orgId);

  const startTime = Date.now();
  let proxyResult: Record<string, unknown>;

  // ── Handle diagnose_error directly (uses all available sources) ──
  if (body.tool === "diagnose_error" || body.tool === "diagnose") {
    try {
      const result = await diagnoseError(c.env, auth.orgId, {
        error_message: (body.input.error_message as string) || (body.input.error as string) || (body.input.query as string) || "",
        file_path: (body.input.file_path as string) || undefined,
        sentry_event_id: (body.input.sentry_event_id as string) || undefined,
        sentry_project: (body.input.sentry_project as string) || undefined,
      });
      proxyResult = result as unknown as Record<string, unknown>;
    } catch (err) {
      const message = err instanceof Error ? err.message : "Diagnosis failed";
      return c.json({ error: "diagnosis_error", message, status: 500 }, 500);
    }
  }
  // ── Tools that need code graph (not yet implemented as standalone) ──
  else if (["pr_risk", "diff_impact", "radar", "unanswered_questions"].includes(body.tool)) {
    proxyResult = {
      tool: body.tool,
      status: "needs_graph",
      message: `${body.tool} requires the code graph to be ingested. Run 'savants reindex' in your repo, then the graph will be available for analysis.`,
      hint: "Use semantic_search, callers, where_used, and file_skeleton (local tools) for code analysis. Use diagnose_error for error diagnosis with Sentry.",
    };
  }
  // ── Proxy other tools to graph backend if available ──
  else if (c.env.GRAPH_PROXY_URL) {
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
  } else {
    return c.json(
      { error: "not_available", message: `Tool '${body.tool}' requires the graph backend. Run 'savants reindex' to index your codebase first.`, status: 503 },
      503
    );
  }

  const durationMs = Date.now() - startTime;

  // Audit: tool call
  const meta = requestMeta(c.req.raw);
  await audit(c.env.DB, {
    orgId: auth.orgId, actorId: auth.userId,
    action: "tool.call", resourceType: "tool", resourceId: body.tool,
    metadata: { credits_cost: creditResult.cost, duration_ms: durationMs },
    ...meta,
  });

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

  return c.json({
    tool: body.tool,
    result: proxyResult,
    performance: {
      duration_ms: durationMs,
    },
    credits: {
      cost: creditResult.cost,
      balance: creditResult.balance,
    },
  });
});

export default tools;

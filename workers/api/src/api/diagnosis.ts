/**
 * Standalone diagnosis engine.
 * Uses whatever data sources are available:
 *   1. Error message + stack trace (always available)
 *   2. Sentry event details (if integration configured)
 *   3. Code graph (if graph proxy available)
 *   4. K8s state (if cluster connected)
 *
 * Gracefully degrades - uses what it has.
 */

import type { Env } from "../lib/types";
import { getIntegration } from "../db/queries";

interface DiagnosisInput {
  error_message: string;
  file_path?: string;
  sentry_event_id?: string;
  sentry_project?: string;
}

interface DiagnosisResult {
  root_cause: string;
  file: string | null;
  line: number | null;
  function: string | null;
  call_chain: string[];
  suggested_fix: string;
  severity: "critical" | "error" | "warning";
  confidence: number;
  sources_used: string[];
  sentry_context?: {
    breadcrumbs: string[];
    tags: Record<string, string>;
    user: string | null;
  };
  graph_context?: {
    callers: string[];
    importers: string[];
    blast_radius: number;
  };
}

export async function diagnoseError(
  env: Env,
  orgId: string,
  input: DiagnosisInput
): Promise<DiagnosisResult> {
  const sources: string[] = [];
  let sentryData: any = null;
  let graphData: any = null;

  // ── Source 1: Parse the error message (always available) ──
  sources.push("error_message");
  const parsed = parseErrorMessage(input.error_message);

  // ── Source 2: Sentry enrichment (if integration exists) ──
  try {
    const sentryIntegration = await getIntegration(env.DB, orgId, "sentry");
    if (sentryIntegration) {
      const creds = JSON.parse(sentryIntegration.credentials);
      const config = JSON.parse(sentryIntegration.config);

      if (creds.auth_token) {
        // Try to fetch event details if we have event ID + project
        const project = input.sentry_project || config.project_slugs?.[0];
        if (input.sentry_event_id && project) {
          sentryData = await fetchSentryEvent(
            creds.auth_token,
            config.org_slug,
            project,
            input.sentry_event_id
          );
          if (sentryData) sources.push("sentry_event");
        }

        // If no specific event, search recent events matching the error
        if (!sentryData) {
          sentryData = await searchSentryEvents(
            creds.auth_token,
            config.org_slug,
            input.error_message
          );
          if (sentryData) sources.push("sentry_search");
        }
      }
    }
  } catch {
    // Sentry unavailable - continue without it
  }

  // ── Source 3: Code graph enrichment (if proxy available) ──
  if (env.GRAPH_PROXY_URL && parsed.function) {
    try {
      const proxyRes = await fetch(`${env.GRAPH_PROXY_URL}/api/v1/tools/call`, {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-Org-Id": orgId },
        body: JSON.stringify({
          tool: "callers",
          input: { function: parsed.function },
        }),
        signal: AbortSignal.timeout(5000),
      });
      if (proxyRes.ok) {
        graphData = await proxyRes.json();
        sources.push("code_graph");
      }
    } catch {
      // Graph unavailable - continue without it
    }
  }

  // ── Build diagnosis from all available sources ──
  return buildDiagnosis(parsed, sentryData, graphData, sources);
}

function parseErrorMessage(msg: string): {
  type: string;
  message: string;
  file: string | null;
  line: number | null;
  function: string | null;
  call_chain: string[];
} {
  const lines = msg.split("\n").map((l) => l.trim());
  let type = "Error";
  let message = msg;
  let file: string | null = null;
  let line: number | null = null;
  let fn: string | null = null;
  const call_chain: string[] = [];

  // Extract error type: "TypeName: message"
  const typeMatch = msg.match(/^(\w+Error|\w+Exception):\s*(.*)/);
  if (typeMatch) {
    type = typeMatch[1];
    message = typeMatch[2];
  }

  // Extract file:line from patterns like "at file.ts:123" or "in file.ts:123"
  const fileMatch = msg.match(/(?:at|in)\s+([^\s:]+\.[a-z]+):(\d+)/i);
  if (fileMatch) {
    file = fileMatch[1];
    line = parseInt(fileMatch[2]);
  }

  // Extract function name from "in functionName()" or "at functionName"
  const fnMatch = msg.match(/(?:at|in)\s+(\w+)\(\)/);
  if (fnMatch) {
    fn = fnMatch[1];
  }

  // Build call chain from "at X -> at Y -> at Z" or stack trace lines
  const stackLines = lines.filter(
    (l) => l.match(/^\s*(at\s+|->|→)/) || l.match(/\w+\(\)/)
  );
  for (const sl of stackLines) {
    const fnName = sl.match(/(?:at\s+)?(\w+(?:\.\w+)*)\s*\(/)?.[1];
    if (fnName) call_chain.push(fnName);
  }

  // Also extract from "X() -> Y() -> Z()" patterns
  const chainMatch = msg.match(/(\w+\(\)(?:\s*(?:->|→|at)\s*\w+\(\))+)/);
  if (chainMatch && call_chain.length === 0) {
    const fns = chainMatch[0].match(/\w+(?=\(\))/g);
    if (fns) call_chain.push(...fns);
  }

  return { type, message, file, line, function: fn, call_chain };
}

async function fetchSentryEvent(
  token: string,
  org: string,
  project: string,
  eventId: string
): Promise<any | null> {
  try {
    const res = await fetch(
      `https://sentry.io/api/0/projects/${org}/${project}/events/${eventId}/`,
      {
        headers: { Authorization: `Bearer ${token}` },
        signal: AbortSignal.timeout(5000),
      }
    );
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

async function searchSentryEvents(
  token: string,
  org: string,
  errorMessage: string
): Promise<any | null> {
  // Search across all projects for matching recent events
  try {
    const query = encodeURIComponent(errorMessage.slice(0, 100));
    const res = await fetch(
      `https://sentry.io/api/0/organizations/${org}/events/?query=${query}&per_page=1&full=true`,
      {
        headers: { Authorization: `Bearer ${token}` },
        signal: AbortSignal.timeout(5000),
      }
    );
    if (!res.ok) return null;
    const events = (await res.json()) as any[];
    return events?.[0] ?? null;
  } catch {
    return null;
  }
}

function buildDiagnosis(
  parsed: ReturnType<typeof parseErrorMessage>,
  sentryData: any | null,
  graphData: any | null,
  sources: string[]
): DiagnosisResult {
  let rootCause = "";
  let file = parsed.file;
  let line = parsed.line;
  let fn = parsed.function;
  let callChain = parsed.call_chain;
  let suggestedFix = "";
  let severity: "critical" | "error" | "warning" = "error";
  let confidence = 0.5;
  let sentryContext: DiagnosisResult["sentry_context"] = undefined;
  let graphContext: DiagnosisResult["graph_context"] = undefined;

  // ── Enrich from Sentry ──
  if (sentryData) {
    confidence += 0.2;

    // Extract better stack trace
    const entries = sentryData.entries || [];
    for (const entry of entries) {
      if (entry.type === "exception") {
        const values = entry.data?.values || [];
        for (const exc of values) {
          const frames = exc.stacktrace?.frames || [];
          const appFrames = frames.filter((f: any) => f.inApp);
          if (appFrames.length > 0) {
            const topFrame = appFrames[appFrames.length - 1];
            file = topFrame.filename || file;
            line = topFrame.lineNo || line;
            fn = topFrame.function || fn;
            callChain = appFrames.map(
              (f: any) => `${f.function || "?"}() at ${f.filename || "?"}:${f.lineNo || "?"}`
            );
          }
        }
      }

      if (entry.type === "breadcrumbs") {
        const crumbs = entry.data?.values || [];
        sentryContext = {
          breadcrumbs: crumbs.slice(-5).map(
            (b: any) => `[${b.category || "?"}] ${b.message || JSON.stringify(b.data || {})}`
          ),
          tags: {},
          user: null,
        };
      }
    }

    // Tags
    const tags = sentryData.tags || [];
    if (sentryContext && Array.isArray(tags)) {
      for (const t of tags) {
        if (t.key && t.value) sentryContext.tags[t.key] = t.value;
      }
    }

    // User
    if (sentryData.user && sentryContext) {
      sentryContext.user =
        sentryData.user.email || sentryData.user.username || sentryData.user.ip_address || null;
    }
  }

  // ── Enrich from code graph ──
  if (graphData?.result) {
    confidence += 0.15;
    graphContext = {
      callers: [],
      importers: [],
      blast_radius: 0,
    };
    // Parse graph result if available
    const result = graphData.result;
    if (typeof result === "string") {
      const callerLines = result.split("\n").filter((l: string) => l.includes("("));
      graphContext.callers = callerLines.slice(0, 10);
      graphContext.blast_radius = callerLines.length;
    }
  }

  // ── Generate diagnosis based on error patterns ──
  const errorType = parsed.type;
  const errorMsg = parsed.message;

  if (errorType.includes("Prisma") || errorMsg.includes("Foreign key")) {
    rootCause = `Database foreign key constraint violation. The code at ${file || "?"}:${line || "?"} in ${fn || "?"}() tries to create a record referencing a row that doesn't exist in the parent table.`;
    suggestedFix = `1. Add a null check before the database write: verify the referenced ID exists with a findUnique() call.\n2. Wrap the create() in a try/catch so the error handler doesn't itself crash.\n3. Consider using connectOrCreate instead of a hard foreign key reference.`;
    severity = "error";
    confidence += 0.15;
  } else if (errorType.includes("LLMValidation") || errorMsg.includes("schema validation")) {
    rootCause = `LLM output does not match the expected schema. The AI model returned data that fails validation at ${file || "?"}:${line || "?"}.`;
    suggestedFix = `1. Add a fallback/retry with a more explicit prompt when validation fails.\n2. Use a default value for fields the LLM gets wrong (e.g., extract email from input data instead of LLM output).\n3. Consider using structured output mode (JSON mode) if available.`;
    severity = "warning";
    confidence += 0.1;
  } else if (errorType.includes("TypeError") || errorMsg.includes("Cannot read prop")) {
    const propMatch = errorMsg.match(/property '(\w+)' of (undefined|null)/);
    const prop = propMatch?.[1] || "unknown";
    rootCause = `Null reference: accessing property '${prop}' on undefined/null at ${file || "?"}:${line || "?"}.`;
    suggestedFix = `Add optional chaining (?.) or a null guard before accessing '${prop}'.`;
    severity = "error";
    confidence += 0.1;
  } else if (errorType.includes("ConnectionError") || errorMsg.includes("ECONNREFUSED")) {
    rootCause = `Service connection failure at ${file || "?"}:${line || "?"}. The downstream service is unreachable.`;
    suggestedFix = `1. Check if the target service is running and healthy.\n2. Add retry logic with exponential backoff.\n3. Add a circuit breaker to fail fast.`;
    severity = "critical";
  } else {
    rootCause = `${errorType} at ${file || "?"}:${line || "?"} in ${fn || "?"}(): ${errorMsg.slice(0, 200)}`;
    suggestedFix = `Review the code at the error location and add appropriate error handling.`;
  }

  // Cascade analysis
  if (callChain.length > 1) {
    rootCause += `\n\nCall chain: ${callChain.join(" → ")}`;
    if (callChain.length > 3) {
      rootCause += `\n\nThis error propagates through ${callChain.length} frames. The originating call is ${callChain[callChain.length - 1]}.`;
    }
  }

  return {
    root_cause: rootCause,
    file,
    line,
    function: fn,
    call_chain: callChain,
    suggested_fix: suggestedFix,
    severity,
    confidence: Math.min(confidence, 1),
    sources_used: sources,
    sentry_context: sentryContext,
    graph_context: graphContext,
  };
}

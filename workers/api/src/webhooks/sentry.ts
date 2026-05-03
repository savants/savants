import { Hono } from "hono";
import type { Env, AuthContext, SentryConfig, SentryCredentials } from "../lib/types";
import { bufToHex, hmacSign } from "../lib/crypto";
import { getIntegrationsByType, getIntegration, logUsageEvent } from "../db/queries";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const sentry = new Hono<HonoEnv>();

async function verifySentrySignature(
  payload: string,
  sigHeader: string,
  secret: string
): Promise<boolean> {
  const expectedSig = bufToHex(await hmacSign(secret, payload));

  if (sigHeader.length !== expectedSig.length) return false;
  let result = 0;
  for (let i = 0; i < sigHeader.length; i++) {
    result |= sigHeader.charCodeAt(i) ^ expectedSig.charCodeAt(i);
  }
  return result === 0;
}

// POST /webhooks/sentry
sentry.post("/", async (c) => {
  const sigHeader = c.req.header("sentry-hook-signature") ?? "";
  const resource = c.req.header("sentry-hook-resource") ?? "";

  if (!sigHeader) {
    return c.json({ error: "missing_signature", message: "No Sentry signature header", status: 400 }, 400);
  }

  const rawBody = await c.req.text();

  // Look up the integration by matching the client_secret against all sentry integrations.
  // Sentry webhooks do not include an org identifier, so we verify the signature against
  // each configured client_secret until one matches.
  const sentryIntegrations = await getIntegrationsByType(c.env.DB, "sentry");

  let matchedOrgId: string | null = null;
  let matchedConfig: SentryConfig | null = null;
  let matchedCredentials: SentryCredentials | null = null;

  for (const integration of sentryIntegrations) {
    const creds = JSON.parse(integration.credentials) as SentryCredentials;
    if (!creds.client_secret) continue;

    const valid = await verifySentrySignature(rawBody, sigHeader, creds.client_secret);
    if (valid) {
      matchedOrgId = integration.org_id;
      matchedConfig = JSON.parse(integration.config) as SentryConfig;
      matchedCredentials = creds;
      break;
    }
  }

  if (!matchedOrgId || !matchedConfig || !matchedCredentials) {
    return c.json({ error: "invalid_signature", message: "Sentry signature verification failed", status: 401 }, 401);
  }

  // Handle different resource types
  switch (resource) {
    case "installation":
      return c.json({ received: true, resource });

    case "event_alert":
    case "issue": {
      const payload = JSON.parse(rawBody);
      return await handleSentryAlert(c, payload, resource, matchedOrgId, matchedConfig, matchedCredentials);
    }

    case "metric_alert":
      console.log(`[sentry] metric_alert received for org ${matchedOrgId}, not processed yet`);
      return c.json({ received: true, resource });

    default:
      return c.json({ received: true, resource });
  }
});

async function handleSentryAlert(
  c: { env: Env; json: (data: unknown, status?: number) => Response },
  payload: Record<string, unknown>,
  resource: string,
  orgId: string,
  config: SentryConfig,
  credentials: SentryCredentials
): Promise<Response> {
  // Extract error details from the Sentry payload
  const data = (payload.data ?? {}) as Record<string, unknown>;

  let errorMessage = "";
  let stacktrace = "";
  let eventId = "";
  let projectSlug = "";
  let issueUrl = "";

  if (resource === "event_alert") {
    const event = (data.event ?? {}) as Record<string, unknown>;
    errorMessage = (event.title as string) ?? (event.message as string) ?? "";
    eventId = (event.event_id as string) ?? "";
    issueUrl = (event.web_url as string) ?? "";

    const triggeredRule = (data.triggered_rule as string) ?? "";
    if (triggeredRule && !errorMessage) {
      errorMessage = triggeredRule;
    }

    // Try to extract project slug from tags or event URL
    const tags = (event.tags ?? []) as Array<{ key: string; value: string }>;
    const projectTag = tags.find((t) => t.key === "project");
    if (projectTag) {
      projectSlug = projectTag.value;
    }

    // Extract exception data if present
    const exception = (event.exception ?? {}) as { values?: Array<{ type?: string; value?: string; stacktrace?: { frames?: Array<{ filename?: string; function?: string; lineno?: number; context_line?: string }> } }> };
    if (exception.values && exception.values.length > 0) {
      const exc = exception.values[0];
      if (exc.type && exc.value) {
        errorMessage = `${exc.type}: ${exc.value}`;
      }
      if (exc.stacktrace?.frames) {
        stacktrace = exc.stacktrace.frames
          .slice(-20)
          .map((f) => {
            const loc = [f.filename, f.function, f.lineno].filter(Boolean).join(":");
            return f.context_line ? `  ${loc}\n    ${f.context_line.trim()}` : `  ${loc}`;
          })
          .join("\n");
      }
    }
  } else if (resource === "issue") {
    const issue = (data.issue ?? data) as Record<string, unknown>;
    errorMessage = (issue.title as string) ?? (issue.culprit as string) ?? "";
    issueUrl = (issue.web_url as string) ?? "";

    const metadata = (issue.metadata ?? {}) as Record<string, string>;
    if (metadata.type && metadata.value) {
      errorMessage = `${metadata.type}: ${metadata.value}`;
    }
    if (metadata.filename) {
      stacktrace = `  at ${metadata.filename}${metadata.function ? `:${metadata.function}` : ""}`;
    }
  }

  if (!errorMessage) {
    return c.json({ received: true, resource, skipped: true, reason: "no_error_message" });
  }

  // If we have an event ID and project slug, fetch full event details from Sentry API
  let breadcrumbs = "";
  let sentryTags = "";
  let sentryContexts = "";

  if (eventId && (projectSlug || config.project_slugs?.[0])) {
    const project = projectSlug || config.project_slugs![0];
    try {
      const eventRes = await fetch(
        `https://sentry.io/api/0/projects/${config.org_slug}/${project}/events/${eventId}/`,
        {
          headers: {
            Authorization: `Bearer ${credentials.auth_token}`,
            "Content-Type": "application/json",
          },
        }
      );

      if (eventRes.ok) {
        const eventData = await eventRes.json<Record<string, unknown>>();

        // Extract richer exception data
        const entries = (eventData.entries ?? []) as Array<{ type: string; data: Record<string, unknown> }>;
        for (const entry of entries) {
          if (entry.type === "exception" && !stacktrace) {
            const values = ((entry.data.values ?? []) as Array<{ type?: string; value?: string; stacktrace?: { frames?: Array<{ filename?: string; function?: string; lineno?: number; context_line?: string }> } }>);
            if (values.length > 0) {
              const exc = values[0];
              if (exc.type && exc.value) {
                errorMessage = `${exc.type}: ${exc.value}`;
              }
              if (exc.stacktrace?.frames) {
                stacktrace = exc.stacktrace.frames
                  .slice(-20)
                  .map((f) => {
                    const loc = [f.filename, f.function, f.lineno].filter(Boolean).join(":");
                    return f.context_line ? `  ${loc}\n    ${f.context_line.trim()}` : `  ${loc}`;
                  })
                  .join("\n");
              }
            }
          }

          if (entry.type === "breadcrumbs") {
            const crumbs = ((entry.data.values ?? []) as Array<{ category?: string; message?: string; level?: string }>);
            breadcrumbs = crumbs
              .slice(-10)
              .map((b) => `  [${b.level ?? "info"}] ${b.category ?? ""}: ${b.message ?? ""}`)
              .join("\n");
          }
        }

        // Extract tags
        const tags = (eventData.tags ?? []) as Array<{ key: string; value: string }>;
        sentryTags = tags
          .filter((t) => ["environment", "server_name", "release", "browser", "os", "runtime"].includes(t.key))
          .map((t) => `${t.key}=${t.value}`)
          .join(", ");

        // Extract contexts
        const contexts = (eventData.contexts ?? {}) as Record<string, Record<string, unknown>>;
        const contextParts: string[] = [];
        if (contexts.runtime) contextParts.push(`runtime: ${contexts.runtime.name ?? ""} ${contexts.runtime.version ?? ""}`);
        if (contexts.os) contextParts.push(`os: ${contexts.os.name ?? ""} ${contexts.os.version ?? ""}`);
        if (contexts.browser) contextParts.push(`browser: ${contexts.browser.name ?? ""} ${contexts.browser.version ?? ""}`);
        sentryContexts = contextParts.join(", ");

        // Extract user info
        const user = eventData.user as Record<string, string> | undefined;
        if (user) {
          sentryContexts += sentryContexts ? `, user: ${user.email ?? user.username ?? user.id ?? "unknown"}` : `user: ${user.email ?? user.username ?? user.id ?? "unknown"}`;
        }
      }
    } catch {
      // Non-fatal: proceed with what we have from the webhook payload
    }
  }

  // Build the diagnosis prompt
  let diagnosisInput = errorMessage;
  if (stacktrace) diagnosisInput += `\n\nStacktrace:\n${stacktrace}`;
  if (breadcrumbs) diagnosisInput += `\n\nBreadcrumbs:\n${breadcrumbs}`;
  if (sentryTags) diagnosisInput += `\n\nTags: ${sentryTags}`;
  if (sentryContexts) diagnosisInput += `\n\nContext: ${sentryContexts}`;

  // Check auto_diagnose setting (defaults to true)
  if (config.auto_diagnose === false) {
    return c.json({ received: true, resource, auto_diagnose: false, error_message: errorMessage });
  }

  // Run diagnosis directly (no proxy needed - we have the diagnosis engine locally)
  const { diagnoseError } = await import("../api/diagnosis");
  const startTime = Date.now();
  let diagnosisResult: Record<string, unknown> = {};

  try {
    const result = await diagnoseError(c.env, orgId, {
      error_message: diagnosisInput,
      sentry_event_id: eventId || undefined,
      sentry_project: projectSlug || undefined,
    });
    diagnosisResult = result as unknown as Record<string, unknown>;
  } catch (err) {
    const message = err instanceof Error ? err.message : "Diagnosis failed";
    console.error(`[sentry] diagnosis error for org ${orgId}: ${message}`);
    return c.json({ received: true, resource, diagnosis_error: true, message });
  }

  const durationMs = Date.now() - startTime;

  // Store as graph event for causal inference
  try {
    // Find the project for this sentry project slug
    const project = await c.env.DB.prepare(
      "SELECT project_id FROM project_sources WHERE source_type = 'sentry' AND source_config LIKE ?1 LIMIT 1"
    ).bind(`%${projectSlug || "vocator"}%`).first<{ project_id: string }>();

    if (project) {
      // Match error to a code graph node via function name from stack trace
      let nodeId: string | null = null;
      const fn = (diagnosisResult as any).function;
      if (fn) {
        const node = await c.env.DB.prepare(
          "SELECT id FROM graph_nodes WHERE project_id = ?1 AND name = ?2 LIMIT 1"
        ).bind(project.project_id, fn).first<{ id: string }>();
        nodeId = node?.id || null;
      }

      await c.env.DB.prepare(`
        INSERT INTO graph_events (id, project_id, type, title, description, severity, node_id, source_type, source_ref, metadata, occurred_at)
        VALUES (?1, ?2, 'sentry_error', ?3, ?4, ?5, ?6, 'sentry', ?7, ?8, ?9)
      `).bind(
        crypto.randomUUID(),
        project.project_id,
        errorMessage.slice(0, 200),
        diagnosisInput.slice(0, 1000),
        (diagnosisResult as any).severity || "error",
        nodeId,
        eventId || issueUrl,
        JSON.stringify({ project_slug: projectSlug, issue_url: issueUrl, confidence: (diagnosisResult as any).confidence }),
        Math.floor(Date.now() / 1000),
      ).run();
    }
  } catch {
    // Non-fatal: graph storage failed but diagnosis still proceeds
  }

  // Log usage event
  await logUsageEvent(c.env.DB, {
    id: crypto.randomUUID(),
    orgId,
    userId: null,
    toolName: "diagnose_error",
    graphScopeId: null,
    tokensIn: (diagnosisResult.tokens_in as number) ?? 0,
    tokensOut: (diagnosisResult.tokens_out as number) ?? 0,
    durationMs,
  });

  // Format the diagnosis for Slack
  const dr = diagnosisResult as any;
  let diagnosis = dr.root_cause || "";
  if (dr.file) diagnosis += `\nFile: ${dr.file}:${dr.line || "?"}`;
  if (dr.function) diagnosis += `\nFunction: ${dr.function}()`;
  if (dr.suggested_fix) diagnosis += `\nFix: ${dr.suggested_fix}`;
  if (dr.confidence) diagnosis += `\nConfidence: ${Math.round(dr.confidence * 100)}%`;
  if (!diagnosis) diagnosis = JSON.stringify(diagnosisResult, null, 2).substring(0, 2000);

  // Post to Slack if the org has a Slack integration configured
  if (config.slack_channel) {
    const slackIntegration = await getIntegration(c.env.DB, orgId, "slack");
    const slackToken = slackIntegration
      ? (JSON.parse(slackIntegration.credentials) as { bot_token?: string }).bot_token
      : c.env.SLACK_BOT_TOKEN;

    if (slackToken) {
      const slackMessage = formatSlackMessage(errorMessage, diagnosis, issueUrl);

      try {
        await fetch("https://slack.com/api/chat.postMessage", {
          method: "POST",
          headers: {
            Authorization: `Bearer ${slackToken}`,
            "Content-Type": "application/json",
          },
          body: JSON.stringify({
            channel: config.slack_channel,
            text: slackMessage.fallback,
            blocks: slackMessage.blocks,
          }),
        });
      } catch (err) {
        const message = err instanceof Error ? err.message : "Unknown Slack error";
        console.error(`[sentry] Slack post error for org ${orgId}: ${message}`);
      }
    }
  }

  return c.json({
    received: true,
    resource,
    diagnosed: true,
    error_message: errorMessage,
    duration_ms: durationMs,
  });
}

function formatSlackMessage(
  errorMessage: string,
  diagnosis: string,
  issueUrl: string
): { fallback: string; blocks: Array<Record<string, unknown>> } {
  const fallback = `Sentry Alert - ${errorMessage}\n\nDiagnosis:\n${diagnosis}`;

  const blocks: Array<Record<string, unknown>> = [
    {
      type: "header",
      text: {
        type: "plain_text",
        text: "Sentry Alert - Auto-Diagnosed",
        emoji: false,
      },
    },
    {
      type: "section",
      text: {
        type: "mrkdwn",
        text: `*Error:*\n\`\`\`${errorMessage.substring(0, 500)}\`\`\``,
      },
    },
    {
      type: "section",
      text: {
        type: "mrkdwn",
        text: `*Diagnosis:*\n${diagnosis.substring(0, 2500)}`,
      },
    },
  ];

  if (issueUrl) {
    blocks.push({
      type: "section",
      text: {
        type: "mrkdwn",
        text: `<${issueUrl}|View in Sentry>`,
      },
    });
  }

  blocks.push({
    type: "context",
    elements: [
      {
        type: "mrkdwn",
        text: "Automated by Savants - sentry integration",
      },
    ],
  });

  return { fallback, blocks };
}

export default sentry;

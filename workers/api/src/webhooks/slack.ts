import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const slack = new Hono<HonoEnv>();

// POST /webhooks/slack
slack.post("/", async (c) => {
  const contentType = c.req.header("content-type") ?? "";
  let body: Record<string, unknown>;

  if (contentType.includes("application/x-www-form-urlencoded")) {
    const formData = await c.req.parseBody();
    // Slack sends JSON payload inside a "payload" field for interactive components
    if (typeof formData.payload === "string") {
      body = JSON.parse(formData.payload);
    } else {
      body = formData as Record<string, unknown>;
    }
  } else {
    body = await c.req.json<Record<string, unknown>>();
  }

  // Handle Slack URL verification challenge
  if (body.type === "url_verification") {
    return c.json({ challenge: body.challenge });
  }

  // Verify Slack request timestamp to prevent replay attacks
  const slackTimestamp = c.req.header("x-slack-request-timestamp");
  if (slackTimestamp) {
    const ts = parseInt(slackTimestamp, 10);
    const now = Math.floor(Date.now() / 1000);
    if (Math.abs(now - ts) > 300) {
      return c.json({ error: "stale_request", message: "Request timestamp too old", status: 401 }, 401);
    }
  }

  // Handle slash commands
  if (body.command) {
    return await handleSlashCommand(c, body);
  }

  // Handle event callbacks
  if (body.type === "event_callback") {
    const event = body.event as Record<string, unknown>;
    if (event.type === "app_mention" || event.type === "message") {
      return await handleMessage(c, event);
    }
  }

  return c.json({ ok: true });
});

async function handleSlashCommand(
  c: { env: Env; json: (data: unknown, status?: number) => Response },
  body: Record<string, unknown>
): Promise<Response> {
  const command = body.command as string;
  const text = (body.text as string) ?? "";
  const responseUrl = body.response_url as string;
  const channelId = body.channel_id as string;

  let toolName = "";
  let toolInput: Record<string, unknown> = {};

  if (command === "/savants" || command === "/svt") {
    const parts = text.trim().split(/\s+/);
    const subcommand = parts[0] ?? "help";
    const rest = parts.slice(1).join(" ");

    switch (subcommand) {
      case "diagnose":
        toolName = "diagnose_error";
        toolInput = { error_message: rest };
        break;
      case "explain":
        toolName = "explain_symbol";
        toolInput = { symbol: rest };
        break;
      case "callers":
        toolName = "find_callers";
        toolInput = { symbol: rest };
        break;
      case "unanswered":
        toolName = "unanswered_questions";
        toolInput = { channel: channelId, since_hours: parseInt(rest, 10) || 24 };
        break;
      case "help":
      default:
        return c.json({
          response_type: "ephemeral",
          text: [
            "*Savants Commands:*",
            "`/savants diagnose <error>` - Diagnose an error",
            "`/savants explain <symbol>` - Explain a function/class",
            "`/savants callers <symbol>` - Find all callers",
            "`/savants unanswered [hours]` - Surface unanswered questions",
            "`/savants help` - Show this help",
          ].join("\n"),
        });
    }
  }

  if (!toolName) {
    return c.json({ response_type: "ephemeral", text: "Unknown command. Try `/savants help`" });
  }

  // Acknowledge immediately, then process async via response_url
  // We send the proxy request and post the result back
  const env = c.env;

  // Use waitUntil pattern - since we are in Hono, we process inline but respond fast
  // Respond with a "thinking" message first
  const thinkingResponse = {
    response_type: "in_channel" as const,
    text: `Running \`${toolName}\`...`,
  };

  // Fire off the proxy call and post result back to Slack
  processAndRespond(env, toolName, toolInput, responseUrl, channelId).catch(() => {
    // If the background processing fails, we already sent the thinking message
  });

  return c.json(thinkingResponse);
}

async function processAndRespond(
  env: Env,
  toolName: string,
  toolInput: Record<string, unknown>,
  responseUrl: string,
  channelId: string
): Promise<void> {
  let resultText = "";

  try {
    const proxyRes = await fetch(`${env.GRAPH_PROXY_URL}/api/v1/tools/call`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Webhook-Source": "slack",
        "X-Channel-Id": channelId,
      },
      body: JSON.stringify({ tool: toolName, input: toolInput }),
    });

    if (proxyRes.ok) {
      const result = await proxyRes.json<Record<string, unknown>>();
      resultText = formatToolResult(toolName, result);
    } else {
      const errText = await proxyRes.text();
      resultText = `Error from analysis engine: ${proxyRes.status} - ${errText.substring(0, 500)}`;
    }
  } catch (err) {
    const message = err instanceof Error ? err.message : "Unknown error";
    resultText = `Failed to reach analysis engine: ${message}`;
  }

  // Post result back to Slack via response_url
  await fetch(responseUrl, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      response_type: "in_channel",
      replace_original: true,
      text: resultText,
    }),
  });
}

async function handleMessage(
  c: { env: Env; json: (data: unknown, status?: number) => Response },
  event: Record<string, unknown>
): Promise<Response> {
  const text = (event.text as string) ?? "";
  const channel = event.channel as string;
  const threadTs = (event.thread_ts as string) ?? (event.ts as string);

  // Simple heuristic: if the message contains "diagnose" or an error-like pattern
  let toolName = "";
  let toolInput: Record<string, unknown> = {};

  const cleaned = text.replace(/<@[A-Z0-9]+>/g, "").trim();

  if (/error|exception|traceback|panic|failed/i.test(cleaned)) {
    toolName = "diagnose_error";
    toolInput = { error_message: cleaned };
  } else if (/explain|what is|what does/i.test(cleaned)) {
    const symbol = cleaned.replace(/^(explain|what is|what does)\s+/i, "").trim();
    toolName = "explain_symbol";
    toolInput = { symbol };
  } else if (/callers?|who calls/i.test(cleaned)) {
    const symbol = cleaned.replace(/^(callers?|who calls)\s+/i, "").trim();
    toolName = "find_callers";
    toolInput = { symbol };
  } else if (/unanswered|open questions/i.test(cleaned)) {
    toolName = "unanswered_questions";
    toolInput = { channel, since_hours: 24 };
  } else {
    // Default to diagnose if we cannot classify
    toolName = "diagnose_error";
    toolInput = { error_message: cleaned };
  }

  // Proxy to astra
  let resultText = "";
  try {
    const proxyRes = await fetch(`${c.env.GRAPH_PROXY_URL}/api/v1/tools/call`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Webhook-Source": "slack",
        "X-Channel-Id": channel,
      },
      body: JSON.stringify({ tool: toolName, input: toolInput }),
    });

    if (proxyRes.ok) {
      const result = await proxyRes.json<Record<string, unknown>>();
      resultText = formatToolResult(toolName, result);
    } else {
      resultText = "I had trouble analyzing that. Please try again.";
    }
  } catch {
    resultText = "I could not reach the analysis engine. Please try again later.";
  }

  // Post reply to Slack
  await fetch("https://slack.com/api/chat.postMessage", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${c.env.SLACK_BOT_TOKEN}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      channel,
      thread_ts: threadTs,
      text: resultText,
    }),
  });

  return c.json({ ok: true });
}

function formatToolResult(toolName: string, result: Record<string, unknown>): string {
  if (result.summary) {
    return `*${toolName}*\n${result.summary}`;
  }

  if (result.diagnosis) {
    return `*Diagnosis*\n${result.diagnosis}`;
  }

  if (result.explanation) {
    return `*${result.symbol ?? toolName}*\n${result.explanation}`;
  }

  if (result.callers && Array.isArray(result.callers)) {
    const callerList = (result.callers as Array<{ name: string; file: string }>)
      .slice(0, 10)
      .map((caller) => `- \`${caller.name}\` in \`${caller.file}\``)
      .join("\n");
    return `*Callers*\n${callerList}`;
  }

  if (result.questions && Array.isArray(result.questions)) {
    const questionList = (result.questions as Array<{ text: string; source: string }>)
      .slice(0, 10)
      .map((q, i) => `${i + 1}. ${q.text} (from ${q.source})`)
      .join("\n");
    return `*Unanswered Questions*\n${questionList}`;
  }

  // Fallback: pretty-print the JSON
  return `*${toolName}*\n\`\`\`${JSON.stringify(result, null, 2).substring(0, 2000)}\`\`\``;
}

export default slack;

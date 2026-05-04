/**
 * Slack tools - replaces the Slack MCP server.
 *
 * 8 core tools: search, history, replies, post, react, channels, users, unreads.
 * Graph-enriched: messages mentioning functions linked to code graph.
 * Plus: unanswered questions detection (killer feature).
 */

import type { Env } from "../lib/types";
import { getIntegration } from "../db/queries";

async function getSlackToken(db: Env["DB"], orgId: string): Promise<string | null> {
  const row = await getIntegration(db, orgId, "slack");
  if (!row) return null;
  const config = JSON.parse(row.config || "{}");
  const creds = JSON.parse(row.credentials || "{}");
  return creds.bot_token || config.bot_token || null;
}

async function slackGet(token: string, method: string, params: Record<string, string> = {}): Promise<any> {
  const qs = new URLSearchParams(params).toString();
  const res = await fetch(`https://slack.com/api/${method}?${qs}`, {
    headers: { Authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(10000),
  });
  if (!res.ok) return null;
  const data = await res.json<any>();
  return data.ok ? data : null;
}

async function slackPost(token: string, method: string, body: Record<string, unknown>): Promise<any> {
  const res = await fetch(`https://slack.com/api/${method}`, {
    method: "POST",
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(10000),
  });
  if (!res.ok) return null;
  return res.json();
}

// ── search_slack_messages ──
async function searchMessages(db: Env["DB"], orgId: string, input: { query: string; count?: number }): Promise<any> {
  const token = await getSlackToken(db, orgId);
  if (!token) return { error: "Slack not connected" };
  const data = await slackGet(token, "search.messages", { query: input.query, count: String(input.count || 10), sort: "timestamp" });
  if (!data) return { results: [], count: 0 };
  return {
    results: data.messages?.matches?.map((m: any) => ({
      text: m.text?.slice(0, 300), user: m.username || m.user, channel: m.channel?.name,
      timestamp: m.ts, permalink: m.permalink,
    })) || [],
    count: data.messages?.total || 0,
  };
}

// ── get_slack_channel_history ──
async function getHistory(db: Env["DB"], orgId: string, input: { channel: string; limit?: number }): Promise<any> {
  const token = await getSlackToken(db, orgId);
  if (!token) return { error: "Slack not connected" };

  // Resolve channel name to ID
  let channelId = input.channel;
  if (!channelId.startsWith("C")) {
    const channels = await slackGet(token, "conversations.list", { limit: "200", types: "public_channel,private_channel" });
    const match = channels?.channels?.find((c: any) => c.name === input.channel || c.name === input.channel.replace("#", ""));
    channelId = match?.id || input.channel;
  }

  const data = await slackGet(token, "conversations.history", { channel: channelId, limit: String(input.limit || 20) });
  if (!data) return { messages: [] };
  return {
    messages: data.messages?.map((m: any) => ({
      text: m.text?.slice(0, 300), user: m.user, timestamp: m.ts,
      thread_ts: m.thread_ts, reply_count: m.reply_count,
      reactions: m.reactions?.map((r: any) => `${r.name}(${r.count})`) || [],
    })) || [],
  };
}

// ── get_slack_thread_replies ──
async function getThreadReplies(db: Env["DB"], orgId: string, input: { channel: string; thread_ts: string }): Promise<any> {
  const token = await getSlackToken(db, orgId);
  if (!token) return { error: "Slack not connected" };
  const data = await slackGet(token, "conversations.replies", { channel: input.channel, ts: input.thread_ts, limit: "50" });
  if (!data) return { replies: [] };
  return {
    replies: data.messages?.map((m: any) => ({
      text: m.text?.slice(0, 300), user: m.user, timestamp: m.ts,
    })) || [],
  };
}

// ── post_slack_message ──
async function postMessage(db: Env["DB"], orgId: string, input: { channel: string; text: string; thread_ts?: string }): Promise<any> {
  const token = await getSlackToken(db, orgId);
  if (!token) return { error: "Slack not connected" };

  // Resolve channel name to ID
  let channelId = input.channel;
  if (!channelId.startsWith("C")) {
    const channels = await slackGet(token, "conversations.list", { limit: "200" });
    const match = channels?.channels?.find((c: any) => c.name === input.channel.replace("#", ""));
    channelId = match?.id || input.channel;
  }

  const body: any = { channel: channelId, text: input.text };
  if (input.thread_ts) body.thread_ts = input.thread_ts;

  const data = await slackPost(token, "chat.postMessage", body);
  return { posted: data?.ok || false, ts: data?.ts, channel: data?.channel };
}

// ── list_slack_channels ──
async function listChannels(db: Env["DB"], orgId: string, input: {}): Promise<any> {
  const token = await getSlackToken(db, orgId);
  if (!token) return { error: "Slack not connected" };
  const data = await slackGet(token, "conversations.list", { limit: "200", types: "public_channel,private_channel" });
  if (!data) return { channels: [] };
  return {
    channels: data.channels?.map((c: any) => ({
      id: c.id, name: c.name, topic: c.topic?.value?.slice(0, 100),
      members: c.num_members, is_private: c.is_private,
    })) || [],
  };
}

// ── search_slack_users ──
async function searchUsers(db: Env["DB"], orgId: string, input: { query: string }): Promise<any> {
  const token = await getSlackToken(db, orgId);
  if (!token) return { error: "Slack not connected" };
  const data = await slackGet(token, "users.list", { limit: "100" });
  if (!data) return { users: [] };

  const query = input.query.toLowerCase();
  const matched = data.members?.filter((u: any) =>
    !u.deleted && !u.is_bot && (
      u.real_name?.toLowerCase().includes(query) ||
      u.name?.toLowerCase().includes(query) ||
      u.profile?.email?.toLowerCase().includes(query)
    )
  ) || [];

  return {
    users: matched.map((u: any) => ({
      id: u.id, name: u.real_name || u.name, email: u.profile?.email,
      title: u.profile?.title, status: u.profile?.status_text,
      is_online: u.presence === "active",
    })),
  };
}

// ── get_slack_unreads ──
async function getUnreads(db: Env["DB"], orgId: string, input: {}): Promise<any> {
  const token = await getSlackToken(db, orgId);
  if (!token) return { error: "Slack not connected" };
  // Get channels with unread messages
  const data = await slackGet(token, "conversations.list", { limit: "200", types: "public_channel,private_channel" });
  if (!data) return { channels: [] };

  const unreads = data.channels?.filter((c: any) => c.unread_count > 0)
    ?.map((c: any) => ({ name: c.name, unread: c.unread_count, mention_count: c.mention_count }))
    ?.sort((a: any, b: any) => b.unread - a.unread) || [];

  return { channels: unreads, total_unread: unreads.reduce((s: number, c: any) => s + c.unread, 0) };
}

// ── find_unanswered_questions (KILLER FEATURE) ──
async function findUnanswered(db: Env["DB"], orgId: string, input: { channel?: string; hours?: number }): Promise<any> {
  const token = await getSlackToken(db, orgId);
  if (!token) return { error: "Slack not connected" };

  const hours = input.hours || 24;
  const since = Math.floor(Date.now() / 1000) - hours * 3600;

  // Get channels to scan
  let channels: any[] = [];
  if (input.channel) {
    channels = [{ id: input.channel, name: input.channel }];
  } else {
    const data = await slackGet(token, "conversations.list", { limit: "50", types: "public_channel" });
    channels = data?.channels?.filter((c: any) => c.num_members > 2)?.slice(0, 20) || [];
  }

  const unanswered: any[] = [];

  for (const channel of channels) {
    const history = await slackGet(token, "conversations.history", {
      channel: channel.id, oldest: String(since), limit: "50",
    });
    if (!history?.messages) continue;

    for (const msg of history.messages) {
      // Detect questions: ends with ?, or contains question words
      const isQuestion = msg.text?.includes("?") ||
        /\b(how|what|why|where|when|who|does anyone|can someone|help|anyone know)\b/i.test(msg.text || "");

      // No replies = unanswered
      const hasReplies = (msg.reply_count || 0) > 0;

      if (isQuestion && !hasReplies && !msg.bot_id) {
        unanswered.push({
          channel: channel.name,
          text: msg.text?.slice(0, 200),
          user: msg.user,
          timestamp: msg.ts,
          hours_ago: Math.round((Date.now() / 1000 - parseFloat(msg.ts)) / 3600),
        });
      }
    }
  }

  // Graph enrichment: match question content to code owners
  const project = await db.prepare(
    "SELECT id FROM projects WHERE org_id = ?1 ORDER BY updated_at DESC LIMIT 1"
  ).bind(orgId).first<{ id: string }>();

  if (project) {
    for (const q of unanswered) {
      const words = (q.text || "").split(/\s+/).filter((w: string) => w.length > 5).slice(0, 3);
      for (const word of words) {
        const node = await db.prepare(
          "SELECT name, file_path FROM graph_nodes WHERE project_id = ?1 AND name LIKE ?2 AND type = 'function' LIMIT 1"
        ).bind(project.id, `%${word}%`).first<any>();
        if (node) {
          q.related_function = node.name;
          q.related_file = node.file_path;
          break;
        }
      }
    }
  }

  unanswered.sort((a, b) => b.hours_ago - a.hours_ago);

  return {
    unanswered,
    count: unanswered.length,
    channels_scanned: channels.length,
    hours_searched: hours,
  };
}

// ── Dispatcher ──
export const SLACK_TOOL_NAMES = [
  "search_slack_messages", "get_slack_history", "get_slack_thread",
  "post_slack_message", "list_slack_channels", "search_slack_users",
  "get_slack_unreads", "find_unanswered_questions",
];

export async function executeSlackTool(
  db: Env["DB"], orgId: string, tool: string, input: Record<string, unknown>
): Promise<any> {
  switch (tool) {
    case "search_slack_messages": return searchMessages(db, orgId, input as any);
    case "get_slack_history": return getHistory(db, orgId, input as any);
    case "get_slack_thread": return getThreadReplies(db, orgId, input as any);
    case "post_slack_message": return postMessage(db, orgId, input as any);
    case "list_slack_channels": return listChannels(db, orgId, input as any);
    case "search_slack_users": return searchUsers(db, orgId, input as any);
    case "get_slack_unreads": return getUnreads(db, orgId, input as any);
    case "find_unanswered_questions": return findUnanswered(db, orgId, input as any);
    default: return { error: `Unknown slack tool: ${tool}` };
  }
}

import type { Env, User, Org, Membership, ApiKey, AgentKey, UsageEvent, Integration } from "../lib/types";

// ─── Users ───────────────────────────────────────────────────────────────────

export async function upsertUser(
  db: D1Database,
  user: { id: string; email: string; name: string; avatar_url: string | null; provider: string; provider_id: string }
): Promise<User> {
  const now = Math.floor(Date.now() / 1000);
  await db
    .prepare(
      `INSERT INTO users (id, email, name, avatar_url, provider, provider_id, created_at, updated_at)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
       ON CONFLICT(email) DO UPDATE SET
         name = excluded.name,
         avatar_url = excluded.avatar_url,
         updated_at = ?7`
    )
    .bind(user.id, user.email, user.name, user.avatar_url, user.provider, user.provider_id, now)
    .run();

  const row = await db.prepare("SELECT * FROM users WHERE email = ?1").bind(user.email).first<User>();
  return row!;
}

export async function getUserById(db: D1Database, id: string): Promise<User | null> {
  return db.prepare("SELECT * FROM users WHERE id = ?1").bind(id).first<User>();
}

export async function getUserByEmail(db: D1Database, email: string): Promise<User | null> {
  return db.prepare("SELECT * FROM users WHERE email = ?1").bind(email).first<User>();
}

// ─── Orgs ────────────────────────────────────────────────────────────────────

export async function createOrg(
  db: D1Database,
  org: { id: string; name: string; slug: string }
): Promise<Org> {
  const now = Math.floor(Date.now() / 1000);
  await db
    .prepare(
      `INSERT INTO orgs (id, name, slug, plan, created_at, updated_at)
       VALUES (?1, ?2, ?3, 'free', ?4, ?4)`
    )
    .bind(org.id, org.name, org.slug, now)
    .run();
  return db.prepare("SELECT * FROM orgs WHERE id = ?1").bind(org.id).first<Org>() as Promise<Org>;
}

export async function getOrgById(db: D1Database, id: string): Promise<Org | null> {
  return db.prepare("SELECT * FROM orgs WHERE id = ?1").bind(id).first<Org>();
}

export async function updateOrgPlan(
  db: D1Database,
  orgId: string,
  plan: string,
  stripeCustomerId: string | null,
  stripeSubscriptionId: string | null
): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  await db
    .prepare(
      `UPDATE orgs SET plan = ?1, stripe_customer_id = ?2, stripe_subscription_id = ?3, updated_at = ?4 WHERE id = ?5`
    )
    .bind(plan, stripeCustomerId, stripeSubscriptionId, now, orgId)
    .run();
}

export async function getOrgByStripeCustomer(db: D1Database, customerId: string): Promise<Org | null> {
  return db.prepare("SELECT * FROM orgs WHERE stripe_customer_id = ?1").bind(customerId).first<Org>();
}

export async function getOrgByStripeSubscription(db: D1Database, subscriptionId: string): Promise<Org | null> {
  return db.prepare("SELECT * FROM orgs WHERE stripe_subscription_id = ?1").bind(subscriptionId).first<Org>();
}

// ─── Memberships ─────────────────────────────────────────────────────────────

export async function addMembership(
  db: D1Database,
  membership: { id: string; userId: string; orgId: string; role: string }
): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  await db
    .prepare(
      `INSERT INTO memberships (id, user_id, org_id, role, created_at)
       VALUES (?1, ?2, ?3, ?4, ?5)
       ON CONFLICT(user_id, org_id) DO NOTHING`
    )
    .bind(membership.id, membership.userId, membership.orgId, membership.role, now)
    .run();
}

export async function getOrgMembers(
  db: D1Database,
  orgId: string
): Promise<Array<{ id: string; user_id: string; email: string; name: string; role: string; created_at: number }>> {
  const result = await db
    .prepare(
      `SELECT m.id, m.user_id, u.email, u.name, m.role, m.created_at
       FROM memberships m
       JOIN users u ON u.id = m.user_id
       WHERE m.org_id = ?1
       ORDER BY m.created_at ASC`
    )
    .bind(orgId)
    .all();
  return result.results as Array<{ id: string; user_id: string; email: string; name: string; role: string; created_at: number }>;
}

export async function getUserOrgMembership(
  db: D1Database,
  userId: string
): Promise<Membership | null> {
  return db
    .prepare("SELECT * FROM memberships WHERE user_id = ?1 ORDER BY created_at ASC LIMIT 1")
    .bind(userId)
    .first<Membership>();
}

// ─── API Keys ────────────────────────────────────────────────────────────────

export async function createApiKey(
  db: D1Database,
  key: { id: string; orgId: string; name: string; prefix: string; keyHash: string; scopes: string; createdBy: string }
): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  await db
    .prepare(
      `INSERT INTO api_keys (id, org_id, name, prefix, key_hash, scopes, created_by, created_at)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)`
    )
    .bind(key.id, key.orgId, key.name, key.prefix, key.keyHash, key.scopes, key.createdBy, now)
    .run();
}

export async function listApiKeys(
  db: D1Database,
  orgId: string
): Promise<Array<{ id: string; name: string; prefix: string; scopes: string; last_used_at: number | null; created_at: number }>> {
  const result = await db
    .prepare("SELECT id, name, prefix, scopes, last_used_at, created_at FROM api_keys WHERE org_id = ?1 ORDER BY created_at DESC")
    .bind(orgId)
    .all();
  return result.results as Array<{ id: string; name: string; prefix: string; scopes: string; last_used_at: number | null; created_at: number }>;
}

export async function getApiKeyByPrefix(
  db: D1Database,
  prefix: string
): Promise<ApiKey | null> {
  return db.prepare("SELECT * FROM api_keys WHERE prefix = ?1").bind(prefix).first<ApiKey>();
}

export async function deleteApiKey(db: D1Database, id: string, orgId: string): Promise<boolean> {
  const result = await db.prepare("DELETE FROM api_keys WHERE id = ?1 AND org_id = ?2").bind(id, orgId).run();
  return (result.meta?.changes ?? 0) > 0;
}

export async function touchApiKeyLastUsed(db: D1Database, id: string): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  await db.prepare("UPDATE api_keys SET last_used_at = ?1 WHERE id = ?2").bind(now, id).run();
}

// ─── Agent Keys ──────────────────────────────────────────────────────────────

export async function getAgentKeyByPrefix(
  db: D1Database,
  prefix: string
): Promise<AgentKey | null> {
  return db.prepare("SELECT * FROM agent_keys WHERE prefix = ?1").bind(prefix).first<AgentKey>();
}

export async function touchAgentKeyLastUsed(db: D1Database, id: string): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  await db.prepare("UPDATE agent_keys SET last_used_at = ?1 WHERE id = ?2").bind(now, id).run();
}

// ─── Graph Scopes ────────────────────────────────────────────────────────────

export async function listGraphScopes(
  db: D1Database,
  orgId: string
): Promise<Array<{ id: string; graph_name: string; source_type: string; source_url: string | null; created_at: number }>> {
  const result = await db
    .prepare("SELECT id, graph_name, source_type, source_url, created_at FROM graph_scopes WHERE org_id = ?1 ORDER BY created_at DESC")
    .bind(orgId)
    .all();
  return result.results as Array<{ id: string; graph_name: string; source_type: string; source_url: string | null; created_at: number }>;
}

// ─── Usage Events ────────────────────────────────────────────────────────────

export async function logUsageEvent(
  db: D1Database,
  event: { id: string; orgId: string; userId: string | null; toolName: string; graphScopeId: string | null; tokensIn: number; tokensOut: number; durationMs: number }
): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  await db
    .prepare(
      `INSERT INTO usage_events (id, org_id, user_id, tool_name, graph_scope_id, tokens_in, tokens_out, duration_ms, created_at)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)`
    )
    .bind(event.id, event.orgId, event.userId, event.toolName, event.graphScopeId, event.tokensIn, event.tokensOut, event.durationMs, now)
    .run();
}

export async function getMonthlyUsage(
  db: D1Database,
  orgId: string
): Promise<Array<{ tool_name: string; call_count: number; total_tokens_in: number; total_tokens_out: number; total_duration_ms: number }>> {
  const now = new Date();
  const startOfMonth = Math.floor(new Date(now.getFullYear(), now.getMonth(), 1).getTime() / 1000);
  const result = await db
    .prepare(
      `SELECT tool_name,
              COUNT(*) AS call_count,
              SUM(tokens_in) AS total_tokens_in,
              SUM(tokens_out) AS total_tokens_out,
              SUM(duration_ms) AS total_duration_ms
       FROM usage_events
       WHERE org_id = ?1 AND created_at >= ?2
       GROUP BY tool_name`
    )
    .bind(orgId, startOfMonth)
    .all();
  return result.results as Array<{ tool_name: string; call_count: number; total_tokens_in: number; total_tokens_out: number; total_duration_ms: number }>;
}

export async function getMonthlyToolCallCount(
  db: D1Database,
  orgId: string
): Promise<number> {
  const now = new Date();
  const startOfMonth = Math.floor(new Date(now.getFullYear(), now.getMonth(), 1).getTime() / 1000);
  const row = await db
    .prepare("SELECT COUNT(*) AS cnt FROM usage_events WHERE org_id = ?1 AND created_at >= ?2")
    .bind(orgId, startOfMonth)
    .first<{ cnt: number }>();
  return row?.cnt ?? 0;
}

// ─── Integrations ────────────────────────────────────────────────────────────

export async function getIntegration(
  db: D1Database,
  orgId: string,
  type: string
): Promise<Integration | null> {
  return db
    .prepare("SELECT * FROM integrations WHERE org_id = ?1 AND type = ?2")
    .bind(orgId, type)
    .first<Integration>();
}

export async function getIntegrationsByType(
  db: D1Database,
  type: string
): Promise<Integration[]> {
  const result = await db
    .prepare("SELECT * FROM integrations WHERE type = ?1 AND enabled = 1")
    .bind(type)
    .all();
  return result.results as unknown as Integration[];
}

export async function listIntegrations(
  db: D1Database,
  orgId: string
): Promise<Integration[]> {
  const result = await db
    .prepare("SELECT * FROM integrations WHERE org_id = ?1 ORDER BY created_at DESC")
    .bind(orgId)
    .all();
  return result.results as unknown as Integration[];
}

export async function upsertIntegration(
  db: D1Database,
  integration: { id: string; orgId: string; type: string; config: string; credentials: string }
): Promise<Integration> {
  const now = Math.floor(Date.now() / 1000);
  await db
    .prepare(
      `INSERT INTO integrations (id, org_id, type, config, credentials, enabled, created_at, updated_at)
       VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)
       ON CONFLICT(org_id, type) DO UPDATE SET
         config = excluded.config,
         credentials = excluded.credentials,
         enabled = 1,
         updated_at = ?6`
    )
    .bind(integration.id, integration.orgId, integration.type, integration.config, integration.credentials, now)
    .run();

  const row = await db
    .prepare("SELECT * FROM integrations WHERE org_id = ?1 AND type = ?2")
    .bind(integration.orgId, integration.type)
    .first<Integration>();
  return row!;
}

export async function deleteIntegration(
  db: D1Database,
  orgId: string,
  type: string
): Promise<void> {
  await db
    .prepare("DELETE FROM integrations WHERE org_id = ?1 AND type = ?2")
    .bind(orgId, type)
    .run();
}

// ─── Billing Events ─────────────────────────────────────────────────────────

export async function logBillingEvent(
  db: D1Database,
  event: { id: string; orgId: string; eventType: string; stripeEventId: string | null; amountCents: number | null; currency: string | null; metadata: string | null }
): Promise<void> {
  const now = Math.floor(Date.now() / 1000);
  await db
    .prepare(
      `INSERT INTO billing_events (id, org_id, event_type, stripe_event_id, amount_cents, currency, metadata, created_at)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
       ON CONFLICT(stripe_event_id) DO NOTHING`
    )
    .bind(event.id, event.orgId, event.eventType, event.stripeEventId, event.amountCents, event.currency, event.metadata, now)
    .run();
}

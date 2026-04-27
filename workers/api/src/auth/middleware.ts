import { Context, Next } from "hono";
import type { Env, AuthContext } from "../lib/types";
import { verifyJwt } from "./jwt";
import { verifyKeyHash } from "../lib/crypto";
import { getApiKeyByPrefix, getAgentKeyByPrefix, touchApiKeyLastUsed, touchAgentKeyLastUsed, getUserOrgMembership } from "../db/queries";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

export function authMiddleware() {
  return async (c: Context<HonoEnv>, next: Next) => {
    const authHeader = c.req.header("Authorization");
    if (!authHeader) {
      return c.json({ error: "unauthorized", message: "Missing Authorization header", status: 401 }, 401);
    }

    const token = authHeader.startsWith("Bearer ")
      ? authHeader.slice(7).trim()
      : authHeader.trim();

    if (!token) {
      return c.json({ error: "unauthorized", message: "Empty token", status: 401 }, 401);
    }

    let auth: AuthContext | null = null;

    if (token.startsWith("ey")) {
      auth = await resolveJwt(token, c.env.JWT_SECRET);
    } else if (token.startsWith("sk_live_")) {
      auth = await resolveApiKey(token, c.env.DB);
    } else if (token.startsWith("svt_agent_")) {
      auth = await resolveAgentKey(token, c.env.DB);
    }

    if (!auth) {
      return c.json({ error: "unauthorized", message: "Invalid or expired token", status: 401 }, 401);
    }

    c.set("auth", auth);
    await next();
  };
}

async function resolveJwt(token: string, secret: string): Promise<AuthContext | null> {
  const payload = await verifyJwt(token, secret);
  if (!payload) return null;
  return { userId: payload.sub, orgId: payload.org };
}

async function resolveApiKey(rawKey: string, db: D1Database): Promise<AuthContext | null> {
  const prefix = rawKey.substring(0, 20);
  const keyRecord = await getApiKeyByPrefix(db, prefix);
  if (!keyRecord) return null;

  const valid = await verifyKeyHash(rawKey, keyRecord.key_hash);
  if (!valid) return null;

  await touchApiKeyLastUsed(db, keyRecord.id);

  const membership = await getUserOrgMembership(db, keyRecord.created_by);
  if (!membership) return null;

  return { userId: keyRecord.created_by, orgId: keyRecord.org_id };
}

async function resolveAgentKey(rawKey: string, db: D1Database): Promise<AuthContext | null> {
  const prefix = rawKey.substring(0, 22);
  const keyRecord = await getAgentKeyByPrefix(db, prefix);
  if (!keyRecord) return null;

  const valid = await verifyKeyHash(rawKey, keyRecord.key_hash);
  if (!valid) return null;

  await touchAgentKeyLastUsed(db, keyRecord.id);

  return { userId: keyRecord.created_by, orgId: keyRecord.org_id };
}

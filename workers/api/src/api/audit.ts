import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const auditApi = new Hono<HonoEnv>();

// GET /api/v1/audit - Query audit log (admin only)
auditApi.get("/", async (c) => {
  const auth = c.get("auth");
  const action = c.req.query("action");
  const actor = c.req.query("actor_id");
  const resource = c.req.query("resource_type");
  const limit = Math.min(parseInt(c.req.query("limit") || "50"), 200);
  const offset = parseInt(c.req.query("offset") || "0");

  let sql = "SELECT * FROM audit_log WHERE org_id = ?1";
  const params: unknown[] = [auth.orgId];
  let paramIdx = 2;

  if (action) {
    sql += ` AND action = ?${paramIdx}`;
    params.push(action);
    paramIdx++;
  }

  if (actor) {
    sql += ` AND actor_id = ?${paramIdx}`;
    params.push(actor);
    paramIdx++;
  }

  if (resource) {
    sql += ` AND resource_type = ?${paramIdx}`;
    params.push(resource);
    paramIdx++;
  }

  sql += ` ORDER BY created_at DESC LIMIT ?${paramIdx} OFFSET ?${paramIdx + 1}`;
  params.push(limit, offset);

  const result = await c.env.DB.prepare(sql).bind(...params).all();

  const countResult = await c.env.DB
    .prepare("SELECT COUNT(*) as total FROM audit_log WHERE org_id = ?1")
    .bind(auth.orgId)
    .first<{ total: number }>();

  return c.json({
    entries: (result.results as unknown as any[]).map((r) => ({
      ...r,
      metadata: JSON.parse(r.metadata || "{}"),
    })),
    total: countResult?.total ?? 0,
    limit,
    offset,
  });
});

// GET /api/v1/audit/export - Export audit log as CSV (for compliance)
auditApi.get("/export", async (c) => {
  const auth = c.get("auth");
  const since = c.req.query("since"); // unix timestamp

  let sql = "SELECT * FROM audit_log WHERE org_id = ?1";
  const params: unknown[] = [auth.orgId];

  if (since) {
    sql += " AND created_at >= ?2";
    params.push(parseInt(since));
  }

  sql += " ORDER BY created_at ASC";

  const result = await c.env.DB.prepare(sql).bind(...params).all();
  const rows = result.results as unknown as any[];

  let csv = "timestamp,action,actor_id,actor_email,resource_type,resource_id,ip_address,user_agent,metadata\n";
  for (const r of rows) {
    const ts = new Date(r.created_at * 1000).toISOString();
    const meta = (r.metadata || "{}").replace(/"/g, '""');
    csv += `${ts},"${r.action}","${r.actor_id}","${r.actor_email || ""}","${r.resource_type || ""}","${r.resource_id || ""}","${r.ip_address || ""}","${(r.user_agent || "").replace(/"/g, "")}","${meta}"\n`;
  }

  return new Response(csv, {
    headers: {
      "Content-Type": "text/csv",
      "Content-Disposition": `attachment; filename="savants-audit-log-${auth.orgId}.csv"`,
    },
  });
});

export default auditApi;

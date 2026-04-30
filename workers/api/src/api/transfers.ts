import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";
import { audit, requestMeta } from "../lib/audit";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const transfers = new Hono<HonoEnv>();

const TRANSFER_EXPIRY_HOURS = 72;

// POST /api/v1/transfers - Initiate a project transfer
// Source org owner sends project to target org
transfers.post("/", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{
    project_id: string;
    target_org_slug: string;
    note?: string;
  }>();

  if (!body.project_id || !body.target_org_slug) {
    return c.json({ error: "project_id and target_org_slug required", status: 400 }, 400);
  }

  // Verify caller owns the source org
  const membership = await c.env.DB
    .prepare("SELECT role FROM memberships WHERE user_id = ?1 AND org_id = ?2")
    .bind(auth.userId, auth.orgId)
    .first<{ role: string }>();

  if (!membership || membership.role !== "owner") {
    return c.json({ error: "forbidden", message: "Only org owners can initiate transfers", status: 403 }, 403);
  }

  // Verify project belongs to source org
  const project = await c.env.DB
    .prepare("SELECT id, name FROM projects WHERE id = ?1 AND org_id = ?2")
    .bind(body.project_id, auth.orgId)
    .first<{ id: string; name: string }>();

  if (!project) {
    return c.json({ error: "project_not_found", status: 404 }, 404);
  }

  // Find target org
  const targetOrg = await c.env.DB
    .prepare("SELECT id, name FROM orgs WHERE slug = ?1")
    .bind(body.target_org_slug)
    .first<{ id: string; name: string }>();

  if (!targetOrg) {
    return c.json({ error: "target_org_not_found", message: "No org with slug: " + body.target_org_slug, status: 404 }, 404);
  }

  if (targetOrg.id === auth.orgId) {
    return c.json({ error: "same_org", message: "Cannot transfer to the same org", status: 400 }, 400);
  }

  // Check for existing pending transfer
  const existing = await c.env.DB
    .prepare("SELECT id FROM transfer_requests WHERE project_id = ?1 AND status = 'pending'")
    .bind(body.project_id)
    .first();

  if (existing) {
    return c.json({ error: "transfer_pending", message: "A transfer is already pending for this project", status: 409 }, 409);
  }

  const id = crypto.randomUUID();
  const expiresAt = Math.floor(Date.now() / 1000) + TRANSFER_EXPIRY_HOURS * 3600;

  await c.env.DB
    .prepare(
      `INSERT INTO transfer_requests (id, project_id, source_org_id, target_org_id, initiated_by, note, expires_at)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)`
    )
    .bind(id, body.project_id, auth.orgId, targetOrg.id, auth.userId, body.note || null, expiresAt)
    .run();

  const meta = requestMeta(c.req.raw);
  await audit(c.env.DB, {
    orgId: auth.orgId, actorId: auth.userId,
    action: "transfer.initiate", resourceType: "project", resourceId: body.project_id,
    metadata: { target_org: targetOrg.name, project: project.name },
    ...meta,
  });

  return c.json({
    id,
    project: project.name,
    source_org: auth.orgId,
    target_org: targetOrg.name,
    target_org_slug: body.target_org_slug,
    status: "pending",
    expires_in_hours: TRANSFER_EXPIRY_HOURS,
    message: `Transfer initiated. The owner of "${targetOrg.name}" must accept within ${TRANSFER_EXPIRY_HOURS} hours.`,
  }, 201);
});

// GET /api/v1/transfers - List pending transfers (incoming + outgoing)
transfers.get("/", async (c) => {
  const auth = c.get("auth");

  const result = await c.env.DB
    .prepare(`
      SELECT t.*, p.name as project_name,
        so.name as source_org_name, so.slug as source_org_slug,
        to2.name as target_org_name, to2.slug as target_org_slug,
        u.name as initiated_by_name
      FROM transfer_requests t
      JOIN projects p ON p.id = t.project_id
      JOIN orgs so ON so.id = t.source_org_id
      JOIN orgs to2 ON to2.id = t.target_org_id
      JOIN users u ON u.id = t.initiated_by
      WHERE (t.source_org_id = ?1 OR t.target_org_id = ?1)
        AND t.status = 'pending'
        AND t.expires_at > unixepoch()
      ORDER BY t.created_at DESC
    `)
    .bind(auth.orgId)
    .all();

  return c.json({ transfers: result.results });
});

// POST /api/v1/transfers/:id/accept - Accept a transfer (target org owner)
transfers.post("/:id/accept", async (c) => {
  const auth = c.get("auth");
  const transferId = c.req.param("id");

  const transfer = await c.env.DB
    .prepare("SELECT * FROM transfer_requests WHERE id = ?1 AND status = 'pending'")
    .bind(transferId)
    .first<any>();

  if (!transfer) {
    return c.json({ error: "not_found", status: 404 }, 404);
  }

  if (transfer.target_org_id !== auth.orgId) {
    return c.json({ error: "forbidden", message: "Only target org owner can accept", status: 403 }, 403);
  }

  if (transfer.expires_at < Math.floor(Date.now() / 1000)) {
    await c.env.DB.prepare("UPDATE transfer_requests SET status = 'expired' WHERE id = ?1").bind(transferId).run();
    return c.json({ error: "expired", message: "Transfer expired", status: 410 }, 410);
  }

  // Verify caller is owner of target org
  const membership = await c.env.DB
    .prepare("SELECT role FROM memberships WHERE user_id = ?1 AND org_id = ?2")
    .bind(auth.userId, auth.orgId)
    .first<{ role: string }>();

  if (!membership || membership.role !== "owner") {
    return c.json({ error: "forbidden", message: "Only org owners can accept transfers", status: 403 }, 403);
  }

  const now = Math.floor(Date.now() / 1000);

  // Move the project to the target org
  await c.env.DB
    .prepare("UPDATE projects SET org_id = ?1, updated_at = ?2 WHERE id = ?3")
    .bind(transfer.target_org_id, now, transfer.project_id)
    .run();

  // Mark transfer as completed
  await c.env.DB
    .prepare("UPDATE transfer_requests SET status = 'completed', accepted_by = ?1, completed_at = ?2 WHERE id = ?3")
    .bind(auth.userId, now, transferId)
    .run();

  // Audit on both orgs
  const meta = requestMeta(c.req.raw);
  await audit(c.env.DB, {
    orgId: transfer.source_org_id, actorId: auth.userId,
    action: "transfer.completed_source", resourceType: "project", resourceId: transfer.project_id,
    metadata: { target_org: transfer.target_org_id, accepted_by: auth.userId },
    ...meta,
  });
  await audit(c.env.DB, {
    orgId: transfer.target_org_id, actorId: auth.userId,
    action: "transfer.completed_target", resourceType: "project", resourceId: transfer.project_id,
    metadata: { source_org: transfer.source_org_id },
    ...meta,
  });

  return c.json({ status: "completed", project_id: transfer.project_id, message: "Project transferred successfully" });
});

// POST /api/v1/transfers/:id/reject - Reject a transfer
transfers.post("/:id/reject", async (c) => {
  const auth = c.get("auth");
  const transferId = c.req.param("id");

  const transfer = await c.env.DB
    .prepare("SELECT * FROM transfer_requests WHERE id = ?1 AND status = 'pending'")
    .bind(transferId)
    .first<any>();

  if (!transfer) {
    return c.json({ error: "not_found", status: 404 }, 404);
  }

  // Either source or target org owner can reject
  if (transfer.source_org_id !== auth.orgId && transfer.target_org_id !== auth.orgId) {
    return c.json({ error: "forbidden", status: 403 }, 403);
  }

  await c.env.DB
    .prepare("UPDATE transfer_requests SET status = 'rejected', completed_at = ?1 WHERE id = ?2")
    .bind(Math.floor(Date.now() / 1000), transferId)
    .run();

  const meta = requestMeta(c.req.raw);
  await audit(c.env.DB, {
    orgId: auth.orgId, actorId: auth.userId,
    action: "transfer.rejected", resourceType: "project", resourceId: transfer.project_id,
    ...meta,
  });

  return c.json({ status: "rejected" });
});

// ─── Auto-transfer on member removal ────────────────────────────────────────

/**
 * When an org member is removed, transfer ownership of any projects
 * they own to the org owner. Call this from the member removal handler.
 */
export async function transferOwnershipOnRemoval(
  db: D1Database,
  orgId: string,
  removedUserId: string,
  removedBy: string
): Promise<void> {
  // Find org owner
  const orgOwner = await db
    .prepare("SELECT user_id FROM memberships WHERE org_id = ?1 AND role = 'owner' LIMIT 1")
    .bind(orgId)
    .first<{ user_id: string }>();

  if (!orgOwner) return;

  // Transfer any project_members where removed user was owner
  await db
    .prepare(`
      UPDATE project_members SET user_id = ?1, role = 'owner'
      WHERE user_id = ?2 AND role = 'owner'
      AND project_id IN (SELECT id FROM projects WHERE org_id = ?3)
    `)
    .bind(orgOwner.user_id, removedUserId, orgId)
    .run();

  // Remove the user from all projects in this org
  const now = Math.floor(Date.now() / 1000);
  await db
    .prepare(`
      DELETE FROM project_members
      WHERE user_id = ?1
      AND project_id IN (SELECT id FROM projects WHERE org_id = ?2)
    `)
    .bind(removedUserId, orgId)
    .run();
}

export default transfers;

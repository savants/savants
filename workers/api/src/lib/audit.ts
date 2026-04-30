/**
 * Audit logger for SOC 2 Type II compliance.
 *
 * Logs every security-relevant action to an immutable audit trail.
 * Never throws - audit failures should not break user actions.
 *
 * Actions logged:
 *   auth.login, auth.logout, auth.device_code, auth.oauth_callback
 *   key.create, key.revoke
 *   member.invite, member.remove
 *   project.create, project.delete
 *   source.add, source.remove
 *   integration.create, integration.delete
 *   billing.checkout, billing.purchase_credits
 *   settings.update, org.delete
 *   tool.call (cloud tools only)
 */

export interface AuditEntry {
  orgId: string;
  actorId: string;
  actorEmail?: string;
  action: string;
  resourceType?: string;
  resourceId?: string;
  metadata?: Record<string, unknown>;
  ipAddress?: string;
  userAgent?: string;
}

export async function audit(db: D1Database, entry: AuditEntry): Promise<void> {
  try {
    await db
      .prepare(
        `INSERT INTO audit_log (id, org_id, actor_id, actor_email, action, resource_type, resource_id, metadata, ip_address, user_agent)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)`
      )
      .bind(
        crypto.randomUUID(),
        entry.orgId,
        entry.actorId,
        entry.actorEmail || null,
        entry.action,
        entry.resourceType || null,
        entry.resourceId || null,
        JSON.stringify(entry.metadata || {}),
        entry.ipAddress || null,
        entry.userAgent || null
      )
      .run();
  } catch {
    // Never throw from audit logging - don't break user actions
    console.error(`[audit] Failed to log: ${entry.action} by ${entry.actorId}`);
  }
}

/**
 * Extract IP and user agent from a Hono context for audit logging.
 */
export function requestMeta(req: Request): { ipAddress: string; userAgent: string } {
  return {
    ipAddress: req.headers.get("cf-connecting-ip") || req.headers.get("x-forwarded-for") || "",
    userAgent: (req.headers.get("user-agent") || "").slice(0, 200),
  };
}

-- Migration 008: Audit log for SOC 2 Type II compliance
-- Immutable trail of all security-relevant actions.
-- Never deleted. Retained indefinitely for compliance.

CREATE TABLE IF NOT EXISTS audit_log (
  id TEXT PRIMARY KEY,
  org_id TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  actor_email TEXT,
  action TEXT NOT NULL,
  resource_type TEXT,
  resource_id TEXT,
  metadata TEXT DEFAULT '{}',
  ip_address TEXT,
  user_agent TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_audit_org ON audit_log(org_id, created_at);
CREATE INDEX IF NOT EXISTS idx_audit_actor ON audit_log(actor_id, created_at);
CREATE INDEX IF NOT EXISTS idx_audit_action ON audit_log(action);
CREATE INDEX IF NOT EXISTS idx_audit_resource ON audit_log(resource_type, resource_id);

-- Data retention configuration per org
CREATE TABLE IF NOT EXISTS data_retention_policies (
  org_id TEXT PRIMARY KEY REFERENCES orgs(id) ON DELETE CASCADE,
  usage_events_days INTEGER NOT NULL DEFAULT 90,
  credit_transactions_days INTEGER NOT NULL DEFAULT 365,
  graph_events_days INTEGER NOT NULL DEFAULT 90,
  audit_log_days INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

INSERT INTO _migrations (name) VALUES ('008_audit_log') ON CONFLICT DO NOTHING;

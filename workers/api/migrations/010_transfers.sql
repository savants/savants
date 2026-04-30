-- Migration 010: Project transfers + ownership handoff
-- Supports: employee → company, personal → org, org → org

CREATE TABLE IF NOT EXISTS transfer_requests (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  source_org_id TEXT NOT NULL REFERENCES orgs(id),
  target_org_id TEXT NOT NULL REFERENCES orgs(id),
  initiated_by TEXT NOT NULL REFERENCES users(id),
  status TEXT NOT NULL DEFAULT 'pending',
  accepted_by TEXT REFERENCES users(id),
  note TEXT,
  expires_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  completed_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_transfers_source ON transfer_requests(source_org_id, status);
CREATE INDEX IF NOT EXISTS idx_transfers_target ON transfer_requests(target_org_id, status);

-- Track when members are removed (for compliance + re-onboarding)
ALTER TABLE memberships ADD COLUMN removed_at INTEGER;
ALTER TABLE memberships ADD COLUMN removed_by TEXT;

-- Track when project members are removed
ALTER TABLE project_members ADD COLUMN removed_at INTEGER;
ALTER TABLE project_members ADD COLUMN removed_by TEXT;

INSERT INTO _migrations (name) VALUES ('010_transfers') ON CONFLICT DO NOTHING;

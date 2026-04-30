-- Migration 002: Integrations table
-- Applied: 2026-04-29

CREATE TABLE IF NOT EXISTS integrations (
  id TEXT PRIMARY KEY,
  org_id TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
  type TEXT NOT NULL,
  config TEXT NOT NULL DEFAULT '{}',
  credentials TEXT NOT NULL DEFAULT '{}',
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE(org_id, type)
);
CREATE INDEX IF NOT EXISTS idx_integrations_org ON integrations(org_id);
CREATE INDEX IF NOT EXISTS idx_integrations_type ON integrations(type);

INSERT INTO _migrations (name) VALUES ('002_integrations') ON CONFLICT DO NOTHING;

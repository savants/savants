-- Migration 003: Credits billing system
-- Applied: 2026-04-30

CREATE TABLE IF NOT EXISTS credit_balances (
  org_id TEXT PRIMARY KEY REFERENCES orgs(id) ON DELETE CASCADE,
  balance INTEGER NOT NULL DEFAULT 0,
  auto_topup_enabled INTEGER NOT NULL DEFAULT 0,
  auto_topup_package TEXT DEFAULT 'starter',
  updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS credit_transactions (
  id TEXT PRIMARY KEY,
  org_id TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
  type TEXT NOT NULL,
  amount INTEGER NOT NULL,
  balance_after INTEGER NOT NULL,
  description TEXT,
  stripe_payment_id TEXT,
  tool_name TEXT,
  project_id TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_credit_tx_org ON credit_transactions(org_id, created_at);
CREATE INDEX IF NOT EXISTS idx_credit_tx_type ON credit_transactions(type);

INSERT INTO _migrations (name) VALUES ('003_credits') ON CONFLICT DO NOTHING;

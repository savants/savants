-- Migration 009: Billing contacts and cost centers
-- Supports: company pays, individual pays, multi-cost-center enterprise

-- Billing contact (separate from org owner)
ALTER TABLE orgs ADD COLUMN billing_email TEXT;
ALTER TABLE orgs ADD COLUMN billing_name TEXT;
ALTER TABLE orgs ADD COLUMN billing_address TEXT;
ALTER TABLE orgs ADD COLUMN tax_id TEXT;

-- Cost center per project (for enterprise invoice splitting)
ALTER TABLE projects ADD COLUMN cost_center TEXT;
ALTER TABLE projects ADD COLUMN budget_credits INTEGER;

-- Track which project each credit charge belongs to
-- (project_id already added in migration 003 on credit_transactions)

INSERT INTO _migrations (name) VALUES ('009_billing_contacts') ON CONFLICT DO NOTHING;

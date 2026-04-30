-- Migration 006: Add indexes that were created manually but missing from migrations
-- This brings migrations in sync with production

CREATE INDEX IF NOT EXISTS idx_orgs_stripe_customer ON orgs(stripe_customer_id);
CREATE INDEX IF NOT EXISTS idx_device_auth_user_code ON device_auth_sessions(user_code);
CREATE INDEX IF NOT EXISTS idx_device_auth_status ON device_auth_sessions(status);
CREATE INDEX IF NOT EXISTS idx_billing_events_type ON billing_events(event_type);
CREATE INDEX IF NOT EXISTS idx_usage_events_created ON usage_events(created_at);
CREATE INDEX IF NOT EXISTS idx_usage_events_tool ON usage_events(tool_name);

INSERT INTO _migrations (name) VALUES ('006_missing_indexes') ON CONFLICT DO NOTHING;

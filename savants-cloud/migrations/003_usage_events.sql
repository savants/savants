-- Usage event metering for PAYG billing
CREATE TABLE IF NOT EXISTS usage_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID REFERENCES orgs(id) NOT NULL,
    endpoint TEXT NOT NULL,
    duration_ms INT DEFAULT 0,
    status_code INT DEFAULT 200,
    created_at TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_usage_events_org_time ON usage_events(org_id, created_at);
CREATE INDEX IF NOT EXISTS idx_usage_events_org_endpoint ON usage_events(org_id, endpoint, created_at);

-- Monthly aggregation view for billing
CREATE OR REPLACE VIEW usage_monthly AS
SELECT
    org_id,
    date_trunc('month', created_at) AS month,
    endpoint,
    count(*) AS call_count,
    avg(duration_ms) AS avg_duration_ms
FROM usage_events
GROUP BY org_id, date_trunc('month', created_at), endpoint;

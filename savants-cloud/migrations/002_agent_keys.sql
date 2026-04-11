-- Agent keys for headless remote agents (Tailscale auth key model)

CREATE TABLE agent_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID REFERENCES orgs(id) ON DELETE CASCADE NOT NULL,
    name TEXT NOT NULL,           -- human-readable: "prod-us-east", "staging-eu"
    key_hash TEXT NOT NULL,       -- bcrypt hash of the full key
    key_prefix TEXT NOT NULL,     -- first 12 chars: "svt_agent_ab" for display
    cluster_scope TEXT,           -- optional: restrict to one cluster name
    permissions TEXT[] DEFAULT '{push}',  -- 'push', 'read', 'admin'
    last_seen_at TIMESTAMPTZ,    -- last heartbeat from this agent
    last_seen_ip TEXT,           -- IP address of last heartbeat
    agent_version TEXT,          -- savants version reported by agent
    status TEXT DEFAULT 'active', -- 'active', 'revoked'
    revoked_at TIMESTAMPTZ,
    revoked_by UUID REFERENCES users(id),
    created_at TIMESTAMPTZ DEFAULT now(),
    created_by UUID REFERENCES users(id)
);

CREATE INDEX idx_agent_keys_org ON agent_keys(org_id);
CREATE INDEX idx_agent_keys_prefix ON agent_keys(key_prefix);
CREATE INDEX idx_agent_keys_status ON agent_keys(status) WHERE status = 'active';

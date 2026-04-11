-- savants.cloud initial schema

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- Users (created on first SSO login)
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email TEXT UNIQUE NOT NULL,
    name TEXT,
    avatar_url TEXT,
    auth_provider TEXT NOT NULL,  -- 'google', 'github', 'saml'
    auth_provider_id TEXT NOT NULL,
    created_at TIMESTAMPTZ DEFAULT now(),
    UNIQUE(auth_provider, auth_provider_id)
);

-- Organizations
CREATE TABLE orgs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug TEXT UNIQUE NOT NULL,
    name TEXT NOT NULL,
    plan TEXT DEFAULT 'free',  -- 'free', 'team', 'enterprise'
    stripe_customer_id TEXT,
    stripe_subscription_id TEXT,
    max_users INT DEFAULT 1,
    max_clusters INT DEFAULT 1,
    created_at TIMESTAMPTZ DEFAULT now()
);

-- Memberships
CREATE TABLE memberships (
    user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    org_id UUID REFERENCES orgs(id) ON DELETE CASCADE,
    role TEXT DEFAULT 'member',  -- 'owner', 'admin', 'member'
    invited_by UUID REFERENCES users(id),
    created_at TIMESTAMPTZ DEFAULT now(),
    PRIMARY KEY (user_id, org_id)
);

-- Graph scopes (what graphs exist per org)
CREATE TABLE graph_scopes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID REFERENCES orgs(id) ON DELETE CASCADE NOT NULL,
    scope_type TEXT NOT NULL,  -- 'code', 'k8s', 'host', 'aws'
    scope_name TEXT NOT NULL,
    falkordb_graph_name TEXT NOT NULL,
    last_delta_at TIMESTAMPTZ,
    node_count BIGINT DEFAULT 0,
    edge_count BIGINT DEFAULT 0,
    created_at TIMESTAMPTZ DEFAULT now(),
    UNIQUE(org_id, scope_type, scope_name)
);

-- Device auth sessions (RFC 8628)
CREATE TABLE device_auth_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    device_code TEXT UNIQUE NOT NULL,
    user_code TEXT UNIQUE NOT NULL,
    client_ip TEXT,
    user_id UUID REFERENCES users(id),
    org_id UUID REFERENCES orgs(id),
    status TEXT DEFAULT 'pending',  -- 'pending', 'approved', 'denied', 'expired'
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ DEFAULT now()
);

-- Refresh tokens
CREATE TABLE refresh_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID REFERENCES users(id) ON DELETE CASCADE NOT NULL,
    org_id UUID REFERENCES orgs(id) ON DELETE CASCADE NOT NULL,
    device_id TEXT,
    token_hash TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ DEFAULT now()
);

-- API keys (for CI, K8s operator, automation)
CREATE TABLE api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID REFERENCES orgs(id) ON DELETE CASCADE NOT NULL,
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL,
    key_prefix TEXT NOT NULL,  -- first 8 chars for display
    scopes TEXT[] DEFAULT '{write}',
    last_used_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT now()
);

-- Billing events (supplement to Stripe)
CREATE TABLE billing_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID REFERENCES orgs(id) ON DELETE CASCADE NOT NULL,
    event_type TEXT NOT NULL,
    stripe_event_id TEXT UNIQUE,
    payload JSONB,
    created_at TIMESTAMPTZ DEFAULT now()
);

-- SSO config (enterprise)
CREATE TABLE org_sso_configs (
    org_id UUID PRIMARY KEY REFERENCES orgs(id) ON DELETE CASCADE,
    protocol TEXT NOT NULL,  -- 'saml', 'oidc'
    metadata_url TEXT,
    client_id TEXT,
    client_secret_encrypted TEXT,
    issuer TEXT,
    created_at TIMESTAMPTZ DEFAULT now()
);

-- Indices
CREATE INDEX idx_memberships_org ON memberships(org_id);
CREATE INDEX idx_graph_scopes_org ON graph_scopes(org_id);
CREATE INDEX idx_device_auth_status ON device_auth_sessions(status) WHERE status = 'pending';
CREATE INDEX idx_device_auth_expires ON device_auth_sessions(expires_at);
CREATE INDEX idx_api_keys_org ON api_keys(org_id);
CREATE INDEX idx_billing_events_org ON billing_events(org_id);

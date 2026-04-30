export interface Env {
  DB: D1Database;
  KV: KVNamespace;
  ENVIRONMENT: string;
  JWT_SECRET: string;
  GOOGLE_CLIENT_ID: string;
  GOOGLE_CLIENT_SECRET: string;
  GITHUB_CLIENT_ID: string;
  GITHUB_CLIENT_SECRET: string;
  STRIPE_SECRET_KEY: string;
  STRIPE_WEBHOOK_SECRET: string;
  STRIPE_PRICE_ID: string;
  SLACK_BOT_TOKEN: string;
  GITHUB_APP_TOKEN: string;
  GRAPH_PROXY_URL: string;
  CF_API_TOKEN: string;
}

export interface AuthContext {
  userId: string;
  orgId: string;
}

export interface User {
  id: string;
  email: string;
  name: string;
  avatar_url: string | null;
  provider: string;
  provider_id: string;
  created_at: number;
  updated_at: number;
}

export interface Org {
  id: string;
  name: string;
  slug: string;
  plan: string;
  stripe_customer_id: string | null;
  stripe_subscription_id: string | null;
  created_at: number;
  updated_at: number;
}

export interface Membership {
  id: string;
  user_id: string;
  org_id: string;
  role: string;
  created_at: number;
}

export interface GraphScope {
  id: string;
  org_id: string;
  graph_name: string;
  source_type: string;
  source_url: string | null;
  created_at: number;
}

export interface DeviceAuthSession {
  device_code: string;
  user_code: string;
  status: "pending" | "approved" | "expired";
  user_id: string | null;
  org_id: string | null;
  expires_at: number;
}

export interface ApiKey {
  id: string;
  org_id: string;
  name: string;
  prefix: string;
  key_hash: string;
  scopes: string;
  created_by: string;
  last_used_at: number | null;
  created_at: number;
}

export interface AgentKey {
  id: string;
  org_id: string;
  name: string;
  prefix: string;
  key_hash: string;
  agent_type: string;
  created_by: string;
  last_used_at: number | null;
  created_at: number;
}

export interface BillingEvent {
  id: string;
  org_id: string;
  event_type: string;
  stripe_event_id: string | null;
  amount_cents: number | null;
  currency: string | null;
  metadata: string | null;
  created_at: number;
}

export interface UsageEvent {
  id: string;
  org_id: string;
  user_id: string | null;
  tool_name: string;
  graph_scope_id: string | null;
  tokens_in: number;
  tokens_out: number;
  duration_ms: number;
  created_at: number;
}

export interface JwtPayload {
  sub: string;
  org: string;
  email: string;
  iat: number;
  exp: number;
}

export interface ToolDefinition {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
  pricing: {
    free_monthly_calls: number | null;
    overage_per_call_cents: number;
    tier: "local" | "cloud";
  };
}

export interface Integration {
  id: string;
  org_id: string;
  type: string;
  config: string;
  credentials: string;
  enabled: number;
  created_at: number;
  updated_at: number;
}

export interface SentryConfig {
  org_slug: string;
  project_slugs?: string[];
  auto_diagnose: boolean;
  slack_channel?: string;
}

export interface SentryCredentials {
  auth_token: string;
  client_secret: string;
}

export interface ApiError {
  error: string;
  message: string;
  status: number;
}

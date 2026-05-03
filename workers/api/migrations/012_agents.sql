-- Migration 012: Agent registration and query routing
-- Agents are savants binaries running on servers/clusters that report to cloud.

CREATE TABLE IF NOT EXISTS agents (
  id TEXT PRIMARY KEY,
  org_id TEXT NOT NULL REFERENCES orgs(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  hostname TEXT,
  os TEXT,
  arch TEXT,
  capabilities TEXT DEFAULT '[]',
  last_heartbeat INTEGER,
  version TEXT,
  status TEXT DEFAULT 'online',
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_agents_org ON agents(org_id);
CREATE INDEX IF NOT EXISTS idx_agents_status ON agents(org_id, status);

-- Pending queries: cloud writes, agent polls, executes, writes result back
CREATE TABLE IF NOT EXISTS agent_queries (
  id TEXT PRIMARY KEY,
  org_id TEXT NOT NULL,
  agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
  tool TEXT NOT NULL,
  input TEXT NOT NULL DEFAULT '{}',
  status TEXT DEFAULT 'pending',
  result TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  completed_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_agent_queries_pending ON agent_queries(agent_id, status);

INSERT INTO _migrations (name) VALUES ('012_agents') ON CONFLICT DO NOTHING;

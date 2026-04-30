-- Migration 005: D1 graph engine (replaces FalkorDB)
-- Applied: 2026-04-30

CREATE TABLE IF NOT EXISTS graph_nodes (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  type TEXT NOT NULL,
  name TEXT NOT NULL,
  qualified_name TEXT,
  file_path TEXT,
  line_start INTEGER,
  line_end INTEGER,
  language TEXT,
  content_summary TEXT,
  metadata TEXT DEFAULT '{}',
  source_type TEXT NOT NULL,
  source_id TEXT,
  content_hash TEXT,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_nodes_project ON graph_nodes(project_id, type);
CREATE INDEX IF NOT EXISTS idx_nodes_name ON graph_nodes(name);
CREATE INDEX IF NOT EXISTS idx_nodes_file ON graph_nodes(project_id, file_path);
CREATE INDEX IF NOT EXISTS idx_nodes_source ON graph_nodes(source_type, source_id);

CREATE TABLE IF NOT EXISTS graph_edges (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  source_node TEXT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
  target_node TEXT NOT NULL REFERENCES graph_nodes(id) ON DELETE CASCADE,
  type TEXT NOT NULL,
  weight REAL DEFAULT 1.0,
  metadata TEXT DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_edges_project ON graph_edges(project_id);
CREATE INDEX IF NOT EXISTS idx_edges_source ON graph_edges(source_node, type);
CREATE INDEX IF NOT EXISTS idx_edges_target ON graph_edges(target_node, type);
CREATE INDEX IF NOT EXISTS idx_edges_type ON graph_edges(project_id, type);

CREATE TABLE IF NOT EXISTS graph_events (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  type TEXT NOT NULL,
  title TEXT NOT NULL,
  description TEXT,
  severity TEXT DEFAULT 'info',
  node_id TEXT REFERENCES graph_nodes(id),
  source_type TEXT NOT NULL,
  source_ref TEXT,
  metadata TEXT DEFAULT '{}',
  occurred_at INTEGER NOT NULL DEFAULT (unixepoch()),
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_events_project ON graph_events(project_id, occurred_at);
CREATE INDEX IF NOT EXISTS idx_events_node ON graph_events(node_id);
CREATE INDEX IF NOT EXISTS idx_events_type ON graph_events(project_id, type);

INSERT INTO _migrations (name) VALUES ('005_graph') ON CONFLICT DO NOTHING;

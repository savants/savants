-- Migration 007: Add graph_db_id to orgs for per-org database isolation
-- When null, the shared DB is used (single-tenant mode)
-- When set, points to a dedicated D1 database for this org's graph data

ALTER TABLE orgs ADD COLUMN graph_db_id TEXT;
ALTER TABLE orgs ADD COLUMN graph_db_region TEXT;

INSERT INTO _migrations (name) VALUES ('007_org_graph_db') ON CONFLICT DO NOTHING;

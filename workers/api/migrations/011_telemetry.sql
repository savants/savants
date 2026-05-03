-- Migration 011: Anonymous telemetry events
-- No PII, no code, no queries. Just tool name + duration + OS.

CREATE TABLE IF NOT EXISTS telemetry_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  device_id TEXT NOT NULL,
  tool TEXT NOT NULL,
  duration_ms INTEGER NOT NULL,
  os TEXT,
  arch TEXT,
  version TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_telemetry_tool ON telemetry_events(tool);
CREATE INDEX IF NOT EXISTS idx_telemetry_created ON telemetry_events(created_at);

INSERT INTO _migrations (name) VALUES ('011_telemetry') ON CONFLICT DO NOTHING;

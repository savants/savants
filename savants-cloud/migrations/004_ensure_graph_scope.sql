-- Ensure every org gets a default graph scope on creation.
-- Also add a helper function callable from app code.

CREATE OR REPLACE FUNCTION ensure_default_graph_scope(p_org_id UUID)
RETURNS TEXT AS $$
DECLARE
    v_graph_name TEXT;
BEGIN
    -- Deterministic namespace: org_ + first 8 hex chars of org UUID (no hyphens)
    v_graph_name := 'org_' || replace(p_org_id::text, '-', '');

    INSERT INTO graph_scopes (org_id, scope_type, scope_name, falkordb_graph_name)
    VALUES (p_org_id, 'code', 'default', v_graph_name)
    ON CONFLICT (org_id, scope_type, scope_name) DO NOTHING;

    RETURN v_graph_name;
END;
$$ LANGUAGE plpgsql;

-- Backfill: create a default graph scope for any org that doesn't have one yet
INSERT INTO graph_scopes (org_id, scope_type, scope_name, falkordb_graph_name)
SELECT id, 'code', 'default', 'org_' || replace(id::text, '-', '')
FROM orgs
WHERE id NOT IN (SELECT org_id FROM graph_scopes WHERE scope_type = 'code' AND scope_name = 'default')
ON CONFLICT DO NOTHING;

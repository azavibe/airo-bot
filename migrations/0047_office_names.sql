-- Offices: a person can have several workspaces ("offices"), each with its
-- own rooms. The first one was created with a placeholder name; give it a
-- plain one. Names chosen by people (office setup renames it) are kept.
UPDATE workspaces
SET name = 'My office', description = 'Your first office', updated_at = now()
WHERE name = 'Default Workspace' AND deleted_at IS NULL;

-- Rooms are listed per office.
CREATE INDEX IF NOT EXISTS threads_workspace_idx ON threads (workspace_id, created_at)
    WHERE deleted_at IS NULL;

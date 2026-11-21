-- 0055_archive_apps.sql
-- An app can be archived (hidden from the office and from its bots, and
-- restorable) or deleted (soft, like everything else). A deleted app's slug
-- can be installed again, so the slug only has to be unique among the apps
-- that are not deleted.

ALTER TABLE office_apps ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;

ALTER TABLE office_apps DROP CONSTRAINT IF EXISTS office_apps_workspace_id_slug_key;
CREATE UNIQUE INDEX IF NOT EXISTS office_apps_live_slug_idx
    ON office_apps (workspace_id, slug)
    WHERE deleted_at IS NULL;

-- 0067_drive_folders.sql
-- Google Drive folders as office documents (issue #34): the owner links a
-- folder through a Google connection that has Drive (read only); each
-- supported file in it becomes a document, kept in step once a day.

CREATE TABLE IF NOT EXISTS document_folders (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    connection_id UUID NOT NULL REFERENCES connections (id),
    drive_folder_id TEXT NOT NULL CHECK (char_length(drive_folder_id) BETWEEN 1 AND 200),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 200),
    -- When the folder was last read, and why that failed (null when it worked).
    synced_at TIMESTAMPTZ,
    sync_error TEXT CHECK (sync_error IS NULL OR char_length(sync_error) <= 500),
    sync_failures INTEGER NOT NULL DEFAULT 0 CHECK (sync_failures >= 0),
    visibility TEXT NOT NULL DEFAULT 'private'
        CHECK (visibility IN ('private', 'unlisted', 'public')),
    created_by_user_id UUID REFERENCES users (id),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS document_folders_once_idx
    ON document_folders (workspace_id, drive_folder_id)
    WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS document_folders_idempotency_idx
    ON document_folders (workspace_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

-- A document from a Drive folder: kind 'drive', the folder it came from,
-- the Drive file id, and the file's modifiedTime when last read (to skip
-- files that didn't change).
ALTER TABLE documents DROP CONSTRAINT IF EXISTS documents_kind_check;
ALTER TABLE documents ADD CONSTRAINT documents_kind_check
    CHECK (kind IN ('file', 'text', 'page', 'drive'));
ALTER TABLE documents
    ADD COLUMN IF NOT EXISTS folder_id UUID REFERENCES document_folders (id),
    ADD COLUMN IF NOT EXISTS external_id TEXT
        CHECK (external_id IS NULL OR char_length(external_id) <= 200),
    ADD COLUMN IF NOT EXISTS external_modified TEXT
        CHECK (external_modified IS NULL OR char_length(external_modified) <= 64);

CREATE UNIQUE INDEX IF NOT EXISTS documents_folder_file_idx
    ON documents (folder_id, external_id)
    WHERE folder_id IS NOT NULL AND deleted_at IS NULL;

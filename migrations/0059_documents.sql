-- 0059_documents.sql
-- What an office's bots answer from: uploaded files (PDF, Word, text),
-- pasted text such as an FAQ, and pages of the business's website. Each is
-- kept as plain text and split into chunks that Postgres full-text search
-- indexes, so a bot is shown the few passages that match the conversation.
-- A bot can be told to answer only from these (agents.documents_only).

CREATE TABLE IF NOT EXISTS documents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200),
    kind TEXT NOT NULL CHECK (kind IN ('file', 'text', 'page')),
    filename TEXT CHECK (filename IS NULL OR char_length(filename) <= 255),
    source_url TEXT CHECK (source_url IS NULL OR char_length(source_url) <= 2000),
    body TEXT NOT NULL,
    chars INTEGER NOT NULL CHECK (chars >= 0),
    visibility TEXT NOT NULL DEFAULT 'private'
        CHECK (visibility IN ('private', 'unlisted', 'public')),
    created_by_user_id UUID REFERENCES users (id),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS documents_workspace_idx
    ON documents (workspace_id, created_at DESC)
    WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS documents_idempotency_idx
    ON documents (workspace_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

-- Chunks are derived from a document's text; they follow the document's
-- deleted_at rather than having their own. `search` weights the
-- document's title above the passage itself.
CREATE TABLE IF NOT EXISTS document_chunks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    document_id UUID NOT NULL REFERENCES documents (id),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    position INTEGER NOT NULL CHECK (position >= 0),
    body TEXT NOT NULL,
    search TSVECTOR NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (document_id, position)
);

CREATE INDEX IF NOT EXISTS document_chunks_search_idx
    ON document_chunks USING GIN (search);
CREATE INDEX IF NOT EXISTS document_chunks_workspace_idx
    ON document_chunks (workspace_id);

ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS documents_only BOOLEAN NOT NULL DEFAULT false;

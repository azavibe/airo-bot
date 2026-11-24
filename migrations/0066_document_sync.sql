-- 0066_document_sync.sql
-- Keeping office documents current (issue #34): pages of the business's
-- website are re-read on a schedule, and the owner can mark any document
-- outdated (bots stop answering from it) from a bot's answer or the
-- Documents tab.

ALTER TABLE documents
    -- Set: bots no longer see or cite it; the owner can mark it current.
    ADD COLUMN IF NOT EXISTS outdated_at TIMESTAMPTZ,
    -- Pages: when the page was last read again (null: only when added).
    ADD COLUMN IF NOT EXISTS synced_at TIMESTAMPTZ,
    -- Pages: why the last re-read failed (null when it worked).
    ADD COLUMN IF NOT EXISTS sync_error TEXT
        CHECK (sync_error IS NULL OR char_length(sync_error) <= 500),
    -- Pages: failed re-reads in a row; the owner is told on the first.
    ADD COLUMN IF NOT EXISTS sync_failures INTEGER NOT NULL DEFAULT 0
        CHECK (sync_failures >= 0);

CREATE INDEX IF NOT EXISTS documents_page_sync_idx
    ON documents ((COALESCE(synced_at, created_at)))
    WHERE kind = 'page' AND deleted_at IS NULL;

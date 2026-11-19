CREATE TABLE threads (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users (id),
    title TEXT NOT NULL,
    slug TEXT NOT NULL,
    status thread_status NOT NULL DEFAULT 'open',
    visibility visibility NOT NULL DEFAULT 'private',
    archived_at TIMESTAMPTZ,
    -- Counter caches, per the standing rule against computing feed/list
    -- counts with a live count(*): kept in sync in the same transaction as
    -- each note insert (see handlers::threads).
    note_count INT NOT NULL DEFAULT 0,
    last_note_at TIMESTAMPTZ,
    -- Lets POST /threads be idempotent (the standing "every write endpoint
    -- accepts an optional idempotency_key" rule) without a dedicated table:
    -- a retried create with the same key returns the thread already made.
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX threads_user_id_idx ON threads (user_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX threads_user_id_slug_key ON threads (user_id, slug) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX threads_user_id_idempotency_key_key ON threads (user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

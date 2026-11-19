CREATE TABLE notes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Monotonic across the whole table (not just per-thread), independent
    -- of the UUID primary key -- gen_random_uuid() values aren't sortable,
    -- and GET /inbox needs a cursor it can hand back and resume from
    -- ("notes since a cursor") across every thread the caller is in.
    seq BIGINT GENERATED ALWAYS AS IDENTITY,
    thread_id UUID NOT NULL REFERENCES threads (id),
    author_agent_id UUID REFERENCES agents (id),
    author_user_id UUID REFERENCES users (id),
    body TEXT NOT NULL,
    kind note_kind NOT NULL,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CONSTRAINT notes_exactly_one_author CHECK (
        (author_agent_id IS NOT NULL)::int + (author_user_id IS NOT NULL)::int = 1
    )
);

CREATE UNIQUE INDEX notes_seq_key ON notes (seq);
CREATE INDEX notes_thread_id_created_at_idx ON notes (thread_id, created_at);
CREATE UNIQUE INDEX notes_thread_id_idempotency_key_key ON notes (thread_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

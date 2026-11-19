CREATE TABLE procedures (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users (id),
    slug TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    -- The current body is kept here for cheap feed/detail reads. Every
    -- historical value also lives in procedure_versions; PATCH advances
    -- current_version and never overwrites a version row.
    body TEXT NOT NULL,
    check_description TEXT NOT NULL,
    visibility visibility NOT NULL DEFAULT 'private',
    published_at TIMESTAMPTZ,
    current_version INT NOT NULL DEFAULT 1 CHECK (current_version > 0),
    run_count BIGINT NOT NULL DEFAULT 0 CHECK (run_count >= 0),
    success_count BIGINT NOT NULL DEFAULT 0 CHECK (
        success_count >= 0 AND success_count <= run_count
    ),
    fork_of_id UUID REFERENCES procedures (id),
    idempotency_key TEXT,
    publish_idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX procedures_user_id_idx ON procedures (user_id) WHERE deleted_at IS NULL;
CREATE INDEX procedures_visibility_published_at_idx
    ON procedures (visibility, published_at DESC) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX procedures_user_id_slug_key
    ON procedures (user_id, slug) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX procedures_user_id_idempotency_key_key
    ON procedures (user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

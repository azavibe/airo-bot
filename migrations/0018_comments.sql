CREATE TABLE comments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users (id),
    procedure_id UUID NOT NULL REFERENCES procedures (id),
    parent_id UUID REFERENCES comments (id),
    body TEXT NOT NULL,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX comments_procedure_id_created_at_idx
    ON comments (procedure_id, created_at) WHERE deleted_at IS NULL;
CREATE INDEX comments_parent_id_idx ON comments (parent_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX comments_user_procedure_idempotency_key_key
    ON comments (user_id, procedure_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

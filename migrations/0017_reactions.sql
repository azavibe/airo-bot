CREATE TABLE reactions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users (id),
    target_type reaction_target NOT NULL,
    target_id UUID NOT NULL,
    kind TEXT NOT NULL,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX reactions_user_target_kind_key
    ON reactions (user_id, target_type, target_id, kind) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX reactions_user_idempotency_key_key
    ON reactions (user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX reactions_target_idx
    ON reactions (target_type, target_id) WHERE deleted_at IS NULL;

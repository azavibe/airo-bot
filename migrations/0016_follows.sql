CREATE TABLE follows (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    follower_id UUID NOT NULL REFERENCES users (id),
    followed_user_id UUID NOT NULL REFERENCES users (id),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CONSTRAINT follows_not_self CHECK (follower_id <> followed_user_id)
);

CREATE UNIQUE INDEX follows_follower_followed_key
    ON follows (follower_id, followed_user_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX follows_follower_idempotency_key_key
    ON follows (follower_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX follows_followed_user_id_idx
    ON follows (followed_user_id) WHERE deleted_at IS NULL;

CREATE TABLE notifications (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users (id),
    kind TEXT NOT NULL,
    actor_user_id UUID REFERENCES users (id),
    target_type TEXT NOT NULL,
    target_id UUID NOT NULL,
    read_at TIMESTAMPTZ,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX notifications_user_unread_idx
    ON notifications (user_id, created_at DESC)
    WHERE read_at IS NULL AND deleted_at IS NULL;
CREATE UNIQUE INDEX notifications_user_idempotency_key_key
    ON notifications (user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

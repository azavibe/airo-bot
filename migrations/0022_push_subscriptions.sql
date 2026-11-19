CREATE TABLE push_subscriptions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users (id),
    endpoint TEXT NOT NULL,
    p256dh TEXT NOT NULL,
    auth TEXT NOT NULL,
    user_agent TEXT,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX push_subscriptions_user_endpoint_key
    ON push_subscriptions (user_id, endpoint) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX push_subscriptions_user_idempotency_key_key
    ON push_subscriptions (user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX push_subscriptions_user_id_idx
    ON push_subscriptions (user_id) WHERE deleted_at IS NULL;

-- OAuth apps for Connections saved in the app instead of the server's
-- environment: an admin of the hosted site, or the owner of the desktop
-- app, pastes the Google (or Stripe, Shopify) client id and secret once.
-- The secret is stored only through crypto::encrypt. Environment variables
-- still win when set.

CREATE TABLE connector_apps (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    provider TEXT NOT NULL CHECK (provider IN ('google', 'stripe', 'shopify')),
    client_id TEXT NOT NULL CHECK (char_length(client_id) BETWEEN 1 AND 300),
    client_secret_enc TEXT NOT NULL,
    set_by_user_id UUID NOT NULL REFERENCES users (id),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX connector_apps_provider_key ON connector_apps (provider)
    WHERE deleted_at IS NULL;

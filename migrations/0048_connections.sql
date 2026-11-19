-- Connections: an office's links to the tools a small business already uses
-- (Google, Stripe, Shopify) and generic webhooks for everything else.
--
-- Credentials the server must present again (OAuth access and refresh
-- tokens) are stored only through crypto::encrypt. Stripe needs none: calls
-- use the platform key with the connected account's id.

CREATE TABLE connections (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    created_by_user_id UUID NOT NULL REFERENCES users (id),
    provider TEXT NOT NULL CHECK (provider IN ('google', 'stripe', 'shopify', 'webhook')),
    -- What people see: the Google email, the Stripe account, the shop.
    label TEXT NOT NULL,
    -- The provider's id for the account (Google subject, acct_..., shop
    -- domain). Webhooks have none.
    external_account_id TEXT,
    scopes TEXT[] NOT NULL DEFAULT '{}',
    access_token_enc TEXT,
    refresh_token_enc TEXT,
    token_expires_at TIMESTAMPTZ,
    -- Non-secret settings: the room a webhook posts into, its outbound URL,
    -- the salt its inbound token is derived from.
    config JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'needs_reconnect')),
    last_used_at TIMESTAMPTZ,
    last_error TEXT,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX connections_workspace_idx ON connections (workspace_id, created_at)
    WHERE deleted_at IS NULL;
-- Connecting the same account to the same office again refreshes it.
CREATE UNIQUE INDEX connections_account_key ON connections (workspace_id, provider, external_account_id)
    WHERE deleted_at IS NULL AND external_account_id IS NOT NULL;
CREATE UNIQUE INDEX connections_idempotency_key ON connections (workspace_id, idempotency_key)
    WHERE deleted_at IS NULL AND idempotency_key IS NOT NULL;

-- One sign-in round trip to a provider. The state handed to the provider is
-- `{id}.{secret}`; only an Argon2 hash of the secret is kept, and the PKCE
-- verifier is derived from the secret, so nothing here can finish a flow.
CREATE TABLE oauth_states (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    secret_hash TEXT NOT NULL,
    user_id UUID NOT NULL REFERENCES users (id),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    provider TEXT NOT NULL,
    -- Shopify only: the shop the person named.
    shop TEXT,
    -- Google only: the products asked for (gmail, calendar, sheets).
    products TEXT[] NOT NULL DEFAULT '{}',
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX oauth_states_expires_idx ON oauth_states (expires_at) WHERE used_at IS NULL;

-- Every action run through a connection: who ran it and whether it worked.
-- The result is kept only when the caller sent an idempotency key, so a
-- retry gets the same answer without repeating the action.
CREATE TABLE connection_actions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    connection_id UUID NOT NULL REFERENCES connections (id),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    actor_user_id UUID REFERENCES users (id),
    actor_agent_id UUID REFERENCES agents (id),
    action TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('ok', 'error')),
    result JSONB,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX connection_actions_connection_idx ON connection_actions (connection_id, created_at);
CREATE UNIQUE INDEX connection_actions_idempotency_key
    ON connection_actions (connection_id, action, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

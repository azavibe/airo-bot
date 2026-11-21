-- 0057_bot_access.sql
-- Which of an office's apps and connected tools one bot may use. Without a
-- row a bot gets its role's defaults (the Finance Manager sees the invoices
-- app and Stripe, the Store Manager sees Shopify, ...); once the owner
-- changes a bot's access, the row is the whole list.
-- `connections` holds "<connection id>:<product>" keys: a Google
-- connection's products are gmail, calendar and sheets; Stripe, Shopify and
-- webhook connections have one product named after the provider.

CREATE TABLE IF NOT EXISTS bot_access (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    agent_id UUID NOT NULL REFERENCES agents (id),
    apps TEXT[] NOT NULL DEFAULT '{}',
    connections TEXT[] NOT NULL DEFAULT '{}',
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS bot_access_one_per_office
    ON bot_access (workspace_id, agent_id)
    WHERE deleted_at IS NULL;

-- 0058_contacts.sql
-- One record per person or customer an office deals with. A contact is
-- unique by email within its office; external ids (Stripe customer, Shopify
-- customer, Google contact) make imports update instead of duplicating.
-- App records (a lead, an invoice, a booking, a ticket) link to a contact,
-- and facts remembered about one customer belong to that contact.

CREATE TABLE IF NOT EXISTS contacts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 200),
    email CITEXT CHECK (email IS NULL OR char_length(email) <= 254),
    phone TEXT CHECK (phone IS NULL OR char_length(phone) <= 40),
    company TEXT CHECK (company IS NULL OR char_length(company) <= 200),
    tags TEXT[] NOT NULL DEFAULT '{}',
    notes TEXT CHECK (notes IS NULL OR char_length(notes) <= 4000),
    -- Who made it: the owner, a bot, or an import.
    source TEXT NOT NULL DEFAULT 'owner'
        CHECK (source IN ('owner', 'bot', 'stripe', 'shopify', 'google', 'import')),
    -- Email marketing consent, kept across imports.
    marketing_consent TEXT NOT NULL DEFAULT 'unknown'
        CHECK (marketing_consent IN ('unknown', 'subscribed', 'unsubscribed')),
    consent_updated_at TIMESTAMPTZ,
    last_contacted_at TIMESTAMPTZ,
    created_by_agent_id UUID REFERENCES agents (id),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS contacts_email_per_office
    ON contacts (workspace_id, email)
    WHERE deleted_at IS NULL AND email IS NOT NULL;
CREATE INDEX IF NOT EXISTS contacts_phone_idx
    ON contacts (workspace_id, phone)
    WHERE deleted_at IS NULL AND phone IS NOT NULL;
CREATE INDEX IF NOT EXISTS contacts_recent_idx
    ON contacts (workspace_id, updated_at DESC)
    WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS contacts_idempotency_idx
    ON contacts (workspace_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS contact_external_ids (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    contact_id UUID NOT NULL REFERENCES contacts (id),
    provider TEXT NOT NULL CHECK (provider IN ('stripe', 'shopify', 'google')),
    external_id TEXT NOT NULL CHECK (char_length(external_id) <= 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, provider, external_id)
);

ALTER TABLE office_records ADD COLUMN IF NOT EXISTS contact_id UUID REFERENCES contacts (id);
CREATE INDEX IF NOT EXISTS office_records_contact_idx
    ON office_records (contact_id)
    WHERE contact_id IS NOT NULL AND deleted_at IS NULL;

ALTER TABLE memories ADD COLUMN IF NOT EXISTS contact_id UUID REFERENCES contacts (id);
CREATE INDEX IF NOT EXISTS memories_contact_idx
    ON memories (contact_id)
    WHERE contact_id IS NOT NULL AND deleted_at IS NULL;

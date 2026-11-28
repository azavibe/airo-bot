-- 0074_whatsapp.sql
-- WhatsApp for an office, on Meta's official WhatsApp Business Cloud API.
-- The owner connects their own Meta app's number (its id, an access token
-- and the app secret, both kept only encrypted). Customers write to the
-- business on WhatsApp; a Receptionist bot answers within WhatsApp's
-- 24-hour window, takes bookings and leads into the office's apps, and
-- hands over to a person. Every message shows in the office's WhatsApp
-- room, where a person can answer a customer too.

CREATE TABLE IF NOT EXISTS whatsapp_numbers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    owner_user_id UUID NOT NULL REFERENCES users (id),
    -- Meta's id for the business number, and how it reads.
    phone_number_id TEXT NOT NULL CHECK (phone_number_id ~ '^[0-9]{5,32}$'),
    display_number TEXT NOT NULL CHECK (char_length(display_number) <= 40),
    verified_name TEXT CHECK (verified_name IS NULL OR char_length(verified_name) <= 200),
    access_token_encrypted TEXT NOT NULL,
    app_secret_encrypted TEXT NOT NULL,
    -- The Receptionist bot and the room its conversations show in.
    agent_id UUID NOT NULL REFERENCES agents (id),
    thread_id UUID NOT NULL REFERENCES threads (id),
    -- Off: messages still arrive in the room, and only people answer.
    auto_reply BOOLEAN NOT NULL DEFAULT true,
    last_error TEXT CHECK (last_error IS NULL OR char_length(last_error) <= 300),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS whatsapp_numbers_office_idx
    ON whatsapp_numbers (workspace_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS whatsapp_numbers_number_idx
    ON whatsapp_numbers (phone_number_id) WHERE deleted_at IS NULL;
-- A person's message in a room is checked against this on every post.
CREATE INDEX IF NOT EXISTS whatsapp_numbers_room_idx
    ON whatsapp_numbers (thread_id) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS whatsapp_messages (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    number_id UUID NOT NULL REFERENCES whatsapp_numbers (id),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    -- The customer's WhatsApp id (their number, digits only).
    customer TEXT NOT NULL CHECK (customer ~ '^[0-9]{6,20}$'),
    customer_name TEXT CHECK (customer_name IS NULL OR char_length(customer_name) <= 200),
    direction TEXT NOT NULL CHECK (direction IN ('in', 'out')),
    -- customer (in), bot or person (out).
    sent_by TEXT NOT NULL CHECK (sent_by IN ('customer', 'bot', 'person')),
    body TEXT NOT NULL CHECK (char_length(body) <= 4096),
    -- WhatsApp's id for the message: a webhook delivered twice is kept once.
    wamid TEXT CHECK (wamid IS NULL OR char_length(wamid) <= 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS whatsapp_messages_wamid_idx
    ON whatsapp_messages (number_id, wamid) WHERE wamid IS NOT NULL;
CREATE INDEX IF NOT EXISTS whatsapp_messages_conversation_idx
    ON whatsapp_messages (number_id, customer, created_at DESC) WHERE deleted_at IS NULL;

-- Customers who write on WhatsApp are added to the office's contacts.
ALTER TABLE contacts DROP CONSTRAINT IF EXISTS contacts_source_check;
ALTER TABLE contacts ADD CONSTRAINT contacts_source_check
    CHECK (source IN ('owner', 'bot', 'stripe', 'shopify', 'google', 'import', 'whatsapp'));

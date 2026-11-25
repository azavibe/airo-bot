-- 0073_phone.sql
-- A phone receptionist per office (issue #32). The owner connects a Twilio
-- number (account SID, auth token kept only encrypted, the number) and
-- Airobot points the number's calls at this server. A Receptionist bot
-- answers in the office's language, takes bookings and messages into the
-- office's apps, and hands over to a person (a number to forward to) when
-- the caller asks. Each call's transcript is kept, and shown in the
-- office's Phone calls room.

CREATE TABLE IF NOT EXISTS phone_lines (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    owner_user_id UUID NOT NULL REFERENCES users (id),
    account_sid TEXT NOT NULL CHECK (account_sid ~ '^AC[0-9a-f]{32}$'),
    auth_token_encrypted TEXT NOT NULL,
    -- E.164, and Twilio's id for it.
    phone_number TEXT NOT NULL CHECK (phone_number ~ '^\+[1-9][0-9]{6,14}$'),
    number_sid TEXT NOT NULL CHECK (char_length(number_sid) <= 64),
    -- A person to hand callers over to.
    forward_to TEXT CHECK (forward_to IS NULL OR forward_to ~ '^\+[1-9][0-9]{6,14}$'),
    greeting TEXT CHECK (greeting IS NULL OR char_length(greeting) <= 300),
    -- The Receptionist bot and the room its calls show in.
    agent_id UUID NOT NULL REFERENCES agents (id),
    thread_id UUID NOT NULL REFERENCES threads (id),
    last_error TEXT CHECK (last_error IS NULL OR char_length(last_error) <= 300),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS phone_lines_office_idx
    ON phone_lines (workspace_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS phone_lines_number_idx
    ON phone_lines (phone_number) WHERE deleted_at IS NULL;

CREATE TABLE IF NOT EXISTS phone_calls (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    line_id UUID NOT NULL REFERENCES phone_lines (id),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    call_sid TEXT NOT NULL CHECK (char_length(call_sid) <= 64),
    from_number TEXT CHECK (from_number IS NULL OR char_length(from_number) <= 32),
    -- [{"who": "caller" | "receptionist", "text": "..."}]
    transcript JSONB NOT NULL DEFAULT '[]',
    records_added INTEGER NOT NULL DEFAULT 0,
    -- Turns in a row the caller said nothing.
    silent_turns INTEGER NOT NULL DEFAULT 0,
    transferred BOOLEAN NOT NULL DEFAULT false,
    ended_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS phone_calls_sid_idx ON phone_calls (call_sid);
CREATE INDEX IF NOT EXISTS phone_calls_office_idx ON phone_calls (workspace_id, created_at DESC);

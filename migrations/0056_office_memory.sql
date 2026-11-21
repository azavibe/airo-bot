-- 0056_office_memory.sql
-- What an office's bots always know: where the business is going (goal),
-- what it is like (about: hours, prices, tone, policies) and a short list
-- of remembered facts. A fact belongs to the whole office, or to one bot
-- when agent_id is set. The owner sees, edits and deletes every fact; a bot
-- may add one (source 'bot'), shown in the chat with an Undo button.

ALTER TABLE workspaces
    ADD COLUMN IF NOT EXISTS goal TEXT CHECK (goal IS NULL OR char_length(goal) <= 300),
    ADD COLUMN IF NOT EXISTS about TEXT CHECK (about IS NULL OR char_length(about) <= 4000);

CREATE TABLE IF NOT EXISTS memories (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    agent_id UUID REFERENCES agents (id),
    body TEXT NOT NULL CHECK (char_length(body) BETWEEN 1 AND 500),
    source TEXT NOT NULL DEFAULT 'owner' CHECK (source IN ('owner', 'bot')),
    created_by_agent_id UUID REFERENCES agents (id),
    thread_id UUID REFERENCES threads (id),
    pinned BOOLEAN NOT NULL DEFAULT false,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS memories_workspace_idx
    ON memories (workspace_id, pinned DESC, created_at DESC)
    WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS memories_idempotency_idx
    ON memories (workspace_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

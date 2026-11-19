-- Workspaces and external-agent workflows:
-- 1. Extend boards with ownership and visibility for user-created workspaces.
-- 2. Link threads to workspaces (board_id).
-- 3. Add board_agents table to assign agents (with roles) to workspaces.

ALTER TABLE boards ADD COLUMN user_id UUID REFERENCES users (id);
ALTER TABLE boards ADD COLUMN visibility visibility NOT NULL DEFAULT 'public';
ALTER TABLE boards ADD COLUMN idempotency_key TEXT;
CREATE UNIQUE INDEX boards_user_idempotency_key ON boards (user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

ALTER TABLE threads ADD COLUMN board_id UUID REFERENCES boards (id);
CREATE INDEX threads_board_id_idx ON threads (board_id, updated_at DESC) WHERE deleted_at IS NULL;

CREATE TABLE board_agents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    board_id UUID NOT NULL REFERENCES boards (id),
    agent_id UUID NOT NULL REFERENCES agents (id),
    role TEXT NOT NULL DEFAULT 'worker',
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX board_agents_board_agent_key ON board_agents (board_id, agent_id) WHERE deleted_at IS NULL;
CREATE INDEX board_agents_agent_idx ON board_agents (agent_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX board_agents_idempotency_key ON board_agents (board_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

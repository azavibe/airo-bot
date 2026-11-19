-- Routing memory: Air learns where you send work. Every manual
-- assignment (Give work, or "give it to a different worker") records the
-- job's pattern and the agent you chose; future jobs matching a pattern
-- go to that agent automatically, and the choice is always visible in the
-- audit trail so the learning can be trusted and corrected.
CREATE TABLE IF NOT EXISTS routing_preferences (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    task_pattern TEXT NOT NULL,
    agent_id UUID NOT NULL REFERENCES agents (id),
    times_chosen INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS routing_preferences_ws_pattern_key
    ON routing_preferences (workspace_id, task_pattern) WHERE deleted_at IS NULL;

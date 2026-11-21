-- 0062_goals.sql
-- Goals that move: milestones toward the office's goal (the owner's, or
-- proposed by the Architect and kept by the owner), opt-in check-ins where
-- the Architect reports progress (off by default), and a weekly report
-- card posted in the office chat every Monday, built from the database
-- (records added, jobs and their outcomes, spend, milestones), costing no
-- AI credits.

CREATE TABLE IF NOT EXISTS milestones (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200),
    due_on DATE,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('proposed', 'active', 'done')),
    source TEXT NOT NULL DEFAULT 'owner' CHECK (source IN ('owner', 'bot')),
    created_by_agent_id UUID REFERENCES agents (id),
    thread_id UUID REFERENCES threads (id),
    done_at TIMESTAMPTZ,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS milestones_workspace_idx
    ON milestones (workspace_id, status, due_on)
    WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS milestones_idempotency_idx
    ON milestones (workspace_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

ALTER TABLE workspaces
    ADD COLUMN IF NOT EXISTS check_in TEXT NOT NULL DEFAULT 'off'
        CHECK (check_in IN ('off', 'daily', 'weekly', 'monthly')),
    ADD COLUMN IF NOT EXISTS last_check_in_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS weekly_report BOOLEAN NOT NULL DEFAULT true,
    ADD COLUMN IF NOT EXISTS last_report_at TIMESTAMPTZ;

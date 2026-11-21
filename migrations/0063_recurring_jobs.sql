-- 0063_recurring_jobs.sql
-- Recurring jobs set from chat: "every Monday at 9, list overdue invoices".
-- A bot proposes one with an `EVERY ...:` line, or the owner adds one in
-- the bot's settings. Each runs as one of the bot's jobs in its room, at
-- the hour given in the owner's time, and reports an honest outcome.
-- Per-bot permission modes: `ask_first` (the default) means a schedule the
-- bot proposes waits for the owner's Start; `trusted` starts it at once
-- (the owner can stop it). Anything sent to an outside tool needs an
-- approval in every mode.

CREATE TABLE IF NOT EXISTS recurring_jobs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    agent_id UUID NOT NULL REFERENCES agents (id),
    thread_id UUID NOT NULL REFERENCES threads (id),
    job TEXT NOT NULL CHECK (char_length(job) BETWEEN 1 AND 500),
    every TEXT NOT NULL CHECK (every IN ('day', 'week', 'month')),
    -- ISO weekday (1 = Monday) for weekly jobs.
    weekday SMALLINT CHECK (weekday IS NULL OR weekday BETWEEN 1 AND 7),
    day_of_month SMALLINT CHECK (day_of_month IS NULL OR day_of_month BETWEEN 1 AND 28),
    hour SMALLINT NOT NULL DEFAULT 9 CHECK (hour BETWEEN 0 AND 23),
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('proposed', 'active', 'stopped')),
    source TEXT NOT NULL DEFAULT 'owner' CHECK (source IN ('owner', 'bot')),
    -- When it was last started: a slot earlier than this doesn't run.
    started_at TIMESTAMPTZ,
    last_run_at TIMESTAMPTZ,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CHECK ((every = 'week') = (weekday IS NOT NULL)),
    CHECK ((every = 'month') = (day_of_month IS NOT NULL))
);

CREATE INDEX IF NOT EXISTS recurring_jobs_due_idx
    ON recurring_jobs (status, last_run_at)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS recurring_jobs_agent_idx
    ON recurring_jobs (agent_id, created_at)
    WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS recurring_jobs_idempotency_idx
    ON recurring_jobs (agent_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

ALTER TABLE agents
    ADD COLUMN IF NOT EXISTS permission_mode TEXT NOT NULL DEFAULT 'ask_first'
        CHECK (permission_mode IN ('ask_first', 'trusted'));

-- 0061_task_outcomes.sql
-- Honest outcomes for a bot's jobs: done, nothing to do, blocked, needs the
-- owner's OK, or failed, with a short note. Server tasks get one when they
-- finish; jobs a bot does in chat (a task the Architect handed it, a
-- kickoff task) are now kept as tasks too (executor 'chat': a record only,
-- never claimed or run by the dispatcher or an agent), so a bot's settings
-- can list its recent work.

ALTER TABLE tasks
    ADD COLUMN IF NOT EXISTS outcome TEXT
        CHECK (outcome IS NULL OR outcome IN ('done', 'nothing_to_do', 'blocked', 'needs_ok', 'failed')),
    ADD COLUMN IF NOT EXISTS outcome_note TEXT
        CHECK (outcome_note IS NULL OR char_length(outcome_note) <= 500);

ALTER TABLE tasks DROP CONSTRAINT IF EXISTS tasks_executor_check;
ALTER TABLE tasks
    ADD CONSTRAINT tasks_executor_check CHECK (executor IN ('agent', 'server', 'chat'));

CREATE INDEX IF NOT EXISTS tasks_agent_recent_idx
    ON tasks (assigned_agent_id, created_at DESC)
    WHERE deleted_at IS NULL AND assigned_agent_id IS NOT NULL;

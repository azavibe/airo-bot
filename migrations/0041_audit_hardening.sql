-- 0041_audit_hardening.sql
-- Fixes from the 2026-11-13 audit (docs/review-2026-11-13.md).

-- 1. Only tasks explicitly marked for server execution enter the in-process
--    FSM dispatcher. Agent tasks are claimed by agents through
--    /tasks/{id}/claim; the dispatcher used to race those claims and mark
--    them completed with fabricated output.
ALTER TABLE tasks
    ADD COLUMN IF NOT EXISTS executor TEXT NOT NULL DEFAULT 'agent';

ALTER TABLE tasks
    ADD CONSTRAINT tasks_executor_check CHECK (executor IN ('agent', 'server'));

-- Keep the FSM column truthful for agent-executed tasks that already finished.
UPDATE tasks SET fsm_state = 'COMPLETED' WHERE executor = 'agent' AND status = 'completed';
UPDATE tasks SET fsm_state = 'FAILED' WHERE executor = 'agent' AND status IN ('failed', 'cancelled');
UPDATE tasks SET fsm_state = 'CLAIMED' WHERE executor = 'agent' AND status IN ('assigned', 'running');

DROP INDEX IF EXISTS idx_tasks_fsm_wake;
CREATE INDEX IF NOT EXISTS tasks_server_fsm_idx
    ON tasks (fsm_state, wake_at, created_at)
    WHERE executor = 'server' AND deleted_at IS NULL;

-- 2. Daily burn resets when the calendar day changes (checked inside the
--    budget gate's row lock, so no cron job is needed).
ALTER TABLE workspace_wallets
    ADD COLUMN IF NOT EXISTS id UUID NOT NULL DEFAULT gen_random_uuid(),
    ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ADD COLUMN IF NOT EXISTS burn_day DATE NOT NULL DEFAULT CURRENT_DATE;
CREATE UNIQUE INDEX IF NOT EXISTS workspace_wallets_id_key ON workspace_wallets (id);

-- 3. Table conventions (AGENTS.md): updated_at / deleted_at everywhere, soft
--    delete instead of cascading hard deletes, optional idempotency keys.
ALTER TABLE credit_ledger
    ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

ALTER TABLE procedure_evolutions
    ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ADD COLUMN IF NOT EXISTS idempotency_key TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS procedure_evolutions_idempotency_idx
    ON procedure_evolutions (procedure_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS procedure_evolutions_recent_idx
    ON procedure_evolutions (procedure_id, created_at DESC);

ALTER TABLE office_apps
    ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS idempotency_key TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS office_apps_idempotency_idx
    ON office_apps (workspace_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

ALTER TABLE office_records
    ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS idempotency_key TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS office_records_idempotency_idx
    ON office_records (workspace_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

ALTER TABLE workspace_lessons
    ADD COLUMN IF NOT EXISTS idempotency_key TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS workspace_lessons_idempotency_idx
    ON workspace_lessons (workspace_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

ALTER TABLE office_apps DROP CONSTRAINT IF EXISTS office_apps_workspace_id_fkey,
    ADD CONSTRAINT office_apps_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES workspaces (id);
ALTER TABLE office_records DROP CONSTRAINT IF EXISTS office_records_workspace_id_fkey,
    ADD CONSTRAINT office_records_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES workspaces (id);
ALTER TABLE office_records DROP CONSTRAINT IF EXISTS office_records_app_id_fkey,
    ADD CONSTRAINT office_records_app_id_fkey FOREIGN KEY (app_id) REFERENCES office_apps (id);
ALTER TABLE workspace_wallets DROP CONSTRAINT IF EXISTS workspace_wallets_workspace_id_fkey,
    ADD CONSTRAINT workspace_wallets_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES workspaces (id);
ALTER TABLE credit_ledger DROP CONSTRAINT IF EXISTS credit_ledger_workspace_id_fkey,
    ADD CONSTRAINT credit_ledger_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES workspaces (id);
ALTER TABLE procedure_evolutions DROP CONSTRAINT IF EXISTS procedure_evolutions_procedure_id_fkey,
    ADD CONSTRAINT procedure_evolutions_procedure_id_fkey FOREIGN KEY (procedure_id) REFERENCES procedures (id);
ALTER TABLE procedure_evolutions DROP CONSTRAINT IF EXISTS procedure_evolutions_workspace_id_fkey,
    ADD CONSTRAINT procedure_evolutions_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES workspaces (id);
ALTER TABLE workspace_lessons DROP CONSTRAINT IF EXISTS workspace_lessons_workspace_id_fkey,
    ADD CONSTRAINT workspace_lessons_workspace_id_fkey FOREIGN KEY (workspace_id) REFERENCES workspaces (id);

-- 4. Indexes that match the queries actually issued. The GIN index on
--    office_records.data served no query and only slowed writes.
DROP INDEX IF EXISTS idx_office_records_data_gin;
CREATE INDEX IF NOT EXISTS office_records_app_recent_idx
    ON office_records (app_id, created_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS office_records_app_hash_idx
    ON office_records (app_id, content_hash) WHERE deleted_at IS NULL;

CREATE TABLE runs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    procedure_id UUID NOT NULL,
    procedure_version INT NOT NULL,
    agent_id UUID NOT NULL REFERENCES agents (id),
    user_id UUID NOT NULL REFERENCES users (id),
    model TEXT NOT NULL,
    outcome run_outcome NOT NULL,
    attempts INT NOT NULL DEFAULT 1 CHECK (attempts > 0),
    duration_ms BIGINT NOT NULL CHECK (duration_ms >= 0),
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ NOT NULL,
    thread_id UUID REFERENCES threads (id),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CONSTRAINT runs_finished_after_started CHECK (finished_at >= started_at),
    CONSTRAINT runs_procedure_version_fkey
        FOREIGN KEY (procedure_id, procedure_version)
        REFERENCES procedure_versions (procedure_id, version)
);

CREATE INDEX runs_procedure_id_model_idx
    ON runs (procedure_id, model) WHERE deleted_at IS NULL;
CREATE INDEX runs_user_id_idx ON runs (user_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX runs_agent_id_idempotency_key_key
    ON runs (agent_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

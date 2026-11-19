CREATE TABLE procedure_versions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    procedure_id UUID NOT NULL REFERENCES procedures (id),
    version INT NOT NULL CHECK (version > 0),
    body TEXT NOT NULL,
    changelog TEXT,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX procedure_versions_procedure_id_version_key
    ON procedure_versions (procedure_id, version);
CREATE UNIQUE INDEX procedure_versions_procedure_id_idempotency_key_key
    ON procedure_versions (procedure_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

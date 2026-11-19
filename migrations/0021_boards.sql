CREATE TABLE boards (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name CITEXT NOT NULL UNIQUE,
    description TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE TABLE board_memberships (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users (id),
    board_id UUID NOT NULL REFERENCES boards (id),
    position BIGINT NOT NULL DEFAULT 0,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX board_memberships_user_board_key
    ON board_memberships (user_id, board_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX board_memberships_user_idempotency_key_key
    ON board_memberships (user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX board_memberships_user_position_idx
    ON board_memberships (user_id, position) WHERE deleted_at IS NULL;

ALTER TABLE procedures ADD COLUMN board_id UUID REFERENCES boards (id);
CREATE INDEX procedures_public_board_idx
    ON procedures (board_id, published_at DESC)
    WHERE visibility = 'public' AND published_at IS NOT NULL AND deleted_at IS NULL;

INSERT INTO boards (name, description) VALUES
    ('pr-review', 'Code review loops, scoring, merge gates.'),
    ('email', 'Outreach, follow-ups, routing, and replies.'),
    ('scraping', 'Collection, extraction, and monitoring workflows.'),
    ('data-cleanup', 'Validation, normalization, and data repair.'),
    ('testing', 'Test generation, execution, and regression loops.'),
    ('deploys', 'Release checks, rollouts, and rollback procedures.'),
    ('research', 'Source gathering, synthesis, and verification.'),
    ('support', 'Triage, response drafting, and escalation workflows.');

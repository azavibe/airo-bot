-- 0070_office_invites.sql
-- Team invites and roles for an office (issue #29). The owner invites a
-- teammate by email; the link carries a one-time secret (stored only as an
-- Argon2 hash) and works only for an account with that email. Accepting
-- adds a workspace_members row with the invited role:
--   owner   the office's owner (workspaces.owner_user_id), one per office
--   member  works with the bots: chats, approves
--   viewer  reads the office's chats, apps and approvals; changes nothing

CREATE TABLE IF NOT EXISTS office_invites (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces (id),
    email TEXT NOT NULL CHECK (char_length(email) BETWEEN 3 AND 320),
    role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('member', 'viewer')),
    token_hash TEXT NOT NULL,
    invited_by UUID NOT NULL REFERENCES users (id),
    expires_at TIMESTAMPTZ NOT NULL,
    accepted_at TIMESTAMPTZ,
    accepted_by UUID REFERENCES users (id),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Revoked by the owner, or replaced by a newer invite to the same email.
    deleted_at TIMESTAMPTZ
);

-- One open invite per email per office.
CREATE UNIQUE INDEX IF NOT EXISTS office_invites_open_idx
    ON office_invites (workspace_id, lower(email))
    WHERE deleted_at IS NULL AND accepted_at IS NULL;

CREATE UNIQUE INDEX IF NOT EXISTS office_invites_idempotency_idx
    ON office_invites (workspace_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

-- Roles are one of three; existing rows are only owner or member. NOT VALID
-- skips re-checking old rows on deploy; new and changed rows are checked.
ALTER TABLE workspace_members
    ADD CONSTRAINT workspace_members_role_check
    CHECK (role IN ('owner', 'member', 'viewer')) NOT VALID;

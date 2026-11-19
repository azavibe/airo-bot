CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Nullable: a user exists (and can hold a session) as soon as their
    -- email is verified, but sign-up asks for a handle as a separate step
    -- right after. A unique index still applies once it's set -- Postgres
    -- unique indexes don't compare NULLs against each other.
    handle CITEXT,
    email CITEXT NOT NULL,
    display_name TEXT,
    avatar_url TEXT,
    bio TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

-- Partial (not plain) unique indexes: a soft-deleted user's handle/email
-- must be free for someone else to take, per the "never hard delete" rule.
CREATE UNIQUE INDEX users_handle_key ON users (handle) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX users_email_key ON users (email) WHERE deleted_at IS NULL;

-- 0075_sync.sql
-- Desktop and cloud sync (issue #28). The desktop app links to an Airobot
-- Cloud (or self-hosted) account with a device code the person allows in
-- the cloud's Settings, and then both keep the account's offices the same:
-- offices, rooms, messages, bots (never their keys or AI tool settings),
-- apps and records, contacts, memory and documents. The cloud is the
-- source of truth; the newest edit of a row wins (the cloud's on a tie)
-- and messages are only ever added. Rows travel by when they last changed
-- on each side (`changed_at`), over the server's own API.

-- A device code can link a computer for sync instead of signing in an agent.
ALTER TABLE login_codes ADD COLUMN IF NOT EXISTS purpose TEXT NOT NULL DEFAULT 'agent'
    CHECK (purpose IN ('agent', 'sync'));

-- Cloud: a linked computer's key (kept only as an Argon2 hash).
CREATE TABLE IF NOT EXISTS sync_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users (id),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 64),
    key_hash TEXT NOT NULL,
    last_used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS sync_keys_user_idx
    ON sync_keys (user_id) WHERE revoked_at IS NULL AND deleted_at IS NULL;

-- Desktop: its link to the cloud (at most one). The key and the pending
-- device code are kept only encrypted (the desktop must present them).
CREATE TABLE IF NOT EXISTS sync_links (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_user_id UUID NOT NULL REFERENCES users (id),
    cloud_url TEXT NOT NULL CHECK (char_length(cloud_url) <= 300),
    cloud_user_id UUID,
    cloud_handle TEXT CHECK (cloud_handle IS NULL OR char_length(cloud_handle) <= 64),
    key_encrypted TEXT,
    -- While the person hasn't allowed the link yet.
    device_code_encrypted TEXT,
    link_code TEXT CHECK (link_code IS NULL OR char_length(link_code) <= 16),
    link_url TEXT CHECK (link_url IS NULL OR char_length(link_url) <= 400),
    link_expires_at TIMESTAMPTZ,
    -- {"push": {table: changed_at}, "pull": {table: changed_at}}
    cursors JSONB NOT NULL DEFAULT '{}',
    last_synced_at TIMESTAMPTZ,
    last_error TEXT CHECK (last_error IS NULL OR char_length(last_error) <= 300),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS sync_links_one_idx
    ON sync_links ((true)) WHERE deleted_at IS NULL;

-- Desktop: a row that is the same as one in the cloud under another id (the
-- person's Architect on both sides, an app or a contact made on both).
CREATE TABLE IF NOT EXISTS sync_id_map (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    table_name TEXT NOT NULL CHECK (char_length(table_name) <= 40),
    local_id UUID NOT NULL,
    cloud_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS sync_id_map_local_idx ON sync_id_map (table_name, local_id);
CREATE UNIQUE INDEX IF NOT EXISTS sync_id_map_cloud_idx ON sync_id_map (table_name, cloud_id);

-- When a row last changed on this server, for the sync cursors. A row the
-- desktop writes from the cloud keeps its value, so it isn't sent back.
CREATE OR REPLACE FUNCTION sync_touch() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('airobot.sync_quiet', true) = 'on' THEN
        IF TG_OP = 'INSERT' THEN
            NEW.changed_at := '-infinity';
        ELSE
            NEW.changed_at := OLD.changed_at;
        END IF;
    ELSE
        NEW.changed_at := clock_timestamp();
    END IF;
    RETURN NEW;
END $$;

DO $$
DECLARE t TEXT;
BEGIN
    FOREACH t IN ARRAY ARRAY['workspaces', 'agents', 'threads', 'thread_agents', 'notes',
                             'office_apps', 'contacts', 'office_records', 'memories', 'documents'] LOOP
        EXECUTE format('ALTER TABLE %I ADD COLUMN IF NOT EXISTS changed_at TIMESTAMPTZ NOT NULL DEFAULT now()', t);
        EXECUTE format('CREATE INDEX IF NOT EXISTS %I ON %I (changed_at, id)', t || '_changed_at_idx', t);
        EXECUTE format('DROP TRIGGER IF EXISTS sync_touch ON %I', t);
        EXECUTE format('CREATE TRIGGER sync_touch BEFORE INSERT OR UPDATE ON %I
                        FOR EACH ROW EXECUTE FUNCTION sync_touch()', t);
    END LOOP;
END $$;

-- Everything that works gets an @handle (Instagram-style: searchable,
-- mentionable, stable across renames), and software agents get a character:
-- `instructions` is the system prompt that shapes how they talk. The
-- "cactus" pattern: name it, give it a handle and a personality, text it
-- now; attach a humidity sensor to the same agent later.
ALTER TABLE agents ADD COLUMN IF NOT EXISTS handle TEXT;
ALTER TABLE agents ADD COLUMN IF NOT EXISTS instructions TEXT;
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS handle TEXT;

-- Backfill: slug of the name, uniquified with a short id suffix on
-- collision, so every existing agent/device is searchable immediately.
UPDATE agents SET handle = lower(regexp_replace(name, '[^a-zA-Z0-9]+', '-', 'g'))
WHERE handle IS NULL AND deleted_at IS NULL;
UPDATE nodes SET handle = lower(regexp_replace(name, '[^a-zA-Z0-9]+', '-', 'g'))
WHERE handle IS NULL AND deleted_at IS NULL;

UPDATE agents a SET handle = a.handle || '-' || substr(a.id::text, 1, 4)
WHERE EXISTS (SELECT 1 FROM agents b WHERE b.handle = a.handle AND b.id <> a.id AND b.deleted_at IS NULL);
UPDATE nodes n SET handle = n.handle || '-' || substr(n.id::text, 1, 4)
WHERE EXISTS (SELECT 1 FROM nodes b WHERE b.handle = n.handle AND b.id <> n.id AND b.deleted_at IS NULL);

CREATE UNIQUE INDEX IF NOT EXISTS agents_handle_key ON agents (handle) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS nodes_handle_key ON nodes (handle) WHERE deleted_at IS NULL;

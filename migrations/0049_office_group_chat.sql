-- Every office has one group chat where all of its bots listen and the
-- Architect decides who handles a message. It is a thread with kind
-- 'office'; the other threads stay one-to-one rooms ('room').
ALTER TABLE threads
    ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'room'
        CHECK (kind IN ('room', 'office'));

-- An office's old "#master-architect" room becomes its group chat.
UPDATE threads SET kind = 'office', title = '#office'
WHERE id IN (
    SELECT DISTINCT ON (workspace_id) id
    FROM threads
    WHERE title = '#master-architect' AND workspace_id IS NOT NULL AND deleted_at IS NULL
    ORDER BY workspace_id, created_at
);

CREATE UNIQUE INDEX IF NOT EXISTS threads_one_office_chat_idx
    ON threads (workspace_id) WHERE kind = 'office' AND deleted_at IS NULL;

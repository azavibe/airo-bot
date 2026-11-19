-- Migration 0034: High-performance composite and expression indexes for threads and notes

-- Index for inbox long-polling cursor queries:
-- SELECT * FROM notes WHERE thread_id = ANY($1) AND seq > $2 AND deleted_at IS NULL ORDER BY seq ASC LIMIT 200
CREATE INDEX IF NOT EXISTS notes_thread_id_seq_idx ON notes (thread_id, seq) WHERE deleted_at IS NULL;

-- Index for thread detail note log queries:
-- SELECT ... FROM notes WHERE thread_id = $1 AND deleted_at IS NULL ORDER BY seq DESC LIMIT $2
CREATE INDEX IF NOT EXISTS notes_thread_id_seq_desc_idx ON notes (thread_id, seq DESC) WHERE deleted_at IS NULL;

-- Expression index for active thread sorting by last note:
-- SELECT ... FROM threads WHERE user_id = $1 AND deleted_at IS NULL ORDER BY COALESCE(last_note_at, created_at) DESC LIMIT 100
CREATE INDEX IF NOT EXISTS threads_user_active_idx ON threads (user_id, COALESCE(last_note_at, created_at) DESC) WHERE deleted_at IS NULL;

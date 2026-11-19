ALTER TABLE notifications ADD COLUMN pushed_at TIMESTAMPTZ;

CREATE INDEX notifications_pending_push_idx
    ON notifications (user_id, created_at)
    WHERE pushed_at IS NULL AND deleted_at IS NULL;

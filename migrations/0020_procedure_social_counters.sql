-- Feed cards must not aggregate runs or comments live. These counters are
-- maintained in the same transaction as their writes.
ALTER TABLE procedures
    ADD COLUMN attempt_count BIGINT NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    ADD COLUMN comment_count BIGINT NOT NULL DEFAULT 0 CHECK (comment_count >= 0);

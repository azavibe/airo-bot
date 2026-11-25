-- 0068_office_language.sql
-- A language per office (issue #31): its bots write posts, emails, quotes
-- and replies to customers in it. Null: no set language (bots follow
-- whoever writes to them). The server keeps the list of codes it offers;
-- the check only keeps the column to the shape of a language code.

ALTER TABLE workspaces
    ADD COLUMN IF NOT EXISTS language TEXT
        CHECK (language IS NULL OR language ~ '^[a-z]{2,3}(-[A-Z]{2})?$');

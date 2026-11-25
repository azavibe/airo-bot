-- 0069_ui_language.sql
-- The language a person reads the app in (issue #31). Null: automatic,
-- from their browser or computer. Kept on the account so it follows them
-- to every browser and the desktop app.

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS ui_language TEXT
        CHECK (ui_language IS NULL OR ui_language ~ '^[a-z]{2,3}(-[A-Z]{2})?$');

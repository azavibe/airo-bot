-- citext gives case-insensitive uniqueness for handles and emails without
-- the app having to normalize case itself.
CREATE EXTENSION IF NOT EXISTS citext;

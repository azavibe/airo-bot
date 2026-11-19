-- `visibility` is shared across every table the standing conventions say
-- needs it (threads now, procedures in Phase 3).
CREATE TYPE visibility AS ENUM ('private', 'unlisted', 'public');

CREATE TYPE thread_status AS ENUM ('open', 'waiting', 'done', 'failed');

CREATE TYPE note_kind AS ENUM ('status', 'command', 'comment', 'system');

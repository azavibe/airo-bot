ALTER TABLE procedures
ADD COLUMN kind TEXT NOT NULL DEFAULT 'procedure'
CHECK (kind IN ('procedure', 'post'));


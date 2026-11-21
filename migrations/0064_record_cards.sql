-- 0064_record_cards.sql
-- Live app records in chat. A message that adds or changes a record keeps
-- card references (notes.cards: [{type: "app_record_card", app_slug,
-- record_id, action, title, changed_fields?, approval_id?}]), not copies;
-- the app draws each card from the record as it is now. A bot's change to a
-- record that waits for the owner's OK is an approval tied to the record,
-- with the proposed field values.

ALTER TABLE notes ADD COLUMN IF NOT EXISTS cards JSONB;

ALTER TABLE approvals
    ADD COLUMN IF NOT EXISTS record_id UUID REFERENCES office_records (id),
    ADD COLUMN IF NOT EXISTS changes JSONB;

CREATE INDEX IF NOT EXISTS approvals_record_pending_idx
    ON approvals (record_id)
    WHERE record_id IS NOT NULL AND status = 'pending' AND deleted_at IS NULL;

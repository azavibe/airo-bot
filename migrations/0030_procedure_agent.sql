ALTER TABLE procedures ADD COLUMN agent_id UUID REFERENCES agents (id);
CREATE INDEX procedures_agent_id_idx ON procedures (agent_id) WHERE deleted_at IS NULL;

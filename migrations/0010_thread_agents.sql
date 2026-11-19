CREATE TABLE thread_agents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    thread_id UUID NOT NULL REFERENCES threads (id),
    agent_id UUID NOT NULL REFERENCES agents (id),
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX thread_agents_thread_id_agent_id_key ON thread_agents (thread_id, agent_id);
CREATE INDEX thread_agents_agent_id_idx ON thread_agents (agent_id);

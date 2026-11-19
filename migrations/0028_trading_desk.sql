CREATE TABLE trading_desks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users (id),
    name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'paused')),
    paper_mode BOOLEAN NOT NULL DEFAULT true,
    risk_policy JSONB NOT NULL DEFAULT '{}'::jsonb,
    thread_id UUID REFERENCES threads (id),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX trading_desks_user_id_idx ON trading_desks (user_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX trading_desks_user_id_idempotency_key_key ON trading_desks (user_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE trade_candidates (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    desk_id UUID NOT NULL REFERENCES trading_desks (id),
    symbol TEXT NOT NULL,
    venue TEXT NOT NULL,
    timeframe TEXT NOT NULL DEFAULT '1h',
    market_snapshot JSONB NOT NULL,
    state TEXT NOT NULL DEFAULT 'screening' CHECK (state IN ('screening', 'analyzing', 'reviewing', 'approved', 'rejected', 'expired')),
    revision INT NOT NULL DEFAULT 1,
    expiry_at TIMESTAMPTZ NOT NULL,
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX trade_candidates_desk_id_idx ON trade_candidates (desk_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX trade_candidates_desk_id_idempotency_key_key ON trade_candidates (desk_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL AND deleted_at IS NULL;

CREATE TABLE agent_reports (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    candidate_id UUID NOT NULL REFERENCES trade_candidates (id),
    candidate_revision INT NOT NULL DEFAULT 1,
    role TEXT NOT NULL CHECK (role IN ('scout', 'news', 'sentiment', 'charts', 'skeptic', 'chief')),
    verdict TEXT NOT NULL CHECK (verdict IN ('proceed', 'wait', 'reject', 'neutral')),
    confidence DOUBLE PRECISION NOT NULL DEFAULT 1.0,
    summary TEXT NOT NULL,
    details JSONB NOT NULL DEFAULT '{}'::jsonb,
    citations JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX agent_reports_candidate_id_idx ON agent_reports (candidate_id, candidate_revision) WHERE deleted_at IS NULL;

CREATE TABLE trade_proposals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    candidate_id UUID NOT NULL REFERENCES trade_candidates (id),
    candidate_revision INT NOT NULL DEFAULT 1,
    direction TEXT NOT NULL CHECK (direction IN ('long', 'short')),
    entry_price DOUBLE PRECISION NOT NULL,
    stop_loss DOUBLE PRECISION NOT NULL,
    take_profit DOUBLE PRECISION NOT NULL,
    risk_reward_ratio DOUBLE PRECISION NOT NULL,
    valid_until TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX trade_proposals_candidate_id_idx ON trade_proposals (candidate_id, candidate_revision) WHERE deleted_at IS NULL;

CREATE TABLE trade_decisions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    candidate_id UUID NOT NULL REFERENCES trade_candidates (id),
    proposal_id UUID REFERENCES trade_proposals (id),
    candidate_revision INT NOT NULL DEFAULT 1,
    skeptic_verdict TEXT NOT NULL CHECK (skeptic_verdict IN ('execute', 'wait', 'reject')),
    chief_verdict TEXT NOT NULL CHECK (chief_verdict IN ('execute', 'wait', 'reject')),
    risk_gate_passed BOOLEAN NOT NULL,
    risk_gate_reasons JSONB NOT NULL DEFAULT '[]'::jsonb,
    final_decision TEXT NOT NULL CHECK (final_decision IN ('approved', 'rejected')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX trade_decisions_candidate_id_idx ON trade_decisions (candidate_id, candidate_revision) WHERE deleted_at IS NULL;

CREATE TABLE market_data_cache (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    symbol TEXT NOT NULL,
    venue TEXT NOT NULL,
    data_type TEXT NOT NULL CHECK (data_type IN ('candle', 'orderbook', 'news', 'search')),
    cached_data JSONB NOT NULL,
    fetched_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX market_data_cache_lookup_idx ON market_data_cache (symbol, venue, data_type);

CREATE TABLE trading_orders (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    desk_id UUID NOT NULL REFERENCES trading_desks (id),
    candidate_id UUID REFERENCES trade_candidates (id),
    proposal_id UUID REFERENCES trade_proposals (id),
    client_order_id TEXT NOT NULL,
    venue TEXT NOT NULL,
    symbol TEXT NOT NULL,
    side TEXT NOT NULL CHECK (side IN ('buy', 'sell')),
    order_type TEXT NOT NULL CHECK (order_type IN ('market', 'limit', 'stop_loss', 'take_profit')),
    price DOUBLE PRECISION,
    quantity DOUBLE PRECISION NOT NULL,
    status TEXT NOT NULL DEFAULT 'filled' CHECK (status IN ('pending', 'open', 'filled', 'canceled', 'rejected')),
    idempotency_key TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX trading_orders_desk_id_idx ON trading_orders (desk_id) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX trading_orders_client_order_id_key ON trading_orders (desk_id, client_order_id) WHERE deleted_at IS NULL;

CREATE TABLE trading_fills (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    order_id UUID NOT NULL REFERENCES trading_orders (id),
    desk_id UUID NOT NULL REFERENCES trading_desks (id),
    fill_price DOUBLE PRECISION NOT NULL,
    fill_quantity DOUBLE PRECISION NOT NULL,
    fee DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    fee_currency TEXT NOT NULL DEFAULT 'USD',
    executed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX trading_fills_order_id_idx ON trading_fills (order_id) WHERE deleted_at IS NULL;
CREATE INDEX trading_fills_desk_id_idx ON trading_fills (desk_id) WHERE deleted_at IS NULL;

CREATE TABLE trading_positions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    desk_id UUID NOT NULL REFERENCES trading_desks (id),
    symbol TEXT NOT NULL,
    side TEXT NOT NULL CHECK (side IN ('long', 'short')),
    quantity DOUBLE PRECISION NOT NULL,
    entry_price DOUBLE PRECISION NOT NULL,
    current_price DOUBLE PRECISION NOT NULL,
    unrealized_pnl DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    realized_pnl DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX trading_positions_desk_id_idx ON trading_positions (desk_id) WHERE deleted_at IS NULL;

CREATE TABLE account_snapshots (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    desk_id UUID NOT NULL REFERENCES trading_desks (id),
    balance DOUBLE PRECISION NOT NULL,
    equity DOUBLE PRECISION NOT NULL,
    margin_used DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    free_margin DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    unrealized_pnl DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    realized_pnl DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE INDEX account_snapshots_desk_id_idx ON account_snapshots (desk_id) WHERE deleted_at IS NULL;

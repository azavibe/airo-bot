ALTER TABLE trading_positions ADD COLUMN IF NOT EXISTS stop_loss DOUBLE PRECISION;
ALTER TABLE trading_positions ADD COLUMN IF NOT EXISTS take_profit DOUBLE PRECISION;
ALTER TABLE trading_positions ADD COLUMN IF NOT EXISTS close_price DOUBLE PRECISION;
ALTER TABLE trading_positions ADD COLUMN IF NOT EXISTS closed_at TIMESTAMPTZ;

CREATE TABLE market_news (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    symbol TEXT NOT NULL,
    venue TEXT NOT NULL DEFAULT 'global',
    headline TEXT NOT NULL,
    url TEXT,
    source TEXT NOT NULL,
    sentiment_score DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    published_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    content_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX market_news_content_hash_idx ON market_news (content_hash) WHERE deleted_at IS NULL;
CREATE INDEX market_news_symbol_published_idx ON market_news (symbol, published_at DESC) WHERE deleted_at IS NULL;

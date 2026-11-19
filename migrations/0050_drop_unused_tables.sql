-- The trading desk (migrations 0028-0029) and agent ratings (0035) were never
-- used by any code in this product; nothing reads or writes these tables.
-- Dropped together so tables that reference each other go in one statement.
DROP TABLE IF EXISTS
    trading_fills,
    trading_orders,
    trading_positions,
    trade_decisions,
    trade_proposals,
    agent_reports,
    trade_candidates,
    trading_desks,
    market_data_cache,
    market_news,
    account_snapshots,
    ratings;

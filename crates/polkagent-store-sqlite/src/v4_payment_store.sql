-- Migration V4: Payment store tables.
--
-- Adds tables for tracking payment intents, on-chain receipts, LLM cost
-- records, and daily usage aggregates.

-- ---------------------------------------------------------------------------
-- Payment intents
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS payment_intents (
    id               TEXT    PRIMARY KEY,           -- UUID v7 (stringified)
    agent_id         TEXT    NOT NULL,
    run_id           TEXT    NOT NULL,
    amount_value     TEXT    NOT NULL,              -- u128 stored as decimal text
    amount_asset     TEXT    NOT NULL,              -- JSON-encoded AssetId
    amount_decimals  INTEGER NOT NULL DEFAULT 10,
    recipient        TEXT    NOT NULL,
    idempotency_key  TEXT    NOT NULL UNIQUE,
    status           TEXT    NOT NULL DEFAULT 'pending',
    -- pending | approved | submitted | confirmed | failed | cancelled
    created_at       TEXT    NOT NULL,              -- ISO 8601 UTC
    updated_at       TEXT    NOT NULL               -- ISO 8601 UTC
);

CREATE INDEX IF NOT EXISTS idx_payment_intents_agent
    ON payment_intents(agent_id, created_at);
CREATE INDEX IF NOT EXISTS idx_payment_intents_run
    ON payment_intents(run_id);
CREATE INDEX IF NOT EXISTS idx_payment_intents_status
    ON payment_intents(status);
CREATE INDEX IF NOT EXISTS idx_payment_intents_idempotency
    ON payment_intents(idempotency_key);

-- ---------------------------------------------------------------------------
-- Payment receipts (on-chain confirmations)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS payment_receipts (
    id            TEXT    PRIMARY KEY,              -- UUID v7 (generated on insert)
    intent_id     TEXT    NOT NULL UNIQUE REFERENCES payment_intents(id),
    tx_hash       TEXT    NOT NULL,
    block_number  INTEGER NOT NULL,
    fee_value     TEXT    NOT NULL,                 -- u128 stored as decimal text
    fee_asset     TEXT    NOT NULL,                 -- JSON-encoded AssetId
    fee_decimals  INTEGER NOT NULL DEFAULT 10,
    confirmed_at  TEXT    NOT NULL                  -- ISO 8601 UTC
);

CREATE INDEX IF NOT EXISTS idx_payment_receipts_intent
    ON payment_receipts(intent_id);
CREATE INDEX IF NOT EXISTS idx_payment_receipts_tx_hash
    ON payment_receipts(tx_hash);

-- ---------------------------------------------------------------------------
-- LLM cost records
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS cost_records (
    id              TEXT    PRIMARY KEY,            -- UUID v7 (generated on insert)
    run_id          TEXT    NOT NULL,
    provider        TEXT    NOT NULL,
    model           TEXT    NOT NULL,
    input_tokens    INTEGER NOT NULL DEFAULT 0,
    output_tokens   INTEGER NOT NULL DEFAULT 0,
    estimated_usd   REAL    NOT NULL DEFAULT 0.0,
    recorded_at     TEXT    NOT NULL               -- ISO 8601 UTC
);

CREATE INDEX IF NOT EXISTS idx_cost_records_run
    ON cost_records(run_id);
CREATE INDEX IF NOT EXISTS idx_cost_records_recorded_at
    ON cost_records(recorded_at);

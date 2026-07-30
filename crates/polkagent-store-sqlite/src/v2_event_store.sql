-- Migration V2: Event store tables for the EventStore trait.
--
-- Adds `durable_events` and `diagnostic_events` tables that match the
-- StoredEvent shape defined in polkagent-store-trait.  The original
-- `run_events` table (V1) is left untouched for backward compatibility.

-- ---------------------------------------------------------------------------
-- Durable events (append-only, global-sequenced)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS durable_events (
    id               TEXT    PRIMARY KEY,       -- UUID v7
    event_type       TEXT    NOT NULL,          -- canonical event type name
    sequence         INTEGER NOT NULL,          -- per-run monotonic, 1-based
    global_sequence  INTEGER NOT NULL UNIQUE,   -- auto-assigned, scope-wide total order
    run_id           TEXT    NOT NULL,          -- owning run
    conversation_id  TEXT,                      -- optional conversation scope
    correlation_id   TEXT    NOT NULL,          -- links related events
    causation_id     TEXT,                      -- direct causal parent event
    scope_id         TEXT    NOT NULL,          -- workspace / tenant scope
    timestamp        TEXT    NOT NULL,          -- ISO 8601 UTC
    durability       TEXT    NOT NULL DEFAULT 'durable',
    payload          TEXT    NOT NULL DEFAULT '{}',  -- JSON-encoded event payload
    trace_id         TEXT,                      -- W3C trace ID (32 hex chars)
    span_id          TEXT,                      -- W3C span ID (16 hex chars)
    schema_version   INTEGER NOT NULL DEFAULT 1,
    UNIQUE (run_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_durable_events_run_seq
    ON durable_events(run_id, sequence);
CREATE INDEX IF NOT EXISTS idx_durable_events_global_seq
    ON durable_events(global_sequence);
CREATE INDEX IF NOT EXISTS idx_durable_events_type_ts
    ON durable_events(event_type, timestamp);
CREATE INDEX IF NOT EXISTS idx_durable_events_scope
    ON durable_events(scope_id, global_sequence);

-- ---------------------------------------------------------------------------
-- Diagnostic events (separate table, with expiry for retention)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS diagnostic_events (
    id               TEXT    PRIMARY KEY,       -- UUID v7
    event_type       TEXT    NOT NULL,
    sequence         INTEGER NOT NULL,
    global_sequence  INTEGER NOT NULL,          -- assigned but not enforced as strictly
    run_id           TEXT    NOT NULL,
    conversation_id  TEXT,
    correlation_id   TEXT    NOT NULL,
    causation_id     TEXT,
    scope_id         TEXT    NOT NULL,
    timestamp        TEXT    NOT NULL,          -- ISO 8601 UTC
    durability       TEXT    NOT NULL DEFAULT 'diagnostic',
    payload          TEXT    NOT NULL DEFAULT '{}',
    trace_id         TEXT,
    span_id          TEXT,
    schema_version   INTEGER NOT NULL DEFAULT 1,
    expires_at       TEXT    NOT NULL           -- ISO 8601 UTC retention expiry
);

CREATE INDEX IF NOT EXISTS idx_diagnostic_events_run
    ON diagnostic_events(run_id);
CREATE INDEX IF NOT EXISTS idx_diagnostic_events_expires
    ON diagnostic_events(expires_at);

-- ---------------------------------------------------------------------------
-- Global sequence counter
-- ---------------------------------------------------------------------------
-- A single-row table that tracks the next global_sequence value.
-- Using a dedicated counter avoids SELECT MAX() races under concurrency.

CREATE TABLE IF NOT EXISTS global_sequence_counter (
    id    INTEGER PRIMARY KEY CHECK (id = 1),  -- exactly one row
    value INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO global_sequence_counter (id, value) VALUES (1, 0);

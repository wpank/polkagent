-- Polkagent SQLite schema
-- Applied as a single migration by the migrations module.
-- All timestamps are ISO 8601 text (UTC, produced by chrono::Utc::now().to_rfc3339()).
-- All IDs are UUID v7 text (lowercase hyphenated).

-- ---------------------------------------------------------------------------
-- Schema version tracking
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS schema_migrations (
    version     INTEGER PRIMARY KEY,
    description TEXT    NOT NULL,
    applied_at  TEXT    NOT NULL,   -- ISO 8601
    checksum    TEXT    NOT NULL    -- SHA-256 hex of the migration SQL
);

-- ---------------------------------------------------------------------------
-- Agents
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS agents (
    id          TEXT    PRIMARY KEY,            -- UUID v7
    name        TEXT    NOT NULL,
    description TEXT,
    state       TEXT    NOT NULL DEFAULT 'active',  -- active | disabled | archived
    spec_json   TEXT    NOT NULL DEFAULT '{}',      -- serialised AgentSpec
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agents_name ON agents(name);
CREATE INDEX IF NOT EXISTS idx_agents_state ON agents(state);

-- ---------------------------------------------------------------------------
-- Runs
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS runs (
    id              TEXT    PRIMARY KEY,    -- UUID v7
    agent_id        TEXT    NOT NULL REFERENCES agents(id),
    conversation_id TEXT,
    state           TEXT    NOT NULL DEFAULT 'created',
    -- created | started | working | completed | failed | cancelled | timed_out
    params_json     TEXT    NOT NULL DEFAULT '{}',  -- run parameters
    created_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL,
    completed_at    TEXT                    -- NULL until terminal state
);

CREATE INDEX IF NOT EXISTS idx_runs_by_agent ON runs(agent_id, created_at);
CREATE INDEX IF NOT EXISTS idx_runs_by_state ON runs(state) WHERE state NOT IN ('completed','failed','cancelled','timed_out');

-- ---------------------------------------------------------------------------
-- Turns
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS turns (
    id            TEXT    PRIMARY KEY,   -- UUID v7
    run_id        TEXT    NOT NULL REFERENCES runs(id),
    sequence      INTEGER NOT NULL,      -- monotonic within a run, 1-based
    role          TEXT    NOT NULL,      -- user | assistant | system | tool
    started_at    TEXT    NOT NULL,
    completed_at  TEXT,
    input_tokens  INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_turns_run_sequence ON turns(run_id, sequence);
CREATE INDEX IF NOT EXISTS idx_turns_by_run ON turns(run_id, sequence);

-- ---------------------------------------------------------------------------
-- Steps
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS steps (
    id           TEXT    PRIMARY KEY,   -- UUID v7
    turn_id      TEXT    NOT NULL REFERENCES turns(id),
    sequence     INTEGER NOT NULL,      -- monotonic within a turn, 1-based
    kind         TEXT    NOT NULL,      -- model_call | tool_call | approval | chain_action | ...
    started_at   TEXT    NOT NULL,
    completed_at TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_steps_turn_sequence ON steps(turn_id, sequence);
CREATE INDEX IF NOT EXISTS idx_steps_by_turn ON steps(turn_id, sequence);

-- ---------------------------------------------------------------------------
-- Effect intents
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS effect_intents (
    id              TEXT    PRIMARY KEY,    -- UUID v7
    run_id          TEXT    NOT NULL REFERENCES runs(id),
    turn_id         TEXT    REFERENCES turns(id),
    step_id         TEXT    REFERENCES steps(id),
    kind            TEXT    NOT NULL,       -- model | tool | sign | broadcast | finality | ...
    params_json     TEXT    NOT NULL DEFAULT '{}',
    idempotency_key TEXT    NOT NULL UNIQUE,
    created_at      TEXT    NOT NULL,
    -- Lease tracking (set when an attempt claims this intent)
    claimed_by      TEXT,                  -- worker UUID
    claimed_until   TEXT                   -- ISO 8601 lease expiry; NULL if unclaimed
);

CREATE INDEX IF NOT EXISTS idx_effects_by_run ON effect_intents(run_id);
CREATE INDEX IF NOT EXISTS idx_effects_by_idempotency_key ON effect_intents(idempotency_key);
CREATE INDEX IF NOT EXISTS idx_effects_unclaimed ON effect_intents(created_at)
    WHERE claimed_by IS NULL;

-- ---------------------------------------------------------------------------
-- Effect attempts
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS effect_attempts (
    id             TEXT    PRIMARY KEY,   -- UUID v7
    intent_id      TEXT    NOT NULL REFERENCES effect_intents(id),
    attempt_number INTEGER NOT NULL,
    started_at     TEXT    NOT NULL,
    completed_at   TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_attempts_intent_number ON effect_attempts(intent_id, attempt_number);
CREATE INDEX IF NOT EXISTS idx_attempts_by_intent ON effect_attempts(intent_id);

-- ---------------------------------------------------------------------------
-- Effect outcomes  (immutable — trigger enforces no updates to core fields)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS effect_outcomes (
    id          TEXT    PRIMARY KEY,   -- UUID v7
    intent_id   TEXT    NOT NULL UNIQUE REFERENCES effect_intents(id),
    status      TEXT    NOT NULL,      -- success | failure | timeout | cancelled | unknown
    result_json TEXT    NOT NULL DEFAULT '{}',
    created_at  TEXT    NOT NULL
);

-- Immutability trigger: prevent UPDATE of any core column after insert
CREATE TRIGGER IF NOT EXISTS trg_effect_outcomes_immutable
BEFORE UPDATE ON effect_outcomes
BEGIN
    SELECT RAISE(ABORT, 'effect_outcomes rows are immutable after creation');
END;

-- ---------------------------------------------------------------------------
-- Artifacts
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS artifacts (
    id            TEXT    PRIMARY KEY,   -- UUID v7
    run_id        TEXT    REFERENCES runs(id),
    kind          TEXT    NOT NULL,      -- ArtifactKind enum string
    digest_hex    TEXT    NOT NULL,      -- SHA-256 hex of body
    size_bytes    INTEGER NOT NULL DEFAULT 0,
    metadata_json TEXT    NOT NULL DEFAULT '{}',
    created_at    TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_artifacts_by_run ON artifacts(run_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_by_digest ON artifacts(digest_hex);
CREATE INDEX IF NOT EXISTS idx_artifacts_by_kind ON artifacts(kind, created_at);

-- Immutability trigger: prevent UPDATE of any core column after insert
CREATE TRIGGER IF NOT EXISTS trg_artifacts_immutable
BEFORE UPDATE ON artifacts
BEGIN
    SELECT RAISE(ABORT, 'artifacts rows are immutable after creation');
END;

-- ---------------------------------------------------------------------------
-- Artifact bodies  (content-addressed, deduped by digest)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS artifact_bodies (
    digest_hex  TEXT    PRIMARY KEY,    -- SHA-256 hex — same as artifacts.digest_hex
    body        BLOB    NOT NULL
);

-- ---------------------------------------------------------------------------
-- Artifact lineage  (DAG of parent→child derivation edges)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS artifact_lineage (
    child_id   TEXT NOT NULL REFERENCES artifacts(id),
    parent_id  TEXT NOT NULL REFERENCES artifacts(id),
    PRIMARY KEY (child_id, parent_id)
);

CREATE INDEX IF NOT EXISTS idx_lineage_parent ON artifact_lineage(parent_id);

-- ---------------------------------------------------------------------------
-- Run events  (durable ordered log)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS run_events (
    id             TEXT    PRIMARY KEY,    -- UUID v7
    run_id         TEXT    NOT NULL REFERENCES runs(id),
    sequence       INTEGER NOT NULL,       -- per-run monotonic, 1-based
    kind           TEXT    NOT NULL,       -- RunCreated | TurnStarted | EffectIntentCreated | ...
    data_json      TEXT    NOT NULL DEFAULT '{}',
    timestamp      TEXT    NOT NULL,       -- ISO 8601
    correlation_id TEXT,
    schema_version INTEGER NOT NULL DEFAULT 1,
    UNIQUE (run_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_events_by_run_sequence ON run_events(run_id, sequence);
CREATE INDEX IF NOT EXISTS idx_events_by_kind ON run_events(kind, timestamp);

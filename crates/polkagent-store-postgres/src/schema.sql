-- Polkagent PostgreSQL schema with Row-Level Security for multi-tenant isolation.
--
-- Every tenant-scoped table includes a `tenant_id` column. RLS policies
-- ensure that queries only see rows belonging to the current tenant,
-- identified by the session variable `app.tenant_id`.
--
-- Usage:
--   SET LOCAL app.tenant_id = '<tenant-uuid>';
--   -- all subsequent queries in this transaction are scoped to that tenant.

-- =========================================================================
-- Extensions
-- =========================================================================
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- =========================================================================
-- Schema migrations tracking
-- =========================================================================
CREATE TABLE IF NOT EXISTS schema_migrations (
    version     INTEGER PRIMARY KEY,
    applied_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    checksum    TEXT
);

-- =========================================================================
-- Agents
-- =========================================================================
CREATE TABLE IF NOT EXISTS agents (
    id          TEXT        PRIMARY KEY,
    tenant_id   TEXT        NOT NULL,
    name        TEXT        NOT NULL DEFAULT '',
    state       TEXT        NOT NULL DEFAULT 'active',
    spec_json   TEXT        NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_agents_tenant ON agents (tenant_id);

ALTER TABLE agents ENABLE ROW LEVEL SECURITY;
ALTER TABLE agents FORCE ROW LEVEL SECURITY;
CREATE POLICY agents_tenant_isolation ON agents
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- =========================================================================
-- Runs
-- =========================================================================
CREATE TABLE IF NOT EXISTS runs (
    id           TEXT        PRIMARY KEY,
    tenant_id    TEXT        NOT NULL,
    agent_id     TEXT        NOT NULL REFERENCES agents(id),
    state        TEXT        NOT NULL DEFAULT 'created',
    params_json  TEXT        NOT NULL DEFAULT '{}',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at   TIMESTAMPTZ,
    completed_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_runs_tenant ON runs (tenant_id);
CREATE INDEX IF NOT EXISTS idx_runs_agent  ON runs (agent_id);
CREATE INDEX IF NOT EXISTS idx_runs_state  ON runs (state);

ALTER TABLE runs ENABLE ROW LEVEL SECURITY;
ALTER TABLE runs FORCE ROW LEVEL SECURITY;
CREATE POLICY runs_tenant_isolation ON runs
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- =========================================================================
-- Turns
-- =========================================================================
CREATE TABLE IF NOT EXISTS turns (
    id            TEXT        PRIMARY KEY,
    tenant_id     TEXT        NOT NULL,
    run_id        TEXT        NOT NULL REFERENCES runs(id),
    sequence      INTEGER     NOT NULL,
    role          TEXT        NOT NULL,
    started_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at  TIMESTAMPTZ,
    input_tokens  INTEGER     NOT NULL DEFAULT 0,
    output_tokens INTEGER     NOT NULL DEFAULT 0,
    UNIQUE (run_id, sequence)
);
CREATE INDEX IF NOT EXISTS idx_turns_tenant ON turns (tenant_id);

ALTER TABLE turns ENABLE ROW LEVEL SECURITY;
ALTER TABLE turns FORCE ROW LEVEL SECURITY;
CREATE POLICY turns_tenant_isolation ON turns
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- =========================================================================
-- Steps
-- =========================================================================
CREATE TABLE IF NOT EXISTS steps (
    id          TEXT        PRIMARY KEY,
    tenant_id   TEXT        NOT NULL,
    turn_id     TEXT        NOT NULL REFERENCES turns(id),
    sequence    INTEGER     NOT NULL,
    kind        TEXT        NOT NULL,
    started_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    UNIQUE (turn_id, sequence)
);
CREATE INDEX IF NOT EXISTS idx_steps_tenant ON steps (tenant_id);

ALTER TABLE steps ENABLE ROW LEVEL SECURITY;
ALTER TABLE steps FORCE ROW LEVEL SECURITY;
CREATE POLICY steps_tenant_isolation ON steps
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- =========================================================================
-- Effect Intents
-- =========================================================================
CREATE TABLE IF NOT EXISTS effect_intents (
    id              TEXT        PRIMARY KEY,
    tenant_id       TEXT        NOT NULL,
    run_id          TEXT        NOT NULL REFERENCES runs(id),
    turn_id         TEXT,
    step_id         TEXT,
    kind            TEXT        NOT NULL,
    params_json     TEXT        NOT NULL DEFAULT '{}',
    idempotency_key TEXT        NOT NULL,
    claimed_by      TEXT,
    claimed_until   TIMESTAMPTZ,
    priority        INTEGER     NOT NULL DEFAULT 1,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_effects_tenant ON effect_intents (tenant_id);
CREATE INDEX IF NOT EXISTS idx_effects_run    ON effect_intents (run_id);
CREATE INDEX IF NOT EXISTS idx_effects_claim  ON effect_intents (claimed_by) WHERE claimed_by IS NULL;
CREATE INDEX IF NOT EXISTS idx_effects_idempotency ON effect_intents (run_id, idempotency_key);

ALTER TABLE effect_intents ENABLE ROW LEVEL SECURITY;
ALTER TABLE effect_intents FORCE ROW LEVEL SECURITY;
CREATE POLICY effect_intents_tenant_isolation ON effect_intents
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- =========================================================================
-- Effect Attempts
-- =========================================================================
CREATE TABLE IF NOT EXISTS effect_attempts (
    id          TEXT        PRIMARY KEY,
    tenant_id   TEXT        NOT NULL,
    intent_id   TEXT        NOT NULL REFERENCES effect_intents(id),
    worker_id   TEXT        NOT NULL,
    payload     JSONB       NOT NULL DEFAULT '{}',
    started_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_attempts_tenant ON effect_attempts (tenant_id);

ALTER TABLE effect_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE effect_attempts FORCE ROW LEVEL SECURITY;
CREATE POLICY effect_attempts_tenant_isolation ON effect_attempts
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- =========================================================================
-- Effect Outcomes
-- =========================================================================
CREATE TABLE IF NOT EXISTS effect_outcomes (
    id          TEXT        PRIMARY KEY,
    tenant_id   TEXT        NOT NULL,
    intent_id   TEXT        NOT NULL REFERENCES effect_intents(id),
    attempt_id  TEXT        NOT NULL,
    run_id      TEXT        NOT NULL REFERENCES runs(id),
    consumed    BOOLEAN     NOT NULL DEFAULT FALSE,
    payload     JSONB       NOT NULL DEFAULT '{}',
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (intent_id)
);
CREATE INDEX IF NOT EXISTS idx_outcomes_tenant ON effect_outcomes (tenant_id);
CREATE INDEX IF NOT EXISTS idx_outcomes_run    ON effect_outcomes (run_id);

ALTER TABLE effect_outcomes ENABLE ROW LEVEL SECURITY;
ALTER TABLE effect_outcomes FORCE ROW LEVEL SECURITY;
CREATE POLICY effect_outcomes_tenant_isolation ON effect_outcomes
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- =========================================================================
-- Durable Events
-- =========================================================================
CREATE TABLE IF NOT EXISTS durable_events (
    global_sequence BIGSERIAL   PRIMARY KEY,
    id              TEXT        NOT NULL UNIQUE,
    tenant_id       TEXT        NOT NULL,
    run_id          TEXT        NOT NULL,
    sequence        BIGINT      NOT NULL,
    kind            TEXT        NOT NULL,
    data_json       JSONB       NOT NULL DEFAULT '{}',
    timestamp       TIMESTAMPTZ NOT NULL DEFAULT now(),
    correlation_id  TEXT,
    schema_version  INTEGER     NOT NULL DEFAULT 1,
    UNIQUE (run_id, sequence)
);
CREATE INDEX IF NOT EXISTS idx_durable_events_tenant ON durable_events (tenant_id);
CREATE INDEX IF NOT EXISTS idx_durable_events_run    ON durable_events (run_id);

ALTER TABLE durable_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE durable_events FORCE ROW LEVEL SECURITY;
CREATE POLICY durable_events_tenant_isolation ON durable_events
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- =========================================================================
-- Diagnostic Events
-- =========================================================================
CREATE TABLE IF NOT EXISTS diagnostic_events (
    id              TEXT        PRIMARY KEY,
    tenant_id       TEXT        NOT NULL,
    run_id          TEXT        NOT NULL,
    sequence        BIGINT      NOT NULL,
    kind            TEXT        NOT NULL,
    data_json       JSONB       NOT NULL DEFAULT '{}',
    timestamp       TIMESTAMPTZ NOT NULL DEFAULT now(),
    correlation_id  TEXT,
    schema_version  INTEGER     NOT NULL DEFAULT 1,
    expires_at      TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_diag_events_tenant ON diagnostic_events (tenant_id);

ALTER TABLE diagnostic_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE diagnostic_events FORCE ROW LEVEL SECURITY;
CREATE POLICY diagnostic_events_tenant_isolation ON diagnostic_events
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- =========================================================================
-- Artifacts
-- =========================================================================
CREATE TABLE IF NOT EXISTS artifacts (
    id              TEXT        PRIMARY KEY,
    tenant_id       TEXT        NOT NULL,
    run_id          TEXT,
    kind            TEXT        NOT NULL,
    algorithm       TEXT        NOT NULL DEFAULT 'blake3',
    digest_hex      TEXT        NOT NULL,
    classification  TEXT        NOT NULL DEFAULT 'public',
    size_bytes      BIGINT      NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_artifacts_tenant ON artifacts (tenant_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_run    ON artifacts (run_id);

ALTER TABLE artifacts ENABLE ROW LEVEL SECURITY;
ALTER TABLE artifacts FORCE ROW LEVEL SECURITY;
CREATE POLICY artifacts_tenant_isolation ON artifacts
    USING (tenant_id = current_setting('app.tenant_id', true))
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true));

-- =========================================================================
-- Artifact Bodies (content-addressed, shared across tenants)
-- =========================================================================
CREATE TABLE IF NOT EXISTS artifact_bodies (
    digest_hex  TEXT    PRIMARY KEY,
    body        BYTEA   NOT NULL
);
-- No RLS on artifact_bodies: content-addressed and shared.
-- Access is always mediated through the tenant-scoped `artifacts` table.

-- =========================================================================
-- Insert migration record
-- =========================================================================
INSERT INTO schema_migrations (version, checksum) VALUES (1, 'initial')
ON CONFLICT (version) DO NOTHING;

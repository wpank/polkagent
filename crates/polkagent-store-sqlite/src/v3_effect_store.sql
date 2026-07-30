-- Migration V3: Add columns required by the EffectStore trait.
--
-- The EffectStore trait (polkagent-store-trait) expects richer metadata on
-- intents, attempts, and outcomes than the original V1 schema provides.
-- This migration adds the missing columns with defaults that preserve
-- backward compatibility with existing data.

-- ---------------------------------------------------------------------------
-- effect_intents: add state and retry_class columns
-- ---------------------------------------------------------------------------

ALTER TABLE effect_intents ADD COLUMN state TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE effect_intents ADD COLUMN retry_class TEXT NOT NULL DEFAULT 'idempotent';

CREATE INDEX IF NOT EXISTS idx_effects_pending
    ON effect_intents(created_at) WHERE state = 'pending';

-- ---------------------------------------------------------------------------
-- effect_attempts: add worker_id and payload_json columns
-- ---------------------------------------------------------------------------

ALTER TABLE effect_attempts ADD COLUMN worker_id TEXT;
ALTER TABLE effect_attempts ADD COLUMN payload_json TEXT NOT NULL DEFAULT '{}';

-- ---------------------------------------------------------------------------
-- effect_outcomes: add attempt_id, run_id, and consumed columns
-- ---------------------------------------------------------------------------

ALTER TABLE effect_outcomes ADD COLUMN attempt_id TEXT REFERENCES effect_attempts(id);
ALTER TABLE effect_outcomes ADD COLUMN run_id TEXT;
ALTER TABLE effect_outcomes ADD COLUMN consumed INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_outcomes_unconsumed
    ON effect_outcomes(run_id) WHERE consumed = 0;

-- Replace the blanket immutability trigger with one that only allows
-- updating the consumed flag.  Core columns remain immutable.
DROP TRIGGER IF EXISTS trg_effect_outcomes_immutable;

CREATE TRIGGER trg_effect_outcomes_immutable
BEFORE UPDATE ON effect_outcomes
WHEN (
    NEW.id          != OLD.id          OR
    NEW.intent_id   != OLD.intent_id   OR
    NEW.status      != OLD.status      OR
    NEW.result_json != OLD.result_json OR
    NEW.created_at  != OLD.created_at
)
BEGIN
    SELECT RAISE(ABORT, 'effect_outcomes core columns are immutable after creation');
END;

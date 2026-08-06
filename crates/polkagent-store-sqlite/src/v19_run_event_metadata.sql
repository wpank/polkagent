-- Migration V19: preserve the complete persisted RunEvent/EventEnvelope
-- metadata carried by StoredEvent.
--
-- V1's run_events table retained only correlation_id. Keep every legacy row
-- and its rowid (the public global_sequence cursor), adding nullable metadata
-- and truthful defaults for fields that did not exist historically.

ALTER TABLE run_events ADD COLUMN conversation_id TEXT;
ALTER TABLE run_events ADD COLUMN causation_id TEXT;
ALTER TABLE run_events ADD COLUMN scope_id TEXT NOT NULL DEFAULT '';
ALTER TABLE run_events ADD COLUMN durability TEXT NOT NULL DEFAULT 'durable'
    CHECK (durability IN ('durable', 'diagnostic', 'ephemeral'));
ALTER TABLE run_events ADD COLUMN trace_id TEXT;
ALTER TABLE run_events ADD COLUMN span_id TEXT;

-- EventCorrelation component identifiers from the canonical RunEvent.
-- correlation.run_id is already the required run_id column.
ALTER TABLE run_events ADD COLUMN turn_id TEXT;
ALTER TABLE run_events ADD COLUMN step_id TEXT;
ALTER TABLE run_events ADD COLUMN effect_intent_id TEXT;
ALTER TABLE run_events ADD COLUMN effect_attempt_id TEXT;

-- Legacy diagnostic records were distinguished only by their kind prefix.
UPDATE run_events
SET durability = 'diagnostic'
WHERE kind LIKE 'diagnostic:%';

CREATE INDEX IF NOT EXISTS idx_events_by_correlation
    ON run_events(correlation_id);
CREATE INDEX IF NOT EXISTS idx_events_by_conversation
    ON run_events(conversation_id)
    WHERE conversation_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_events_by_scope
    ON run_events(scope_id);
CREATE INDEX IF NOT EXISTS idx_events_by_turn
    ON run_events(turn_id)
    WHERE turn_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_events_by_step
    ON run_events(step_id)
    WHERE step_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_events_by_effect_intent
    ON run_events(effect_intent_id)
    WHERE effect_intent_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_events_by_effect_attempt
    ON run_events(effect_attempt_id)
    WHERE effect_attempt_id IS NOT NULL;

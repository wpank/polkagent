-- V16: make immutable effect-outcome lineage exact.
--
-- V3 added attempt_id and run_id after the original immutability trigger was
-- written, but its replacement trigger did not protect those new columns.
-- Recreate it with NULL-safe comparisons so only consumed may change.

DROP TRIGGER IF EXISTS trg_effect_outcomes_immutable;

CREATE TRIGGER trg_effect_outcomes_immutable
BEFORE UPDATE ON effect_outcomes
WHEN (
    NEW.id          IS NOT OLD.id          OR
    NEW.intent_id   IS NOT OLD.intent_id   OR
    NEW.status      IS NOT OLD.status      OR
    NEW.result_json IS NOT OLD.result_json OR
    NEW.created_at  IS NOT OLD.created_at  OR
    NEW.attempt_id  IS NOT OLD.attempt_id  OR
    NEW.run_id      IS NOT OLD.run_id
)
BEGIN
    SELECT RAISE(ABORT, 'effect_outcomes core columns and lineage are immutable after creation');
END;

-- Migration V9: Add priority column to effect_intents.
--
-- The EffectPipeline stores priority on every intent, but the column was
-- missing from the table.  Workers now ORDER BY priority DESC so that
-- Critical/High intents are claimed before Normal/Low ones.

ALTER TABLE effect_intents ADD COLUMN priority INTEGER NOT NULL DEFAULT 1;
-- 0 = Low, 1 = Normal, 2 = High, 3 = Critical

CREATE INDEX IF NOT EXISTS idx_effects_priority
    ON effect_intents(priority DESC, created_at ASC)
    WHERE claimed_by IS NULL;

-- Migration V7: Feed store tables.
--
-- Adds tables for durable feed processing: feeds, triggers, recipes, and
-- feed item queues. Supports cursor-backed at-least-once delivery, trigger
-- cooldown tracking, and reusable recipe blueprints.

-- ---------------------------------------------------------------------------
-- Feeds
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS feeds (
    id                        TEXT    PRIMARY KEY,              -- UUID v7
    name                      TEXT    NOT NULL,                 -- human-readable name
    source_json               TEXT    NOT NULL,                 -- JSON-encoded FeedSource
    cursor_position           TEXT    NOT NULL DEFAULT '',      -- opaque position string
    cursor_last_processed_at  TEXT    NOT NULL,                 -- ISO 8601 UTC
    cursor_items_processed    INTEGER NOT NULL DEFAULT 0,       -- total items processed
    status                    TEXT    NOT NULL DEFAULT 'active',-- active | paused | error:...
    agent_id                  TEXT    NOT NULL,                 -- owning agent UUID
    created_at                TEXT    NOT NULL                  -- ISO 8601 UTC
);

CREATE INDEX IF NOT EXISTS idx_feeds_agent_id
    ON feeds(agent_id);

CREATE INDEX IF NOT EXISTS idx_feeds_status
    ON feeds(status);

-- ---------------------------------------------------------------------------
-- Feed Triggers
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS feed_triggers (
    id              TEXT    PRIMARY KEY,                        -- UUID v7
    name            TEXT    NOT NULL,                          -- human-readable name
    feed_id         TEXT    NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    condition_json  TEXT    NOT NULL,                          -- JSON-encoded TriggerCondition
    action_json     TEXT    NOT NULL,                          -- JSON-encoded TriggerAction
    cooldown_secs   INTEGER,                                   -- NULL means no cooldown
    last_fired_at   TEXT,                                      -- ISO 8601 UTC, NULL if never fired
    enabled         INTEGER NOT NULL DEFAULT 1                 -- 1 = enabled, 0 = disabled
);

CREATE INDEX IF NOT EXISTS idx_feed_triggers_feed_id
    ON feed_triggers(feed_id);

-- ---------------------------------------------------------------------------
-- Feed Recipes
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS feed_recipes (
    id               TEXT    PRIMARY KEY,                      -- UUID v7
    name             TEXT    NOT NULL,                         -- human-readable name
    description      TEXT    NOT NULL DEFAULT '',              -- longer description
    version          TEXT    NOT NULL DEFAULT '1.0.0',         -- semantic version
    source_json      TEXT    NOT NULL,                         -- JSON-encoded FeedSource template
    trigger_json     TEXT    NOT NULL,                         -- JSON-encoded TriggerCondition template
    action_json      TEXT    NOT NULL,                         -- JSON-encoded TriggerAction template
    parameters_json  TEXT    NOT NULL DEFAULT '[]'             -- JSON-encoded Vec<RecipeParameter>
);

-- ---------------------------------------------------------------------------
-- Feed Items (queue)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS feed_items (
    id           TEXT    PRIMARY KEY,                          -- UUID v7
    feed_id      TEXT    NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    payload_json TEXT    NOT NULL,                             -- JSON-encoded payload
    received_at  TEXT    NOT NULL,                             -- ISO 8601 UTC (defines FIFO order)
    processed    INTEGER NOT NULL DEFAULT 0                    -- 0 = pending, 1 = processed
);

CREATE INDEX IF NOT EXISTS idx_feed_items_feed_id
    ON feed_items(feed_id);

CREATE INDEX IF NOT EXISTS idx_feed_items_processed
    ON feed_items(processed);

CREATE INDEX IF NOT EXISTS idx_feed_items_feed_processed
    ON feed_items(feed_id, processed, received_at);

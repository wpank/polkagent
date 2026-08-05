-- Migration V14: safe durable configuration and lifecycle for interactions.
--
-- The existing conversations table remains authoritative for presentation
-- title and transcript messages. This one-to-one projection stores only safe
-- defaults and lifecycle state; credentials never belong here.

CREATE TABLE IF NOT EXISTS interaction_sessions (
    conversation_id  TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    config_json      TEXT NOT NULL,
    state            TEXT NOT NULL CHECK (state IN ('active', 'archived')),
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_interaction_sessions_recency
    ON interaction_sessions(state, updated_at DESC, conversation_id DESC);

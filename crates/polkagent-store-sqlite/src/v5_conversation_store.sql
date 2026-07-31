-- Migration V5: Conversation store tables.
--
-- Adds tables for persistent conversation sessions and their messages.
-- Supports token-count tracking per message and chronological ordering.

-- ---------------------------------------------------------------------------
-- Conversations
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS conversations (
    id            TEXT    PRIMARY KEY,              -- UUID v7
    agent_id      TEXT    NOT NULL,
    title         TEXT,                             -- optional human-readable title
    message_count INTEGER NOT NULL DEFAULT 0,
    metadata_json TEXT    NOT NULL DEFAULT '{}',    -- JSON key-value metadata
    created_at    TEXT    NOT NULL,                 -- ISO 8601 UTC
    updated_at    TEXT    NOT NULL                  -- ISO 8601 UTC
);

CREATE INDEX IF NOT EXISTS idx_conversations_agent
    ON conversations(agent_id, updated_at);

-- ---------------------------------------------------------------------------
-- Messages
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS conversation_messages (
    id               TEXT    PRIMARY KEY,           -- UUID v7
    conversation_id  TEXT    NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role             TEXT    NOT NULL,              -- user | assistant | system | tool
    content_json     TEXT    NOT NULL,              -- JSON-encoded MessageContent
    token_count      INTEGER,                       -- NULL means unknown/unset
    created_at       TEXT    NOT NULL               -- ISO 8601 UTC (defines order)
);

CREATE INDEX IF NOT EXISTS idx_messages_conversation_created
    ON conversation_messages(conversation_id, created_at);

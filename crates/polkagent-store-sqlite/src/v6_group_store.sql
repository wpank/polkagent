-- Migration V6: Group store tables.
--
-- Adds tables for multi-agent group coordination: persistent group records
-- and their membership entries, as defined by the polkagent-group crate.

-- ---------------------------------------------------------------------------
-- Groups
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS groups (
    id               TEXT    PRIMARY KEY,              -- UUID v7
    name             TEXT    NOT NULL,
    description      TEXT    NOT NULL DEFAULT '',
    owner_agent_id   TEXT    NOT NULL,
    quorum_policy    TEXT    NOT NULL DEFAULT 'majority',  -- JSON (tagged enum)
    budget_json      TEXT    NOT NULL DEFAULT '{}',    -- JSON-serialized GroupBudget fields
    created_at       TEXT    NOT NULL,                 -- ISO 8601 UTC
    updated_at       TEXT    NOT NULL                  -- ISO 8601 UTC
);

CREATE INDEX IF NOT EXISTS idx_groups_owner
    ON groups(owner_agent_id);

-- ---------------------------------------------------------------------------
-- Group members
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS group_members (
    group_id           TEXT    NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    agent_id           TEXT    NOT NULL,
    role               TEXT    NOT NULL DEFAULT 'worker',      -- leader | worker | observer
    grant_override_json TEXT,                                  -- NULL or JSON GrantSpec
    joined_at          TEXT    NOT NULL,                       -- ISO 8601 UTC
    PRIMARY KEY (group_id, agent_id)
);

CREATE INDEX IF NOT EXISTS idx_group_members_agent
    ON group_members(agent_id);

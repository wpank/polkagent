-- Migration V8: Skill registry table.
--
-- Adds the `skills` table used by `polkagent skill` CLI subcommands to
-- persist installed skill manifests.

-- ---------------------------------------------------------------------------
-- Skills
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS skills (
    name            TEXT    PRIMARY KEY,              -- unique skill identifier
    version         TEXT    NOT NULL DEFAULT '0.1.0',
    description     TEXT    NOT NULL DEFAULT '',
    path            TEXT    NOT NULL,                 -- filesystem path to skill directory / manifest
    manifest_json   TEXT    NOT NULL DEFAULT '{}',    -- raw manifest content (JSON-encoded)
    installed_at    TEXT    NOT NULL,                 -- ISO 8601 UTC
    updated_at      TEXT    NOT NULL                  -- ISO 8601 UTC
);

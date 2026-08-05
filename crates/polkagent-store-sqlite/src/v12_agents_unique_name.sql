-- V12: Enforce unique agent names.
--
-- The original schema only created a plain index on agents(name), allowing
-- duplicate names to be inserted.  This migration replaces it with a UNIQUE
-- index so the database enforces name uniqueness independently of the
-- application layer.
--
-- SQLite does not support ALTER INDEX, so we drop the old plain index first
-- and then create the new unique index in its place.
DROP INDEX IF EXISTS idx_agents_name;
CREATE UNIQUE INDEX IF NOT EXISTS idx_agents_name ON agents(name);

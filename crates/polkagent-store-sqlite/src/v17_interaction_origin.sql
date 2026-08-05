-- V17: persist immutable working-directory provenance for interactions.
--
-- Existing rows remain NULL because their origin cannot be reconstructed
-- truthfully. Runtime/editor verification fails closed for those legacy rows,
-- while generic read and archive operations remain available.

ALTER TABLE interaction_sessions
    ADD COLUMN origin_working_directory TEXT;

CREATE TRIGGER trg_interaction_origin_required
BEFORE INSERT ON interaction_sessions
WHEN NEW.origin_working_directory IS NULL OR NEW.origin_working_directory = ''
BEGIN
    SELECT RAISE(ABORT, 'interaction origin working directory is required');
END;

CREATE TRIGGER trg_interaction_origin_immutable
BEFORE UPDATE OF origin_working_directory ON interaction_sessions
WHEN NEW.origin_working_directory IS NOT OLD.origin_working_directory
BEGIN
    SELECT RAISE(ABORT, 'interaction origin working directory is immutable');
END;

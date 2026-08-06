-- V20: normalize legacy approval coordinator event identifiers to UUIDs.
--
-- V18 used `approval:<approval-id>:<kind>` strings for otherwise durable
-- run-event identities. V19 began validating the complete event envelope and
-- correctly rejected those non-UUID identifiers. Derive the same stable UUID
-- form used by the coordinator now, preserving each event rowid/global cursor.

DROP TRIGGER approval_requests_immutable_lineage;
DROP TRIGGER approval_requests_validate_transition;

UPDATE run_events
SET id = CASE
    WHEN id LIKE 'approval:%:requested'
        THEN 'a1' || substr(id, 12, 34)
    WHEN id LIKE 'approval:%:resolved'
        THEN 'a2' || substr(id, 12, 34)
    WHEN id LIKE 'approval:%:effects-resolved'
        THEN 'a3' || substr(id, 12, 34)
    ELSE id
END
WHERE id LIKE 'approval:%';

UPDATE approval_requests
SET requested_event_id = 'a1' || substr(id, 3),
    decision_event_id = CASE
        WHEN decision_event_id IS NULL THEN NULL
        ELSE 'a2' || substr(id, 3)
    END
WHERE requested_event_id LIKE 'approval:%'
   OR decision_event_id LIKE 'approval:%';

CREATE TRIGGER approval_requests_immutable_lineage
BEFORE UPDATE ON approval_requests
WHEN
    NEW.id IS NOT OLD.id
    OR NEW.effect_id IS NOT OLD.effect_id
    OR NEW.run_id IS NOT OLD.run_id
    OR NEW.turn_id IS NOT OLD.turn_id
    OR NEW.conversation_id IS NOT OLD.conversation_id
    OR NEW.agent_id IS NOT OLD.agent_id
    OR NEW.subject_schema_version IS NOT OLD.subject_schema_version
    OR NEW.subject_digest IS NOT OLD.subject_digest
    OR NEW.subject_json IS NOT OLD.subject_json
    OR NEW.policy_snapshot_digest IS NOT OLD.policy_snapshot_digest
    OR NEW.tenant_id IS NOT OLD.tenant_id
    OR NEW.workspace_id IS NOT OLD.workspace_id
    OR NEW.authorized_principal_id IS NOT OLD.authorized_principal_id
    OR NEW.title IS NOT OLD.title
    OR NEW.description IS NOT OLD.description
    OR NEW.request_reason IS NOT OLD.request_reason
    OR NEW.deadline_at IS NOT OLD.deadline_at
    OR NEW.requested_at IS NOT OLD.requested_at
    OR NEW.requested_event_id IS NOT OLD.requested_event_id
BEGIN
    SELECT RAISE(ABORT, 'approval request lineage is immutable');
END;

CREATE TRIGGER approval_requests_validate_transition
BEFORE UPDATE ON approval_requests
WHEN NOT (
    (OLD.status = 'pending' AND NEW.status != 'pending')
    OR (
        OLD.status = NEW.status
        AND NEW.decided_by IS OLD.decided_by
        AND NEW.principal_type IS OLD.principal_type
        AND NEW.decision_surface IS OLD.decision_surface
        AND NEW.rationale IS OLD.rationale
        AND NEW.conditions_json IS OLD.conditions_json
        AND NEW.decision IS OLD.decision
        AND NEW.decision_run_state_version IS OLD.decision_run_state_version
        AND NEW.decided_at IS OLD.decided_at
        AND NEW.decision_event_id IS OLD.decision_event_id
    )
)
BEGIN
    SELECT RAISE(ABORT, 'approval decision is immutable');
END;

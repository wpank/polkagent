-- V18: durable approval/checkpoint coordinator foundation.
--
-- This migration makes effect_intents.state authoritative, removes legacy
-- claimed_by state sentinels, adds exact run CAS versions, and introduces the
-- approval/checkpoint records required for an atomic pause-and-resume boundary.

ALTER TABLE runs ADD COLUMN state_version INTEGER NOT NULL DEFAULT 0
    CHECK (state_version >= 0);

CREATE TABLE approval_requests (
    id                       TEXT PRIMARY KEY,
    effect_id                TEXT NOT NULL UNIQUE REFERENCES effect_intents(id),
    run_id                   TEXT NOT NULL REFERENCES runs(id),
    turn_id                  TEXT NOT NULL REFERENCES turns(id),
    conversation_id          TEXT NOT NULL,
    agent_id                 TEXT NOT NULL REFERENCES agents(id),
    subject_schema_version   INTEGER NOT NULL CHECK (subject_schema_version = 1),
    subject_digest           TEXT NOT NULL,
    subject_json             TEXT NOT NULL,
    policy_snapshot_digest   TEXT NOT NULL,
    tenant_id                TEXT NOT NULL,
    workspace_id             TEXT NOT NULL,
    authorized_principal_id  TEXT NOT NULL,
    title                    TEXT NOT NULL,
    description              TEXT NOT NULL,
    request_reason           TEXT NOT NULL,
    status                   TEXT NOT NULL CHECK (
        status IN ('pending', 'approved', 'denied', 'expired', 'cancelled')
    ),
    deadline_at              TEXT NOT NULL,
    requested_at             TEXT NOT NULL,
    decided_by               TEXT,
    principal_type           TEXT CHECK (
        principal_type IS NULL OR principal_type IN ('human', 'service', 'quorum')
    ),
    decision_surface         TEXT,
    rationale                TEXT,
    conditions_json          TEXT NOT NULL DEFAULT '[]',
    decision                 TEXT CHECK (
        decision IS NULL OR decision IN ('allow_once', 'reject_once', 'expire', 'cancel')
    ),
    decided_at               TEXT,
    requested_event_id       TEXT NOT NULL UNIQUE,
    decision_event_id        TEXT UNIQUE,
    CHECK (
        (status = 'pending' AND decided_by IS NULL AND principal_type IS NULL
            AND decision_surface IS NULL AND decision IS NULL AND decided_at IS NULL
            AND decision_event_id IS NULL)
        OR
        (status != 'pending' AND decided_by IS NOT NULL AND principal_type IS NOT NULL
            AND decision_surface IS NOT NULL AND decision IS NOT NULL AND decided_at IS NOT NULL
            AND decision_event_id IS NOT NULL)
    ),
    CHECK (
        (status = 'pending' AND decision IS NULL)
        OR (status = 'approved' AND decision = 'allow_once')
        OR (status = 'denied' AND decision = 'reject_once')
        OR (status = 'expired' AND decision = 'expire')
        OR (status = 'cancelled' AND decision = 'cancel')
    )
);

CREATE INDEX idx_approval_pending_scope
    ON approval_requests(
        tenant_id, workspace_id, authorized_principal_id,
        conversation_id, requested_at, id
    )
    WHERE status = 'pending';
CREATE INDEX idx_approval_run ON approval_requests(run_id, requested_at, id);

ALTER TABLE effect_intents ADD COLUMN approval_id TEXT
    REFERENCES approval_requests(id);
CREATE UNIQUE INDEX idx_effects_approval
    ON effect_intents(approval_id) WHERE approval_id IS NOT NULL;

CREATE TABLE execution_checkpoints (
    run_id                    TEXT PRIMARY KEY REFERENCES runs(id),
    turn_id                   TEXT NOT NULL REFERENCES turns(id),
    conversation_id           TEXT NOT NULL,
    agent_id                  TEXT NOT NULL REFERENCES agents(id),
    schema_version            INTEGER NOT NULL CHECK (schema_version = 1),
    version                   INTEGER NOT NULL CHECK (version > 0),
    status                    TEXT NOT NULL CHECK (
        status IN ('paused_for_approval', 'resumable', 'leased', 'terminal')
    ),
    checkpoint_json           TEXT NOT NULL,
    integrity_digest          TEXT NOT NULL,
    classification            TEXT NOT NULL CHECK (
        classification IN ('public', 'internal', 'private', 'sensitive', 'secret_forbidden')
    ),
    deadline_at               TEXT,
    retention_expires_at      TEXT,
    lease_owner               TEXT,
    lease_expires_at          TEXT,
    created_at                TEXT NOT NULL,
    updated_at                TEXT NOT NULL,
    CHECK (
        (status = 'leased' AND lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR
        (status != 'leased' AND lease_owner IS NULL AND lease_expires_at IS NULL)
    )
);

CREATE INDEX idx_checkpoints_resumable
    ON execution_checkpoints(status, updated_at, run_id)
    WHERE status = 'resumable';
CREATE INDEX idx_checkpoints_expired_lease
    ON execution_checkpoints(lease_expires_at, run_id)
    WHERE status = 'leased';

-- V3 created state with a default of pending, but legacy implementations
-- encoded actual state in claimed_by. Derive only rows still carrying that
-- untouched legacy default. Unknown explicit states are preserved and remain
-- unclaimable rather than being reinterpreted as pending.
UPDATE effect_intents
SET state = CASE
    WHEN EXISTS (
        SELECT 1 FROM effect_outcomes outcome WHERE outcome.intent_id = effect_intents.id
    ) THEN 'resolved'
    WHEN claimed_by = 'resolved' THEN 'resolved'
    WHEN claimed_by = 'failed' THEN 'failed'
    WHEN claimed_by = 'permanently_failed' THEN 'permanently_failed'
    WHEN claimed_by IS NOT NULL THEN 'claimed'
    ELSE 'pending'
END
WHERE state = 'pending';

-- claimed_by now contains only a worker identity or NULL.
UPDATE effect_intents
SET claimed_by = NULL, claimed_until = NULL
WHERE claimed_by IN ('resolved', 'failed', 'permanently_failed');

DROP INDEX IF EXISTS idx_effects_unclaimed;
DROP INDEX IF EXISTS idx_effects_pending;
DROP INDEX IF EXISTS idx_effects_priority;

CREATE INDEX idx_effects_pending
    ON effect_intents(priority DESC, created_at ASC, id)
    WHERE state = 'pending' AND claimed_by IS NULL;

CREATE TRIGGER effect_intents_validate_state_insert
BEFORE INSERT ON effect_intents
WHEN
    NEW.state NOT IN (
        'pending', 'awaiting_approval', 'approved', 'claimed', 'executing',
        'denied', 'expired', 'cancelled', 'resolved', 'failed', 'permanently_failed',
        'dead_lettered'
    )
    OR NEW.claimed_by IN ('resolved', 'failed', 'permanently_failed')
    OR (NEW.state IN ('claimed', 'executing')
        AND (NEW.claimed_by IS NULL OR NEW.claimed_until IS NULL))
    OR (NEW.state NOT IN ('claimed', 'executing')
        AND (NEW.claimed_by IS NOT NULL OR NEW.claimed_until IS NOT NULL))
BEGIN
    SELECT RAISE(ABORT, 'invalid effect intent state/lease combination');
END;

CREATE TRIGGER effect_intents_validate_state_update
BEFORE UPDATE OF state, claimed_by, claimed_until, approval_id ON effect_intents
WHEN
    NEW.state NOT IN (
        'pending', 'awaiting_approval', 'approved', 'claimed', 'executing',
        'denied', 'expired', 'cancelled', 'resolved', 'failed', 'permanently_failed',
        'dead_lettered'
    )
    OR NEW.claimed_by IN ('resolved', 'failed', 'permanently_failed')
    OR (NEW.state IN ('claimed', 'executing')
        AND (NEW.claimed_by IS NULL OR NEW.claimed_until IS NULL))
    OR (NEW.state NOT IN ('claimed', 'executing')
        AND (NEW.claimed_by IS NOT NULL OR NEW.claimed_until IS NOT NULL))
    OR (NEW.approval_id IS NOT NULL AND NEW.state = 'awaiting_approval' AND NOT EXISTS (
        SELECT 1 FROM approval_requests approval
        WHERE approval.id = NEW.approval_id AND approval.status = 'pending'
    ))
    OR (NEW.approval_id IS NOT NULL
        AND NEW.state IN ('approved', 'claimed', 'executing', 'resolved',
                          'failed', 'permanently_failed', 'dead_lettered')
        AND NOT EXISTS (
            SELECT 1 FROM approval_requests approval
            WHERE approval.id = NEW.approval_id AND approval.status = 'approved'
        ))
    OR (NEW.approval_id IS NOT NULL AND NEW.state = 'denied' AND NOT EXISTS (
        SELECT 1 FROM approval_requests approval
        WHERE approval.id = NEW.approval_id AND approval.status = 'denied'
    ))
    OR (NEW.approval_id IS NOT NULL AND NEW.state = 'expired' AND NOT EXISTS (
        SELECT 1 FROM approval_requests approval
        WHERE approval.id = NEW.approval_id AND approval.status = 'expired'
    ))
    OR (NEW.approval_id IS NOT NULL AND NEW.state = 'cancelled' AND NOT EXISTS (
        SELECT 1 FROM approval_requests approval
        WHERE approval.id = NEW.approval_id AND approval.status = 'cancelled'
    ))
BEGIN
    SELECT RAISE(ABORT, 'invalid effect intent state/lease combination');
END;

-- Approval lineage and canonical subject are immutable after insertion. Only
-- pending decision fields may advance through the coordinator CAS.
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

-- A pending decision can advance once. Terminal audit evidence is immutable;
-- repeating the same update is harmless but cannot rewrite any decision field.
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
        AND NEW.decided_at IS OLD.decided_at
        AND NEW.decision_event_id IS OLD.decision_event_id
    )
)
BEGIN
    SELECT RAISE(ABORT, 'approval decision is immutable');
END;

//! Atomic `SQLite` approval/checkpoint coordinator.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use polkagent_core::{ApprovalId, EventKind, RunId, Timestamp, WorkerId};
use polkagent_store_trait::approval::{
    ApprovalCoordinatorStore, ApprovalDecision, ApprovalPage, ApprovalPrincipalType,
    ApprovalRequestMetadata, ApprovalScope, ApprovalStatus, ApprovalStoreError, ApprovalSubject,
    CheckpointCommitStatus, CheckpointEffectStatus, CheckpointProgress, CheckpointStatus,
    ClaimApprovedEffect, ClaimedEffect, ExecutionCheckpoint, ExecutionCheckpointStore,
    LeasedCheckpoint, PauseForApproval, ResolveApproval, ResolveApprovalResult, ResolveDisposition,
    StoredApproval, StoredCheckpoint, APPROVAL_SUBJECT_SCHEMA_VERSION,
    EXECUTION_CHECKPOINT_SCHEMA_VERSION, MAX_APPROVAL_PAGE_SIZE,
};
use polkagent_store_trait::StoreRetryClass;
use rusqlite::{Connection, OptionalExtension as _};
use uuid::Uuid;

use crate::SqlitePool;

const MAX_TITLE_BYTES: usize = 200;
const MAX_TEXT_BYTES: usize = 2_000;
const MAX_LABEL_BYTES: usize = 256;
const MAX_CONDITIONS: usize = 16;
const MAX_CONDITION_BYTES: usize = 500;

#[async_trait]
impl ApprovalCoordinatorStore for SqlitePool {
    async fn pause_for_approval(
        &self,
        request: PauseForApproval,
    ) -> Result<StoredApproval, ApprovalStoreError> {
        validate_pause(&request)?;
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            with_immediate_transaction(&pool, |connection| pause_tx(connection, &request))
        })
        .await
        .map_err(map_join)?
    }

    async fn resolve_approval(
        &self,
        request: ResolveApproval,
    ) -> Result<ResolveApprovalResult, ApprovalStoreError> {
        validate_resolution(&request)?;
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            with_immediate_transaction(&pool, |connection| resolve_tx(connection, &request))
        })
        .await
        .map_err(map_join)?
    }

    async fn get_approval(
        &self,
        approval_id: ApprovalId,
        scope: ApprovalScope,
    ) -> Result<StoredApproval, ApprovalStoreError> {
        validate_scope(&scope)?;
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let approval = load_approval(&writer, approval_id)?;
            ensure_scope(&approval, &scope)?;
            Ok(approval)
        })
        .await
        .map_err(map_join)?
    }

    async fn list_pending(
        &self,
        scope: ApprovalScope,
        page: ApprovalPage,
    ) -> Result<Vec<StoredApproval>, ApprovalStoreError> {
        validate_scope(&scope)?;
        validate_page(page)?;
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let conversation = scope.conversation_id.map(|id| id.to_string());
            let mut statement = writer
                .prepare(
                    "SELECT id FROM approval_requests
                     WHERE status = 'pending' AND tenant_id = ?1 AND workspace_id = ?2
                       AND authorized_principal_id = ?3
                       AND (?4 IS NULL OR conversation_id = ?4)
                     ORDER BY requested_at ASC, id ASC LIMIT ?5 OFFSET ?6",
                )
                .map_err(map_sqlite)?;
            let rows = statement
                .query_map(
                    rusqlite::params![
                        scope.tenant_id,
                        scope.workspace_id,
                        scope.principal_id.to_string(),
                        conversation,
                        page.limit,
                        page.offset,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;
            drop(statement);
            rows.into_iter()
                .map(|id| {
                    parse_id::<ApprovalId>(&id, "Approval")
                        .and_then(|id| load_approval(&writer, id))
                })
                .collect()
        })
        .await
        .map_err(map_join)?
    }

    async fn claim_approved_effect(
        &self,
        request: ClaimApprovedEffect,
    ) -> Result<ClaimedEffect, ApprovalStoreError> {
        if request.lease_duration.is_zero() {
            return Err(invalid(
                "Effect",
                request.effect_id,
                "positive lease",
                "zero",
            ));
        }
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            with_immediate_transaction(&pool, |connection| claim_approved_tx(connection, &request))
        })
        .await
        .map_err(map_join)?
    }
}

#[async_trait]
impl ExecutionCheckpointStore for SqlitePool {
    async fn get_checkpoint(&self, run_id: RunId) -> Result<StoredCheckpoint, ApprovalStoreError> {
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            load_checkpoint(&writer, run_id)
        })
        .await
        .map_err(map_join)?
    }

    async fn lease_resumable(
        &self,
        worker_id: WorkerId,
        lease_duration: Duration,
        page: ApprovalPage,
    ) -> Result<Vec<LeasedCheckpoint>, ApprovalStoreError> {
        validate_page(page)?;
        if lease_duration.is_zero() {
            return Err(invalid("Checkpoint", "page", "positive lease", "zero"));
        }
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            with_immediate_transaction(&pool, |connection| {
                lease_resumable_tx(connection, worker_id, lease_duration, page)
            })
        })
        .await
        .map_err(map_join)?
    }

    async fn commit_progress(
        &self,
        expected_version: u64,
        next: CheckpointProgress,
    ) -> Result<u64, ApprovalStoreError> {
        validate_checkpoint(&next.checkpoint)?;
        let required_version = expected_version.checked_add(1).ok_or_else(|| {
            invalid(
                "Checkpoint",
                next.checkpoint.run_id,
                "version below u64::MAX",
                expected_version,
            )
        })?;
        if next.checkpoint.version != required_version {
            return Err(invalid(
                "Checkpoint",
                next.checkpoint.run_id,
                required_version.to_string(),
                next.checkpoint.version.to_string(),
            ));
        }
        let pool = self.clone();
        tokio::task::spawn_blocking(move || {
            with_immediate_transaction(&pool, |connection| {
                commit_progress_tx(connection, expected_version, &next)
            })
        })
        .await
        .map_err(map_join)?
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the atomic pause transaction is kept contiguous so its write set and ordering are auditable"
)]
fn pause_tx(
    connection: &Connection,
    request: &PauseForApproval,
) -> Result<StoredApproval, ApprovalStoreError> {
    if pause_has_identity_collision(connection, request)? {
        return load_matching_pause_retry(connection, request);
    }
    let subject = &request.subject;
    let run = connection
        .query_row(
            "SELECT agent_id, conversation_id, state, state_version
             FROM runs WHERE id = ?1",
            [subject.run_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite)?
        .ok_or_else(|| not_found("Run", subject.run_id))?;
    if run.2 != "running" {
        return Err(invalid("Run", subject.run_id, "running", run.2));
    }
    if run.3 != request.expected_run_state_version {
        return Err(invalid(
            "Run",
            subject.run_id,
            request.expected_run_state_version,
            run.3,
        ));
    }
    if run.0 != subject.agent_id.to_string()
        || run.1.as_deref() != Some(subject.conversation_id.to_string().as_str())
    {
        return Err(ApprovalStoreError::ScopeMismatch);
    }
    let exact_step = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM steps step
                JOIN turns turn ON turn.id = step.turn_id
                WHERE step.id = ?1 AND turn.id = ?2 AND turn.run_id = ?3
             )",
            rusqlite::params![
                subject.step_id.to_string(),
                subject.turn_id.to_string(),
                subject.run_id.to_string(),
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(map_sqlite)?;
    if !exact_step {
        return Err(ApprovalStoreError::ScopeMismatch);
    }

    let now = Utc::now();
    let subject_json = serialize(&request.subject, "approval subject")?;
    let checkpoint_json = serialize(&request.checkpoint, "execution checkpoint")?;
    let effect_payload = serialize(&request.effect_payload, "effect payload")?;
    let conditions_json = "[]";
    let requested_event_id = approval_event_id(request.approval_id, "requested");
    let kind = request
        .effect_payload
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("tool_call");
    let retry_class = retry_class_str(request.retry_class);

    // Insert the effect unlinked first, then the approval, then close the
    // circular relationship. Foreign keys stay immediate and every row is
    // invisible until this transaction commits.
    connection
        .execute(
            "INSERT INTO effect_intents
                (id, run_id, turn_id, step_id, kind, params_json,
                 idempotency_key, created_at, state, retry_class, priority, approval_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                     'awaiting_approval', ?9, 1, NULL)",
            rusqlite::params![
                subject.effect_id.to_string(),
                subject.run_id.to_string(),
                subject.turn_id.to_string(),
                subject.step_id.to_string(),
                kind,
                effect_payload,
                request.effect_idempotency_key,
                now.to_rfc3339(),
                retry_class,
            ],
        )
        .map_err(|error| map_insert(error, "Effect", subject.effect_id))?;

    connection
        .execute(
            "INSERT INTO approval_requests
                (id, effect_id, run_id, turn_id, conversation_id, agent_id,
                 subject_schema_version, subject_digest, subject_json,
                 policy_snapshot_digest, tenant_id, workspace_id, title,
                 authorized_principal_id, description, request_reason, status,
                 deadline_at, requested_at, conditions_json, requested_event_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                     ?11, ?12, ?13, ?14, ?15, ?16, 'pending', ?17, ?18, ?19, ?20)",
            rusqlite::params![
                request.approval_id.to_string(),
                subject.effect_id.to_string(),
                subject.run_id.to_string(),
                subject.turn_id.to_string(),
                subject.conversation_id.to_string(),
                subject.agent_id.to_string(),
                subject.schema_version,
                subject.subject_digest,
                subject_json,
                subject.policy_snapshot_digest,
                request.metadata.tenant_id,
                request.metadata.workspace_id,
                request.metadata.title,
                request.metadata.authorized_principal_id.to_string(),
                request.metadata.description,
                request.metadata.reason,
                request.deadline_at.to_rfc3339(),
                now.to_rfc3339(),
                conditions_json,
                requested_event_id,
            ],
        )
        .map_err(|error| map_insert(error, "Approval", request.approval_id))?;

    let linked = connection
        .execute(
            "UPDATE effect_intents SET approval_id = ?1
             WHERE id = ?2 AND state = 'awaiting_approval' AND approval_id IS NULL",
            rusqlite::params![
                request.approval_id.to_string(),
                subject.effect_id.to_string()
            ],
        )
        .map_err(map_sqlite)?;
    require_changed(linked, "Effect", subject.effect_id, "unlinked", "changed")?;

    connection
        .execute(
            "INSERT INTO execution_checkpoints
                (run_id, turn_id, conversation_id, agent_id, schema_version,
                 version, status, checkpoint_json, integrity_digest,
                 classification, deadline_at, retention_expires_at,
                 created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'paused_for_approval',
                     ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                subject.run_id.to_string(),
                subject.turn_id.to_string(),
                subject.conversation_id.to_string(),
                subject.agent_id.to_string(),
                request.checkpoint.schema_version,
                request.checkpoint.version,
                checkpoint_json,
                request.checkpoint.integrity_digest,
                request.checkpoint.classification.to_string(),
                request
                    .checkpoint
                    .deadline_at
                    .map(|value| value.to_rfc3339()),
                request
                    .checkpoint
                    .retention_expires_at
                    .map(|value| value.to_rfc3339()),
                now.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )
        .map_err(|error| map_insert(error, "Checkpoint", subject.run_id))?;

    let next_run_state = format!("awaiting_approval:{}", request.approval_id);
    let run_changed = connection
        .execute(
            "UPDATE runs
             SET state = ?1, state_version = state_version + 1, updated_at = ?2
             WHERE id = ?3 AND state = 'running' AND state_version = ?4",
            rusqlite::params![
                next_run_state,
                now.to_rfc3339(),
                subject.run_id.to_string(),
                request.expected_run_state_version,
            ],
        )
        .map_err(map_sqlite)?;
    require_changed(run_changed, "Run", subject.run_id, "running", "changed")?;

    insert_approval_event(
        connection,
        &requested_event_id,
        subject.run_id,
        request.approval_id,
        &EventKind::ApprovalRequested {
            request_id: request.approval_id.to_string(),
        },
        now,
    )?;

    load_approval(connection, request.approval_id)
}

fn pause_has_identity_collision(
    connection: &Connection,
    request: &PauseForApproval,
) -> Result<bool, ApprovalStoreError> {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM approval_requests
                WHERE id = ?1 OR effect_id = ?2
                UNION ALL
                SELECT 1 FROM effect_intents
                WHERE id = ?2 OR (run_id = ?3 AND idempotency_key = ?4)
             )",
            rusqlite::params![
                request.approval_id.to_string(),
                request.subject.effect_id.to_string(),
                request.subject.run_id.to_string(),
                request.effect_idempotency_key,
            ],
            |row| row.get(0),
        )
        .map_err(map_sqlite)
}

fn load_matching_pause_retry(
    connection: &Connection,
    request: &PauseForApproval,
) -> Result<StoredApproval, ApprovalStoreError> {
    let mismatch = || {
        conflict(
            "Approval",
            request.approval_id,
            "stable pause identity was reused with different fields",
        )
    };
    let approval = load_approval(connection, request.approval_id).map_err(|_| mismatch())?;
    if approval.subject != request.subject
        || approval.metadata != request.metadata
        || approval.deadline_at != request.deadline_at
        || approval.status != ApprovalStatus::Pending
        || approval.requested_event_id != approval_event_id(request.approval_id, "requested")
    {
        return Err(mismatch());
    }

    let effect = connection
        .query_row(
            "SELECT run_id, step_id, kind, params_json, idempotency_key,
                    state, retry_class, approval_id, claimed_by, claimed_until
             FROM effect_intents WHERE id = ?1",
            [request.subject.effect_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite)?
        .ok_or_else(mismatch)?;
    let expected_kind = request
        .effect_payload
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("tool_call");
    if effect.0 != request.subject.run_id.to_string()
        || effect.1 != request.subject.step_id.to_string()
        || effect.2 != expected_kind
        || deserialize::<serde_json::Value>(&effect.3, "effect payload")? != request.effect_payload
        || effect.4 != request.effect_idempotency_key
        || effect.5 != "awaiting_approval"
        || effect.6 != retry_class_str(request.retry_class)
        || effect.7.as_deref() != Some(request.approval_id.to_string().as_str())
        || effect.8.is_some()
        || effect.9.is_some()
    {
        return Err(mismatch());
    }

    let checkpoint = load_checkpoint(connection, request.subject.run_id).map_err(|_| mismatch())?;
    if checkpoint.checkpoint != request.checkpoint
        || checkpoint.status != CheckpointStatus::PausedForApproval
        || checkpoint.lease_owner.is_some()
        || checkpoint.lease_expires_at.is_some()
    {
        return Err(mismatch());
    }
    let expected_run_version = request
        .expected_run_state_version
        .checked_add(1)
        .ok_or_else(mismatch)?;
    let expected_run_state = format!("awaiting_approval:{}", request.approval_id);
    let run_matches = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM runs
             WHERE id = ?1 AND state = ?2 AND state_version = ?3)",
            rusqlite::params![
                request.subject.run_id.to_string(),
                expected_run_state,
                expected_run_version,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(map_sqlite)?;
    let event_matches = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM run_events
             WHERE id = ?1 AND run_id = ?2 AND kind = 'approval_requested'
               AND correlation_id = ?3)",
            rusqlite::params![
                approval.requested_event_id,
                request.subject.run_id.to_string(),
                request.approval_id.to_string(),
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(map_sqlite)?;
    if !run_matches || !event_matches {
        return Err(mismatch());
    }
    Ok(approval)
}

#[allow(
    clippy::too_many_lines,
    reason = "the atomic resolution transaction is kept contiguous so every expected-state write is auditable"
)]
fn resolve_tx(
    connection: &Connection,
    request: &ResolveApproval,
) -> Result<ResolveApprovalResult, ApprovalStoreError> {
    let current = load_approval(connection, request.approval_id)?;
    ensure_resolution_lineage(&current, request)?;
    if current.status != ApprovalStatus::Pending {
        if resolution_matches(&current, request) {
            return Ok(ResolveApprovalResult {
                approval: current,
                disposition: ResolveDisposition::AlreadyApplied,
            });
        }
        return Err(conflict(
            "Approval",
            request.approval_id,
            "a different or late decision already won",
        ));
    }

    let now = Utc::now();
    match request.decision {
        ApprovalDecision::AllowOnce | ApprovalDecision::RejectOnce
            if now >= current.deadline_at =>
        {
            return Err(ApprovalStoreError::DeadlineElapsed);
        }
        ApprovalDecision::Expire if now < current.deadline_at => {
            return Err(invalid(
                "Approval",
                request.approval_id,
                "deadline elapsed",
                "deadline active",
            ));
        }
        _ => {}
    }

    let approval_status = request.decision.status();
    let effect_status = decision_effect_status(request.decision);
    let principal_type = principal_type_str(request.principal_type);
    let decision = decision_str(request.decision);
    let conditions_json = serialize(&request.conditions, "approval conditions")?;
    let decision_event_id = approval_event_id(request.approval_id, "resolved");
    let updated = connection
        .execute(
            "UPDATE approval_requests
             SET status = ?1, decided_by = ?2, principal_type = ?3,
                 decision_surface = ?4, rationale = ?5, conditions_json = ?6,
                 decision = ?7, decision_run_state_version = ?8,
                 decided_at = ?9, decision_event_id = ?10
             WHERE id = ?11 AND status = 'pending'",
            rusqlite::params![
                approval_status_str(approval_status),
                request.principal_id.to_string(),
                principal_type,
                request.surface,
                request.rationale,
                conditions_json,
                decision,
                request.expected_run_state_version,
                now.to_rfc3339(),
                decision_event_id,
                request.approval_id.to_string(),
            ],
        )
        .map_err(map_sqlite)?;
    require_changed(
        updated,
        "Approval",
        request.approval_id,
        "pending",
        "changed",
    )?;

    let effect_updated = connection
        .execute(
            "UPDATE effect_intents
             SET state = ?1
             WHERE id = ?2 AND approval_id = ?3 AND state = 'awaiting_approval'
               AND claimed_by IS NULL AND claimed_until IS NULL",
            rusqlite::params![
                effect_status_str(effect_status),
                request.effect_id.to_string(),
                request.approval_id.to_string(),
            ],
        )
        .map_err(map_sqlite)?;
    require_changed(
        effect_updated,
        "Effect",
        request.effect_id,
        "awaiting_approval",
        "changed",
    )?;

    let mut checkpoint = load_checkpoint(connection, request.run_id)?;
    if checkpoint.status != CheckpointStatus::PausedForApproval {
        return Err(invalid(
            "Checkpoint",
            request.run_id,
            "paused_for_approval",
            checkpoint_status_str(checkpoint.status),
        ));
    }
    let checkpoint_effect = checkpoint
        .checkpoint
        .effects
        .iter_mut()
        .find(|effect| effect.effect_id == request.effect_id)
        .ok_or_else(|| ApprovalStoreError::Integrity {
            message: "checkpoint does not contain the approval effect".to_owned(),
        })?;
    if checkpoint_effect.status != CheckpointEffectStatus::AwaitingApproval {
        return Err(invalid(
            "CheckpointEffect",
            request.effect_id,
            "awaiting_approval",
            checkpoint_effect_status_str(checkpoint_effect.status),
        ));
    }
    checkpoint_effect.status = effect_status;
    let expected_checkpoint_version = checkpoint.checkpoint.version;
    checkpoint.checkpoint.version =
        checkpoint
            .checkpoint
            .version
            .checked_add(1)
            .ok_or_else(|| ApprovalStoreError::Integrity {
                message: "checkpoint version is exhausted".to_owned(),
            })?;
    let checkpoint_status = match request.decision {
        ApprovalDecision::AllowOnce | ApprovalDecision::RejectOnce => CheckpointStatus::Resumable,
        ApprovalDecision::Expire | ApprovalDecision::Cancel => CheckpointStatus::Terminal,
    };
    let checkpoint_json = serialize(&checkpoint.checkpoint, "execution checkpoint")?;
    let checkpoint_updated = connection
        .execute(
            "UPDATE execution_checkpoints
             SET version = ?1, status = ?2, checkpoint_json = ?3,
                 updated_at = ?4, lease_owner = NULL, lease_expires_at = NULL
             WHERE run_id = ?5 AND version = ?6 AND status = 'paused_for_approval'",
            rusqlite::params![
                checkpoint.checkpoint.version,
                checkpoint_status_str(checkpoint_status),
                checkpoint_json,
                now.to_rfc3339(),
                request.run_id.to_string(),
                expected_checkpoint_version,
            ],
        )
        .map_err(map_sqlite)?;
    require_changed(
        checkpoint_updated,
        "Checkpoint",
        request.run_id,
        expected_checkpoint_version,
        "changed",
    )?;

    let expected_run_state = format!("awaiting_approval:{}", request.approval_id);
    let next_run_state = match request.decision {
        ApprovalDecision::AllowOnce | ApprovalDecision::RejectOnce => {
            format!("waiting_effect:{}", request.effect_id)
        }
        ApprovalDecision::Expire => "timed_out".to_owned(),
        ApprovalDecision::Cancel => "cancelled:approval_cancelled".to_owned(),
    };
    let terminal = matches!(
        request.decision,
        ApprovalDecision::Expire | ApprovalDecision::Cancel
    );
    let run_updated = connection
        .execute(
            "UPDATE runs
             SET state = ?1, state_version = state_version + 1, updated_at = ?2,
                 completed_at = CASE WHEN ?3 THEN ?2 ELSE completed_at END
             WHERE id = ?4 AND state = ?5 AND state_version = ?6",
            rusqlite::params![
                next_run_state,
                now.to_rfc3339(),
                terminal,
                request.run_id.to_string(),
                expected_run_state,
                request.expected_run_state_version,
            ],
        )
        .map_err(map_sqlite)?;
    require_changed(
        run_updated,
        "Run",
        request.run_id,
        expected_run_state,
        "changed",
    )?;

    let event_kind = match request.decision {
        ApprovalDecision::AllowOnce => EventKind::ApprovalGranted {
            approval_id: request.approval_id.to_string(),
        },
        ApprovalDecision::RejectOnce => EventKind::ApprovalDenied {
            reason: request
                .rationale
                .clone()
                .unwrap_or_else(|| "approval rejected".to_owned()),
        },
        ApprovalDecision::Expire => EventKind::RunTimedOut,
        ApprovalDecision::Cancel => EventKind::RunCancelled {
            reason: "approval cancelled".to_owned(),
        },
    };
    insert_approval_event(
        connection,
        &decision_event_id,
        request.run_id,
        request.approval_id,
        &event_kind,
        now,
    )?;

    Ok(ResolveApprovalResult {
        approval: load_approval(connection, request.approval_id)?,
        disposition: ResolveDisposition::Applied,
    })
}

#[allow(
    clippy::too_many_lines,
    reason = "the paired effect/checkpoint lease transaction is kept contiguous for auditability"
)]
fn claim_approved_tx(
    connection: &Connection,
    request: &ClaimApprovedEffect,
) -> Result<ClaimedEffect, ApprovalStoreError> {
    let approval = load_approval(connection, request.approval_id)?;
    if approval.status != ApprovalStatus::Approved
        || approval.subject.effect_id != request.effect_id
        || approval.subject.run_id != request.run_id
    {
        return Err(ApprovalStoreError::ScopeMismatch);
    }
    if approval.subject.subject_digest != request.subject_digest {
        return Err(ApprovalStoreError::DigestMismatch);
    }
    let checkpoint = load_checkpoint(connection, request.run_id)?;
    if checkpoint.checkpoint.version != request.expected_checkpoint_version {
        return Err(invalid(
            "Checkpoint",
            request.run_id,
            request.expected_checkpoint_version,
            checkpoint.checkpoint.version,
        ));
    }

    let now = Utc::now();
    let lease_expires_at = now + chrono_duration(request.lease_duration)?;
    let effect_row = connection
        .query_row(
            "SELECT state, claimed_by, claimed_until, params_json, retry_class,
                    (SELECT COUNT(*) FROM effect_attempts attempt
                     WHERE attempt.intent_id = effect_intents.id)
             FROM effect_intents
             WHERE id = ?1 AND approval_id = ?2 AND run_id = ?3",
            rusqlite::params![
                request.effect_id.to_string(),
                request.approval_id.to_string(),
                request.run_id.to_string(),
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u64>(5)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite)?
        .ok_or_else(|| not_found("Effect", request.effect_id))?;

    let worker_id = request.worker_id.to_string();
    let existing_expiry = effect_row.2.as_deref().map(parse_ts).transpose()?;
    if effect_row.0 == "claimed"
        && effect_row.1.as_deref() == Some(worker_id.as_str())
        && existing_expiry.is_some_and(|expiry| expiry > now)
    {
        let existing_expiry = existing_expiry.ok_or_else(|| ApprovalStoreError::Integrity {
            message: "claimed effect is missing its lease expiry".to_owned(),
        })?;
        if checkpoint.status != CheckpointStatus::Leased
            || checkpoint.lease_owner != Some(request.worker_id)
            || checkpoint.lease_expires_at != Some(existing_expiry)
        {
            return Err(invalid(
                "Effect",
                request.effect_id,
                "active matching effect/checkpoint lease",
                "stale or divergent lease",
            ));
        }
        return Ok(ClaimedEffect {
            approval_id: request.approval_id,
            effect_id: request.effect_id,
            run_id: request.run_id,
            worker_id: request.worker_id,
            lease_expires_at: existing_expiry,
            effect_payload: deserialize(&effect_row.3, "effect payload")?,
            retry_class: parse_retry_class(&effect_row.4)?,
            checkpoint_version: checkpoint.checkpoint.version,
        });
    }

    let reclaiming_pre_io_claim = effect_row.0 == "claimed"
        && existing_expiry.is_some_and(|expiry| expiry <= now)
        && effect_row.5 == 0;
    if reclaiming_pre_io_claim {
        let checkpoint_reclaimable = checkpoint.status == CheckpointStatus::Leased
            && (checkpoint
                .lease_expires_at
                .is_some_and(|expiry| expiry <= now)
                || (checkpoint.lease_owner == Some(request.worker_id)
                    && checkpoint
                        .lease_expires_at
                        .is_some_and(|expiry| expiry > now)));
        if !checkpoint_reclaimable {
            return Err(invalid(
                "Checkpoint",
                request.run_id,
                "expired lease or active lease by this recovery worker",
                checkpoint_status_str(checkpoint.status),
            ));
        }
        let effect_updated = connection
            .execute(
                "UPDATE effect_intents
                 SET claimed_by = ?1, claimed_until = ?2
                 WHERE id = ?3 AND approval_id = ?4 AND state = 'claimed'
                   AND claimed_until <= ?5 AND NOT EXISTS (
                       SELECT 1 FROM effect_attempts attempt
                       WHERE attempt.intent_id = effect_intents.id
                   )",
                rusqlite::params![
                    worker_id,
                    lease_expires_at.to_rfc3339(),
                    request.effect_id.to_string(),
                    request.approval_id.to_string(),
                    now.to_rfc3339(),
                ],
            )
            .map_err(map_sqlite)?;
        require_changed(
            effect_updated,
            "Effect",
            request.effect_id,
            "expired pre-I/O claim",
            "changed",
        )?;
        let checkpoint_updated = connection
            .execute(
                "UPDATE execution_checkpoints
                 SET lease_owner = ?1, lease_expires_at = ?2, updated_at = ?3
                 WHERE run_id = ?4 AND version = ?5 AND status = 'leased'
                   AND (lease_expires_at <= ?3
                        OR (lease_owner = ?1 AND lease_expires_at > ?3))",
                rusqlite::params![
                    worker_id,
                    lease_expires_at.to_rfc3339(),
                    now.to_rfc3339(),
                    request.run_id.to_string(),
                    request.expected_checkpoint_version,
                ],
            )
            .map_err(map_sqlite)?;
        require_changed(
            checkpoint_updated,
            "Checkpoint",
            request.run_id,
            "reclaimable lease",
            "changed",
        )?;
        return Ok(ClaimedEffect {
            approval_id: request.approval_id,
            effect_id: request.effect_id,
            run_id: request.run_id,
            worker_id: request.worker_id,
            lease_expires_at,
            effect_payload: deserialize(&effect_row.3, "effect payload")?,
            retry_class: parse_retry_class(&effect_row.4)?,
            checkpoint_version: checkpoint.checkpoint.version,
        });
    }
    if effect_row.0 != "approved" || effect_row.1.is_some() || effect_row.2.is_some() {
        return Err(invalid(
            "Effect",
            request.effect_id,
            "approved and unleased",
            effect_row.0,
        ));
    }

    match checkpoint.status {
        CheckpointStatus::Resumable => {}
        CheckpointStatus::Leased
            if checkpoint.lease_owner == Some(request.worker_id)
                && checkpoint
                    .lease_expires_at
                    .is_some_and(|expiry| expiry > now) => {}
        other => {
            return Err(invalid(
                "Checkpoint",
                request.run_id,
                "resumable or leased by this worker",
                checkpoint_status_str(other),
            ));
        }
    }

    let effect_updated = connection
        .execute(
            "UPDATE effect_intents
             SET state = 'claimed', claimed_by = ?1, claimed_until = ?2
             WHERE id = ?3 AND approval_id = ?4 AND state = 'approved'
               AND claimed_by IS NULL AND claimed_until IS NULL",
            rusqlite::params![
                request.worker_id.to_string(),
                lease_expires_at.to_rfc3339(),
                request.effect_id.to_string(),
                request.approval_id.to_string(),
            ],
        )
        .map_err(map_sqlite)?;
    require_changed(
        effect_updated,
        "Effect",
        request.effect_id,
        "approved",
        "changed",
    )?;
    let checkpoint_updated = connection
        .execute(
            "UPDATE execution_checkpoints
             SET status = 'leased', lease_owner = ?1, lease_expires_at = ?2,
                 updated_at = ?3
             WHERE run_id = ?4 AND version = ?5
               AND (status = 'resumable'
                    OR (status = 'leased' AND lease_owner = ?1 AND lease_expires_at > ?3))",
            rusqlite::params![
                request.worker_id.to_string(),
                lease_expires_at.to_rfc3339(),
                now.to_rfc3339(),
                request.run_id.to_string(),
                request.expected_checkpoint_version,
            ],
        )
        .map_err(map_sqlite)?;
    require_changed(
        checkpoint_updated,
        "Checkpoint",
        request.run_id,
        "resumable",
        "changed",
    )?;

    Ok(ClaimedEffect {
        approval_id: request.approval_id,
        effect_id: request.effect_id,
        run_id: request.run_id,
        worker_id: request.worker_id,
        lease_expires_at,
        effect_payload: deserialize(&effect_row.3, "effect payload")?,
        retry_class: parse_retry_class(&effect_row.4)?,
        checkpoint_version: checkpoint.checkpoint.version,
    })
}

fn lease_resumable_tx(
    connection: &Connection,
    worker_id: WorkerId,
    lease_duration: Duration,
    page: ApprovalPage,
) -> Result<Vec<LeasedCheckpoint>, ApprovalStoreError> {
    let now = Utc::now();
    let expiry = now + chrono_duration(lease_duration)?;
    let mut statement = connection
        .prepare(
            "SELECT run_id FROM execution_checkpoints
             WHERE status = 'resumable'
                OR (status = 'leased' AND lease_expires_at < ?1)
             ORDER BY updated_at ASC, run_id ASC LIMIT ?2 OFFSET ?3",
        )
        .map_err(map_sqlite)?;
    let run_ids = statement
        .query_map(
            rusqlite::params![now.to_rfc3339(), page.limit, page.offset],
            |row| row.get::<_, String>(0),
        )
        .map_err(map_sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite)?;
    drop(statement);

    let mut leased = Vec::with_capacity(run_ids.len());
    for run_id in run_ids {
        let run_id = parse_id::<RunId>(&run_id, "Run")?;
        let changed = connection
            .execute(
                "UPDATE execution_checkpoints
                 SET status = 'leased', lease_owner = ?1, lease_expires_at = ?2,
                     updated_at = ?3
                 WHERE run_id = ?4 AND (status = 'resumable'
                    OR (status = 'leased' AND lease_expires_at < ?3))",
                rusqlite::params![
                    worker_id.to_string(),
                    expiry.to_rfc3339(),
                    now.to_rfc3339(),
                    run_id.to_string(),
                ],
            )
            .map_err(map_sqlite)?;
        if changed == 1 {
            leased.push(LeasedCheckpoint {
                stored: load_checkpoint(connection, run_id)?,
            });
        }
    }
    Ok(leased)
}

fn commit_progress_tx(
    connection: &Connection,
    expected_version: u64,
    next: &CheckpointProgress,
) -> Result<u64, ApprovalStoreError> {
    let current = load_checkpoint(connection, next.checkpoint.run_id)?;
    if current.checkpoint.run_id != next.checkpoint.run_id
        || current.checkpoint.turn_id != next.checkpoint.turn_id
        || current.checkpoint.conversation_id != next.checkpoint.conversation_id
        || current.checkpoint.agent_id != next.checkpoint.agent_id
        || current.checkpoint.model_id != next.checkpoint.model_id
        || current.checkpoint.executor_id != next.checkpoint.executor_id
    {
        return Err(ApprovalStoreError::ScopeMismatch);
    }
    let next_status = match next.status {
        CheckpointCommitStatus::Resumable => CheckpointStatus::Resumable,
        CheckpointCommitStatus::Terminal => CheckpointStatus::Terminal,
    };
    let checkpoint_json = serialize(&next.checkpoint, "execution checkpoint")?;
    let now = Utc::now();
    let changed = connection
        .execute(
            "UPDATE execution_checkpoints
             SET schema_version = ?1, version = ?2, status = ?3,
                 checkpoint_json = ?4, integrity_digest = ?5,
                 classification = ?6, deadline_at = ?7,
                 retention_expires_at = ?8, lease_owner = NULL,
                 lease_expires_at = NULL, updated_at = ?9
             WHERE run_id = ?10 AND version = ?11 AND status = 'leased'
               AND lease_owner = ?12 AND lease_expires_at > ?9",
            rusqlite::params![
                next.checkpoint.schema_version,
                next.checkpoint.version,
                checkpoint_status_str(next_status),
                checkpoint_json,
                next.checkpoint.integrity_digest,
                next.checkpoint.classification.to_string(),
                next.checkpoint.deadline_at.map(|value| value.to_rfc3339()),
                next.checkpoint
                    .retention_expires_at
                    .map(|value| value.to_rfc3339()),
                now.to_rfc3339(),
                next.checkpoint.run_id.to_string(),
                expected_version,
                next.worker_id.to_string(),
            ],
        )
        .map_err(map_sqlite)?;
    if changed != 1 {
        let actual = load_checkpoint(connection, next.checkpoint.run_id)?;
        return Err(invalid(
            "Checkpoint",
            next.checkpoint.run_id,
            format!("leased version {expected_version} by {}", next.worker_id),
            format!(
                "{} version {}",
                checkpoint_status_str(actual.status),
                actual.checkpoint.version
            ),
        ));
    }
    Ok(next.checkpoint.version)
}

fn load_approval(
    connection: &Connection,
    approval_id: ApprovalId,
) -> Result<StoredApproval, ApprovalStoreError> {
    let raw = connection
        .query_row(
            "SELECT subject_json, status, title, description, request_reason,
                    tenant_id, workspace_id, authorized_principal_id,
                    deadline_at, requested_at,
                    decided_by, principal_type, decision_surface, rationale,
                    conditions_json, decision, decided_at, requested_event_id,
                    decision_run_state_version, decision_event_id,
                    effect_id, run_id, turn_id, conversation_id,
                    agent_id, subject_digest, policy_snapshot_digest
             FROM approval_requests WHERE id = ?1",
            [approval_id.to_string()],
            |row| {
                Ok(RawApproval {
                    subject_json: row.get(0)?,
                    status: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    reason: row.get(4)?,
                    tenant_id: row.get(5)?,
                    workspace_id: row.get(6)?,
                    authorized_principal_id: row.get(7)?,
                    deadline_at: row.get(8)?,
                    requested_at: row.get(9)?,
                    decided_by: row.get(10)?,
                    principal_type: row.get(11)?,
                    decision_surface: row.get(12)?,
                    rationale: row.get(13)?,
                    conditions_json: row.get(14)?,
                    decision: row.get(15)?,
                    decided_at: row.get(16)?,
                    requested_event_id: row.get(17)?,
                    decision_run_state_version: row.get(18)?,
                    decision_event_id: row.get(19)?,
                    effect_id: row.get(20)?,
                    run_id: row.get(21)?,
                    turn_id: row.get(22)?,
                    conversation_id: row.get(23)?,
                    agent_id: row.get(24)?,
                    subject_digest: row.get(25)?,
                    policy_snapshot_digest: row.get(26)?,
                })
            },
        )
        .optional()
        .map_err(map_sqlite)?
        .ok_or_else(|| not_found("Approval", approval_id))?;
    let subject: ApprovalSubject = deserialize(&raw.subject_json, "approval subject")?;
    if subject.effect_id.to_string() != raw.effect_id
        || subject.run_id.to_string() != raw.run_id
        || subject.turn_id.to_string() != raw.turn_id
        || subject.conversation_id.to_string() != raw.conversation_id
        || subject.agent_id.to_string() != raw.agent_id
        || subject.subject_digest != raw.subject_digest
        || subject.policy_snapshot_digest != raw.policy_snapshot_digest
    {
        return Err(ApprovalStoreError::Integrity {
            message: "approval subject does not match immutable lineage columns".to_owned(),
        });
    }
    Ok(StoredApproval {
        id: approval_id,
        subject,
        status: parse_approval_status(&raw.status)?,
        metadata: ApprovalRequestMetadata {
            title: raw.title,
            description: raw.description,
            reason: raw.reason,
            tenant_id: raw.tenant_id,
            workspace_id: raw.workspace_id,
            authorized_principal_id: parse_id(&raw.authorized_principal_id, "Principal")?,
        },
        deadline_at: parse_ts(&raw.deadline_at)?,
        requested_at: parse_ts(&raw.requested_at)?,
        decided_by: raw
            .decided_by
            .as_deref()
            .map(|value| parse_id(value, "Principal"))
            .transpose()?,
        principal_type: raw
            .principal_type
            .as_deref()
            .map(parse_principal_type)
            .transpose()?,
        decision_surface: raw.decision_surface,
        rationale: raw.rationale,
        conditions: deserialize(&raw.conditions_json, "approval conditions")?,
        decision: raw.decision.as_deref().map(parse_decision).transpose()?,
        decision_run_state_version: raw.decision_run_state_version,
        decided_at: raw.decided_at.as_deref().map(parse_ts).transpose()?,
        requested_event_id: raw.requested_event_id,
        decision_event_id: raw.decision_event_id,
    })
}

fn load_checkpoint(
    connection: &Connection,
    run_id: RunId,
) -> Result<StoredCheckpoint, ApprovalStoreError> {
    let raw = connection
        .query_row(
            "SELECT checkpoint_json, schema_version, version, status,
                    integrity_digest, classification, deadline_at,
                    retention_expires_at, lease_owner, lease_expires_at,
                    created_at, updated_at, turn_id, conversation_id, agent_id
             FROM execution_checkpoints WHERE run_id = ?1",
            [run_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, String>(14)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite)?
        .ok_or_else(|| not_found("Checkpoint", run_id))?;
    if raw.1 != EXECUTION_CHECKPOINT_SCHEMA_VERSION {
        return Err(ApprovalStoreError::UnsupportedCheckpointVersion { version: raw.1 });
    }
    let checkpoint: ExecutionCheckpoint = deserialize(&raw.0, "execution checkpoint")?;
    if checkpoint.schema_version != raw.1
        || checkpoint.version != raw.2
        || checkpoint.run_id != run_id
        || checkpoint.turn_id.to_string() != raw.12
        || checkpoint.conversation_id.to_string() != raw.13
        || checkpoint.agent_id.to_string() != raw.14
        || checkpoint.integrity_digest != raw.4
        || checkpoint.classification.to_string() != raw.5
        || checkpoint.deadline_at.map(|value| value.to_rfc3339()) != raw.6
        || checkpoint
            .retention_expires_at
            .map(|value| value.to_rfc3339())
            != raw.7
    {
        return Err(ApprovalStoreError::Integrity {
            message: "checkpoint payload does not match indexed envelope".to_owned(),
        });
    }
    Ok(StoredCheckpoint {
        checkpoint,
        status: parse_checkpoint_status(&raw.3)?,
        lease_owner: raw
            .8
            .as_deref()
            .map(|value| parse_id(value, "Worker"))
            .transpose()?,
        lease_expires_at: raw.9.as_deref().map(parse_ts).transpose()?,
        created_at: parse_ts(&raw.10)?,
        updated_at: parse_ts(&raw.11)?,
    })
}

struct RawApproval {
    subject_json: String,
    status: String,
    title: String,
    description: String,
    reason: String,
    tenant_id: String,
    workspace_id: String,
    authorized_principal_id: String,
    deadline_at: String,
    requested_at: String,
    decided_by: Option<String>,
    principal_type: Option<String>,
    decision_surface: Option<String>,
    rationale: Option<String>,
    conditions_json: String,
    decision: Option<String>,
    decided_at: Option<String>,
    requested_event_id: String,
    decision_run_state_version: Option<u64>,
    decision_event_id: Option<String>,
    effect_id: String,
    run_id: String,
    turn_id: String,
    conversation_id: String,
    agent_id: String,
    subject_digest: String,
    policy_snapshot_digest: String,
}

fn insert_approval_event(
    connection: &Connection,
    event_id: &str,
    run_id: RunId,
    approval_id: ApprovalId,
    event_kind: &EventKind,
    timestamp: Timestamp,
) -> Result<(), ApprovalStoreError> {
    let event_type = approval_event_type(event_kind)?;
    let sequence: u64 = connection
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM run_events WHERE run_id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .map_err(map_sqlite)?;
    connection
        .execute(
            "INSERT INTO run_events
                (id, run_id, sequence, kind, data_json, timestamp,
                 correlation_id, schema_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
            rusqlite::params![
                event_id,
                run_id.to_string(),
                sequence,
                event_type,
                serialize(event_kind, "approval event")?,
                timestamp.to_rfc3339(),
                approval_id.to_string(),
            ],
        )
        .map_err(|error| map_insert(error, "ApprovalEvent", event_id))?;
    Ok(())
}

fn approval_event_type(event_kind: &EventKind) -> Result<&'static str, ApprovalStoreError> {
    match event_kind {
        EventKind::ApprovalRequested { .. } => Ok("approval_requested"),
        EventKind::ApprovalGranted { .. } => Ok("approval_granted"),
        EventKind::ApprovalDenied { .. } => Ok("approval_denied"),
        EventKind::RunTimedOut => Ok("run_timed_out"),
        EventKind::RunCancelled { .. } => Ok("run_cancelled"),
        _ => Err(ApprovalStoreError::Integrity {
            message: "unsupported approval coordinator event kind".to_owned(),
        }),
    }
}

fn ensure_resolution_lineage(
    approval: &StoredApproval,
    request: &ResolveApproval,
) -> Result<(), ApprovalStoreError> {
    ensure_scope_lineage(approval, &request.scope)?;
    if approval.subject.effect_id != request.effect_id
        || approval.subject.run_id != request.run_id
        || approval.subject.turn_id != request.turn_id
        || approval.subject.conversation_id != request.conversation_id
    {
        return Err(ApprovalStoreError::ScopeMismatch);
    }
    if request.principal_id != request.scope.principal_id
        || !principal_can_resolve(request.principal_type, request.decision)
        || (!matches!(request.principal_type, ApprovalPrincipalType::Service)
            && approval.metadata.authorized_principal_id != request.principal_id)
    {
        return Err(ApprovalStoreError::ScopeMismatch);
    }
    if approval.subject.subject_digest != request.subject_digest {
        return Err(ApprovalStoreError::DigestMismatch);
    }
    Ok(())
}

fn ensure_scope(
    approval: &StoredApproval,
    scope: &ApprovalScope,
) -> Result<(), ApprovalStoreError> {
    ensure_scope_lineage(approval, scope)?;
    if approval.metadata.authorized_principal_id != scope.principal_id {
        return Err(ApprovalStoreError::ScopeMismatch);
    }
    Ok(())
}

fn ensure_scope_lineage(
    approval: &StoredApproval,
    scope: &ApprovalScope,
) -> Result<(), ApprovalStoreError> {
    if approval.metadata.tenant_id != scope.tenant_id
        || approval.metadata.workspace_id != scope.workspace_id
        || scope
            .conversation_id
            .is_some_and(|id| id != approval.subject.conversation_id)
    {
        return Err(ApprovalStoreError::ScopeMismatch);
    }
    Ok(())
}

fn resolution_matches(approval: &StoredApproval, request: &ResolveApproval) -> bool {
    approval.decision == Some(request.decision)
        && approval.decided_by == Some(request.principal_id)
        && approval.principal_type == Some(request.principal_type)
        && approval.decision_surface.as_deref() == Some(request.surface.as_str())
        && approval.rationale == request.rationale
        && approval.conditions == request.conditions
        && approval.decision_run_state_version == Some(request.expected_run_state_version)
}

fn validate_pause(request: &PauseForApproval) -> Result<(), ApprovalStoreError> {
    if request.subject.schema_version != APPROVAL_SUBJECT_SCHEMA_VERSION {
        return Err(ApprovalStoreError::Integrity {
            message: format!(
                "unsupported approval subject schema version {}",
                request.subject.schema_version
            ),
        });
    }
    validate_checkpoint(&request.checkpoint)?;
    let subject = &request.subject;
    if subject.run_id != request.checkpoint.run_id
        || subject.turn_id != request.checkpoint.turn_id
        || subject.conversation_id != request.checkpoint.conversation_id
        || subject.agent_id != request.checkpoint.agent_id
        || request.checkpoint.version != 1
        || request.checkpoint.retry_class != request.retry_class
        || request.checkpoint.deadline_at != Some(request.deadline_at)
        || !request.checkpoint.effects.iter().any(|effect| {
            effect.effect_id == subject.effect_id
                && effect.status == CheckpointEffectStatus::AwaitingApproval
        })
    {
        return Err(ApprovalStoreError::ScopeMismatch);
    }
    validate_nonempty("subject digest", &subject.subject_digest, MAX_LABEL_BYTES)?;
    validate_nonempty("tool name", &subject.tool_name, MAX_LABEL_BYTES)?;
    validate_nonempty("tool call id", &subject.tool_call_id, MAX_LABEL_BYTES)?;
    validate_nonempty(
        "tool spec digest",
        &subject.tool_spec_digest,
        MAX_LABEL_BYTES,
    )?;
    validate_nonempty(
        "policy snapshot digest",
        &subject.policy_snapshot_digest,
        MAX_LABEL_BYTES,
    )?;
    validate_nonempty(
        "effect idempotency key",
        &request.effect_idempotency_key,
        MAX_TEXT_BYTES,
    )?;
    validate_text("title", &request.metadata.title, MAX_TITLE_BYTES)?;
    validate_text("description", &request.metadata.description, MAX_TEXT_BYTES)?;
    validate_text("request reason", &request.metadata.reason, MAX_TEXT_BYTES)?;
    validate_scope(&ApprovalScope {
        tenant_id: request.metadata.tenant_id.clone(),
        workspace_id: request.metadata.workspace_id.clone(),
        conversation_id: Some(subject.conversation_id),
        principal_id: request.metadata.authorized_principal_id,
    })?;
    if request.deadline_at <= Utc::now() {
        return Err(ApprovalStoreError::DeadlineElapsed);
    }
    Ok(())
}

fn validate_checkpoint(checkpoint: &ExecutionCheckpoint) -> Result<(), ApprovalStoreError> {
    if checkpoint.schema_version != EXECUTION_CHECKPOINT_SCHEMA_VERSION {
        return Err(ApprovalStoreError::UnsupportedCheckpointVersion {
            version: checkpoint.schema_version,
        });
    }
    if checkpoint.version == 0
        || checkpoint.next_model_turn == 0
        || checkpoint.next_step_sequence == 0
        || checkpoint.next_effect_sequence == 0
        || checkpoint.effects.is_empty()
    {
        return Err(ApprovalStoreError::Integrity {
            message: "checkpoint versions, sequences, and effect set must be non-zero".to_owned(),
        });
    }
    validate_nonempty("model id", &checkpoint.model_id, MAX_LABEL_BYTES)?;
    validate_nonempty("executor id", &checkpoint.executor_id, MAX_LABEL_BYTES)?;
    validate_nonempty(
        "checkpoint integrity digest",
        &checkpoint.integrity_digest,
        MAX_LABEL_BYTES,
    )
}

fn validate_resolution(request: &ResolveApproval) -> Result<(), ApprovalStoreError> {
    validate_scope(&request.scope)?;
    if !principal_can_resolve(request.principal_type, request.decision) {
        return Err(ApprovalStoreError::ScopeMismatch);
    }
    validate_nonempty("subject digest", &request.subject_digest, MAX_LABEL_BYTES)?;
    validate_nonempty("surface", &request.surface, MAX_LABEL_BYTES)?;
    if let Some(rationale) = &request.rationale {
        validate_text("rationale", rationale, MAX_TEXT_BYTES)?;
    }
    if request.conditions.len() > MAX_CONDITIONS {
        return Err(ApprovalStoreError::Integrity {
            message: "too many approval conditions".to_owned(),
        });
    }
    for condition in &request.conditions {
        validate_text("condition", condition, MAX_CONDITION_BYTES)?;
    }
    Ok(())
}

const fn principal_can_resolve(
    principal_type: ApprovalPrincipalType,
    decision: ApprovalDecision,
) -> bool {
    matches!(
        (principal_type, decision),
        (
            ApprovalPrincipalType::Human | ApprovalPrincipalType::Quorum,
            ApprovalDecision::AllowOnce | ApprovalDecision::RejectOnce
        ) | (
            ApprovalPrincipalType::Service,
            ApprovalDecision::Expire | ApprovalDecision::Cancel
        )
    )
}

fn validate_scope(scope: &ApprovalScope) -> Result<(), ApprovalStoreError> {
    validate_nonempty("tenant id", &scope.tenant_id, MAX_LABEL_BYTES)?;
    validate_nonempty("workspace id", &scope.workspace_id, MAX_LABEL_BYTES)
}

fn validate_page(page: ApprovalPage) -> Result<(), ApprovalStoreError> {
    if page.limit == 0 || page.limit > MAX_APPROVAL_PAGE_SIZE {
        return Err(ApprovalStoreError::Integrity {
            message: format!("approval page limit must be between 1 and {MAX_APPROVAL_PAGE_SIZE}"),
        });
    }
    Ok(())
}

fn validate_nonempty(field: &str, value: &str, maximum: usize) -> Result<(), ApprovalStoreError> {
    if value.trim().is_empty() || value.len() > maximum {
        return Err(ApprovalStoreError::Integrity {
            message: format!("{field} must be non-empty and at most {maximum} bytes"),
        });
    }
    Ok(())
}

fn validate_text(field: &str, value: &str, maximum: usize) -> Result<(), ApprovalStoreError> {
    if value.len() > maximum {
        return Err(ApprovalStoreError::Integrity {
            message: format!("{field} must be at most {maximum} bytes"),
        });
    }
    Ok(())
}

fn with_immediate_transaction<T>(
    pool: &SqlitePool,
    operation: impl FnOnce(&Connection) -> Result<T, ApprovalStoreError>,
) -> Result<T, ApprovalStoreError> {
    let writer = pool.writer();
    writer
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(map_sqlite)?;
    let result = operation(&writer);
    match result {
        Ok(value) => {
            writer.execute_batch("COMMIT").map_err(map_sqlite)?;
            Ok(value)
        }
        Err(error) => {
            let _ = writer.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn require_changed(
    changed: usize,
    resource_type: impl ToString,
    id: impl ToString,
    expected: impl ToString,
    actual: impl ToString,
) -> Result<(), ApprovalStoreError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(invalid(resource_type, id, expected, actual))
    }
}

fn approval_event_id(approval_id: ApprovalId, kind: &str) -> String {
    format!("approval:{approval_id}:{kind}")
}

fn chrono_duration(duration: Duration) -> Result<chrono::Duration, ApprovalStoreError> {
    chrono::Duration::from_std(duration).map_err(|error| ApprovalStoreError::Integrity {
        message: format!("lease duration is out of range: {error}"),
    })
}

fn serialize<T: serde::Serialize>(value: &T, label: &str) -> Result<String, ApprovalStoreError> {
    serde_json::to_string(value).map_err(|error| ApprovalStoreError::Integrity {
        message: format!("cannot serialize {label}: {error}"),
    })
}

fn deserialize<T: serde::de::DeserializeOwned>(
    value: &str,
    label: &str,
) -> Result<T, ApprovalStoreError> {
    serde_json::from_str(value).map_err(|error| ApprovalStoreError::Integrity {
        message: format!("cannot deserialize {label}: {error}"),
    })
}

fn parse_id<T: From<Uuid>>(value: &str, label: &str) -> Result<T, ApprovalStoreError> {
    Uuid::parse_str(value)
        .map(T::from)
        .map_err(|_| ApprovalStoreError::Integrity {
            message: format!("stored {label} identity is invalid"),
        })
}

fn parse_ts(value: &str) -> Result<Timestamp, ApprovalStoreError> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|_| ApprovalStoreError::Integrity {
            message: "stored approval timestamp is invalid".to_owned(),
        })
}

fn retry_class_str(value: StoreRetryClass) -> &'static str {
    match value {
        StoreRetryClass::Idempotent => "idempotent",
        StoreRetryClass::CheckBeforeRetry => "check_before_retry",
        StoreRetryClass::NoAutoRetry => "no_auto_retry",
    }
}

fn parse_retry_class(value: &str) -> Result<StoreRetryClass, ApprovalStoreError> {
    match value {
        "idempotent" => Ok(StoreRetryClass::Idempotent),
        "check_before_retry" => Ok(StoreRetryClass::CheckBeforeRetry),
        "no_auto_retry" => Ok(StoreRetryClass::NoAutoRetry),
        _ => Err(ApprovalStoreError::Integrity {
            message: "stored effect retry class is invalid".to_owned(),
        }),
    }
}

fn approval_status_str(value: ApprovalStatus) -> &'static str {
    match value {
        ApprovalStatus::Pending => "pending",
        ApprovalStatus::Approved => "approved",
        ApprovalStatus::Denied => "denied",
        ApprovalStatus::Expired => "expired",
        ApprovalStatus::Cancelled => "cancelled",
    }
}

fn parse_approval_status(value: &str) -> Result<ApprovalStatus, ApprovalStoreError> {
    match value {
        "pending" => Ok(ApprovalStatus::Pending),
        "approved" => Ok(ApprovalStatus::Approved),
        "denied" => Ok(ApprovalStatus::Denied),
        "expired" => Ok(ApprovalStatus::Expired),
        "cancelled" => Ok(ApprovalStatus::Cancelled),
        _ => Err(ApprovalStoreError::Integrity {
            message: "stored approval status is invalid".to_owned(),
        }),
    }
}

fn decision_str(value: ApprovalDecision) -> &'static str {
    match value {
        ApprovalDecision::AllowOnce => "allow_once",
        ApprovalDecision::RejectOnce => "reject_once",
        ApprovalDecision::Expire => "expire",
        ApprovalDecision::Cancel => "cancel",
    }
}

fn parse_decision(value: &str) -> Result<ApprovalDecision, ApprovalStoreError> {
    match value {
        "allow_once" => Ok(ApprovalDecision::AllowOnce),
        "reject_once" => Ok(ApprovalDecision::RejectOnce),
        "expire" => Ok(ApprovalDecision::Expire),
        "cancel" => Ok(ApprovalDecision::Cancel),
        _ => Err(ApprovalStoreError::Integrity {
            message: "stored approval decision is invalid".to_owned(),
        }),
    }
}

fn principal_type_str(value: ApprovalPrincipalType) -> &'static str {
    match value {
        ApprovalPrincipalType::Human => "human",
        ApprovalPrincipalType::Service => "service",
        ApprovalPrincipalType::Quorum => "quorum",
    }
}

fn parse_principal_type(value: &str) -> Result<ApprovalPrincipalType, ApprovalStoreError> {
    match value {
        "human" => Ok(ApprovalPrincipalType::Human),
        "service" => Ok(ApprovalPrincipalType::Service),
        "quorum" => Ok(ApprovalPrincipalType::Quorum),
        _ => Err(ApprovalStoreError::Integrity {
            message: "stored approval principal type is invalid".to_owned(),
        }),
    }
}

fn decision_effect_status(value: ApprovalDecision) -> CheckpointEffectStatus {
    match value {
        ApprovalDecision::AllowOnce => CheckpointEffectStatus::Approved,
        ApprovalDecision::RejectOnce => CheckpointEffectStatus::Denied,
        ApprovalDecision::Expire => CheckpointEffectStatus::Expired,
        ApprovalDecision::Cancel => CheckpointEffectStatus::Cancelled,
    }
}

fn effect_status_str(value: CheckpointEffectStatus) -> &'static str {
    match value {
        CheckpointEffectStatus::AwaitingApproval => "awaiting_approval",
        CheckpointEffectStatus::Approved => "approved",
        CheckpointEffectStatus::Denied => "denied",
        CheckpointEffectStatus::Expired => "expired",
        CheckpointEffectStatus::Cancelled => "cancelled",
    }
}

fn checkpoint_effect_status_str(value: CheckpointEffectStatus) -> &'static str {
    effect_status_str(value)
}

fn checkpoint_status_str(value: CheckpointStatus) -> &'static str {
    match value {
        CheckpointStatus::PausedForApproval => "paused_for_approval",
        CheckpointStatus::Resumable => "resumable",
        CheckpointStatus::Leased => "leased",
        CheckpointStatus::Terminal => "terminal",
    }
}

fn parse_checkpoint_status(value: &str) -> Result<CheckpointStatus, ApprovalStoreError> {
    match value {
        "paused_for_approval" => Ok(CheckpointStatus::PausedForApproval),
        "resumable" => Ok(CheckpointStatus::Resumable),
        "leased" => Ok(CheckpointStatus::Leased),
        "terminal" => Ok(CheckpointStatus::Terminal),
        _ => Err(ApprovalStoreError::Integrity {
            message: "stored checkpoint status is invalid".to_owned(),
        }),
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "used as a Result::map_err callback, which transfers the task error"
)]
fn map_join(error: tokio::task::JoinError) -> ApprovalStoreError {
    ApprovalStoreError::Backend {
        message: format!("blocking approval task failed: {error}"),
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "used as a Result::map_err callback, which transfers the database error"
)]
fn map_sqlite(error: rusqlite::Error) -> ApprovalStoreError {
    ApprovalStoreError::Backend {
        message: format!("sqlite approval operation failed: {error}"),
    }
}

fn map_insert(
    error: rusqlite::Error,
    resource_type: impl ToString,
    id: impl ToString,
) -> ApprovalStoreError {
    if crate::StoreError::is_unique_violation(&error) {
        conflict(resource_type, id, "identity already exists")
    } else if crate::StoreError::is_fk_violation(&error) {
        ApprovalStoreError::ScopeMismatch
    } else {
        map_sqlite(error)
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "error constructors accept both owned and borrowed displayable context"
)]
fn not_found(resource_type: impl ToString, id: impl ToString) -> ApprovalStoreError {
    ApprovalStoreError::NotFound {
        resource_type: resource_type.to_string(),
        id: id.to_string(),
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "error constructors accept both owned and borrowed displayable context"
)]
fn conflict(
    resource_type: impl ToString,
    id: impl ToString,
    message: impl ToString,
) -> ApprovalStoreError {
    ApprovalStoreError::Conflict {
        resource_type: resource_type.to_string(),
        id: id.to_string(),
        message: message.to_string(),
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "error constructors accept both owned and borrowed displayable context"
)]
fn invalid(
    resource_type: impl ToString,
    id: impl ToString,
    expected: impl ToString,
    actual: impl ToString,
) -> ApprovalStoreError {
    ApprovalStoreError::InvalidTransition {
        resource_type: resource_type.to_string(),
        id: id.to_string(),
        expected: expected.to_string(),
        actual: actual.to_string(),
    }
}

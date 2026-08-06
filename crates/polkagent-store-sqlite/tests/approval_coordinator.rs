//! APR-00/APR-01 approval/checkpoint coordinator conformance and race tests.

#![allow(
    clippy::expect_used,
    reason = "integration fixtures stop at the exact violated persistence invariant"
)]

use std::path::Path;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use polkagent_core::{
    AgentId, ApprovalId, ConversationId, DataClassification, EffectAttemptId, EffectId,
    EffectOutcomeId, EventKind, PrincipalId, RunId, StepId, TurnId, WorkerId,
};
use polkagent_event::EventType;
use polkagent_store_sqlite::{migrations, SqlitePool};
use polkagent_store_trait::approval::{
    ApprovalCoordinatorStore, ApprovalDecision, ApprovalPage, ApprovalPrincipalType,
    ApprovalRequestMetadata, ApprovalScope, ApprovalStatus, ApprovalStoreError, ApprovalSubject,
    CheckpointEffect, CheckpointEffectStatus, CheckpointStatus, ClaimApprovedEffect,
    ExecutionCheckpoint, ExecutionCheckpointStore, PauseForApproval, ResolveApproval,
    ResolveDisposition, APPROVAL_SUBJECT_SCHEMA_VERSION, EXECUTION_CHECKPOINT_SCHEMA_VERSION,
};
use polkagent_store_trait::conformance::{self, ApprovalConformanceFixture};
use polkagent_store_trait::event::EventStore as _;
use polkagent_store_trait::{EffectStore, RunStore, StoreRetryClass, StoredOutcome};
use rusqlite::Connection;

#[derive(Clone)]
struct Fixture {
    agent_id: AgentId,
    conversation_id: ConversationId,
    step_id: StepId,
    run_id: RunId,
    effect_id: EffectId,
    approval_id: ApprovalId,
    pause: PauseForApproval,
    scope: ApprovalScope,
    allow: ResolveApproval,
    claim: ClaimApprovedEffect,
}

fn open_pool(path: Option<&Path>) -> SqlitePool {
    let pool = match path {
        Some(path) => SqlitePool::open(path).expect("open file SQLite pool"),
        None => SqlitePool::open_in_memory().expect("open in-memory SQLite pool"),
    };
    {
        let writer = pool.writer();
        migrations::migrate(&writer).expect("apply migrations");
    }
    pool
}

#[allow(
    clippy::too_many_lines,
    reason = "the complete correlated persistence fixture is intentionally visible in one place"
)]
fn seed_fixture(pool: &SqlitePool) -> Fixture {
    seed_fixture_with_deadline(pool, ChronoDuration::minutes(10))
}

#[allow(
    clippy::too_many_lines,
    reason = "the complete correlated persistence fixture is intentionally visible in one place"
)]
fn seed_fixture_with_deadline(pool: &SqlitePool, deadline_after: ChronoDuration) -> Fixture {
    let agent_id = AgentId::new();
    let conversation_id = ConversationId::new();
    let turn_id = TurnId::new();
    let step_id = StepId::new();
    let run_id = RunId::new();
    let effect_id = EffectId::new();
    let approval_id = ApprovalId::new();
    let principal_id = PrincipalId::new();
    let worker_id = WorkerId::new();
    let now = Utc::now();
    let deadline = now + deadline_after;

    {
        let writer = pool.writer();
        writer
            .execute(
                "INSERT INTO agents
                    (id, name, state, spec_json, created_at, updated_at)
                 VALUES (?1, ?2, 'active', '{}', ?3, ?3)",
                rusqlite::params![
                    agent_id.to_string(),
                    format!("agent-{agent_id}"),
                    now.to_rfc3339(),
                ],
            )
            .expect("seed agent");
        writer
            .execute(
                "INSERT INTO conversations
                    (id, agent_id, title, metadata_json, created_at, updated_at)
                 VALUES (?1, ?2, 'approval fixture', '{}', ?3, ?3)",
                rusqlite::params![
                    conversation_id.to_string(),
                    agent_id.to_string(),
                    now.to_rfc3339(),
                ],
            )
            .expect("seed conversation");
        writer
            .execute(
                "INSERT INTO runs
                    (id, agent_id, conversation_id, state, params_json,
                     created_at, updated_at, started_at)
                 VALUES (?1, ?2, ?3, 'running', '{}', ?4, ?4, ?4)",
                rusqlite::params![
                    run_id.to_string(),
                    agent_id.to_string(),
                    conversation_id.to_string(),
                    now.to_rfc3339(),
                ],
            )
            .expect("seed running run");
        writer
            .execute(
                "INSERT INTO turns (id, run_id, sequence, role, started_at)
                 VALUES (?1, ?2, 1, 'assistant', ?3)",
                rusqlite::params![turn_id.to_string(), run_id.to_string(), now.to_rfc3339()],
            )
            .expect("seed executor turn");
        writer
            .execute(
                "INSERT INTO steps (id, turn_id, sequence, kind, started_at)
                 VALUES (?1, ?2, 1, 'tool_call', ?3)",
                rusqlite::params![step_id.to_string(), turn_id.to_string(), now.to_rfc3339()],
            )
            .expect("seed tool step");
    }

    let subject = ApprovalSubject {
        schema_version: APPROVAL_SUBJECT_SCHEMA_VERSION,
        conversation_id,
        turn_id,
        step_id,
        run_id,
        agent_id,
        effect_id,
        tool_call_id: "call-write-1".to_owned(),
        tool_name: "filesystem.write".to_owned(),
        validated_arguments: serde_json::json!({"path":"notes.txt","content":"safe"}),
        required_action: "write".to_owned(),
        required_resource: "workspace/notes.txt".to_owned(),
        working_directory: Some("/workspace".to_owned()),
        security_scope: serde_json::json!({
            "tenant_id":"tenant-a",
            "workspace_id":"workspace-a"
        }),
        tool_spec_digest: "blake3:tool-spec".to_owned(),
        policy_snapshot_digest: "blake3:policy".to_owned(),
        subject_digest: "blake3:subject".to_owned(),
    };
    let checkpoint = ExecutionCheckpoint {
        schema_version: EXECUTION_CHECKPOINT_SCHEMA_VERSION,
        run_id,
        turn_id,
        conversation_id,
        agent_id,
        model_id: "fixture-model".to_owned(),
        executor_id: "fixture-provider".to_owned(),
        messages: serde_json::json!([
            {"role":"assistant","content":"preparing tool"}
        ]),
        tool_calls: serde_json::json!([{
            "id":"call-write-1",
            "name":"filesystem.write"
        }]),
        next_model_turn: 2,
        next_step_sequence: 2,
        next_effect_sequence: (1_u64 << 32) | 7,
        accumulated_usage: serde_json::json!({"input_tokens":12,"output_tokens":4}),
        accumulated_cost: Some("0.0012".to_owned()),
        deadline_at: Some(deadline),
        retry_class: StoreRetryClass::NoAutoRetry,
        effects: vec![CheckpointEffect {
            effect_id,
            status: CheckpointEffectStatus::AwaitingApproval,
        }],
        version: 1,
        classification: DataClassification::Private,
        retention_expires_at: Some(deadline + ChronoDuration::days(7)),
        integrity_digest: "blake3:checkpoint-v1".to_owned(),
    };
    let metadata = ApprovalRequestMetadata {
        title: "Write notes.txt".to_owned(),
        description: "Write one file in the selected workspace".to_owned(),
        reason: "filesystem write grant requires approval".to_owned(),
        tenant_id: "tenant-a".to_owned(),
        workspace_id: "workspace-a".to_owned(),
        authorized_principal_id: principal_id,
    };
    let pause = PauseForApproval {
        approval_id,
        expected_run_state_version: 0,
        subject: subject.clone(),
        effect_payload: serde_json::json!({
            "kind":"tool_call",
            "tool_name":"filesystem.write",
            "arguments":{"path":"notes.txt","content":"safe"}
        }),
        effect_idempotency_key: format!("approval-effect-{effect_id}"),
        retry_class: StoreRetryClass::NoAutoRetry,
        checkpoint,
        metadata,
        deadline_at: deadline,
    };
    let scope = ApprovalScope {
        tenant_id: "tenant-a".to_owned(),
        workspace_id: "workspace-a".to_owned(),
        conversation_id: Some(conversation_id),
        principal_id,
    };
    let allow = ResolveApproval {
        approval_id,
        expected_run_state_version: 1,
        effect_id,
        run_id,
        turn_id,
        conversation_id,
        subject_digest: subject.subject_digest.clone(),
        scope: scope.clone(),
        principal_id,
        principal_type: ApprovalPrincipalType::Human,
        surface: "conformance".to_owned(),
        decision: ApprovalDecision::AllowOnce,
        rationale: Some("reviewed exact operation".to_owned()),
        conditions: vec!["once".to_owned()],
    };
    let claim = ClaimApprovedEffect {
        approval_id,
        effect_id,
        run_id,
        subject_digest: subject.subject_digest.clone(),
        expected_checkpoint_version: 2,
        worker_id,
        lease_duration: Duration::from_secs(60),
    };
    Fixture {
        agent_id,
        conversation_id,
        step_id,
        run_id,
        effect_id,
        approval_id,
        pause,
        scope,
        allow,
        claim,
    }
}

#[tokio::test]
async fn approval_foundation_shared_conformance_and_run_cas() {
    let pool = open_pool(None);
    let fixture = seed_fixture(&pool);
    conformance::test_approval_foundation(
        &pool,
        ApprovalConformanceFixture {
            pause: fixture.pause,
            scope: fixture.scope,
            allow: fixture.allow,
            claim: fixture.claim,
        },
    )
    .await;

    let cas_run = RunId::new();
    {
        let writer = pool.writer();
        writer
            .execute(
                "INSERT INTO runs
                    (id, agent_id, conversation_id, state, params_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'running', '{}', ?4, ?4)",
                rusqlite::params![
                    cas_run.to_string(),
                    fixture.agent_id.to_string(),
                    fixture.conversation_id.to_string(),
                    Utc::now().to_rfc3339(),
                ],
            )
            .expect("seed CAS run");
    }
    conformance::test_run_store_compare_and_swap(&pool, cas_run).await;
}

#[tokio::test]
async fn identical_pause_retry_is_idempotent_and_changed_retry_conflicts() {
    let pool = open_pool(None);
    let fixture = seed_fixture(&pool);
    let mut stale = fixture.pause.clone();
    stale.expected_run_state_version = 1;
    assert!(matches!(
        pool.pause_for_approval(stale).await,
        Err(ApprovalStoreError::InvalidTransition { .. })
    ));
    assert!(pool
        .read_run_events(fixture.run_id)
        .await
        .expect("events after stale pause")
        .is_empty());
    let first = pool
        .pause_for_approval(fixture.pause.clone())
        .await
        .expect("first pause");
    let retry = pool
        .pause_for_approval(fixture.pause.clone())
        .await
        .expect("identical stable-ID pause retry");
    assert_eq!(retry, first);
    assert_eq!(
        pool.state_version(fixture.run_id)
            .await
            .expect("run revision after retry"),
        1
    );
    assert_eq!(
        pool.read_run_events(fixture.run_id)
            .await
            .expect("events after retry")
            .len(),
        1
    );

    let mut changed_subject = fixture.pause.clone();
    changed_subject.subject.subject_digest = "blake3:changed-subject".to_owned();
    assert!(matches!(
        pool.pause_for_approval(changed_subject).await,
        Err(ApprovalStoreError::Conflict { .. })
    ));
    let mut changed_checkpoint = fixture.pause.clone();
    changed_checkpoint.checkpoint.integrity_digest = "blake3:changed-checkpoint".to_owned();
    assert!(matches!(
        pool.pause_for_approval(changed_checkpoint).await,
        Err(ApprovalStoreError::Conflict { .. })
    ));
    let mut changed_metadata = fixture.pause.clone();
    changed_metadata.metadata.title = "Changed title".to_owned();
    assert!(matches!(
        pool.pause_for_approval(changed_metadata).await,
        Err(ApprovalStoreError::Conflict { .. })
    ));
}

#[tokio::test]
async fn reject_once_is_human_authorized_resumable_and_never_claimable() {
    let pool = open_pool(None);
    let fixture = seed_fixture(&pool);
    pool.pause_for_approval(fixture.pause.clone())
        .await
        .expect("pause");
    let mut reject = fixture.allow.clone();
    reject.decision = ApprovalDecision::RejectOnce;
    reject.rationale = Some("operator rejected exact write".to_owned());
    let resolved = pool
        .resolve_approval(reject.clone())
        .await
        .expect("human reject");
    assert_eq!(resolved.approval.status, ApprovalStatus::Denied);
    assert_eq!(
        pool.resolve_approval(reject.clone())
            .await
            .expect("identical reject retry")
            .disposition,
        ResolveDisposition::AlreadyApplied
    );
    let mut wrong_version = reject.clone();
    wrong_version.expected_run_state_version = 2;
    assert!(matches!(
        pool.resolve_approval(wrong_version).await,
        Err(ApprovalStoreError::Conflict { .. })
    ));
    let mut wrong_lineage = reject;
    wrong_lineage.conversation_id = ConversationId::new();
    assert!(matches!(
        pool.resolve_approval(wrong_lineage).await,
        Err(ApprovalStoreError::ScopeMismatch)
    ));
    assert_decision_state(
        &pool,
        &fixture,
        "denied",
        CheckpointStatus::Resumable,
        CheckpointEffectStatus::Denied,
        &format!("waiting_effect:{}", fixture.effect_id),
        "approval_denied",
    )
    .await;
    assert!(pool.claim_approved_effect(fixture.claim).await.is_err());
}

#[tokio::test]
async fn expire_is_service_authorized_only_after_deadline_and_terminal() {
    let pool = open_pool(None);
    let fixture = seed_fixture_with_deadline(&pool, ChronoDuration::milliseconds(500));
    pool.pause_for_approval(fixture.pause.clone())
        .await
        .expect("pause");
    let service_id = PrincipalId::new();
    let mut expire = fixture.allow.clone();
    expire.decision = ApprovalDecision::Expire;
    expire.principal_type = ApprovalPrincipalType::Service;
    expire.principal_id = service_id;
    expire.scope.principal_id = service_id;
    expire.rationale = Some("durable deadline elapsed".to_owned());
    assert!(matches!(
        pool.resolve_approval(expire.clone()).await,
        Err(ApprovalStoreError::InvalidTransition { .. })
    ));
    tokio::time::sleep(Duration::from_millis(600)).await;
    let resolved = pool
        .resolve_approval(expire.clone())
        .await
        .expect("service expiry after deadline");
    assert_eq!(resolved.approval.status, ApprovalStatus::Expired);
    assert_eq!(
        pool.resolve_approval(expire.clone())
            .await
            .expect("identical expiry retry after deadline")
            .disposition,
        ResolveDisposition::AlreadyApplied
    );
    let mut wrong_digest = expire;
    wrong_digest.subject_digest = "blake3:wrong-terminal-retry".to_owned();
    assert!(matches!(
        pool.resolve_approval(wrong_digest).await,
        Err(ApprovalStoreError::DigestMismatch)
    ));
    assert_decision_state(
        &pool,
        &fixture,
        "expired",
        CheckpointStatus::Terminal,
        CheckpointEffectStatus::Expired,
        "timed_out",
        "run_timed_out",
    )
    .await;
    assert!(pool.claim_approved_effect(fixture.claim).await.is_err());
}

#[tokio::test]
async fn cancel_is_service_authorized_terminal_and_human_cancel_fails_closed() {
    let pool = open_pool(None);
    let fixture = seed_fixture(&pool);
    pool.pause_for_approval(fixture.pause.clone())
        .await
        .expect("pause");
    let mut human_cancel = fixture.allow.clone();
    human_cancel.decision = ApprovalDecision::Cancel;
    assert!(matches!(
        pool.resolve_approval(human_cancel).await,
        Err(ApprovalStoreError::ScopeMismatch)
    ));
    let service_id = PrincipalId::new();
    let mut cancel = fixture.allow.clone();
    cancel.decision = ApprovalDecision::Cancel;
    cancel.principal_type = ApprovalPrincipalType::Service;
    cancel.principal_id = service_id;
    cancel.scope.principal_id = service_id;
    cancel.rationale = Some("parent session cancelled".to_owned());
    let resolved = pool.resolve_approval(cancel).await.expect("service cancel");
    assert_eq!(resolved.approval.status, ApprovalStatus::Cancelled);
    assert_decision_state(
        &pool,
        &fixture,
        "cancelled",
        CheckpointStatus::Terminal,
        CheckpointEffectStatus::Cancelled,
        "cancelled:approval_cancelled",
        "run_cancelled",
    )
    .await;
    assert!(pool.claim_approved_effect(fixture.claim).await.is_err());
}

async fn assert_decision_state(
    pool: &SqlitePool,
    fixture: &Fixture,
    expected_effect_state: &str,
    expected_checkpoint_status: CheckpointStatus,
    expected_checkpoint_effect: CheckpointEffectStatus,
    expected_run_state: &str,
    expected_event_type: &str,
) {
    let intent = pool
        .get_intent(fixture.effect_id)
        .await
        .expect("load decision effect");
    assert_eq!(intent.state, expected_effect_state);
    let checkpoint = pool
        .get_checkpoint(fixture.run_id)
        .await
        .expect("load decision checkpoint");
    assert_eq!(checkpoint.status, expected_checkpoint_status);
    assert_eq!(checkpoint.checkpoint.version, 2);
    assert_eq!(
        checkpoint.checkpoint.effects[0].status,
        expected_checkpoint_effect
    );
    let run = pool.get(fixture.run_id).await.expect("load decision run");
    assert_eq!(run.status.as_str(), expected_run_state);
    assert_eq!(
        pool.state_version(fixture.run_id)
            .await
            .expect("load decision run revision"),
        2
    );
    assert!(pool
        .claim_intent(WorkerId::new(), Duration::from_secs(60))
        .await
        .expect("generic claim query")
        .is_none());
    let events = pool
        .read_from_cursor(0, 10)
        .await
        .expect("cursor replay of coordinator events");
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].event_type, expected_event_type);
    for event in events {
        let kind: EventKind =
            serde_json::from_value(event.payload).expect("canonical EventKind payload");
        let event_type = EventType::from_kind(&kind).expect("catalogued coordinator event kind");
        assert_eq!(event_type.as_str(), event.event_type);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_allow_and_reject_have_one_winner() {
    let pool = open_pool(None);
    let fixture = seed_fixture(&pool);
    pool.pause_for_approval(fixture.pause.clone())
        .await
        .expect("pause");
    let allow = fixture.allow.clone();
    let mut reject = fixture.allow.clone();
    reject.decision = ApprovalDecision::RejectOnce;
    reject.rationale = Some("rejected exact operation".to_owned());

    let allow_store = pool.clone();
    let reject_store = pool.clone();
    let (allow_result, reject_result) = tokio::join!(
        allow_store.resolve_approval(allow),
        reject_store.resolve_approval(reject)
    );
    assert_ne!(allow_result.is_ok(), reject_result.is_ok());
    let loser = if allow_result.is_err() {
        allow_result.expect_err("allow lost")
    } else {
        reject_result.expect_err("reject lost")
    };
    assert!(matches!(loser, ApprovalStoreError::Conflict { .. }));

    let stored = pool
        .get_approval(fixture.approval_id, fixture.scope)
        .await
        .expect("load race winner");
    assert!(matches!(
        stored.status,
        ApprovalStatus::Approved | ApprovalStatus::Denied
    ));
    let events = pool
        .read_run_events(fixture.run_id)
        .await
        .expect("load approval events");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "approval_requested");
    assert!(matches!(
        events[1].event_type.as_str(),
        "approval_granted" | "approval_denied"
    ));

    let writer = pool.writer();
    let immutable = writer
        .execute(
            "UPDATE approval_requests SET rationale = 'rewritten' WHERE id = ?1",
            [fixture.approval_id.to_string()],
        )
        .expect_err("terminal audit evidence must be immutable");
    assert!(immutable
        .to_string()
        .contains("approval decision is immutable"));
}

#[tokio::test]
async fn exact_prior_state_failure_rolls_back_the_whole_resolution() {
    let pool = open_pool(None);
    let fixture = seed_fixture(&pool);
    pool.pause_for_approval(fixture.pause.clone())
        .await
        .expect("pause");
    {
        let writer = pool.writer();
        writer
            .execute(
                "UPDATE runs SET state_version = state_version + 1 WHERE id = ?1",
                [fixture.run_id.to_string()],
            )
            .expect("simulate an external winning run revision");
    }

    assert!(matches!(
        pool.resolve_approval(fixture.allow.clone()).await,
        Err(ApprovalStoreError::InvalidTransition { .. })
    ));
    let approval = pool
        .get_approval(fixture.approval_id, fixture.scope)
        .await
        .expect("load rolled-back approval");
    assert_eq!(approval.status, ApprovalStatus::Pending);
    let checkpoint = pool
        .get_checkpoint(fixture.run_id)
        .await
        .expect("load rolled-back checkpoint");
    assert_eq!(checkpoint.status, CheckpointStatus::PausedForApproval);
    assert_eq!(checkpoint.checkpoint.version, 1);
    let effect_state: String = {
        let writer = pool.writer();
        writer
            .query_row(
                "SELECT state FROM effect_intents WHERE id = ?1",
                [fixture.effect_id.to_string()],
                |row| row.get(0),
            )
            .expect("load rolled-back effect")
    };
    assert_eq!(effect_state, "awaiting_approval");
    let events = pool
        .read_run_events(fixture.run_id)
        .await
        .expect("load rolled-back event set");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "approval_requested");
}

#[tokio::test]
async fn wrong_digest_conversation_and_principal_fail_closed() {
    let pool = open_pool(None);
    let fixture = seed_fixture(&pool);
    pool.pause_for_approval(fixture.pause.clone())
        .await
        .expect("pause");

    let mut wrong_digest = fixture.allow.clone();
    wrong_digest.subject_digest = "blake3:changed".to_owned();
    assert!(matches!(
        pool.resolve_approval(wrong_digest).await,
        Err(ApprovalStoreError::DigestMismatch)
    ));

    let mut wrong_conversation = fixture.allow.clone();
    wrong_conversation.conversation_id = ConversationId::new();
    assert!(matches!(
        pool.resolve_approval(wrong_conversation).await,
        Err(ApprovalStoreError::ScopeMismatch)
    ));

    let mut wrong_principal = fixture.allow.clone();
    wrong_principal.principal_id = PrincipalId::new();
    wrong_principal.scope.principal_id = wrong_principal.principal_id;
    assert!(matches!(
        pool.resolve_approval(wrong_principal).await,
        Err(ApprovalStoreError::ScopeMismatch)
    ));

    let service_id = PrincipalId::new();
    let mut service_allow = fixture.allow.clone();
    service_allow.principal_type = ApprovalPrincipalType::Service;
    service_allow.principal_id = service_id;
    service_allow.scope.principal_id = service_id;
    assert!(matches!(
        pool.resolve_approval(service_allow).await,
        Err(ApprovalStoreError::ScopeMismatch)
    ));

    let resolved = pool
        .resolve_approval(fixture.allow)
        .await
        .expect("valid principal resolves after refusals");
    assert_eq!(resolved.disposition, ResolveDisposition::Applied);
}

#[tokio::test]
async fn reopen_preserves_approval_checkpoint_event_and_exact_claim() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("approval.sqlite");
    let pool = open_pool(Some(&path));
    let fixture = seed_fixture(&pool);
    pool.pause_for_approval(fixture.pause.clone())
        .await
        .expect("pause");
    pool.resolve_approval(fixture.allow.clone())
        .await
        .expect("approve");
    drop(pool);

    let reopened = open_pool(Some(&path));
    let approval = reopened
        .get_approval(fixture.approval_id, fixture.scope.clone())
        .await
        .expect("approval survives reopen");
    assert_eq!(approval.status, ApprovalStatus::Approved);
    let checkpoint = reopened
        .get_checkpoint(fixture.run_id)
        .await
        .expect("checkpoint survives reopen");
    assert_eq!(checkpoint.status, CheckpointStatus::Resumable);
    assert_eq!(
        checkpoint.checkpoint.next_effect_sequence,
        (1_u64 << 32) | 7
    );
    let generic = reopened
        .claim_intent(WorkerId::new(), Duration::from_secs(60))
        .await
        .expect("generic claim query");
    assert!(
        generic.is_none(),
        "generic workers cannot claim approved effects"
    );
    let claimed = reopened
        .claim_approved_effect(fixture.claim)
        .await
        .expect("exact claim after reopen");
    assert_eq!(claimed.effect_id, fixture.effect_id);
    let events = reopened
        .read_run_events(fixture.run_id)
        .await
        .expect("events survive reopen");
    assert_eq!(events.len(), 2);
}

#[tokio::test]
async fn reopen_reclaims_expired_approved_claim_only_before_attempt_start() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("approval-claim-recovery.sqlite");
    let pool = open_pool(Some(&path));
    let fixture = seed_fixture(&pool);
    pool.pause_for_approval(fixture.pause.clone())
        .await
        .expect("pause");
    pool.resolve_approval(fixture.allow.clone())
        .await
        .expect("approve");
    let mut initial_claim = fixture.claim.clone();
    initial_claim.lease_duration = Duration::from_millis(50);
    pool.claim_approved_effect(initial_claim)
        .await
        .expect("claim before simulated crash");
    drop(pool);
    tokio::time::sleep(Duration::from_millis(75)).await;

    let reopened = open_pool(Some(&path));
    let recovery_worker = WorkerId::new();
    let leased = reopened
        .lease_resumable(
            recovery_worker,
            Duration::from_secs(60),
            ApprovalPage {
                limit: 10,
                offset: 0,
            },
        )
        .await
        .expect("re-lease expired checkpoint");
    assert_eq!(leased.len(), 1);
    let mut recovery_claim = fixture.claim;
    recovery_claim.worker_id = recovery_worker;
    let recovered = reopened
        .claim_approved_effect(recovery_claim)
        .await
        .expect("reclaim exact expired pre-I/O effect");
    assert_eq!(recovered.worker_id, recovery_worker);
    let intent = reopened
        .get_intent(fixture.effect_id)
        .await
        .expect("reclaimed effect");
    assert_eq!(intent.state, "claimed");
    assert_eq!(intent.lease_owner, Some(recovery_worker));
}

#[tokio::test]
async fn generic_effect_paths_cannot_rewrite_or_steal_approved_lineage() {
    let pool = open_pool(None);
    let fixture = seed_fixture(&pool);
    pool.pause_for_approval(fixture.pause.clone())
        .await
        .expect("pause");
    pool.resolve_approval(fixture.allow.clone())
        .await
        .expect("approve");

    assert!(pool
        .claim_intent_by_id(fixture.effect_id, WorkerId::new(), Duration::from_secs(60))
        .await
        .is_err());
    assert!(matches!(
        pool.update_intent_state(fixture.effect_id, "failed").await,
        Err(polkagent_store_trait::StoreError::InvalidTransition { .. })
    ));

    let claimed = pool
        .claim_approved_effect(fixture.claim.clone())
        .await
        .expect("exact continuation claim");
    pool.release_claim(fixture.effect_id, fixture.claim.worker_id)
        .await
        .expect("generic release remains a safe no-op");
    assert_eq!(
        pool.get_intent(fixture.effect_id)
            .await
            .expect("effect after generic release")
            .state,
        "claimed"
    );
    assert!(matches!(
        pool.update_intent_state(fixture.effect_id, "pending").await,
        Err(polkagent_store_trait::StoreError::InvalidTransition { .. })
    ));

    {
        let writer = pool.writer();
        writer
            .execute(
                "UPDATE effect_intents SET claimed_until = ?1 WHERE id = ?2",
                rusqlite::params![
                    (Utc::now() - ChronoDuration::seconds(1)).to_rfc3339(),
                    fixture.effect_id.to_string(),
                ],
            )
            .expect("expire exact continuation lease for retry isolation test");
    }
    assert!(pool
        .expired_leases(Utc::now())
        .await
        .expect("generic expired lease query")
        .is_empty());
    assert!(pool
        .claim_intent_by_id(fixture.effect_id, WorkerId::new(), Duration::from_secs(60))
        .await
        .is_err());
    assert!(pool
        .claim_intent(WorkerId::new(), Duration::from_secs(60))
        .await
        .expect("generic claim scan")
        .is_none());
    assert_eq!(claimed.worker_id, fixture.claim.worker_id);
}

#[tokio::test]
async fn approved_claim_uses_the_same_durable_executing_boundary() {
    let pool = open_pool(None);
    let fixture = seed_fixture(&pool);
    pool.pause_for_approval(fixture.pause.clone())
        .await
        .expect("pause");
    pool.resolve_approval(fixture.allow).await.expect("approve");
    pool.claim_approved_effect(fixture.claim.clone())
        .await
        .expect("exact approval claim");
    let attempt_id = EffectAttemptId::new();
    pool.record_attempt_start(
        attempt_id,
        fixture.effect_id,
        fixture.claim.worker_id,
        serde_json::json!({"strategy":"approved-once"}),
    )
    .await
    .expect("durable approved attempt start");
    assert_eq!(
        pool.get_intent(fixture.effect_id)
            .await
            .expect("executing approval effect")
            .state,
        "executing"
    );
    let mut forbidden_reclaim = fixture.claim.clone();
    forbidden_reclaim.worker_id = WorkerId::new();
    assert!(pool.claim_approved_effect(forbidden_reclaim).await.is_err());
    pool.record_outcome(StoredOutcome {
        id: EffectOutcomeId::new(),
        intent_id: fixture.effect_id,
        attempt_id,
        run_id: fixture.run_id,
        consumed: false,
        payload: serde_json::json!({"variant":"success"}),
        observed_at: Utc::now(),
    })
    .await
    .expect("approved outcome");
    assert_eq!(
        pool.get_intent(fixture.effect_id)
            .await
            .expect("resolved approval effect")
            .state,
        "resolved"
    );
}

#[tokio::test]
async fn generic_state_updates_cannot_mint_approval_but_dlq_remains_supported() {
    let pool = open_pool(None);
    let fixture = seed_fixture(&pool);
    let intent = polkagent_store_trait::StoredIntent {
        id: fixture.effect_id,
        run_id: fixture.run_id,
        step_id: fixture.step_id,
        state: "pending".to_owned(),
        lease_owner: None,
        lease_expires: None,
        retry_class: StoreRetryClass::NoAutoRetry,
        payload: serde_json::json!({"kind":"tool_call","params":{}}),
        idempotency_key: format!("dlq-{}", fixture.effect_id),
        created_at: Utc::now(),
    };
    pool.propose_intent(intent)
        .await
        .expect("propose ordinary intent");
    for forbidden in ["approved", "denied"] {
        assert!(matches!(
            pool.update_intent_state(fixture.effect_id, forbidden).await,
            Err(polkagent_store_trait::StoreError::InvalidTransition { .. })
        ));
    }
    let dead = pool
        .update_intent_state(fixture.effect_id, "dead_lettered")
        .await
        .expect("DLQ terminal state remains supported");
    assert_eq!(dead.state, "dead_lettered");
    assert!(pool
        .claim_intent(WorkerId::new(), Duration::from_secs(60))
        .await
        .expect("generic claim query")
        .is_none());
}

#[test]
fn v18_backfill_is_deterministic_and_preserves_unknown_states() {
    let connection = Connection::open_in_memory().expect("open legacy fixture");
    connection
        .execute_batch(include_str!("../src/schema.sql"))
        .expect("apply v1");
    connection
        .execute_batch(include_str!("../src/v3_effect_store.sql"))
        .expect("apply v3");
    connection
        .execute_batch(include_str!("../src/v9_intent_priority.sql"))
        .expect("apply v9");
    let now = Utc::now().to_rfc3339();
    let agent_id = AgentId::new();
    let run_id = RunId::new();
    let turn_id = TurnId::new();
    let step_id = StepId::new();
    connection
        .execute(
            "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
             VALUES (?1, 'legacy', 'active', '{}', ?2, ?2)",
            rusqlite::params![agent_id.to_string(), now],
        )
        .expect("seed agent");
    connection
        .execute(
            "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at)
             VALUES (?1, ?2, 'running', '{}', ?3, ?3)",
            rusqlite::params![run_id.to_string(), agent_id.to_string(), now],
        )
        .expect("seed run");
    connection
        .execute(
            "INSERT INTO turns (id, run_id, sequence, role, started_at)
             VALUES (?1, ?2, 1, 'assistant', ?3)",
            rusqlite::params![turn_id.to_string(), run_id.to_string(), now],
        )
        .expect("seed turn");
    connection
        .execute(
            "INSERT INTO steps (id, turn_id, sequence, kind, started_at)
             VALUES (?1, ?2, 1, 'tool_call', ?3)",
            rusqlite::params![step_id.to_string(), turn_id.to_string(), now],
        )
        .expect("seed step");

    let resolved = EffectId::new();
    let claimed = EffectId::new();
    let unknown = EffectId::new();
    for (id, state, owner, lease) in [
        (resolved, "pending", Some("resolved".to_owned()), None),
        (
            claimed,
            "pending",
            Some(WorkerId::new().to_string()),
            Some((Utc::now() + ChronoDuration::minutes(1)).to_rfc3339()),
        ),
        (unknown, "future_state", None, None),
    ] {
        connection
            .execute(
                "INSERT INTO effect_intents
                    (id, run_id, turn_id, step_id, kind, params_json,
                     idempotency_key, created_at, claimed_by, claimed_until,
                     state, retry_class, priority)
                 VALUES (?1, ?2, ?3, ?4, 'tool_call', '{}', ?5, ?6, ?7, ?8, ?9,
                         'idempotent', 1)",
                rusqlite::params![
                    id.to_string(),
                    run_id.to_string(),
                    turn_id.to_string(),
                    step_id.to_string(),
                    format!("legacy-{id}"),
                    now,
                    owner,
                    lease,
                    state,
                ],
            )
            .expect("seed legacy intent");
    }

    connection
        .execute_batch(include_str!("../src/v18_approval_foundation.sql"))
        .expect("apply v18");
    let load = |id: EffectId| {
        connection
            .query_row(
                "SELECT state, claimed_by FROM effect_intents WHERE id = ?1",
                [id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .expect("load migrated intent")
    };
    assert_eq!(load(resolved), ("resolved".to_owned(), None));
    let claimed_row = load(claimed);
    assert_eq!(claimed_row.0, "claimed");
    assert!(claimed_row.1.is_some());
    assert_eq!(load(unknown), ("future_state".to_owned(), None));

    connection
        .execute(
            "UPDATE effect_intents SET state = 'dead_lettered' WHERE id = ?1",
            [unknown.to_string()],
        )
        .expect("DLQ state is accepted by V18 guard");
    let sentinel_error = connection
        .execute(
            "UPDATE effect_intents SET state = 'failed', claimed_by = 'failed'
             WHERE id = ?1",
            [unknown.to_string()],
        )
        .expect_err("state sentinels must be rejected after V18");
    assert!(sentinel_error
        .to_string()
        .contains("invalid effect intent state/lease combination"));
}

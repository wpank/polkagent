//! APR-00/APR-01 approval/checkpoint coordinator conformance and race tests.

#![allow(
    clippy::expect_used,
    reason = "integration fixtures stop at the exact violated persistence invariant"
)]

use std::path::Path;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use polkagent_core::{
    AgentId, ApprovalId, ConversationId, DataClassification, EffectId, PrincipalId, RunId, StepId,
    TurnId, WorkerId,
};
use polkagent_store_sqlite::{migrations, SqlitePool};
use polkagent_store_trait::approval::{
    ApprovalCoordinatorStore, ApprovalDecision, ApprovalPrincipalType, ApprovalRequestMetadata,
    ApprovalScope, ApprovalStatus, ApprovalStoreError, ApprovalSubject, CheckpointEffect,
    CheckpointEffectStatus, CheckpointStatus, ClaimApprovedEffect, ExecutionCheckpoint,
    ExecutionCheckpointStore, PauseForApproval, ResolveApproval, ResolveDisposition,
    APPROVAL_SUBJECT_SCHEMA_VERSION, EXECUTION_CHECKPOINT_SCHEMA_VERSION,
};
use polkagent_store_trait::conformance::{self, ApprovalConformanceFixture};
use polkagent_store_trait::event::EventStore as _;
use polkagent_store_trait::{EffectStore, StoreRetryClass};
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
    let deadline = now + ChronoDuration::minutes(10);

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
    assert_eq!(events[1].event_type, "approval_resolved");

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
                "UPDATE runs SET state = 'cancelled:external' WHERE id = ?1",
                [fixture.run_id.to_string()],
            )
            .expect("simulate an external winning run transition");
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

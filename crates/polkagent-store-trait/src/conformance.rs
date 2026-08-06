//! Shared conformance test suite for store trait implementations.
//!
//! This module defines the canonical set of behavioural tests that **every**
//! adapter implementing the store traits must pass (PRD-15).  Tests are plain
//! `async fn`s — not macros — so any adapter can call them from its own
//! `tests/` directory.
//!
//! # Covered traits
//!
//! - [`RunStore`] — via `test_run_store_*` functions
//! - [`EffectStore`] — via `test_effect_store_*` functions
//! - [`EventStore`] — via `test_event_store_*` functions
//! - [`ArtifactStore`] — via `test_artifact_store_*` functions
//!
//! # Design
//!
//! Each function receives a **pre-configured** store reference.  The caller
//! is responsible for:
//!
//! 1. Creating the store (in-memory or otherwise).
//! 2. Running any required schema migrations.
//! 3. Inserting any prerequisite records (e.g., agent rows required by foreign
//!    key constraints).
//! 4. Calling the conformance function.
//!
//! This keeps the conformance module free of adapter-specific setup details.

// Conformance helpers are assertion functions: a failed prerequisite should
// stop immediately with the operation-specific message supplied at each site.
#![allow(
    clippy::expect_used,
    reason = "conformance assertions intentionally panic with operation-specific diagnostics"
)]

use std::time::Duration;

use chrono::Utc;

use crate::approval::{
    ApprovalCoordinatorStore, ApprovalDecision, ApprovalPage, ApprovalScope, ApprovalStoreError,
    CheckpointCommitStatus, CheckpointProgress, CheckpointStatus, ClaimApprovedEffect,
    ExecutionCheckpointStore, PauseForApproval, ResolveApproval, ResolveDisposition,
};
use crate::event::{EventStore, EventStoreError, StoredEvent};
use crate::{
    ArtifactStore, EffectStore, RunStatus, RunStore, StoreError, StoreRetryClass, StoredIntent,
    StoredOutcome,
};
use polkagent_core::ids::{
    ArtifactId, EffectAttemptId, EffectId, EffectOutcomeId, RunId, StepId, TurnId, WorkerId,
};

/// Complete pre-seeded fixture for the shared approval foundation contract.
#[derive(Debug, Clone)]
pub struct ApprovalConformanceFixture {
    /// Atomic pause request whose parent rows already exist.
    pub pause: PauseForApproval,
    /// Authorized scope for reads.
    pub scope: ApprovalScope,
    /// Winning allow-once request.
    pub allow: ResolveApproval,
    /// Exact continuation claim.
    pub claim: ClaimApprovedEffect,
}

// ---------------------------------------------------------------------------
// Builder helpers
// ---------------------------------------------------------------------------

/// Build a minimal [`StoredIntent`] in the `"pending"` state.
///
/// The `step_id` must reference a step that satisfies any foreign-key
/// constraints imposed by the backing store.
#[must_use]
pub fn make_intent(run_id: RunId, step_id: StepId, idempotency_key: &str) -> StoredIntent {
    StoredIntent {
        id: EffectId::new(),
        run_id,
        step_id,
        state: "pending".into(),
        lease_owner: None,
        lease_expires: None,
        retry_class: StoreRetryClass::Idempotent,
        payload: serde_json::json!({"kind": "conformance_effect", "value": 1}),
        idempotency_key: idempotency_key.into(),
        created_at: Utc::now(),
    }
}

/// Build a minimal [`StoredOutcome`] linked to the given intent.
#[must_use]
pub fn make_outcome(
    intent_id: EffectId,
    attempt_id: EffectAttemptId,
    run_id: RunId,
) -> StoredOutcome {
    StoredOutcome {
        id: EffectOutcomeId::new(),
        intent_id,
        attempt_id,
        run_id,
        consumed: false,
        payload: serde_json::json!({"status": "success"}),
        observed_at: Utc::now(),
    }
}

/// Build a minimal [`StoredEvent`] for the given run and sequence.
#[must_use]
pub fn make_event(run_id: &str, sequence: u64, event_type: &str) -> StoredEvent {
    StoredEvent {
        id: uuid::Uuid::now_v7().to_string(),
        event_type: event_type.to_string(),
        sequence,
        global_sequence: 0, // assigned by the store
        run_id: run_id.to_string(),
        turn_id: None,
        step_id: None,
        effect_intent_id: None,
        effect_attempt_id: None,
        conversation_id: None,
        correlation_id: "conformance-corr".to_string(),
        causation_id: None,
        scope_id: "conformance-scope".to_string(),
        timestamp: Utc::now().to_rfc3339(),
        durability: "durable".to_string(),
        payload: serde_json::json!({"conformance": true}),
        trace_id: None,
        span_id: None,
        schema_version: 1,
    }
}

// ---------------------------------------------------------------------------
// RunStore conformance tests
// ---------------------------------------------------------------------------

/// Conformance: `create()` then `get()` returns the same data.
///
/// **Precondition:** the store must be empty (no prior run with the generated
/// ID).  The `agent_id` string must satisfy any foreign-key constraints of
/// the backing store (e.g., the agent must exist in an `agents` table).
pub async fn test_run_store_crud(store: &dyn RunStore, agent_id: &str) {
    let run_id = RunId::new();

    // Create a run.
    store
        .create(run_id, agent_id, RunStatus::new("created"))
        .await
        .expect("create() must not fail for a fresh run_id");

    // get() must return the same data.
    let summary = store
        .get(run_id)
        .await
        .expect("get() must succeed after create()");

    assert_eq!(summary.id, run_id, "get() must return the requested run_id");
    assert_eq!(
        summary.agent_id, agent_id,
        "get() must return the correct agent_id"
    );
    assert_eq!(
        summary.status.as_str(),
        "created",
        "get() must return the initial status"
    );
    assert!(
        summary.completed_at.is_none(),
        "completed_at must be None for a non-terminal status"
    );

    // update_state() must persist the new status.
    store
        .update_state(run_id, RunStatus::new("running"))
        .await
        .expect("update_state() must not fail for an existing run");

    let updated = store
        .get(run_id)
        .await
        .expect("get() must succeed after update_state()");

    assert_eq!(
        updated.status.as_str(),
        "running",
        "get() must reflect the new status after update_state()"
    );

    // list_by_agent() must include this run.
    let listed = store
        .list_by_agent(agent_id, 100, 0)
        .await
        .expect("list_by_agent() must not fail");

    assert!(
        listed.iter().any(|r| r.id == run_id),
        "list_by_agent() must include the created run"
    );

    // list_by_state() must find the run by its current status.
    let by_state = store
        .list_by_state(RunStatus::new("running"), 100, 0)
        .await
        .expect("list_by_state() must not fail");

    assert!(
        by_state.iter().any(|r| r.id == run_id),
        "list_by_state() must include the run with matching status"
    );
}

/// Conformance: `create()` with a duplicate `run_id` returns `StoreError::Conflict`.
pub async fn test_run_store_duplicate_conflict(store: &dyn RunStore, agent_id: &str) {
    let run_id = RunId::new();

    store
        .create(run_id, agent_id, RunStatus::new("created"))
        .await
        .expect("first create() must succeed");

    let err = store
        .create(run_id, agent_id, RunStatus::new("created"))
        .await
        .expect_err("second create() with the same run_id must fail");

    assert!(
        matches!(err, StoreError::Conflict { .. }),
        "duplicate create() must return StoreError::Conflict; got: {err:?}"
    );
}

/// Conformance: `get()` for a non-existent `run_id` returns `StoreError::NotFound`.
pub async fn test_run_store_get_not_found(store: &dyn RunStore) {
    let run_id = RunId::new();

    let err = store
        .get(run_id)
        .await
        .expect_err("get() on a non-existent run_id must fail");

    assert!(
        matches!(err, StoreError::NotFound { .. }),
        "get() on missing run must return StoreError::NotFound; got: {err:?}"
    );
}

/// Conformance: `update_state()` to a terminal status sets `completed_at`.
///
/// When a run transitions to `"completed"`, `"failed"`, `"cancelled"`, or
/// `"timed_out"`, the `completed_at` field must be populated.
pub async fn test_run_store_terminal_state_sets_completed_at(store: &dyn RunStore, agent_id: &str) {
    let run_id = RunId::new();
    store
        .create(run_id, agent_id, RunStatus::new("created"))
        .await
        .expect("create");

    for terminal in &["completed", "failed", "cancelled", "timed_out"] {
        store
            .update_state(run_id, RunStatus::new("running"))
            .await
            .expect("reset to running");

        store
            .update_state(run_id, RunStatus::new(*terminal))
            .await
            .expect("update to terminal");

        let summary = store.get(run_id).await.expect("get");
        assert_eq!(summary.status.as_str(), *terminal);
        assert!(
            summary.completed_at.is_some(),
            "completed_at must be set when transitioning to terminal status '{terminal}'"
        );
    }
}

/// Conformance: a normalized step can be inserted beneath an existing turn,
/// rejects a duplicate ID, and can then be completed exactly once.
///
/// **Precondition:** `turn_id` must already exist in the backing store.
pub async fn test_run_store_step_lifecycle(store: &dyn RunStore, turn_id: TurnId) {
    let step_id = StepId::new();
    store
        .insert_step(step_id, turn_id, 1, "tool_call", "2024-01-01T00:00:00Z")
        .await
        .expect("insert_step() must persist a fresh normalized step");

    let duplicate = store
        .insert_step(step_id, turn_id, 1, "tool_call", "2024-01-01T00:00:00Z")
        .await
        .expect_err("insert_step() must reject a duplicate step ID");
    assert!(
        matches!(duplicate, StoreError::Conflict { .. }),
        "duplicate normalized step must return StoreError::Conflict; got: {duplicate:?}"
    );

    store
        .complete_step(step_id, "2024-01-01T00:00:01Z")
        .await
        .expect("complete_step() must complete the persisted step");
    let second_completion = store
        .complete_step(step_id, "2024-01-01T00:00:02Z")
        .await
        .expect_err("complete_step() must not rewrite a completed step");
    assert!(
        matches!(second_completion, StoreError::NotFound { .. }),
        "second normalized-step completion must fail closed; got: {second_completion:?}"
    );
}

// ---------------------------------------------------------------------------
// EffectStore conformance tests
// ---------------------------------------------------------------------------

/// Conformance: full intent lifecycle — propose, claim, record attempt, record
/// outcome, mark consumed.
///
/// **Precondition:** `run_id` and `step_id` must already exist in the backing
/// store (satisfy any FK constraints).
pub async fn test_effect_store_crud(store: &dyn EffectStore, run_id: RunId, step_id: StepId) {
    // 1. Propose.
    let intent = make_intent(run_id, step_id, "conformance-crud-1");
    let intent_id = intent.id;
    store
        .propose_intent(intent)
        .await
        .expect("propose_intent() must succeed for a fresh intent");

    // 2. Verify get_intent() returns the proposed intent.
    let fetched = store
        .get_intent(intent_id)
        .await
        .expect("get_intent() must succeed after propose_intent()");

    assert_eq!(fetched.id, intent_id);
    assert_eq!(fetched.state, "pending");
    assert!(fetched.lease_owner.is_none());

    // 3. Claim.
    let worker = WorkerId::new();
    let claimed = store
        .claim_intent(worker, Duration::from_secs(300))
        .await
        .expect("claim_intent() must not fail")
        .expect("claim_intent() must return Some when a pending intent exists");

    assert_eq!(
        claimed.id, intent_id,
        "claimed intent must be the proposed one"
    );
    assert_eq!(claimed.state, "claimed");
    assert_eq!(claimed.lease_owner, Some(worker));
    assert!(claimed.lease_expires.is_some());

    // 4. Record attempt start.
    let attempt_id = EffectAttemptId::new();
    store
        .record_attempt_start(attempt_id, intent_id, worker, serde_json::json!({}))
        .await
        .expect("record_attempt_start() must not fail");
    let executing = store
        .get_intent(intent_id)
        .await
        .expect("get_intent() after durable attempt start");
    assert_eq!(
        executing.state, "executing",
        "attempt start must durably advance claimed to executing before I/O"
    );

    // 5. Record outcome.
    let outcome = make_outcome(intent_id, attempt_id, run_id);
    let outcome_id = outcome.id;
    store
        .record_outcome(outcome)
        .await
        .expect("record_outcome() must not fail");

    // Intent must now be resolved.
    let resolved = store
        .get_intent(intent_id)
        .await
        .expect("get_intent() after outcome");
    assert_eq!(
        resolved.state, "resolved",
        "intent must be resolved after recording an outcome"
    );

    // 6. Verify unconsumed_outcomes().
    let unconsumed = store
        .unconsumed_outcomes(run_id)
        .await
        .expect("unconsumed_outcomes() must not fail");

    assert!(
        unconsumed.iter().any(|o| o.id == outcome_id),
        "outcome must appear in unconsumed_outcomes()"
    );

    // 7. mark_outcomes_consumed is a no-op in the production schema (no
    //    `consumed` column exists).  We verify it succeeds without error but
    //    do not assert that outcomes disappear from unconsumed_outcomes().
    store
        .mark_outcomes_consumed(&[outcome_id])
        .await
        .expect("mark_outcomes_consumed() must not fail");
}

/// Conformance: `propose_intent()` with a duplicate ID returns `StoreError::Conflict`.
pub async fn test_effect_store_propose_duplicate_conflict(
    store: &dyn EffectStore,
    run_id: RunId,
    step_id: StepId,
) {
    let intent = make_intent(run_id, step_id, "conformance-dup-intent");
    let dup = StoredIntent {
        id: intent.id,
        idempotency_key: "different-key".into(),
        ..make_intent(run_id, step_id, "unused")
    };

    store.propose_intent(intent).await.expect("first propose");
    let err = store
        .propose_intent(dup)
        .await
        .expect_err("duplicate intent ID must fail");

    assert!(
        matches!(err, StoreError::Conflict { .. }),
        "propose_intent() with duplicate ID must return StoreError::Conflict; got: {err:?}"
    );
}

/// Conformance: `claim_intent()` on an empty store returns `None`.
pub async fn test_effect_store_claim_empty_returns_none(store: &dyn EffectStore) {
    let worker = WorkerId::new();
    let result = store
        .claim_intent(worker, Duration::from_secs(30))
        .await
        .expect("claim_intent() on empty store must not error");

    assert!(
        result.is_none(),
        "claim_intent() on empty store must return None"
    );
}

/// Conformance: `release_claim()` restores a claimed intent to `"pending"`.
pub async fn test_effect_store_release_restores_pending(
    store: &dyn EffectStore,
    run_id: RunId,
    step_id: StepId,
) {
    let intent = make_intent(run_id, step_id, "conformance-release");
    let intent_id = intent.id;
    store.propose_intent(intent).await.expect("propose");

    let worker = WorkerId::new();
    store
        .claim_intent(worker, Duration::from_secs(300))
        .await
        .expect("claim")
        .expect("Some");

    store
        .release_claim(intent_id, worker)
        .await
        .expect("release_claim() must not fail for a valid claim");

    let restored = store
        .get_intent(intent_id)
        .await
        .expect("get_intent() after release");

    assert_eq!(
        restored.state, "pending",
        "intent must return to 'pending' after release_claim()"
    );
    assert!(
        restored.lease_owner.is_none(),
        "lease_owner must be None after release_claim()"
    );
}

// ---------------------------------------------------------------------------
// EventStore conformance tests
// ---------------------------------------------------------------------------

/// Conformance: `append_durable()` then query by run returns all appended events.
pub async fn test_event_store_append_and_query(store: &dyn EventStore) {
    let run_id = uuid::Uuid::now_v7().to_string();

    let e1 = store
        .append_durable(make_event(&run_id, 1, "run_started"))
        .await
        .expect("append e1");

    let e2 = store
        .append_durable(make_event(&run_id, 2, "turn_started"))
        .await
        .expect("append e2");

    let e3 = store
        .append_durable(make_event(&run_id, 3, "step_started"))
        .await
        .expect("append e3");

    // global_sequence must be strictly increasing.
    assert!(
        e1.global_sequence < e2.global_sequence,
        "global_sequence must increase: e1={} e2={}",
        e1.global_sequence,
        e2.global_sequence,
    );
    assert!(
        e2.global_sequence < e3.global_sequence,
        "global_sequence must increase: e2={} e3={}",
        e2.global_sequence,
        e3.global_sequence,
    );

    // read_run_events() must return all three events.
    let run_uuid: RunId = run_id.parse().expect("parse run_id as RunId");
    let events = store
        .read_run_events(run_uuid)
        .await
        .expect("read_run_events()");

    assert_eq!(
        events.len(),
        3,
        "read_run_events() must return all 3 appended events"
    );
    assert_eq!(events[0].sequence, 1);
    assert_eq!(events[1].sequence, 2);
    assert_eq!(events[2].sequence, 3);
}

/// Conformance: non-monotonic per-run sequence is rejected.
pub async fn test_event_store_non_monotonic_sequence_rejected(store: &dyn EventStore) {
    let run_id = uuid::Uuid::now_v7().to_string();

    store
        .append_durable(make_event(&run_id, 1, "run_started"))
        .await
        .expect("first append");

    let err = store
        .append_durable(make_event(&run_id, 1, "turn_started"))
        .await
        .expect_err("duplicate sequence must fail");

    assert!(
        matches!(err, EventStoreError::NonMonotonicSequence { .. }),
        "non-monotonic sequence must return EventStoreError::NonMonotonicSequence; got: {err:?}"
    );
}

/// Conformance: duplicate terminal event is rejected.
pub async fn test_event_store_duplicate_terminal_rejected(store: &dyn EventStore) {
    let run_id = uuid::Uuid::now_v7().to_string();

    store
        .append_durable(make_event(&run_id, 1, "run_started"))
        .await
        .expect("start");

    store
        .append_durable(make_event(&run_id, 2, "run_completed"))
        .await
        .expect("first terminal");

    let err = store
        .append_durable(make_event(&run_id, 3, "run_failed"))
        .await
        .expect_err("second terminal must fail");

    assert!(
        matches!(err, EventStoreError::DuplicateTerminalEvent { .. }),
        "second terminal event must return EventStoreError::DuplicateTerminalEvent; got: {err:?}"
    );
}

/// Conformance: `read_from_cursor()` returns events with `global_sequence` > cursor.
pub async fn test_event_store_cursor_pagination(store: &dyn EventStore) {
    let run_id = uuid::Uuid::now_v7().to_string();

    let mut appended = Vec::new();
    for seq in 1..=4_u64 {
        let ev = store
            .append_durable(make_event(&run_id, seq, "turn_started"))
            .await
            .expect("append");
        appended.push(ev);
    }

    // Read from cursor = appended[1].global_sequence (i.e., get events 3 and 4).
    let cursor = appended[1].global_sequence;
    let page = store
        .read_from_cursor(cursor, 10)
        .await
        .expect("read_from_cursor()");

    assert_eq!(
        page.len(),
        2,
        "read_from_cursor() must return exactly 2 events after cursor={cursor}"
    );

    for ev in &page {
        assert!(
            ev.global_sequence > cursor,
            "all returned events must have global_sequence > cursor={cursor}; \
             got global_sequence={}",
            ev.global_sequence,
        );
    }
}

/// Conformance: exact event-ID lookup returns the stored record and reports a
/// typed not-found error for an absent ID.
pub async fn test_event_store_get_by_id(store: &dyn EventStore) {
    let run_id = uuid::Uuid::now_v7().to_string();
    let first = store
        .append_durable(make_event(&run_id, 1, "run_started"))
        .await
        .expect("append lookup prefix event");
    let target = store
        .append_durable(make_event(&run_id, 2, "turn_started"))
        .await
        .expect("append lookup target event");

    let found = store
        .get_event_by_id(&target.id)
        .await
        .expect("get_event_by_id() must find the exact event");
    assert_eq!(found.id, target.id);
    assert_eq!(found.global_sequence, target.global_sequence);
    assert_ne!(found.id, first.id);

    let error = store
        .get_event_by_id("00000000-0000-0000-0000-000000000000")
        .await
        .expect_err("missing event ID must fail");
    assert!(
        matches!(error, EventStoreError::NotFound(_)),
        "missing event ID must return EventStoreError::NotFound; got: {error:?}"
    );
}

/// Conformance: `max_sequence()` returns the current maximum per-run sequence.
pub async fn test_event_store_max_sequence(store: &dyn EventStore) {
    let run_id = uuid::Uuid::now_v7().to_string();

    let run_uuid: RunId = run_id.parse().expect("parse run_id");

    // No events yet — must return 0.
    let initial = store
        .max_sequence(run_uuid)
        .await
        .expect("max_sequence() on empty run");
    assert_eq!(
        initial, 0,
        "max_sequence() must return 0 for a run with no events"
    );

    store
        .append_durable(make_event(&run_id, 1, "run_started"))
        .await
        .expect("append seq 1");

    store
        .append_durable(make_event(&run_id, 2, "turn_started"))
        .await
        .expect("append seq 2");

    let after = store
        .max_sequence(run_uuid)
        .await
        .expect("max_sequence() after appending");
    assert_eq!(
        after, 2,
        "max_sequence() must return 2 after appending seq 1 and 2"
    );
}

/// Conformance: `has_terminal_event()` is `false` before and `true` after a
/// terminal event is appended.
pub async fn test_event_store_has_terminal_event(store: &dyn EventStore) {
    let run_id = uuid::Uuid::now_v7().to_string();
    let run_uuid: RunId = run_id.parse().expect("parse run_id");

    let before = store
        .has_terminal_event(run_uuid)
        .await
        .expect("has_terminal_event() before terminal");
    assert!(
        !before,
        "has_terminal_event() must be false before any terminal event"
    );

    store
        .append_durable(make_event(&run_id, 1, "run_started"))
        .await
        .expect("start");
    store
        .append_durable(make_event(&run_id, 2, "run_completed"))
        .await
        .expect("terminal");

    let after = store
        .has_terminal_event(run_uuid)
        .await
        .expect("has_terminal_event() after terminal");
    assert!(
        after,
        "has_terminal_event() must be true after a terminal event is appended"
    );
}

// ---------------------------------------------------------------------------
// ArtifactStore conformance tests
// ---------------------------------------------------------------------------

/// Conformance: `store()` then `get()` and `get_body()` return consistent data.
///
/// This test exercises the full artifact CRUD cycle.
pub async fn test_artifact_store_crud(store: &dyn ArtifactStore) {
    let artifact_id = ArtifactId::new();
    let body = b"conformance artifact body bytes";
    // Compute a deterministic "digest" (we use a fixed string; a real adapter
    // should use BLAKE3, but for conformance we just verify round-trip).
    let digest_hex = format!("{:064x}", 0xcafe_babe_u64);

    store
        .store(
            artifact_id,
            None,
            "conformance_test",
            "blake3",
            &digest_hex,
            "public",
            body,
        )
        .await
        .expect("store() must not fail for a valid artifact");

    // get() metadata.
    let summary = store
        .get(artifact_id)
        .await
        .expect("get() must succeed after store()");

    assert_eq!(
        summary.id, artifact_id,
        "get() must return the stored artifact_id"
    );
    assert_eq!(
        summary.kind, "conformance_test",
        "get() must return the stored kind"
    );
    assert_eq!(
        summary.algorithm, "blake3",
        "get() must return the stored algorithm"
    );
    assert_eq!(
        summary.digest_hex, digest_hex,
        "get() must return the stored digest_hex"
    );

    // Second store() with same ID must succeed (idempotency contract).
    store
        .store(
            artifact_id,
            None,
            "conformance_test",
            "blake3",
            &digest_hex,
            "public",
            body,
        )
        .await
        .expect("store() with the same artifact_id must succeed (idempotency contract)");
}

/// Conformance: `get()` on a non-existent artifact returns `StoreError::NotFound`.
pub async fn test_artifact_store_get_not_found(store: &dyn ArtifactStore) {
    let id = ArtifactId::new();
    let err = store
        .get(id)
        .await
        .expect_err("get() on a non-existent artifact must fail");

    assert!(
        matches!(err, StoreError::NotFound { .. }),
        "get() on missing artifact must return StoreError::NotFound; got: {err:?}"
    );
}

/// Conformance: `verify()` returns `false` for a non-existent artifact.
pub async fn test_artifact_store_verify_missing_returns_false(store: &dyn ArtifactStore) {
    let id = ArtifactId::new();
    let result = store
        .verify(id)
        .await
        .expect("verify() must not error for a non-existent artifact; expected false");

    assert!(
        !result,
        "verify() must return false for a non-existent artifact"
    );
}

/// Conformance: `list_for_run()` returns only artifacts for that run.
pub async fn test_artifact_store_list_for_run(store: &dyn ArtifactStore) {
    let run_a = RunId::new();
    let run_b = RunId::new();

    let run_a_artifact_one_id = ArtifactId::new();
    let run_a_artifact_two_id = ArtifactId::new();
    let run_b_artifact_id = ArtifactId::new();

    let run_a_digest_one = format!("{:064x}", 0xaaaa_u64);
    let run_a_digest_two = format!("{:064x}", 0xbbbb_u64);
    let run_b_digest = format!("{:064x}", 0xcccc_u64);

    store
        .store(
            run_a_artifact_one_id,
            Some(run_a),
            "test",
            "blake3",
            &run_a_digest_one,
            "public",
            b"body-a1",
        )
        .await
        .expect("store a1");
    store
        .store(
            run_a_artifact_two_id,
            Some(run_a),
            "test",
            "blake3",
            &run_a_digest_two,
            "public",
            b"body-a2",
        )
        .await
        .expect("store a2");
    store
        .store(
            run_b_artifact_id,
            Some(run_b),
            "test",
            "blake3",
            &run_b_digest,
            "public",
            b"body-b1",
        )
        .await
        .expect("store b1");

    let for_run_a = store
        .list_for_run(run_a)
        .await
        .expect("list_for_run() must not fail");

    assert_eq!(
        for_run_a.len(),
        2,
        "list_for_run() must return exactly 2 artifacts for run_a; got {}",
        for_run_a.len()
    );

    assert!(
        for_run_a.iter().all(|a| a.run_id == Some(run_a)),
        "list_for_run() must return only artifacts for run_a"
    );

    let for_run_b = store
        .list_for_run(run_b)
        .await
        .expect("list_for_run() for run_b");

    assert_eq!(
        for_run_b.len(),
        1,
        "list_for_run() must return 1 artifact for run_b; got {}",
        for_run_b.len()
    );
}

/// Conformance: expected-state run updates have exactly one winning prior state.
///
/// **Precondition:** `run_id` exists in the `running` state.
pub async fn test_run_store_compare_and_swap(store: &dyn RunStore, run_id: RunId) {
    let initial_version = store
        .state_version(run_id)
        .await
        .expect("read initial run state revision");
    let next_version = store
        .compare_and_swap_state(
            run_id,
            RunStatus::new("running"),
            initial_version,
            RunStatus::new("awaiting_approval:test"),
        )
        .await
        .expect("first expected-state run CAS must win");
    assert_eq!(next_version, initial_version + 1);
    assert_eq!(
        store
            .state_version(run_id)
            .await
            .expect("read advanced run state revision"),
        next_version
    );

    let stale = store
        .compare_and_swap_state(
            run_id,
            RunStatus::new("awaiting_approval:test"),
            initial_version,
            RunStatus::new("cancelled:stale"),
        )
        .await
        .expect_err("stale expected-state run CAS must fail");
    assert!(
        matches!(stale, StoreError::InvalidTransition { .. }),
        "stale run CAS must be an invalid transition, got {stale:?}"
    );
    let stored = store.get(run_id).await.expect("load CAS-updated run");
    assert_eq!(stored.status.as_str(), "awaiting_approval:test");
}

/// Conformance: the APR-00/APR-01 coordinator persists one atomic, resumable,
/// idempotent approval foundation.
///
/// **Precondition:** all run/turn/step/agent lineage referenced by `fixture`
/// exists, and the run is exactly `running`.
pub async fn test_approval_foundation<S>(store: &S, fixture: ApprovalConformanceFixture)
where
    S: ApprovalCoordinatorStore + ExecutionCheckpointStore + EffectStore,
{
    let approval = store
        .pause_for_approval(fixture.pause.clone())
        .await
        .expect("atomic pause_for_approval must succeed");
    assert_eq!(approval.status, crate::approval::ApprovalStatus::Pending);

    let checkpoint = store
        .get_checkpoint(fixture.pause.subject.run_id)
        .await
        .expect("paused checkpoint must be durable");
    assert_eq!(checkpoint.status, CheckpointStatus::PausedForApproval);
    assert_eq!(checkpoint.checkpoint.version, 1);

    // Generic workers cannot claim an approval-gated effect.
    let generic_claim = store
        .claim_intent(WorkerId::new(), Duration::from_secs(60))
        .await
        .expect("generic effect claim query must succeed");
    assert!(generic_claim.is_none());

    let listed = store
        .list_pending(
            fixture.scope.clone(),
            ApprovalPage {
                limit: 10,
                offset: 0,
            },
        )
        .await
        .expect("pending approval query must succeed");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, fixture.pause.approval_id);

    let resolved = store
        .resolve_approval(fixture.allow.clone())
        .await
        .expect("allow-once resolution must succeed");
    assert_eq!(resolved.disposition, ResolveDisposition::Applied);

    let retry = store
        .resolve_approval(fixture.allow.clone())
        .await
        .expect("identical resolution retry must succeed");
    assert_eq!(retry.disposition, ResolveDisposition::AlreadyApplied);
    assert_eq!(retry.approval.decided_at, resolved.approval.decided_at);
    assert_eq!(
        retry.approval.decision_event_id,
        resolved.approval.decision_event_id
    );

    let mut opposite = fixture.allow.clone();
    opposite.decision = ApprovalDecision::RejectOnce;
    let conflict = store
        .resolve_approval(opposite)
        .await
        .expect_err("different terminal decision must conflict");
    assert!(matches!(conflict, ApprovalStoreError::Conflict { .. }));

    let checkpoint = store
        .get_checkpoint(fixture.pause.subject.run_id)
        .await
        .expect("resolved checkpoint must remain durable");
    assert_eq!(checkpoint.status, CheckpointStatus::Resumable);
    assert_eq!(checkpoint.checkpoint.version, 2);

    let claimed = store
        .claim_approved_effect(fixture.claim.clone())
        .await
        .expect("exact continuation must claim approved effect");
    let retry_claim = store
        .claim_approved_effect(fixture.claim.clone())
        .await
        .expect("same claim retry must be idempotent");
    assert_eq!(retry_claim.lease_expires_at, claimed.lease_expires_at);

    let mut wrong_worker = fixture.claim.clone();
    wrong_worker.worker_id = WorkerId::new();
    let conflict = store
        .claim_approved_effect(wrong_worker)
        .await
        .expect_err("a second worker must not claim the approved effect");
    assert!(matches!(
        conflict,
        ApprovalStoreError::InvalidTransition { .. } | ApprovalStoreError::Conflict { .. }
    ));

    let leased = store
        .get_checkpoint(fixture.pause.subject.run_id)
        .await
        .expect("claim must lease checkpoint");
    assert_eq!(leased.status, CheckpointStatus::Leased);
    assert_eq!(leased.lease_owner, Some(fixture.claim.worker_id));

    let expected_version = leased.checkpoint.version;
    let mut next_checkpoint = leased.checkpoint;
    next_checkpoint.version += 1;
    "conformance-progress-digest".clone_into(&mut next_checkpoint.integrity_digest);
    let next_version = store
        .commit_progress(
            expected_version,
            CheckpointProgress {
                worker_id: fixture.claim.worker_id,
                checkpoint: next_checkpoint.clone(),
                status: CheckpointCommitStatus::Resumable,
            },
        )
        .await
        .expect("leased checkpoint progress CAS must succeed");
    assert_eq!(next_version, expected_version + 1);

    let stale = store
        .commit_progress(
            expected_version,
            CheckpointProgress {
                worker_id: fixture.claim.worker_id,
                checkpoint: next_checkpoint,
                status: CheckpointCommitStatus::Resumable,
            },
        )
        .await
        .expect_err("stale checkpoint progress CAS must fail");
    assert!(matches!(
        stale,
        ApprovalStoreError::InvalidTransition { .. }
    ));
}

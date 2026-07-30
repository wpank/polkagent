//! Full effect pipeline integration test.
//!
//! Exercises the complete effect lifecycle: propose -> claim -> attempt ->
//! outcome, plus idempotency key deduplication and crash recovery.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use polkagent_core::{EffectAttemptId, EffectId, EffectOutcomeId, RetryClass, RunId, StepId, WorkerId};
use polkagent_effect::pipeline::EffectIntentSpec;
use polkagent_effect::recovery::{CrashRecovery, RecoveryAction};
use polkagent_effect::types::{AttemptState, EffectAttempt, EffectKind, EffectOutcome, OutcomeResult};
use polkagent_effect::IdempotencyKey;
use polkagent_integration_tests::{make_effect_pipeline, MemEffectStore};
use polkagent_store_trait::{EffectStore, StoredIntent, StoreRetryClass};

// ---------------------------------------------------------------------------
// Helper: build a spec for a given run_id and kind
// ---------------------------------------------------------------------------

fn make_spec(run_id: RunId, kind: EffectKind) -> EffectIntentSpec {
    EffectIntentSpec {
        run_id,
        turn_id: polkagent_core::TurnId::new(),
        step_id: StepId::new(),
        kind,
        sequence: 1,
        idempotency_key: None,
        payload: serde_json::json!({"test": true}),
        retry_class: None,
        priority: None,
        max_attempts: None,
    }
}

// ---------------------------------------------------------------------------
// Full pipeline: propose -> claim -> attempt -> outcome
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_effect_lifecycle() {
    let (pipeline, store) = make_effect_pipeline();
    let run_id = RunId::new();

    // 1. Propose
    let intent_id = pipeline
        .propose(make_spec(run_id, EffectKind::ModelCall))
        .await
        .expect("propose");

    // Verify the intent is persisted in "pending" state.
    {
        let intents = store.intents.lock().expect("lock");
        let intent = intents.get(&intent_id).expect("intent exists");
        assert_eq!(intent.state, "pending");
    }

    // 2. Claim
    let guard = pipeline
        .claim_with_duration(Duration::from_secs(60))
        .await
        .expect("claim")
        .expect("guard exists");

    {
        let intents = store.intents.lock().expect("lock");
        let intent = intents.get(&intent_id).expect("intent exists");
        assert_eq!(intent.state, "claimed");
    }

    // 3. Record attempt
    let now = Utc::now();
    let attempt = EffectAttempt {
        id: EffectAttemptId::new(),
        intent_id,
        run_id,
        attempt_number: 1,
        idempotency_key: IdempotencyKey::generate(
            run_id,
            1,
            0,
            EffectKind::ModelCall,
            IdempotencyKey::hash_params(b"test"),
        ),
        worker_id: WorkerId::new(),
        lease_expires: now + chrono::Duration::seconds(60),
        retry_class: RetryClass::Idempotent,
        state: AttemptState::InProgress,
        created_at: now,
        claimed_at: now,
        started_at: Some(now),
        completed_at: None,
    };
    pipeline
        .record_attempt(intent_id, &attempt)
        .await
        .expect("record_attempt");

    {
        let attempts = store.attempts.lock().expect("lock");
        assert_eq!(attempts.len(), 1, "one attempt should be recorded");
    }

    // 4. Record outcome
    let outcome = EffectOutcome {
        id: EffectOutcomeId::new(),
        attempt_id: attempt.id,
        intent_id,
        run_id,
        result: OutcomeResult::Success {
            data: serde_json::json!({"ok": true}),
        },
        observed_at: Utc::now(),
        digest: [0u8; 32],
    };
    pipeline
        .record_outcome(intent_id, &outcome)
        .await
        .expect("record_outcome");

    {
        let intents = store.intents.lock().expect("lock");
        let intent = intents.get(&intent_id).expect("intent exists");
        assert_eq!(intent.state, "resolved");
    }
    {
        let outcomes = store.outcomes.lock().expect("lock");
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].consumed);
    }

    // Keep the guard alive until after outcome is recorded.
    // Explicitly forget so the Drop-based release does not interfere.
    std::mem::forget(guard);
}

// ---------------------------------------------------------------------------
// Idempotency key deduplication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_idempotency_key_is_rejected() {
    let (pipeline, _store) = make_effect_pipeline();
    let run_id = RunId::new();

    let key = IdempotencyKey::generate(
        run_id,
        1,
        0,
        EffectKind::ModelCall,
        IdempotencyKey::hash_params(b"fixed_params"),
    );

    let spec1 = EffectIntentSpec {
        run_id,
        turn_id: polkagent_core::TurnId::new(),
        step_id: StepId::new(),
        kind: EffectKind::ModelCall,
        sequence: 1,
        idempotency_key: Some(key),
        payload: serde_json::json!({}),
        retry_class: None,
        priority: None,
        max_attempts: None,
    };

    // First propose should succeed.
    pipeline.propose(spec1.clone()).await.expect("first propose");

    // Second propose with the same key must be rejected.
    let err = pipeline
        .propose(spec1.clone())
        .await
        .expect_err("second propose should fail");

    let err_msg = format!("{err:?}");
    assert!(
        err_msg.contains("Duplicate") || err_msg.contains("duplicate"),
        "expected duplicate error, got: {err_msg}"
    );
}

// ---------------------------------------------------------------------------
// Crash recovery: propose -> simulated crash -> recover -> no duplicates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn crash_recovery_finds_expired_leases() {
    let store = Arc::new(MemEffectStore::default());
    let store_dyn: Arc<dyn EffectStore> = Arc::clone(&store) as Arc<dyn EffectStore>;

    let run_id = RunId::new();
    let intent_id = EffectId::new();
    let worker_id = WorkerId::new();

    // Manually insert a "claimed" intent with an already-expired lease.
    let expired_intent = StoredIntent {
        id: intent_id,
        run_id,
        step_id: StepId::new(),
        state: "claimed".to_string(),
        lease_owner: Some(worker_id),
        lease_expires: Some(Utc::now() - chrono::Duration::seconds(10)),
        retry_class: StoreRetryClass::Idempotent,
        payload: serde_json::json!({"kind": "model_call"}),
        idempotency_key: "test-key-1".to_string(),
        created_at: Utc::now(),
    };
    store
        .propose_intent(expired_intent)
        .await
        .expect("insert expired");

    // Run crash recovery.
    let recovery = CrashRecovery::new(Arc::clone(&store_dyn));
    let actions = recovery.recover_all().await.expect("recover");

    assert_eq!(actions.len(), 1, "one expired lease found");
    assert!(
        matches!(&actions[0], RecoveryAction::Retry { .. }),
        "idempotent intent should be retried"
    );
    assert_eq!(actions[0].intent_id(), intent_id);
}

#[tokio::test]
async fn crash_recovery_no_auto_retry_marks_failed() {
    let store = Arc::new(MemEffectStore::default());
    let store_dyn: Arc<dyn EffectStore> = Arc::clone(&store) as Arc<dyn EffectStore>;

    let intent = StoredIntent {
        id: EffectId::new(),
        run_id: RunId::new(),
        step_id: StepId::new(),
        state: "claimed".to_string(),
        lease_owner: Some(WorkerId::new()),
        lease_expires: Some(Utc::now() - chrono::Duration::seconds(5)),
        retry_class: StoreRetryClass::NoAutoRetry,
        payload: serde_json::json!({"kind": "tool_call"}),
        idempotency_key: "test-key-2".to_string(),
        created_at: Utc::now(),
    };
    store.propose_intent(intent).await.expect("insert");

    let recovery = CrashRecovery::new(store_dyn);
    let actions = recovery.recover_all().await.expect("recover");

    assert_eq!(actions.len(), 1);
    assert!(
        matches!(&actions[0], RecoveryAction::MarkFailed { .. }),
        "NoAutoRetry non-signing intent should be marked failed"
    );
}

// ---------------------------------------------------------------------------
// Outcome immutability: recording same outcome twice fails
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recording_outcome_twice_fails() {
    let (pipeline, _store) = make_effect_pipeline();
    let run_id = RunId::new();

    let intent_id = pipeline
        .propose(make_spec(run_id, EffectKind::ToolCall))
        .await
        .expect("propose");

    let guard = pipeline
        .claim_with_duration(Duration::from_secs(60))
        .await
        .expect("claim")
        .expect("guard");

    let outcome = EffectOutcome {
        id: EffectOutcomeId::new(),
        attempt_id: EffectAttemptId::new(),
        intent_id,
        run_id,
        result: OutcomeResult::Success {
            data: serde_json::json!({"ok": true}),
        },
        observed_at: Utc::now(),
        digest: [0u8; 32],
    };

    pipeline
        .record_outcome(intent_id, &outcome)
        .await
        .expect("first outcome");

    let err = pipeline
        .record_outcome(intent_id, &outcome)
        .await
        .expect_err("second outcome should fail");

    let err_msg = format!("{err:?}");
    assert!(
        err_msg.contains("AlreadyRecorded") || err_msg.contains("Conflict"),
        "expected outcome-already-recorded error, got: {err_msg}"
    );

    std::mem::forget(guard);
}

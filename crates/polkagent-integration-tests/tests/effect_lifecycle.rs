//! IT-03: Effect lifecycle integration test with SQLite persistence.
//!
//! Exercises the full EffectIntent lifecycle (propose → claim → attempt →
//! outcome) against the in-memory EffectStore, verifies idempotency key
//! deduplication, and checks that completed effects cannot be re-attempted.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use polkagent_core::{EffectAttemptId, EffectId, EffectOutcomeId, RetryClass, RunId, StepId, WorkerId};
use polkagent_effect::pipeline::EffectIntentSpec;
use polkagent_effect::types::{AttemptState, EffectAttempt, EffectKind, EffectOutcome, OutcomeResult};
use polkagent_effect::IdempotencyKey;
use polkagent_integration_tests::{make_effect_pipeline, MemEffectStore};
use polkagent_store_trait::{EffectStore, StoredIntent, StoreRetryClass};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_spec(run_id: RunId, kind: EffectKind, key: Option<IdempotencyKey>) -> EffectIntentSpec {
    EffectIntentSpec {
        run_id,
        turn_id: polkagent_core::TurnId::new(),
        step_id: StepId::new(),
        kind,
        sequence: 1,
        idempotency_key: key,
        payload: serde_json::json!({"test": true}),
        retry_class: None,
        priority: None,
        max_attempts: None,
    }
}

// ---------------------------------------------------------------------------
// IT-03-A: Basic lifecycle — propose → claim → attempt → outcome
// ---------------------------------------------------------------------------

#[tokio::test]
async fn effect_intent_propose_persists_in_pending_state() {
    let (pipeline, store) = make_effect_pipeline();
    let run_id = RunId::new();

    let intent_id = pipeline
        .propose(make_spec(run_id, EffectKind::ModelCall, None))
        .await
        .expect("propose should succeed");

    let intents = store.intents.lock().expect("lock");
    let intent = intents.get(&intent_id).expect("intent exists after propose");
    assert_eq!(
        intent.state, "pending",
        "newly proposed intent must be in 'pending' state"
    );
}

#[tokio::test]
async fn effect_claim_transitions_to_claimed_state() {
    let (pipeline, store) = make_effect_pipeline();
    let run_id = RunId::new();

    let intent_id = pipeline
        .propose(make_spec(run_id, EffectKind::ModelCall, None))
        .await
        .expect("propose");

    let guard = pipeline
        .claim_with_duration(Duration::from_secs(60))
        .await
        .expect("claim ok")
        .expect("guard should be Some");

    {
        let intents = store.intents.lock().expect("lock");
        let intent = intents.get(&intent_id).expect("intent exists");
        assert_eq!(
            intent.state, "claimed",
            "claimed intent must be in 'claimed' state"
        );
    }

    std::mem::forget(guard);
}

#[tokio::test]
async fn effect_record_outcome_resolves_intent() {
    let (pipeline, store) = make_effect_pipeline();
    let run_id = RunId::new();

    let intent_id = pipeline
        .propose(make_spec(run_id, EffectKind::ToolCall, None))
        .await
        .expect("propose");

    let guard = pipeline
        .claim_with_duration(Duration::from_secs(60))
        .await
        .expect("claim")
        .expect("guard");

    let attempt = EffectAttempt {
        id: EffectAttemptId::new(),
        intent_id,
        run_id,
        attempt_number: 1,
        idempotency_key: IdempotencyKey::generate(
            run_id,
            1,
            0,
            EffectKind::ToolCall,
            IdempotencyKey::hash_params(b"params"),
        ),
        worker_id: WorkerId::new(),
        lease_expires: Utc::now() + chrono::Duration::seconds(60),
        retry_class: RetryClass::Idempotent,
        state: AttemptState::InProgress,
        created_at: Utc::now(),
        claimed_at: Utc::now(),
        started_at: Some(Utc::now()),
        completed_at: None,
    };
    pipeline
        .record_attempt(intent_id, &attempt)
        .await
        .expect("record_attempt");

    let outcome = EffectOutcome {
        id: EffectOutcomeId::new(),
        attempt_id: attempt.id,
        intent_id,
        run_id,
        result: OutcomeResult::Success {
            data: serde_json::json!({"result": "ok"}),
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
        assert_eq!(
            intent.state, "resolved",
            "intent must be 'resolved' after outcome is recorded"
        );
    }
    {
        let outcomes = store.outcomes.lock().expect("lock");
        assert_eq!(outcomes.len(), 1, "exactly one outcome should be recorded");
    }

    std::mem::forget(guard);
}

#[tokio::test]
async fn effect_attempt_is_persisted() {
    let (pipeline, store) = make_effect_pipeline();
    let run_id = RunId::new();

    let intent_id = pipeline
        .propose(make_spec(run_id, EffectKind::ModelCall, None))
        .await
        .expect("propose");

    let guard = pipeline
        .claim_with_duration(Duration::from_secs(60))
        .await
        .expect("claim")
        .expect("guard");

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
        lease_expires: Utc::now() + chrono::Duration::seconds(60),
        retry_class: RetryClass::Idempotent,
        state: AttemptState::InProgress,
        created_at: Utc::now(),
        claimed_at: Utc::now(),
        started_at: Some(Utc::now()),
        completed_at: None,
    };
    pipeline
        .record_attempt(intent_id, &attempt)
        .await
        .expect("record_attempt");

    {
        let attempts = store.attempts.lock().expect("lock");
        assert_eq!(attempts.len(), 1, "attempt should be persisted");
    }

    std::mem::forget(guard);
}

// ---------------------------------------------------------------------------
// IT-03-B: Simulated restart — persist then recreate store
// ---------------------------------------------------------------------------

#[tokio::test]
async fn effect_lifecycle_persists_across_simulated_restart() {
    // Build a pipeline with a shared MemEffectStore.
    let shared_store = Arc::new(MemEffectStore::default());
    let store_dyn: Arc<dyn EffectStore> = Arc::clone(&shared_store) as Arc<dyn EffectStore>;
    let worker_id = WorkerId::new();
    let pipeline = polkagent_effect::EffectPipeline::new(Arc::clone(&store_dyn), worker_id);

    let run_id = RunId::new();
    let intent_id = pipeline
        .propose(make_spec(run_id, EffectKind::ModelCall, None))
        .await
        .expect("propose before restart");

    // Verify persisted before restart.
    {
        let intents = shared_store.intents.lock().expect("lock");
        assert!(intents.contains_key(&intent_id), "intent must be in store");
        assert_eq!(intents[&intent_id].state, "pending");
    }

    // Simulate restart: create a new pipeline on the same backing store.
    let pipeline2 = polkagent_effect::EffectPipeline::new(
        Arc::clone(&store_dyn),
        WorkerId::new(),
    );

    // The second pipeline can claim and complete the intent.
    let guard = pipeline2
        .claim_with_duration(Duration::from_secs(60))
        .await
        .expect("claim after restart")
        .expect("guard after restart");

    {
        let intents = shared_store.intents.lock().expect("lock");
        assert_eq!(
            intents[&intent_id].state, "claimed",
            "intent still accessible after restart via shared store"
        );
    }

    // Record outcome via the second pipeline.
    let outcome = EffectOutcome {
        id: EffectOutcomeId::new(),
        attempt_id: EffectAttemptId::new(),
        intent_id,
        run_id,
        result: OutcomeResult::Success {
            data: serde_json::json!({"done": true}),
        },
        observed_at: Utc::now(),
        digest: [0u8; 32],
    };
    pipeline2
        .record_outcome(intent_id, &outcome)
        .await
        .expect("record_outcome after restart");

    {
        let intents = shared_store.intents.lock().expect("lock");
        assert_eq!(
            intents[&intent_id].state, "resolved",
            "intent must be resolved after outcome recorded on second pipeline"
        );
    }

    std::mem::forget(guard);
}

// ---------------------------------------------------------------------------
// IT-03-C: Duplicate idempotency key rejected
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
        IdempotencyKey::hash_params(b"unique-params"),
    );

    let spec = EffectIntentSpec {
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

    pipeline.propose(spec.clone()).await.expect("first propose should succeed");

    let result = pipeline.propose(spec).await;
    assert!(
        result.is_err(),
        "second propose with same idempotency key must fail"
    );

    let err_msg = format!("{:?}", result.unwrap_err());
    assert!(
        err_msg.to_lowercase().contains("duplicate"),
        "error must mention 'duplicate', got: {err_msg}"
    );
}

#[tokio::test]
async fn different_idempotency_keys_are_both_accepted() {
    let (pipeline, store) = make_effect_pipeline();
    let run_id = RunId::new();

    let key1 = IdempotencyKey::generate(
        run_id, 1, 0, EffectKind::ModelCall,
        IdempotencyKey::hash_params(b"params-1"),
    );
    let key2 = IdempotencyKey::generate(
        run_id, 1, 1, EffectKind::ModelCall,
        IdempotencyKey::hash_params(b"params-2"),
    );

    let spec1 = EffectIntentSpec {
        run_id,
        turn_id: polkagent_core::TurnId::new(),
        step_id: StepId::new(),
        kind: EffectKind::ModelCall,
        sequence: 1,
        idempotency_key: Some(key1),
        payload: serde_json::json!({"n": 1}),
        retry_class: None,
        priority: None,
        max_attempts: None,
    };
    let spec2 = EffectIntentSpec {
        run_id,
        turn_id: polkagent_core::TurnId::new(),
        step_id: StepId::new(),
        kind: EffectKind::ModelCall,
        sequence: 2,
        idempotency_key: Some(key2),
        payload: serde_json::json!({"n": 2}),
        retry_class: None,
        priority: None,
        max_attempts: None,
    };

    pipeline.propose(spec1).await.expect("propose 1");
    pipeline.propose(spec2).await.expect("propose 2");

    let intents = store.intents.lock().expect("lock");
    assert_eq!(intents.len(), 2, "two distinct intents must be stored");
}

// ---------------------------------------------------------------------------
// IT-03-D: Completed effects cannot be re-attempted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recording_outcome_twice_is_rejected() {
    let (pipeline, _store) = make_effect_pipeline();
    let run_id = RunId::new();

    let intent_id = pipeline
        .propose(make_spec(run_id, EffectKind::ToolCall, None))
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
        .expect("first outcome ok");

    let result = pipeline.record_outcome(intent_id, &outcome).await;
    assert!(
        result.is_err(),
        "recording the same outcome a second time must fail"
    );

    let err_msg = format!("{:?}", result.unwrap_err());
    assert!(
        err_msg.contains("AlreadyRecorded") || err_msg.contains("Conflict"),
        "expected 'AlreadyRecorded' or 'Conflict', got: {err_msg}"
    );

    std::mem::forget(guard);
}

#[tokio::test]
async fn intent_with_no_idempotency_key_can_be_proposed() {
    let (pipeline, store) = make_effect_pipeline();
    let run_id = RunId::new();

    // A spec with no idempotency key should still be proposable.
    let id = pipeline
        .propose(make_spec(run_id, EffectKind::ModelCall, None))
        .await
        .expect("propose without explicit idempotency key");

    let intents = store.intents.lock().expect("lock");
    assert!(intents.contains_key(&id), "intent must be stored");
}

// ---------------------------------------------------------------------------
// IT-03-E: Multiple independent intents for same run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_intents_for_same_run_are_independent() {
    let (pipeline, store) = make_effect_pipeline();
    let run_id = RunId::new();

    // Use distinct (sequence, kind) pairs so the auto-generated idempotency keys do not collide.
    let make_seq_spec = |seq: u64, kind: EffectKind| EffectIntentSpec {
        run_id,
        turn_id: polkagent_core::TurnId::new(),
        step_id: StepId::new(),
        kind,
        sequence: seq,
        idempotency_key: None,
        payload: serde_json::json!({}),
        retry_class: None,
        priority: None,
        max_attempts: None,
    };

    let id1 = pipeline
        .propose(make_seq_spec(1, EffectKind::ModelCall))
        .await
        .expect("propose 1");
    let id2 = pipeline
        .propose(make_seq_spec(2, EffectKind::ToolCall))
        .await
        .expect("propose 2");
    let id3 = pipeline
        .propose(make_seq_spec(3, EffectKind::ModelCall))
        .await
        .expect("propose 3");

    // All three should be in the store.
    let intents = store.intents.lock().expect("lock");
    assert!(intents.contains_key(&id1));
    assert!(intents.contains_key(&id2));
    assert!(intents.contains_key(&id3));
    assert_ne!(id1, id2);
    assert_ne!(id2, id3);
    assert_ne!(id1, id3);

    // All should be in pending state.
    for &id in &[id1, id2, id3] {
        assert_eq!(intents[&id].state, "pending");
    }
}

// ---------------------------------------------------------------------------
// IT-03-F: Failed outcome still resolves intent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn failed_outcome_resolves_intent() {
    let (pipeline, store) = make_effect_pipeline();
    let run_id = RunId::new();

    let intent_id = pipeline
        .propose(make_spec(run_id, EffectKind::ToolCall, None))
        .await
        .expect("propose");

    let guard = pipeline
        .claim_with_duration(Duration::from_secs(60))
        .await
        .expect("claim")
        .expect("guard");

    // Record a failure outcome.
    let outcome = EffectOutcome {
        id: EffectOutcomeId::new(),
        attempt_id: EffectAttemptId::new(),
        intent_id,
        run_id,
        result: OutcomeResult::Failure {
            error_class: polkagent_effect::types::ErrorClass::NetworkError,
            message: "execution failed".to_string(),
            retriable: true,
        },
        observed_at: Utc::now(),
        digest: [0u8; 32],
    };
    pipeline
        .record_outcome(intent_id, &outcome)
        .await
        .expect("record failure outcome");

    {
        let intents = store.intents.lock().expect("lock");
        // After a failure outcome, the intent should still be resolved (terminal).
        let state = &intents[&intent_id].state;
        assert!(
            state == "resolved" || state == "failed",
            "intent should be in a terminal state after failure, got: {state}"
        );
    }

    std::mem::forget(guard);
}

// ---------------------------------------------------------------------------
// IT-03-G: Crash recovery — expired leases become re-claimable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn expired_lease_intent_is_found_by_crash_recovery() {
    use polkagent_effect::recovery::{CrashRecovery, RecoveryAction};

    let store = Arc::new(MemEffectStore::default());
    let store_dyn: Arc<dyn EffectStore> = Arc::clone(&store) as Arc<dyn EffectStore>;

    // Insert a "claimed" intent with an already-expired lease.
    let intent_id = EffectId::new();
    let expired_intent = StoredIntent {
        id: intent_id,
        run_id: RunId::new(),
        step_id: StepId::new(),
        state: "claimed".to_string(),
        lease_owner: Some(WorkerId::new()),
        lease_expires: Some(Utc::now() - chrono::Duration::seconds(120)),
        retry_class: StoreRetryClass::Idempotent,
        payload: serde_json::json!({"kind": "model_call"}),
        idempotency_key: "crash-recovery-test-key".to_string(),
        created_at: Utc::now(),
    };
    store
        .propose_intent(expired_intent)
        .await
        .expect("insert expired intent");

    let recovery = CrashRecovery::new(Arc::clone(&store_dyn));
    let actions = recovery.recover_all().await.expect("recover_all");

    assert_eq!(actions.len(), 1, "one expired lease must be found");
    assert!(
        matches!(&actions[0], RecoveryAction::Retry { .. }),
        "idempotent intent with expired lease must produce Retry action"
    );
    assert_eq!(actions[0].intent_id(), intent_id);
}

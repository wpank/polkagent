//! Fault injection integration tests (PRD-15 §5.5).
//!
//! These tests verify crash safety and resilience properties of the Polkagent
//! effect pipeline by injecting controlled faults at key persistence boundaries.
//!
//! # Test cases
//!
//! - **FI-01**: Process crash at the intent-persist boundary — the intent
//!   survives and is recoverable after restart.
//! - **FI-02**: Process crash after attempt-start is recorded but before
//!   the outcome is persisted — the attempt survives and the outcome is retried.
//! - **FI-03**: Process crash during outbox drain — no duplicate delivery
//!   occurs on restart because the outbox is durable.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;

use polkagent_core::{
    EffectAttemptId, EffectId, EffectOutcomeId, RunId, StepId, WorkerId,
};
use polkagent_effect::pipeline::EffectIntentSpec;
use polkagent_effect::types::{AttemptState, EffectAttempt, EffectKind, EffectOutcome, OutcomeResult};
use polkagent_effect::{EffectPipeline, IdempotencyKey};
use polkagent_fault::{Fault, FaultInjector, FaultSchedule};
use polkagent_fault::store::FaultStore;
use polkagent_store_trait::{
    EffectStore, StoredIntent, StoredOutcome, StoreError, StoreRetryClass,
};

// ---------------------------------------------------------------------------
// Minimal in-memory EffectStore (shared with other integration tests)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct MemEffectStore {
    pub intents: Mutex<HashMap<EffectId, StoredIntent>>,
    pub attempts: Mutex<Vec<serde_json::Value>>,
    pub outcomes: Mutex<Vec<StoredOutcome>>,
}

#[async_trait::async_trait]
impl EffectStore for MemEffectStore {
    async fn propose_intent(&self, intent: StoredIntent) -> Result<(), StoreError> {
        let mut intents = self.intents.lock().expect("lock");
        if intents.contains_key(&intent.id) {
            return Err(StoreError::Conflict {
                resource_type: "EffectIntent",
                id: intent.id.to_string(),
            });
        }
        intents.insert(intent.id, intent);
        Ok(())
    }

    async fn claim_intent(
        &self,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<Option<StoredIntent>, StoreError> {
        let mut intents = self.intents.lock().expect("lock");
        let now = Utc::now();
        let pending_id = intents
            .values()
            .find(|i| i.state.eq_ignore_ascii_case("pending"))
            .map(|i| i.id);
        match pending_id {
            None => Ok(None),
            Some(id) => {
                let intent = intents.get_mut(&id).expect("exists");
                let expires = now
                    + chrono::Duration::from_std(lease_duration)
                        .unwrap_or_else(|_| chrono::Duration::seconds(60));
                intent.state = "claimed".to_string();
                intent.lease_owner = Some(worker_id);
                intent.lease_expires = Some(expires);
                Ok(Some(intent.clone()))
            }
        }
    }

    async fn claim_intent_by_id(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<StoredIntent, StoreError> {
        let mut intents = self.intents.lock().expect("lock");
        let now = Utc::now();
        match intents.get_mut(&intent_id) {
            None => Err(StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            }),
            Some(intent) => {
                let expires = now
                    + chrono::Duration::from_std(lease_duration)
                        .unwrap_or_else(|_| chrono::Duration::seconds(60));
                intent.state = "claimed".to_string();
                intent.lease_owner = Some(worker_id);
                intent.lease_expires = Some(expires);
                Ok(intent.clone())
            }
        }
    }

    async fn release_claim(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
    ) -> Result<(), StoreError> {
        let mut intents = self.intents.lock().expect("lock");
        if let Some(intent) = intents.get_mut(&intent_id) {
            if intent.lease_owner == Some(worker_id) {
                intent.state = "pending".to_string();
                intent.lease_owner = None;
                intent.lease_expires = None;
            }
        }
        Ok(())
    }

    async fn update_intent_state(
        &self,
        intent_id: EffectId,
        new_state: &str,
    ) -> Result<StoredIntent, StoreError> {
        let mut intents = self.intents.lock().expect("lock");
        match intents.get_mut(&intent_id) {
            None => Err(StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            }),
            Some(intent) => {
                intent.state = new_state.to_string();
                Ok(intent.clone())
            }
        }
    }

    async fn get_intent(&self, intent_id: EffectId) -> Result<StoredIntent, StoreError> {
        self.intents
            .lock()
            .expect("lock")
            .get(&intent_id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            })
    }

    async fn get_by_run(&self, run_id: RunId) -> Result<Vec<StoredIntent>, StoreError> {
        Ok(self
            .intents
            .lock()
            .expect("lock")
            .values()
            .filter(|i| i.run_id == run_id)
            .cloned()
            .collect())
    }

    async fn expired_leases(
        &self,
        cutoff: polkagent_core::Timestamp,
    ) -> Result<Vec<StoredIntent>, StoreError> {
        Ok(self
            .intents
            .lock()
            .expect("lock")
            .values()
            .filter(|i| {
                i.state.eq_ignore_ascii_case("claimed")
                    && i.lease_expires.map(|exp| exp < cutoff).unwrap_or(false)
            })
            .cloned()
            .collect())
    }

    async fn record_attempt_start(
        &self,
        _attempt_id: EffectAttemptId,
        _intent_id: EffectId,
        _worker_id: WorkerId,
        payload: serde_json::Value,
    ) -> Result<(), StoreError> {
        self.attempts.lock().expect("lock").push(payload);
        Ok(())
    }

    async fn record_outcome(&self, outcome: StoredOutcome) -> Result<(), StoreError> {
        let mut outcomes = self.outcomes.lock().expect("lock");
        if outcomes.iter().any(|o| o.id == outcome.id) {
            return Err(StoreError::Conflict {
                resource_type: "EffectOutcome",
                id: outcome.id.to_string(),
            });
        }
        let mut intents = self.intents.lock().expect("lock");
        if let Some(intent) = intents.get_mut(&outcome.intent_id) {
            intent.state = "resolved".to_string();
            intent.lease_owner = None;
            intent.lease_expires = None;
        }
        outcomes.push(outcome);
        Ok(())
    }

    async fn unconsumed_outcomes(
        &self,
        run_id: RunId,
    ) -> Result<Vec<StoredOutcome>, StoreError> {
        Ok(self
            .outcomes
            .lock()
            .expect("lock")
            .iter()
            .filter(|o| o.run_id == run_id && !o.consumed)
            .cloned()
            .collect())
    }

    async fn mark_outcomes_consumed(
        &self,
        outcome_ids: &[EffectOutcomeId],
    ) -> Result<(), StoreError> {
        let mut outcomes = self.outcomes.lock().expect("lock");
        for o in outcomes.iter_mut() {
            if outcome_ids.contains(&o.id) {
                o.consumed = true;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_spec(run_id: RunId) -> EffectIntentSpec {
    EffectIntentSpec {
        run_id,
        turn_id: polkagent_core::TurnId::new(),
        step_id: StepId::new(),
        kind: EffectKind::ModelCall,
        sequence: 1,
        idempotency_key: None,
        payload: serde_json::json!({"test": true}),
        retry_class: None,
        priority: None,
        max_attempts: None,
        action_card: None,
    }
}

fn make_attempt(intent_id: EffectId, run_id: RunId) -> EffectAttempt {
    let now = Utc::now();
    EffectAttempt {
        id: EffectAttemptId::new(),
        intent_id,
        run_id,
        attempt_number: 1,
        idempotency_key: IdempotencyKey::generate(
            run_id,
            1,
            0,
            EffectKind::ModelCall,
            IdempotencyKey::hash_params(b"fi-test"),
        ),
        worker_id: WorkerId::new(),
        lease_expires: now + chrono::Duration::seconds(60),
        retry_class: polkagent_core::RetryClass::Idempotent,
        state: AttemptState::InProgress,
        created_at: now,
        claimed_at: now,
        started_at: Some(now),
        completed_at: None,
    }
}

fn make_outcome(attempt: &EffectAttempt) -> EffectOutcome {
    EffectOutcome {
        id: EffectOutcomeId::new(),
        attempt_id: attempt.id,
        intent_id: attempt.intent_id,
        run_id: attempt.run_id,
        result: OutcomeResult::Success { data: serde_json::json!({"ok": true}) },
        observed_at: Utc::now(),
        digest: [0u8; 32],
    }
}

// ---------------------------------------------------------------------------
// FI-01: Crash at intent-persist boundary — intent survives
// ---------------------------------------------------------------------------

/// FI-01: A simulated crash immediately after writing the intent to the store
/// must leave the intent in a recoverable "pending" state.
///
/// Scenario:
/// 1. Propose an intent → write succeeds (intent lands in store).
/// 2. Inject a crash fault on the NEXT write (`after_write`, `Once`).
/// 3. Simulate "after crash recovery" by reading the intent back directly
///    from the backing store.
/// 4. Confirm the intent is in "pending" state and can be reclaimed.
#[tokio::test]
async fn fi_01_intent_survives_crash_at_persist_boundary() {
    let backing = Arc::new(MemEffectStore::default());
    let injector = Arc::new(FaultInjector::new());

    // Wrap with FaultStore.
    let fault_store = Arc::new(FaultStore::new(
        Arc::clone(&backing) as Arc<dyn EffectStore>,
        Arc::clone(&injector),
    ));

    let run_id = RunId::new();
    let spec = make_spec(run_id);

    // Inject crash on the after_write point so it fires AFTER the intent
    // is durably written but before any post-write logic completes.
    injector.add_fault("after_write", Fault::Crash, FaultSchedule::Once);

    // Propose the intent. The FaultStore will:
    //   1. Check before_write (no fault) → OK.
    //   2. Forward to inner store → intent is now DURABLE.
    //   3. Check after_write → CRASH (panic).
    // We spawn a dedicated OS thread with its own Tokio runtime so that
    // catch_unwind does not run inside the test's Tokio runtime.
    let fault_store_clone = Arc::clone(&fault_store);
    let propose_result = std::thread::spawn(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let rt = tokio::runtime::Runtime::new().expect("rt");
            rt.block_on(async {
                let pipeline = EffectPipeline::new(
                    fault_store_clone as Arc<dyn EffectStore>,
                    WorkerId::new(),
                );
                pipeline.propose(spec).await
            })
        }))
    })
    .join()
    .expect("thread join");

    // The crash happened AFTER the write — the panic is expected.
    assert!(propose_result.is_err(), "crash fault should panic");

    // After restart: the backing store still holds the intent.
    let intents = backing.intents.lock().expect("lock");
    assert_eq!(intents.len(), 1, "intent must survive the crash");
    let intent = intents.values().next().expect("one intent");
    assert_eq!(intent.state, "pending", "intent state must be pending after crash");
    assert_eq!(intent.run_id, run_id, "intent belongs to the correct run");
}

// ---------------------------------------------------------------------------
// FI-02: Crash after attempt but before outcome
// ---------------------------------------------------------------------------

/// FI-02: A simulated crash after recording the attempt start but before
/// recording the outcome must leave both intent and attempt visible for
/// crash recovery to retry.
///
/// Scenario:
/// 1. Propose intent → OK.
/// 2. Claim intent → OK (intent transitions to "claimed").
/// 3. Record attempt start → OK (attempt is durable).
/// 4. Inject crash on next write (record_outcome → before_write → Crash).
/// 5. Verify: intent is still "claimed" (crash prevents it moving to resolved),
///    and the attempt record is present.
#[tokio::test]
async fn fi_02_attempt_survives_crash_before_outcome() {
    let backing = Arc::new(MemEffectStore::default());
    let injector = Arc::new(FaultInjector::new());

    let fault_store = Arc::new(FaultStore::new(
        Arc::clone(&backing) as Arc<dyn EffectStore>,
        Arc::clone(&injector),
    ));

    let run_id = RunId::new();
    let worker_id = WorkerId::new();

    // 1. Propose intent (no fault yet).
    let intent_id = {
        let pipeline = EffectPipeline::new(
            Arc::clone(&fault_store) as Arc<dyn EffectStore>,
            worker_id,
        );
        pipeline.propose(make_spec(run_id)).await.expect("propose")
    };

    // 2. Claim intent.
    {
        let pipeline = EffectPipeline::new(
            Arc::clone(&fault_store) as Arc<dyn EffectStore>,
            worker_id,
        );
        let guard = pipeline
            .claim_with_duration(Duration::from_secs(60))
            .await
            .expect("claim")
            .expect("guard");
        std::mem::forget(guard); // prevent Drop from releasing the claim
    }

    // 3. Record attempt start.
    let attempt = make_attempt(intent_id, run_id);
    {
        let pipeline = EffectPipeline::new(
            Arc::clone(&fault_store) as Arc<dyn EffectStore>,
            worker_id,
        );
        pipeline.record_attempt(intent_id, &attempt).await.expect("record_attempt");
    }

    // 4. Inject crash for the NEXT write (outcome recording).
    injector.add_fault("before_write", Fault::Crash, FaultSchedule::Once);

    let outcome = make_outcome(&attempt);
    let fault_store_clone = Arc::clone(&fault_store);
    let record_result = std::thread::spawn(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let rt = tokio::runtime::Runtime::new().expect("rt");
            rt.block_on(async {
                let pipeline = EffectPipeline::new(
                    fault_store_clone as Arc<dyn EffectStore>,
                    worker_id,
                );
                pipeline.record_outcome(intent_id, &outcome).await
            })
        }))
    })
    .join()
    .expect("thread join");
    assert!(record_result.is_err(), "crash fault at before_write should panic");

    // 5. Verify: intent is still claimed (not resolved), attempt is present.
    {
        let intents = backing.intents.lock().expect("lock");
        let intent = intents.get(&intent_id).expect("intent exists");
        assert_eq!(
            intent.state, "claimed",
            "intent must remain 'claimed' after crash before outcome"
        );
    }
    {
        let attempts = backing.attempts.lock().expect("lock");
        assert_eq!(attempts.len(), 1, "attempt record must survive the crash");
    }
    {
        let outcomes = backing.outcomes.lock().expect("lock");
        assert!(outcomes.is_empty(), "outcome must NOT be present after crash");
    }
}

// ---------------------------------------------------------------------------
// FI-03: Crash during outbox drain — no duplicate delivery on restart
// ---------------------------------------------------------------------------

/// FI-03: A crash midway through consuming outcomes from the outbox must not
/// produce duplicate delivery: outcomes marked as consumed before the crash
/// remain consumed; only unconsumed outcomes are replayed.
///
/// Scenario:
/// 1. Propose two intents and record outcomes for each.
/// 2. Mark first outcome as consumed — OK (now consumed = true in store).
/// 3. Inject crash on next mark_outcomes_consumed write.
/// 4. Simulate "restart": query unconsumed_outcomes → should return only the
///    second (not-yet-consumed) outcome.
#[tokio::test]
async fn fi_03_no_duplicate_delivery_on_restart_after_crash_during_drain() {
    let backing = Arc::new(MemEffectStore::default());
    let injector = Arc::new(FaultInjector::new());

    let fault_store = Arc::new(FaultStore::new(
        Arc::clone(&backing) as Arc<dyn EffectStore>,
        Arc::clone(&injector),
    ));

    let run_id = RunId::new();
    let worker_id = WorkerId::new();

    // Helper: propose intent + record outcome directly in backing store.
    let propose_and_outcome = |intent_id: EffectId| {
        let backing = Arc::clone(&backing);
        let run_id = run_id;
        let worker_id = worker_id;
        async move {
            let intent = StoredIntent {
                id: intent_id,
                run_id,
                step_id: StepId::new(),
                state: "resolved".into(),
                lease_owner: Some(worker_id),
                lease_expires: None,
                retry_class: StoreRetryClass::Idempotent,
                payload: serde_json::json!({"kind": "model_call"}),
                idempotency_key: format!("key-{}", intent_id),
                created_at: Utc::now(),
            };
            backing.propose_intent(intent).await.expect("propose");

            let attempt_id = EffectAttemptId::new();
            let outcome = StoredOutcome {
                id: EffectOutcomeId::new(),
                intent_id,
                attempt_id,
                run_id,
                consumed: false,
                payload: serde_json::json!({"ok": true}),
                observed_at: Utc::now(),
            };
            backing.record_outcome(outcome.clone()).await.expect("record outcome");
            outcome.id
        }
    };

    let outcome_id_1 = propose_and_outcome(EffectId::new()).await;
    let _outcome_id_2 = propose_and_outcome(EffectId::new()).await;

    // Both outcomes are unconsumed.
    let unconsumed = fault_store.unconsumed_outcomes(run_id).await.expect("unconsumed");
    assert_eq!(unconsumed.len(), 2, "both outcomes should be unconsumed initially");

    // Mark first outcome consumed — no fault yet.
    fault_store
        .mark_outcomes_consumed(&[outcome_id_1])
        .await
        .expect("mark first consumed");

    // Confirm only one remains.
    let unconsumed = fault_store.unconsumed_outcomes(run_id).await.expect("unconsumed");
    assert_eq!(unconsumed.len(), 1, "one outcome should remain unconsumed");
    let remaining_id = unconsumed[0].id;

    // Inject crash on the NEXT mark_outcomes_consumed write.
    injector.add_fault("before_write", Fault::Crash, FaultSchedule::Once);

    let fault_store_clone = Arc::clone(&fault_store);
    let crash_result = std::thread::spawn(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let rt = tokio::runtime::Runtime::new().expect("rt");
            rt.block_on(async {
                // Try to mark the second outcome consumed — crashes.
                let store = fault_store_clone as Arc<dyn EffectStore>;
                store.mark_outcomes_consumed(&[remaining_id]).await
            })
        }))
    })
    .join()
    .expect("thread join");
    assert!(crash_result.is_err(), "crash fault should panic");

    // Simulate restart: the first outcome is permanently consumed,
    // the second is still in the outbox (not consumed).
    let post_crash_unconsumed =
        backing.unconsumed_outcomes(run_id).await.expect("unconsumed after crash");
    assert_eq!(
        post_crash_unconsumed.len(),
        1,
        "exactly one outcome must remain unconsumed after crash"
    );
    assert_eq!(
        post_crash_unconsumed[0].id, remaining_id,
        "the un-committed outcome must be the one not yet consumed"
    );

    // Critically: the first outcome must not reappear (no duplicate delivery).
    let first_still_consumed = backing
        .outcomes
        .lock()
        .expect("lock")
        .iter()
        .find(|o| o.id == outcome_id_1)
        .map(|o| o.consumed)
        .unwrap_or(false);
    assert!(
        first_still_consumed,
        "first outcome must remain consumed — no duplicate delivery"
    );
}

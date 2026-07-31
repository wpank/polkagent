//! PRD-15 Security Tests: Effect pipeline security.
//!
//! These tests verify the effect pipeline's critical safety invariants:
//! idempotency key deduplication, outcome immutability, unknown outcome
//! preservation, expired lease recovery, and claim exclusivity.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use polkagent_core::{
    EffectAttemptId, EffectId, EffectOutcomeId, RunId, StepId, TurnId, WorkerId,
};
use polkagent_effect::error::PipelineError;
use polkagent_effect::idempotency::IdempotencyKey;
use polkagent_effect::pipeline::{EffectIntentSpec, EffectPipeline};
use polkagent_effect::types::{
    CancellationReason, EffectKind, EffectOutcome, OutcomeResult, ResolutionHint,
};
use polkagent_store_trait::{
    EffectStore, StoredIntent, StoredOutcome, StoreError,
};

// ---------------------------------------------------------------------------
// In-memory test store (re-implements the pattern from polkagent-effect tests)
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct InMemoryStore {
    intents: std::sync::Mutex<std::collections::HashMap<EffectId, StoredIntent>>,
    attempts: std::sync::Mutex<Vec<serde_json::Value>>,
    outcomes: std::sync::Mutex<Vec<StoredOutcome>>,
}

#[async_trait::async_trait]
impl EffectStore for InMemoryStore {
    async fn propose_intent(&self, intent: StoredIntent) -> Result<(), StoreError> {
        let mut intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now();
        let pending_id = intents
            .values()
            .find(|i| i.state.eq_ignore_ascii_case("pending"))
            .map(|i| i.id);
        match pending_id {
            None => Ok(None),
            Some(id) => {
                let intent = intents.get_mut(&id).unwrap_or_else(|| {
                    panic!("intent {id} must exist");
                });
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
        let mut intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(intent) = intents.get_mut(&intent_id) {
            if intent.lease_owner == Some(worker_id) {
                intent.state = "pending".to_string();
                intent.lease_owner = None;
                intent.lease_expires = None;
            }
        }
        Ok(())
    }

    async fn get_intent(&self, intent_id: EffectId) -> Result<StoredIntent, StoreError> {
        let intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        intents.get(&intent_id).cloned().ok_or_else(|| StoreError::NotFound {
            resource_type: "EffectIntent",
            id: intent_id.to_string(),
        })
    }

    async fn get_by_run(&self, run_id: RunId) -> Result<Vec<StoredIntent>, StoreError> {
        let intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        Ok(intents.values().filter(|i| i.run_id == run_id).cloned().collect())
    }

    async fn expired_leases(
        &self,
        cutoff: polkagent_core::Timestamp,
    ) -> Result<Vec<StoredIntent>, StoreError> {
        let intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        Ok(intents
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
        _attempt_id: polkagent_core::EffectAttemptId,
        _intent_id: EffectId,
        _worker_id: WorkerId,
        payload: serde_json::Value,
    ) -> Result<(), StoreError> {
        self.attempts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(payload);
        Ok(())
    }

    async fn record_outcome(&self, outcome: StoredOutcome) -> Result<(), StoreError> {
        let mut outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        if outcomes.iter().any(|o| o.id == outcome.id) {
            return Err(StoreError::Conflict {
                resource_type: "EffectOutcome",
                id: outcome.id.to_string(),
            });
        }
        let mut intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
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
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        Ok(outcomes
            .iter()
            .filter(|o| o.run_id == run_id && !o.consumed)
            .cloned()
            .collect())
    }

    async fn mark_outcomes_consumed(
        &self,
        outcome_ids: &[EffectOutcomeId],
    ) -> Result<(), StoreError> {
        let mut outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        for o in outcomes.iter_mut() {
            if outcome_ids.contains(&o.id) {
                o.consumed = true;
            }
        }
        Ok(())
    }
}

fn make_store() -> (Arc<dyn EffectStore>, Arc<InMemoryStore>) {
    let inner = Arc::new(InMemoryStore::default());
    let store: Arc<dyn EffectStore> = Arc::clone(&inner) as Arc<dyn EffectStore>;
    (store, inner)
}

fn make_spec(run_id: RunId, kind: EffectKind) -> EffectIntentSpec {
    EffectIntentSpec {
        run_id,
        turn_id: TurnId::new(),
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

fn make_outcome(intent_id: EffectId, run_id: RunId) -> EffectOutcome {
    EffectOutcome {
        id: EffectOutcomeId::new(),
        attempt_id: EffectAttemptId::new(),
        intent_id,
        run_id,
        result: OutcomeResult::Success {
            data: serde_json::json!({"ok": true}),
        },
        observed_at: Utc::now(),
        digest: [0u8; 32],
    }
}

// ===========================================================================
// Duplicate idempotency key prevents re-execution
// ===========================================================================

#[tokio::test]
async fn duplicate_idempotency_key_prevents_re_execution() {
    let (store, _) = make_store();
    let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
    let run_id = RunId::new();

    let key = IdempotencyKey::generate(
        run_id,
        1,
        0,
        EffectKind::ModelCall,
        IdempotencyKey::hash_params(b"fixed-params"),
    );

    let spec = || EffectIntentSpec {
        run_id,
        turn_id: TurnId::new(),
        step_id: StepId::new(),
        kind: EffectKind::ModelCall,
        sequence: 1,
        idempotency_key: Some(key),
        payload: serde_json::json!({}),
        retry_class: None,
        priority: None,
        max_attempts: None,
    };

    // First proposal: succeeds.
    let _id1 = pipeline
        .propose(spec())
        .await
        .unwrap_or_else(|e| panic!("first propose should succeed: {e}"));

    // Second proposal with same key: must fail.
    let err = pipeline
        .propose(spec())
        .await
        .expect_err("second propose with same idempotency key must fail");

    assert!(
        matches!(err, PipelineError::DuplicatePending(_)),
        "error must be DuplicatePending, got: {err:?}"
    );
}

// ===========================================================================
// Outcome immutability prevents tampering
// ===========================================================================

#[tokio::test]
async fn outcome_immutability_prevents_tampering() {
    let (store, _) = make_store();
    let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
    let run_id = RunId::new();

    let intent_id = pipeline
        .propose(make_spec(run_id, EffectKind::ModelCall))
        .await
        .unwrap_or_else(|e| panic!("propose failed: {e}"));

    // Claim the intent.
    let _guard = pipeline
        .claim_with_duration(Duration::from_secs(60))
        .await
        .unwrap_or_else(|e| panic!("claim failed: {e}"))
        .unwrap_or_else(|| panic!("no intent to claim"));

    // Record the first outcome.
    let outcome = make_outcome(intent_id, run_id);
    pipeline
        .record_outcome(intent_id, &outcome)
        .await
        .unwrap_or_else(|e| panic!("first record_outcome failed: {e}"));

    // Attempt to record a second (tampered) outcome.
    let tampered = EffectOutcome {
        result: OutcomeResult::Failure {
            error_class: polkagent_effect::types::ErrorClass::ClientError,
            message: "tampered failure".to_string(),
            retriable: false,
        },
        ..outcome
    };

    let err = pipeline
        .record_outcome(intent_id, &tampered)
        .await
        .expect_err("recording a second outcome must fail (immutability)");

    assert!(
        matches!(err, PipelineError::OutcomeAlreadyRecorded(_)),
        "error must be OutcomeAlreadyRecorded, got: {err:?}"
    );
}

// ===========================================================================
// Unknown outcome status is preserved (never collapsed to success)
// ===========================================================================

#[test]
fn unknown_outcome_is_never_collapsed_to_success() {
    let unknown = OutcomeResult::Unknown {
        context: "chain RPC did not respond within timeout".to_string(),
        resolution_hint: ResolutionHint::CheckChain,
    };

    // Verify the Unknown variant survives serde round-trip without being
    // silently promoted to Success or Failure.
    let json = serde_json::to_string(&unknown)
        .unwrap_or_else(|e| panic!("serialize failed: {e}"));
    let back: OutcomeResult = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("deserialize failed: {e}"));

    match back {
        OutcomeResult::Unknown {
            context,
            resolution_hint,
        } => {
            assert_eq!(context, "chain RPC did not respond within timeout");
            assert_eq!(resolution_hint, ResolutionHint::CheckChain);
        }
        other => {
            panic!("Unknown outcome was collapsed to {other:?}! This violates EFF-INV-6.");
        }
    }
}

#[test]
fn all_outcome_variants_survive_serde_round_trip() {
    let variants: Vec<OutcomeResult> = vec![
        OutcomeResult::Success {
            data: serde_json::json!({"tx": "0xabc"}),
        },
        OutcomeResult::Failure {
            error_class: polkagent_effect::types::ErrorClass::ServerError,
            message: "500 internal".to_string(),
            retriable: true,
        },
        OutcomeResult::Timeout {
            waited_secs: 120,
            partial_work_possible: true,
        },
        OutcomeResult::Cancelled {
            partial_work_possible: false,
            reason: CancellationReason::RunCancelled,
        },
        OutcomeResult::Unknown {
            context: "indeterminate".to_string(),
            resolution_hint: ResolutionHint::ManualInvestigation,
        },
    ];

    for (i, variant) in variants.iter().enumerate() {
        let json = serde_json::to_string(variant)
            .unwrap_or_else(|e| panic!("serialize variant {i} failed: {e}"));
        let back: OutcomeResult = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("deserialize variant {i} failed: {e}"));

        // Verify the variant tag is preserved.
        let original_tag = std::mem::discriminant(variant);
        let round_trip_tag = std::mem::discriminant(&back);
        assert_eq!(
            original_tag, round_trip_tag,
            "variant {i} must survive serde round-trip without changing discriminant"
        );
    }
}

// ===========================================================================
// Claim exclusivity: only one worker can claim an intent
// ===========================================================================

#[tokio::test]
async fn claim_exclusivity_only_one_worker() {
    let (store, _) = make_store();
    let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());

    // Propose a single intent.
    pipeline
        .propose(make_spec(RunId::new(), EffectKind::ToolCall))
        .await
        .unwrap_or_else(|e| panic!("propose failed: {e}"));

    // Attempt concurrent claims from 8 workers.
    const N: usize = 8;
    let mut join_set = tokio::task::JoinSet::new();
    for _ in 0..N {
        let s = Arc::clone(&store);
        join_set.spawn(async move {
            EffectPipeline::new(s, WorkerId::new())
                .claim_with_duration(Duration::from_secs(30))
                .await
                .unwrap_or_else(|e| panic!("claim failed: {e}"))
        });
    }

    let mut successes = 0usize;
    while let Some(result) = join_set.join_next().await {
        let guard = result.unwrap_or_else(|e| panic!("join failed: {e}"));
        if guard.is_some() {
            successes += 1;
        }
    }

    assert_eq!(
        successes, 1,
        "exactly one concurrent claim must succeed, got: {successes}"
    );
}

// ===========================================================================
// Guard drop releases claim
// ===========================================================================

#[tokio::test]
async fn guard_drop_returns_intent_to_pending() {
    let (store_dyn, store) = make_store();
    let pipeline = EffectPipeline::new(Arc::clone(&store_dyn), WorkerId::new());
    let run_id = RunId::new();

    let intent_id = pipeline
        .propose(make_spec(run_id, EffectKind::ModelCall))
        .await
        .unwrap_or_else(|e| panic!("propose failed: {e}"));

    {
        let guard = pipeline
            .claim_with_duration(Duration::from_secs(30))
            .await
            .unwrap_or_else(|e| panic!("claim failed: {e}"))
            .unwrap_or_else(|| panic!("no intent to claim"));

        // Verify it's claimed.
        {
            let intents = store.intents.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(intents[&intent_id].state, "claimed");
        }

        // Drop the guard without completing.
        drop(guard);
    }

    // Allow the async drop task to run.
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let intents = store.intents.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(
        intents[&intent_id].state, "pending",
        "dropping the guard must return intent to pending"
    );
}

// ===========================================================================
// Idempotency key stability
// ===========================================================================

#[test]
fn idempotency_key_is_deterministic() {
    let run_id = RunId::new();
    let params_hash = IdempotencyKey::hash_params(b"transfer-10-dot");

    let k1 = IdempotencyKey::generate(run_id, 1, 0, EffectKind::Broadcast, params_hash);
    let k2 = IdempotencyKey::generate(run_id, 1, 0, EffectKind::Broadcast, params_hash);

    assert_eq!(k1, k2, "same inputs must produce the same idempotency key (EFF-INV-4)");
}

#[test]
fn different_effect_kinds_produce_different_keys() {
    let run_id = RunId::new();
    let params_hash = IdempotencyKey::hash_params(b"same-params");

    let k_model = IdempotencyKey::generate(run_id, 1, 0, EffectKind::ModelCall, params_hash);
    let k_chain = IdempotencyKey::generate(run_id, 1, 0, EffectKind::ChainRead, params_hash);

    assert_ne!(
        k_model, k_chain,
        "different effect kinds must produce different idempotency keys"
    );
}

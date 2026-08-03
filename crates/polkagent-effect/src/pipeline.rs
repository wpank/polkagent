//! The `EffectPipeline` — the central safety component of Polkagent.
//!
//! This module owns the complete effect lifecycle:
//!
//! ```text
//! propose  →  claim  →  record_attempt  →  record_outcome
//!   ↑
//!   │ (intent persisted BEFORE any I/O)
//! ```
//!
//! ## Critical invariant
//!
//! **EFF-INV-1:** An intent is persisted to durable storage inside `propose`
//! before any external I/O occurs. If the process crashes after `propose` but
//! before I/O completes, the intent remains in the outbox with state `Pending`.
//! The lease-reaper picks it up and the [`crate::recovery::CrashRecovery`]
//! module decides the appropriate action.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use polkagent_core::{EffectId, RetryClass, RunId, WorkerId};
use serde_json::json;
use tracing::{debug, info};

use polkagent_card::ActionCard;

use crate::claim::ClaimGuard;
use crate::error::PipelineError;
use crate::idempotency::{self, IdempotencyKey};
use crate::types::{EffectAttempt, EffectKind, EffectOutcome, EffectPriority};
use polkagent_core::{StepId, TurnId};
use polkagent_store_trait::{EffectStore, StoredIntent, StoredOutcome, StoreRetryClass};

// ---------------------------------------------------------------------------
// EffectIntentSpec
// ---------------------------------------------------------------------------

/// Builder input for proposing a new effect intent.
#[derive(Debug, Clone)]
pub struct EffectIntentSpec {
    pub run_id: RunId,
    pub turn_id: TurnId,
    pub step_id: StepId,
    pub kind: EffectKind,
    pub sequence: u64,
    pub idempotency_key: Option<IdempotencyKey>,
    pub payload: serde_json::Value,
    pub retry_class: Option<RetryClass>,
    pub priority: Option<EffectPriority>,
    pub max_attempts: Option<u32>,
    /// Optional action card attached to this intent.
    ///
    /// When provided, the card is embedded in the `StoredIntent.payload` JSON
    /// under the `"action_card"` key so that it can be retrieved during
    /// approval flows without requiring a separate lookup.
    pub action_card: Option<ActionCard>,
}

impl EffectIntentSpec {
    fn default_max_attempts(retry_class: RetryClass) -> u32 {
        match retry_class {
            RetryClass::Idempotent => 3,
            RetryClass::CheckBeforeRetry => 2,
            RetryClass::NoAutoRetry => 1,
        }
    }
}

// ---------------------------------------------------------------------------
// EffectPipeline
// ---------------------------------------------------------------------------

/// The central effect pipeline. Cheap to clone.
#[derive(Clone)]
pub struct EffectPipeline {
    store: Arc<dyn EffectStore>,
    worker_id: WorkerId,
}

impl EffectPipeline {
    #[must_use]
    pub fn new(store: Arc<dyn EffectStore>, worker_id: WorkerId) -> Self {
        Self { store, worker_id }
    }

    #[must_use]
    pub fn store(&self) -> &Arc<dyn EffectStore> {
        &self.store
    }

    /// Persist a new effect intent BEFORE any I/O occurs (EFF-INV-1).
    pub async fn propose(&self, spec: EffectIntentSpec) -> Result<EffectId, PipelineError> {
        let kind = spec.kind;
        let retry_class = spec.retry_class.unwrap_or_else(|| kind.default_retry_class());
        let priority = spec.priority.unwrap_or_default();
        let max_attempts = spec
            .max_attempts
            .unwrap_or_else(|| EffectIntentSpec::default_max_attempts(retry_class));

        // 1. Derive the idempotency key if not provided.
        let idempotency_key = spec.idempotency_key.unwrap_or_else(|| {
            let params_hash = IdempotencyKey::hash_params(
                serde_json::to_vec(&spec.payload).unwrap_or_default().as_slice(),
            );
            IdempotencyKey::generate(spec.run_id, spec.sequence, 0, kind, params_hash)
        });

        // 2. Check for duplicates before touching the store.
        idempotency::check_duplicate(&self.store, &idempotency_key, spec.run_id).await?;

        // 3. Assign an ID.
        let intent_id = EffectId::new();

        // 4. Build the payload JSON.
        // The action_card is embedded under "action_card" so that approve_effect
        // in the service layer can extract it without a separate lookup.
        let payload = json!({
            "id": intent_id,
            "run_id": spec.run_id,
            "turn_id": spec.turn_id,
            "step_id": spec.step_id,
            "kind": kind,
            "idempotency_key": idempotency_key.to_hex(),
            "sequence": spec.sequence,
            "priority": priority,
            "retry_class": retry_class,
            "max_attempts": max_attempts,
            "params": spec.payload,
            "action_card": spec.action_card,
        });

        let store_retry_class = match retry_class {
            RetryClass::Idempotent => StoreRetryClass::Idempotent,
            RetryClass::CheckBeforeRetry => StoreRetryClass::CheckBeforeRetry,
            RetryClass::NoAutoRetry => StoreRetryClass::NoAutoRetry,
        };

        let stored = StoredIntent {
            id: intent_id,
            run_id: spec.run_id,
            step_id: spec.step_id,
            state: "pending".to_string(),
            lease_owner: None,
            lease_expires: None,
            retry_class: store_retry_class,
            payload,
            idempotency_key: idempotency_key.to_hex(),
            created_at: Utc::now(),
        };

        // 5. Persist — DURABLE write before any I/O.
        self.store
            .propose_intent(stored)
            .await
            .map_err(PipelineError::Store)?;

        info!(
            intent_id = %intent_id,
            run_id = %spec.run_id,
            kind = kind.discriminant_str(),
            retry_class = ?retry_class,
            "intent proposed and persisted"
        );

        Ok(intent_id)
    }

    /// Claim the next pending intent with a 60s default lease.
    pub async fn claim(&self) -> Result<Option<ClaimGuard>, PipelineError> {
        self.claim_with_duration(Duration::from_secs(60)).await
    }

    /// Claim the next pending intent with the specified lease duration.
    pub async fn claim_with_duration(
        &self,
        lease_duration: Duration,
    ) -> Result<Option<ClaimGuard>, PipelineError> {
        let result = self
            .store
            .claim_intent(self.worker_id, lease_duration)
            .await
            .map_err(PipelineError::Store)?;

        match result {
            None => {
                debug!(worker_id = %self.worker_id, "outbox empty");
                Ok(None)
            }
            Some(stored) => {
                let lease_expires = stored
                    .lease_expires
                    .unwrap_or_else(|| Utc::now() + chrono::Duration::seconds(60));

                info!(
                    intent_id = %stored.id,
                    worker_id = %self.worker_id,
                    lease_expires = %lease_expires,
                    "intent claimed"
                );

                Ok(Some(ClaimGuard::new(
                    stored.id,
                    self.worker_id,
                    lease_expires,
                    Arc::clone(&self.store),
                )))
            }
        }
    }

    /// Claim a specific intent by ID (recovery path).
    pub async fn claim_by_id(
        &self,
        intent_id: EffectId,
        lease_duration: Duration,
    ) -> Result<ClaimGuard, PipelineError> {
        let stored = self
            .store
            .claim_intent_by_id(intent_id, self.worker_id, lease_duration)
            .await
            .map_err(PipelineError::Store)?;

        let lease_expires = stored
            .lease_expires
            .unwrap_or_else(|| Utc::now() + chrono::Duration::seconds(60));

        info!(
            intent_id = %intent_id,
            worker_id = %self.worker_id,
            lease_expires = %lease_expires,
            "intent claimed by ID (recovery)"
        );

        Ok(ClaimGuard::new(
            intent_id,
            self.worker_id,
            lease_expires,
            Arc::clone(&self.store),
        ))
    }

    /// Record the start of an attempt (called after claim, before I/O).
    pub async fn record_attempt(
        &self,
        intent_id: EffectId,
        attempt: &EffectAttempt,
    ) -> Result<(), PipelineError> {
        let payload = serde_json::to_value(attempt)
            .map_err(|e| PipelineError::Internal(format!("failed to serialise attempt: {e}")))?;

        self.store
            .record_attempt_start(attempt.id, intent_id, self.worker_id, payload)
            .await
            .map_err(PipelineError::Store)?;

        debug!(attempt_id = %attempt.id, intent_id = %intent_id, "attempt recorded");
        Ok(())
    }

    /// Record an immutable outcome (EFF-INV-3).
    ///
    /// The BLAKE3 digest of the serialised result is computed here so that
    /// callers do not need to supply it — the pipeline owns integrity.
    pub async fn record_outcome(
        &self,
        intent_id: EffectId,
        outcome: &EffectOutcome,
    ) -> Result<(), PipelineError> {
        let result_bytes = serde_json::to_vec(&outcome.result)
            .map_err(|e| PipelineError::Internal(format!("failed to serialise outcome: {e}")))?;
        let hash = blake3::hash(&result_bytes);
        let digest_hex = hash.to_hex().to_string();

        let mut payload: serde_json::Value = serde_json::from_slice(&result_bytes)
            .map_err(|e| PipelineError::Internal(format!("failed to parse outcome: {e}")))?;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("digest_hex".to_string(), serde_json::Value::String(digest_hex.clone()));
        }

        let stored = StoredOutcome {
            id: outcome.id,
            intent_id,
            attempt_id: outcome.attempt_id,
            run_id: outcome.run_id,
            consumed: false,
            payload,
            observed_at: outcome.observed_at,
        };

        self.store
            .record_outcome(stored)
            .await
            .map_err(|e| match e {
                polkagent_store_trait::StoreError::Conflict { .. } => {
                    PipelineError::OutcomeAlreadyRecorded(intent_id)
                }
                other => PipelineError::Store(other),
            })?;

        info!(
            outcome_id = %outcome.id,
            intent_id = %intent_id,
            attempt_id = %outcome.attempt_id,
            digest = %digest_hex,
            "outcome recorded (immutable)"
        );
        Ok(())
    }

    /// Fetch a single intent by ID.
    pub async fn get_intent(&self, intent_id: EffectId) -> Result<StoredIntent, PipelineError> {
        self.store
            .get_intent(intent_id)
            .await
            .map_err(|e| match e {
                polkagent_store_trait::StoreError::NotFound { .. } => {
                    PipelineError::NotFound(intent_id)
                }
                other => PipelineError::Store(other),
            })
    }
}

// ---------------------------------------------------------------------------
// Test utilities (in-memory store + helper builders)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::idempotency::IdempotencyKey;
    use crate::types::{EffectKind, OutcomeResult};
    use polkagent_core::{EffectAttemptId, EffectId, EffectOutcomeId, RunId, Timestamp};
    use polkagent_store_trait::{EffectStore, StoredIntent, StoredOutcome, StoreError};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;

    /// A minimal in-memory EffectStore for pipeline tests.
    #[derive(Debug, Default)]
    pub struct InMemoryStore {
        pub intents: Mutex<HashMap<EffectId, StoredIntent>>,
        pub attempts: Mutex<Vec<serde_json::Value>>,
        pub outcomes: Mutex<Vec<StoredOutcome>>,
    }

    impl InMemoryStore {
        pub fn new() -> Self {
            Self::default()
        }
    }

    #[async_trait::async_trait]
    impl EffectStore for InMemoryStore {
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

        async fn release_claim(&self, intent_id: EffectId, worker_id: WorkerId) -> Result<(), StoreError> {
            let mut intents = self.intents.lock().expect("lock");
            if let Some(intent) = intents.get_mut(&intent_id) {
                if intent.lease_owner == Some(worker_id) && intent.state != "resolved" {
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
            let intents = self.intents.lock().expect("lock");
            intents.get(&intent_id).cloned().ok_or_else(|| StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            })
        }

        async fn get_by_run(&self, run_id: RunId) -> Result<Vec<StoredIntent>, StoreError> {
            let intents = self.intents.lock().expect("lock");
            Ok(intents.values().filter(|i| i.run_id == run_id).cloned().collect())
        }

        async fn expired_leases(&self, cutoff: Timestamp) -> Result<Vec<StoredIntent>, StoreError> {
            let intents = self.intents.lock().expect("lock");
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

        async fn unconsumed_outcomes(&self, run_id: RunId) -> Result<Vec<StoredOutcome>, StoreError> {
            let outcomes = self.outcomes.lock().expect("lock");
            Ok(outcomes.iter().filter(|o| o.run_id == run_id && !o.consumed).cloned().collect())
        }

        async fn mark_outcomes_consumed(&self, outcome_ids: &[EffectOutcomeId]) -> Result<(), StoreError> {
            let mut outcomes = self.outcomes.lock().expect("lock");
            for o in outcomes.iter_mut() {
                if outcome_ids.contains(&o.id) {
                    o.consumed = true;
                }
            }
            Ok(())
        }
    }

    pub fn make_in_memory_store() -> (Arc<dyn EffectStore>, Arc<InMemoryStore>) {
        let inner = Arc::new(InMemoryStore::new());
        let store: Arc<dyn EffectStore> = Arc::clone(&inner) as Arc<dyn EffectStore>;
        (store, inner)
    }

    pub fn make_spec(run_id: RunId, kind: EffectKind) -> EffectIntentSpec {
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
            action_card: None,
        }
    }

    pub fn make_outcome(
        intent_id: EffectId,
        attempt_id: EffectAttemptId,
        run_id: RunId,
    ) -> EffectOutcome {
        let result = OutcomeResult::Success { data: serde_json::json!({"ok": true}) };
        let digest = *blake3::hash(
            &serde_json::to_vec(&result).unwrap_or_default(),
        ).as_bytes();
        EffectOutcome {
            id: EffectOutcomeId::new(),
            attempt_id,
            intent_id,
            run_id,
            result,
            observed_at: Utc::now(),
            digest,
        }
    }

    // ----- Tests -----

    #[tokio::test]
    async fn propose_persists_intent_before_return() {
        let (store_dyn, store) = make_in_memory_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store_dyn), WorkerId::new());
        let run_id = RunId::new();
        let intent_id = pipeline.propose(make_spec(run_id, EffectKind::ModelCall)).await.expect("propose");
        let intents = store.intents.lock().expect("lock");
        assert!(intents.contains_key(&intent_id), "intent must be persisted before propose returns");
        assert_eq!(intents[&intent_id].state, "pending");
    }

    #[tokio::test]
    async fn propose_same_key_returns_duplicate_error() {
        let (store, _) = make_in_memory_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
        let run_id = RunId::new();
        let key = IdempotencyKey::generate(run_id, 1, 0, EffectKind::ModelCall, IdempotencyKey::hash_params(b"fixed"));
        let make = || EffectIntentSpec {
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
            action_card: None,
        };
        pipeline.propose(make()).await.expect("first");
        let err = pipeline.propose(make()).await.expect_err("second");
        assert!(matches!(err, crate::error::PipelineError::DuplicatePending(_)));
    }

    #[tokio::test]
    async fn claim_returns_guard_for_pending_intent() {
        let (store, _) = make_in_memory_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
        let run_id = RunId::new();
        pipeline.propose(make_spec(run_id, EffectKind::ChainRead)).await.expect("propose");
        let guard = pipeline.claim_with_duration(Duration::from_secs(30)).await.expect("claim");
        assert!(guard.is_some());
    }

    #[tokio::test]
    async fn claim_returns_none_for_empty_outbox() {
        let (store, _) = make_in_memory_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
        let guard = pipeline.claim_with_duration(Duration::from_secs(30)).await.expect("claim");
        assert!(guard.is_none());
    }

    #[tokio::test]
    async fn record_outcome_marks_intent_resolved() {
        let (store_dyn, store) = make_in_memory_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store_dyn), WorkerId::new());
        let run_id = RunId::new();
        let intent_id = pipeline.propose(make_spec(run_id, EffectKind::ModelCall)).await.expect("propose");
        let _guard = pipeline.claim_with_duration(Duration::from_secs(60)).await.expect("claim").expect("guard");
        let outcome = make_outcome(intent_id, EffectAttemptId::new(), run_id);
        pipeline.record_outcome(intent_id, &outcome).await.expect("record");
        let intents = store.intents.lock().expect("lock");
        assert_eq!(intents[&intent_id].state, "resolved");
    }

    #[tokio::test]
    async fn record_outcome_twice_returns_error() {
        let (store, _) = make_in_memory_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
        let run_id = RunId::new();
        let intent_id = pipeline.propose(make_spec(run_id, EffectKind::ModelCall)).await.expect("propose");
        let _guard = pipeline.claim_with_duration(Duration::from_secs(60)).await.expect("claim").expect("guard");
        let outcome = make_outcome(intent_id, EffectAttemptId::new(), run_id);
        pipeline.record_outcome(intent_id, &outcome).await.expect("first");
        let err = pipeline.record_outcome(intent_id, &outcome).await.expect_err("second");
        assert!(matches!(err, crate::error::PipelineError::OutcomeAlreadyRecorded(_)));
    }

    #[tokio::test]
    async fn concurrent_claims_only_one_succeeds() {
        use tokio::task::JoinSet;
        let (store, _) = make_in_memory_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
        pipeline.propose(make_spec(RunId::new(), EffectKind::ToolCall)).await.expect("propose");
        const N: usize = 8;
        let mut set = JoinSet::new();
        for _ in 0..N {
            let s = Arc::clone(&store);
            set.spawn(async move {
                EffectPipeline::new(s, WorkerId::new())
                    .claim_with_duration(Duration::from_secs(30))
                    .await
                    .expect("claim")
            });
        }
        let mut successes = 0usize;
        while let Some(r) = set.join_next().await {
            if r.expect("join").is_some() { successes += 1; }
        }
        assert_eq!(successes, 1, "exactly one concurrent claim should succeed");
    }

    #[tokio::test]
    async fn guard_drop_releases_claim() {
        let (store_dyn, store) = make_in_memory_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store_dyn), WorkerId::new());
        let run_id = RunId::new();
        let intent_id = pipeline.propose(make_spec(run_id, EffectKind::ModelCall)).await.expect("propose");
        {
            let guard = pipeline.claim_with_duration(Duration::from_secs(30)).await.expect("claim").expect("guard");
            { let intents = store.intents.lock().expect("lock"); assert_eq!(intents[&intent_id].state, "claimed"); }
            drop(guard);
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let intents = store.intents.lock().expect("lock");
        assert_eq!(intents[&intent_id].state, "pending");
    }

    #[tokio::test]
    async fn no_auto_retry_intent_has_max_attempts_1() {
        let (store_dyn, store) = make_in_memory_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store_dyn), WorkerId::new());
        let spec = EffectIntentSpec {
            run_id: RunId::new(),
            turn_id: TurnId::new(),
            step_id: StepId::new(),
            kind: EffectKind::Broadcast,
            sequence: 1,
            idempotency_key: None,
            payload: serde_json::json!({}),
            retry_class: None,
            priority: None,
            max_attempts: None,
            action_card: None,
        };
        let intent_id = pipeline.propose(spec).await.expect("propose");
        let intents = store.intents.lock().expect("lock");
        let max = intents[&intent_id].payload["max_attempts"].as_u64().expect("max_attempts");
        assert_eq!(max, 1);
    }
}

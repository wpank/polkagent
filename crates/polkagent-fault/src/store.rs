//! Fault-injecting [`EffectStore`] wrapper.
//!
//! [`FaultStore`] wraps any type that implements [`EffectStore`] (via dynamic
//! dispatch) and intercepts storage operations at named injection points,
//! simulating disk-full conditions, corrupt reads, and slow I/O.
//!
//! # Injection points
//!
//! - `"before_write"` — checked before `propose_intent`, `record_outcome`, etc.
//! - `"after_write"` — checked after the inner store confirms the write.
//! - `"before_read"` — checked before `get_intent`, `unconsumed_outcomes`, etc.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use polkagent_core::{EffectAttemptId, EffectId, EffectOutcomeId, RunId, Timestamp, WorkerId};
use polkagent_store_trait::{EffectStore, StoreError, StoredIntent, StoredOutcome};

use crate::injector::FaultInjector;
use crate::types::Fault;

// ---------------------------------------------------------------------------
// FaultStore
// ---------------------------------------------------------------------------

/// A fault-injecting wrapper around any [`EffectStore`].
///
/// Uses dynamic dispatch (`Arc<dyn EffectStore>`) so it can wrap any backing
/// store without generic parameters.
pub struct FaultStore {
    inner: Arc<dyn EffectStore>,
    injector: Arc<FaultInjector>,
}

impl std::fmt::Debug for FaultStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FaultStore")
            .field("injector", &self.injector)
            .finish_non_exhaustive()
    }
}

impl FaultStore {
    /// Wrap `inner` (a dynamic-dispatch effect store) with a fault injector.
    pub fn new(inner: Arc<dyn EffectStore>, injector: Arc<FaultInjector>) -> Self {
        Self { inner, injector }
    }
}

#[async_trait]
impl EffectStore for FaultStore {
    // -----------------------------------------------------------------------
    // Intent lifecycle
    // -----------------------------------------------------------------------

    async fn propose_intent(&self, intent: StoredIntent) -> Result<(), StoreError> {
        apply_store_fault_write(self.injector.check("before_write")).await?;
        self.inner.propose_intent(intent).await?;
        apply_store_fault_write(self.injector.check("after_write")).await?;
        Ok(())
    }

    async fn claim_intent(
        &self,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<Option<StoredIntent>, StoreError> {
        self.inner.claim_intent(worker_id, lease_duration).await
    }

    async fn claim_intent_by_id(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<StoredIntent, StoreError> {
        self.inner
            .claim_intent_by_id(intent_id, worker_id, lease_duration)
            .await
    }

    async fn release_claim(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
    ) -> Result<(), StoreError> {
        self.inner.release_claim(intent_id, worker_id).await
    }

    async fn update_intent_state(
        &self,
        intent_id: EffectId,
        new_state: &str,
    ) -> Result<StoredIntent, StoreError> {
        apply_store_fault_write(self.injector.check("before_write")).await?;
        let result = self.inner.update_intent_state(intent_id, new_state).await?;
        apply_store_fault_write(self.injector.check("after_write")).await?;
        Ok(result)
    }

    async fn get_intent(&self, intent_id: EffectId) -> Result<StoredIntent, StoreError> {
        apply_store_fault_read(self.injector.check("before_read")).await?;
        let mut intent = self.inner.get_intent(intent_id).await?;
        // Corrupt read: apply corruption to the JSON payload.
        if let Some(Fault::CorruptData { corruption }) = self.injector.check("before_read") {
            let mut bytes = serde_json::to_vec(&intent.payload).unwrap_or_default();
            corruption.apply(&mut bytes);
            intent.payload = serde_json::from_slice(&bytes)
                .unwrap_or(serde_json::Value::String("[CORRUPTED]".into()));
        }
        Ok(intent)
    }

    async fn get_by_run(&self, run_id: RunId) -> Result<Vec<StoredIntent>, StoreError> {
        apply_store_fault_read(self.injector.check("before_read")).await?;
        self.inner.get_by_run(run_id).await
    }

    async fn expired_leases(&self, cutoff: Timestamp) -> Result<Vec<StoredIntent>, StoreError> {
        self.inner.expired_leases(cutoff).await
    }

    // -----------------------------------------------------------------------
    // Attempt lifecycle
    // -----------------------------------------------------------------------

    async fn record_attempt_start(
        &self,
        attempt_id: EffectAttemptId,
        intent_id: EffectId,
        worker_id: WorkerId,
        payload: serde_json::Value,
    ) -> Result<(), StoreError> {
        apply_store_fault_write(self.injector.check("before_write")).await?;
        self.inner
            .record_attempt_start(attempt_id, intent_id, worker_id, payload)
            .await?;
        apply_store_fault_write(self.injector.check("after_write")).await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Outcome lifecycle
    // -----------------------------------------------------------------------

    async fn record_outcome(&self, outcome: StoredOutcome) -> Result<(), StoreError> {
        apply_store_fault_write(self.injector.check("before_write")).await?;
        self.inner.record_outcome(outcome).await?;
        apply_store_fault_write(self.injector.check("after_write")).await?;
        Ok(())
    }

    async fn unconsumed_outcomes(&self, run_id: RunId) -> Result<Vec<StoredOutcome>, StoreError> {
        apply_store_fault_read(self.injector.check("before_read")).await?;
        self.inner.unconsumed_outcomes(run_id).await
    }

    async fn mark_outcomes_consumed(
        &self,
        outcome_ids: &[EffectOutcomeId],
    ) -> Result<(), StoreError> {
        apply_store_fault_write(self.injector.check("before_write")).await?;
        self.inner.mark_outcomes_consumed(outcome_ids).await?;
        apply_store_fault_write(self.injector.check("after_write")).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Evaluate a fault for a write operation.
///
/// - `Crash`: panics.
/// - `Error` / `Timeout`: returns a [`StoreError::Internal`] to simulate
///   disk full or I/O error.
/// - `SlowDown`: sleeps then continues.
/// - Others: no-op (write proceeds normally).
async fn apply_store_fault_write(fault: Option<Fault>) -> Result<(), StoreError> {
    let Some(fault) = fault else { return Ok(()) };
    match fault {
        Fault::Crash => panic!("FaultStore: crash at write point"),
        Fault::Error { message } => Err(StoreError::Internal { message }),
        Fault::Timeout { ms } => {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Err(StoreError::Internal {
                message: "write timed out".into(),
            })
        }
        Fault::SlowDown { ms } => {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(())
        }
        Fault::CorruptData { .. } | Fault::PartialWrite => Ok(()),
    }
}

/// Evaluate a fault for a read operation.
async fn apply_store_fault_read(fault: Option<Fault>) -> Result<(), StoreError> {
    let Some(fault) = fault else { return Ok(()) };
    match fault {
        Fault::Crash => panic!("FaultStore: crash at read point"),
        Fault::Error { message } => Err(StoreError::Internal { message }),
        Fault::Timeout { ms } => {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Err(StoreError::Internal {
                message: "read timed out".into(),
            })
        }
        Fault::SlowDown { ms } => {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(())
        }
        // CorruptData is handled specially in get_intent; here we just proceed.
        Fault::CorruptData { .. } | Fault::PartialWrite => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// The in-memory test double and assertions intentionally fail fast on poisoned fixtures.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::types::FaultSchedule;
    use polkagent_core::{EffectId, RunId, StepId};
    use polkagent_store_trait::{StoreRetryClass, StoredIntent};
    use std::collections::HashMap;
    use std::sync::Mutex;

    // -----------------------------------------------------------------------
    // Minimal in-memory EffectStore for tests
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct MemStore {
        intents: Mutex<HashMap<EffectId, StoredIntent>>,
        outcomes: Mutex<Vec<StoredOutcome>>,
    }

    #[async_trait]
    impl EffectStore for MemStore {
        async fn propose_intent(&self, intent: StoredIntent) -> Result<(), StoreError> {
            self.intents.lock().expect("lock").insert(intent.id, intent);
            Ok(())
        }

        async fn claim_intent(
            &self,
            _worker_id: WorkerId,
            _lease_duration: Duration,
        ) -> Result<Option<StoredIntent>, StoreError> {
            Ok(None)
        }

        async fn claim_intent_by_id(
            &self,
            intent_id: EffectId,
            _worker_id: WorkerId,
            _lease_duration: Duration,
        ) -> Result<StoredIntent, StoreError> {
            self.intents
                .lock()
                .expect("lock")
                .get(&intent_id)
                .cloned()
                .ok_or_else(|| StoreError::NotFound {
                    resource_type: "Intent",
                    id: intent_id.to_string(),
                })
        }

        async fn release_claim(
            &self,
            _intent_id: EffectId,
            _worker_id: WorkerId,
        ) -> Result<(), StoreError> {
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
                    resource_type: "Intent",
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
                    resource_type: "Intent",
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
            _cutoff: Timestamp,
        ) -> Result<Vec<StoredIntent>, StoreError> {
            Ok(vec![])
        }

        async fn record_attempt_start(
            &self,
            _attempt_id: EffectAttemptId,
            _intent_id: EffectId,
            _worker_id: WorkerId,
            _payload: serde_json::Value,
        ) -> Result<(), StoreError> {
            Ok(())
        }

        async fn record_outcome(&self, outcome: StoredOutcome) -> Result<(), StoreError> {
            self.outcomes.lock().expect("lock").push(outcome);
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

    fn make_intent() -> StoredIntent {
        StoredIntent {
            id: EffectId::new(),
            run_id: RunId::new(),
            step_id: StepId::new(),
            state: "pending".into(),
            lease_owner: None,
            lease_expires: None,
            retry_class: StoreRetryClass::Idempotent,
            payload: serde_json::json!({"kind": "test"}),
            idempotency_key: "key-1".into(),
            created_at: chrono::Utc::now(),
        }
    }

    fn make_outcome(intent_id: EffectId, run_id: RunId) -> StoredOutcome {
        StoredOutcome {
            id: EffectOutcomeId::new(),
            intent_id,
            attempt_id: polkagent_core::EffectAttemptId::new(),
            run_id,
            consumed: false,
            payload: serde_json::json!({"ok": true}),
            observed_at: chrono::Utc::now(),
        }
    }

    fn make_fault_store(injector: Arc<FaultInjector>) -> FaultStore {
        let inner = Arc::new(MemStore::default());
        FaultStore::new(inner, injector)
    }

    #[tokio::test]
    async fn no_fault_write_succeeds() {
        let injector = Arc::new(FaultInjector::new());
        let store = make_fault_store(Arc::clone(&injector));
        let intent = make_intent();
        assert!(store.propose_intent(intent).await.is_ok());
    }

    #[tokio::test]
    async fn disk_full_prevents_write() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "before_write",
            Fault::Error {
                message: "disk full".into(),
            },
            FaultSchedule::Always,
        );
        let store = make_fault_store(Arc::clone(&injector));
        let result = store.propose_intent(make_intent()).await;
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("disk full"));
    }

    #[tokio::test]
    async fn no_fault_read_succeeds() {
        let injector = Arc::new(FaultInjector::new());
        let inner = Arc::new(MemStore::default());
        let intent = make_intent();
        let intent_id = intent.id;
        inner.propose_intent(intent).await.expect("insert");
        let store = FaultStore::new(Arc::clone(&inner) as Arc<dyn EffectStore>, injector);
        let fetched = store.get_intent(intent_id).await.expect("read ok");
        assert_eq!(fetched.id, intent_id);
    }

    #[tokio::test]
    async fn read_fault_returns_error() {
        let injector = Arc::new(FaultInjector::new());
        let inner = Arc::new(MemStore::default());
        let intent = make_intent();
        let intent_id = intent.id;
        inner.propose_intent(intent).await.expect("insert");
        injector.add_fault(
            "before_read",
            Fault::Error {
                message: "I/O error".into(),
            },
            FaultSchedule::Always,
        );
        let store = FaultStore::new(Arc::clone(&inner) as Arc<dyn EffectStore>, injector);
        let result = store.get_intent(intent_id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn outcome_write_fails_on_disk_full() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "before_write",
            Fault::Error {
                message: "no space left".into(),
            },
            FaultSchedule::Always,
        );
        let store = make_fault_store(Arc::clone(&injector));
        let intent_id = EffectId::new();
        let run_id = RunId::new();
        let outcome = make_outcome(intent_id, run_id);
        let result = store.record_outcome(outcome).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn crash_fault_on_write_panics() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault("before_write", Fault::Crash, FaultSchedule::Always);
        let store = Arc::new(make_fault_store(Arc::clone(&injector)));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let rt = tokio::runtime::Runtime::new().expect("rt");
            rt.block_on(store.propose_intent(make_intent()))
        }));
        assert!(result.is_err(), "crash fault should panic");
    }

    #[tokio::test]
    async fn after_write_fault_returns_error() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "after_write",
            Fault::Error {
                message: "post-write failure".into(),
            },
            FaultSchedule::Always,
        );
        let store = make_fault_store(Arc::clone(&injector));
        // The inner write succeeded but after_write fault fires.
        let result = store.propose_intent(make_intent()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn slow_down_does_not_prevent_write() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "before_write",
            Fault::SlowDown { ms: 1 }, // 1ms to keep test fast
            FaultSchedule::Always,
        );
        let store = make_fault_store(Arc::clone(&injector));
        // Write should still succeed despite slow-down.
        assert!(store.propose_intent(make_intent()).await.is_ok());
    }
}

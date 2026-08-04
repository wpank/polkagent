//! Crash recovery for the effect pipeline.
//!
//! After a process restart, the [`CrashRecovery`] scanner examines the store
//! for intents that were in-flight at crash time and recommends an action.
//!
//! ## Recovery procedure (PRD-03 §8.3)
//!
//! For each claimed intent with an expired lease:
//!
//! ```text
//! retry_class == Idempotent          → Retry   (safe to re-execute)
//! retry_class == CheckBeforeRetry    → Retry   (worker will check before acting)
//! retry_class == NoAutoRetry         → MarkFailed / RequiresManualResolution
//! ```
//!
//! For pending intents with no active worker:
//! - They remain in the outbox. Normal claim processing handles them.
//! - `recover_all` does not act on these; it reports them as `Retry`.
//!
//! ## Invariants
//!
//! - **CRASH-INV-2:** A crash between commit and I/O leaves a `Pending`
//!   intent. A worker will claim it.
//! - **CRASH-INV-3:** A crash during I/O (between claim and outcome) leaves a
//!   `Claimed` intent with an expiring lease. Recovery handles it per retry
//!   class.
//! - **CRASH-INV-5:** Signing and broadcast effects are NEVER automatically
//!   retried. They produce `Unknown` outcomes that require explicit
//!   investigation.

use std::sync::Arc;

use chrono::Utc;
use polkagent_core::EffectId;
use polkagent_store_trait::{EffectStore, StoreRetryClass, StoredIntent};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::PipelineError;

// ---------------------------------------------------------------------------
// RecoveryAction
// ---------------------------------------------------------------------------

/// The action recommended by [`CrashRecovery`] for a single recovered intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RecoveryAction {
    /// Re-queue the intent for processing. Safe because the effect is
    /// idempotent or the worker will check for prior completion before acting.
    Retry {
        intent_id: EffectId,
        /// Which retry-class applies.
        retry_class: RecoveryRetryClass,
        /// A human-readable explanation for logging.
        reason: String,
    },

    /// Mark the intent as failed with an `Unknown` outcome. Used for
    /// `NoAutoRetry` effects where the actual outcome is indeterminate.
    MarkFailed { intent_id: EffectId, reason: String },

    /// The intent cannot be automatically resolved. An operator must
    /// investigate (e.g., check whether a chain transaction was submitted).
    RequiresManualResolution {
        intent_id: EffectId,
        context: String,
        /// Suggested next step for the operator.
        resolution_hint: String,
    },
}

impl RecoveryAction {
    /// Return the intent ID this action pertains to.
    #[must_use]
    pub fn intent_id(&self) -> EffectId {
        match self {
            Self::Retry { intent_id, .. }
            | Self::MarkFailed { intent_id, .. }
            | Self::RequiresManualResolution { intent_id, .. } => *intent_id,
        }
    }
}

/// Retry class as seen by the recovery module (mirrors
/// [`polkagent_store_trait::StoreRetryClass`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryRetryClass {
    Idempotent,
    CheckBeforeRetry,
}

// ---------------------------------------------------------------------------
// CrashRecovery
// ---------------------------------------------------------------------------

/// On-startup crash-recovery scanner.
///
/// Create an instance with [`CrashRecovery::new`] and call
/// [`recover_all`](CrashRecovery::recover_all) once per startup. The returned
/// [`RecoveryAction`] list is advisory — the caller (typically the scheduler
/// or executor) applies the actions by either re-claiming intents or recording
/// `Unknown` outcomes.
pub struct CrashRecovery {
    store: Arc<dyn EffectStore>,
}

impl CrashRecovery {
    /// Create a new recovery scanner bound to the given store.
    #[must_use]
    pub fn new(store: Arc<dyn EffectStore>) -> Self {
        Self { store }
    }

    /// Scan for all claimed intents with expired leases and return recommended
    /// actions.
    ///
    /// A lease is considered expired if `lease_expires < now()`. Pending
    /// intents with no active worker are included as `Retry` (they will be
    /// picked up by normal outbox processing).
    ///
    /// # Errors
    ///
    /// Returns [`PipelineError::Store`] if the store scan fails.
    pub async fn recover_all(&self) -> Result<Vec<RecoveryAction>, PipelineError> {
        let now = Utc::now();
        let expired = self
            .store
            .expired_leases(now)
            .await
            .map_err(PipelineError::Store)?;

        if expired.is_empty() {
            info!("crash recovery: no expired leases found");
            return Ok(vec![]);
        }

        info!(
            count = expired.len(),
            "crash recovery: found expired leases"
        );

        let actions: Vec<RecoveryAction> = expired
            .into_iter()
            .map(|intent| classify_expired(&intent))
            .collect();

        for action in &actions {
            match action {
                RecoveryAction::Retry {
                    intent_id,
                    retry_class,
                    reason,
                } => {
                    info!(
                        intent_id = %intent_id,
                        retry_class = ?retry_class,
                        reason = reason,
                        "recovery action: Retry"
                    );
                }
                RecoveryAction::MarkFailed { intent_id, reason } => {
                    warn!(
                        intent_id = %intent_id,
                        reason = reason,
                        "recovery action: MarkFailed"
                    );
                }
                RecoveryAction::RequiresManualResolution {
                    intent_id,
                    context,
                    resolution_hint,
                } => {
                    warn!(
                        intent_id = %intent_id,
                        context = context,
                        hint = resolution_hint,
                        "recovery action: RequiresManualResolution"
                    );
                }
            }
        }

        // Apply the recovery actions to the store so that intents are
        // actually transitioned, not just reported on.
        self.apply_recovery_actions(&actions).await?;

        Ok(actions)
    }

    /// Apply a list of recovery actions to the store.
    ///
    /// - **Retry**: resets the intent state to `"pending"` so a worker can
    ///   re-claim it.
    /// - **MarkFailed**: transitions the intent to `"permanently_failed"`.
    /// - **RequiresManualResolution**: logs a warning; no automatic state
    ///   change is made. An operator must investigate.
    ///
    /// # Errors
    ///
    /// Returns [`PipelineError::Store`] if any store call fails. Actions are
    /// applied sequentially; a failure on one action aborts the remaining
    /// actions.
    pub async fn apply_recovery_actions(
        &self,
        actions: &[RecoveryAction],
    ) -> Result<(), PipelineError> {
        for action in actions {
            match action {
                RecoveryAction::Retry {
                    intent_id,
                    retry_class,
                    reason,
                } => {
                    info!(
                        intent_id = %intent_id,
                        retry_class = ?retry_class,
                        reason = reason,
                        "applying recovery: resetting intent to pending"
                    );
                    self.store
                        .update_intent_state(*intent_id, "pending")
                        .await
                        .map_err(PipelineError::Store)?;
                }
                RecoveryAction::MarkFailed { intent_id, reason } => {
                    warn!(
                        intent_id = %intent_id,
                        reason = reason,
                        "applying recovery: marking intent as permanently_failed"
                    );
                    self.store
                        .update_intent_state(*intent_id, "permanently_failed")
                        .await
                        .map_err(PipelineError::Store)?;
                }
                RecoveryAction::RequiresManualResolution {
                    intent_id,
                    context,
                    resolution_hint,
                } => {
                    warn!(
                        intent_id = %intent_id,
                        context = context,
                        hint = resolution_hint,
                        "recovery: intent requires manual resolution — no automatic \
                         state change applied"
                    );
                }
            }
        }
        Ok(())
    }

    /// Check whether a specific intent is idempotent (safe to retry).
    ///
    /// Convenience method for callers that want to check individual intents.
    pub fn is_idempotent(intent: &StoredIntent) -> bool {
        matches!(
            intent.retry_class,
            StoreRetryClass::Idempotent | StoreRetryClass::CheckBeforeRetry
        )
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Classify a single expired-lease intent into a [`RecoveryAction`].
fn classify_expired(intent: &StoredIntent) -> RecoveryAction {
    let intent_id = intent.id;

    match intent.retry_class {
        StoreRetryClass::Idempotent => RecoveryAction::Retry {
            intent_id,
            retry_class: RecoveryRetryClass::Idempotent,
            reason: "lease expired; idempotent intent is safe to retry automatically".to_string(),
        },

        StoreRetryClass::CheckBeforeRetry => RecoveryAction::Retry {
            intent_id,
            retry_class: RecoveryRetryClass::CheckBeforeRetry,
            reason: "lease expired; worker will check for prior completion before retrying"
                .to_string(),
        },

        StoreRetryClass::NoAutoRetry => {
            // Extract kind context from payload if available.
            let kind_context = intent
                .payload
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            // For signing and broadcasting effects, the operator must check
            // the chain or signer service.
            if kind_context == "signature_request" || kind_context == "broadcast" {
                RecoveryAction::RequiresManualResolution {
                    intent_id,
                    context: format!(
                        "lease expired for {kind_context} intent. Outcome is unknown. \
                         Check the external service/chain for partial completion."
                    ),
                    resolution_hint: format!(
                        "For '{kind_context}': check the chain or signer for evidence of \
                         prior execution. If confirmed, create a new intent with the \
                         known result. If not confirmed, create a new intent to retry."
                    ),
                }
            } else {
                RecoveryAction::MarkFailed {
                    intent_id,
                    reason: format!(
                        "lease expired for NoAutoRetry intent of kind '{kind_context}'. \
                         Marking as failed with Unknown outcome."
                    ),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use polkagent_core::{EffectId, RunId, WorkerId};
    use polkagent_store_trait::{StoreRetryClass, StoredIntent};
    use std::time::Duration as StdDuration;

    // -----------------------------------------------------------------------
    // Helper: build a StoredIntent with a known retry class and state.
    // -----------------------------------------------------------------------

    fn make_stored_intent(id: EffectId, retry_class: StoreRetryClass, state: &str) -> StoredIntent {
        StoredIntent {
            id,
            run_id: RunId::new(),
            step_id: polkagent_core::StepId::new(),
            state: state.to_string(),
            lease_owner: Some(WorkerId::new()),
            lease_expires: Some(
                Utc::now() - Duration::seconds(10), // already expired
            ),
            retry_class,
            payload: serde_json::json!({ "kind": "model_call" }),
            idempotency_key: format!("test-{}", id),
            created_at: Utc::now(),
        }
    }

    fn make_stored_intent_with_kind(
        id: EffectId,
        retry_class: StoreRetryClass,
        kind: &str,
    ) -> StoredIntent {
        StoredIntent {
            id,
            run_id: RunId::new(),
            step_id: polkagent_core::StepId::new(),
            state: "claimed".to_string(),
            lease_owner: Some(WorkerId::new()),
            lease_expires: Some(Utc::now() - Duration::seconds(5)),
            retry_class,
            payload: serde_json::json!({ "kind": kind }),
            idempotency_key: format!("test-{}", id),
            created_at: Utc::now(),
        }
    }

    // -----------------------------------------------------------------------
    // classify_expired unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn idempotent_expired_lease_becomes_retry() {
        let intent = make_stored_intent(EffectId::new(), StoreRetryClass::Idempotent, "claimed");
        let action = classify_expired(&intent);
        assert!(
            matches!(
                action,
                RecoveryAction::Retry {
                    retry_class: RecoveryRetryClass::Idempotent,
                    ..
                }
            ),
            "idempotent expired lease should become Retry"
        );
    }

    #[test]
    fn check_before_retry_expired_lease_becomes_retry() {
        let intent = make_stored_intent(
            EffectId::new(),
            StoreRetryClass::CheckBeforeRetry,
            "claimed",
        );
        let action = classify_expired(&intent);
        assert!(
            matches!(
                action,
                RecoveryAction::Retry {
                    retry_class: RecoveryRetryClass::CheckBeforeRetry,
                    ..
                }
            ),
            "check-before-retry expired lease should become Retry"
        );
    }

    #[test]
    fn no_auto_retry_model_call_becomes_mark_failed() {
        let intent = make_stored_intent_with_kind(
            EffectId::new(),
            StoreRetryClass::NoAutoRetry,
            "model_call",
        );
        let action = classify_expired(&intent);
        assert!(
            matches!(action, RecoveryAction::MarkFailed { .. }),
            "NoAutoRetry model_call should become MarkFailed, got {action:?}"
        );
    }

    #[test]
    fn no_auto_retry_signature_request_becomes_manual_resolution() {
        let intent = make_stored_intent_with_kind(
            EffectId::new(),
            StoreRetryClass::NoAutoRetry,
            "signature_request",
        );
        let action = classify_expired(&intent);
        assert!(
            matches!(action, RecoveryAction::RequiresManualResolution { .. }),
            "NoAutoRetry signature_request should become RequiresManualResolution"
        );
    }

    #[test]
    fn no_auto_retry_broadcast_becomes_manual_resolution() {
        let intent = make_stored_intent_with_kind(
            EffectId::new(),
            StoreRetryClass::NoAutoRetry,
            "broadcast",
        );
        let action = classify_expired(&intent);
        assert!(
            matches!(action, RecoveryAction::RequiresManualResolution { .. }),
            "NoAutoRetry broadcast should become RequiresManualResolution"
        );
    }

    #[test]
    fn recovery_action_intent_id_accessor() {
        let id = EffectId::new();
        let action = RecoveryAction::Retry {
            intent_id: id,
            retry_class: RecoveryRetryClass::Idempotent,
            reason: "test".to_string(),
        };
        assert_eq!(action.intent_id(), id);
    }

    // -----------------------------------------------------------------------
    // CrashRecovery integration tests
    // -----------------------------------------------------------------------

    /// A custom store that returns a fixed set of "expired" intents.
    struct ExpiredLeaseStore {
        intents: Vec<StoredIntent>,
    }

    #[async_trait::async_trait]
    impl EffectStore for ExpiredLeaseStore {
        async fn propose_intent(
            &self,
            _: StoredIntent,
        ) -> Result<(), polkagent_store_trait::StoreError> {
            Ok(())
        }
        async fn claim_intent(
            &self,
            _: WorkerId,
            _: StdDuration,
        ) -> Result<Option<StoredIntent>, polkagent_store_trait::StoreError> {
            Ok(None)
        }
        async fn claim_intent_by_id(
            &self,
            id: EffectId,
            _: WorkerId,
            _: StdDuration,
        ) -> Result<StoredIntent, polkagent_store_trait::StoreError> {
            self.intents
                .iter()
                .find(|i| i.id == id)
                .cloned()
                .ok_or_else(|| polkagent_store_trait::StoreError::NotFound {
                    resource_type: "StoredIntent",
                    id: id.to_string(),
                })
        }
        async fn release_claim(
            &self,
            _: EffectId,
            _: WorkerId,
        ) -> Result<(), polkagent_store_trait::StoreError> {
            Ok(())
        }
        async fn update_intent_state(
            &self,
            id: EffectId,
            _new_state: &str,
        ) -> Result<StoredIntent, polkagent_store_trait::StoreError> {
            // Read-only test store — just return the existing intent unchanged.
            self.intents
                .iter()
                .find(|i| i.id == id)
                .cloned()
                .ok_or_else(|| polkagent_store_trait::StoreError::NotFound {
                    resource_type: "StoredIntent",
                    id: id.to_string(),
                })
        }
        async fn get_intent(
            &self,
            id: EffectId,
        ) -> Result<StoredIntent, polkagent_store_trait::StoreError> {
            self.intents
                .iter()
                .find(|i| i.id == id)
                .cloned()
                .ok_or_else(|| polkagent_store_trait::StoreError::NotFound {
                    resource_type: "StoredIntent",
                    id: id.to_string(),
                })
        }
        async fn get_by_run(
            &self,
            _: RunId,
        ) -> Result<Vec<StoredIntent>, polkagent_store_trait::StoreError> {
            Ok(vec![])
        }
        async fn expired_leases(
            &self,
            _cutoff: chrono::DateTime<Utc>,
        ) -> Result<Vec<StoredIntent>, polkagent_store_trait::StoreError> {
            Ok(self.intents.clone())
        }
        async fn record_attempt_start(
            &self,
            _: polkagent_core::EffectAttemptId,
            _: EffectId,
            _: WorkerId,
            _: serde_json::Value,
        ) -> Result<(), polkagent_store_trait::StoreError> {
            Ok(())
        }
        async fn record_outcome(
            &self,
            _: polkagent_store_trait::StoredOutcome,
        ) -> Result<(), polkagent_store_trait::StoreError> {
            Ok(())
        }
        async fn unconsumed_outcomes(
            &self,
            _: RunId,
        ) -> Result<Vec<polkagent_store_trait::StoredOutcome>, polkagent_store_trait::StoreError>
        {
            Ok(vec![])
        }
        async fn mark_outcomes_consumed(
            &self,
            _: &[polkagent_core::EffectOutcomeId],
        ) -> Result<(), polkagent_store_trait::StoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn recover_all_empty_store_returns_no_actions() {
        let store = Arc::new(ExpiredLeaseStore { intents: vec![] });
        let recovery = CrashRecovery::new(store);
        let actions = recovery.recover_all().await.expect("recover");
        assert!(actions.is_empty());
    }

    #[tokio::test]
    async fn recover_all_idempotent_expired_returns_retry() {
        let intent = make_stored_intent(EffectId::new(), StoreRetryClass::Idempotent, "claimed");
        let store = Arc::new(ExpiredLeaseStore {
            intents: vec![intent],
        });
        let recovery = CrashRecovery::new(store);
        let actions = recovery.recover_all().await.expect("recover");
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], RecoveryAction::Retry { .. }));
    }

    #[tokio::test]
    async fn recover_all_broadcast_expired_returns_manual_resolution() {
        let intent = make_stored_intent_with_kind(
            EffectId::new(),
            StoreRetryClass::NoAutoRetry,
            "broadcast",
        );
        let store = Arc::new(ExpiredLeaseStore {
            intents: vec![intent],
        });
        let recovery = CrashRecovery::new(store);
        let actions = recovery.recover_all().await.expect("recover");
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            RecoveryAction::RequiresManualResolution { .. }
        ));
    }

    #[tokio::test]
    async fn recover_all_mixed_intents_returns_correct_actions() {
        let intents = vec![
            make_stored_intent_with_kind(
                EffectId::new(),
                StoreRetryClass::Idempotent,
                "chain_read",
            ),
            make_stored_intent_with_kind(
                EffectId::new(),
                StoreRetryClass::NoAutoRetry,
                "signature_request",
            ),
            make_stored_intent_with_kind(
                EffectId::new(),
                StoreRetryClass::CheckBeforeRetry,
                "tool_call",
            ),
        ];

        let store = Arc::new(ExpiredLeaseStore { intents });
        let recovery = CrashRecovery::new(store);
        let actions = recovery.recover_all().await.expect("recover");

        assert_eq!(actions.len(), 3);

        let retries: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, RecoveryAction::Retry { .. }))
            .collect();
        let manual: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, RecoveryAction::RequiresManualResolution { .. }))
            .collect();

        assert_eq!(retries.len(), 2, "chain_read and tool_call should retry");
        assert_eq!(
            manual.len(),
            1,
            "signature_request should require manual resolution"
        );
    }

    #[test]
    fn is_idempotent_returns_true_for_idempotent_and_check_before() {
        let idempotent =
            make_stored_intent(EffectId::new(), StoreRetryClass::Idempotent, "pending");
        let check_before = make_stored_intent(
            EffectId::new(),
            StoreRetryClass::CheckBeforeRetry,
            "pending",
        );
        let no_auto = make_stored_intent(EffectId::new(), StoreRetryClass::NoAutoRetry, "pending");

        assert!(CrashRecovery::is_idempotent(&idempotent));
        assert!(CrashRecovery::is_idempotent(&check_before));
        assert!(!CrashRecovery::is_idempotent(&no_auto));
    }

    #[test]
    fn recovery_action_serde_round_trip() {
        let id = EffectId::new();
        let action = RecoveryAction::RequiresManualResolution {
            intent_id: id,
            context: "broadcast may have been submitted".to_string(),
            resolution_hint: "check the chain".to_string(),
        };
        let json = serde_json::to_string(&action).unwrap();
        let back: RecoveryAction = serde_json::from_str(&json).unwrap();
        assert_eq!(action, back);
    }
}

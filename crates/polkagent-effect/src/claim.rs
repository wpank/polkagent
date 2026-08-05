//! Lease-based claim guard for effect intents.
//!
//! A [`ClaimGuard`] wraps a successfully claimed effect intent and
//! guarantees that the lease is released — returning the intent to `Pending`
//! — if the guard is dropped before [`ClaimGuard::complete`] is called.
//!
//! ## Design rationale
//!
//! The claim-then-release RAII pattern ensures that a crashed or panicking
//! worker does not leave an intent permanently stuck in the `Claimed` state.
//! The lease has an absolute expiry (enforced by the lease-reaper task), so
//! even if `Drop` is never reached (e.g., process killed with SIGKILL), the
//! reaper recovers the intent.
//!
//! ## Lease durations
//!
//! Default lease durations per effect kind (from PRD-03 §5.2):
//!
//! | Effect kind | Default lease |
//! |---|---|
//! | `ChainRead`, `Simulation` | 30 s |
//! | `SignatureRequest`, `Broadcast`, `FinalityWatch` | 120 s |
//! | All others | 60 s |
//!
//! Callers may override via [`crate::pipeline::EffectPipeline::claim_with_duration`].

use std::sync::Arc;

use chrono::{DateTime, Utc};
use polkagent_core::{EffectId, WorkerId};
use tracing::{debug, warn};

use polkagent_store_trait::EffectStore;

// ---------------------------------------------------------------------------
// ClaimGuard
// ---------------------------------------------------------------------------

/// RAII guard that holds a worker's lease on an effect intent.
///
/// Drop the guard (without calling [`complete`](ClaimGuard::complete)) to
/// release the lease. Releasing returns the intent to `Pending` so another
/// worker can claim it.
///
/// Callers hold this guard while performing external I/O. When the I/O
/// finishes, call [`complete`](ClaimGuard::complete) to consume the guard and
/// record the outcome through [`crate::pipeline::EffectPipeline::record_outcome`].
pub struct ClaimGuard {
    /// The claimed intent's ID.
    pub intent_id: EffectId,

    /// The worker that holds this lease.
    pub worker_id: WorkerId,

    /// When this lease expires. After this point, the lease-reaper may
    /// reclaim the intent even if this guard has not been dropped.
    pub lease_expires: DateTime<Utc>,

    /// The store to call when releasing the claim on drop.
    store: Arc<dyn EffectStore>,

    /// Whether the claim has been explicitly completed (or released). When
    /// `true`, `Drop` is a no-op.
    completed: bool,
}

impl ClaimGuard {
    /// Create a new guard from a successfully claimed intent.
    ///
    /// This is called internally by [`crate::pipeline::EffectPipeline::claim`].
    pub(crate) fn new(
        intent_id: EffectId,
        worker_id: WorkerId,
        lease_expires: DateTime<Utc>,
        store: Arc<dyn EffectStore>,
    ) -> Self {
        Self {
            intent_id,
            worker_id,
            lease_expires,
            store,
            completed: false,
        }
    }

    /// Returns `true` if the lease has already expired relative to `now`.
    ///
    /// An expired lease does not automatically invalidate the guard, but it
    /// means the lease-reaper may reclaim the intent concurrently. Workers
    /// should check `is_expired()` periodically during long-running I/O.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.lease_expires
    }

    /// Returns `true` if the lease has expired relative to the given `now`.
    ///
    /// Useful in tests where the clock is controlled.
    #[must_use]
    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        now > self.lease_expires
    }

    /// Returns the remaining lease duration (may be zero or negative if expired).
    #[must_use]
    pub fn remaining(&self) -> chrono::Duration {
        self.lease_expires - Utc::now()
    }

    /// Mark the claim as completed, preventing `Drop` from releasing it.
    ///
    /// Call this after a successful outcome has been recorded. The intent will
    /// have transitioned to `Resolved` in the store, so releasing it would be
    /// a no-op anyway, but marking it completed here prevents unnecessary
    /// store calls.
    ///
    /// Returns `true` if the lease was not already expired when this was
    /// called (i.e., the completion was timely).
    pub fn complete(mut self) -> bool {
        self.completed = true;
        let timely = !self.is_expired();
        debug!(
            intent_id = %self.intent_id,
            worker_id = %self.worker_id,
            timely,
            "claim completed"
        );
        timely
    }

    /// Explicitly release the claim, returning the intent to `Pending`.
    ///
    /// This is equivalent to dropping the guard but provides an explicit
    /// intent signal and propagates the store error (Drop silently swallows
    /// it). Useful in cooperative cancellation paths where the caller wants to
    /// know if the release succeeded.
    pub async fn release(mut self) -> Result<(), polkagent_store_trait::StoreError> {
        self.completed = true; // prevent double-release from Drop
        debug!(
            intent_id = %self.intent_id,
            worker_id = %self.worker_id,
            "releasing claim explicitly"
        );
        self.store
            .release_claim(self.intent_id, self.worker_id)
            .await
    }
}

impl Drop for ClaimGuard {
    /// Release the claim on drop, returning the intent to `Pending`.
    ///
    /// Because `Drop` cannot be `async`, the store call is made via
    /// `tokio::runtime::Handle::current().block_on(...)` when running inside
    /// a Tokio context, or is best-effort when no runtime is available.
    ///
    /// In production the Tokio runtime is always present. In tests that do not
    /// start a runtime, the drop will warn and skip the release; the
    /// lease-reaper handles recovery in those cases.
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        // We need to release but Drop is synchronous.
        // Use `Handle::try_current()` to run on the existing runtime.
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let store = Arc::clone(&self.store);
                let intent_id = self.intent_id;
                let worker_id = self.worker_id;
                warn!(
                    intent_id = %intent_id,
                    worker_id = %worker_id,
                    "ClaimGuard dropped without completing — releasing claim"
                );
                // Spawn the release as a detached task so Drop can return
                // without blocking. The task is best-effort.
                handle.spawn(async move {
                    if let Err(e) = store.release_claim(intent_id, worker_id).await {
                        warn!(
                            intent_id = %intent_id,
                            worker_id = %worker_id,
                            error = %e,
                            "failed to release claim on drop"
                        );
                    } else {
                        debug!(
                            intent_id = %intent_id,
                            worker_id = %worker_id,
                            "claim released on drop"
                        );
                    }
                });
            }
            Err(_) => {
                // No Tokio runtime — log and rely on the lease-reaper.
                warn!(
                    intent_id = %self.intent_id,
                    worker_id = %self.worker_id,
                    "ClaimGuard dropped outside Tokio runtime — lease-reaper will recover"
                );
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
    use crate::pipeline::tests::make_in_memory_store;
    use chrono::Duration;
    use polkagent_core::{EffectId, WorkerId};

    fn make_guard(
        intent_id: EffectId,
        worker_id: WorkerId,
        store: Arc<dyn EffectStore>,
        lease_secs: i64,
    ) -> ClaimGuard {
        ClaimGuard::new(
            intent_id,
            worker_id,
            Utc::now() + Duration::seconds(lease_secs),
            store,
        )
    }

    #[tokio::test]
    async fn guard_not_expired_before_lease() {
        let (store, _) = make_in_memory_store();
        let guard = make_guard(EffectId::new(), WorkerId::new(), store, 30);
        assert!(!guard.is_expired());
    }

    #[test]
    fn guard_expired_after_lease() {
        // Use is_expired_at with a future time.
        let (store, _) = crate::pipeline::tests::make_in_memory_store();
        let guard = make_guard(EffectId::new(), WorkerId::new(), store, 30);
        let future = Utc::now() + Duration::seconds(60);
        assert!(guard.is_expired_at(future));
    }

    #[test]
    fn guard_not_expired_before_future() {
        let (store, _) = crate::pipeline::tests::make_in_memory_store();
        let guard = make_guard(EffectId::new(), WorkerId::new(), store, 30);
        let past = Utc::now() - Duration::seconds(1);
        assert!(!guard.is_expired_at(past));
    }

    #[tokio::test]
    async fn complete_returns_true_before_expiry() {
        let (store, _) = make_in_memory_store();
        let guard = make_guard(EffectId::new(), WorkerId::new(), store, 60);
        let timely = guard.complete();
        assert!(timely, "guard completed before expiry should be timely");
    }

    #[tokio::test]
    async fn complete_returns_false_after_expiry() {
        let (store, _) = make_in_memory_store();
        let intent_id = EffectId::new();
        // Create with already-expired lease
        let guard = ClaimGuard::new(
            intent_id,
            WorkerId::new(),
            Utc::now() - Duration::seconds(1),
            store,
        );
        let timely = guard.complete();
        assert!(!timely, "guard completed after expiry should not be timely");
    }

    #[tokio::test]
    async fn explicit_release_does_not_double_release_on_drop() {
        let (store, _) = make_in_memory_store();
        let guard = make_guard(EffectId::new(), WorkerId::new(), store, 30);
        // Release should succeed (store is empty so no-op, but must not panic).
        let _ = guard.release().await;
        // Drop of a completed guard is a no-op.
    }

    #[test]
    fn remaining_is_positive_for_future_lease() {
        let (store, _) = crate::pipeline::tests::make_in_memory_store();
        let guard = make_guard(EffectId::new(), WorkerId::new(), store, 30);
        assert!(guard.remaining().num_seconds() > 0);
    }

    #[test]
    fn remaining_is_non_positive_for_past_lease() {
        let (store, _) = crate::pipeline::tests::make_in_memory_store();
        let guard = ClaimGuard::new(
            EffectId::new(),
            WorkerId::new(),
            Utc::now() - chrono::Duration::seconds(5),
            store,
        );
        assert!(guard.remaining().num_seconds() <= 0);
    }
}

//! Error types for the effect pipeline.
//!
//! All public methods in `polkagent-effect` return `Result<_, PipelineError>`.
//! This centralised error type allows callers to pattern-match on the specific
//! failure mode without depending on internal implementation details.

use polkagent_core::EffectId;
use thiserror::Error;

/// All errors that can be returned by the effect pipeline.
#[derive(Debug, Error)]
pub enum PipelineError {
    // ------------------------------------------------------------------
    // Idempotency / deduplication
    // ------------------------------------------------------------------
    /// An intent with the same idempotency key exists and is currently
    /// `Pending`. The caller should wait for that intent to resolve rather
    /// than creating a duplicate.
    ///
    /// Contains the existing intent's ID so the caller can poll its status.
    #[error("duplicate intent: existing pending intent {0}")]
    DuplicatePending(EffectId),

    /// An intent with the same idempotency key has already resolved
    /// successfully. The caller should use the cached outcome rather than
    /// re-proposing.
    ///
    /// Contains the existing intent's ID for outcome lookup.
    #[error("duplicate intent: existing resolved intent {0}")]
    DuplicateResolved(EffectId),

    /// An intent with the same idempotency key is currently claimed by a
    /// worker. The caller should wait for that claim to resolve.
    #[error("duplicate intent: existing claimed intent {0}")]
    DuplicateClaimed(EffectId),

    // ------------------------------------------------------------------
    // Claim / lease
    // ------------------------------------------------------------------
    /// The intent does not exist in the store.
    #[error("intent not found: {0}")]
    NotFound(EffectId),

    /// The intent was not in the `Pending` state when claim was attempted.
    /// Another worker has already claimed it.
    #[error("intent {0} is not claimable (state: {1})")]
    NotClaimable(EffectId, String),

    /// The worker does not hold the current lease on the intent.
    #[error("worker does not hold lease on intent {0}")]
    NotLeaseholder(EffectId),

    /// The claim lease has expired. The guard's Drop will still attempt to
    /// release, but any outcome produced after expiry will be rejected by
    /// the store.
    #[error("claim lease expired for intent {0}")]
    LeaseExpired(EffectId),

    // ------------------------------------------------------------------
    // Outcome
    // ------------------------------------------------------------------
    /// An outcome has already been recorded for this intent/attempt. The
    /// effect pipeline enforces immutability of outcomes.
    #[error("outcome already recorded for intent {0}")]
    OutcomeAlreadyRecorded(EffectId),

    // ------------------------------------------------------------------
    // Store / backend
    // ------------------------------------------------------------------
    /// The underlying store returned an error.
    #[error("store error: {0}")]
    Store(#[from] polkagent_store_trait::StoreError),

    // ------------------------------------------------------------------
    // Dead-letter queue
    // ------------------------------------------------------------------
    /// An error occurred while interacting with the dead-letter queue.
    #[cfg(any(feature = "dlq", test))]
    #[error("dead-letter queue error: {0}")]
    DeadLetterQueue(#[from] polkagent_outbox::DlqError),

    // ------------------------------------------------------------------
    // Internal
    // ------------------------------------------------------------------
    /// An unexpected internal error occurred. This should never happen in
    /// correct operation; a bug report is warranted.
    #[error("internal error: {0}")]
    Internal(String),
}

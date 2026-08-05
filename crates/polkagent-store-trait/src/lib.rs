//! Storage trait abstractions for the Polkagent platform.
//!
//! This crate defines the hexagonal architecture storage port boundaries.
//! Each trait represents a distinct persistence contract. Implementations
//! (SQLite, Postgres, in-memory) live in separate adapter crates.
//!
//! # Traits
//!
//! - [`RunStore`] — CRUD for Run lifecycle records.
//! - [`EffectStore`] — Durable storage for the effect pipeline (intents,
//!   attempts, outcomes).
//! - [`ArtifactStore`] — Content-addressed artifact storage.
//! - [`EventStore`] — Append-only ordered event log (see [`event`] module).

pub mod event;

#[cfg(feature = "test-contracts")]
pub mod conformance;

use async_trait::async_trait;
use polkagent_core::{
    ArtifactId, EffectAttemptId, EffectId, EffectOutcomeId, RunId, StepId, Timestamp, TurnId,
    WorkerId,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

// ---------------------------------------------------------------------------
// StoreError — shared error taxonomy for all store traits
// ---------------------------------------------------------------------------

/// Unified error type returned by all store trait methods.
///
/// Adapters must map their backend-specific errors into these variants so that
/// the application layer can handle errors without depending on any specific
/// storage technology.
#[derive(Debug, Error)]
pub enum StoreError {
    /// A required record does not exist in the store.
    #[error("not found: {resource_type} id={id}")]
    NotFound {
        /// Short name of the resource type (e.g. "Run", "Artifact").
        resource_type: &'static str,
        /// String representation of the requested ID.
        id: String,
    },

    /// A write would violate a uniqueness constraint (e.g. duplicate primary key).
    #[error("conflict: {resource_type} id={id} already exists")]
    Conflict {
        resource_type: &'static str,
        id: String,
    },

    /// Attempted to append an event with a sequence number already used for that run.
    ///
    /// This is the primary guard against duplicate event writes and ensures
    /// strict ordering within a run. See also [`event::EventStoreError::NonMonotonicSequence`].
    #[error("sequence conflict: event sequence {sequence} already exists for run {run_id}")]
    SequenceConflict {
        run_id: RunId,
        /// The duplicate sequence number.
        sequence: u64,
    },

    /// A content-digest verification failed during artifact retrieval.
    ///
    /// Indicates storage corruption or tampering.
    #[error("integrity error: digest mismatch for {resource_type} id={id}")]
    IntegrityError {
        resource_type: &'static str,
        id: String,
    },

    /// The store backend is unavailable. The operation may be retried.
    #[error("connection error: {message}")]
    ConnectionError { message: String },

    /// An operation was attempted on a record in an invalid state for that
    /// transition (e.g., claiming an already-claimed intent).
    #[error("invalid state transition: {message}")]
    InvalidTransition { message: String },

    /// Serialisation or deserialisation of a stored value failed.
    #[error("serialisation error: {message}")]
    Serialisation { message: String },

    /// An internal backend error that does not fit other variants.
    #[error("internal store error: {message}")]
    Internal { message: String },
}

impl StoreError {
    /// Returns `true` if the caller may safely retry the operation.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::ConnectionError { .. })
    }
}

// ---------------------------------------------------------------------------
// Supporting types for RunStore
// ---------------------------------------------------------------------------

/// Opaque run status string used by the store layer.
///
/// The store layer does not interpret the status; it is serialized and
/// deserialized as-is. The kernel layer owns the state machine logic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStatus(pub String);

impl RunStatus {
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A minimal run summary record returned by list operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: RunId,
    pub agent_id: String,
    pub status: RunStatus,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    /// Wall-clock deadline for the run. `None` if no timeout is configured.
    pub deadline_at: Option<Timestamp>,
}

/// A minimal turn summary record returned by [`RunStore::list_turns`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnSummaryRecord {
    /// Turn UUID.
    pub id: TurnId,
    /// Parent run UUID.
    pub run_id: RunId,
    /// 1-based sequence within the run.
    pub sequence: u32,
    /// Role of this turn (user, assistant, system, tool).
    pub role: String,
    /// When the turn started (ISO 8601).
    pub started_at: Timestamp,
    /// When the turn completed (ISO 8601), if finished.
    pub completed_at: Option<Timestamp>,
    /// Input tokens consumed by this turn.
    pub input_tokens: u32,
    /// Output tokens produced by this turn.
    pub output_tokens: u32,
}

// ---------------------------------------------------------------------------
// Supporting types for EffectStore
//
// These are intentionally simpler than the full domain types in polkagent-effect
// to avoid a circular dependency. The effect crate owns the rich types; the
// store trait owns only the minimal shapes the trait needs.
// ---------------------------------------------------------------------------

/// Retry class for an effect intent. Mirrors `polkagent_core::RetryClass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreRetryClass {
    Idempotent,
    CheckBeforeRetry,
    NoAutoRetry,
}

/// Lightweight summary of an effect intent stored in the outbox.
///
/// The full intent (with kind, parameters, etc.) is stored as a JSON blob in
/// the `payload` field to avoid defining the full domain type in this crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredIntent {
    /// Stable identifier for this intent.
    pub id: EffectId,
    /// The run this intent belongs to.
    pub run_id: RunId,
    /// The step that created this intent.
    pub step_id: StepId,
    /// State tag (e.g. "pending", "claimed", "resolved").
    pub state: String,
    /// Worker that holds the current lease, if any.
    pub lease_owner: Option<WorkerId>,
    /// When the current lease expires.
    pub lease_expires: Option<Timestamp>,
    /// Retry class controlling auto-retry behaviour.
    pub retry_class: StoreRetryClass,
    /// Full intent payload as an opaque JSON value.
    pub payload: serde_json::Value,
    /// Idempotency key for deduplication within a run.
    pub idempotency_key: String,
    /// Created-at timestamp.
    pub created_at: Timestamp,
}

/// Lightweight summary of a recorded outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredOutcome {
    pub id: EffectOutcomeId,
    pub intent_id: EffectId,
    pub attempt_id: EffectAttemptId,
    pub run_id: RunId,
    /// Whether the outcome has already been fed to the reducer.
    pub consumed: bool,
    /// Full outcome payload as an opaque JSON value.
    pub payload: serde_json::Value,
    pub observed_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Supporting types for ArtifactStore
// ---------------------------------------------------------------------------

/// A minimal artifact metadata record returned by list operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSummary {
    pub id: ArtifactId,
    /// Serialized `ArtifactKind` tag string.
    pub kind: String,
    /// Hash algorithm (e.g. "blake3").
    pub algorithm: String,
    /// Hex-encoded digest of the artifact body.
    pub digest_hex: String,
    /// Data classification string (e.g. "public", "private").
    pub classification: String,
    pub run_id: Option<RunId>,
    pub created_at: Timestamp,
}

// ---------------------------------------------------------------------------
// EffectStore trait
// ---------------------------------------------------------------------------

/// Durable storage for the effect pipeline.
///
/// All write paths open their transactions with `BEGIN IMMEDIATE` (SQLite) or
/// `SELECT ... FOR UPDATE` (Postgres) to serialise concurrent writers. The
/// single-writer Tokio task pattern described in PRD-03 §13.4 reinforces this
/// at the application layer.
///
/// # Invariants
///
/// - `propose_intent` persists an intent **before** any external I/O occurs
///   (EFF-INV-1). Callers must not perform I/O between constructing the intent
///   and calling this method.
/// - `claim_intent` is atomic: exactly one caller succeeds for any given
///   intent when multiple callers race (EFF-INV-2 / CONC-5).
/// - `record_outcome` is idempotent with respect to uniqueness: calling it
///   twice with the same `outcome_id` must return `StoreError::Conflict` on
///   the second call rather than silently overwriting the outcome (EFF-INV-3).
/// - `release_claim` must be a no-op if the intent is already resolved or
///   the claim has already expired — this makes the `ClaimGuard::drop` path
///   safe to call from any context.
#[async_trait]
pub trait EffectStore: Send + Sync {
    // ------------------------------------------------------------------
    // Intent lifecycle
    // ------------------------------------------------------------------

    /// Persist a new intent in the `Pending` state.
    ///
    /// This is the first operation in the effect pipeline. The intent must be
    /// fully formed before calling this method. On return, the intent is
    /// durably stored and visible to workers via `claim_intent`.
    ///
    /// Returns `StoreError::Conflict` if an intent with the same `id` already
    /// exists (idempotency-key deduplication is handled in the pipeline layer,
    /// not here).
    async fn propose_intent(&self, intent: StoredIntent) -> Result<(), StoreError>;

    /// Atomically claim the next `Pending` intent for `worker_id`, granting
    /// a lease of `lease_duration`.
    ///
    /// Only intents whose `state == "Pending"` (and whose previous lease, if
    /// any, has expired) are eligible. Returns `None` when the outbox is empty.
    async fn claim_intent(
        &self,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<Option<StoredIntent>, StoreError>;

    /// Claim a specific intent by ID, used during recovery to re-lease an
    /// intent whose previous worker crashed.
    async fn claim_intent_by_id(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<StoredIntent, StoreError>;

    /// Release the claim on an intent, returning it to `Pending` so another
    /// worker can claim it.
    ///
    /// Must be a no-op if the intent is already `Resolved` or if the claim
    /// is held by a different worker.
    async fn release_claim(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
    ) -> Result<(), StoreError>;

    /// Fetch a single intent by ID.
    async fn get_intent(&self, intent_id: EffectId) -> Result<StoredIntent, StoreError>;

    /// Fetch all intents for a given run (pending, claimed, and resolved).
    async fn get_by_run(&self, run_id: RunId) -> Result<Vec<StoredIntent>, StoreError>;

    /// Fetch an intent by its idempotency key.
    ///
    /// Returns `Ok(None)` if no intent with the given key exists.
    ///
    /// Implementations backed by SQL should use the
    /// `idx_effects_by_idempotency_key` index for O(1) lookup. The default
    /// implementation falls back to `get_by_run` + linear scan, which is
    /// O(N) in the number of intents for the run.
    async fn get_by_idempotency_key(
        &self,
        key: &str,
        run_id: RunId,
    ) -> Result<Option<StoredIntent>, StoreError> {
        let intents = self.get_by_run(run_id).await?;
        Ok(intents.into_iter().find(|i| i.idempotency_key == key))
    }

    /// Fetch all intents whose lease has expired (i.e., state is `"claimed"`
    /// and `lease_expires < cutoff`).
    async fn expired_leases(&self, cutoff: Timestamp) -> Result<Vec<StoredIntent>, StoreError>;

    /// Atomically update the state of an existing intent.
    ///
    /// Used by the approval subsystem to transition an intent between states
    /// (e.g. `"pending"` → `"approved"` or `"pending"` → `"denied"`).
    ///
    /// Returns `StoreError::NotFound` if no intent with `intent_id` exists.
    /// Returns `StoreError::InvalidTransition` if the requested transition is
    /// not valid from the intent's current state.
    async fn update_intent_state(
        &self,
        intent_id: EffectId,
        new_state: &str,
    ) -> Result<StoredIntent, StoreError>;

    // ------------------------------------------------------------------
    // Attempt lifecycle
    // ------------------------------------------------------------------

    /// Record that an attempt is starting for the given intent.
    ///
    /// The attempt is linked to the intent and the worker that holds the
    /// current lease.
    async fn record_attempt_start(
        &self,
        attempt_id: EffectAttemptId,
        intent_id: EffectId,
        worker_id: WorkerId,
        payload: serde_json::Value,
    ) -> Result<(), StoreError>;

    // ------------------------------------------------------------------
    // Outcome lifecycle
    // ------------------------------------------------------------------

    /// Record an immutable outcome for an attempt.
    ///
    /// Transitions the corresponding intent to `Resolved`. Returns
    /// `StoreError::Conflict` if an outcome for this `attempt_id` already
    /// exists.
    async fn record_outcome(&self, outcome: StoredOutcome) -> Result<(), StoreError>;

    /// Fetch all unconsumed outcomes for a run (i.e., outcomes that have not
    /// yet been fed to the reducer).
    async fn unconsumed_outcomes(&self, run_id: RunId) -> Result<Vec<StoredOutcome>, StoreError>;

    /// Mark outcomes as consumed after the reducer has processed them.
    async fn mark_outcomes_consumed(
        &self,
        outcome_ids: &[EffectOutcomeId],
    ) -> Result<(), StoreError>;
}

// ---------------------------------------------------------------------------
// RunStore
// ---------------------------------------------------------------------------

/// Persistence boundary for Run lifecycle records.
///
/// # Contract
///
/// - `create` fails with [`StoreError::Conflict`] if the `run_id` already
///   exists.
/// - `get` returns [`StoreError::NotFound`] if no run exists with the given
///   ID.
/// - `update_state` must be atomic: partial updates are not permitted.
/// - `list_by_agent` and `list_by_state` return summaries in descending
///   creation order unless the implementation specifies otherwise.
/// - Implementations must be `Send + Sync + 'static`.
#[async_trait]
pub trait RunStore: Send + Sync + 'static {
    /// Persist a new run record.
    ///
    /// Returns [`StoreError::Conflict`] if the ID already exists.
    async fn create(
        &self,
        run_id: RunId,
        agent_id: &str,
        status: RunStatus,
    ) -> Result<(), StoreError>;

    /// Retrieve a run summary by ID.
    ///
    /// Returns [`StoreError::NotFound`] if no matching record exists.
    async fn get(&self, run_id: RunId) -> Result<RunSummary, StoreError>;

    /// Atomically update the status of an existing run.
    async fn update_state(&self, run_id: RunId, new_status: RunStatus) -> Result<(), StoreError>;

    /// List runs belonging to a specific agent, ordered by creation time
    /// descending.
    async fn list_by_agent(
        &self,
        agent_id: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<RunSummary>, StoreError>;

    /// List runs with a specific status, ordered by creation time descending.
    async fn list_by_state(
        &self,
        status: RunStatus,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<RunSummary>, StoreError>;

    /// Persist a completed turn record (including token usage).
    ///
    /// The turn is identified by `turn_id` and linked to its parent run via
    /// `run_id`. `sequence` is 1-based and monotonic within the run. The
    /// `role`, `started_at`, `completed_at`, `input_tokens`, and
    /// `output_tokens` fields are stored verbatim.
    ///
    /// Returns [`StoreError::Conflict`] if a turn with the same `turn_id`
    /// already exists.
    async fn insert_turn(
        &self,
        turn_id: TurnId,
        run_id: RunId,
        sequence: u32,
        role: &str,
        started_at: &str,
        completed_at: Option<&str>,
        input_tokens: u32,
        output_tokens: u32,
    ) -> Result<(), StoreError>;

    /// List all turns for a run, ordered by sequence ascending.
    ///
    /// Returns an empty `Vec` if no turns have been recorded for the run.
    /// The default implementation returns an empty list for backward
    /// compatibility with stores that have not yet implemented turn queries.
    async fn list_turns(&self, _run_id: RunId) -> Result<Vec<TurnSummaryRecord>, StoreError> {
        Ok(vec![])
    }

    /// Set the wall-clock deadline for a run.
    ///
    /// Stores the absolute `DateTime<Utc>` so the deadline survives process
    /// restarts. Pass `None` to clear a previously set deadline.
    ///
    /// The default implementation is a no-op for backward compatibility with
    /// stores that have not yet added the `deadline_at` column.
    async fn set_deadline(
        &self,
        _run_id: RunId,
        _deadline: Option<polkagent_core::Timestamp>,
    ) -> Result<(), StoreError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ArtifactStore
// ---------------------------------------------------------------------------

/// Content-addressed, classification-enforcing artifact persistence boundary.
///
/// # Contract
///
/// - `store` is idempotent: storing the same artifact twice (same digest)
///   must succeed on the second call without error or duplication.
/// - `get_body` **must** verify the BLAKE3 digest of the returned bytes
///   against the stored `digest_hex`. A mismatch must return
///   [`StoreError::IntegrityError`].
/// - `SecretForbidden`-classified artifacts must be rejected at store
///   boundaries that are not secret-specific stores.
/// - `verify` checks the stored body without returning it (cheaper than
///   `get_body` for large blobs).
#[async_trait]
pub trait ArtifactStore: Send + Sync + 'static {
    /// Persist an artifact and its body bytes.
    ///
    /// Idempotent: a second call with the same `artifact_id` and matching
    /// digest must succeed silently.
    async fn store(
        &self,
        artifact_id: ArtifactId,
        run_id: Option<RunId>,
        kind: &str,
        algorithm: &str,
        digest_hex: &str,
        classification: &str,
        body: &[u8],
    ) -> Result<(), StoreError>;

    /// Retrieve artifact metadata by ID.
    async fn get(&self, id: ArtifactId) -> Result<ArtifactSummary, StoreError>;

    /// Retrieve and verify the raw artifact body.
    ///
    /// Implementations **must** verify the stored digest of the returned bytes.
    /// If verification fails, return [`StoreError::IntegrityError`].
    async fn get_body(&self, id: ArtifactId) -> Result<Vec<u8>, StoreError>;

    /// Verify that the stored body matches the artifact's digest without
    /// returning the body bytes.
    ///
    /// Returns `true` if the body passes verification, `false` if no body is
    /// stored, and `Err` for backend or integrity errors.
    async fn verify(&self, id: ArtifactId) -> Result<bool, StoreError>;

    /// List artifact summaries for a given run, in creation order.
    async fn list_for_run(&self, run_id: RunId) -> Result<Vec<ArtifactSummary>, StoreError>;

    /// Record a parent-child lineage edge.
    ///
    /// Adding a duplicate edge MUST be idempotent.
    ///
    /// The default implementation returns an error indicating the store does
    /// not support lineage tracking.
    async fn add_lineage(
        &self,
        _child_id: ArtifactId,
        _parent_id: ArtifactId,
    ) -> Result<(), StoreError> {
        Err(StoreError::Internal {
            message: "lineage tracking not supported by this store".into(),
        })
    }

    /// Return the ordered chain of ancestor artifact IDs for `id`.
    ///
    /// The ordering is implementation-defined but MUST be consistent across
    /// calls.  Typically ancestors are returned in breadth-first order
    /// (nearest parents first).
    ///
    /// The default implementation returns an empty vec (no lineage).
    async fn get_lineage(&self, _id: ArtifactId) -> Result<Vec<ArtifactId>, StoreError> {
        Ok(Vec::new())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_error_not_found_is_not_retryable() {
        let e = StoreError::NotFound {
            resource_type: "Run",
            id: "abc".into(),
        };
        assert!(!e.is_retryable());
    }

    #[test]
    fn store_error_connection_is_retryable() {
        let e = StoreError::ConnectionError {
            message: "lost".into(),
        };
        assert!(e.is_retryable());
    }

    #[test]
    fn store_error_conflict_is_not_retryable() {
        let e = StoreError::Conflict {
            resource_type: "Artifact",
            id: "123".into(),
        };
        assert!(!e.is_retryable());
    }

    #[test]
    fn store_error_sequence_conflict_display() {
        let run_id = RunId::new();
        let e = StoreError::SequenceConflict {
            run_id,
            sequence: 7,
        };
        let msg = format!("{e}");
        assert!(msg.contains('7'));
    }

    #[test]
    fn run_status_round_trip() {
        let s = RunStatus::new("executing");
        assert_eq!(s.as_str(), "executing");
        let json = serde_json::to_string(&s).expect("serialize");
        let back: RunStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
    }

    #[test]
    fn artifact_summary_serializes() {
        let summary = ArtifactSummary {
            id: ArtifactId::new(),
            kind: "file".into(),
            algorithm: "blake3".into(),
            digest_hex: "abc123".into(),
            classification: "public".into(),
            run_id: None,
            created_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&summary).expect("serialize");
        assert!(json.contains("blake3"));
    }

    #[test]
    fn stored_intent_has_step_id() {
        let intent = StoredIntent {
            id: EffectId::new(),
            run_id: RunId::new(),
            step_id: StepId::new(),
            state: "pending".into(),
            lease_owner: None,
            lease_expires: None,
            retry_class: StoreRetryClass::Idempotent,
            payload: serde_json::Value::Null,
            idempotency_key: "key-1".into(),
            created_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&intent).expect("serialize");
        assert!(json.contains("step_id"));
    }

    /// Compile-time check: `RunStore` can be used as a `dyn` trait object.
    #[allow(dead_code)]
    fn _run_store_is_object_safe(_s: &dyn RunStore) {}

    /// Compile-time check: `EffectStore` can be used as a `dyn` trait object.
    #[allow(dead_code)]
    fn _effect_store_is_object_safe(_s: &dyn EffectStore) {}

    /// Compile-time check: `ArtifactStore` can be used as a `dyn` trait object.
    #[allow(dead_code)]
    fn _artifact_store_is_object_safe(_s: &dyn ArtifactStore) {}
}

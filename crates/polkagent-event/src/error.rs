//! Error types for the `polkagent-event` crate.

use polkagent_core::RunId;
use thiserror::Error;

/// All errors that can be returned by the event recording and projection APIs.
#[derive(Debug, Error)]
pub enum EventError {
    // ── Recording invariant violations ───────────────────────────────────
    /// Attempted to record a second terminal event for a run that already has
    /// one (PRD-10 REQ-EVT-004).
    #[error("run {run_id} already has a terminal event; duplicate rejected")]
    DuplicateTerminalEvent { run_id: RunId },

    /// The sequence number provided is not strictly greater than the current
    /// maximum for the run (PRD-10 REQ-EVT-001).
    #[error(
        "sequence {proposed} is not monotonically greater than current \
         max {current} for run {run_id}"
    )]
    NonMonotonicSequence {
        run_id: RunId,
        current: u64,
        proposed: u64,
    },

    // ── Store errors ──────────────────────────────────────────────────────
    /// The underlying event store rejected the write.
    #[error("event store error: {0}")]
    Store(#[from] polkagent_store_trait::event::EventStoreError),

    // ── Projection errors ─────────────────────────────────────────────────
    /// A projection's `apply` method returned an error.
    #[error("projection '{name}' failed to apply event: {source}")]
    ProjectionApply {
        name: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    // ── Serialisation ─────────────────────────────────────────────────────
    /// Serialisation or deserialisation of an event payload failed.
    #[error("serialisation error: {0}")]
    Serialisation(#[from] serde_json::Error),

    /// A persisted event envelope cannot be projected into its canonical type.
    #[error("invalid stored event field: {0}")]
    InvalidStoredEvent(&'static str),
}

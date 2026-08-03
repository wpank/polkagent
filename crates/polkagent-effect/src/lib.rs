//! Effect pipeline for the Polkagent platform.
//!
//! This crate is the **critical safety component** of Polkagent's execution
//! model. It enforces the core invariant:
//!
//! > An `EffectIntent` is persisted to durable storage **before** any external
//! > I/O occurs. If the process crashes after persisting the intent but before
//! > the I/O completes, the intent remains in the outbox and crash recovery
//! > handles it safely.
//!
//! ## Architecture
//!
//! ```text
//!  Reducer                Effect Pipeline               External World
//!    │                          │                            │
//!    │── propose(intent) ──────►│                            │
//!    │                          │── store(intent) ──────────►│(DB)
//!    │◄── EffectId ─────────────│                            │
//!    │                          │                            │
//!    │    Worker loop:          │                            │
//!    │                          │── claim() ───────────────  │
//!    │                          │◄── ClaimGuard ──────────── │
//!    │                          │                            │
//!    │                          │── [external I/O] ─────────►│
//!    │                          │◄── result ─────────────────│
//!    │                          │                            │
//!    │                          │── record_outcome() ───────►│(DB)
//!    │◄── outcome notification ─│                            │
//! ```
//!
//! ## Modules
//!
//! - [`pipeline`] — [`pipeline::EffectPipeline`]: the central struct. Owns
//!   `propose`, `claim`, `record_attempt`, and `record_outcome`.
//! - [`idempotency`] — [`idempotency::IdempotencyKey`]: deterministic key
//!   generation and deduplication rules.
//! - [`claim`] — [`claim::ClaimGuard`]: RAII guard that auto-releases the
//!   lease on drop, preventing stuck `Claimed` intents.
//! - [`recovery`] — [`recovery::CrashRecovery`]: on-startup scanner that
//!   classifies expired leases and recommends retry vs. manual resolution.
//! - [`types`] — domain types: `EffectIntent`, `EffectAttempt`,
//!   `EffectOutcome`, `EffectKind`, state machines, etc.
//! - [`error`] — [`error::PipelineError`]: the unified error type.
//!
//! ## Key invariants enforced by this crate
//!
//! - **EFF-INV-1:** Intent is persisted before any I/O (enforced by `propose`).
//! - **EFF-INV-2:** Claims are lease-based with configurable expiry.
//! - **EFF-INV-3:** Outcomes are immutable once recorded (enforced by
//!   `record_outcome`).
//! - **EFF-INV-4:** Idempotency key is stable across retries.
//! - **EFF-INV-5:** `NoAutoRetry` intents are never automatically retried.

pub mod claim;
pub mod error;
pub mod idempotency;
pub mod pipeline;
pub mod recovery;
pub mod types;

#[cfg(any(feature = "dlq", test))]
pub mod dlq_bridge;

#[cfg(any(feature = "audit", test))]
pub mod audit;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use claim::ClaimGuard;
pub use error::PipelineError;
pub use idempotency::IdempotencyKey;
pub use pipeline::{EffectIntentSpec, EffectPipeline};
pub use recovery::{CrashRecovery, RecoveryAction};
pub use types::{
    ApprovalDecision, ApprovalRecord, ApprovalType, AttemptState, CancellationReason,
    EffectAttempt, EffectIntent, EffectIntentState, EffectKind, EffectOutcome, EffectPriority,
    ErrorClass, OutcomeResult, ResolutionHint, SupersessionReason,
};

#[cfg(feature = "audit")]
pub use audit::{AuditContext, AuditedPipeline};

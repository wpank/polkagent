//! Content-addressed artifact storage for the Polkagent platform.
//!
//! This crate provides:
//!
//! - [`ArtifactService`] — the application-layer facade for creating,
//!   retrieving, verifying, and listing artifacts, as well as recording and
//!   querying lineage.
//! - [`ArtifactStore`] — the storage port trait.  Inject a concrete
//!   implementation (e.g. [`MemoryStore`]) via `Arc<impl ArtifactStore>`.
//! - [`Artifact`] and [`ArtifactKind`] — the core domain types (re-exported
//!   from `polkagent-core`).
//! - [`BlobRef`] — the BLAKE3 content address (re-exported from
//!   `polkagent-core`).
//! - [`LineageGraph`] — an in-memory DAG for tracking artifact provenance.
//! - [`compute_digest`] / [`verify_digest`] — standalone BLAKE3 utilities.
//!
//! # Crate-level invariant
//!
//! An artifact's `blob_ref` is a BLAKE3 hash of its body bytes computed at
//! creation time.  Every read path (including the service layer) re-verifies
//! this hash before returning bytes to callers.  If the stored body does not
//! match the recorded digest, the operation fails with
//! [`ArtifactError::DigestMismatch`].

pub mod digest;
pub mod lineage;
pub mod memory;
pub mod service;
pub mod store;
pub mod types;

// ---------------------------------------------------------------------------
// Top-level re-exports
// ---------------------------------------------------------------------------

/// Domain types re-exported from `polkagent-core`.
pub use polkagent_core::artifact::{Artifact, ArtifactKind, BlobRef};

pub use digest::{compute_digest, verify_digest};
pub use lineage::LineageGraph;
pub use memory::MemoryStore;
pub use service::{ArtifactError, ArtifactService};
pub use store::{ArtifactStore, StoreError};

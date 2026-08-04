//! The `ArtifactStore` port trait and associated error type.
//!
//! This module defines the storage abstraction consumed by [`ArtifactService`].
//! Concrete implementations (SQLite, in-memory, S3, …) live in separate crates
//! and inject their store via `Arc<dyn ArtifactStore>`.
//!
//! The trait is intentionally narrow: it covers the operations that
//! `ArtifactService` requires.  Lineage edges are stored through separate
//! methods so that implementations can choose the most efficient representation.
//!
//! [`ArtifactService`]: crate::service::ArtifactService

use std::future::Future;

use polkagent_core::artifact::Artifact;
use polkagent_core::ids::{ArtifactId, RunId};
use thiserror::Error;

// ---------------------------------------------------------------------------
// StoreError
// ---------------------------------------------------------------------------

/// Errors that an [`ArtifactStore`] implementation may return.
#[derive(Debug, Error)]
pub enum StoreError {
    /// The requested artifact or body was not found.
    #[error("artifact not found: {0}")]
    NotFound(ArtifactId),

    /// The stored body's digest does not match the artifact record.
    #[error("digest mismatch for artifact {0}")]
    DigestMismatch(ArtifactId),

    /// An I/O or backend-specific error.
    #[error("store backend error: {0}")]
    Backend(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),
}

// ---------------------------------------------------------------------------
// ArtifactStore trait
// ---------------------------------------------------------------------------

/// Storage port for artifact metadata and bodies.
///
/// Implementations must be `Send + Sync` so that they can be shared across
/// async tasks behind an `Arc`.
///
/// # Digest contract
///
/// Implementations MUST verify body digests on [`get_body`].  The caller
/// must not need to re-verify what the store has already checked.  If a body
/// is found but its BLAKE3 hash does not match the artifact's `blob_ref`,
/// the implementation MUST return [`StoreError::DigestMismatch`].
///
/// [`get_body`]: ArtifactStore::get_body
pub trait ArtifactStore: Send + Sync + 'static {
    /// Persist an artifact's metadata record and its raw body bytes atomically.
    ///
    /// If an artifact with the same [`ArtifactId`] already exists the
    /// implementation MAY either succeed idempotently or return an error;
    /// callers should not rely on either behaviour.
    fn store(
        &self,
        artifact: &Artifact,
        body: &[u8],
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Retrieve artifact metadata by ID.
    fn get(&self, id: ArtifactId) -> impl Future<Output = Result<Artifact, StoreError>> + Send;

    /// Retrieve and **verify** the raw body for an artifact.
    ///
    /// Implementations MUST check that the returned bytes hash to
    /// `artifact.blob_ref.blake3_hex`; a failed check MUST produce
    /// [`StoreError::DigestMismatch`].
    fn get_body(&self, id: ArtifactId) -> impl Future<Output = Result<Vec<u8>, StoreError>> + Send;

    /// Verify the integrity of a stored artifact without returning the body.
    ///
    /// Returns `Ok(true)` when the stored body matches the record's digest,
    /// `Ok(false)` when the body is absent or the digest does not match.
    fn verify(&self, id: ArtifactId) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Return all artifact metadata records for the given run, in creation
    /// order (ascending by `created_at`).
    fn list_for_run(
        &self,
        run_id: RunId,
    ) -> impl Future<Output = Result<Vec<Artifact>, StoreError>> + Send;

    /// Record a parent-child lineage edge.
    ///
    /// Adding a duplicate edge MUST be idempotent.
    fn add_lineage(
        &self,
        child_id: ArtifactId,
        parent_id: ArtifactId,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Return the ordered chain of ancestor IDs for `id`.
    ///
    /// The ordering is implementation-defined but MUST be consistent across
    /// calls.  Typically ancestors are returned in breadth-first order
    /// (nearest first).
    fn get_lineage(
        &self,
        id: ArtifactId,
    ) -> impl Future<Output = Result<Vec<ArtifactId>, StoreError>> + Send;
}

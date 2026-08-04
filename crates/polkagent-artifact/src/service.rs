//! [`ArtifactService`] — application-layer facade for content-addressed artifact
//! storage.
//!
//! The service owns no mutable state of its own; all persistence is delegated
//! to the injected [`ArtifactStore`].  Business logic that lives here:
//!
//! - Computing the BLAKE3 digest before storage.
//! - Asserting digest integrity on retrieval (`get_body`, `verify`).
//! - Translating store errors into richer [`ArtifactError`] variants.
//! - Structured tracing at every operation boundary.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use polkagent_core::artifact::{Artifact, ArtifactKind};
use polkagent_core::config::DataClassification;
use polkagent_core::ids::{ArtifactId, RunId};
use tracing::{debug, instrument, warn};

use crate::digest::{compute_digest, compute_sha256_digest, verify_digest};
use crate::store::{ArtifactStore, StoreError};

// ---------------------------------------------------------------------------
// ArtifactError
// ---------------------------------------------------------------------------

/// Errors that [`ArtifactService`] methods can produce.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    /// The requested artifact does not exist.
    #[error("artifact not found: {0}")]
    NotFound(ArtifactId),

    /// The stored body's digest does not match the artifact's recorded digest.
    ///
    /// This indicates either storage corruption or deliberate tampering.
    #[error("digest mismatch for artifact {0}: stored body does not match recorded digest")]
    DigestMismatch(ArtifactId),

    /// A failure in the underlying storage backend.
    #[error("store error: {0}")]
    Store(StoreError),
}

impl From<StoreError> for ArtifactError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(id) => Self::NotFound(id),
            StoreError::DigestMismatch(id) => Self::DigestMismatch(id),
            other => Self::Store(other),
        }
    }
}

// ---------------------------------------------------------------------------
// ArtifactService
// ---------------------------------------------------------------------------

/// Application-layer service for content-addressed artifact operations.
///
/// Construct with [`ArtifactService::new`], passing a concrete store
/// implementation wrapped in an `Arc`.
///
/// # Example
///
/// ```ignore
/// let store: Arc<MemoryStore> = Arc::new(MemoryStore::new());
/// let svc = ArtifactService::new(store);
/// let artifact = svc.create(
///     None,
///     ArtifactKind::Custom { type_uri: "test://v1".into() },
///     b"hello",
///     HashMap::new(),
/// ).await?;
/// ```
#[derive(Clone)]
pub struct ArtifactService<S> {
    store: Arc<S>,
}

impl<S: ArtifactStore> ArtifactService<S> {
    /// Create a new [`ArtifactService`] backed by the given store.
    #[must_use]
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    /// Create and persist a new artifact.
    ///
    /// The BLAKE3 digest is computed over `body` and embedded in the returned
    /// [`Artifact`]'s `blob_ref`.  The body is stored under that digest so
    /// that retrieval can always verify integrity.
    ///
    /// # Arguments
    ///
    /// - `run_id`        — optional run that produced this artifact.
    /// - `kind`          — semantic type of the artifact.
    /// - `body`          — raw content bytes; may be empty.
    /// - `metadata`      — arbitrary key/value string annotations.
    /// - `classification` — data sensitivity level (defaults to `Public`).
    #[instrument(skip(self, body, metadata), fields(
        run_id = run_id.as_ref().map(|r| r.to_string()).as_deref().unwrap_or("none"),
        body_len = body.len(),
    ))]
    pub async fn create(
        &self,
        run_id: Option<RunId>,
        kind: ArtifactKind,
        body: &[u8],
        metadata: HashMap<String, String>,
    ) -> Result<Artifact, ArtifactError> {
        let id = ArtifactId::new();
        let mut blob_ref = compute_digest(body);
        blob_ref.sha256_hex = Some(compute_sha256_digest(body));

        let artifact = Artifact {
            id,
            run_id,
            step_id: None,
            kind,
            blob_ref,
            classification: DataClassification::default(),
            parents: Vec::new(),
            created_at: Utc::now(),
            metadata,
        };

        debug!(artifact_id = %id, digest = %artifact.blob_ref.blake3_hex, "storing artifact");

        self.store
            .store(&artifact, body)
            .await
            .map_err(ArtifactError::from)?;

        Ok(artifact)
    }

    /// Retrieve artifact metadata by ID.
    #[instrument(skip(self))]
    pub async fn get(&self, artifact_id: ArtifactId) -> Result<Artifact, ArtifactError> {
        debug!(artifact_id = %artifact_id, "fetching artifact metadata");
        self.store
            .get(artifact_id)
            .await
            .map_err(ArtifactError::from)
    }

    /// Retrieve the raw body for an artifact, verifying the digest before
    /// returning.
    ///
    /// Returns [`ArtifactError::DigestMismatch`] if the stored bytes do not
    /// hash to the artifact's `blob_ref`.
    #[instrument(skip(self))]
    pub async fn get_body(&self, artifact_id: ArtifactId) -> Result<Vec<u8>, ArtifactError> {
        debug!(artifact_id = %artifact_id, "fetching artifact body");

        // Fetch metadata to learn the expected digest.
        let artifact = self
            .store
            .get(artifact_id)
            .await
            .map_err(ArtifactError::from)?;

        // Fetch the body (store may also verify, but we re-verify defensively).
        let body = self
            .store
            .get_body(artifact_id)
            .await
            .map_err(|e| match e {
                StoreError::DigestMismatch(id) => ArtifactError::DigestMismatch(id),
                other => ArtifactError::from(other),
            })?;

        // Service-layer integrity check: re-verify regardless of what the store
        // did, so that a misbehaving or untrusted backend cannot bypass the
        // digest guarantee.
        if !verify_digest(&body, &artifact.blob_ref) {
            warn!(
                artifact_id = %artifact_id,
                expected = %artifact.blob_ref.blake3_hex,
                "digest mismatch detected during get_body"
            );
            return Err(ArtifactError::DigestMismatch(artifact_id));
        }

        Ok(body)
    }

    /// Check whether the stored body for an artifact matches its digest,
    /// without returning the body.
    ///
    /// Returns `Ok(true)` on success, `Ok(false)` if the artifact is missing
    /// or its body does not match.
    #[instrument(skip(self))]
    pub async fn verify(&self, artifact_id: ArtifactId) -> Result<bool, ArtifactError> {
        debug!(artifact_id = %artifact_id, "verifying artifact integrity");
        self.store
            .verify(artifact_id)
            .await
            .map_err(ArtifactError::from)
    }

    /// Return all artifacts associated with a run, in creation order.
    #[instrument(skip(self))]
    pub async fn list_for_run(&self, run_id: RunId) -> Result<Vec<Artifact>, ArtifactError> {
        debug!(run_id = %run_id, "listing artifacts for run");
        self.store
            .list_for_run(run_id)
            .await
            .map_err(ArtifactError::from)
    }

    /// Record that `child_id` was derived from `parent_id`.
    ///
    /// Adding a duplicate edge is idempotent.
    #[instrument(skip(self))]
    pub async fn add_lineage(
        &self,
        child_id: ArtifactId,
        parent_id: ArtifactId,
    ) -> Result<(), ArtifactError> {
        debug!(child_id = %child_id, parent_id = %parent_id, "adding lineage edge");
        self.store
            .add_lineage(child_id, parent_id)
            .await
            .map_err(ArtifactError::from)
    }

    /// Return the ancestor chain for `artifact_id` in breadth-first order
    /// (direct parents first).
    #[instrument(skip(self))]
    pub async fn get_lineage(
        &self,
        artifact_id: ArtifactId,
    ) -> Result<Vec<ArtifactId>, ArtifactError> {
        debug!(artifact_id = %artifact_id, "fetching lineage");
        self.store
            .get_lineage(artifact_id)
            .await
            .map_err(ArtifactError::from)
    }
}

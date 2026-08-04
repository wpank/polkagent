//! Retention enforcement for artifacts and diagnostic events.
//!
//! This module implements configurable retention policies that soft-delete,
//! hard-delete, and archive expired artifacts, as well as removing diagnostic
//! events past their expiry timestamp.
//!
//! # Design
//!
//! Retention operates through the [`RetentionStore`] trait, which extends the
//! base [`ArtifactStore`] with the lifecycle operations needed for enforcement:
//! listing all artifacts, soft-deleting, hard-deleting, and archiving.
//!
//! The [`RetentionEnforcer`] consults configured [`RetentionPolicy`] values and
//! drives the lifecycle transitions, emitting a [`RetentionReport`] summarising
//! the work done and the total bytes freed.
//!
//! # Parent protection (REQ-STORE-053)
//!
//! An artifact that is a parent of any non-deleted artifact must not be
//! deleted.  The enforcer skips such artifacts and records them in
//! `RetentionReport::skipped`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use polkagent_core::artifact::Artifact;
use polkagent_core::ids::ArtifactId;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::store::{ArtifactStore, StoreError};

// ---------------------------------------------------------------------------
// RetentionPolicy
// ---------------------------------------------------------------------------

/// Configures the lifecycle durations for a group of artifacts.
///
/// Durations are measured from each artifact's `created_at` timestamp.
///
/// - `soft_delete_after`: after this duration the artifact metadata is marked
///   as deleted and its body is removed.
/// - `hard_delete_after`: after this additional duration following soft-delete
///   the metadata row itself is purged.
/// - `archive_after`: if `Some`, body bytes are moved to cold storage after
///   this duration (before soft-delete applies).
#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    /// Duration from creation to soft-delete.
    pub soft_delete_after: Duration,
    /// Duration from *soft-delete* to hard-delete of the metadata row.
    pub hard_delete_after: Duration,
    /// Optional duration from creation to archival (cold-storage move).
    pub archive_after: Option<Duration>,
}

impl RetentionPolicy {
    /// Create a simple policy with no archival step.
    #[must_use]
    pub fn new(soft_delete_after: Duration, hard_delete_after: Duration) -> Self {
        Self {
            soft_delete_after,
            hard_delete_after,
            archive_after: None,
        }
    }

    /// Create a policy that also archives artifacts before soft-deletion.
    #[must_use]
    pub fn with_archive(
        soft_delete_after: Duration,
        hard_delete_after: Duration,
        archive_after: Duration,
    ) -> Self {
        Self {
            soft_delete_after,
            hard_delete_after,
            archive_after: Some(archive_after),
        }
    }
}

// ---------------------------------------------------------------------------
// RetentionReport
// ---------------------------------------------------------------------------

/// Summary produced by a single retention enforcement sweep.
#[derive(Debug, Default, Clone)]
pub struct RetentionReport {
    /// Number of artifacts soft-deleted during this sweep.
    pub soft_deleted: usize,
    /// Number of artifact metadata rows hard-deleted during this sweep.
    pub hard_deleted: usize,
    /// Number of artifact bodies moved to cold/archive storage.
    pub archived: usize,
    /// Number of artifacts skipped due to active parent references or errors.
    pub skipped: usize,
    /// Total bytes freed by body removal (soft-delete bodies + hard-delete rows).
    pub bytes_freed: u64,
    /// Diagnostic events removed during this sweep.
    pub events_deleted: usize,
    /// Errors encountered; non-fatal (sweep continues after recording them).
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// DiagnosticEvent (minimal representation used by retention)
// ---------------------------------------------------------------------------

/// A diagnostic event record as seen by the retention enforcer.
///
/// In a real implementation this would come from an event store; here we
/// define the minimal shape needed for retention logic.
#[derive(Debug, Clone)]
pub struct DiagnosticEvent {
    /// Unique identifier for the event.
    pub id: String,
    /// When this event expires (must be present for diagnostic events).
    pub expires_at: DateTime<Utc>,
    /// Body size in bytes, used for byte accounting.
    pub size_bytes: u64,
}

// ---------------------------------------------------------------------------
// ArtifactState
// ---------------------------------------------------------------------------

/// Lifecycle state of a stored artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactState {
    /// Active: body present, not deleted.
    Active,
    /// Soft-deleted: metadata retained, body removed.
    SoftDeleted {
        /// When the soft-delete occurred.
        deleted_at: DateTime<Utc>,
    },
    /// Archived: body moved to cold storage.
    Archived,
    /// Hard-deleted: metadata row removed from primary store.
    HardDeleted,
}

// ---------------------------------------------------------------------------
// RetentionStore trait
// ---------------------------------------------------------------------------

/// Extension of [`ArtifactStore`] with lifecycle-mutation operations required
/// by the retention enforcer.
///
/// Implementors must also implement `ArtifactStore`; the two traits are kept
/// separate so that read-only stores do not need to implement retention methods.
pub trait RetentionStore: ArtifactStore {
    /// Return all artifact metadata records currently in the store, regardless
    /// of lifecycle state.
    fn list_all(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<(Artifact, ArtifactState)>, StoreError>> + Send;

    /// Soft-delete an artifact: mark it as deleted and remove its body.
    ///
    /// After this call `get_body` must return `StoreError::NotFound` and
    /// `get` must still return the metadata row with `deleted_at` set.
    fn soft_delete(
        &self,
        id: ArtifactId,
    ) -> impl std::future::Future<Output = Result<(), StoreError>> + Send;

    /// Hard-delete an artifact: remove the metadata row entirely.
    ///
    /// After this call both `get` and `get_body` must return
    /// `StoreError::NotFound`.
    fn hard_delete(
        &self,
        id: ArtifactId,
    ) -> impl std::future::Future<Output = Result<(), StoreError>> + Send;

    /// Archive an artifact: move body to cold storage tier.
    ///
    /// Metadata remains queryable; body retrieval returns it from the archive
    /// tier.  This is a best-effort operation: implementations that lack cold
    /// storage may treat it as a no-op and return `Ok(())`.
    fn archive(
        &self,
        id: ArtifactId,
    ) -> impl std::future::Future<Output = Result<(), StoreError>> + Send;

    /// Return the current lifecycle state for an artifact.
    fn get_state(
        &self,
        id: ArtifactId,
    ) -> impl std::future::Future<Output = Result<ArtifactState, StoreError>> + Send;
}

// ---------------------------------------------------------------------------
// DiagnosticEventStore trait
// ---------------------------------------------------------------------------

/// Store port for diagnostic events, used by retention enforcement.
pub trait DiagnosticEventStore: Send + Sync + 'static {
    /// Return all diagnostic events whose `expires_at` is at or before `now`.
    fn list_expired(
        &self,
        now: DateTime<Utc>,
    ) -> impl std::future::Future<Output = Result<Vec<DiagnosticEvent>, StoreError>> + Send;

    /// Delete a diagnostic event by ID.
    fn delete_event(
        &self,
        id: &str,
    ) -> impl std::future::Future<Output = Result<(), StoreError>> + Send;
}

// ---------------------------------------------------------------------------
// RetentionEnforcer
// ---------------------------------------------------------------------------

/// Drives periodic retention enforcement sweeps over an artifact store and
/// an optional diagnostic-event store.
///
/// # Usage
///
/// ```ignore
/// use std::time::Duration;
/// use polkagent_artifact::retention::{RetentionEnforcer, RetentionPolicy};
/// use polkagent_artifact::retention::MemoryRetentionStore;
/// use std::sync::Arc;
///
/// let store = Arc::new(MemoryRetentionStore::new());
/// let policy = RetentionPolicy::new(
///     Duration::from_secs(90 * 24 * 3600),
///     Duration::from_secs(30 * 24 * 3600),
/// );
/// let enforcer = RetentionEnforcer::new();
/// let report = enforcer.enforce_artifact_retention(&*store, &policy).await?;
/// println!("soft-deleted: {}", report.soft_deleted);
/// ```
#[derive(Debug, Default, Clone)]
pub struct RetentionEnforcer;

impl RetentionEnforcer {
    /// Create a new enforcer.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Run a retention sweep over `store` using `policy`.
    ///
    /// The sweep:
    /// 1. Archives bodies that have exceeded `policy.archive_after` (if set).
    /// 2. Soft-deletes bodies that have exceeded `policy.soft_delete_after`,
    ///    **unless** the artifact is a parent of any non-deleted artifact
    ///    (protect-parents invariant).
    /// 3. Hard-deletes metadata rows that were soft-deleted more than
    ///    `policy.hard_delete_after` ago.
    ///
    /// Returns a [`RetentionReport`] summarising the work done.
    pub async fn enforce_artifact_retention<S: RetentionStore>(
        &self,
        store: &S,
        policy: &RetentionPolicy,
    ) -> Result<RetentionReport, StoreError> {
        let now = Utc::now();
        let mut report = RetentionReport::default();

        let all = store.list_all().await?;

        // Build a set of artifact IDs that are parents of non-deleted
        // artifacts so we can skip them.
        let protected_parents: HashSet<ArtifactId> = all
            .iter()
            .filter(|(_, state)| {
                !matches!(
                    state,
                    ArtifactState::SoftDeleted { .. } | ArtifactState::HardDeleted
                )
            })
            .flat_map(|(artifact, _)| artifact.parents.iter().copied())
            .collect();

        // Also build a child-lookup: for each artifact, collect its children.
        // We need this to detect whether a soft-deleted artifact still has
        // living children that reference it via ArtifactStore::get_lineage.
        // (parents field on Artifact records the direct parents.)
        let artifact_map: HashMap<ArtifactId, (Artifact, ArtifactState)> =
            all.into_iter().map(|(a, s)| (a.id, (a, s))).collect();

        for (id, (artifact, state)) in &artifact_map {
            match state {
                ArtifactState::Active => {
                    let age = now
                        .signed_duration_since(artifact.created_at)
                        .to_std()
                        .unwrap_or_default();

                    // Check archival first.
                    if let Some(archive_after) = policy.archive_after {
                        if age >= archive_after {
                            debug!(artifact_id = %id, "archiving artifact");
                            match store.archive(*id).await {
                                Ok(()) => {
                                    report.archived += 1;
                                    report.bytes_freed += artifact.blob_ref.size_bytes;
                                }
                                Err(e) => {
                                    warn!(artifact_id = %id, error = %e, "archive failed");
                                    report.errors.push(format!("archive {id}: {e}"));
                                    report.skipped += 1;
                                }
                            }
                            // Do not also soft-delete in the same sweep.
                            continue;
                        }
                    }

                    // Soft-delete if old enough and not a protected parent.
                    if age >= policy.soft_delete_after {
                        if protected_parents.contains(id) {
                            debug!(artifact_id = %id, "skipping soft-delete: protected parent");
                            report.skipped += 1;
                            continue;
                        }
                        debug!(artifact_id = %id, "soft-deleting artifact");
                        match store.soft_delete(*id).await {
                            Ok(()) => {
                                report.soft_deleted += 1;
                                report.bytes_freed += artifact.blob_ref.size_bytes;
                            }
                            Err(e) => {
                                warn!(artifact_id = %id, error = %e, "soft-delete failed");
                                report.errors.push(format!("soft_delete {id}: {e}"));
                                report.skipped += 1;
                            }
                        }
                    }
                }

                ArtifactState::SoftDeleted { deleted_at } => {
                    // Hard-delete if the grace period after soft-delete has elapsed.
                    let since_deletion = now
                        .signed_duration_since(*deleted_at)
                        .to_std()
                        .unwrap_or_default();

                    if since_deletion >= policy.hard_delete_after {
                        // Still protect parents even during hard-delete.
                        if protected_parents.contains(id) {
                            debug!(artifact_id = %id, "skipping hard-delete: protected parent");
                            report.skipped += 1;
                            continue;
                        }
                        debug!(artifact_id = %id, "hard-deleting artifact");
                        match store.hard_delete(*id).await {
                            Ok(()) => {
                                report.hard_deleted += 1;
                                // Metadata size is small; we account only for
                                // body bytes which were freed at soft-delete time.
                            }
                            Err(e) => {
                                warn!(artifact_id = %id, error = %e, "hard-delete failed");
                                report.errors.push(format!("hard_delete {id}: {e}"));
                                report.skipped += 1;
                            }
                        }
                    }
                }

                ArtifactState::Archived => {
                    // Archived artifacts may still be eligible for soft-delete.
                    let age = now
                        .signed_duration_since(artifact.created_at)
                        .to_std()
                        .unwrap_or_default();
                    if age >= policy.soft_delete_after && !protected_parents.contains(id) {
                        debug!(artifact_id = %id, "soft-deleting archived artifact");
                        match store.soft_delete(*id).await {
                            Ok(()) => {
                                report.soft_deleted += 1;
                                // Body was already in cold storage; accounting
                                // for it here would double-count, so we skip it.
                            }
                            Err(e) => {
                                warn!(artifact_id = %id, error = %e, "soft-delete of archived artifact failed");
                                report
                                    .errors
                                    .push(format!("soft_delete_archived {id}: {e}"));
                                report.skipped += 1;
                            }
                        }
                    }
                }

                ArtifactState::HardDeleted => {
                    // Nothing to do; already gone.
                }
            }
        }

        info!(
            soft_deleted = report.soft_deleted,
            hard_deleted = report.hard_deleted,
            archived = report.archived,
            skipped = report.skipped,
            bytes_freed = report.bytes_freed,
            errors = report.errors.len(),
            "retention sweep complete"
        );

        Ok(report)
    }

    /// Delete all diagnostic events whose `expires_at` has passed.
    ///
    /// Returns a [`RetentionReport`] with only `events_deleted` and `errors`
    /// populated.
    pub async fn enforce_event_retention<E: DiagnosticEventStore>(
        &self,
        event_store: &E,
    ) -> Result<RetentionReport, StoreError> {
        let now = Utc::now();
        let mut report = RetentionReport::default();

        let expired = event_store.list_expired(now).await?;
        for event in expired {
            debug!(event_id = %event.id, expires_at = %event.expires_at, "deleting expired diagnostic event");
            match event_store.delete_event(&event.id).await {
                Ok(()) => {
                    report.events_deleted += 1;
                    report.bytes_freed += event.size_bytes;
                }
                Err(e) => {
                    warn!(event_id = %event.id, error = %e, "event deletion failed");
                    report
                        .errors
                        .push(format!("delete_event {}: {e}", event.id));
                }
            }
        }

        info!(
            events_deleted = report.events_deleted,
            bytes_freed = report.bytes_freed,
            errors = report.errors.len(),
            "event retention sweep complete"
        );

        Ok(report)
    }
}

// ---------------------------------------------------------------------------
// MemoryRetentionStore
// ---------------------------------------------------------------------------

/// An in-memory [`RetentionStore`] suitable for tests and examples.
///
/// Extends [`crate::memory::MemoryStore`]'s semantics with soft-delete,
/// hard-delete, archive, and `list_all` operations backed by a separate
/// lifecycle-state table.
#[derive(Debug, Default, Clone)]
pub struct MemoryRetentionStore {
    inner: Arc<RwLock<RetentionInner>>,
}

#[derive(Debug, Default)]
struct RetentionInner {
    artifacts: HashMap<ArtifactId, Artifact>,
    bodies: HashMap<ArtifactId, Vec<u8>>,
    lineage: HashMap<ArtifactId, Vec<ArtifactId>>,
    states: HashMap<ArtifactId, ArtifactState>,
}

impl MemoryRetentionStore {
    /// Create a new, empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ArtifactStore for MemoryRetentionStore {
    async fn store(&self, artifact: &Artifact, body: &[u8]) -> Result<(), StoreError> {
        let mut g = self.inner.write().await;
        g.artifacts.insert(artifact.id, artifact.clone());
        g.bodies.insert(artifact.id, body.to_vec());
        g.states.insert(artifact.id, ArtifactState::Active);
        Ok(())
    }

    async fn get(&self, id: ArtifactId) -> Result<Artifact, StoreError> {
        let g = self.inner.read().await;
        g.artifacts
            .get(&id)
            .cloned()
            .ok_or(StoreError::NotFound(id))
    }

    async fn get_body(&self, id: ArtifactId) -> Result<Vec<u8>, StoreError> {
        let g = self.inner.read().await;
        let artifact = g.artifacts.get(&id).ok_or(StoreError::NotFound(id))?;
        let body = g.bodies.get(&id).ok_or(StoreError::NotFound(id))?.clone();
        if !crate::digest::verify_digest(&body, &artifact.blob_ref) {
            return Err(StoreError::DigestMismatch(id));
        }
        Ok(body)
    }

    async fn verify(&self, id: ArtifactId) -> Result<bool, StoreError> {
        let g = self.inner.read().await;
        let Some(artifact) = g.artifacts.get(&id) else {
            return Ok(false);
        };
        let Some(body) = g.bodies.get(&id) else {
            return Ok(false);
        };
        Ok(crate::digest::verify_digest(body, &artifact.blob_ref))
    }

    async fn list_for_run(
        &self,
        run_id: polkagent_core::ids::RunId,
    ) -> Result<Vec<Artifact>, StoreError> {
        let g = self.inner.read().await;
        let mut results: Vec<Artifact> = g
            .artifacts
            .values()
            .filter(|a| a.run_id == Some(run_id))
            .cloned()
            .collect();
        results.sort_by_key(|a| a.created_at);
        Ok(results)
    }

    async fn add_lineage(
        &self,
        child_id: ArtifactId,
        parent_id: ArtifactId,
    ) -> Result<(), StoreError> {
        let mut g = self.inner.write().await;
        let parents = g.lineage.entry(child_id).or_default();
        if !parents.contains(&parent_id) {
            parents.push(parent_id);
        }
        Ok(())
    }

    async fn get_lineage(&self, id: ArtifactId) -> Result<Vec<ArtifactId>, StoreError> {
        use std::collections::{HashSet, VecDeque};
        let g = self.inner.read().await;
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        visited.insert(id);
        queue.push_back(id);
        while let Some(current) = queue.pop_front() {
            if let Some(parents) = g.lineage.get(&current) {
                for &parent in parents {
                    if visited.insert(parent) {
                        result.push(parent);
                        queue.push_back(parent);
                    }
                }
            }
        }
        Ok(result)
    }
}

impl RetentionStore for MemoryRetentionStore {
    async fn list_all(&self) -> Result<Vec<(Artifact, ArtifactState)>, StoreError> {
        let g = self.inner.read().await;
        Ok(g.artifacts
            .values()
            .map(|a| {
                let state = g
                    .states
                    .get(&a.id)
                    .cloned()
                    .unwrap_or(ArtifactState::Active);
                (a.clone(), state)
            })
            .collect())
    }

    async fn soft_delete(&self, id: ArtifactId) -> Result<(), StoreError> {
        let mut g = self.inner.write().await;
        if !g.artifacts.contains_key(&id) {
            return Err(StoreError::NotFound(id));
        }
        g.bodies.remove(&id);
        g.states.insert(
            id,
            ArtifactState::SoftDeleted {
                deleted_at: Utc::now(),
            },
        );
        Ok(())
    }

    async fn hard_delete(&self, id: ArtifactId) -> Result<(), StoreError> {
        let mut g = self.inner.write().await;
        if !g.artifacts.contains_key(&id) {
            return Err(StoreError::NotFound(id));
        }
        g.artifacts.remove(&id);
        g.bodies.remove(&id);
        g.states.insert(id, ArtifactState::HardDeleted);
        Ok(())
    }

    async fn archive(&self, id: ArtifactId) -> Result<(), StoreError> {
        let mut g = self.inner.write().await;
        if !g.artifacts.contains_key(&id) {
            return Err(StoreError::NotFound(id));
        }
        // In-memory: simply mark as archived; body stays in RAM.
        g.states.insert(id, ArtifactState::Archived);
        Ok(())
    }

    async fn get_state(&self, id: ArtifactId) -> Result<ArtifactState, StoreError> {
        let g = self.inner.read().await;
        g.states.get(&id).cloned().ok_or(StoreError::NotFound(id))
    }
}

// ---------------------------------------------------------------------------
// MemoryDiagnosticEventStore
// ---------------------------------------------------------------------------

/// An in-memory [`DiagnosticEventStore`] for tests.
#[derive(Debug, Default, Clone)]
pub struct MemoryDiagnosticEventStore {
    inner: Arc<RwLock<HashMap<String, DiagnosticEvent>>>,
}

impl MemoryDiagnosticEventStore {
    /// Create a new, empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a diagnostic event.
    pub async fn insert(&self, event: DiagnosticEvent) {
        let mut g = self.inner.write().await;
        g.insert(event.id.clone(), event);
    }

    /// Return the number of events currently held.
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Return `true` if the store is empty.
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }
}

impl DiagnosticEventStore for MemoryDiagnosticEventStore {
    async fn list_expired(&self, now: DateTime<Utc>) -> Result<Vec<DiagnosticEvent>, StoreError> {
        let g = self.inner.read().await;
        Ok(g.values()
            .filter(|e| e.expires_at <= now)
            .cloned()
            .collect())
    }

    async fn delete_event(&self, id: &str) -> Result<(), StoreError> {
        let mut g = self.inner.write().await;
        g.remove(id);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::compute_digest;
    use chrono::Duration as ChronoDuration;
    use polkagent_core::artifact::ArtifactKind;
    use polkagent_core::config::DataClassification;
    use std::collections::HashMap;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_artifact_at(body: &[u8], created_at: DateTime<Utc>) -> (Artifact, Vec<u8>) {
        let blob_ref = compute_digest(body);
        let artifact = Artifact {
            id: ArtifactId::new(),
            run_id: None,
            step_id: None,
            kind: ArtifactKind::Custom {
                type_uri: "test://retention".into(),
            },
            blob_ref,
            classification: DataClassification::default(),
            parents: Vec::new(),
            created_at,
            metadata: HashMap::new(),
        };
        (artifact, body.to_vec())
    }

    /// Make an artifact that appears to be `days_old` days old.
    fn make_old_artifact(body: &[u8], days_old: i64) -> (Artifact, Vec<u8>) {
        let created_at = Utc::now() - ChronoDuration::days(days_old);
        make_artifact_at(body, created_at)
    }

    /// A policy that soft-deletes after 30 days and hard-deletes 7 days later.
    fn default_policy() -> RetentionPolicy {
        RetentionPolicy::new(
            Duration::from_secs(30 * 24 * 3600),
            Duration::from_secs(7 * 24 * 3600),
        )
    }

    // -----------------------------------------------------------------------
    // RetentionPolicy
    // -----------------------------------------------------------------------

    #[test]
    fn retention_policy_new_has_no_archive() {
        let policy = RetentionPolicy::new(Duration::from_secs(1), Duration::from_secs(2));
        assert!(policy.archive_after.is_none());
    }

    #[test]
    fn retention_policy_with_archive_stores_duration() {
        let policy = RetentionPolicy::with_archive(
            Duration::from_secs(90 * 86400),
            Duration::from_secs(30 * 86400),
            Duration::from_secs(60 * 86400),
        );
        assert_eq!(policy.archive_after, Some(Duration::from_secs(60 * 86400)));
    }

    // -----------------------------------------------------------------------
    // ArtifactStore basics via MemoryRetentionStore
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn store_and_get_round_trip() {
        let store = MemoryRetentionStore::new();
        let (artifact, body) = make_old_artifact(b"hello", 0);
        let id = artifact.id;
        store.store(&artifact, &body).await.unwrap();
        let fetched = store.get(id).await.unwrap();
        assert_eq!(fetched.id, id);
    }

    #[tokio::test]
    async fn initial_state_is_active() {
        let store = MemoryRetentionStore::new();
        let (artifact, body) = make_old_artifact(b"active", 0);
        let id = artifact.id;
        store.store(&artifact, &body).await.unwrap();
        let state = store.get_state(id).await.unwrap();
        assert_eq!(state, ArtifactState::Active);
    }

    // -----------------------------------------------------------------------
    // Soft-delete
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn soft_delete_removes_body() {
        let store = MemoryRetentionStore::new();
        let (artifact, body) = make_old_artifact(b"to be soft deleted", 0);
        let id = artifact.id;
        store.store(&artifact, &body).await.unwrap();

        store.soft_delete(id).await.unwrap();

        // Body should be gone.
        let err = store.get_body(id).await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
        // Metadata still present.
        let meta = store.get(id).await.unwrap();
        assert_eq!(meta.id, id);
        // State is SoftDeleted.
        let state = store.get_state(id).await.unwrap();
        assert!(matches!(state, ArtifactState::SoftDeleted { .. }));
    }

    #[tokio::test]
    async fn soft_delete_nonexistent_returns_not_found() {
        let store = MemoryRetentionStore::new();
        let id = ArtifactId::new();
        let err = store.soft_delete(id).await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
    }

    // -----------------------------------------------------------------------
    // Hard-delete
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn hard_delete_removes_metadata() {
        let store = MemoryRetentionStore::new();
        let (artifact, body) = make_old_artifact(b"to be hard deleted", 0);
        let id = artifact.id;
        store.store(&artifact, &body).await.unwrap();

        store.hard_delete(id).await.unwrap();

        let err = store.get(id).await.unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
        let state = store.get_state(id).await.unwrap();
        assert_eq!(state, ArtifactState::HardDeleted);
    }

    // -----------------------------------------------------------------------
    // Archive
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn archive_marks_state() {
        let store = MemoryRetentionStore::new();
        let (artifact, body) = make_old_artifact(b"archive me", 0);
        let id = artifact.id;
        store.store(&artifact, &body).await.unwrap();

        store.archive(id).await.unwrap();

        let state = store.get_state(id).await.unwrap();
        assert_eq!(state, ArtifactState::Archived);
    }

    // -----------------------------------------------------------------------
    // Enforce artifact retention — no expiry
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn retention_no_op_for_fresh_artifacts() {
        let store = MemoryRetentionStore::new();
        let (artifact, body) = make_old_artifact(b"fresh", 0);
        store.store(&artifact, &body).await.unwrap();

        let enforcer = RetentionEnforcer::new();
        let policy = default_policy();
        let report = enforcer
            .enforce_artifact_retention(&store, &policy)
            .await
            .unwrap();

        assert_eq!(report.soft_deleted, 0);
        assert_eq!(report.hard_deleted, 0);
        assert_eq!(report.archived, 0);
        assert_eq!(report.skipped, 0);
        assert_eq!(report.bytes_freed, 0);
        assert!(report.errors.is_empty());
    }

    // -----------------------------------------------------------------------
    // Enforce artifact retention — soft-delete on expiry
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn retention_soft_deletes_expired_artifact() {
        let store = MemoryRetentionStore::new();
        let (artifact, body) = make_old_artifact(b"old enough", 31);
        let size = artifact.blob_ref.size_bytes;
        store.store(&artifact, &body).await.unwrap();

        let enforcer = RetentionEnforcer::new();
        let policy = default_policy(); // soft-delete after 30 days
        let report = enforcer
            .enforce_artifact_retention(&store, &policy)
            .await
            .unwrap();

        assert_eq!(report.soft_deleted, 1);
        assert_eq!(report.bytes_freed, size);
        assert!(report.errors.is_empty());
    }

    // -----------------------------------------------------------------------
    // Enforce artifact retention — protect parents
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn retention_skips_parent_artifact_with_active_child() {
        let store = MemoryRetentionStore::new();

        // Parent created 31 days ago (expired).
        let (parent, parent_body) = make_old_artifact(b"parent data", 31);
        let parent_id = parent.id;
        store.store(&parent, &parent_body).await.unwrap();

        // Child references parent and is still fresh.
        let blob_ref = compute_digest(b"child data");
        let child = Artifact {
            id: ArtifactId::new(),
            run_id: None,
            step_id: None,
            kind: ArtifactKind::Custom {
                type_uri: "test://retention".into(),
            },
            blob_ref,
            classification: DataClassification::default(),
            parents: vec![parent_id],
            created_at: Utc::now(),
            metadata: HashMap::new(),
        };
        store.store(&child, b"child data").await.unwrap();

        let enforcer = RetentionEnforcer::new();
        let policy = default_policy();
        let report = enforcer
            .enforce_artifact_retention(&store, &policy)
            .await
            .unwrap();

        // Parent must be skipped because child is active.
        assert_eq!(report.soft_deleted, 0, "parent must be protected");
        assert_eq!(report.skipped, 1);

        // Parent state must remain Active.
        let state = store.get_state(parent_id).await.unwrap();
        assert_eq!(state, ArtifactState::Active);
    }

    // -----------------------------------------------------------------------
    // Enforce artifact retention — hard-delete after grace period
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn retention_hard_deletes_after_grace_period() {
        let store = MemoryRetentionStore::new();
        let (artifact, body) = make_old_artifact(b"to be hard deleted", 0);
        let id = artifact.id;
        store.store(&artifact, &body).await.unwrap();

        // Manually put the artifact into SoftDeleted state with a deleted_at
        // 8 days ago (exceeds 7-day hard-delete grace).
        {
            let mut g = store.inner.write().await;
            g.states.insert(
                id,
                ArtifactState::SoftDeleted {
                    deleted_at: Utc::now() - ChronoDuration::days(8),
                },
            );
            g.bodies.remove(&id);
        }

        let enforcer = RetentionEnforcer::new();
        let policy = default_policy(); // hard-delete after 7 days
        let report = enforcer
            .enforce_artifact_retention(&store, &policy)
            .await
            .unwrap();

        assert_eq!(report.hard_deleted, 1);
        let state = store.get_state(id).await.unwrap();
        assert_eq!(state, ArtifactState::HardDeleted);
    }

    // -----------------------------------------------------------------------
    // Enforce artifact retention — archival
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn retention_archives_artifact_before_soft_delete() {
        let store = MemoryRetentionStore::new();
        // Artifact is 61 days old. Policy: archive after 60 days, soft-delete after 90.
        let (artifact, body) = make_old_artifact(b"archive candidate", 61);
        let size = artifact.blob_ref.size_bytes;
        store.store(&artifact, &body).await.unwrap();

        let policy = RetentionPolicy::with_archive(
            Duration::from_secs(90 * 86400),
            Duration::from_secs(30 * 86400),
            Duration::from_secs(60 * 86400),
        );

        let enforcer = RetentionEnforcer::new();
        let report = enforcer
            .enforce_artifact_retention(&store, &policy)
            .await
            .unwrap();

        assert_eq!(report.archived, 1);
        assert_eq!(report.bytes_freed, size);
        assert_eq!(report.soft_deleted, 0);
    }

    // -----------------------------------------------------------------------
    // Multiple artifacts, mixed ages
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn retention_processes_multiple_artifacts_correctly() {
        let store = MemoryRetentionStore::new();

        // Fresh artifact — should not be touched.
        let (fresh, fresh_body) = make_old_artifact(b"fresh", 1);
        store.store(&fresh, &fresh_body).await.unwrap();

        // Old artifact — should be soft-deleted.
        let (old, old_body) = make_old_artifact(b"old content data", 35);
        let old_id = old.id;
        store.store(&old, &old_body).await.unwrap();

        let policy = default_policy();
        let enforcer = RetentionEnforcer::new();
        let report = enforcer
            .enforce_artifact_retention(&store, &policy)
            .await
            .unwrap();

        assert_eq!(report.soft_deleted, 1);
        assert_eq!(report.skipped, 0);
        let old_state = store.get_state(old_id).await.unwrap();
        assert!(matches!(old_state, ArtifactState::SoftDeleted { .. }));
        let fresh_state = store.get_state(fresh.id).await.unwrap();
        assert_eq!(fresh_state, ArtifactState::Active);
    }

    // -----------------------------------------------------------------------
    // Event retention
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn event_retention_removes_expired_events() {
        let event_store = MemoryDiagnosticEventStore::new();

        let expired = DiagnosticEvent {
            id: "evt-expired".into(),
            expires_at: Utc::now() - ChronoDuration::seconds(1),
            size_bytes: 128,
        };
        let active = DiagnosticEvent {
            id: "evt-active".into(),
            expires_at: Utc::now() + ChronoDuration::hours(1),
            size_bytes: 64,
        };
        event_store.insert(expired).await;
        event_store.insert(active).await;

        let enforcer = RetentionEnforcer::new();
        let report = enforcer
            .enforce_event_retention(&event_store)
            .await
            .unwrap();

        assert_eq!(report.events_deleted, 1);
        assert_eq!(report.bytes_freed, 128);
        assert_eq!(event_store.len().await, 1);
        assert!(report.errors.is_empty());
    }

    #[tokio::test]
    async fn event_retention_no_op_when_nothing_expired() {
        let event_store = MemoryDiagnosticEventStore::new();
        event_store
            .insert(DiagnosticEvent {
                id: "evt-1".into(),
                expires_at: Utc::now() + ChronoDuration::hours(24),
                size_bytes: 100,
            })
            .await;

        let enforcer = RetentionEnforcer::new();
        let report = enforcer
            .enforce_event_retention(&event_store)
            .await
            .unwrap();

        assert_eq!(report.events_deleted, 0);
        assert_eq!(report.bytes_freed, 0);
    }

    #[tokio::test]
    async fn event_retention_empty_store_is_ok() {
        let event_store = MemoryDiagnosticEventStore::new();
        let enforcer = RetentionEnforcer::new();
        let report = enforcer
            .enforce_event_retention(&event_store)
            .await
            .unwrap();
        assert_eq!(report.events_deleted, 0);
        assert!(report.errors.is_empty());
    }

    // -----------------------------------------------------------------------
    // list_all includes all states
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_all_returns_all_states() {
        let store = MemoryRetentionStore::new();
        let (a1, b1) = make_old_artifact(b"a1", 0);
        let (a2, b2) = make_old_artifact(b"a2", 0);
        store.store(&a1, &b1).await.unwrap();
        store.store(&a2, &b2).await.unwrap();
        store.soft_delete(a2.id).await.unwrap();

        let all = store.list_all().await.unwrap();
        assert_eq!(all.len(), 2);
        let active = all
            .iter()
            .filter(|(_, s)| *s == ArtifactState::Active)
            .count();
        let soft_deleted = all
            .iter()
            .filter(|(_, s)| matches!(s, ArtifactState::SoftDeleted { .. }))
            .count();
        assert_eq!(active, 1);
        assert_eq!(soft_deleted, 1);
    }

    // -----------------------------------------------------------------------
    // RetentionReport defaults
    // -----------------------------------------------------------------------

    #[test]
    fn retention_report_default_all_zeros() {
        let r = RetentionReport::default();
        assert_eq!(r.soft_deleted, 0);
        assert_eq!(r.hard_deleted, 0);
        assert_eq!(r.archived, 0);
        assert_eq!(r.skipped, 0);
        assert_eq!(r.bytes_freed, 0);
        assert_eq!(r.events_deleted, 0);
        assert!(r.errors.is_empty());
    }

    // -----------------------------------------------------------------------
    // Bytes freed accounting
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn bytes_freed_accounts_for_body_size() {
        let store = MemoryRetentionStore::new();
        let body = b"exactly twenty four bytes!!".as_ref();
        let (artifact, body_bytes) = make_old_artifact(body, 35);
        let expected_bytes = artifact.blob_ref.size_bytes;
        store.store(&artifact, &body_bytes).await.unwrap();

        let policy = default_policy();
        let enforcer = RetentionEnforcer::new();
        let report = enforcer
            .enforce_artifact_retention(&store, &policy)
            .await
            .unwrap();

        assert_eq!(report.bytes_freed, expected_bytes);
    }

    // -----------------------------------------------------------------------
    // Chained lineage: parent of parent protected
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn retention_protects_grandparent_when_child_is_active() {
        let store = MemoryRetentionStore::new();

        // Grandparent: 31 days old (expired)
        let (grandparent, gp_body) = make_old_artifact(b"grandparent", 31);
        let gp_id = grandparent.id;
        store.store(&grandparent, &gp_body).await.unwrap();

        // Parent: 31 days old (expired), references grandparent
        let gp_blob = compute_digest(b"parent body");
        let parent = Artifact {
            id: ArtifactId::new(),
            run_id: None,
            step_id: None,
            kind: ArtifactKind::Custom {
                type_uri: "test://retention".into(),
            },
            blob_ref: gp_blob,
            classification: DataClassification::default(),
            parents: vec![gp_id],
            created_at: Utc::now() - ChronoDuration::days(31),
            metadata: HashMap::new(),
        };
        let parent_id = parent.id;
        store.store(&parent, b"parent body").await.unwrap();

        // Child: fresh, references parent
        let child_blob = compute_digest(b"child body");
        let child = Artifact {
            id: ArtifactId::new(),
            run_id: None,
            step_id: None,
            kind: ArtifactKind::Custom {
                type_uri: "test://retention".into(),
            },
            blob_ref: child_blob,
            classification: DataClassification::default(),
            parents: vec![parent_id],
            created_at: Utc::now(),
            metadata: HashMap::new(),
        };
        store.store(&child, b"child body").await.unwrap();

        let enforcer = RetentionEnforcer::new();
        let policy = default_policy();
        let report = enforcer
            .enforce_artifact_retention(&store, &policy)
            .await
            .unwrap();

        // Both grandparent and parent must be skipped.
        assert_eq!(report.soft_deleted, 0);
        assert_eq!(report.skipped, 2);
        assert_eq!(store.get_state(gp_id).await.unwrap(), ArtifactState::Active);
        assert_eq!(
            store.get_state(parent_id).await.unwrap(),
            ArtifactState::Active
        );
    }

    // -----------------------------------------------------------------------
    // Multiple expired events in one sweep
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn event_retention_removes_multiple_expired() {
        let event_store = MemoryDiagnosticEventStore::new();
        for i in 0..5_u32 {
            event_store
                .insert(DiagnosticEvent {
                    id: format!("evt-{i}"),
                    expires_at: Utc::now() - ChronoDuration::seconds(1),
                    size_bytes: 10,
                })
                .await;
        }
        // One future event.
        event_store
            .insert(DiagnosticEvent {
                id: "evt-future".into(),
                expires_at: Utc::now() + ChronoDuration::hours(1),
                size_bytes: 10,
            })
            .await;

        let enforcer = RetentionEnforcer::new();
        let report = enforcer
            .enforce_event_retention(&event_store)
            .await
            .unwrap();

        assert_eq!(report.events_deleted, 5);
        assert_eq!(report.bytes_freed, 50);
        assert_eq!(event_store.len().await, 1);
    }
}

//! An in-memory [`ArtifactStore`] implementation used in tests and examples.
//!
//! `MemoryStore` is deliberately simple: it uses a `tokio::sync::RwLock` to
//! serialize concurrent access to three `HashMap` tables:
//!
//! - `artifacts`: ArtifactId → Artifact metadata
//! - `bodies`: ArtifactId → raw body bytes
//! - `lineage`: ArtifactId → Vec<ArtifactId> (parent list, insertion order)
//!
//! **Do not use in production.**  There is no persistence, no size limit, and
//! no support for large bodies.

use std::collections::HashMap;
use std::sync::Arc;

use polkagent_core::artifact::Artifact;
use polkagent_core::ids::{ArtifactId, RunId};
use tokio::sync::RwLock;

use crate::digest::verify_digest;
use crate::store::{ArtifactStore, StoreError};

// ---------------------------------------------------------------------------
// MemoryStore
// ---------------------------------------------------------------------------

/// Shared inner state.
#[derive(Debug, Default)]
struct Inner {
    artifacts: HashMap<ArtifactId, Artifact>,
    bodies: HashMap<ArtifactId, Vec<u8>>,
    /// child → list of direct parents (insertion order preserved)
    lineage: HashMap<ArtifactId, Vec<ArtifactId>>,
}

/// Thread-safe, heap-backed artifact store.
#[derive(Debug, Default, Clone)]
pub struct MemoryStore {
    inner: Arc<RwLock<Inner>>,
}

impl MemoryStore {
    /// Create a new, empty [`MemoryStore`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ArtifactStore for MemoryStore {
    async fn store(&self, artifact: &Artifact, body: &[u8]) -> Result<(), StoreError> {
        let mut guard = self.inner.write().await;
        guard.artifacts.insert(artifact.id, artifact.clone());
        guard.bodies.insert(artifact.id, body.to_vec());
        Ok(())
    }

    async fn get(&self, id: ArtifactId) -> Result<Artifact, StoreError> {
        let guard = self.inner.read().await;
        guard
            .artifacts
            .get(&id)
            .cloned()
            .ok_or(StoreError::NotFound(id))
    }

    async fn get_body(&self, id: ArtifactId) -> Result<Vec<u8>, StoreError> {
        let guard = self.inner.read().await;

        let artifact = guard
            .artifacts
            .get(&id)
            .ok_or(StoreError::NotFound(id))?;

        let body = guard
            .bodies
            .get(&id)
            .ok_or(StoreError::NotFound(id))?
            .clone();

        // Verify digest before returning.
        if !verify_digest(&body, &artifact.blob_ref) {
            return Err(StoreError::DigestMismatch(id));
        }

        Ok(body)
    }

    async fn verify(&self, id: ArtifactId) -> Result<bool, StoreError> {
        let guard = self.inner.read().await;

        let Some(artifact) = guard.artifacts.get(&id) else {
            return Ok(false);
        };

        let Some(body) = guard.bodies.get(&id) else {
            return Ok(false);
        };

        Ok(verify_digest(body, &artifact.blob_ref))
    }

    async fn list_for_run(&self, run_id: RunId) -> Result<Vec<Artifact>, StoreError> {
        let guard = self.inner.read().await;
        let mut results: Vec<Artifact> = guard
            .artifacts
            .values()
            .filter(|a| a.run_id == Some(run_id))
            .cloned()
            .collect();
        // Sort by creation time (ascending).
        results.sort_by_key(|a| a.created_at);
        Ok(results)
    }

    async fn add_lineage(
        &self,
        child_id: ArtifactId,
        parent_id: ArtifactId,
    ) -> Result<(), StoreError> {
        let mut guard = self.inner.write().await;
        let parents = guard.lineage.entry(child_id).or_default();
        if !parents.contains(&parent_id) {
            parents.push(parent_id);
        }
        Ok(())
    }

    async fn get_lineage(&self, id: ArtifactId) -> Result<Vec<ArtifactId>, StoreError> {
        // BFS ancestor traversal over the in-memory lineage map.
        use std::collections::{HashSet, VecDeque};

        let guard = self.inner.read().await;

        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        visited.insert(id);
        queue.push_back(id);

        while let Some(current) = queue.pop_front() {
            if let Some(parents) = guard.lineage.get(&current) {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::compute_digest;
    use chrono::Utc;
    use polkagent_core::artifact::ArtifactKind;
    use polkagent_core::config::DataClassification;
    use std::collections::HashMap;

    fn make_artifact(body: &[u8]) -> (Artifact, Vec<u8>) {
        let blob_ref = compute_digest(body);
        let artifact = Artifact {
            id: ArtifactId::new(),
            run_id: None,
            step_id: None,
            kind: ArtifactKind::Custom {
                type_uri: "test://blob".into(),
            },
            blob_ref,
            classification: DataClassification::default(),
            parents: Vec::new(),
            created_at: Utc::now(),
            metadata: HashMap::new(),
        };
        (artifact, body.to_vec())
    }

    #[tokio::test]
    async fn store_and_get_round_trip() {
        let store = MemoryStore::new();
        let (artifact, body) = make_artifact(b"hello world");
        let id = artifact.id;

        store.store(&artifact, &body).await.expect("store");
        let fetched = store.get(id).await.expect("get");
        assert_eq!(fetched.id, id);
    }

    #[tokio::test]
    async fn get_body_verifies_digest() {
        let store = MemoryStore::new();
        let (artifact, body) = make_artifact(b"verify me");
        let id = artifact.id;
        store.store(&artifact, &body).await.expect("store");

        let fetched_body = store.get_body(id).await.expect("get_body");
        assert_eq!(fetched_body, body);
    }

    #[tokio::test]
    async fn get_body_detects_tampering() {
        let store = MemoryStore::new();
        let (artifact, _body) = make_artifact(b"original");
        let id = artifact.id;

        // Store with correct metadata but wrong body bytes.
        {
            let mut guard = store.inner.write().await;
            guard.artifacts.insert(id, artifact);
            guard.bodies.insert(id, b"tampered".to_vec());
        }

        let err = store.get_body(id).await.expect_err("should fail");
        assert!(
            matches!(err, StoreError::DigestMismatch(_)),
            "expected DigestMismatch, got {err:?}"
        );
    }

    #[tokio::test]
    async fn verify_returns_true_for_intact_artifact() {
        let store = MemoryStore::new();
        let (artifact, body) = make_artifact(b"intact");
        let id = artifact.id;
        store.store(&artifact, &body).await.expect("store");
        assert!(store.verify(id).await.expect("verify"));
    }

    #[tokio::test]
    async fn verify_returns_false_for_missing_artifact() {
        let store = MemoryStore::new();
        let id = ArtifactId::new();
        assert!(!store.verify(id).await.expect("verify"));
    }

    #[tokio::test]
    async fn list_for_run_returns_correct_artifacts() {
        let store = MemoryStore::new();
        let run_id = RunId::new();

        // Two artifacts for the target run.
        for content in [b"a" as &[u8], b"b"] {
            let blob_ref = compute_digest(content);
            let artifact = Artifact {
                id: ArtifactId::new(),
                run_id: Some(run_id),
                step_id: None,
                kind: ArtifactKind::Custom {
                    type_uri: "test://blob".into(),
                },
                blob_ref,
                classification: DataClassification::default(),
                parents: Vec::new(),
                created_at: Utc::now(),
                metadata: HashMap::new(),
            };
            store.store(&artifact, content).await.expect("store");
        }

        // One artifact for a different run.
        let other_run = RunId::new();
        let blob_ref = compute_digest(b"other");
        let other = Artifact {
            id: ArtifactId::new(),
            run_id: Some(other_run),
            step_id: None,
            kind: ArtifactKind::Custom {
                type_uri: "test://blob".into(),
            },
            blob_ref,
            classification: DataClassification::default(),
            parents: Vec::new(),
            created_at: Utc::now(),
            metadata: HashMap::new(),
        };
        store.store(&other, b"other").await.expect("store");

        let list = store.list_for_run(run_id).await.expect("list_for_run");
        assert_eq!(list.len(), 2);
        for a in &list {
            assert_eq!(a.run_id, Some(run_id));
        }
    }

    #[tokio::test]
    async fn lineage_add_and_get() {
        let store = MemoryStore::new();
        let root = ArtifactId::new();
        let child = ArtifactId::new();
        let grandchild = ArtifactId::new();

        store.add_lineage(child, root).await.expect("add_lineage");
        store.add_lineage(grandchild, child).await.expect("add_lineage");

        let ancestors = store.get_lineage(grandchild).await.expect("get_lineage");
        assert!(ancestors.contains(&child));
        assert!(ancestors.contains(&root));
        assert_eq!(ancestors.len(), 2);
    }
}

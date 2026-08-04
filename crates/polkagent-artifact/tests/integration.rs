//! Integration tests for `polkagent-artifact`.
//!
//! Each test exercises the public API through [`ArtifactService`] backed by
//! the in-memory [`MemoryStore`], covering the scenarios required by the crate
//! specification:
//!
//! 1. Digest computation and verification via standalone utilities.
//! 2. Round-trip: create an artifact then retrieve its body with digest check.
//! 3. Tampered body detection (service-layer re-verification catches a
//!    misbehaving store that returns wrong bytes).
//! 4. Lineage chain traversal across 10 artifacts.
//! 5. Idempotent store: same content produces the same [`BlobRef`].

use std::collections::HashMap;
use std::sync::Arc;

use polkagent_artifact::{
    compute_digest, verify_digest, ArtifactError, ArtifactKind, ArtifactService, BlobRef,
    MemoryStore,
};
use polkagent_core::ids::{ArtifactId, RunId};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn svc() -> ArtifactService<MemoryStore> {
    ArtifactService::new(Arc::new(MemoryStore::new()))
}

fn blob_kind() -> ArtifactKind {
    ArtifactKind::Custom {
        type_uri: "test://blob".into(),
    }
}

fn text_kind() -> ArtifactKind {
    ArtifactKind::Custom {
        type_uri: "test://text".into(),
    }
}

// ---------------------------------------------------------------------------
// 1. Digest computation and verification
// ---------------------------------------------------------------------------

#[test]
fn digest_computation_is_deterministic() {
    let data = b"polkagent artifact body";
    let d1 = compute_digest(data);
    let d2 = compute_digest(data);
    assert_eq!(d1.blake3_hex, d2.blake3_hex);
}

#[test]
fn digest_verification_correct_body() {
    let data = b"correct body";
    let digest = compute_digest(data);
    assert!(verify_digest(data, &digest));
}

#[test]
fn digest_verification_wrong_body_returns_false() {
    let data = b"original body";
    let digest = compute_digest(data);
    assert!(!verify_digest(b"different body", &digest));
}

#[test]
fn blob_ref_display_via_blake3_hex_is_64_chars() {
    let digest = compute_digest(b"hex display");
    assert_eq!(digest.blake3_hex.len(), 64);
    assert!(digest
        .blake3_hex
        .chars()
        .all(|c| "0123456789abcdef".contains(c)));
}

// ---------------------------------------------------------------------------
// 2. Round-trip: create then retrieve with digest check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn round_trip_create_get_body() {
    let svc = svc();
    let body = b"round trip body content";

    let artifact = svc
        .create(None, text_kind(), body, HashMap::new())
        .await
        .expect("create");

    // blob_ref size matches.
    assert_eq!(artifact.blob_ref.size_bytes, body.len() as u64);

    // Metadata retrieval.
    let fetched = svc.get(artifact.id).await.expect("get");
    assert_eq!(fetched.id, artifact.id);
    assert_eq!(fetched.blob_ref.blake3_hex, artifact.blob_ref.blake3_hex);

    // Body retrieval with digest verification.
    let fetched_body = svc.get_body(artifact.id).await.expect("get_body");
    assert_eq!(fetched_body.as_slice(), body.as_slice());
}

#[tokio::test]
async fn round_trip_with_run_id_and_metadata() {
    let svc = svc();
    let run_id = RunId::new();
    let body = b"metadata body";
    let mut meta = HashMap::new();
    meta.insert("source".to_string(), "test".to_string());

    let artifact = svc
        .create(Some(run_id), blob_kind(), body, meta.clone())
        .await
        .expect("create");

    assert_eq!(artifact.run_id, Some(run_id));
    assert_eq!(artifact.metadata.get("source"), meta.get("source"));
}

// ---------------------------------------------------------------------------
// 3. Tampered body detection
// ---------------------------------------------------------------------------

/// A [`ArtifactStore`] wrapper that returns wrong bytes for `get_body`.
///
/// This simulates a misbehaving or compromised backend.  The service MUST
/// catch the mismatch at its own layer even if the store does not.
mod tamper {
    use std::collections::HashMap;
    use std::sync::Arc;

    use polkagent_artifact::{
        ArtifactError, ArtifactKind, ArtifactService, ArtifactStore, MemoryStore, StoreError,
    };
    use polkagent_core::artifact::Artifact;
    use polkagent_core::ids::{ArtifactId, RunId};

    #[derive(Clone)]
    struct TamperingStore {
        inner: MemoryStore,
    }

    impl TamperingStore {
        fn new() -> Self {
            Self {
                inner: MemoryStore::new(),
            }
        }
    }

    impl ArtifactStore for TamperingStore {
        async fn store(&self, artifact: &Artifact, body: &[u8]) -> Result<(), StoreError> {
            self.inner.store(artifact, body).await
        }

        async fn get(&self, id: ArtifactId) -> Result<Artifact, StoreError> {
            self.inner.get(id).await
        }

        /// Always return garbage bytes regardless of what was stored.
        async fn get_body(&self, _id: ArtifactId) -> Result<Vec<u8>, StoreError> {
            Ok(b"this is not the original body".to_vec())
        }

        async fn verify(&self, id: ArtifactId) -> Result<bool, StoreError> {
            self.inner.verify(id).await
        }

        async fn list_for_run(&self, run_id: RunId) -> Result<Vec<Artifact>, StoreError> {
            self.inner.list_for_run(run_id).await
        }

        async fn add_lineage(
            &self,
            child_id: ArtifactId,
            parent_id: ArtifactId,
        ) -> Result<(), StoreError> {
            self.inner.add_lineage(child_id, parent_id).await
        }

        async fn get_lineage(&self, id: ArtifactId) -> Result<Vec<ArtifactId>, StoreError> {
            self.inner.get_lineage(id).await
        }
    }

    #[tokio::test]
    pub async fn tampered_body_is_caught_by_service() {
        let store = Arc::new(TamperingStore::new());
        let svc: ArtifactService<TamperingStore> = ArtifactService::new(store);

        let kind = ArtifactKind::Custom {
            type_uri: "test://blob".into(),
        };
        let original_body = b"original honest body";
        let artifact = svc
            .create(None, kind, original_body, HashMap::new())
            .await
            .expect("create");

        // The store will silently return tampered bytes.  The service re-checks.
        let err = svc
            .get_body(artifact.id)
            .await
            .expect_err("tampered body should be rejected");

        assert!(
            matches!(err, ArtifactError::DigestMismatch(_)),
            "expected DigestMismatch, got {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Lineage chain traversal: 10 artifacts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lineage_chain_10_artifacts() {
    let svc = svc();

    // Create 10 artifacts in sequence; each is a child of the previous.
    let mut ids: Vec<ArtifactId> = Vec::with_capacity(10);
    for i in 0u8..10 {
        let body = format!("artifact {i}").into_bytes();
        let artifact = svc
            .create(None, blob_kind(), &body, HashMap::new())
            .await
            .expect("create");
        ids.push(artifact.id);
    }

    // Wire up: ids[i+1] is derived from ids[i].
    for i in 0..9 {
        svc.add_lineage(ids[i + 1], ids[i])
            .await
            .expect("add_lineage");
    }

    // The ancestor chain of the last artifact should contain 9 ancestors.
    let ancestors = svc.get_lineage(ids[9]).await.expect("get_lineage");
    assert_eq!(ancestors.len(), 9, "last artifact should have 9 ancestors");

    // The direct parent should be first in the BFS result.
    assert_eq!(ancestors[0], ids[8]);

    // The root should be the last in the BFS result.
    assert_eq!(*ancestors.last().expect("non-empty"), ids[0]);

    // The root has no ancestors.
    let root_ancestors = svc.get_lineage(ids[0]).await.expect("get_lineage");
    assert!(root_ancestors.is_empty(), "root should have no ancestors");
}

// ---------------------------------------------------------------------------
// 5. Idempotent store: same content → same BlobRef
// ---------------------------------------------------------------------------

#[test]
fn same_content_produces_same_blob_ref() {
    let content = b"idempotent artifact content";
    let d1 = compute_digest(content);
    let d2 = compute_digest(content);
    assert_eq!(
        d1.blake3_hex, d2.blake3_hex,
        "same content must always produce the same BlobRef"
    );
}

#[tokio::test]
async fn two_creates_with_same_body_produce_same_body_ref() {
    let svc = svc();
    let body = b"shared content";

    let a1 = svc
        .create(None, blob_kind(), body, HashMap::new())
        .await
        .expect("create a1");
    let a2 = svc
        .create(None, blob_kind(), body, HashMap::new())
        .await
        .expect("create a2");

    // Different artifact IDs (UUIDs) but identical content addresses.
    assert_ne!(a1.id, a2.id, "each artifact must have a unique ID");
    assert_eq!(
        a1.blob_ref.blake3_hex, a2.blob_ref.blake3_hex,
        "same body must produce the same BlobRef"
    );
}

// ---------------------------------------------------------------------------
// 6. list_for_run
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_for_run_returns_only_relevant_artifacts() {
    let svc = svc();
    let run_a = RunId::new();
    let run_b = RunId::new();

    // Store 3 artifacts under run_a and 1 under run_b.
    for content in [b"x" as &[u8], b"y", b"z"] {
        svc.create(Some(run_a), blob_kind(), content, HashMap::new())
            .await
            .expect("create");
    }
    svc.create(Some(run_b), blob_kind(), b"other", HashMap::new())
        .await
        .expect("create");

    let run_a_artifacts = svc.list_for_run(run_a).await.expect("list_for_run");
    assert_eq!(run_a_artifacts.len(), 3);
    for a in &run_a_artifacts {
        assert_eq!(a.run_id, Some(run_a));
    }

    let run_b_artifacts = svc.list_for_run(run_b).await.expect("list_for_run");
    assert_eq!(run_b_artifacts.len(), 1);
}

// ---------------------------------------------------------------------------
// 7. verify
// ---------------------------------------------------------------------------

#[tokio::test]
async fn verify_returns_true_for_intact_artifact() {
    let svc = svc();
    let artifact = svc
        .create(None, text_kind(), b"verify me", HashMap::new())
        .await
        .expect("create");

    let ok = svc.verify(artifact.id).await.expect("verify");
    assert!(ok, "intact artifact must verify as true");
}

#[tokio::test]
async fn verify_returns_false_for_unknown_id() {
    let svc = svc();
    let unknown = ArtifactId::new();
    let ok = svc.verify(unknown).await.expect("verify");
    assert!(!ok, "unknown artifact must verify as false");
}

// ---------------------------------------------------------------------------
// 8. get on unknown ID returns NotFound
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_unknown_artifact_returns_not_found() {
    let svc = svc();
    let unknown = ArtifactId::new();
    let err = svc.get(unknown).await.expect_err("should be NotFound");
    assert!(
        matches!(err, ArtifactError::NotFound(_)),
        "expected NotFound, got {err:?}"
    );
}

#[tokio::test]
async fn get_body_unknown_artifact_returns_not_found() {
    let svc = svc();
    let unknown = ArtifactId::new();
    let err = svc.get_body(unknown).await.expect_err("should be NotFound");
    assert!(
        matches!(err, ArtifactError::NotFound(_)),
        "expected NotFound, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 9. Lineage idempotency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_lineage_edge_is_idempotent() {
    let svc = svc();

    let parent = svc
        .create(None, blob_kind(), b"parent", HashMap::new())
        .await
        .expect("create parent");
    let child = svc
        .create(None, blob_kind(), b"child", HashMap::new())
        .await
        .expect("create child");

    svc.add_lineage(child.id, parent.id)
        .await
        .expect("first add_lineage");
    svc.add_lineage(child.id, parent.id)
        .await
        .expect("second add_lineage");

    let ancestors = svc.get_lineage(child.id).await.expect("get_lineage");
    assert_eq!(
        ancestors.len(),
        1,
        "duplicate edges must not duplicate ancestors"
    );
    assert_eq!(ancestors[0], parent.id);
}

// ---------------------------------------------------------------------------
// 10. BlobRef serde round-trip
// ---------------------------------------------------------------------------

#[test]
fn blob_ref_serde_round_trip() {
    let original = compute_digest(b"serde test data");
    let json = serde_json::to_string(&original).expect("serialize");
    let back: BlobRef = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, back);
}

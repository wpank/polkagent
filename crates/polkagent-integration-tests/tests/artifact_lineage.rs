//! Artifact chain integration test.
//!
//! Creates artifacts for a run, establishes lineage (parent -> child),
//! queries ancestors and descendants, and verifies BLAKE3 digest integrity.

// This assertion-oriented integration target uses `expect`/`unwrap` to identify
// the exact cross-crate fixture step or behavioral contract that failed.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use polkagent_artifact::digest::{compute_digest, verify_digest};
use polkagent_artifact::memory::MemoryStore;
use polkagent_artifact::service::ArtifactService;
use polkagent_core::artifact::ArtifactKind;
use polkagent_core::ids::RunId;

// ---------------------------------------------------------------------------
// Create artifacts for a run and verify listing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_and_list_artifacts_for_run() {
    let store = Arc::new(MemoryStore::new());
    let svc = ArtifactService::new(store);
    let run_id = RunId::new();

    let a1 = svc
        .create(
            Some(run_id),
            ArtifactKind::Custom {
                type_uri: "test://prompt".into(),
            },
            b"user prompt text",
            HashMap::new(),
        )
        .await
        .expect("create a1");

    let a2 = svc
        .create(
            Some(run_id),
            ArtifactKind::Custom {
                type_uri: "test://response".into(),
            },
            b"model response text",
            HashMap::new(),
        )
        .await
        .expect("create a2");

    let artifacts = svc.list_for_run(run_id).await.expect("list_for_run");
    assert_eq!(artifacts.len(), 2);
    let ids: Vec<_> = artifacts.iter().map(|a| a.id).collect();
    assert!(ids.contains(&a1.id));
    assert!(ids.contains(&a2.id));
}

// ---------------------------------------------------------------------------
// Establish lineage and query ancestors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lineage_parent_child_grandchild() {
    let store = Arc::new(MemoryStore::new());
    let svc = ArtifactService::new(store);
    let run_id = RunId::new();

    // Create a lineage chain: root -> child -> grandchild.
    let root = svc
        .create(
            Some(run_id),
            ArtifactKind::Custom {
                type_uri: "test://root".into(),
            },
            b"root content",
            HashMap::new(),
        )
        .await
        .expect("create root");

    let child = svc
        .create(
            Some(run_id),
            ArtifactKind::Custom {
                type_uri: "test://child".into(),
            },
            b"child content",
            HashMap::new(),
        )
        .await
        .expect("create child");

    let grandchild = svc
        .create(
            Some(run_id),
            ArtifactKind::Custom {
                type_uri: "test://grandchild".into(),
            },
            b"grandchild content",
            HashMap::new(),
        )
        .await
        .expect("create grandchild");

    // Establish lineage edges.
    svc.add_lineage(child.id, root.id)
        .await
        .expect("add lineage child->root");
    svc.add_lineage(grandchild.id, child.id)
        .await
        .expect("add lineage grandchild->child");

    // Query ancestors of grandchild: should find both child and root.
    let ancestors = svc.get_lineage(grandchild.id).await.expect("get_lineage");
    assert_eq!(
        ancestors.len(),
        2,
        "grandchild should have 2 ancestors (child, root)"
    );
    assert!(ancestors.contains(&child.id));
    assert!(ancestors.contains(&root.id));

    // Query ancestors of child: should find only root.
    let child_ancestors = svc.get_lineage(child.id).await.expect("get_lineage child");
    assert_eq!(child_ancestors.len(), 1);
    assert!(child_ancestors.contains(&root.id));

    // Query ancestors of root: should find none.
    let root_ancestors = svc.get_lineage(root.id).await.expect("get_lineage root");
    assert!(root_ancestors.is_empty());
}

// ---------------------------------------------------------------------------
// BLAKE3 digest integrity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn digest_is_verified_on_retrieval() {
    let store = Arc::new(MemoryStore::new());
    let svc = ArtifactService::new(store);

    let body = b"content for digest verification";
    let artifact = svc
        .create(
            None,
            ArtifactKind::Custom {
                type_uri: "test://digest".into(),
            },
            body,
            HashMap::new(),
        )
        .await
        .expect("create artifact");

    // Verify the digest stored in the artifact matches our expectation.
    let expected_digest = compute_digest(body);
    assert_eq!(artifact.blob_ref.blake3_hex, expected_digest.blake3_hex);

    // Retrieve the body and verify integrity.
    let retrieved = svc.get_body(artifact.id).await.expect("get_body");
    assert_eq!(retrieved, body);

    // The verify method should return true.
    assert!(svc.verify(artifact.id).await.expect("verify"));
}

#[test]
fn digest_detects_tampered_content() {
    let body = b"original content";
    let blob_ref = compute_digest(body);

    // Verification with correct body succeeds.
    assert!(verify_digest(body, &blob_ref));

    // Verification with tampered body fails.
    assert!(!verify_digest(b"tampered content", &blob_ref));
}

// ---------------------------------------------------------------------------
// Multiple lineage parents (diamond pattern)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lineage_diamond_pattern() {
    let store = Arc::new(MemoryStore::new());
    let svc = ArtifactService::new(store);

    // Diamond: root -> a, root -> b, a -> merged, b -> merged
    let root = svc
        .create(
            None,
            ArtifactKind::Custom {
                type_uri: "test://root".into(),
            },
            b"root",
            HashMap::new(),
        )
        .await
        .expect("root");

    let a = svc
        .create(
            None,
            ArtifactKind::Custom {
                type_uri: "test://a".into(),
            },
            b"branch a",
            HashMap::new(),
        )
        .await
        .expect("a");

    let b = svc
        .create(
            None,
            ArtifactKind::Custom {
                type_uri: "test://b".into(),
            },
            b"branch b",
            HashMap::new(),
        )
        .await
        .expect("b");

    let merged = svc
        .create(
            None,
            ArtifactKind::Custom {
                type_uri: "test://merged".into(),
            },
            b"merged output",
            HashMap::new(),
        )
        .await
        .expect("merged");

    svc.add_lineage(a.id, root.id).await.expect("a->root");
    svc.add_lineage(b.id, root.id).await.expect("b->root");
    svc.add_lineage(merged.id, a.id).await.expect("merged->a");
    svc.add_lineage(merged.id, b.id).await.expect("merged->b");

    // Ancestors of merged: should find a, b, and root (3 total).
    let ancestors = svc
        .get_lineage(merged.id)
        .await
        .expect("get_lineage merged");
    assert_eq!(ancestors.len(), 3, "merged has 3 unique ancestors");
    assert!(ancestors.contains(&a.id));
    assert!(ancestors.contains(&b.id));
    assert!(ancestors.contains(&root.id));
}

// ---------------------------------------------------------------------------
// Duplicate lineage edge is idempotent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_lineage_is_idempotent() {
    let store = Arc::new(MemoryStore::new());
    let svc = ArtifactService::new(store);

    let parent = svc
        .create(
            None,
            ArtifactKind::Custom {
                type_uri: "test://parent".into(),
            },
            b"parent",
            HashMap::new(),
        )
        .await
        .expect("parent");

    let child = svc
        .create(
            None,
            ArtifactKind::Custom {
                type_uri: "test://child".into(),
            },
            b"child",
            HashMap::new(),
        )
        .await
        .expect("child");

    // Adding the same edge twice should not duplicate it.
    svc.add_lineage(child.id, parent.id).await.expect("first");
    svc.add_lineage(child.id, parent.id)
        .await
        .expect("second (idempotent)");

    let ancestors = svc.get_lineage(child.id).await.expect("lineage");
    assert_eq!(
        ancestors.len(),
        1,
        "duplicate edge should not create two entries"
    );
}

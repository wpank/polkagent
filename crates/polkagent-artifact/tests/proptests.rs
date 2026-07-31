//! Property-based tests for `polkagent-artifact`.
//!
//! These tests use `proptest` to verify invariants of BLAKE3 digest
//! computation and lineage graph traversal across randomly generated inputs.

use std::collections::HashSet;

use proptest::prelude::*;

use polkagent_artifact::digest::{compute_digest, verify_digest};
use polkagent_artifact::lineage::LineageGraph;
use polkagent_core::ids::ArtifactId;

// =========================================================================
// 1. BLAKE3 digest is deterministic (same bytes -> same hash)
// =========================================================================

proptest! {
    /// Computing a BLAKE3 digest twice over the same data yields identical
    /// results every time.
    #[test]
    fn blake3_deterministic(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        let a = compute_digest(&data);
        let b = compute_digest(&data);
        prop_assert_eq!(
            &a.blake3_hex, &b.blake3_hex,
            "digest must be deterministic"
        );
        prop_assert_eq!(a.size_bytes, b.size_bytes);
    }
}

// =========================================================================
// 2. Different bytes produce different digests (with high probability)
// =========================================================================

proptest! {
    /// Two distinct byte sequences produce different BLAKE3 hashes.
    #[test]
    fn different_bytes_different_digest(
        data in prop::collection::vec(any::<u8>(), 1..4096),
        other in prop::collection::vec(any::<u8>(), 1..4096),
    ) {
        prop_assume!(data != other);
        let a = compute_digest(&data);
        let b = compute_digest(&other);
        prop_assert_ne!(
            a.blake3_hex, b.blake3_hex,
            "different data should produce different hashes"
        );
    }

    /// The digest of data always verifies against that same data.
    #[test]
    fn digest_self_verifies(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        let digest = compute_digest(&data);
        prop_assert!(
            verify_digest(&data, &digest),
            "digest must verify its own source data"
        );
    }

    /// The digest of data rejects different data.
    #[test]
    fn digest_rejects_tampered(
        data in prop::collection::vec(any::<u8>(), 1..4096),
        other in prop::collection::vec(any::<u8>(), 1..4096),
    ) {
        prop_assume!(data != other);
        let digest = compute_digest(&data);
        prop_assert!(
            !verify_digest(&other, &digest),
            "digest must reject different data"
        );
    }
}

// =========================================================================
// 3. Lineage graph handles diamond DAGs without duplicates
// =========================================================================

proptest! {
    /// In a diamond DAG (root -> A, root -> B, A -> leaf, B -> leaf),
    /// ancestors of `leaf` never contain duplicates.
    #[test]
    fn diamond_dag_no_duplicate_ancestors(_ in 0..5u8) {
        let root = ArtifactId::new();
        let a = ArtifactId::new();
        let b = ArtifactId::new();
        let leaf = ArtifactId::new();

        let mut graph = LineageGraph::new();
        graph.add_edge(a, root);
        graph.add_edge(b, root);
        graph.add_edge(leaf, a);
        graph.add_edge(leaf, b);

        let ancestors = graph.ancestors(leaf);
        let unique: HashSet<_> = ancestors.iter().copied().collect();

        prop_assert_eq!(
            ancestors.len(), unique.len(),
            "ancestors must not contain duplicates in a diamond DAG"
        );
        // Should be exactly 3 ancestors: a, b, root.
        prop_assert_eq!(ancestors.len(), 3);
        prop_assert!(unique.contains(&root));
        prop_assert!(unique.contains(&a));
        prop_assert!(unique.contains(&b));
    }

    /// In a wider diamond (root -> nodes[0..width], all nodes -> leaf),
    /// ancestors of `leaf` still has no duplicates.
    #[test]
    fn wide_diamond_no_duplicates(width in 2..8usize) {
        let root = ArtifactId::new();
        let leaf = ArtifactId::new();
        let middles: Vec<_> = (0..width).map(|_| ArtifactId::new()).collect();

        let mut graph = LineageGraph::new();
        for &mid in &middles {
            graph.add_edge(mid, root);
            graph.add_edge(leaf, mid);
        }

        let ancestors = graph.ancestors(leaf);
        let unique: HashSet<_> = ancestors.iter().copied().collect();
        prop_assert_eq!(
            ancestors.len(), unique.len(),
            "ancestors must not contain duplicates"
        );
        // Should have width middle nodes + root.
        prop_assert_eq!(ancestors.len(), width + 1);
    }
}

// =========================================================================
// 4. Lineage is idempotent (adding same edge twice = same graph)
// =========================================================================

proptest! {
    /// Adding the same edge to the lineage graph multiple times does not
    /// change the ancestor or descendant sets.
    #[test]
    fn lineage_idempotent(repeats in 2..10usize) {
        let parent = ArtifactId::new();
        let child = ArtifactId::new();

        let mut graph = LineageGraph::new();
        for _ in 0..repeats {
            graph.add_edge(child, parent);
        }

        let ancestors = graph.ancestors(child);
        let descendants = graph.descendants(parent);

        prop_assert_eq!(ancestors.len(), 1, "child should have exactly one ancestor");
        prop_assert_eq!(ancestors[0], parent);
        prop_assert_eq!(descendants.len(), 1, "parent should have exactly one descendant");
        prop_assert_eq!(descendants[0], child);
    }
}

// =========================================================================
// 5. BFS traversal visits every reachable node exactly once
// =========================================================================

proptest! {
    /// In a linear chain of `n` artifacts, BFS from the leaf visits every
    /// ancestor exactly once in nearest-first order.
    #[test]
    fn bfs_visits_every_node_exactly_once(n in 2..15usize) {
        let ids: Vec<ArtifactId> = (0..n).map(|_| ArtifactId::new()).collect();
        let mut graph = LineageGraph::new();
        for i in 0..n - 1 {
            graph.add_edge(ids[i + 1], ids[i]);
        }

        // BFS ancestors from ids[n-1] should visit ids[n-2], ids[n-3], ..., ids[0].
        let ancestors = graph.ancestors(ids[n - 1]);

        // Correct count: all nodes except the leaf itself.
        prop_assert_eq!(ancestors.len(), n - 1, "should visit n-1 ancestors");

        // Every ancestor appears exactly once.
        let unique: HashSet<_> = ancestors.iter().copied().collect();
        prop_assert_eq!(ancestors.len(), unique.len(), "no duplicates in BFS");

        // Nearest ancestor (ids[n-2]) is first.
        prop_assert_eq!(ancestors[0], ids[n - 2], "nearest ancestor should be first");
        // Root (ids[0]) is last.
        prop_assert_eq!(*ancestors.last().unwrap(), ids[0], "root should be last");

        // BFS descendants from ids[0] should visit ids[1] through ids[n-1].
        let descendants = graph.descendants(ids[0]);
        prop_assert_eq!(descendants.len(), n - 1, "should visit n-1 descendants");
        let desc_unique: HashSet<_> = descendants.iter().copied().collect();
        prop_assert_eq!(descendants.len(), desc_unique.len(), "no duplicates in BFS descendants");
    }
}

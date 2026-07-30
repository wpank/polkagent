//! Directed acyclic graph for tracking artifact parent-child relationships.
//!
//! [`LineageGraph`] stores edges of the form `child → parent`, reflecting the
//! provenance direction: a child artifact was *derived from* its parents.
//! The graph provides forward (ancestor) and reverse (descendant) traversals
//! using iterative BFS to avoid stack overflows on long chains.
//!
//! # Invariants
//!
//! - The graph is a DAG; callers are responsible for avoiding cycles.  Adding
//!   a cycle does not cause undefined behaviour but will cause [`ancestors`]
//!   and [`descendants`] to loop forever on naïve implementations, so this
//!   module uses a `visited` set in each traversal.
//! - The graph is in-memory only.  Persistent lineage is stored by the
//!   [`ArtifactStore`](crate::store::ArtifactStore) backend.

use std::collections::{HashMap, HashSet, VecDeque};

use polkagent_core::ids::ArtifactId;

// ---------------------------------------------------------------------------
// LineageGraph
// ---------------------------------------------------------------------------

/// An in-memory directed acyclic graph of artifact parent-child edges.
///
/// Edges represent *provenance*: `(child, parent)` means the child was derived
/// from the parent.
///
/// ```text
///   root ──► child_a ──► grandchild
///       └──► child_b
/// ```
#[derive(Debug, Default, Clone)]
pub struct LineageGraph {
    /// child → set of direct parents
    parents: HashMap<ArtifactId, HashSet<ArtifactId>>,
    /// parent → set of direct children  (reverse index for descendants)
    children: HashMap<ArtifactId, HashSet<ArtifactId>>,
}

impl LineageGraph {
    /// Create an empty lineage graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `child` was derived from `parent`.
    ///
    /// Adding a duplicate edge is a no-op.
    pub fn add_edge(&mut self, child: ArtifactId, parent: ArtifactId) {
        self.parents
            .entry(child)
            .or_default()
            .insert(parent);
        self.children
            .entry(parent)
            .or_default()
            .insert(child);
        // Ensure every node appears in both maps even if it has no edges on
        // one side.  This simplifies traversal code.
        self.parents.entry(parent).or_default();
        self.children.entry(child).or_default();
    }

    /// Return all ancestors of `id` in breadth-first order (direct parents
    /// first, then grandparents, etc.).
    ///
    /// The result excludes `id` itself.  If `id` is unknown, returns an empty
    /// `Vec`.
    #[must_use]
    pub fn ancestors(&self, id: ArtifactId) -> Vec<ArtifactId> {
        self.bfs_from(id, |node| {
            self.parents
                .get(&node)
                .map(|s| s.iter().copied().collect::<Vec<_>>())
                .unwrap_or_default()
        })
    }

    /// Return all descendants of `id` in breadth-first order (direct children
    /// first, then grandchildren, etc.).
    ///
    /// The result excludes `id` itself.  If `id` is unknown, returns an empty
    /// `Vec`.
    #[must_use]
    pub fn descendants(&self, id: ArtifactId) -> Vec<ArtifactId> {
        self.bfs_from(id, |node| {
            self.children
                .get(&node)
                .map(|s| s.iter().copied().collect::<Vec<_>>())
                .unwrap_or_default()
        })
    }

    /// Return the direct parents of `id`.
    #[must_use]
    pub fn direct_parents(&self, id: ArtifactId) -> Vec<ArtifactId> {
        self.parents
            .get(&id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Return the direct children of `id`.
    #[must_use]
    pub fn direct_children(&self, id: ArtifactId) -> Vec<ArtifactId> {
        self.children
            .get(&id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Generic BFS starting from `start`, using `next_fn` to produce
    /// neighbours for each visited node.  `start` itself is excluded from the
    /// output.
    fn bfs_from<F>(&self, start: ArtifactId, next_fn: F) -> Vec<ArtifactId>
    where
        F: Fn(ArtifactId) -> Vec<ArtifactId>,
    {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        visited.insert(start);
        queue.push_back(start);

        while let Some(node) = queue.pop_front() {
            for neighbour in next_fn(node) {
                if visited.insert(neighbour) {
                    result.push(neighbour);
                    queue.push_back(neighbour);
                }
            }
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ids(n: usize) -> Vec<ArtifactId> {
        (0..n).map(|_| ArtifactId::new()).collect()
    }

    #[test]
    fn empty_graph_returns_empty_ancestors() {
        let graph = LineageGraph::new();
        let id = ArtifactId::new();
        assert!(graph.ancestors(id).is_empty());
    }

    #[test]
    fn empty_graph_returns_empty_descendants() {
        let graph = LineageGraph::new();
        let id = ArtifactId::new();
        assert!(graph.descendants(id).is_empty());
    }

    #[test]
    fn single_edge_parent_and_child() {
        let ids = make_ids(2);
        let (child, parent) = (ids[0], ids[1]);

        let mut graph = LineageGraph::new();
        graph.add_edge(child, parent);

        assert_eq!(graph.ancestors(child), vec![parent]);
        assert!(graph.ancestors(parent).is_empty());

        assert_eq!(graph.descendants(parent), vec![child]);
        assert!(graph.descendants(child).is_empty());
    }

    #[test]
    fn linear_chain_traversal_10_artifacts() {
        // ids[0] is root; ids[i+1] is a child of ids[i].
        let ids = make_ids(10);
        let mut graph = LineageGraph::new();
        for i in 0..9 {
            graph.add_edge(ids[i + 1], ids[i]);
        }

        // All ancestors of ids[9] should be ids[0..=8] in BFS order
        // (nearest ancestor first).
        let ancestors = graph.ancestors(ids[9]);
        assert_eq!(ancestors.len(), 9, "should have 9 ancestors");
        // Nearest ancestor (ids[8]) must be first.
        assert_eq!(ancestors[0], ids[8]);
        // Root (ids[0]) must be last.
        assert_eq!(*ancestors.last().expect("non-empty"), ids[0]);

        // All descendants of ids[0] should be ids[1..=9].
        let descendants = graph.descendants(ids[0]);
        assert_eq!(descendants.len(), 9);
        assert_eq!(descendants[0], ids[1]);
        assert_eq!(*descendants.last().expect("non-empty"), ids[9]);
    }

    #[test]
    fn diamond_dag_no_duplicates_in_ancestors() {
        //   root
        //   / \
        //  a   b
        //   \ /
        //  leaf
        let ids = make_ids(4);
        let (root, a, b, leaf) = (ids[0], ids[1], ids[2], ids[3]);
        let mut graph = LineageGraph::new();
        graph.add_edge(a, root);
        graph.add_edge(b, root);
        graph.add_edge(leaf, a);
        graph.add_edge(leaf, b);

        let ancestors = graph.ancestors(leaf);
        // root must appear exactly once despite two paths.
        let root_count = ancestors.iter().filter(|&&x| x == root).count();
        assert_eq!(root_count, 1, "root must appear exactly once");
        assert_eq!(ancestors.len(), 3); // a, b, root
    }

    #[test]
    fn duplicate_edge_is_idempotent() {
        let ids = make_ids(2);
        let (child, parent) = (ids[0], ids[1]);
        let mut graph = LineageGraph::new();
        graph.add_edge(child, parent);
        graph.add_edge(child, parent); // duplicate
        assert_eq!(graph.ancestors(child).len(), 1);
        assert_eq!(graph.descendants(parent).len(), 1);
    }

    #[test]
    fn direct_parents_and_children() {
        let ids = make_ids(3);
        let (root, a, b) = (ids[0], ids[1], ids[2]);
        let mut graph = LineageGraph::new();
        graph.add_edge(a, root);
        graph.add_edge(b, root);

        let parents_of_a = graph.direct_parents(a);
        assert_eq!(parents_of_a, vec![root]);

        let children_of_root = graph.direct_children(root);
        assert!(children_of_root.contains(&a));
        assert!(children_of_root.contains(&b));
        assert_eq!(children_of_root.len(), 2);
    }
}

//! DAG execution engine for workflow composition.
//!
//! This module provides a directed acyclic graph (DAG) abstraction for
//! composing multi-step workflows. Each node in the DAG represents a
//! discrete [`StepKind`] that can be executed once its dependencies have
//! completed. The DAG validates structural constraints (no cycles, valid
//! edges) and tracks node-level execution state.
//!
//! # Key types
//!
//! | Type | Responsibility |
//! |---|---|
//! | [`ExecutionDag`] | Graph structure: nodes, edges, state tracking |
//! | [`DagExecutor`] | Drives execution: batches ready nodes, records outcomes |
//! | [`DagError`] | Structural and runtime error cases |
//!
//! # Workflow builders
//!
//! Three convenience constructors cover common patterns:
//!
//! - [`sequential`] — linear chain `A -> B -> C`
//! - [`parallel`] — all nodes independent
//! - [`fan_out_fan_in`] — one source fans out to many, then a single sink

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use polkagent_core::StepKind;

// ---------------------------------------------------------------------------
// NodeId
// ---------------------------------------------------------------------------

/// Lightweight identifier for a node within an [`ExecutionDag`].
///
/// `NodeId` is a thin wrapper around [`u32`] — cheap to copy, hash, and
/// compare. IDs are assigned sequentially by `ExecutionDag::add_node`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u32);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "node-{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// NodeState
// ---------------------------------------------------------------------------

/// Execution state of a single [`DagNode`].
///
/// The state progresses linearly for the happy path:
/// `Pending -> Ready -> Running -> Completed`.
///
/// A node may also be marked `Failed` (unrecoverable error) or `Skipped`
/// (a dependency failed and this node cannot run).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeState {
    /// The node has unmet dependencies and cannot run yet.
    Pending,
    /// All dependencies are satisfied; the node is eligible for execution.
    Ready,
    /// The node is currently being executed.
    Running,
    /// The node finished successfully.
    Completed {
        /// The result value produced by this node.
        result: Value,
    },
    /// The node encountered an unrecoverable error.
    Failed {
        /// Human-readable description of the failure.
        error: String,
    },
    /// The node was skipped (typically because a dependency failed).
    Skipped,
}

impl NodeState {
    /// Returns `true` if the node is in a terminal state (completed, failed,
    /// or skipped).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Skipped
        )
    }
}

// ---------------------------------------------------------------------------
// DagNode
// ---------------------------------------------------------------------------

/// A single node in the execution DAG.
///
/// Each node represents one logical step that will be executed when all of
/// its [`dependencies`](DagNode::dependencies) have reached a terminal
/// state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNode {
    /// Unique identifier within the owning [`ExecutionDag`].
    pub id: NodeId,
    /// Human-readable label (e.g. `"assemble context"`, `"call GPT-4"`).
    pub label: String,
    /// The kind of step this node represents.
    pub step_kind: StepKind,
    /// IDs of nodes that must complete before this node can run.
    pub dependencies: Vec<NodeId>,
    /// Current execution state of this node.
    pub state: NodeState,
}

// ---------------------------------------------------------------------------
// DagEdge
// ---------------------------------------------------------------------------

/// A directed edge in the DAG: `from` must complete before `to` can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DagEdge {
    /// The predecessor node.
    pub from: NodeId,
    /// The successor node (depends on `from`).
    pub to: NodeId,
}

// ---------------------------------------------------------------------------
// DagError
// ---------------------------------------------------------------------------

/// Errors that can occur when building or validating an [`ExecutionDag`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DagError {
    /// A cycle was detected in the graph. The `nodes` field contains the
    /// node IDs that participate in the cycle.
    CycleDetected {
        /// Node IDs that are part of the cycle.
        nodes: Vec<NodeId>,
    },
    /// A referenced node does not exist in the DAG.
    NodeNotFound(NodeId),
    /// An edge could not be added because of a structural constraint.
    InvalidEdge {
        /// Human-readable explanation.
        reason: String,
    },
    /// An operation was attempted on a node that has already reached a
    /// terminal state.
    AlreadyTerminal(NodeId),
}

impl fmt::Display for DagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CycleDetected { nodes } => {
                let ids: Vec<String> = nodes.iter().map(ToString::to_string).collect();
                write!(f, "cycle detected involving nodes: {}", ids.join(", "))
            }
            Self::NodeNotFound(id) => write!(f, "node not found: {id}"),
            Self::InvalidEdge { reason } => write!(f, "invalid edge: {reason}"),
            Self::AlreadyTerminal(id) => {
                write!(f, "node {id} is already in a terminal state")
            }
        }
    }
}

impl std::error::Error for DagError {}

// ---------------------------------------------------------------------------
// ExecutionDag
// ---------------------------------------------------------------------------

/// A directed acyclic graph for composing multi-step workflows.
///
/// Nodes are added with [`add_node`](Self::add_node) and edges with
/// [`add_edge`](Self::add_edge). The DAG validates that no cycles are
/// introduced when edges are added, and tracks per-node execution state.
///
/// # Example
///
/// ```
/// use polkagent_run::dag::ExecutionDag;
/// use polkagent_core::StepKind;
///
/// let mut dag = ExecutionDag::new();
/// let a = dag.add_node("assemble context", StepKind::ContextAssembly);
/// let b = dag.add_node("call model", StepKind::ModelInference);
/// dag.add_edge(a, b).unwrap();
///
/// // `a` is ready (no deps), `b` is pending.
/// let ready = dag.ready_nodes();
/// assert_eq!(ready, vec![a]);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionDag {
    nodes: HashMap<NodeId, DagNode>,
    edges: Vec<DagEdge>,
    next_id: u32,
}

impl Default for ExecutionDag {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutionDag {
    /// Create an empty DAG with no nodes or edges.
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            next_id: 0,
        }
    }

    /// Add a node to the DAG and return its [`NodeId`].
    ///
    /// Nodes start in [`NodeState::Pending`] if they will later have
    /// dependencies, or [`NodeState::Ready`] if they never get any.
    /// The state is lazily recomputed by [`ready_nodes`](Self::ready_nodes).
    pub fn add_node(&mut self, label: impl Into<String>, step_kind: StepKind) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        let node = DagNode {
            id,
            label: label.into(),
            step_kind,
            dependencies: Vec::new(),
            state: NodeState::Pending,
        };
        self.nodes.insert(id, node);
        id
    }

    /// Add a directed edge from `from` to `to`.
    ///
    /// This means `to` depends on `from`: the `from` node must reach a
    /// terminal state before `to` becomes ready.
    ///
    /// # Errors
    ///
    /// Returns [`DagError::NodeNotFound`] if either node ID is unknown, or
    /// [`DagError::CycleDetected`] if adding the edge would create a cycle.
    /// Returns [`DagError::InvalidEdge`] if the edge is a self-loop or
    /// already exists.
    pub fn add_edge(&mut self, from: NodeId, to: NodeId) -> Result<(), DagError> {
        // Self-loop check.
        if from == to {
            return Err(DagError::InvalidEdge {
                reason: format!("self-loop on {from}"),
            });
        }

        // Existence check.
        if !self.nodes.contains_key(&from) {
            return Err(DagError::NodeNotFound(from));
        }
        if !self.nodes.contains_key(&to) {
            return Err(DagError::NodeNotFound(to));
        }

        // Duplicate check.
        if self.edges.iter().any(|e| e.from == from && e.to == to) {
            return Err(DagError::InvalidEdge {
                reason: format!("edge {from} -> {to} already exists"),
            });
        }

        // Tentatively add the edge and check for cycles.
        self.edges.push(DagEdge { from, to });

        if let Err(e) = self.topological_order() {
            // Remove the edge we just added.
            self.edges.pop();
            return Err(e);
        }

        // Record the dependency on the target node.
        if let Some(node) = self.nodes.get_mut(&to) {
            if !node.dependencies.contains(&from) {
                node.dependencies.push(from);
            }
        }

        Ok(())
    }

    /// Convenience wrapper: `node` depends on `depends_on`.
    ///
    /// Equivalent to `add_edge(depends_on, node)`.
    ///
    /// # Errors
    ///
    /// Forwards errors from [`add_edge`](Self::add_edge).
    pub fn add_dependency(&mut self, node: NodeId, depends_on: NodeId) -> Result<(), DagError> {
        self.add_edge(depends_on, node)
    }

    /// Return all nodes whose dependencies have all completed (reached a
    /// terminal state) and that are themselves still in [`NodeState::Pending`].
    ///
    /// The returned nodes are eligible for immediate execution.
    #[must_use]
    pub fn ready_nodes(&self) -> Vec<NodeId> {
        let mut ready = Vec::new();
        for node in self.nodes.values() {
            if node.state != NodeState::Pending {
                continue;
            }
            let all_deps_done = node.dependencies.iter().all(|dep_id| {
                self.nodes
                    .get(dep_id)
                    .is_some_and(|dep| dep.state.is_terminal())
            });
            if all_deps_done {
                ready.push(node.id);
            }
        }
        ready.sort_by_key(|id| id.0);
        ready
    }

    /// Mark a node as running.
    ///
    /// # Errors
    ///
    /// Returns [`DagError::NodeNotFound`] if the node does not exist, or
    /// [`DagError::AlreadyTerminal`] if it has already finished.
    pub fn mark_running(&mut self, id: NodeId) -> Result<(), DagError> {
        let node = self.nodes.get_mut(&id).ok_or(DagError::NodeNotFound(id))?;
        if node.state.is_terminal() {
            return Err(DagError::AlreadyTerminal(id));
        }
        node.state = NodeState::Running;
        Ok(())
    }

    /// Mark a node as successfully completed with a result value.
    ///
    /// # Errors
    ///
    /// Returns [`DagError::NodeNotFound`] if the node does not exist, or
    /// [`DagError::AlreadyTerminal`] if it has already finished.
    pub fn mark_completed(&mut self, id: NodeId, result: Value) -> Result<(), DagError> {
        let node = self.nodes.get_mut(&id).ok_or(DagError::NodeNotFound(id))?;
        if node.state.is_terminal() {
            return Err(DagError::AlreadyTerminal(id));
        }
        node.state = NodeState::Completed { result };
        Ok(())
    }

    /// Mark a node as failed with an error message.
    ///
    /// # Errors
    ///
    /// Returns [`DagError::NodeNotFound`] if the node does not exist, or
    /// [`DagError::AlreadyTerminal`] if it has already finished.
    pub fn mark_failed(&mut self, id: NodeId, error: impl Into<String>) -> Result<(), DagError> {
        let node = self.nodes.get_mut(&id).ok_or(DagError::NodeNotFound(id))?;
        if node.state.is_terminal() {
            return Err(DagError::AlreadyTerminal(id));
        }
        node.state = NodeState::Failed {
            error: error.into(),
        };
        Ok(())
    }

    /// Mark a node as skipped (e.g. because a dependency failed).
    ///
    /// # Errors
    ///
    /// Returns [`DagError::NodeNotFound`] if the node does not exist, or
    /// [`DagError::AlreadyTerminal`] if it has already finished.
    pub fn mark_skipped(&mut self, id: NodeId) -> Result<(), DagError> {
        let node = self.nodes.get_mut(&id).ok_or(DagError::NodeNotFound(id))?;
        if node.state.is_terminal() {
            return Err(DagError::AlreadyTerminal(id));
        }
        node.state = NodeState::Skipped;
        Ok(())
    }

    /// Returns `true` when every node in the DAG has reached a terminal
    /// state (completed, failed, or skipped).
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.nodes.values().all(|n| n.state.is_terminal())
    }

    /// Returns `true` if any node in the DAG has failed.
    #[must_use]
    pub fn has_failed(&self) -> bool {
        self.nodes
            .values()
            .any(|n| matches!(n.state, NodeState::Failed { .. }))
    }

    /// Compute a topological ordering of all nodes using Kahn's algorithm.
    ///
    /// If the graph contains a cycle, the nodes participating in the cycle
    /// are reported via [`DagError::CycleDetected`].
    ///
    /// # Errors
    ///
    /// Returns [`DagError::CycleDetected`] if the graph contains a cycle.
    pub fn topological_order(&self) -> Result<Vec<NodeId>, DagError> {
        // Build in-degree map.
        let mut in_degree: HashMap<NodeId, usize> = HashMap::new();
        for id in self.nodes.keys() {
            in_degree.entry(*id).or_insert(0);
        }
        for edge in &self.edges {
            *in_degree.entry(edge.to).or_insert(0) += 1;
        }

        // Seed the queue with zero-in-degree nodes (sorted for determinism).
        let mut queue: VecDeque<NodeId> = VecDeque::new();
        let mut zero_deg: Vec<NodeId> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(id, _)| *id)
            .collect();
        zero_deg.sort_by_key(|id| id.0);
        for id in zero_deg {
            queue.push_back(id);
        }

        let mut order = Vec::with_capacity(self.nodes.len());

        while let Some(node_id) = queue.pop_front() {
            order.push(node_id);
            // Collect successors to process, then sort for determinism.
            let mut successors: Vec<NodeId> = self
                .edges
                .iter()
                .filter(|e| e.from == node_id)
                .map(|e| e.to)
                .collect();
            successors.sort_by_key(|id| id.0);
            for succ in successors {
                if let Some(deg) = in_degree.get_mut(&succ) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(succ);
                    }
                }
            }
        }

        if order.len() == self.nodes.len() {
            Ok(order)
        } else {
            // Nodes not in the order are part of a cycle.
            let in_order: HashSet<NodeId> = order.into_iter().collect();
            let mut cycle_nodes: Vec<NodeId> = self
                .nodes
                .keys()
                .filter(|id| !in_order.contains(id))
                .copied()
                .collect();
            cycle_nodes.sort_by_key(|id| id.0);
            Err(DagError::CycleDetected { nodes: cycle_nodes })
        }
    }

    /// Validate the DAG structure: check for cycles and orphan references.
    ///
    /// # Errors
    ///
    /// Returns [`DagError::CycleDetected`] if cycles exist, or
    /// [`DagError::NodeNotFound`] if any edge references a non-existent node.
    pub fn validate(&self) -> Result<(), DagError> {
        // Check edge references.
        for edge in &self.edges {
            if !self.nodes.contains_key(&edge.from) {
                return Err(DagError::NodeNotFound(edge.from));
            }
            if !self.nodes.contains_key(&edge.to) {
                return Err(DagError::NodeNotFound(edge.to));
            }
        }
        // Check for cycles.
        self.topological_order()?;
        Ok(())
    }

    /// Return a reference to a node by ID, if it exists.
    #[must_use]
    pub fn get_node(&self, id: NodeId) -> Option<&DagNode> {
        self.nodes.get(&id)
    }

    /// Return the total number of nodes in the DAG.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Return all edges in the DAG.
    #[must_use]
    pub fn edges(&self) -> &[DagEdge] {
        &self.edges
    }

    /// Collect all nodes that transitively depend on the given node.
    fn dependents_of(&self, id: NodeId) -> Vec<NodeId> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(id);
        while let Some(current) = queue.pop_front() {
            for edge in &self.edges {
                if edge.from == current && visited.insert(edge.to) {
                    queue.push_back(edge.to);
                }
            }
        }
        let mut result: Vec<NodeId> = visited.into_iter().collect();
        result.sort_by_key(|nid| nid.0);
        result
    }
}

// ---------------------------------------------------------------------------
// DagExecutor
// ---------------------------------------------------------------------------

/// Drives execution of an [`ExecutionDag`] by yielding batches of ready
/// nodes and recording their outcomes.
///
/// The executor does not perform the actual work — it merely coordinates
/// which nodes are eligible to run, marks them running, and accepts
/// completion or failure reports. The caller is responsible for dispatching
/// the work and calling back with results.
///
/// # Example
///
/// ```
/// use polkagent_run::dag::{DagExecutor, ExecutionDag};
/// use polkagent_core::StepKind;
/// use serde_json::json;
///
/// let mut dag = ExecutionDag::new();
/// let a = dag.add_node("step-a", StepKind::ContextAssembly);
///
/// let mut executor = DagExecutor::new(dag);
///
/// // Poll for ready nodes.
/// let batch = executor.next_ready();
/// assert_eq!(batch, Some(vec![a]));
///
/// // Complete the node.
/// executor.complete_node(a, json!("done")).unwrap();
/// assert!(executor.is_complete());
/// ```
pub struct DagExecutor {
    dag: ExecutionDag,
}

impl DagExecutor {
    /// Create a new executor wrapping the given DAG.
    #[must_use]
    pub fn new(dag: ExecutionDag) -> Self {
        Self { dag }
    }

    /// Return the next batch of nodes that are ready for parallel execution.
    ///
    /// Each returned node is automatically marked as [`NodeState::Running`].
    /// Returns `None` when the DAG is complete (all nodes terminal).
    #[must_use]
    pub fn next_ready(&mut self) -> Option<Vec<NodeId>> {
        if self.dag.is_complete() {
            return None;
        }
        let ready = self.dag.ready_nodes();
        if ready.is_empty() {
            return None;
        }
        for &id in &ready {
            // Marking running should not fail for ready nodes.
            let _res = self.dag.mark_running(id);
        }
        Some(ready)
    }

    /// Record successful completion of a node.
    ///
    /// After marking the node completed, the executor checks if any
    /// downstream nodes have become ready.
    ///
    /// # Errors
    ///
    /// Returns [`DagError`] if the node does not exist or is already
    /// terminal.
    pub fn complete_node(&mut self, id: NodeId, result: Value) -> Result<(), DagError> {
        self.dag.mark_completed(id, result)
    }

    /// Record failure of a node and skip all transitive dependents.
    ///
    /// All nodes that directly or transitively depend on the failed node
    /// are marked as [`NodeState::Skipped`].
    ///
    /// # Errors
    ///
    /// Returns [`DagError`] if the node does not exist or is already
    /// terminal.
    pub fn fail_node(&mut self, id: NodeId, error: impl Into<String>) -> Result<(), DagError> {
        self.dag.mark_failed(id, error)?;

        // Skip all transitive dependents.
        let dependents = self.dag.dependents_of(id);
        for dep_id in dependents {
            if let Some(node) = self.dag.nodes.get(&dep_id) {
                if !node.state.is_terminal() {
                    let _res = self.dag.mark_skipped(dep_id);
                }
            }
        }
        Ok(())
    }

    /// Returns `true` when every node is terminal.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.dag.is_complete()
    }

    /// Returns `true` if any node has failed.
    #[must_use]
    pub fn has_failed(&self) -> bool {
        self.dag.has_failed()
    }

    /// Return a shared reference to the underlying DAG.
    #[must_use]
    pub fn dag(&self) -> &ExecutionDag {
        &self.dag
    }
}

// ---------------------------------------------------------------------------
// Workflow builders
// ---------------------------------------------------------------------------

/// Build a linear chain: `steps[0] -> steps[1] -> ... -> steps[n-1]`.
///
/// Each step depends on the previous one. The resulting DAG executes the
/// steps sequentially.
#[must_use]
pub fn sequential(steps: Vec<(String, StepKind)>) -> ExecutionDag {
    let mut dag = ExecutionDag::new();
    let mut prev: Option<NodeId> = None;
    for (label, kind) in steps {
        let id = dag.add_node(label, kind);
        if let Some(prev_id) = prev {
            // Sequential chain cannot produce cycles.
            let _res = dag.add_edge(prev_id, id);
        }
        prev = Some(id);
    }
    dag
}

/// Build a fully parallel graph: all steps are independent.
///
/// No edges are added; every node is immediately ready.
#[must_use]
pub fn parallel(steps: Vec<(String, StepKind)>) -> ExecutionDag {
    let mut dag = ExecutionDag::new();
    for (label, kind) in steps {
        dag.add_node(label, kind);
    }
    dag
}

/// Build a fan-out/fan-in graph.
///
/// ```text
///           ┌── target_0 ──┐
/// source ──┤── target_1 ──├── sink
///           └── target_2 ──┘
/// ```
///
/// The `source` node fans out to all `targets`, and all targets fan in to
/// the single `sink` node.
#[must_use]
pub fn fan_out_fan_in(
    source: (String, StepKind),
    targets: Vec<(String, StepKind)>,
    sink: (String, StepKind),
) -> ExecutionDag {
    let mut dag = ExecutionDag::new();
    let source_id = dag.add_node(source.0, source.1);

    let mut target_ids = Vec::with_capacity(targets.len());
    for (label, kind) in targets {
        let tid = dag.add_node(label, kind);
        // source -> target edge cannot produce a cycle.
        let _res = dag.add_edge(source_id, tid);
        target_ids.push(tid);
    }

    let sink_id = dag.add_node(sink.0, sink.1);
    for tid in target_ids {
        // target -> sink edge cannot produce a cycle.
        let _res = dag.add_edge(tid, sink_id);
    }

    dag
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- Empty DAG ---

    #[test]
    fn empty_dag_is_complete() {
        let dag = ExecutionDag::new();
        assert!(dag.is_complete());
        assert!(!dag.has_failed());
    }

    #[test]
    fn empty_dag_has_no_ready_nodes() {
        let dag = ExecutionDag::new();
        assert!(dag.ready_nodes().is_empty());
    }

    #[test]
    fn empty_dag_topological_order_is_empty() {
        let dag = ExecutionDag::new();
        let order = dag.topological_order().expect("no cycle");
        assert!(order.is_empty());
    }

    #[test]
    fn empty_dag_validates() {
        let dag = ExecutionDag::new();
        dag.validate().expect("valid");
    }

    // --- Single node ---

    #[test]
    fn single_node_is_ready() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let ready = dag.ready_nodes();
        assert_eq!(ready, vec![a]);
    }

    #[test]
    fn single_node_not_complete_until_terminal() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        assert!(!dag.is_complete());

        dag.mark_running(a).expect("mark running");
        assert!(!dag.is_complete());

        dag.mark_completed(a, json!("done"))
            .expect("mark completed");
        assert!(dag.is_complete());
    }

    #[test]
    fn single_node_topological_order() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let order = dag.topological_order().expect("no cycle");
        assert_eq!(order, vec![a]);
    }

    // --- Linear chain ---

    #[test]
    fn linear_chain_ready_nodes() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let b = dag.add_node("b", StepKind::ModelInference);
        let c = dag.add_node("c", StepKind::OutputParsing);
        dag.add_edge(a, b).expect("a->b");
        dag.add_edge(b, c).expect("b->c");

        // Only `a` should be ready.
        assert_eq!(dag.ready_nodes(), vec![a]);

        // Complete `a`, now `b` should be ready.
        dag.mark_running(a).expect("run a");
        dag.mark_completed(a, json!("a-done")).expect("complete a");
        assert_eq!(dag.ready_nodes(), vec![b]);

        // Complete `b`, now `c` should be ready.
        dag.mark_running(b).expect("run b");
        dag.mark_completed(b, json!("b-done")).expect("complete b");
        assert_eq!(dag.ready_nodes(), vec![c]);
    }

    #[test]
    fn linear_chain_topological_order() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let b = dag.add_node("b", StepKind::ModelInference);
        let c = dag.add_node("c", StepKind::OutputParsing);
        dag.add_edge(a, b).expect("a->b");
        dag.add_edge(b, c).expect("b->c");

        let order = dag.topological_order().expect("no cycle");
        assert_eq!(order, vec![a, b, c]);
    }

    // --- Diamond pattern ---

    #[test]
    fn diamond_pattern_ready_nodes() {
        //     A
        //    / \
        //   B   C
        //    \ /
        //     D
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let b = dag.add_node("b", StepKind::ModelInference);
        let c = dag.add_node("c", StepKind::ToolInvocation);
        let d = dag.add_node("d", StepKind::OutputParsing);
        dag.add_edge(a, b).expect("a->b");
        dag.add_edge(a, c).expect("a->c");
        dag.add_edge(b, d).expect("b->d");
        dag.add_edge(c, d).expect("c->d");

        // Initially only A is ready.
        assert_eq!(dag.ready_nodes(), vec![a]);

        // Complete A: B and C should be ready.
        dag.mark_running(a).expect("run a");
        dag.mark_completed(a, json!("a-done")).expect("complete a");
        let mut ready = dag.ready_nodes();
        ready.sort_by_key(|id| id.0);
        assert_eq!(ready, vec![b, c]);

        // Complete B only: D is NOT ready (C still pending).
        dag.mark_running(b).expect("run b");
        dag.mark_completed(b, json!("b-done")).expect("complete b");
        assert!(dag.ready_nodes().is_empty() || dag.ready_nodes() == vec![c]);
        // c is still pending, not terminal, so D cannot be ready.
        // c should be ready though.
        let ready = dag.ready_nodes();
        // c is pending (not running), its dep (a) is completed => c is ready.
        assert_eq!(ready, vec![c]);

        // Complete C: now D is ready.
        dag.mark_running(c).expect("run c");
        dag.mark_completed(c, json!("c-done")).expect("complete c");
        assert_eq!(dag.ready_nodes(), vec![d]);
    }

    #[test]
    fn diamond_topological_order() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let b = dag.add_node("b", StepKind::ModelInference);
        let c = dag.add_node("c", StepKind::ToolInvocation);
        let d = dag.add_node("d", StepKind::OutputParsing);
        dag.add_edge(a, b).expect("a->b");
        dag.add_edge(a, c).expect("a->c");
        dag.add_edge(b, d).expect("b->d");
        dag.add_edge(c, d).expect("c->d");

        let order = dag.topological_order().expect("no cycle");
        // A must come first, D must come last.
        assert_eq!(order[0], a);
        assert_eq!(*order.last().expect("has last"), d);
        // B and C must appear between A and D.
        let b_pos = order.iter().position(|&id| id == b).expect("b in order");
        let c_pos = order.iter().position(|&id| id == c).expect("c in order");
        assert!(b_pos > 0 && b_pos < 3);
        assert!(c_pos > 0 && c_pos < 3);
    }

    // --- Cycle detection ---

    #[test]
    fn cycle_detection_simple() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let b = dag.add_node("b", StepKind::ModelInference);
        dag.add_edge(a, b).expect("a->b");

        let err = dag.add_edge(b, a).expect_err("should detect cycle");
        match err {
            DagError::CycleDetected { nodes } => {
                assert!(nodes.contains(&a));
                assert!(nodes.contains(&b));
            }
            other => panic!("expected CycleDetected, got {other:?}"),
        }
    }

    #[test]
    fn cycle_detection_triangle() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let b = dag.add_node("b", StepKind::ModelInference);
        let c = dag.add_node("c", StepKind::OutputParsing);
        dag.add_edge(a, b).expect("a->b");
        dag.add_edge(b, c).expect("b->c");

        let err = dag.add_edge(c, a).expect_err("should detect cycle");
        match err {
            DagError::CycleDetected { nodes } => {
                assert_eq!(nodes.len(), 3);
            }
            other => panic!("expected CycleDetected, got {other:?}"),
        }
    }

    #[test]
    fn self_loop_rejected() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let err = dag.add_edge(a, a).expect_err("self-loop");
        assert!(matches!(err, DagError::InvalidEdge { .. }));
    }

    #[test]
    fn duplicate_edge_rejected() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let b = dag.add_node("b", StepKind::ModelInference);
        dag.add_edge(a, b).expect("first a->b");
        let err = dag.add_edge(a, b).expect_err("duplicate");
        assert!(matches!(err, DagError::InvalidEdge { .. }));
    }

    #[test]
    fn edge_to_nonexistent_node() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let fake = NodeId(999);
        let err = dag.add_edge(a, fake).expect_err("not found");
        assert!(matches!(err, DagError::NodeNotFound(id) if id == fake));
    }

    // --- Parallel execution ordering ---

    #[test]
    fn parallel_nodes_all_ready() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let b = dag.add_node("b", StepKind::ModelInference);
        let c = dag.add_node("c", StepKind::ToolInvocation);

        let mut ready = dag.ready_nodes();
        ready.sort_by_key(|id| id.0);
        assert_eq!(ready, vec![a, b, c]);
    }

    #[test]
    fn parallel_nodes_complete_independently() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let b = dag.add_node("b", StepKind::ModelInference);
        let c = dag.add_node("c", StepKind::ToolInvocation);

        dag.mark_running(b).expect("run b");
        dag.mark_completed(b, json!("b-done")).expect("complete b");

        // a and c still ready.
        let mut ready = dag.ready_nodes();
        ready.sort_by_key(|id| id.0);
        assert_eq!(ready, vec![a, c]);
        assert!(!dag.is_complete());
    }

    // --- Node state transitions ---

    #[test]
    fn node_state_pending_to_running_to_completed() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);

        assert_eq!(dag.get_node(a).expect("exists").state, NodeState::Pending);
        dag.mark_running(a).expect("mark running");
        assert_eq!(dag.get_node(a).expect("exists").state, NodeState::Running);
        dag.mark_completed(a, json!("ok")).expect("mark completed");
        assert!(matches!(
            dag.get_node(a).expect("exists").state,
            NodeState::Completed { .. }
        ));
    }

    #[test]
    fn node_state_to_failed() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        dag.mark_running(a).expect("run");
        dag.mark_failed(a, "boom").expect("fail");
        assert!(matches!(
            dag.get_node(a).expect("exists").state,
            NodeState::Failed { .. }
        ));
        assert!(dag.has_failed());
    }

    #[test]
    fn node_state_to_skipped() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        dag.mark_skipped(a).expect("skip");
        assert_eq!(dag.get_node(a).expect("exists").state, NodeState::Skipped);
        assert!(dag.is_complete());
    }

    #[test]
    fn cannot_transition_terminal_node() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        dag.mark_completed(a, json!("done")).expect("complete");

        let err = dag.mark_running(a).expect_err("already terminal");
        assert!(matches!(err, DagError::AlreadyTerminal(_)));

        let err = dag.mark_failed(a, "x").expect_err("already terminal");
        assert!(matches!(err, DagError::AlreadyTerminal(_)));

        let err = dag.mark_skipped(a).expect_err("already terminal");
        assert!(matches!(err, DagError::AlreadyTerminal(_)));
    }

    #[test]
    fn mark_nonexistent_node_fails() {
        let mut dag = ExecutionDag::new();
        let fake = NodeId(42);
        assert!(matches!(
            dag.mark_running(fake),
            Err(DagError::NodeNotFound(_))
        ));
        assert!(matches!(
            dag.mark_completed(fake, json!("x")),
            Err(DagError::NodeNotFound(_))
        ));
        assert!(matches!(
            dag.mark_failed(fake, "x"),
            Err(DagError::NodeNotFound(_))
        ));
        assert!(matches!(
            dag.mark_skipped(fake),
            Err(DagError::NodeNotFound(_))
        ));
    }

    // --- DagExecutor ---

    #[test]
    fn executor_empty_dag_returns_none() {
        let dag = ExecutionDag::new();
        let mut executor = DagExecutor::new(dag);
        assert_eq!(executor.next_ready(), None);
        assert!(executor.is_complete());
    }

    #[test]
    fn executor_single_node_lifecycle() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let mut executor = DagExecutor::new(dag);

        let batch = executor.next_ready().expect("has ready");
        assert_eq!(batch, vec![a]);
        // Node should now be Running.
        assert!(matches!(
            executor.dag().get_node(a).expect("exists").state,
            NodeState::Running
        ));

        executor
            .complete_node(a, json!("result"))
            .expect("complete");
        assert!(executor.is_complete());
        assert_eq!(executor.next_ready(), None);
    }

    #[test]
    fn executor_linear_chain() {
        let dag = sequential(vec![
            ("a".into(), StepKind::ContextAssembly),
            ("b".into(), StepKind::ModelInference),
            ("c".into(), StepKind::OutputParsing),
        ]);
        let mut executor = DagExecutor::new(dag);

        // First batch: only A.
        let batch = executor.next_ready().expect("has ready");
        assert_eq!(batch.len(), 1);
        executor
            .complete_node(batch[0], json!("a"))
            .expect("complete a");

        // Second batch: only B.
        let batch = executor.next_ready().expect("has ready");
        assert_eq!(batch.len(), 1);
        executor
            .complete_node(batch[0], json!("b"))
            .expect("complete b");

        // Third batch: only C.
        let batch = executor.next_ready().expect("has ready");
        assert_eq!(batch.len(), 1);
        executor
            .complete_node(batch[0], json!("c"))
            .expect("complete c");

        assert!(executor.is_complete());
    }

    #[test]
    fn executor_fail_node_skips_dependents() {
        let dag = sequential(vec![
            ("a".into(), StepKind::ContextAssembly),
            ("b".into(), StepKind::ModelInference),
            ("c".into(), StepKind::OutputParsing),
        ]);
        let mut executor = DagExecutor::new(dag);

        let batch = executor.next_ready().expect("has ready");
        executor
            .fail_node(batch[0], "provider error")
            .expect("fail a");

        assert!(executor.is_complete());
        assert!(executor.has_failed());

        // B and C should be skipped.
        let b = executor.dag().get_node(NodeId(1)).expect("b exists");
        assert_eq!(b.state, NodeState::Skipped);
        let c = executor.dag().get_node(NodeId(2)).expect("c exists");
        assert_eq!(c.state, NodeState::Skipped);
    }

    #[test]
    fn executor_parallel_batch() {
        let dag = parallel(vec![
            ("a".into(), StepKind::ContextAssembly),
            ("b".into(), StepKind::ModelInference),
            ("c".into(), StepKind::ToolInvocation),
        ]);
        let mut executor = DagExecutor::new(dag);

        let batch = executor.next_ready().expect("has ready");
        assert_eq!(batch.len(), 3);
    }

    // --- Workflow builders ---

    #[test]
    fn sequential_builder_creates_chain() {
        let dag = sequential(vec![
            ("a".into(), StepKind::ContextAssembly),
            ("b".into(), StepKind::ModelInference),
            ("c".into(), StepKind::OutputParsing),
        ]);
        assert_eq!(dag.node_count(), 3);
        assert_eq!(dag.edges().len(), 2);

        let order = dag.topological_order().expect("no cycle");
        assert_eq!(order, vec![NodeId(0), NodeId(1), NodeId(2)]);
    }

    #[test]
    fn sequential_builder_empty() {
        let dag = sequential(vec![]);
        assert_eq!(dag.node_count(), 0);
        assert!(dag.is_complete());
    }

    #[test]
    fn sequential_builder_single() {
        let dag = sequential(vec![("only".into(), StepKind::ContextAssembly)]);
        assert_eq!(dag.node_count(), 1);
        assert_eq!(dag.edges().len(), 0);
    }

    #[test]
    fn parallel_builder_no_edges() {
        let dag = parallel(vec![
            ("a".into(), StepKind::ContextAssembly),
            ("b".into(), StepKind::ModelInference),
        ]);
        assert_eq!(dag.node_count(), 2);
        assert!(dag.edges().is_empty());
    }

    #[test]
    fn fan_out_fan_in_structure() {
        let dag = fan_out_fan_in(
            ("source".into(), StepKind::ContextAssembly),
            vec![
                ("t1".into(), StepKind::ModelInference),
                ("t2".into(), StepKind::ToolInvocation),
                ("t3".into(), StepKind::PolicyEvaluation),
            ],
            ("sink".into(), StepKind::OutputParsing),
        );

        assert_eq!(dag.node_count(), 5);
        // 3 edges from source to targets + 3 edges from targets to sink.
        assert_eq!(dag.edges().len(), 6);

        let order = dag.topological_order().expect("no cycle");
        // Source must be first, sink must be last.
        assert_eq!(order[0], NodeId(0)); // source
        assert_eq!(*order.last().expect("has last"), NodeId(4)); // sink
    }

    #[test]
    fn fan_out_fan_in_execution() {
        let dag = fan_out_fan_in(
            ("source".into(), StepKind::ContextAssembly),
            vec![
                ("t1".into(), StepKind::ModelInference),
                ("t2".into(), StepKind::ToolInvocation),
            ],
            ("sink".into(), StepKind::OutputParsing),
        );
        let mut executor = DagExecutor::new(dag);

        // Step 1: source.
        let batch = executor.next_ready().expect("source ready");
        assert_eq!(batch.len(), 1);
        executor
            .complete_node(batch[0], json!("source-done"))
            .expect("complete source");

        // Step 2: both targets in parallel.
        let batch = executor.next_ready().expect("targets ready");
        assert_eq!(batch.len(), 2);
        for id in &batch {
            executor
                .complete_node(*id, json!("target-done"))
                .expect("complete target");
        }

        // Step 3: sink.
        let batch = executor.next_ready().expect("sink ready");
        assert_eq!(batch.len(), 1);
        executor
            .complete_node(batch[0], json!("sink-done"))
            .expect("complete sink");

        assert!(executor.is_complete());
        assert!(!executor.has_failed());
    }

    // --- Validation ---

    #[test]
    fn validate_valid_dag() {
        let dag = sequential(vec![
            ("a".into(), StepKind::ContextAssembly),
            ("b".into(), StepKind::ModelInference),
        ]);
        dag.validate().expect("valid");
    }

    // --- add_dependency convenience ---

    #[test]
    fn add_dependency_creates_correct_edge() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let b = dag.add_node("b", StepKind::ModelInference);

        dag.add_dependency(b, a).expect("b depends on a");

        // a should be ready, b should not.
        assert_eq!(dag.ready_nodes(), vec![a]);

        // Complete a, now b should be ready.
        dag.mark_completed(a, json!("a-done")).expect("complete a");
        assert_eq!(dag.ready_nodes(), vec![b]);
    }

    // --- NodeState::is_terminal ---

    #[test]
    fn node_state_terminal_checks() {
        assert!(!NodeState::Pending.is_terminal());
        assert!(!NodeState::Ready.is_terminal());
        assert!(!NodeState::Running.is_terminal());
        assert!(NodeState::Completed {
            result: json!(null)
        }
        .is_terminal());
        assert!(NodeState::Failed { error: "x".into() }.is_terminal());
        assert!(NodeState::Skipped.is_terminal());
    }

    // --- Node count and get_node ---

    #[test]
    fn node_count_tracks_additions() {
        let mut dag = ExecutionDag::new();
        assert_eq!(dag.node_count(), 0);
        dag.add_node("a", StepKind::ContextAssembly);
        assert_eq!(dag.node_count(), 1);
        dag.add_node("b", StepKind::ModelInference);
        assert_eq!(dag.node_count(), 2);
    }

    #[test]
    fn get_node_returns_none_for_missing() {
        let dag = ExecutionDag::new();
        assert!(dag.get_node(NodeId(0)).is_none());
    }

    #[test]
    fn get_node_returns_correct_node() {
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("test-label", StepKind::ContextAssembly);
        let node = dag.get_node(a).expect("should exist");
        assert_eq!(node.label, "test-label");
        assert_eq!(node.id, a);
    }

    // --- DagError Display ---

    #[test]
    fn dag_error_display() {
        let err = DagError::CycleDetected {
            nodes: vec![NodeId(0), NodeId(1)],
        };
        let msg = err.to_string();
        assert!(msg.contains("cycle detected"));

        let err = DagError::NodeNotFound(NodeId(42));
        assert!(err.to_string().contains("42"));

        let err = DagError::InvalidEdge {
            reason: "test reason".into(),
        };
        assert!(err.to_string().contains("test reason"));

        let err = DagError::AlreadyTerminal(NodeId(7));
        assert!(err.to_string().contains("terminal"));
    }

    // --- Complex scenarios ---

    #[test]
    fn wide_fan_out_fan_in() {
        let targets: Vec<(String, StepKind)> = (0..10)
            .map(|i| (format!("worker-{i}"), StepKind::ToolInvocation))
            .collect();
        let dag = fan_out_fan_in(
            ("dispatch".into(), StepKind::ContextAssembly),
            targets,
            ("aggregate".into(), StepKind::OutputParsing),
        );

        assert_eq!(dag.node_count(), 12); // 1 source + 10 workers + 1 sink
        assert_eq!(dag.edges().len(), 20); // 10 from source + 10 to sink
        dag.validate().expect("valid");
    }

    #[test]
    fn mixed_dag_topology() {
        // Build: A -> B -> D
        //        A -> C -> D
        //        E (independent)
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let b = dag.add_node("b", StepKind::ModelInference);
        let c = dag.add_node("c", StepKind::ToolInvocation);
        let d = dag.add_node("d", StepKind::OutputParsing);
        let e = dag.add_node("e", StepKind::Delivery);

        dag.add_edge(a, b).expect("a->b");
        dag.add_edge(a, c).expect("a->c");
        dag.add_edge(b, d).expect("b->d");
        dag.add_edge(c, d).expect("c->d");

        // A and E should be ready initially.
        let mut ready = dag.ready_nodes();
        ready.sort_by_key(|id| id.0);
        assert_eq!(ready, vec![a, e]);

        dag.validate().expect("valid");
    }

    #[test]
    fn executor_fail_in_diamond_skips_sink() {
        //     A
        //    / \
        //   B   C
        //    \ /
        //     D
        let mut dag = ExecutionDag::new();
        let a = dag.add_node("a", StepKind::ContextAssembly);
        let b = dag.add_node("b", StepKind::ModelInference);
        let c = dag.add_node("c", StepKind::ToolInvocation);
        let d = dag.add_node("d", StepKind::OutputParsing);
        dag.add_edge(a, b).expect("a->b");
        dag.add_edge(a, c).expect("a->c");
        dag.add_edge(b, d).expect("b->d");
        dag.add_edge(c, d).expect("c->d");

        let mut executor = DagExecutor::new(dag);

        // Run A.
        let batch = executor.next_ready().expect("a ready");
        executor
            .complete_node(batch[0], json!("a-done"))
            .expect("complete a");

        // next_ready returns both B and C, marking them both Running.
        let batch = executor.next_ready().expect("b,c ready");
        assert_eq!(batch.len(), 2);
        let b_id = batch.iter().find(|id| id.0 == 1).copied().expect("b");
        let c_id = batch.iter().find(|id| id.0 == 2).copied().expect("c");

        // Fail B: D should be skipped because B failed.
        executor.fail_node(b_id, "b failed").expect("fail b");

        let d_node = executor.dag().get_node(d).expect("d exists");
        assert_eq!(d_node.state, NodeState::Skipped);

        // C was already marked Running by next_ready, so it is still
        // running (not a dependent of B).
        let c_node = executor.dag().get_node(c_id).expect("c exists");
        assert_eq!(c_node.state, NodeState::Running);

        // Complete C to finish the DAG.
        executor
            .complete_node(c_id, json!("c-done"))
            .expect("complete c");

        assert!(executor.is_complete());
        assert!(executor.has_failed());
    }

    // --- Default trait ---

    #[test]
    fn execution_dag_default() {
        let dag = ExecutionDag::default();
        assert_eq!(dag.node_count(), 0);
        assert!(dag.is_complete());
    }
}

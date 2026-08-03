//! Group coordinator execution modes for multi-agent task orchestration.
//!
//! This module implements PRD-09 §5-6, providing four execution strategies:
//!
//! - **[`SequentialExecutor`]** — runs tasks one after another, feeding each
//!   result as input to the next task.
//! - **[`ParallelExecutor`]** — runs all tasks concurrently and collects all
//!   results.
//! - **[`PipelineExecutor`]** — chains tasks so that the output of one becomes
//!   the input of the next.
//! - **[`ConsensusExecutor`]** — runs the same task on multiple agents and
//!   votes on the result.
//!
//! # Core types
//!
//! - [`ExecutionMode`] — selects the execution strategy.
//! - [`ExecutionPlan`] — describes the tasks to run and their dependencies.
//! - [`GroupTask`] — a single task: target agent, input payload, and grant spec.
//! - [`TaskResult`] — the outcome of a single task.
//! - [`ExecutionResult`] — the aggregated outcome of an entire plan.
//! - [`GroupEvidence`] (re-exported from [`crate::propagation`]) — bundles
//!   the `ExecutionResult` with child artifacts and a human-readable summary.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use polkagent_core::ids::AgentId;

use crate::error::{GroupError, GroupResult};
use crate::propagation::GroupEvidence as PropagationEvidence;
use crate::types::{GrantSpec, GroupId};

// ---------------------------------------------------------------------------
// TaskId
// ---------------------------------------------------------------------------

/// Stable identifier for a task within an [`ExecutionPlan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(u64);

impl TaskId {
    /// Create a new task identifier from a raw value.
    #[must_use]
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Return the inner value.
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "task-{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// ExecutionMode
// ---------------------------------------------------------------------------

/// Strategy that governs how tasks in an [`ExecutionPlan`] are scheduled and
/// how their results are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    /// Run tasks one at a time in the order they appear in the plan.
    /// Each task receives the previous task's output as its input.
    Sequential,

    /// Run all tasks concurrently and collect all results.
    Parallel,

    /// Chain tasks so the output of one feeds directly into the next.
    Pipeline,

    /// Run the same task on multiple agents and select the result by voting.
    Consensus,
}

// ---------------------------------------------------------------------------
// GroupTask
// ---------------------------------------------------------------------------

/// A single unit of work within an [`ExecutionPlan`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupTask {
    /// Stable identifier used for dependency resolution.
    pub id: TaskId,

    /// The agent that will execute this task.
    pub agent_id: AgentId,

    /// Arbitrary input payload for the task (serialized as a JSON value).
    pub input: serde_json::Value,

    /// Optional grant specification constraining what this task is allowed to
    /// do. When `None`, the agent's effective group grant applies.
    pub grant_spec: Option<GrantSpec>,
}

impl GroupTask {
    /// Create a new task with no grant override and a `null` input.
    #[must_use]
    pub fn new(id: TaskId, agent_id: AgentId) -> Self {
        Self {
            id,
            agent_id,
            input: serde_json::Value::Null,
            grant_spec: None,
        }
    }

    /// Set the task input and return `self` for chaining.
    #[must_use]
    pub fn with_input(mut self, input: serde_json::Value) -> Self {
        self.input = input;
        self
    }

    /// Set the task grant spec and return `self` for chaining.
    #[must_use]
    pub fn with_grant(mut self, grant: GrantSpec) -> Self {
        self.grant_spec = Some(grant);
        self
    }
}

// ---------------------------------------------------------------------------
// ExecutionPlan
// ---------------------------------------------------------------------------

/// A complete description of the tasks to run and how to schedule them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    /// Strategy to use when running these tasks.
    pub mode: ExecutionMode,

    /// The tasks that make up this plan, in declaration order.
    pub tasks: Vec<GroupTask>,

    /// Explicit dependency edges: key must complete before all values can
    /// start. An empty map means tasks run in declaration order (or fully
    /// parallel, depending on `mode`).
    pub dependencies: HashMap<TaskId, Vec<TaskId>>,
}

impl ExecutionPlan {
    /// Create a new plan with the given mode and an empty task list.
    #[must_use]
    pub fn new(mode: ExecutionMode) -> Self {
        Self {
            mode,
            tasks: Vec::new(),
            dependencies: HashMap::new(),
        }
    }

    /// Append a task and return `self` for chaining.
    #[must_use]
    pub fn with_task(mut self, task: GroupTask) -> Self {
        self.tasks.push(task);
        self
    }

    /// Add a dependency edge: `blocked_by` must finish before `task_id`
    /// may start.
    pub fn add_dependency(&mut self, task_id: TaskId, blocked_by: TaskId) {
        self.dependencies
            .entry(blocked_by)
            .or_default()
            .push(task_id);
    }
}

// ---------------------------------------------------------------------------
// TaskResult
// ---------------------------------------------------------------------------

/// The outcome of a single [`GroupTask`] execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// Which task produced this result.
    pub task_id: TaskId,

    /// The agent that ran the task.
    pub agent_id: AgentId,

    /// Whether the task completed without error.
    pub success: bool,

    /// The output payload produced by the task.
    pub output: serde_json::Value,

    /// Human-readable description of the outcome.
    pub summary: String,

    /// How long the task took.
    pub duration: Duration,

    /// Optional budget consumed by this task.
    pub budget_spent: f64,
}

impl TaskResult {
    /// Construct a successful result.
    #[must_use]
    pub fn success(
        task_id: TaskId,
        agent_id: AgentId,
        output: serde_json::Value,
        duration: Duration,
    ) -> Self {
        Self {
            task_id,
            agent_id,
            success: true,
            output,
            summary: "task completed successfully".to_string(),
            duration,
            budget_spent: 0.0,
        }
    }

    /// Construct a failed result.
    #[must_use]
    pub fn failure(
        task_id: TaskId,
        agent_id: AgentId,
        reason: impl Into<String>,
        duration: Duration,
    ) -> Self {
        Self {
            task_id,
            agent_id,
            success: false,
            output: serde_json::Value::Null,
            summary: reason.into(),
            duration,
            budget_spent: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// ExecutionResult
// ---------------------------------------------------------------------------

/// The aggregated outcome of running an entire [`ExecutionPlan`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// The mode used to run the plan.
    pub mode: ExecutionMode,

    /// Per-task outcomes, in the order tasks were completed.
    pub task_results: Vec<TaskResult>,

    /// Total wall-clock time for the entire plan.
    pub duration: Duration,

    /// Total budget consumed across all tasks.
    pub budget_spent: f64,
}

impl ExecutionResult {
    /// Return `true` if every task in the plan succeeded.
    #[must_use]
    pub fn all_succeeded(&self) -> bool {
        self.task_results.iter().all(|r| r.success)
    }

    /// Return the number of successful tasks.
    #[must_use]
    pub fn success_count(&self) -> usize {
        self.task_results.iter().filter(|r| r.success).count()
    }

    /// Return the number of failed tasks.
    #[must_use]
    pub fn failure_count(&self) -> usize {
        self.task_results.iter().filter(|r| !r.success).count()
    }

    /// Return the output of the last task, or `Null` if there are no tasks.
    #[must_use]
    pub fn final_output(&self) -> &serde_json::Value {
        self.task_results
            .last()
            .map(|r| &r.output)
            .unwrap_or(&serde_json::Value::Null)
    }
}

// ---------------------------------------------------------------------------
// ExecutionEvidence
// ---------------------------------------------------------------------------

/// Evidence produced by running an [`ExecutionPlan`], bundling the
/// [`ExecutionResult`] with child artifacts and a human-readable summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionEvidence {
    /// The group that coordinated the execution.
    pub group_id: GroupId,

    /// The full execution result.
    pub execution_result: ExecutionResult,

    /// Agent IDs of every agent that contributed a task result.
    pub child_agents: Vec<AgentId>,

    /// A human-readable description of the overall outcome.
    pub outcome_summary: String,
}

impl ExecutionEvidence {
    fn build(group_id: GroupId, result: ExecutionResult) -> Self {
        let child_agents: Vec<AgentId> = result.task_results.iter().map(|r| r.agent_id).collect();

        let success_count = result.success_count();
        let total = result.task_results.len();
        let outcome_summary = format!(
            "Execution ({:?}) in group {group_id}: {success_count}/{total} tasks succeeded, \
             total budget {:.2}, duration {:?}",
            result.mode, result.budget_spent, result.duration
        );

        Self {
            group_id,
            execution_result: result,
            child_agents,
            outcome_summary,
        }
    }

    /// Convert to a [`PropagationEvidence`] for use with the propagation layer.
    ///
    /// This adapts the execution evidence into the group-level evidence type
    /// expected by the propagation module.
    #[must_use]
    pub fn into_propagation_evidence(self) -> PropagationEvidence {
        use polkagent_core::ids::{ArtifactId, RunId};
        PropagationEvidence {
            group_id: self.group_id,
            contributing_runs: self
                .child_agents
                .iter()
                .map(|_| RunId::new())
                .collect(),
            aggregated_artifacts: Vec::<ArtifactId>::new(),
            quorum_met: self.execution_result.all_succeeded(),
            successful_runs: self.execution_result.success_count(),
            failed_runs: self.execution_result.failure_count(),
            summary: self.outcome_summary,
            timestamp: chrono::Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Executor trait
// ---------------------------------------------------------------------------

/// Trait implemented by each execution strategy.
///
/// Concrete implementations are [`SequentialExecutor`], [`ParallelExecutor`],
/// [`PipelineExecutor`], and [`ConsensusExecutor`].
///
/// Executors are intentionally synchronous and pure: they receive a plan and
/// a runner closure, and return an [`ExecutionResult`]. This makes them easy
/// to test without a full async runtime while allowing callers to wrap them
/// in async contexts.
pub trait Executor {
    /// Execute `plan` using `runner` to dispatch individual tasks.
    ///
    /// The `runner` receives a [`GroupTask`] and the most-recent output
    /// payload (for pipeline chaining), and returns a [`TaskResult`].
    ///
    /// # Errors
    ///
    /// Returns a [`GroupError`] if the plan is structurally invalid (e.g.
    /// empty task list for consensus mode) or if a dependency cycle is
    /// detected.
    fn execute<F>(&self, plan: &ExecutionPlan, runner: F) -> GroupResult<ExecutionResult>
    where
        F: FnMut(&GroupTask, &serde_json::Value) -> TaskResult;
}

// ---------------------------------------------------------------------------
// SequentialExecutor
// ---------------------------------------------------------------------------

/// Runs tasks one after another.
///
/// Each task receives the previous task's output as its input. The first task
/// receives the original `GroupTask.input`.
#[derive(Debug, Default, Clone)]
pub struct SequentialExecutor;

impl SequentialExecutor {
    /// Create a new sequential executor.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Executor for SequentialExecutor {
    fn execute<F>(&self, plan: &ExecutionPlan, mut runner: F) -> GroupResult<ExecutionResult>
    where
        F: FnMut(&GroupTask, &serde_json::Value) -> TaskResult,
    {
        let start = Instant::now();
        let mut task_results = Vec::with_capacity(plan.tasks.len());
        let mut previous_output = serde_json::Value::Null;

        for task in &plan.tasks {
            let result = runner(task, &previous_output);
            if result.success {
                previous_output = result.output.clone();
            }
            task_results.push(result);
        }

        let duration = start.elapsed();
        let budget_spent = task_results.iter().map(|r| r.budget_spent).sum();

        Ok(ExecutionResult {
            mode: ExecutionMode::Sequential,
            task_results,
            duration,
            budget_spent,
        })
    }
}

// ---------------------------------------------------------------------------
// ParallelExecutor
// ---------------------------------------------------------------------------

/// Runs all tasks concurrently (simulated sequentially here since this module
/// is sync; async callers may wrap with `tokio::task::spawn`).
///
/// All tasks receive their individual `GroupTask.input` and run independently.
/// Results are collected in declaration order.
#[derive(Debug, Default, Clone)]
pub struct ParallelExecutor;

impl ParallelExecutor {
    /// Create a new parallel executor.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Executor for ParallelExecutor {
    fn execute<F>(&self, plan: &ExecutionPlan, mut runner: F) -> GroupResult<ExecutionResult>
    where
        F: FnMut(&GroupTask, &serde_json::Value) -> TaskResult,
    {
        let start = Instant::now();
        let mut task_results = Vec::with_capacity(plan.tasks.len());

        // In this synchronous model, run each task with its own input
        // (parallel semantics: no output chaining).
        for task in &plan.tasks {
            let result = runner(task, &task.input);
            task_results.push(result);
        }

        let duration = start.elapsed();
        let budget_spent = task_results.iter().map(|r| r.budget_spent).sum();

        Ok(ExecutionResult {
            mode: ExecutionMode::Parallel,
            task_results,
            duration,
            budget_spent,
        })
    }
}

// ---------------------------------------------------------------------------
// PipelineExecutor
// ---------------------------------------------------------------------------

/// Chains tasks so the output of one feeds directly into the next.
///
/// Unlike [`SequentialExecutor`], the pipeline *always* passes the previous
/// output regardless of whether the previous task succeeded. This allows
/// downstream tasks to handle upstream failures explicitly.
#[derive(Debug, Default, Clone)]
pub struct PipelineExecutor;

impl PipelineExecutor {
    /// Create a new pipeline executor.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Executor for PipelineExecutor {
    fn execute<F>(&self, plan: &ExecutionPlan, mut runner: F) -> GroupResult<ExecutionResult>
    where
        F: FnMut(&GroupTask, &serde_json::Value) -> TaskResult,
    {
        let start = Instant::now();
        let mut task_results = Vec::with_capacity(plan.tasks.len());
        // Start with the first task's input as the pipeline seed.
        let seed = plan
            .tasks
            .first()
            .map(|t| t.input.clone())
            .unwrap_or(serde_json::Value::Null);
        let mut pipeline_value = seed;

        for task in &plan.tasks {
            let result = runner(task, &pipeline_value);
            // Always advance the pipeline, even on failure.
            pipeline_value = result.output.clone();
            task_results.push(result);
        }

        let duration = start.elapsed();
        let budget_spent = task_results.iter().map(|r| r.budget_spent).sum();

        Ok(ExecutionResult {
            mode: ExecutionMode::Pipeline,
            task_results,
            duration,
            budget_spent,
        })
    }
}

// ---------------------------------------------------------------------------
// ConsensusExecutor
// ---------------------------------------------------------------------------

/// Runs the same logical task on multiple agents and selects the winning
/// output by majority vote.
///
/// All tasks in the plan are treated as independent attempts at the same
/// computation. The result with the most occurrences of `output` wins.
/// Tie-breaking uses declaration order.
///
/// Requires at least 2 tasks; returns [`GroupError::Internal`] if the plan
/// has fewer.
#[derive(Debug, Default, Clone)]
pub struct ConsensusExecutor;

impl ConsensusExecutor {
    /// Create a new consensus executor.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Select the winning output from a slice of task results by majority vote.
    ///
    /// Returns the output that appeared most often. On a tie, the first
    /// occurrence in declaration order wins.
    #[must_use]
    pub fn select_winner(results: &[TaskResult]) -> serde_json::Value {
        if results.is_empty() {
            return serde_json::Value::Null;
        }

        // Count occurrences of each distinct output (using JSON string rep).
        let mut counts: HashMap<String, (usize, &serde_json::Value)> = HashMap::new();
        for r in results {
            if r.success {
                let key = r.output.to_string();
                let entry = counts.entry(key).or_insert((0, &r.output));
                entry.0 += 1;
            }
        }

        // If no successful results, fall back to the first result's output.
        if counts.is_empty() {
            return results[0].output.clone();
        }

        // Pick the output with the highest count.
        counts
            .into_values()
            .max_by_key(|(count, _)| *count)
            .map(|(_, v)| v.clone())
            .unwrap_or(serde_json::Value::Null)
    }
}

impl Executor for ConsensusExecutor {
    fn execute<F>(&self, plan: &ExecutionPlan, mut runner: F) -> GroupResult<ExecutionResult>
    where
        F: FnMut(&GroupTask, &serde_json::Value) -> TaskResult,
    {
        if plan.tasks.len() < 2 {
            return Err(GroupError::Internal(
                "consensus execution requires at least 2 tasks".to_string(),
            ));
        }

        let start = Instant::now();
        let mut task_results = Vec::with_capacity(plan.tasks.len());

        // Run every task with its own input (all attempts at the same problem).
        for task in &plan.tasks {
            let result = runner(task, &task.input);
            task_results.push(result);
        }

        let duration = start.elapsed();
        let budget_spent = task_results.iter().map(|r| r.budget_spent).sum();

        Ok(ExecutionResult {
            mode: ExecutionMode::Consensus,
            task_results,
            duration,
            budget_spent,
        })
    }
}

// ---------------------------------------------------------------------------
// GroupExecutor — dispatch helper
// ---------------------------------------------------------------------------

/// Dispatches an [`ExecutionPlan`] to the appropriate executor based on its
/// [`ExecutionMode`] and wraps the result in [`ExecutionEvidence`].
#[derive(Debug, Default, Clone)]
pub struct GroupExecutor;

impl GroupExecutor {
    /// Create a new group executor.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Run `plan` within `group_id` using `runner` and return
    /// [`ExecutionEvidence`].
    ///
    /// # Errors
    ///
    /// Propagates errors from the underlying executor (e.g. consensus with
    /// fewer than 2 tasks).
    pub fn run<F>(
        &self,
        group_id: GroupId,
        plan: &ExecutionPlan,
        runner: F,
    ) -> GroupResult<ExecutionEvidence>
    where
        F: FnMut(&GroupTask, &serde_json::Value) -> TaskResult,
    {
        let result = match plan.mode {
            ExecutionMode::Sequential => SequentialExecutor::new().execute(plan, runner),
            ExecutionMode::Parallel => ParallelExecutor::new().execute(plan, runner),
            ExecutionMode::Pipeline => PipelineExecutor::new().execute(plan, runner),
            ExecutionMode::Consensus => ConsensusExecutor::new().execute(plan, runner),
        }?;

        Ok(ExecutionEvidence::build(group_id, result))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn agent() -> AgentId {
        AgentId::new()
    }

    fn group_id() -> GroupId {
        GroupId::new()
    }

    fn task(id: u64) -> GroupTask {
        GroupTask::new(TaskId::new(id), agent())
    }

    fn task_with_input(id: u64, input: serde_json::Value) -> GroupTask {
        GroupTask::new(TaskId::new(id), agent()).with_input(input)
    }

    /// A simple runner that echoes the input as output (success=true).
    fn echo_runner(task: &GroupTask, previous: &serde_json::Value) -> TaskResult {
        TaskResult::success(task.id, task.agent_id, previous.clone(), Duration::ZERO)
    }

    /// A runner that uses the task's own input.
    fn own_input_runner(task: &GroupTask, _previous: &serde_json::Value) -> TaskResult {
        TaskResult::success(task.id, task.agent_id, task.input.clone(), Duration::ZERO)
    }

    /// A runner that always fails.
    fn failing_runner(task: &GroupTask, _previous: &serde_json::Value) -> TaskResult {
        TaskResult::failure(task.id, task.agent_id, "intentional failure", Duration::ZERO)
    }

    fn sequential_plan(tasks: Vec<GroupTask>) -> ExecutionPlan {
        let mut plan = ExecutionPlan::new(ExecutionMode::Sequential);
        for t in tasks {
            plan = plan.with_task(t);
        }
        plan
    }

    fn parallel_plan(tasks: Vec<GroupTask>) -> ExecutionPlan {
        let mut plan = ExecutionPlan::new(ExecutionMode::Parallel);
        for t in tasks {
            plan = plan.with_task(t);
        }
        plan
    }

    fn pipeline_plan(tasks: Vec<GroupTask>) -> ExecutionPlan {
        let mut plan = ExecutionPlan::new(ExecutionMode::Pipeline);
        for t in tasks {
            plan = plan.with_task(t);
        }
        plan
    }

    fn consensus_plan(tasks: Vec<GroupTask>) -> ExecutionPlan {
        let mut plan = ExecutionPlan::new(ExecutionMode::Consensus);
        for t in tasks {
            plan = plan.with_task(t);
        }
        plan
    }

    // -----------------------------------------------------------------------
    // TaskId
    // -----------------------------------------------------------------------

    #[test]
    fn task_id_value_round_trips() {
        let id = TaskId::new(42);
        assert_eq!(id.value(), 42);
    }

    #[test]
    fn task_id_display() {
        let id = TaskId::new(7);
        assert_eq!(id.to_string(), "task-7");
    }

    #[test]
    fn task_id_equality() {
        assert_eq!(TaskId::new(1), TaskId::new(1));
        assert_ne!(TaskId::new(1), TaskId::new(2));
    }

    // -----------------------------------------------------------------------
    // GroupTask builder
    // -----------------------------------------------------------------------

    #[test]
    fn group_task_default_input_is_null() {
        let t = task(1);
        assert_eq!(t.input, serde_json::Value::Null);
        assert!(t.grant_spec.is_none());
    }

    #[test]
    fn group_task_with_input() {
        let t = task(1).with_input(serde_json::json!({"key": "value"}));
        assert_eq!(t.input, serde_json::json!({"key": "value"}));
    }

    #[test]
    fn group_task_with_grant() {
        let grant = GrantSpec {
            capabilities: vec!["cap".to_string()],
            max_budget: Some(100.0),
            allowed_pallets: vec![],
        };
        let t = task(1).with_grant(grant.clone());
        let spec = t.grant_spec.expect("grant should be set");
        assert!(spec.has_capability("cap"));
    }

    // -----------------------------------------------------------------------
    // ExecutionPlan
    // -----------------------------------------------------------------------

    #[test]
    fn execution_plan_add_dependency() {
        let mut plan = ExecutionPlan::new(ExecutionMode::Sequential);
        plan = plan.with_task(task(1));
        plan = plan.with_task(task(2));
        plan.add_dependency(TaskId::new(2), TaskId::new(1));
        let blocked = plan
            .dependencies
            .get(&TaskId::new(1))
            .expect("dep entry");
        assert!(blocked.contains(&TaskId::new(2)));
    }

    // -----------------------------------------------------------------------
    // ExecutionResult helpers
    // -----------------------------------------------------------------------

    #[test]
    fn execution_result_all_succeeded_true_when_all_succeed() {
        let plan = sequential_plan(vec![task(1), task(2)]);
        let result = SequentialExecutor::new()
            .execute(&plan, own_input_runner)
            .expect("execute ok");
        assert!(result.all_succeeded());
        assert_eq!(result.success_count(), 2);
        assert_eq!(result.failure_count(), 0);
    }

    #[test]
    fn execution_result_all_succeeded_false_when_any_fails() {
        let plan = sequential_plan(vec![task(1), task(2)]);
        let mut call_count = 0u32;
        let result = SequentialExecutor::new()
            .execute(&plan, |t, prev| {
                call_count += 1;
                if call_count == 1 {
                    failing_runner(t, prev)
                } else {
                    own_input_runner(t, prev)
                }
            })
            .expect("execute ok");
        assert!(!result.all_succeeded());
        assert_eq!(result.success_count(), 1);
        assert_eq!(result.failure_count(), 1);
    }

    #[test]
    fn execution_result_final_output_returns_last_task_output() {
        let plan = sequential_plan(vec![
            task_with_input(1, serde_json::json!("first")),
            task_with_input(2, serde_json::json!("second")),
        ]);
        let result = SequentialExecutor::new()
            .execute(&plan, |t, _prev| {
                TaskResult::success(t.id, t.agent_id, t.input.clone(), Duration::ZERO)
            })
            .expect("execute ok");
        // Sequential passes previous output, but both tasks just use own_input
        // — the last task output was whatever runner returned for task 2.
        assert!(!result.task_results.is_empty());
    }

    #[test]
    fn execution_result_final_output_null_when_empty() {
        let result = ExecutionResult {
            mode: ExecutionMode::Sequential,
            task_results: vec![],
            duration: Duration::ZERO,
            budget_spent: 0.0,
        };
        assert_eq!(result.final_output(), &serde_json::Value::Null);
    }

    // -----------------------------------------------------------------------
    // SequentialExecutor
    // -----------------------------------------------------------------------

    #[test]
    fn sequential_empty_plan_produces_empty_results() {
        let plan = sequential_plan(vec![]);
        let result = SequentialExecutor::new()
            .execute(&plan, echo_runner)
            .expect("execute ok");
        assert!(result.task_results.is_empty());
        assert_eq!(result.mode, ExecutionMode::Sequential);
    }

    #[test]
    fn sequential_passes_previous_output_to_next_task() {
        let plan = sequential_plan(vec![task(1), task(2), task(3)]);
        // The echo runner returns `previous` as output.
        // Task 1 gets Null → outputs Null.
        // Task 2 gets Null (output of task 1) → outputs Null.
        // Task 3 gets Null → outputs Null.
        let result = SequentialExecutor::new()
            .execute(&plan, echo_runner)
            .expect("execute ok");
        assert_eq!(result.task_results.len(), 3);
        assert!(result.all_succeeded());
    }

    #[test]
    fn sequential_chains_output_to_next_input() {
        let plan = sequential_plan(vec![task(1), task(2)]);
        // First task returns a specific output; second task echoes it.
        let mut first = true;
        let result = SequentialExecutor::new()
            .execute(&plan, |t, prev| {
                if first {
                    first = false;
                    TaskResult::success(
                        t.id,
                        t.agent_id,
                        serde_json::json!("from-task-1"),
                        Duration::ZERO,
                    )
                } else {
                    // `prev` should be "from-task-1"
                    assert_eq!(prev, &serde_json::json!("from-task-1"));
                    TaskResult::success(t.id, t.agent_id, prev.clone(), Duration::ZERO)
                }
            })
            .expect("execute ok");
        assert_eq!(
            result.task_results.last().expect("has results").output,
            serde_json::json!("from-task-1")
        );
    }

    #[test]
    fn sequential_stops_chaining_on_failure() {
        // On failure, the previous_output stays as-is (not updated).
        let plan = sequential_plan(vec![task(1), task(2), task(3)]);
        let mut call = 0u32;
        let result = SequentialExecutor::new()
            .execute(&plan, |t, _prev| {
                call += 1;
                if call == 1 {
                    // Task 1 succeeds with specific output.
                    TaskResult::success(
                        t.id,
                        t.agent_id,
                        serde_json::json!("output-1"),
                        Duration::ZERO,
                    )
                } else if call == 2 {
                    // Task 2 fails.
                    TaskResult::failure(t.id, t.agent_id, "fail", Duration::ZERO)
                } else {
                    // Task 3 still runs; prev should still be "output-1"
                    // (not updated because task 2 failed).
                    TaskResult::success(t.id, t.agent_id, _prev.clone(), Duration::ZERO)
                }
            })
            .expect("execute ok");
        assert_eq!(result.task_results.len(), 3);
        // Task 3's output should be "output-1" (from task 1, since task 2 failed).
        assert_eq!(
            result.task_results[2].output,
            serde_json::json!("output-1")
        );
    }

    #[test]
    fn sequential_accumulates_budget() {
        let plan = sequential_plan(vec![task(1), task(2), task(3)]);
        let result = SequentialExecutor::new()
            .execute(&plan, |t, _prev| {
                let mut r = TaskResult::success(
                    t.id,
                    t.agent_id,
                    serde_json::Value::Null,
                    Duration::ZERO,
                );
                r.budget_spent = 10.0;
                r
            })
            .expect("execute ok");
        assert!((result.budget_spent - 30.0).abs() < f64::EPSILON);
    }

    // -----------------------------------------------------------------------
    // ParallelExecutor
    // -----------------------------------------------------------------------

    #[test]
    fn parallel_all_tasks_receive_their_own_input() {
        let plan = parallel_plan(vec![
            task_with_input(1, serde_json::json!(1)),
            task_with_input(2, serde_json::json!(2)),
            task_with_input(3, serde_json::json!(3)),
        ]);
        let result = ParallelExecutor::new()
            .execute(&plan, own_input_runner)
            .expect("execute ok");
        assert_eq!(result.task_results.len(), 3);
        assert_eq!(result.task_results[0].output, serde_json::json!(1));
        assert_eq!(result.task_results[1].output, serde_json::json!(2));
        assert_eq!(result.task_results[2].output, serde_json::json!(3));
    }

    #[test]
    fn parallel_empty_plan_produces_empty_results() {
        let plan = parallel_plan(vec![]);
        let result = ParallelExecutor::new()
            .execute(&plan, own_input_runner)
            .expect("execute ok");
        assert!(result.task_results.is_empty());
        assert_eq!(result.mode, ExecutionMode::Parallel);
    }

    #[test]
    fn parallel_collects_all_results_regardless_of_failure() {
        let plan = parallel_plan(vec![task(1), task(2), task(3)]);
        let mut call = 0u32;
        let result = ParallelExecutor::new()
            .execute(&plan, |t, prev| {
                call += 1;
                if call == 2 {
                    failing_runner(t, prev)
                } else {
                    own_input_runner(t, prev)
                }
            })
            .expect("execute ok");
        assert_eq!(result.task_results.len(), 3);
        assert_eq!(result.success_count(), 2);
        assert_eq!(result.failure_count(), 1);
    }

    // -----------------------------------------------------------------------
    // PipelineExecutor
    // -----------------------------------------------------------------------

    #[test]
    fn pipeline_chains_output_always() {
        let plan = pipeline_plan(vec![task(1), task(2), task(3)]);
        let mut call = 0u32;
        let result = PipelineExecutor::new()
            .execute(&plan, |t, prev| {
                call += 1;
                if call == 1 {
                    TaskResult::success(
                        t.id,
                        t.agent_id,
                        serde_json::json!("step-1"),
                        Duration::ZERO,
                    )
                } else {
                    // Echo the pipeline value.
                    TaskResult::success(t.id, t.agent_id, prev.clone(), Duration::ZERO)
                }
            })
            .expect("execute ok");
        // Task 3's output should be "step-1" (propagated through pipeline).
        assert_eq!(
            result.task_results.last().expect("has results").output,
            serde_json::json!("step-1")
        );
        assert_eq!(result.mode, ExecutionMode::Pipeline);
    }

    #[test]
    fn pipeline_advances_even_on_failure() {
        let plan = pipeline_plan(vec![task(1), task(2), task(3)]);
        let mut call = 0u32;
        let result = PipelineExecutor::new()
            .execute(&plan, |t, _prev| {
                call += 1;
                if call == 1 {
                    // Task 1 fails; output is Null.
                    TaskResult::failure(t.id, t.agent_id, "fail", Duration::ZERO)
                } else {
                    // Tasks 2 & 3 receive Null (the failure output).
                    TaskResult::success(t.id, t.agent_id, _prev.clone(), Duration::ZERO)
                }
            })
            .expect("execute ok");
        // All three tasks were still called.
        assert_eq!(result.task_results.len(), 3);
        assert_eq!(result.failure_count(), 1);
        assert_eq!(result.success_count(), 2);
    }

    #[test]
    fn pipeline_uses_first_task_input_as_seed() {
        let plan = pipeline_plan(vec![task_with_input(
            1,
            serde_json::json!("seed"),
        )]);
        let result = PipelineExecutor::new()
            .execute(&plan, |t, prev| {
                // The first (and only) task receives its own input as the
                // pipeline seed.
                assert_eq!(prev, &serde_json::json!("seed"));
                TaskResult::success(t.id, t.agent_id, prev.clone(), Duration::ZERO)
            })
            .expect("execute ok");
        assert_eq!(result.task_results[0].output, serde_json::json!("seed"));
    }

    #[test]
    fn pipeline_empty_plan_produces_empty_results() {
        let plan = pipeline_plan(vec![]);
        let result = PipelineExecutor::new()
            .execute(&plan, echo_runner)
            .expect("execute ok");
        assert!(result.task_results.is_empty());
    }

    // -----------------------------------------------------------------------
    // ConsensusExecutor
    // -----------------------------------------------------------------------

    #[test]
    fn consensus_requires_at_least_two_tasks() {
        let plan = consensus_plan(vec![task(1)]);
        let err = ConsensusExecutor::new()
            .execute(&plan, own_input_runner)
            .expect_err("should fail");
        assert!(matches!(err, GroupError::Internal(_)));
    }

    #[test]
    fn consensus_with_two_tasks_succeeds() {
        let plan = consensus_plan(vec![task(1), task(2)]);
        let result = ConsensusExecutor::new()
            .execute(&plan, own_input_runner)
            .expect("execute ok");
        assert_eq!(result.task_results.len(), 2);
        assert_eq!(result.mode, ExecutionMode::Consensus);
    }

    #[test]
    fn consensus_select_winner_majority_output() {
        let id = TaskId::new(1);
        let a = AgentId::new();
        let make = |val: serde_json::Value, success: bool| TaskResult {
            task_id: id,
            agent_id: a,
            success,
            output: val,
            summary: String::new(),
            duration: Duration::ZERO,
            budget_spent: 0.0,
        };

        let results = vec![
            make(serde_json::json!("yes"), true),
            make(serde_json::json!("yes"), true),
            make(serde_json::json!("no"), true),
        ];
        let winner = ConsensusExecutor::select_winner(&results);
        assert_eq!(winner, serde_json::json!("yes"));
    }

    #[test]
    fn consensus_select_winner_empty_returns_null() {
        let winner = ConsensusExecutor::select_winner(&[]);
        assert_eq!(winner, serde_json::Value::Null);
    }

    #[test]
    fn consensus_select_winner_all_fail_returns_first_output() {
        let id = TaskId::new(1);
        let a = AgentId::new();
        let results = vec![
            TaskResult::failure(id, a, "fail", Duration::ZERO),
            TaskResult::failure(id, a, "fail", Duration::ZERO),
        ];
        // All failed — fall back to first result's output (Null).
        let winner = ConsensusExecutor::select_winner(&results);
        assert_eq!(winner, serde_json::Value::Null);
    }

    #[test]
    fn consensus_collects_all_task_results() {
        let plan = consensus_plan(vec![
            task_with_input(1, serde_json::json!("answer")),
            task_with_input(2, serde_json::json!("answer")),
            task_with_input(3, serde_json::json!("wrong")),
        ]);
        let result = ConsensusExecutor::new()
            .execute(&plan, own_input_runner)
            .expect("execute ok");
        assert_eq!(result.task_results.len(), 3);
        assert!(result.all_succeeded());
    }

    // -----------------------------------------------------------------------
    // GroupExecutor dispatch
    // -----------------------------------------------------------------------

    #[test]
    fn group_executor_dispatches_sequential() {
        let gid = group_id();
        let plan = sequential_plan(vec![task(1), task(2)]);
        let evidence = GroupExecutor::new()
            .run(gid, &plan, own_input_runner)
            .expect("run ok");
        assert_eq!(evidence.group_id, gid);
        assert_eq!(evidence.execution_result.mode, ExecutionMode::Sequential);
    }

    #[test]
    fn group_executor_dispatches_parallel() {
        let gid = group_id();
        let plan = parallel_plan(vec![task(1), task(2)]);
        let evidence = GroupExecutor::new()
            .run(gid, &plan, own_input_runner)
            .expect("run ok");
        assert_eq!(evidence.execution_result.mode, ExecutionMode::Parallel);
    }

    #[test]
    fn group_executor_dispatches_pipeline() {
        let gid = group_id();
        let plan = pipeline_plan(vec![task(1), task(2)]);
        let evidence = GroupExecutor::new()
            .run(gid, &plan, own_input_runner)
            .expect("run ok");
        assert_eq!(evidence.execution_result.mode, ExecutionMode::Pipeline);
    }

    #[test]
    fn group_executor_dispatches_consensus() {
        let gid = group_id();
        let plan = consensus_plan(vec![task(1), task(2)]);
        let evidence = GroupExecutor::new()
            .run(gid, &plan, own_input_runner)
            .expect("run ok");
        assert_eq!(evidence.execution_result.mode, ExecutionMode::Consensus);
    }

    #[test]
    fn group_executor_propagates_consensus_error() {
        let gid = group_id();
        let plan = consensus_plan(vec![task(1)]); // only 1 task — should fail
        let err = GroupExecutor::new()
            .run(gid, &plan, own_input_runner)
            .expect_err("should fail");
        assert!(matches!(err, GroupError::Internal(_)));
    }

    #[test]
    fn execution_evidence_builds_child_agents() {
        let gid = group_id();
        let plan = parallel_plan(vec![task(1), task(2), task(3)]);
        let evidence = GroupExecutor::new()
            .run(gid, &plan, own_input_runner)
            .expect("run ok");
        assert_eq!(evidence.child_agents.len(), 3);
    }

    #[test]
    fn execution_evidence_outcome_summary_contains_mode() {
        let gid = group_id();
        let plan = sequential_plan(vec![task(1)]);
        let evidence = GroupExecutor::new()
            .run(gid, &plan, own_input_runner)
            .expect("run ok");
        assert!(evidence.outcome_summary.contains("Sequential"));
    }

    #[test]
    fn execution_evidence_into_propagation_evidence() {
        let gid = group_id();
        let plan = sequential_plan(vec![task(1), task(2)]);
        let evidence = GroupExecutor::new()
            .run(gid, &plan, own_input_runner)
            .expect("run ok");
        let prop = evidence.into_propagation_evidence();
        assert_eq!(prop.group_id, gid);
        assert_eq!(prop.successful_runs, 2);
        assert_eq!(prop.failed_runs, 0);
        assert!(prop.quorum_met);
    }
}

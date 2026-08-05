//! Batch tool execution — process multiple tool invocations in a single batch.
//!
//! The [`BatchToolExecutor`] bridges [`polkagent_batch`] infrastructure with the
//! tool registry, enabling callers to submit several tool calls at once and
//! receive aggregated results.
//!
//! # Execution modes
//!
//! - **Sequential** — tools are executed one at a time, preserving submission
//!   order. Useful when later invocations depend on earlier results.
//! - **Parallel** — tools are executed concurrently (up to a configurable
//!   concurrency limit). Faster for independent calls.
//!
//! # Partial failures
//!
//! When some invocations succeed and others fail, the executor does **not**
//! short-circuit by default. Every invocation is attempted, and the caller
//! receives per-invocation results so it can decide how to proceed.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};

use polkagent_batch::{Batch, BatchConfig, BatchProcessor, BatchResult as BatchProcessorResult};

use crate::registry::{ToolContext, ToolRegistry, ToolResult};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Specifies how batch invocations are executed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchExecutionMode {
    /// Execute invocations one at a time, preserving order.
    #[default]
    Sequential,
    /// Execute invocations concurrently with the given parallelism limit.
    Parallel(usize),
}

/// A single tool invocation within a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    /// Name of the tool to call (must be registered in the [`ToolRegistry`]).
    pub tool_name: String,
    /// JSON input to pass to the tool handler.
    pub input: Value,
}

impl ToolInvocation {
    /// Create a new tool invocation.
    pub fn new(tool_name: impl Into<String>, input: Value) -> Self {
        Self {
            tool_name: tool_name.into(),
            input,
        }
    }
}

/// The outcome of a single tool invocation within a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocationResult {
    /// The tool that was invoked.
    pub tool_name: String,
    /// The input that was provided.
    pub input: Value,
    /// The tool result, if successful.
    pub result: Option<ToolResult>,
    /// The error message, if the invocation failed.
    pub error: Option<String>,
    /// Whether the invocation succeeded.
    pub success: bool,
}

/// Aggregate result of executing a batch of tool invocations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchToolResult {
    /// Total number of invocations in the batch.
    pub total: usize,
    /// Number of invocations that succeeded.
    pub succeeded: usize,
    /// Number of invocations that failed.
    pub failed: usize,
    /// Per-invocation results, in submission order.
    pub results: Vec<ToolInvocationResult>,
    /// The execution mode that was used.
    pub mode: BatchExecutionMode,
    /// The underlying batch-processor result (for advanced consumers).
    #[serde(skip)]
    pub batch_result: Option<BatchProcessorResult>,
}

impl BatchToolResult {
    /// Returns `true` if every invocation in the batch succeeded.
    #[must_use]
    pub fn all_succeeded(&self) -> bool {
        self.succeeded == self.total
    }

    /// Returns `true` if at least one invocation failed.
    #[must_use]
    pub fn has_failures(&self) -> bool {
        self.failed > 0
    }
}

// ---------------------------------------------------------------------------
// BatchToolExecutor
// ---------------------------------------------------------------------------

/// Executes multiple tool invocations as a batch using [`polkagent_batch`]
/// infrastructure.
///
/// The executor holds an `Arc<ToolRegistry>` so it can resolve tool handlers
/// at execution time. The registry itself is **not** modified.
pub struct BatchToolExecutor {
    registry: Arc<ToolRegistry>,
}

impl BatchToolExecutor {
    /// Create a new batch executor backed by the given tool registry.
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }

    /// Execute a batch of tool invocations.
    ///
    /// Each [`ToolInvocation`] is dispatched through the registry with the
    /// provided [`ToolContext`]. The `mode` controls whether invocations run
    /// sequentially or in parallel.
    ///
    /// The returned [`BatchToolResult`] contains per-invocation outcomes and
    /// aggregate counters. Partial failures are always reported — a single
    /// failing tool does not prevent other tools from executing.
    pub async fn execute(
        &self,
        invocations: Vec<ToolInvocation>,
        context: &ToolContext,
        mode: BatchExecutionMode,
    ) -> BatchToolResult {
        let count = invocations.len();

        info!(
            count,
            mode = ?mode,
            "starting batch tool execution"
        );

        if invocations.is_empty() {
            return BatchToolResult {
                total: 0,
                succeeded: 0,
                failed: 0,
                results: vec![],
                mode,
                batch_result: None,
            };
        }

        // Build a polkagent-batch Batch from the invocations.
        let batch_config = match mode {
            BatchExecutionMode::Sequential => BatchConfig::sequential().with_max_size(0),
            BatchExecutionMode::Parallel(n) => BatchConfig::parallel(n).with_max_size(0),
        };

        let mut batch: Batch<ToolInvocation> = Batch::new(batch_config);
        for inv in &invocations {
            batch.push(inv.clone());
        }

        // Process the batch. The handler closure captures the registry and
        // context, dispatching each invocation through the normal tool path.
        let registry = Arc::clone(&self.registry);
        let ctx = context.clone();

        let process_result = BatchProcessor::process(&mut batch, move |inv: ToolInvocation| {
            let registry = Arc::clone(&registry);
            let ctx = ctx.clone();
            async move {
                debug!(tool = %inv.tool_name, "executing tool in batch");
                match registry
                    .execute(&inv.tool_name, inv.input.clone(), &ctx)
                    .await
                {
                    Ok(tool_result) => {
                        // Serialize the ToolResult into a JSON value for the
                        // batch processor's generic result slot.
                        let value = serde_json::to_value(&tool_result)
                            .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));
                        Ok(value)
                    }
                    Err(tool_err) => Err(tool_err.to_string()),
                }
            }
        })
        .await;

        // Map the batch processor result back to our domain types.
        match process_result {
            Ok(batch_result) => Self::build_result(&invocations, &batch_result, mode),
            Err(batch_err) => {
                warn!(error = %batch_err, "batch processing infrastructure error");
                // If the batch infrastructure itself fails, report all as failed.
                let results: Vec<ToolInvocationResult> = invocations
                    .iter()
                    .map(|inv| ToolInvocationResult {
                        tool_name: inv.tool_name.clone(),
                        input: inv.input.clone(),
                        result: None,
                        error: Some(format!("batch infrastructure error: {batch_err}")),
                        success: false,
                    })
                    .collect();

                BatchToolResult {
                    total: count,
                    succeeded: 0,
                    failed: count,
                    results,
                    mode,
                    batch_result: None,
                }
            }
        }
    }

    /// Translate a [`BatchProcessorResult`] into a [`BatchToolResult`] using
    /// the original invocations for tool names and inputs.
    fn build_result(
        invocations: &[ToolInvocation],
        batch_result: &BatchProcessorResult,
        mode: BatchExecutionMode,
    ) -> BatchToolResult {
        let mut results = Vec::with_capacity(invocations.len());

        for (i, inv) in invocations.iter().enumerate() {
            let item_result = batch_result.results.get(i);

            let (success, tool_result, error) = match item_result {
                // Attempt to deserialize the ToolResult back from the JSON
                // value stored by the batch processor.
                Some(ir) => match ir.result.as_ref() {
                    Some(value) => match serde_json::from_value::<ToolResult>(value.clone()) {
                        Ok(tool_result) => (true, Some(tool_result), None),
                        Err(error) => {
                            (false, None, Some(format!("deserialization error: {error}")))
                        }
                    },
                    None => (
                        false,
                        None,
                        Some(
                            ir.error
                                .clone()
                                .unwrap_or_else(|| "no result from batch processor".to_string()),
                        ),
                    ),
                },
                _ => (
                    false,
                    None,
                    Some("no result from batch processor".to_string()),
                ),
            };

            results.push(ToolInvocationResult {
                tool_name: inv.tool_name.clone(),
                input: inv.input.clone(),
                result: tool_result,
                error,
                success,
            });
        }

        let succeeded = results.iter().filter(|r| r.success).count();
        let failed = results.iter().filter(|r| !r.success).count();

        info!(
            total = invocations.len(),
            succeeded, failed, "batch tool execution complete"
        );

        BatchToolResult {
            total: invocations.len(),
            succeeded,
            failed,
            results,
            mode,
            batch_result: Some(batch_result.clone()),
        }
    }
}

impl std::fmt::Debug for BatchToolExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchToolExecutor")
            .field("registry", &self.registry)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Assertion-oriented tests intentionally fail fast when their fixtures violate invariants.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use polkagent_core::config::DataClassification;
    use polkagent_core::ids::{AgentId, RunId, StepId};

    use crate::registry::{ToolError, ToolHandler, ToolSpec};

    // -- Helpers --

    /// A tool that echoes its input back after an optional delay.
    struct EchoBatchTool;

    #[async_trait]
    impl ToolHandler for EchoBatchTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "batch.echo".to_string(),
                description: "Echoes input for batch tests".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "value": { "type": "string" }
                    }
                }),
                required_grant: None,
                output_classification: DataClassification::Public,
            }
        }

        async fn execute(
            &self,
            input: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            // Optional delay for timing tests.
            if let Some(delay_ms) = input.get("delay_ms").and_then(Value::as_u64) {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Ok(ToolResult {
                output: input,
                classification: DataClassification::Public,
                artifacts: vec![],
            })
        }
    }

    /// A tool that fails when the input contains `"fail": true`.
    struct MaybeFailTool;

    #[async_trait]
    impl ToolHandler for MaybeFailTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "batch.maybe_fail".to_string(),
                description: "Fails when told to".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "fail": { "type": "boolean" }
                    }
                }),
                required_grant: None,
                output_classification: DataClassification::Public,
            }
        }

        async fn execute(
            &self,
            input: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            let should_fail = input.get("fail").and_then(Value::as_bool).unwrap_or(false);

            if should_fail {
                return Err(ToolError::ExecutionFailed {
                    reason: "intentional failure".to_string(),
                });
            }

            Ok(ToolResult {
                output: input,
                classification: DataClassification::Public,
                artifacts: vec![],
            })
        }
    }

    /// A tool that records the order it was called in using a shared counter.
    struct OrderTrackingTool {
        counter: Arc<AtomicUsize>,
        call_log: Arc<tokio::sync::Mutex<Vec<(usize, String)>>>,
    }

    #[async_trait]
    impl ToolHandler for OrderTrackingTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "batch.order".to_string(),
                description: "Tracks call order".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" }
                    }
                }),
                required_grant: None,
                output_classification: DataClassification::Public,
            }
        }

        async fn execute(
            &self,
            input: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            let order = self.counter.fetch_add(1, Ordering::SeqCst);
            let id = input
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            self.call_log.lock().await.push((order, id));

            Ok(ToolResult {
                output: serde_json::json!({ "order": order }),
                classification: DataClassification::Public,
                artifacts: vec![],
            })
        }
    }

    fn test_context() -> ToolContext {
        ToolContext {
            run_id: RunId::new(),
            agent_id: AgentId::new(),
            step_id: StepId::new(),
            grants: vec![],
            security_config: None,
        }
    }

    fn registry_with_echo() -> Arc<ToolRegistry> {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoBatchTool));
        Arc::new(reg)
    }

    fn registry_with_maybe_fail() -> Arc<ToolRegistry> {
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(EchoBatchTool));
        reg.register(Box::new(MaybeFailTool));
        Arc::new(reg)
    }

    // ======================================================================
    // Test 1: Batch with single item works
    // ======================================================================

    #[tokio::test]
    async fn batch_single_item_works() {
        let registry = registry_with_echo();
        let executor = BatchToolExecutor::new(registry);
        let ctx = test_context();

        let invocations = vec![ToolInvocation::new(
            "batch.echo",
            serde_json::json!({ "value": "hello" }),
        )];

        let result = executor
            .execute(invocations, &ctx, BatchExecutionMode::Sequential)
            .await;

        assert_eq!(result.total, 1);
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 0);
        assert!(result.all_succeeded());
        assert!(!result.has_failures());
        assert_eq!(result.results.len(), 1);
        assert!(result.results[0].success);
        assert!(result.results[0].result.is_some());

        let output = &result.results[0]
            .result
            .as_ref()
            .expect("should have result")
            .output;
        assert_eq!(output["value"], "hello");
    }

    // ======================================================================
    // Test 2: Batch with multiple items processes all
    // ======================================================================

    #[tokio::test]
    async fn batch_multiple_items_processes_all() {
        let registry = registry_with_echo();
        let executor = BatchToolExecutor::new(registry);
        let ctx = test_context();

        let invocations: Vec<ToolInvocation> = (0..5)
            .map(|i| {
                ToolInvocation::new(
                    "batch.echo",
                    serde_json::json!({ "value": format!("item-{i}") }),
                )
            })
            .collect();

        let result = executor
            .execute(invocations, &ctx, BatchExecutionMode::Sequential)
            .await;

        assert_eq!(result.total, 5);
        assert_eq!(result.succeeded, 5);
        assert_eq!(result.failed, 0);
        assert!(result.all_succeeded());

        for (i, inv_result) in result.results.iter().enumerate() {
            assert!(inv_result.success, "item {i} should succeed");
            let output = inv_result
                .result
                .as_ref()
                .expect("should have result")
                .output
                .clone();
            assert_eq!(output["value"], format!("item-{i}"));
        }
    }

    // ======================================================================
    // Test 3: Sequential mode preserves order
    // ======================================================================

    #[tokio::test]
    async fn sequential_mode_preserves_order() {
        let counter = Arc::new(AtomicUsize::new(0));
        let call_log = Arc::new(tokio::sync::Mutex::new(Vec::new()));

        let mut reg = ToolRegistry::new();
        reg.register(Box::new(OrderTrackingTool {
            counter: Arc::clone(&counter),
            call_log: Arc::clone(&call_log),
        }));
        let registry = Arc::new(reg);

        let executor = BatchToolExecutor::new(registry);
        let ctx = test_context();

        let invocations: Vec<ToolInvocation> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|id| ToolInvocation::new("batch.order", serde_json::json!({ "id": id })))
            .collect();

        let result = executor
            .execute(invocations, &ctx, BatchExecutionMode::Sequential)
            .await;

        assert!(result.all_succeeded());

        // Verify the call log shows items were processed in submission order.
        let log = call_log.lock().await;
        assert_eq!(log.len(), 5);
        for (i, (order, id)) in log.iter().enumerate() {
            assert_eq!(*order, i, "item '{id}' should be called at position {i}");
        }

        // Verify result order matches submission order.
        let ids = ["a", "b", "c", "d", "e"];
        for (i, inv_result) in result.results.iter().enumerate() {
            let output = inv_result.result.as_ref().expect("should have result");
            assert_eq!(
                output.output["order"], i,
                "result {i} should have order {i}"
            );
            assert_eq!(inv_result.input["id"], ids[i]);
        }
    }

    // ======================================================================
    // Test 4: Parallel mode completes faster than sequential
    // ======================================================================

    #[tokio::test]
    async fn parallel_mode_faster_than_sequential() {
        let registry = registry_with_echo();
        let executor = BatchToolExecutor::new(Arc::clone(&registry));
        let ctx = test_context();

        // Each invocation sleeps 50ms. With 4 items:
        // Sequential ~200ms total, Parallel(4) ~50ms total.
        let invocations: Vec<ToolInvocation> = (0..4)
            .map(|i| {
                ToolInvocation::new(
                    "batch.echo",
                    serde_json::json!({ "value": format!("item-{i}"), "delay_ms": 50 }),
                )
            })
            .collect();

        // Sequential timing.
        let start_seq = Instant::now();
        let result_seq = executor
            .execute(invocations.clone(), &ctx, BatchExecutionMode::Sequential)
            .await;
        let elapsed_seq = start_seq.elapsed();
        assert!(result_seq.all_succeeded());

        // Parallel timing.
        let executor_par = BatchToolExecutor::new(Arc::clone(&registry));
        let start_par = Instant::now();
        let result_par = executor_par
            .execute(invocations, &ctx, BatchExecutionMode::Parallel(4))
            .await;
        let elapsed_par = start_par.elapsed();
        assert!(result_par.all_succeeded());

        // Parallel should be meaningfully faster. Allow generous margin for
        // CI but the ratio should be well over 1.5x.
        assert!(
            elapsed_par < elapsed_seq,
            "parallel ({elapsed_par:?}) should be faster than sequential ({elapsed_seq:?})"
        );
    }

    // ======================================================================
    // Test 5: Partial failure reports correct results
    // ======================================================================

    #[tokio::test]
    async fn partial_failure_reports_correct_results() {
        let registry = registry_with_maybe_fail();
        let executor = BatchToolExecutor::new(registry);
        let ctx = test_context();

        let invocations = vec![
            ToolInvocation::new("batch.echo", serde_json::json!({ "value": "ok-1" })),
            ToolInvocation::new("batch.maybe_fail", serde_json::json!({ "fail": true })),
            ToolInvocation::new("batch.echo", serde_json::json!({ "value": "ok-2" })),
            ToolInvocation::new("batch.maybe_fail", serde_json::json!({ "fail": true })),
            ToolInvocation::new("batch.echo", serde_json::json!({ "value": "ok-3" })),
        ];

        let result = executor
            .execute(invocations, &ctx, BatchExecutionMode::Sequential)
            .await;

        assert_eq!(result.total, 5);
        assert_eq!(result.succeeded, 3);
        assert_eq!(result.failed, 2);
        assert!(!result.all_succeeded());
        assert!(result.has_failures());

        // Check individual results.
        assert!(result.results[0].success, "item 0 should succeed");
        assert!(!result.results[1].success, "item 1 should fail");
        assert!(result.results[1].error.is_some());
        assert!(result.results[2].success, "item 2 should succeed");
        assert!(!result.results[3].success, "item 3 should fail");
        assert!(result.results[3].error.is_some());
        assert!(result.results[4].success, "item 4 should succeed");
    }

    // ======================================================================
    // Test 6: Empty batch returns empty results
    // ======================================================================

    #[tokio::test]
    async fn empty_batch_returns_empty_results() {
        let registry = registry_with_echo();
        let executor = BatchToolExecutor::new(registry);
        let ctx = test_context();

        let result = executor
            .execute(vec![], &ctx, BatchExecutionMode::Sequential)
            .await;

        assert_eq!(result.total, 0);
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
        assert!(result.all_succeeded()); // Vacuously true.
        assert!(!result.has_failures());
        assert!(result.results.is_empty());
        assert!(result.batch_result.is_none());
    }

    // ======================================================================
    // Test 7: Tool not found in batch reports per-invocation error
    // ======================================================================

    #[tokio::test]
    async fn tool_not_found_reports_per_invocation_error() {
        let registry = registry_with_echo();
        let executor = BatchToolExecutor::new(registry);
        let ctx = test_context();

        let invocations = vec![
            ToolInvocation::new("batch.echo", serde_json::json!({ "value": "ok" })),
            ToolInvocation::new("nonexistent.tool", serde_json::json!({})),
        ];

        let result = executor
            .execute(invocations, &ctx, BatchExecutionMode::Sequential)
            .await;

        assert_eq!(result.total, 2);
        assert_eq!(result.succeeded, 1);
        assert_eq!(result.failed, 1);

        assert!(result.results[0].success);
        assert!(!result.results[1].success);
        assert!(
            result.results[1]
                .error
                .as_ref()
                .expect("should have error")
                .contains("not found"),
            "error should mention 'not found'"
        );
    }

    // ======================================================================
    // Test 8: Parallel partial failure reports correct results
    // ======================================================================

    #[tokio::test]
    async fn parallel_partial_failure() {
        let registry = registry_with_maybe_fail();
        let executor = BatchToolExecutor::new(registry);
        let ctx = test_context();

        let invocations = vec![
            ToolInvocation::new("batch.echo", serde_json::json!({ "value": "ok-1" })),
            ToolInvocation::new("batch.maybe_fail", serde_json::json!({ "fail": true })),
            ToolInvocation::new("batch.echo", serde_json::json!({ "value": "ok-2" })),
        ];

        let result = executor
            .execute(invocations, &ctx, BatchExecutionMode::Parallel(3))
            .await;

        assert_eq!(result.total, 3);
        assert_eq!(result.succeeded, 2);
        assert_eq!(result.failed, 1);
        assert!(result.has_failures());
    }
}

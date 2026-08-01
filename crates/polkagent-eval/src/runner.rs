//! Eval execution engine for running evaluation suites against a model executor.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Semaphore;

use polkagent_executor_trait::{
    ContentBlock, InferenceMessage, InferenceRequest, MessageRole, ModelExecutor, ToolDefinition,
};

use polkagent_core::{RunId, StepId};

use crate::report::{CaseResult, EvalReport, ToolCallRecord};
use crate::scorer::{Score, score_case};
use crate::types::{EvalCase, EvalSuite};

// ---------------------------------------------------------------------------
// EvalRunnerConfig
// ---------------------------------------------------------------------------

/// Configuration for an [`EvalRunner`].
#[derive(Debug, Clone)]
pub struct EvalRunnerConfig {
    /// Maximum number of cases to run concurrently.
    pub concurrency: usize,
    /// Default model identifier to use when running cases.
    pub model_id: String,
    /// Maximum output tokens per inference call.
    pub max_tokens: u32,
    /// Sampling temperature, or `None` to use the model default.
    pub temperature: Option<f32>,
}

impl Default for EvalRunnerConfig {
    fn default() -> Self {
        Self {
            concurrency: 4,
            model_id: "claude-opus-4-6".into(),
            max_tokens: 2048,
            temperature: Some(0.0),
        }
    }
}

// ---------------------------------------------------------------------------
// EvalRunner
// ---------------------------------------------------------------------------

/// Executes evaluation suites against a model executor.
pub struct EvalRunner {
    executor: Arc<dyn ModelExecutor>,
    config: EvalRunnerConfig,
}

impl EvalRunner {
    /// Create a new `EvalRunner` with the given executor and configuration.
    #[must_use]
    pub fn new(executor: Arc<dyn ModelExecutor>, config: EvalRunnerConfig) -> Self {
        Self { executor, config }
    }

    /// Create a new `EvalRunner` with default configuration.
    #[must_use]
    pub fn with_defaults(executor: Arc<dyn ModelExecutor>) -> Self {
        Self::new(executor, EvalRunnerConfig::default())
    }

    /// Run all cases in a suite and return a consolidated report.
    pub async fn run_suite(&self, suite: &EvalSuite) -> EvalReport {
        let semaphore = Arc::new(Semaphore::new(self.config.concurrency));

        let mut handles = Vec::with_capacity(suite.cases.len());

        for case in &suite.cases {
            let case = case.clone();
            let executor = Arc::clone(&self.executor);
            let config = self.config.clone();
            let permit = Arc::clone(&semaphore);

            let handle = tokio::spawn(async move {
                let _permit = permit.acquire_owned().await.ok();
                run_single_case(executor.as_ref(), &config, &case).await
            });
            handles.push(handle);
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => {
                    // Task panicked or was cancelled — emit a skipped result.
                    results.push(CaseResult {
                        case_id: "unknown".into(),
                        case_name: "unknown".into(),
                        score: Score::zero("Task join error"),
                        model_output: String::new(),
                        tool_calls_made: Vec::new(),
                        duration_ms: 0,
                        error: Some(format!("Task join error: {e}")),
                        category: crate::types::EvalCategory::General,
                    });
                }
            }
        }

        EvalReport::from_results(&suite.name, results)
    }

    /// Run a single case and return its result.
    pub async fn run_case(&self, case: &EvalCase) -> CaseResult {
        run_single_case(self.executor.as_ref(), &self.config, case).await
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Execute one evaluation case against the model executor.
async fn run_single_case(
    executor: &dyn ModelExecutor,
    config: &EvalRunnerConfig,
    case: &EvalCase,
) -> CaseResult {
    let start = Instant::now();

    let tools: Vec<ToolDefinition> = case
        .input
        .tools_available
        .iter()
        .map(|name| ToolDefinition {
            name: name.clone(),
            description: format!("Tool: {name}"),
            input_schema_json: r#"{"type":"object","properties":{}}"#.into(),
        })
        .collect();

    let request = InferenceRequest {
        run_id: RunId::new(),
        step_id: StepId::new(),
        messages: vec![InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: case.input.prompt.clone(),
            }],
        }],
        system: None,
        tools,
        model_id: config.model_id.clone(),
        max_tokens: config.max_tokens,
        temperature: config.temperature,
    };

    let timeout_duration = std::time::Duration::from_secs(case.timeout_secs);

    let exec_result =
        tokio::time::timeout(timeout_duration, executor.complete(request)).await;

    let duration_ms = start.elapsed().as_millis() as u64;

    match exec_result {
        Err(_elapsed) => {
            let mut result = CaseResult {
                case_id: case.id.clone(),
                case_name: case.name.clone(),
                score: Score::zero("Case timed out"),
                model_output: String::new(),
                tool_calls_made: Vec::new(),
                duration_ms,
                error: Some(format!(
                    "Timed out after {}s",
                    case.timeout_secs
                )),
                category: case.category,
            };
            result.score = score_case(&result, &case.expected);
            result
        }
        Ok(Err(e)) => {
            let mut result = CaseResult {
                case_id: case.id.clone(),
                case_name: case.name.clone(),
                score: Score::zero(format!("Executor error: {e}")),
                model_output: String::new(),
                tool_calls_made: Vec::new(),
                duration_ms,
                error: Some(e.to_string()),
                category: case.category,
            };
            result.score = score_case(&result, &case.expected);
            result
        }
        Ok(Ok(response)) => {
            let tool_calls_made: Vec<ToolCallRecord> = response
                .tool_calls
                .iter()
                .map(|tc| ToolCallRecord {
                    tool_name: tc.tool_name.clone(),
                    arguments_json: tc.arguments_json.clone(),
                })
                .collect();

            let mut result = CaseResult {
                case_id: case.id.clone(),
                case_name: case.name.clone(),
                score: Score::perfect(),
                model_output: response.text,
                tool_calls_made,
                duration_ms,
                error: None,
                category: case.category,
            };
            result.score = score_case(&result, &case.expected);
            result
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures::Stream;

    use polkagent_executor_trait::{
        ExecutorError, InferenceRequest, InferenceResponse, ModelExecutor, StreamEvent,
        TokenUsage, ToolCall,
    };

    use super::*;
    use crate::types::{EvalCase, EvalCategory, EvalInput, EvalSuite, Expected, ExpectedOutcome};

    // ------------------------------------------------------------------
    // Fake executor: always returns a fixed response
    // ------------------------------------------------------------------
    struct FakeExecutor {
        response_text: String,
        tool_calls: Vec<ToolCall>,
    }

    impl FakeExecutor {
        fn returning(text: impl Into<String>) -> Arc<Self> {
            Arc::new(Self {
                response_text: text.into(),
                tool_calls: Vec::new(),
            })
        }

        #[allow(dead_code)]
        fn with_tool_call(text: impl Into<String>, call: ToolCall) -> Arc<Self> {
            Arc::new(Self {
                response_text: text.into(),
                tool_calls: vec![call],
            })
        }
    }

    #[async_trait]
    impl ModelExecutor for FakeExecutor {
        async fn complete(
            &self,
            _request: InferenceRequest,
        ) -> Result<InferenceResponse, ExecutorError> {
            Ok(InferenceResponse {
                text: self.response_text.clone(),
                tool_calls: self.tool_calls.clone(),
                stop_reason: "end_turn".into(),
                usage: TokenUsage::default(),
                provider_request_id: None,
            })
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> Result<
            Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
            ExecutorError,
        > {
            Err(ExecutorError::Internal {
                message: "streaming not supported by FakeExecutor".into(),
            })
        }

        async fn health(&self) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    // Executor that always returns an error.
    struct ErrorExecutor;

    #[async_trait]
    impl ModelExecutor for ErrorExecutor {
        async fn complete(
            &self,
            _request: InferenceRequest,
        ) -> Result<InferenceResponse, ExecutorError> {
            Err(ExecutorError::Internal {
                message: "forced error".into(),
            })
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> Result<
            Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
            ExecutorError,
        > {
            Err(ExecutorError::Internal {
                message: "streaming not supported by ErrorExecutor".into(),
            })
        }

        async fn health(&self) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    // Executor that sleeps forever (for timeout tests).
    struct SlowExecutor;

    #[async_trait]
    impl ModelExecutor for SlowExecutor {
        async fn complete(
            &self,
            _request: InferenceRequest,
        ) -> Result<InferenceResponse, ExecutorError> {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            Err(ExecutorError::Cancelled)
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> Result<
            Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
            ExecutorError,
        > {
            Err(ExecutorError::Internal {
                message: "streaming not supported by SlowExecutor".into(),
            })
        }

        async fn health(&self) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    fn simple_case(id: &str, prompt: &str) -> EvalCase {
        EvalCase {
            id: id.into(),
            name: format!("Case {id}"),
            category: EvalCategory::General,
            input: EvalInput::from_prompt(prompt),
            expected: Expected::default(),
            tags: Vec::new(),
            timeout_secs: 30,
        }
    }

    fn simple_suite(cases: Vec<EvalCase>) -> EvalSuite {
        EvalSuite {
            name: "test-suite".into(),
            description: "A test suite".into(),
            version: "1.0.0".into(),
            cases,
        }
    }

    #[tokio::test]
    async fn run_case_returns_model_output() {
        let executor = FakeExecutor::returning("hello from the model");
        let runner = EvalRunner::with_defaults(executor);
        let case = simple_case("c1", "Say hello");
        let result = runner.run_case(&case).await;
        assert_eq!(result.model_output, "hello from the model");
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn run_suite_aggregates_results() {
        let executor = FakeExecutor::returning("output");
        let runner = EvalRunner::with_defaults(executor);
        let suite = simple_suite(vec![simple_case("c1", "p1"), simple_case("c2", "p2")]);
        let report = runner.run_suite(&suite).await;
        assert_eq!(report.total_cases, 2);
        assert_eq!(report.suite_name, "test-suite");
    }

    #[tokio::test]
    async fn run_case_with_must_contain_passing() {
        let executor = FakeExecutor::returning("The answer is 42");
        let runner = EvalRunner::with_defaults(executor);
        let mut case = simple_case("c1", "What is the answer?");
        case.expected.must_contain = vec!["42".into()];
        let result = runner.run_case(&case).await;
        assert!(result.score.passed);
    }

    #[tokio::test]
    async fn run_case_with_must_contain_failing() {
        let executor = FakeExecutor::returning("I don't know");
        let runner = EvalRunner::with_defaults(executor);
        let mut case = simple_case("c1", "What is the answer?");
        case.expected.must_contain = vec!["42".into()];
        let result = runner.run_case(&case).await;
        assert!(!result.score.passed);
    }

    #[tokio::test]
    async fn run_case_executor_error_recorded() {
        let executor = Arc::new(ErrorExecutor);
        let runner = EvalRunner::with_defaults(executor);
        let case = simple_case("c1", "prompt");
        let result = runner.run_case(&case).await;
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn timeout_enforcement() {
        let executor = Arc::new(SlowExecutor);
        let config = EvalRunnerConfig {
            concurrency: 1,
            model_id: "test".into(),
            max_tokens: 100,
            temperature: None,
        };
        let runner = EvalRunner::new(executor, config);
        let mut case = simple_case("c1", "prompt");
        case.timeout_secs = 1; // Very short timeout
        let result = runner.run_case(&case).await;
        assert!(result.error.is_some());
        let err = result.error.unwrap();
        assert!(err.contains("Timed out") || err.contains("timed out"));
    }

    #[tokio::test]
    async fn parallel_execution_respects_concurrency() {
        // Run a suite with many cases at concurrency=2 and confirm all complete.
        let executor = FakeExecutor::returning("ok");
        let config = EvalRunnerConfig {
            concurrency: 2,
            model_id: "test".into(),
            max_tokens: 100,
            temperature: None,
        };
        let runner = EvalRunner::new(executor, config);
        let cases: Vec<_> = (0..8).map(|i| simple_case(&format!("c{i}"), "p")).collect();
        let suite = simple_suite(cases);
        let report = runner.run_suite(&suite).await;
        assert_eq!(report.total_cases, 8);
        assert_eq!(report.results.len(), 8);
    }

    #[tokio::test]
    async fn run_case_refusal_outcome_check() {
        let executor = FakeExecutor::returning("I cannot assist with that request.");
        let runner = EvalRunner::with_defaults(executor);
        let mut case = simple_case("c1", "Do something harmful");
        case.expected.expected_outcome = Some(ExpectedOutcome::Refusal);
        let result = runner.run_case(&case).await;
        assert!(result.score.passed);
    }

    #[tokio::test]
    async fn run_empty_suite() {
        let executor = FakeExecutor::returning("ok");
        let runner = EvalRunner::with_defaults(executor);
        let suite = simple_suite(Vec::new());
        let report = runner.run_suite(&suite).await;
        assert_eq!(report.total_cases, 0);
        assert_eq!(report.passed, 0);
        assert_eq!(report.failed, 0);
    }
}

//! Deterministic fake [`ModelExecutor`] adapter for testing.
//!
//! This crate provides [`FakeExecutor`] — a fully deterministic, configurable
//! implementation of the [`ModelExecutor`] trait intended for unit and
//! integration tests. It never makes network calls.
//!
//! # Quick-start examples
//!
//! ```rust
//! use polkagent_executor_fake::FakeExecutor;
//! use polkagent_executor_trait::ModelExecutor;
//!
//! // Always responds with a fixed text string.
//! let executor = FakeExecutor::new();
//!
//! // Returns pre-configured responses in a cycling sequence.
//! use polkagent_executor_trait::{InferenceResponse, TokenUsage};
//! let responses = vec![
//!     InferenceResponse {
//!         text: "first response".into(),
//!         tool_calls: vec![],
//!         stop_reason: "end_turn".into(),
//!         usage: TokenUsage::default(),
//!         provider_request_id: None,
//!     },
//! ];
//! let executor = FakeExecutor::with_responses(responses);
//!
//! // Always returns an error.
//! use polkagent_executor_trait::ExecutorError;
//! let executor = FakeExecutor::failing(ExecutorError::Cancelled);
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream;
use polkagent_executor_trait::{
    ExecutorError, InferenceRequest, InferenceResponse, ModelExecutor, StreamEvent, TokenUsage,
    ToolCall,
};

// ---------------------------------------------------------------------------
// Internal mode
// ---------------------------------------------------------------------------

enum Mode {
    /// Return the same text for every request.
    FixedText(String),
    /// Return responses from a list, cycling when exhausted.
    Cycling(Vec<InferenceResponse>),
    /// Return a fixed set of tool calls with an empty text body.
    ToolCalls(Vec<ToolCall>),
    /// Always return the given error.
    Failing(ExecutorErrorKind),
}

/// A copyable snapshot of an [`ExecutorError`].
///
/// `ExecutorError` is not `Clone` so we store a factory closure instead.
enum ExecutorErrorKind {
    Transport {
        message: String,
        retryable: bool,
    },
    Authentication {
        message: String,
    },
    RateLimit {
        retry_after_secs: Option<u64>,
    },
    ContextWindowExceeded {
        tokens_requested: u32,
        tokens_allowed: u32,
    },
    InvalidResponse {
        message: String,
    },
    Timeout {
        elapsed_ms: u64,
    },
    Cancelled,
    Internal {
        message: String,
    },
    ContentPolicy {
        message: String,
    },
    ModelNotFound {
        model: String,
        message: String,
    },
}

impl ExecutorErrorKind {
    fn to_error(&self) -> ExecutorError {
        match self {
            Self::Transport { message, retryable } => ExecutorError::Transport {
                message: message.clone(),
                retryable: *retryable,
            },
            Self::Authentication { message } => ExecutorError::Authentication {
                message: message.clone(),
            },
            Self::RateLimit { retry_after_secs } => ExecutorError::RateLimit {
                retry_after_secs: *retry_after_secs,
            },
            Self::ContextWindowExceeded {
                tokens_requested,
                tokens_allowed,
            } => ExecutorError::ContextWindowExceeded {
                tokens_requested: *tokens_requested,
                tokens_allowed: *tokens_allowed,
            },
            Self::InvalidResponse { message } => ExecutorError::InvalidResponse {
                message: message.clone(),
            },
            Self::Timeout { elapsed_ms } => ExecutorError::Timeout {
                elapsed_ms: *elapsed_ms,
            },
            Self::Cancelled => ExecutorError::Cancelled,
            Self::Internal { message } => ExecutorError::Internal {
                message: message.clone(),
            },
            Self::ContentPolicy { message } => ExecutorError::ContentPolicy {
                message: message.clone(),
            },
            Self::ModelNotFound { model, message } => ExecutorError::ModelNotFound {
                model: model.clone(),
                message: message.clone(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// FakeExecutor
// ---------------------------------------------------------------------------

/// A deterministic fake implementation of [`ModelExecutor`].
///
/// `FakeExecutor` never makes network calls. Its behaviour is controlled by
/// the constructor used:
///
/// | Constructor | Behaviour |
/// |---|---|
/// | [`new`] | Returns `"I am a fake assistant"` for every request. |
/// | [`with_responses`] | Returns pre-configured responses, cycling when exhausted. |
/// | [`with_tool_calls`] | Returns the given tool calls with an empty text body. |
/// | [`failing`] | Always returns the given error. |
///
/// After each call, [`FakeExecutor`] increments an internal call counter and
/// records the most recent request so tests can assert on what was received.
///
/// [`new`]: FakeExecutor::new
/// [`with_responses`]: FakeExecutor::with_responses
/// [`with_tool_calls`]: FakeExecutor::with_tool_calls
/// [`failing`]: FakeExecutor::failing
pub struct FakeExecutor {
    mode: Mode,
    call_count: AtomicU64,
    last_request: Mutex<Option<InferenceRequest>>,
}

impl FakeExecutor {
    /// Create a `FakeExecutor` that always responds with `"I am a fake assistant"`.
    ///
    /// The response has `stop_reason = "end_turn"` and zeroed token usage.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            mode: Mode::FixedText("I am a fake assistant".into()),
            call_count: AtomicU64::new(0),
            last_request: Mutex::new(None),
        })
    }

    /// Create a `FakeExecutor` that returns `responses` in order.
    ///
    /// When all responses have been consumed, the sequence repeats from the
    /// beginning (cycling). Panics if `responses` is empty.
    ///
    /// # Panics
    ///
    /// Panics if `responses` is empty.
    #[must_use]
    pub fn with_responses(responses: Vec<InferenceResponse>) -> Arc<Self> {
        assert!(
            !responses.is_empty(),
            "FakeExecutor::with_responses requires at least one response"
        );
        Arc::new(Self {
            mode: Mode::Cycling(responses),
            call_count: AtomicU64::new(0),
            last_request: Mutex::new(None),
        })
    }

    /// Create a `FakeExecutor` that returns `tool_calls` in every response.
    ///
    /// The `text` field of the response is empty. The `stop_reason` is
    /// `"tool_use"`.
    #[must_use]
    pub fn with_tool_calls(tool_calls: Vec<ToolCall>) -> Arc<Self> {
        Arc::new(Self {
            mode: Mode::ToolCalls(tool_calls),
            call_count: AtomicU64::new(0),
            last_request: Mutex::new(None),
        })
    }

    /// Create a `FakeExecutor` that always returns the given error.
    ///
    /// Because [`ExecutorError`] is not `Clone`, this method accepts any
    /// `ExecutorError` value and clones its shape internally.
    #[must_use]
    pub fn failing(error: ExecutorError) -> Arc<Self> {
        let kind = match error {
            ExecutorError::Transport { message, retryable } => {
                ExecutorErrorKind::Transport { message, retryable }
            }
            ExecutorError::Authentication { message } => {
                ExecutorErrorKind::Authentication { message }
            }
            ExecutorError::RateLimit { retry_after_secs } => {
                ExecutorErrorKind::RateLimit { retry_after_secs }
            }
            ExecutorError::ContextWindowExceeded {
                tokens_requested,
                tokens_allowed,
            } => ExecutorErrorKind::ContextWindowExceeded {
                tokens_requested,
                tokens_allowed,
            },
            ExecutorError::InvalidResponse { message } => {
                ExecutorErrorKind::InvalidResponse { message }
            }
            ExecutorError::Timeout { elapsed_ms } => ExecutorErrorKind::Timeout { elapsed_ms },
            ExecutorError::Cancelled => ExecutorErrorKind::Cancelled,
            ExecutorError::Internal { message } => ExecutorErrorKind::Internal { message },
            ExecutorError::ContentPolicy { message } => {
                ExecutorErrorKind::ContentPolicy { message }
            }
            ExecutorError::ModelNotFound { model, message } => {
                ExecutorErrorKind::ModelNotFound { model, message }
            }
        };
        Arc::new(Self {
            mode: Mode::Failing(kind),
            call_count: AtomicU64::new(0),
            last_request: Mutex::new(None),
        })
    }

    // -----------------------------------------------------------------------
    // Assertion helpers
    // -----------------------------------------------------------------------

    /// Return how many times [`complete`] or [`stream`] has been called.
    ///
    /// [`complete`]: ModelExecutor::complete
    /// [`stream`]: ModelExecutor::stream
    #[must_use]
    pub fn call_count(&self) -> u64 {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Return a clone of the most recent [`InferenceRequest`] received.
    ///
    /// Returns `None` if no calls have been made yet.
    #[must_use]
    pub fn last_request(&self) -> Option<InferenceRequest> {
        self.last_request.lock().expect("mutex poisoned").clone()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn record(&self, request: &InferenceRequest) {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let mut guard = self.last_request.lock().expect("mutex poisoned");
        *guard = Some(request.clone());
    }

    fn build_response(&self, call_index: u64) -> Result<InferenceResponse, ExecutorError> {
        match &self.mode {
            Mode::FixedText(text) => Ok(InferenceResponse {
                text: text.clone(),
                tool_calls: vec![],
                stop_reason: "end_turn".into(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
                provider_request_id: Some(format!("fake-{call_index}")),
            }),
            Mode::Cycling(responses) => {
                let idx = (call_index as usize) % responses.len();
                Ok(responses[idx].clone())
            }
            Mode::ToolCalls(calls) => Ok(InferenceResponse {
                text: String::new(),
                tool_calls: calls.clone(),
                stop_reason: "tool_use".into(),
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
                provider_request_id: Some(format!("fake-{call_index}")),
            }),
            Mode::Failing(kind) => Err(kind.to_error()),
        }
    }
}

impl Default for FakeExecutor {
    fn default() -> Self {
        Self {
            mode: Mode::FixedText("I am a fake assistant".into()),
            call_count: AtomicU64::new(0),
            last_request: Mutex::new(None),
        }
    }
}

#[async_trait]
impl ModelExecutor for FakeExecutor {
    /// Execute one inference call and return the complete response.
    ///
    /// The call counter is incremented and the request is recorded before
    /// the result is computed.
    async fn complete(
        &self,
        request: InferenceRequest,
    ) -> Result<InferenceResponse, ExecutorError> {
        self.record(&request);
        let call_index = self.call_count.load(Ordering::SeqCst).saturating_sub(1);
        self.build_response(call_index)
    }

    /// Execute one inference call and return a stream of events.
    ///
    /// For non-error modes, the text content of the response is split into
    /// individual UTF-8 characters and each character is emitted as a
    /// [`StreamEvent::TextDelta`]. A [`StreamEvent::Completed`] event closes
    /// the stream.
    ///
    /// For the error mode, the stream yields a single error item.
    async fn stream(
        &self,
        request: InferenceRequest,
    ) -> Result<
        Box<dyn futures::Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
        ExecutorError,
    > {
        self.record(&request);
        let call_index = self.call_count.load(Ordering::SeqCst).saturating_sub(1);
        let response = self.build_response(call_index)?;

        // Emit each character of the text as a separate TextDelta chunk so
        // that streaming consumers can observe incremental delivery.
        let mut events: Vec<Result<StreamEvent, ExecutorError>> = response
            .text
            .chars()
            .map(|ch| {
                Ok(StreamEvent::TextDelta {
                    delta: ch.to_string(),
                })
            })
            .collect();

        // Append tool call complete events.
        for call in &response.tool_calls {
            events.push(Ok(StreamEvent::ToolCallComplete { call: call.clone() }));
        }

        // Terminal event.
        events.push(Ok(StreamEvent::Completed { result: response }));

        Ok(Box::new(stream::iter(events)))
    }

    /// Health check: always returns `Ok(())` for non-failing executors.
    async fn health(&self) -> Result<(), ExecutorError> {
        if let Mode::Failing(kind) = &self.mode {
            return Err(kind.to_error());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use polkagent_core::{RunId, StepId};
    use polkagent_executor_trait::{ContentBlock, InferenceMessage, MessageRole};

    fn minimal_request() -> InferenceRequest {
        InferenceRequest {
            run_id: RunId::new(),
            step_id: StepId::new(),
            messages: vec![InferenceMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            }],
            system: None,
            tools: vec![],
            model_id: "claude-opus-4-6".into(),
            max_tokens: 1024,
            temperature: None,
        }
    }

    #[tokio::test]
    async fn new_returns_fake_assistant_text() {
        let executor = FakeExecutor::new();
        let req = minimal_request();
        let resp = executor
            .complete(req)
            .await
            .expect("complete should succeed");
        assert_eq!(resp.text, "I am a fake assistant");
        assert_eq!(resp.stop_reason, "end_turn");
        assert!(resp.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn with_responses_cycles_through_all() {
        let r1 = InferenceResponse {
            text: "first".into(),
            tool_calls: vec![],
            stop_reason: "end_turn".into(),
            usage: TokenUsage::default(),
            provider_request_id: None,
        };
        let r2 = InferenceResponse {
            text: "second".into(),
            tool_calls: vec![],
            stop_reason: "end_turn".into(),
            usage: TokenUsage::default(),
            provider_request_id: None,
        };
        let executor = FakeExecutor::with_responses(vec![r1, r2]);

        let resp1 = executor
            .complete(minimal_request())
            .await
            .expect("first call");
        assert_eq!(resp1.text, "first");

        let resp2 = executor
            .complete(minimal_request())
            .await
            .expect("second call");
        assert_eq!(resp2.text, "second");

        // Should cycle back to first.
        let resp3 = executor
            .complete(minimal_request())
            .await
            .expect("third call");
        assert_eq!(resp3.text, "first");
    }

    #[tokio::test]
    async fn with_tool_calls_returns_tool_use_response() {
        let call = ToolCall {
            tool_call_id: "tc-001".into(),
            tool_name: "polkagent.file.read".into(),
            arguments_json: r#"{"path":"/tmp/test"}"#.into(),
        };
        let executor = FakeExecutor::with_tool_calls(vec![call.clone()]);
        let resp = executor
            .complete(minimal_request())
            .await
            .expect("complete");
        assert!(resp.text.is_empty());
        assert_eq!(resp.stop_reason, "tool_use");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].tool_name, "polkagent.file.read");
    }

    #[tokio::test]
    async fn failing_returns_error_on_complete() {
        let executor = FakeExecutor::failing(ExecutorError::Cancelled);
        let result = executor.complete(minimal_request()).await;
        assert!(matches!(result, Err(ExecutorError::Cancelled)));
    }

    #[tokio::test]
    async fn failing_returns_error_on_health() {
        let executor = FakeExecutor::failing(ExecutorError::Internal {
            message: "injected failure".into(),
        });
        let result = executor.health().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn call_count_increments_on_complete() {
        let executor = FakeExecutor::new();
        assert_eq!(executor.call_count(), 0);
        executor.complete(minimal_request()).await.expect("ok");
        assert_eq!(executor.call_count(), 1);
        executor.complete(minimal_request()).await.expect("ok");
        assert_eq!(executor.call_count(), 2);
    }

    #[tokio::test]
    async fn last_request_is_recorded() {
        let executor = FakeExecutor::new();
        assert!(executor.last_request().is_none());
        let req = minimal_request();
        let run_id = req.run_id;
        executor.complete(req).await.expect("ok");
        let captured = executor.last_request().expect("should have a request");
        assert_eq!(captured.run_id, run_id);
    }

    #[tokio::test]
    async fn stream_emits_text_delta_per_character() {
        let executor = FakeExecutor::new();
        let stream_result = executor.stream(minimal_request()).await.expect("stream ok");
        let events: Vec<_> = stream_result.collect().await;

        // There should be at least one TextDelta and one Completed event.
        let text_deltas: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let Ok(StreamEvent::TextDelta { delta }) = e {
                    Some(delta.clone())
                } else {
                    None
                }
            })
            .collect();

        let reconstructed = text_deltas.concat();
        assert_eq!(reconstructed, "I am a fake assistant");

        // Last event must be Completed.
        assert!(matches!(
            events.last().expect("must have events"),
            Ok(StreamEvent::Completed { .. })
        ));
    }

    #[tokio::test]
    async fn stream_with_tool_calls_emits_tool_call_complete() {
        let call = ToolCall {
            tool_call_id: "tc-42".into(),
            tool_name: "polkagent.chain.decode".into(),
            arguments_json: "{}".into(),
        };
        let executor = FakeExecutor::with_tool_calls(vec![call]);
        let stream_result = executor.stream(minimal_request()).await.expect("stream ok");
        let events: Vec<_> = stream_result.collect().await;

        let has_tool_complete = events
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::ToolCallComplete { .. })));
        assert!(
            has_tool_complete,
            "stream should contain a ToolCallComplete event"
        );
    }

    #[tokio::test]
    async fn health_ok_for_non_failing_executor() {
        let executor = FakeExecutor::new();
        assert!(executor.health().await.is_ok());
    }

    #[tokio::test]
    async fn failing_with_rate_limit_error() {
        let executor = FakeExecutor::failing(ExecutorError::RateLimit {
            retry_after_secs: Some(30),
        });
        let err = executor
            .complete(minimal_request())
            .await
            .expect_err("must fail");
        assert!(matches!(
            err,
            ExecutorError::RateLimit {
                retry_after_secs: Some(30)
            }
        ));
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn call_count_increments_on_stream() {
        let executor = FakeExecutor::new();
        assert_eq!(executor.call_count(), 0);
        let _stream = executor.stream(minimal_request()).await.expect("ok");
        assert_eq!(executor.call_count(), 1);
    }

    #[test]
    fn default_executor_behaves_like_new() {
        let executor = FakeExecutor::default();
        assert_eq!(executor.call_count(), 0);
        assert!(executor.last_request().is_none());
    }

    #[tokio::test]
    async fn with_responses_panics_on_empty() {
        let result = std::panic::catch_unwind(|| FakeExecutor::with_responses(vec![]));
        assert!(result.is_err(), "should panic on empty response list");
    }
}

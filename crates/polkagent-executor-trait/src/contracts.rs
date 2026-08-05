//! Reusable contract tests for [`ModelExecutor`] implementations.
//!
//! This module provides test functions that verify any implementation of
//! [`ModelExecutor`] conforms to the expected behaviour defined in the trait
//! contract. Each function accepts a reference to an implementor and exercises
//! one aspect of the contract.
//!
//! # Usage
//!
//! In your adapter crate's integration test:
//!
//! ```rust,ignore
//! use polkagent_executor_trait::contracts;
//!
//! #[tokio::test]
//! async fn complete_returns_response() {
//!     let executor = MyExecutor::new();
//!     contracts::test_complete_returns_response(&executor).await;
//! }
//! ```
//!
//! # Feature gate
//!
//! This module is only available when the `test-contracts` feature is enabled.

use crate::{
    ContentBlock, InferenceMessage, InferenceRequest, MessageRole, ModelExecutor, StreamEvent,
};
use futures::StreamExt;
use polkagent_core::{RunId, StepId};

/// Build a minimal valid [`InferenceRequest`] for contract tests.
///
/// This produces a request with a single user text message and no tools.
/// Contract tests should not depend on the specific content of this request
/// beyond the fact that it is structurally valid.
#[must_use]
pub fn minimal_request() -> InferenceRequest {
    InferenceRequest {
        run_id: RunId::new(),
        step_id: StepId::new(),
        messages: vec![InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: "Hello, this is a contract test.".into(),
            }],
        }],
        system: Some("You are a helpful assistant.".into()),
        tools: vec![],
        model_id: "test-model".into(),
        max_tokens: 1024,
        temperature: None,
    }
}

/// Contract: `complete()` returns a non-empty response.
///
/// A well-behaved executor must return an [`crate::InferenceResponse`] whose `text`
/// field is non-empty, or whose `tool_calls` field is non-empty, or both.
/// The response must also contain a `stop_reason`.
pub async fn test_complete_returns_response(executor: &dyn ModelExecutor) {
    let request = minimal_request();
    let response = executor
        .complete(request)
        .await
        .expect("complete() must not fail for a valid request");

    // Either text or tool_calls must be present.
    let has_content = !response.text.is_empty() || !response.tool_calls.is_empty();
    assert!(
        has_content,
        "complete() must return a response with non-empty text or tool_calls"
    );

    // stop_reason must be set.
    assert!(
        !response.stop_reason.is_empty(),
        "complete() must set a non-empty stop_reason"
    );
}

/// Contract: `complete()` reports non-zero token usage.
///
/// A well-behaved executor must track token consumption. At minimum,
/// `input_tokens` and `output_tokens` should be non-zero for a successful
/// response.
pub async fn test_complete_tracks_token_usage(executor: &dyn ModelExecutor) {
    let request = minimal_request();
    let response = executor
        .complete(request)
        .await
        .expect("complete() must not fail for a valid request");

    let total = response.usage.input_tokens + response.usage.output_tokens;
    assert!(
        total > 0,
        "complete() must report non-zero total token usage (input + output); got {total}"
    );
}

/// Contract: `stream()` ends with a [`StreamEvent::Completed`] event.
///
/// The trait contract states that the stream always terminates with a
/// `Completed` event. This test collects the stream and verifies the last
/// event has the `Completed` variant.
pub async fn test_stream_emits_completed(executor: &dyn ModelExecutor) {
    let request = minimal_request();
    let stream = executor
        .stream(request)
        .await
        .expect("stream() must not fail for a valid request");

    let events: Vec<_> = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("stream events must not contain errors for a valid request");

    assert!(!events.is_empty(), "stream() must emit at least one event");

    let last = events.last().expect("events is non-empty");
    assert!(
        matches!(last, StreamEvent::Completed { .. }),
        "the last stream event must be StreamEvent::Completed, got: {last:?}"
    );
}

/// Contract: `health()` returns without panicking.
///
/// A healthy executor must return `Ok(())`. This test verifies that the
/// health check completes without panic. Whether it returns `Ok` or `Err`
/// depends on the executor's configuration, but it must not panic.
pub async fn test_health_returns_result(executor: &dyn ModelExecutor) {
    // We only care that this does not panic. The result is either Ok or a
    // well-formed ExecutorError.
    let _result = executor.health().await;
}

//! Cross-crate integration tests for executor adapter behaviours.
//!
//! These tests verify the `polkagent-executor-fake` crate against the
//! `polkagent-executor-trait` contract, exercising both the synchronous
//! `complete()` and streaming `stream()` paths from outside the crate
//! boundary.

use futures::StreamExt;

use polkagent_core::{RunId, StepId};
use polkagent_executor_fake::FakeExecutor;
use polkagent_executor_trait::{
    ContentBlock, ExecutorError, InferenceMessage, InferenceRequest, InferenceResponse,
    MessageRole, ModelExecutor, StreamEvent, TokenUsage, ToolCall,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
        model_id: "test-model".into(),
        max_tokens: 512,
        temperature: None,
    }
}

// ---------------------------------------------------------------------------
// FakeExecutor::new() — complete() returns expected response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_executor_complete_returns_expected_response() {
    let executor = FakeExecutor::new();
    let resp = executor
        .complete(minimal_request())
        .await
        .expect("complete should succeed");

    assert_eq!(resp.text, "I am a fake assistant");
    assert_eq!(resp.stop_reason, "end_turn");
    assert!(resp.tool_calls.is_empty());
    assert_eq!(resp.usage.input_tokens, 10);
    assert_eq!(resp.usage.output_tokens, 5);
    assert!(resp.provider_request_id.is_some());
}

// ---------------------------------------------------------------------------
// FakeExecutor::new() — stream() emits events in correct order
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_executor_stream_emits_text_deltas_then_completed() {
    let executor = FakeExecutor::new();
    let stream = executor
        .stream(minimal_request())
        .await
        .expect("stream should succeed");
    let events: Vec<_> = stream.collect().await;

    // Must have at least two events: TextDelta(s) + Completed.
    assert!(
        events.len() >= 2,
        "expected at least 2 events, got {}",
        events.len()
    );

    // All events except the last must be TextDelta.
    for event in &events[..events.len() - 1] {
        let ev = event.as_ref().expect("event should be Ok");
        assert!(
            matches!(ev, StreamEvent::TextDelta { .. }),
            "non-terminal event should be TextDelta, got: {ev:?}"
        );
    }

    // The last event must be Completed.
    let last = events.last().expect("must have events");
    assert!(
        matches!(last, Ok(StreamEvent::Completed { .. })),
        "last event must be Completed"
    );

    // Reconstruct the text from TextDelta events.
    let reconstructed: String = events
        .iter()
        .filter_map(|e| match e {
            Ok(StreamEvent::TextDelta { delta }) => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reconstructed, "I am a fake assistant");
}

// ---------------------------------------------------------------------------
// FakeExecutor::with_responses — cycles through provided responses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_executor_with_responses_cycles_correctly() {
    let r1 = InferenceResponse {
        text: "alpha".into(),
        tool_calls: vec![],
        stop_reason: "end_turn".into(),
        usage: TokenUsage::default(),
        provider_request_id: None,
    };
    let r2 = InferenceResponse {
        text: "beta".into(),
        tool_calls: vec![],
        stop_reason: "end_turn".into(),
        usage: TokenUsage::default(),
        provider_request_id: None,
    };
    let executor = FakeExecutor::with_responses(vec![r1, r2]);

    // First call -> alpha
    let resp1 = executor.complete(minimal_request()).await.expect("call 1");
    assert_eq!(resp1.text, "alpha");

    // Second call -> beta
    let resp2 = executor.complete(minimal_request()).await.expect("call 2");
    assert_eq!(resp2.text, "beta");

    // Third call -> cycles back to alpha
    let resp3 = executor.complete(minimal_request()).await.expect("call 3");
    assert_eq!(resp3.text, "alpha");

    // Verify call count
    assert_eq!(executor.call_count(), 3);
}

// ---------------------------------------------------------------------------
// FakeExecutor::failing — returns errors on complete and stream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_executor_failing_returns_error_on_complete() {
    let executor = FakeExecutor::failing(ExecutorError::Cancelled);
    let result = executor.complete(minimal_request()).await;
    assert!(
        matches!(result, Err(ExecutorError::Cancelled)),
        "expected Cancelled error"
    );
}

#[tokio::test]
async fn fake_executor_failing_returns_error_on_stream() {
    let executor = FakeExecutor::failing(ExecutorError::Internal {
        message: "test failure".into(),
    });
    let result = executor.stream(minimal_request()).await;
    assert!(
        result.is_err(),
        "stream should return error for failing executor"
    );
}

#[tokio::test]
async fn fake_executor_failing_returns_error_on_health() {
    let executor = FakeExecutor::failing(ExecutorError::RateLimit {
        retry_after_secs: Some(60),
    });
    let result = executor.health().await;
    assert!(result.is_err(), "health should fail for failing executor");
}

// ---------------------------------------------------------------------------
// All executors satisfy the ModelExecutor trait — compile-time check
// ---------------------------------------------------------------------------

/// Compile-time verification that `FakeExecutor` satisfies `ModelExecutor`
/// when used behind a `dyn` trait object.
#[allow(dead_code)]
fn _fake_executor_is_model_executor(e: &dyn ModelExecutor) {
    let _ = e;
}

/// Verify that `Arc<FakeExecutor>` can be used as `Arc<dyn ModelExecutor>`.
#[tokio::test]
async fn fake_executor_can_be_used_as_dyn_model_executor() {
    let executor: std::sync::Arc<dyn ModelExecutor> = FakeExecutor::new();
    let resp = executor
        .complete(minimal_request())
        .await
        .expect("should work via trait object");
    assert_eq!(resp.text, "I am a fake assistant");
}

// ---------------------------------------------------------------------------
// Stream with tool calls emits ToolCallComplete events
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_executor_stream_with_tool_calls_emits_tool_call_complete() {
    let call = ToolCall {
        tool_call_id: "tc-integ-1".into(),
        tool_name: "polkagent.test.tool".into(),
        arguments_json: r#"{"key":"value"}"#.into(),
    };
    let executor = FakeExecutor::with_tool_calls(vec![call]);
    let stream = executor.stream(minimal_request()).await.expect("stream ok");
    let events: Vec<_> = stream.collect().await;

    // Should contain a ToolCallComplete event.
    let has_tool_complete = events
        .iter()
        .any(|e| matches!(e, Ok(StreamEvent::ToolCallComplete { .. })));
    assert!(
        has_tool_complete,
        "stream should contain ToolCallComplete event"
    );

    // Last event must always be Completed.
    assert!(matches!(
        events.last().expect("events"),
        Ok(StreamEvent::Completed { .. })
    ));
}

// ---------------------------------------------------------------------------
// Request recording works across crate boundary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fake_executor_records_last_request_across_crate_boundary() {
    let executor = FakeExecutor::new();
    assert!(executor.last_request().is_none());

    let req = minimal_request();
    let run_id = req.run_id;
    executor.complete(req).await.expect("ok");

    let captured = executor
        .last_request()
        .expect("should have recorded request");
    assert_eq!(captured.run_id, run_id);
    assert_eq!(executor.call_count(), 1);
}

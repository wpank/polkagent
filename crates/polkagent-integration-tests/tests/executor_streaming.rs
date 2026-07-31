//! IT-06: Executor streaming integration tests.
//!
//! Exercises FakeExecutor's streaming API: token order, tool call events,
//! multi-turn, empty response, and error propagation.

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
            content: vec![ContentBlock::Text { text: "hello".into() }],
        }],
        system: None,
        tools: vec![],
        model_id: "claude-opus-4-6".into(),
        max_tokens: 1024,
        temperature: None,
    }
}

fn request_with_text(text: &str) -> InferenceRequest {
    InferenceRequest {
        messages: vec![InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }],
        ..minimal_request()
    }
}

fn make_response(text: &str) -> InferenceResponse {
    InferenceResponse {
        text: text.into(),
        tool_calls: vec![],
        stop_reason: "end_turn".into(),
        usage: TokenUsage { input_tokens: 5, output_tokens: 3, ..Default::default() },
        provider_request_id: None,
    }
}

// ---------------------------------------------------------------------------
// IT-06-A: Basic streaming — text deltas in correct order
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_emits_text_delta_per_character_in_order() {
    let executor = FakeExecutor::new();
    let stream = executor
        .stream(minimal_request())
        .await
        .expect("stream should not fail");

    let events: Vec<_> = stream.collect().await;

    let deltas: Vec<String> = events
        .iter()
        .filter_map(|e| {
            if let Ok(StreamEvent::TextDelta { delta }) = e {
                Some(delta.clone())
            } else {
                None
            }
        })
        .collect();

    // The default FakeExecutor text is "I am a fake assistant".
    let reconstructed = deltas.concat();
    assert_eq!(reconstructed, "I am a fake assistant");
}

#[tokio::test]
async fn stream_ends_with_completed_event() {
    let executor = FakeExecutor::new();
    let stream = executor
        .stream(minimal_request())
        .await
        .expect("stream ok");

    let events: Vec<_> = stream.collect().await;

    assert!(
        !events.is_empty(),
        "stream must emit at least one event"
    );

    // The last event must be Completed.
    match events.last().expect("events not empty") {
        Ok(StreamEvent::Completed { result }) => {
            assert_eq!(result.stop_reason, "end_turn");
        }
        other => panic!("expected Completed as last event, got {other:?}"),
    }
}

#[tokio::test]
async fn stream_completed_event_carries_full_response() {
    let response = make_response("Hello world");
    let executor = FakeExecutor::with_responses(vec![response.clone()]);
    let stream = executor.stream(minimal_request()).await.expect("stream");
    let events: Vec<_> = stream.collect().await;

    let completed = events
        .iter()
        .find_map(|e| {
            if let Ok(StreamEvent::Completed { result }) = e {
                Some(result)
            } else {
                None
            }
        })
        .expect("Completed event must be present");

    assert_eq!(completed.text, "Hello world");
    assert_eq!(completed.stop_reason, "end_turn");
}

// ---------------------------------------------------------------------------
// IT-06-B: Token order — deltas reconstruct the full text
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_tokens_reconstruct_complete_text() {
    let text = "The quick brown fox";
    let executor = FakeExecutor::with_responses(vec![make_response(text)]);

    let stream = executor.stream(minimal_request()).await.expect("stream");
    let events: Vec<_> = stream.collect().await;

    let collected: String = events
        .iter()
        .filter_map(|e| {
            if let Ok(StreamEvent::TextDelta { delta }) = e {
                Some(delta.as_str())
            } else {
                None
            }
        })
        .collect();

    assert_eq!(collected, text, "concatenated deltas must equal the original text");
}

#[tokio::test]
async fn streaming_character_count_matches_text_length() {
    let text = "abc";
    let executor = FakeExecutor::with_responses(vec![make_response(text)]);

    let stream = executor.stream(minimal_request()).await.expect("stream");
    let events: Vec<_> = stream.collect().await;

    let delta_count = events
        .iter()
        .filter(|e| matches!(e, Ok(StreamEvent::TextDelta { .. })))
        .count();

    // FakeExecutor emits one delta per character.
    assert_eq!(delta_count, text.len(), "one TextDelta per character expected");
}

// ---------------------------------------------------------------------------
// IT-06-C: Tool call events
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_with_tool_calls_emits_tool_call_complete_event() {
    let call = ToolCall {
        tool_call_id: "tc-001".into(),
        tool_name: "polkagent.file.read".into(),
        arguments_json: r#"{"path": "/tmp/test"}"#.into(),
    };
    let executor = FakeExecutor::with_tool_calls(vec![call.clone()]);

    let stream = executor.stream(minimal_request()).await.expect("stream");
    let events: Vec<_> = stream.collect().await;

    let tool_events: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let Ok(StreamEvent::ToolCallComplete { call }) = e {
                Some(call)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(tool_events.len(), 1, "exactly one ToolCallComplete event expected");
    assert_eq!(tool_events[0].tool_name, "polkagent.file.read");
    assert_eq!(tool_events[0].tool_call_id, "tc-001");
}

#[tokio::test]
async fn stream_with_multiple_tool_calls_emits_all() {
    let calls = vec![
        ToolCall {
            tool_call_id: "tc-1".into(),
            tool_name: "tool.a".into(),
            arguments_json: "{}".into(),
        },
        ToolCall {
            tool_call_id: "tc-2".into(),
            tool_name: "tool.b".into(),
            arguments_json: "{}".into(),
        },
    ];
    let executor = FakeExecutor::with_tool_calls(calls);

    let stream = executor.stream(minimal_request()).await.expect("stream");
    let events: Vec<_> = stream.collect().await;

    let tool_names: Vec<&str> = events
        .iter()
        .filter_map(|e| {
            if let Ok(StreamEvent::ToolCallComplete { call }) = e {
                Some(call.tool_name.as_str())
            } else {
                None
            }
        })
        .collect();

    assert_eq!(tool_names.len(), 2);
    assert!(tool_names.contains(&"tool.a"));
    assert!(tool_names.contains(&"tool.b"));
}

#[tokio::test]
async fn tool_call_response_has_stop_reason_tool_use() {
    let call = ToolCall {
        tool_call_id: "tc-x".into(),
        tool_name: "my.tool".into(),
        arguments_json: "{}".into(),
    };
    let executor = FakeExecutor::with_tool_calls(vec![call]);

    let stream = executor.stream(minimal_request()).await.expect("stream");
    let events: Vec<_> = stream.collect().await;

    let completed = events
        .iter()
        .find_map(|e| {
            if let Ok(StreamEvent::Completed { result }) = e {
                Some(result)
            } else {
                None
            }
        })
        .expect("Completed event");

    assert_eq!(completed.stop_reason, "tool_use");
    assert!(!completed.tool_calls.is_empty());
}

// ---------------------------------------------------------------------------
// IT-06-D: Multi-turn — cycling responses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_turn_executor_cycles_through_responses() {
    let responses = vec![
        make_response("first turn"),
        make_response("second turn"),
        make_response("third turn"),
    ];
    let executor = FakeExecutor::with_responses(responses);

    for (i, expected) in ["first turn", "second turn", "third turn"].iter().enumerate() {
        let resp = executor.complete(minimal_request()).await.expect("complete");
        assert_eq!(resp.text, *expected, "turn {i} response mismatch");
    }

    // Cycle back to the first.
    let wrapped = executor.complete(minimal_request()).await.expect("wrap");
    assert_eq!(wrapped.text, "first turn", "should cycle back to first response");
}

#[tokio::test]
async fn multi_turn_call_count_accumulates() {
    let executor = FakeExecutor::new();
    assert_eq!(executor.call_count(), 0);

    for _ in 0..5 {
        executor.complete(minimal_request()).await.expect("ok");
    }
    assert_eq!(executor.call_count(), 5);

    // Stream calls also count.
    let stream = executor.stream(minimal_request()).await.expect("stream");
    let _: Vec<_> = stream.collect().await;
    assert_eq!(executor.call_count(), 6);
}

// ---------------------------------------------------------------------------
// IT-06-E: Empty response
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_response_produces_no_text_deltas() {
    let empty_response = InferenceResponse {
        text: String::new(),
        tool_calls: vec![],
        stop_reason: "end_turn".into(),
        usage: TokenUsage::default(),
        provider_request_id: None,
    };
    let executor = FakeExecutor::with_responses(vec![empty_response]);

    let stream = executor.stream(minimal_request()).await.expect("stream");
    let events: Vec<_> = stream.collect().await;

    let delta_count = events
        .iter()
        .filter(|e| matches!(e, Ok(StreamEvent::TextDelta { .. })))
        .count();

    assert_eq!(delta_count, 0, "empty response must produce no TextDelta events");

    // Should still have a Completed event.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(StreamEvent::Completed { .. }))),
        "Completed event must still be emitted for empty response"
    );
}

// ---------------------------------------------------------------------------
// IT-06-F: Error propagation in streaming
// ---------------------------------------------------------------------------

#[tokio::test]
async fn failing_executor_propagates_error_from_complete() {
    let executor = FakeExecutor::failing(ExecutorError::Cancelled);
    let result = executor.complete(minimal_request()).await;
    assert!(
        matches!(result, Err(ExecutorError::Cancelled)),
        "failing executor must return Cancelled error from complete"
    );
}

#[tokio::test]
async fn failing_executor_propagates_error_from_stream() {
    let executor = FakeExecutor::failing(ExecutorError::Transport {
        message: "simulated failure".into(),
        retryable: true,
    });
    let result = executor.stream(minimal_request()).await;
    assert!(
        result.is_err(),
        "failing executor must return error from stream"
    );
}

#[tokio::test]
async fn rate_limit_error_is_retryable() {
    let executor = FakeExecutor::failing(ExecutorError::RateLimit { retry_after_secs: Some(30) });
    let err = executor
        .complete(minimal_request())
        .await
        .expect_err("must fail");
    assert!(err.is_retryable(), "RateLimit error must be retryable");
}

#[tokio::test]
async fn cancelled_error_is_not_retryable() {
    let executor = FakeExecutor::failing(ExecutorError::Cancelled);
    let err = executor
        .complete(minimal_request())
        .await
        .expect_err("must fail");
    assert!(!err.is_retryable(), "Cancelled error must not be retryable");
}

// ---------------------------------------------------------------------------
// IT-06-G: Request recording
// ---------------------------------------------------------------------------

#[tokio::test]
async fn last_request_is_recorded_after_complete() {
    let executor = FakeExecutor::new();
    assert!(executor.last_request().is_none());

    let req = request_with_text("Record me");
    executor.complete(req).await.expect("ok");

    let captured = executor.last_request().expect("should have request");
    if let ContentBlock::Text { text } = &captured.messages[0].content[0] {
        assert_eq!(text, "Record me");
    } else {
        panic!("expected text content block");
    }
}

#[tokio::test]
async fn last_request_is_updated_on_subsequent_calls() {
    let executor = FakeExecutor::new();

    executor.complete(request_with_text("first")).await.expect("ok");
    executor.complete(request_with_text("second")).await.expect("ok");

    let captured = executor.last_request().expect("request");
    if let ContentBlock::Text { text } = &captured.messages[0].content[0] {
        assert_eq!(text, "second", "last_request must reflect the most recent call");
    } else {
        panic!("expected text content block");
    }
}

// ---------------------------------------------------------------------------
// IT-06-H: Health check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn healthy_executor_reports_ok() {
    let executor = FakeExecutor::new();
    assert!(executor.health().await.is_ok());
}

#[tokio::test]
async fn failing_executor_reports_unhealthy() {
    let executor = FakeExecutor::failing(ExecutorError::Internal {
        message: "sick".into(),
    });
    assert!(executor.health().await.is_err());
}

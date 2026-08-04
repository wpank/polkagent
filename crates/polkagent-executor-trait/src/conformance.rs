//! Shared conformance test suite for [`ModelExecutor`] implementations.
//!
//! This module defines the canonical set of behavioural tests that **every**
//! adapter implementing [`ModelExecutor`] must pass (PRD-15).  Tests are
//! plain `async fn`s — not macros — so each adapter can invoke them from its
//! own `tests/` directory without any macro magic.
//!
//! # Usage
//!
//! ```rust,ignore
//! // In your adapter crate: tests/conformance.rs
//! use polkagent_executor_trait::conformance;
//! use my_adapter::MyExecutor;
//!
//! #[tokio::test]
//! async fn test_execute_returns_response() {
//!     let exec = MyExecutor::new_for_tests();
//!     conformance::test_execute_returns_response(exec.as_ref()).await;
//! }
//! ```
//!
//! # Feature gate
//!
//! This module is compiled only when the `test-contracts` feature is enabled.
//! Add `polkagent-executor-trait = { ..., features = ["test-contracts"] }` to
//! the `[dev-dependencies]` of your adapter crate.

use futures::StreamExt;

use crate::{
    ContentBlock, InferenceMessage, InferenceRequest, MessageRole, ModelExecutor, StreamEvent,
    ToolDefinition,
};
use polkagent_core::{RunId, StepId};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal, structurally valid [`InferenceRequest`].
///
/// Conformance tests build on this helper so each individual test is focused
/// on one invariant without duplicating boilerplate.
#[must_use]
pub fn minimal_request() -> InferenceRequest {
    InferenceRequest {
        run_id: RunId::new(),
        step_id: StepId::new(),
        messages: vec![InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: "Conformance probe — please respond.".into(),
            }],
        }],
        system: Some("You are a conformance-test assistant.".into()),
        tools: vec![],
        model_id: "test-model".into(),
        max_tokens: 256,
        temperature: None,
    }
}

/// Build a request that carries one tool definition.
#[must_use]
pub fn request_with_tool() -> InferenceRequest {
    let mut req = minimal_request();
    req.tools = vec![ToolDefinition {
        name: "conformance.echo".into(),
        description: "Echoes the input back to the caller.".into(),
        input_schema_json:
            r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#
                .into(),
    }];
    req
}

/// Build a request with a low `max_tokens` ceiling to probe truncation.
#[must_use]
pub fn request_low_max_tokens() -> InferenceRequest {
    let mut req = minimal_request();
    req.max_tokens = 1;
    req
}

/// Build a request whose single user message is empty text.
#[must_use]
pub fn request_empty_prompt() -> InferenceRequest {
    InferenceRequest {
        run_id: RunId::new(),
        step_id: StepId::new(),
        messages: vec![InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: String::new(),
            }],
        }],
        system: None,
        tools: vec![],
        model_id: "test-model".into(),
        max_tokens: 256,
        temperature: None,
    }
}

// ---------------------------------------------------------------------------
// Conformance tests
// ---------------------------------------------------------------------------

/// Conformance: `complete()` returns a response with content or a stop_reason.
///
/// Every adapter must return an [`crate::InferenceResponse`] whose `text` or
/// `tool_calls` is non-empty, and whose `stop_reason` is a non-empty string.
pub async fn test_execute_returns_response(exec: &dyn ModelExecutor) {
    let resp = exec
        .complete(minimal_request())
        .await
        .expect("complete() must not fail for a valid minimal request");

    let has_content = !resp.text.is_empty() || !resp.tool_calls.is_empty();
    assert!(
        has_content,
        "complete() must return a response with non-empty text or at least one tool_call; \
         got text={:?} tool_calls={:?}",
        resp.text, resp.tool_calls,
    );

    assert!(
        !resp.stop_reason.is_empty(),
        "complete() must set a non-empty stop_reason; got {:?}",
        resp.stop_reason,
    );
}

/// Conformance: `complete()` with tool definitions returns a usable response.
///
/// When the request includes tool definitions the adapter must accept them
/// without error. The response may contain tool calls or a plain-text
/// response; both are valid.  What is not valid is a panic or error.
pub async fn test_execute_with_tools(exec: &dyn ModelExecutor) {
    let resp = exec
        .complete(request_with_tool())
        .await
        .expect("complete() must not fail when tools are provided");

    // Response must still have a stop_reason.
    assert!(
        !resp.stop_reason.is_empty(),
        "complete() with tools must set a non-empty stop_reason; got {:?}",
        resp.stop_reason,
    );
}

/// Conformance: `complete()` respects the `max_tokens` ceiling.
///
/// When `max_tokens = 1` the response text should be very short (or the
/// stop_reason should signal truncation).  Adapters must not produce an error
/// for this scenario — low token limits are a valid caller choice.
pub async fn test_execute_respects_max_tokens(exec: &dyn ModelExecutor) {
    let result = exec.complete(request_low_max_tokens()).await;
    // The adapter must not propagate a low max_tokens value as an error.
    // It may return any valid response or a max_tokens stop_reason.
    match result {
        Ok(resp) => {
            assert!(
                !resp.stop_reason.is_empty(),
                "complete() with low max_tokens must still set a stop_reason"
            );
        }
        Err(e) => {
            panic!("complete() must not fail for a low max_tokens request; got error: {e}");
        }
    }
}

/// Conformance: `stream()` yields at least one event and always ends with
/// `StreamEvent::Completed`.
///
/// The trait contract requires that the stream is always terminated by a
/// `Completed` variant, even for non-streaming adapters that collect the full
/// response and emit it as a single terminal event.
pub async fn test_stream_yields_events(exec: &dyn ModelExecutor) {
    let stream = exec
        .stream(minimal_request())
        .await
        .expect("stream() must not fail for a valid minimal request");

    let events: Vec<_> = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("stream must not yield error events for a valid minimal request");

    assert!(!events.is_empty(), "stream() must emit at least one event");

    let last = events.last().expect("events is non-empty");
    assert!(
        matches!(last, StreamEvent::Completed { .. }),
        "the last stream event must be StreamEvent::Completed; got: {last:?}"
    );
}

/// Conformance: `stream()` Completed event carries a non-empty stop_reason.
///
/// The `result` embedded in the terminal `Completed` event must match the
/// contract of a `complete()` call: `stop_reason` must be non-empty.
pub async fn test_stream_completed_carries_stop_reason(exec: &dyn ModelExecutor) {
    let stream = exec
        .stream(minimal_request())
        .await
        .expect("stream() must not fail for a valid minimal request");

    let events: Vec<_> = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("stream must not yield error events");

    let completed = events
        .into_iter()
        .find(|e| matches!(e, StreamEvent::Completed { .. }))
        .expect("stream must contain a Completed event");

    if let StreamEvent::Completed { result } = completed {
        assert!(
            !result.stop_reason.is_empty(),
            "Completed event result must carry a non-empty stop_reason; got {:?}",
            result.stop_reason,
        );
    }
}

/// Conformance: `complete()` with an empty prompt must not panic.
///
/// An empty user message is unusual but structurally valid.  Adapters must
/// handle it gracefully — either returning a response or a well-formed error.
/// What is not acceptable is a panic or an internal unwrap failure.
pub async fn test_empty_prompt_handled(exec: &dyn ModelExecutor) {
    // We do not assert the result is Ok — some adapters may reject empty
    // prompts with a well-typed error.  We only assert no panic occurs.
    let _result = exec.complete(request_empty_prompt()).await;
}

/// Conformance: `complete()` token usage is non-zero for successful calls.
///
/// Adapters must track token consumption and report it.  At minimum the sum
/// of `input_tokens + output_tokens` must be greater than zero.
pub async fn test_token_usage_reported(exec: &dyn ModelExecutor) {
    let resp = exec
        .complete(minimal_request())
        .await
        .expect("complete() must not fail for a valid minimal request");

    let total = resp.usage.input_tokens + resp.usage.output_tokens;
    assert!(
        total > 0,
        "complete() must report non-zero token usage; got input={} output={}",
        resp.usage.input_tokens,
        resp.usage.output_tokens,
    );
}

/// Conformance: `health()` returns without panicking.
///
/// Adapters must implement a health check that does not panic.  The result
/// may be `Ok` or a well-typed error — both are acceptable.
pub async fn test_health_returns_result(exec: &dyn ModelExecutor) {
    // Any result is acceptable; we only verify no panic occurs.
    let _result = exec.health().await;
}

/// Conformance: context-overflow yields a well-typed error or a valid response.
///
/// Sending a message whose content is deliberately large exercises the adapter's
/// handling of context pressure.  The adapter must not panic; it may return
/// either a successful (possibly truncated) response or a well-formed
/// `ExecutorError::ContextWindowExceeded`.
pub async fn test_context_overflow_handled(exec: &dyn ModelExecutor) {
    let large_text = "x".repeat(100_000);
    let req = InferenceRequest {
        run_id: RunId::new(),
        step_id: StepId::new(),
        messages: vec![InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text { text: large_text }],
        }],
        system: None,
        tools: vec![],
        model_id: "test-model".into(),
        max_tokens: 256,
        temperature: None,
    };

    // We accept any non-panicking outcome.
    let _result = exec.complete(req).await;
}

/// Conformance: each successful `complete()` call produces a unique
/// `provider_request_id` (or `None`, which is also acceptable).
///
/// If the adapter assigns request identifiers, they must differ across calls.
/// Adapters that always return `None` also satisfy this contract.
pub async fn test_provider_request_id_varies(exec: &dyn ModelExecutor) {
    let r1 = exec
        .complete(minimal_request())
        .await
        .expect("first complete() must not fail");
    let r2 = exec
        .complete(minimal_request())
        .await
        .expect("second complete() must not fail");

    if let (Some(id1), Some(id2)) = (&r1.provider_request_id, &r2.provider_request_id) {
        assert_ne!(
            id1, id2,
            "each call to complete() must produce a unique provider_request_id; \
             both calls returned {id1:?}"
        );
    }
    // If either is None, the adapter does not assign request IDs — that is fine.
}

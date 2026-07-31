//! Contract and conformance tests for [`FakeExecutor`].
//!
//! These tests verify that the fake executor implementation conforms to the
//! [`ModelExecutor`] trait contract and the shared PRD-15 conformance suite
//! defined in `polkagent-executor-trait`.

use polkagent_executor_fake::FakeExecutor;
use polkagent_executor_trait::{conformance, contracts};

// ---------------------------------------------------------------------------
// Legacy contracts (kept for backwards compatibility)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn complete_returns_response() {
    let executor = FakeExecutor::new();
    contracts::test_complete_returns_response(executor.as_ref()).await;
}

#[tokio::test]
async fn complete_tracks_token_usage() {
    let executor = FakeExecutor::new();
    contracts::test_complete_tracks_token_usage(executor.as_ref()).await;
}

#[tokio::test]
async fn stream_emits_completed() {
    let executor = FakeExecutor::new();
    contracts::test_stream_emits_completed(executor.as_ref()).await;
}

#[tokio::test]
async fn health_returns_result() {
    let executor = FakeExecutor::new();
    contracts::test_health_returns_result(executor.as_ref()).await;
}

// ---------------------------------------------------------------------------
// PRD-15 conformance suite
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_execute_returns_response() {
    let exec = FakeExecutor::new();
    conformance::test_execute_returns_response(exec.as_ref()).await;
}

#[tokio::test]
async fn conformance_execute_with_tools() {
    let exec = FakeExecutor::new();
    conformance::test_execute_with_tools(exec.as_ref()).await;
}

#[tokio::test]
async fn conformance_execute_respects_max_tokens() {
    let exec = FakeExecutor::new();
    conformance::test_execute_respects_max_tokens(exec.as_ref()).await;
}

#[tokio::test]
async fn conformance_stream_yields_events() {
    let exec = FakeExecutor::new();
    conformance::test_stream_yields_events(exec.as_ref()).await;
}

#[tokio::test]
async fn conformance_stream_completed_carries_stop_reason() {
    let exec = FakeExecutor::new();
    conformance::test_stream_completed_carries_stop_reason(exec.as_ref()).await;
}

#[tokio::test]
async fn conformance_empty_prompt_handled() {
    let exec = FakeExecutor::new();
    conformance::test_empty_prompt_handled(exec.as_ref()).await;
}

#[tokio::test]
async fn conformance_token_usage_reported() {
    let exec = FakeExecutor::new();
    conformance::test_token_usage_reported(exec.as_ref()).await;
}

#[tokio::test]
async fn conformance_health_returns_result() {
    let exec = FakeExecutor::new();
    conformance::test_health_returns_result(exec.as_ref()).await;
}

#[tokio::test]
async fn conformance_context_overflow_handled() {
    let exec = FakeExecutor::new();
    conformance::test_context_overflow_handled(exec.as_ref()).await;
}

#[tokio::test]
async fn conformance_provider_request_id_varies() {
    let exec = FakeExecutor::new();
    conformance::test_provider_request_id_varies(exec.as_ref()).await;
}

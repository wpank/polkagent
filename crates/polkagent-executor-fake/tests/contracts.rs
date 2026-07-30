//! Contract tests for [`FakeExecutor`] via the reusable contract test suite.
//!
//! These tests verify that the fake executor implementation conforms to the
//! [`ModelExecutor`] trait contract defined in `polkagent-executor-trait`.

use polkagent_executor_fake::FakeExecutor;
use polkagent_executor_trait::contracts;

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

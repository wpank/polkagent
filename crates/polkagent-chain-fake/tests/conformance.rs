//! PRD-15 conformance tests for [`FakeChainClient`].
//!
//! These tests verify that the fake chain client implementation passes the
//! shared conformance suite from `polkagent-chain-trait`.
//!
//! The `FakeChainClient` is configured with the "conformance-test" chain
//! profile to match [`polkagent_chain_trait::conformance::test_profile_id`].

use polkagent_chain_fake::FakeChainClientBuilder;
use polkagent_chain_trait::conformance;

/// Build a `FakeChainClient` configured for the conformance test profile.
fn conformance_client() -> polkagent_chain_fake::FakeChainClient {
    FakeChainClientBuilder::new("conformance-test")
        .with_block_number(42)
        .with_runtime_version(1_000, 0)
        .build()
}

#[tokio::test]
async fn test_get_runtime_version() {
    let client = conformance_client();
    conformance::test_get_runtime_version(&client).await;
}

#[tokio::test]
async fn test_query_storage() {
    let client = conformance_client();
    conformance::test_query_storage(&client).await;
}

#[tokio::test]
async fn test_get_block_number() {
    let client = conformance_client();
    conformance::test_get_block_number(&client).await;
}

#[tokio::test]
async fn test_submit_extrinsic() {
    let client = conformance_client();
    conformance::test_submit_extrinsic(&client).await;
}

#[tokio::test]
async fn test_genesis_hash_not_empty() {
    let client = conformance_client();
    conformance::test_genesis_hash_not_empty(&client).await;
}

#[tokio::test]
async fn test_decode_call_no_panic() {
    let client = conformance_client();
    conformance::test_decode_call_no_panic(&client).await;
}

#[tokio::test]
async fn test_watch_finality_unknown_on_timeout() {
    let client = conformance_client();
    conformance::test_watch_finality_unknown_on_timeout(&client).await;
}

#[tokio::test]
async fn test_simulate_invalid_extrinsic() {
    let client = conformance_client();
    conformance::test_simulate_invalid_extrinsic(&client).await;
}

#[tokio::test]
async fn test_health_returns_result() {
    let client = conformance_client();
    conformance::test_health_returns_result(&client).await;
}

#[tokio::test]
async fn test_pinned_metadata_timestamp_plausible() {
    let client = conformance_client();
    conformance::test_pinned_metadata_timestamp_plausible(&client).await;
}

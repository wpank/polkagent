//! Live-node smoke tests for the HTTP JSON-RPC transport.
//!
//! These tests intentionally skip unless the corresponding endpoint
//! environment variable is set. The E2E workflow sets both variables after
//! spawning the Zombienet fixture; ordinary workspace test runs stay offline.

#![allow(clippy::expect_used)]

use std::time::{Duration, Instant};

use polkagent_chain_subxt::{config::SubxtConfig, rpc::RpcClient};

const FINALITY_TIMEOUT: Duration = Duration::from_secs(60);
const FINALITY_POLL_INTERVAL: Duration = Duration::from_secs(2);

fn endpoint_from_env(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(endpoint) if !endpoint.trim().is_empty() => Some(endpoint),
        _ => {
            eprintln!("skipping live RPC smoke test: {name} is not set");
            None
        }
    }
}

fn rpc_client() -> RpcClient {
    let config = SubxtConfig::builder()
        .request_timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(5))
        .max_retries(2)
        .build();
    RpcClient::new(config).expect("live-test RPC client configuration should be valid")
}

async fn assert_read_surface(endpoint: &str) {
    assert!(
        endpoint.starts_with("http://") || endpoint.starts_with("https://"),
        "polkagent-chain-subxt uses HTTP JSON-RPC; got {endpoint}"
    );

    let rpc = rpc_client();
    let health = rpc
        .system_health(endpoint)
        .await
        .expect("system_health should succeed against the live node");
    let version = rpc
        .system_version(endpoint)
        .await
        .expect("system_version should succeed against the live node");
    let genesis = rpc
        .chain_get_block_hash(endpoint, Some(0))
        .await
        .expect("genesis block hash should be available");
    let latest = rpc
        .chain_get_block_hash(endpoint, None)
        .await
        .expect("latest block hash should be available");
    let finalized = rpc
        .chain_get_finalized_head(endpoint)
        .await
        .expect("finalized head should be available");
    let finalized_header = rpc
        .chain_get_header(endpoint, Some(&finalized))
        .await
        .expect("finalized header should be available");
    let metadata = rpc
        .state_get_metadata(endpoint, Some(&finalized))
        .await
        .expect("runtime metadata should be available at the finalized head");

    assert!(!version.trim().is_empty(), "node version must not be empty");
    assert!(genesis.starts_with("0x") && genesis.len() > 2);
    assert!(latest.starts_with("0x") && latest.len() > 2);
    assert!(finalized.starts_with("0x") && finalized.len() > 2);
    assert!(
        finalized_header.get("number").is_some(),
        "finalized header must contain a block number"
    );
    assert!(metadata.starts_with("0x") && metadata.len() > 2);

    eprintln!(
        "live node ready: version={version}, peers={}, syncing={}, finalized={finalized}",
        health.peers, health.is_syncing
    );
}

#[tokio::test]
async fn relay_chain_rpc_reads_and_finality_progresses() {
    let Some(endpoint) = endpoint_from_env("POLKAGENT_E2E_RPC") else {
        return;
    };

    assert_read_surface(&endpoint).await;

    let rpc = rpc_client();
    let initial = rpc
        .chain_get_finalized_head(&endpoint)
        .await
        .expect("initial finalized head should be available");
    let deadline = Instant::now() + FINALITY_TIMEOUT;

    while Instant::now() < deadline {
        tokio::time::sleep(FINALITY_POLL_INTERVAL).await;
        let current = rpc
            .chain_get_finalized_head(&endpoint)
            .await
            .expect("finalized-head polling should succeed");
        if current != initial {
            eprintln!("relay finality progressed: {initial} -> {current}");
            return;
        }
    }

    panic!("relay finalized head did not progress within {FINALITY_TIMEOUT:?}");
}

#[tokio::test]
async fn parachain_rpc_exposes_live_read_surface() {
    let Some(endpoint) = endpoint_from_env("POLKAGENT_E2E_PARACHAIN_RPC") else {
        return;
    };

    assert_read_surface(&endpoint).await;
}

//! End-to-end integration tests for the explain-and-sign pipeline.
//!
//! These tests exercise the full `AppService::explain_and_sign` method using
//! `FakeExecutor`, `FakeSigner`, and `FakeChainClient` to verify:
//!
//! 1. The happy-path pipeline: decode -> action card -> sign -> submit -> finality.
//! 2. **AC-P2-003:** The signer receives the EXACT original call bytes.
//! 3. Error paths: missing signer, missing chain client, chain faults.
//! 4. The decode-and-explain stage produces valid action cards.
//! 5. The sign-and-submit stage produces valid tx hashes.
//! 6. The signer-rejected path returns the appropriate service error.

use std::sync::Arc;

use polkagent_chain_fake::FakeChainClientBuilder;
use polkagent_chain_trait::{ChainClient, ChainProfileId, FinalityObservation};
use polkagent_config::Config;
use polkagent_core::{AgentId, RunId};
use polkagent_event::{EventBus, EventRecorder};
use polkagent_executor_fake::FakeExecutor;
use polkagent_executor_trait::ModelExecutor;
use polkagent_service::explain::ExplainRequest;
use polkagent_service::{AppService, ServiceError};
use polkagent_signer_fake::FakeSigner;
use polkagent_signer_trait::Signer;
use polkagent_store_trait::event::EventStore;

use polkagent_integration_tests::{MemEventStore, MemRunStore};

// ---------------------------------------------------------------------------
// Builder helpers
// ---------------------------------------------------------------------------

/// Build an `AppService` with all components wired, including signer and chain.
fn make_full_service(signer: Arc<dyn Signer>, chain: Arc<dyn ChainClient>) -> AppService {
    let run_store = Arc::new(MemRunStore::default());
    let event_store = Arc::new(MemEventStore::default());
    let event_store_dyn = Arc::clone(&event_store) as Arc<dyn EventStore>;
    let bus = EventBus::new(64);
    let recorder = EventRecorder::new(event_store_dyn, bus.clone());
    let executor: Arc<dyn ModelExecutor> = FakeExecutor::new();

    AppService::builder()
        .with_config(Config::default())
        .with_run_store(run_store)
        .with_event_bus(bus)
        .with_event_recorder(recorder)
        .with_executor(executor)
        .with_signer(signer)
        .with_chain_client(chain)
        .build()
        .expect("build full service")
}

/// Build an `AppService` without a signer (chain client only).
fn make_service_without_signer() -> AppService {
    let run_store = Arc::new(MemRunStore::default());
    let event_store = Arc::new(MemEventStore::default());
    let event_store_dyn = Arc::clone(&event_store) as Arc<dyn EventStore>;
    let bus = EventBus::new(64);
    let recorder = EventRecorder::new(event_store_dyn, bus.clone());

    let chain: Arc<dyn ChainClient> = Arc::new(FakeChainClientBuilder::new("polkadot").build());

    AppService::builder()
        .with_config(Config::default())
        .with_run_store(run_store)
        .with_event_bus(bus)
        .with_event_recorder(recorder)
        .with_chain_client(chain)
        // deliberately omitting .with_signer(...)
        .build()
        .expect("build service without signer")
}

/// Build an `AppService` without a chain client (signer only).
fn make_service_without_chain_client() -> AppService {
    let run_store = Arc::new(MemRunStore::default());
    let event_store = Arc::new(MemEventStore::default());
    let event_store_dyn = Arc::clone(&event_store) as Arc<dyn EventStore>;
    let bus = EventBus::new(64);
    let recorder = EventRecorder::new(event_store_dyn, bus.clone());

    let signer: Arc<dyn Signer> = Arc::new(FakeSigner::new());

    AppService::builder()
        .with_config(Config::default())
        .with_run_store(run_store)
        .with_event_bus(bus)
        .with_event_recorder(recorder)
        .with_signer(signer)
        // deliberately omitting .with_chain_client(...)
        .build()
        .expect("build service without chain client")
}

/// Build a default `ExplainRequest` with the given call bytes.
fn make_explain_request(call_bytes: Vec<u8>) -> ExplainRequest {
    ExplainRequest {
        call_bytes,
        chain_profile: ChainProfileId::new("polkadot"),
        run_id: RunId::new(),
        agent_id: AgentId::new(),
    }
}

// ---------------------------------------------------------------------------
// 1. Happy path: full pipeline succeeds end-to-end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_pipeline_succeeds_with_fake_adapters() {
    let signer = Arc::new(FakeSigner::new());
    let chain = Arc::new(FakeChainClientBuilder::new("polkadot").build());
    let svc = make_full_service(signer, chain);

    let call_bytes = vec![0x04, 0x00, 0x01, 0x02, 0x03, 0x04];
    let request = make_explain_request(call_bytes);

    let result = svc.explain_and_sign(request).await;
    assert!(
        result.is_ok(),
        "full pipeline should succeed, got: {result:?}"
    );

    let sign_result = result.unwrap();
    // The tx hash should be a hex string starting with "0x".
    assert!(
        sign_result.tx_hash.0.starts_with("0x"),
        "tx hash should be hex-encoded, got: {}",
        sign_result.tx_hash
    );
    // The tx hash must be non-empty (beyond the "0x" prefix).
    assert!(
        sign_result.tx_hash.0.len() > 2,
        "tx hash should have content beyond the 0x prefix"
    );
}

// ---------------------------------------------------------------------------
// 2. AC-P2-003: Signer receives the EXACT original bytes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn signer_receives_exact_original_bytes_ac_p2_003() {
    let signer = Arc::new(FakeSigner::new());
    let chain = Arc::new(FakeChainClientBuilder::new("polkadot").build());
    let signer_clone = Arc::clone(&signer);
    let svc = make_full_service(signer, chain);

    let original_bytes = vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
    let request = make_explain_request(original_bytes.clone());

    let result = svc.explain_and_sign(request).await;
    assert!(result.is_ok(), "pipeline should succeed");

    // Verify the signer was called exactly once.
    assert_eq!(
        signer_clone.sign_call_count(),
        1,
        "signer should have been called exactly once"
    );

    // AC-P2-003: The signer must have received the exact original bytes.
    let last_req = signer_clone
        .last_request()
        .expect("signer should have recorded the request");
    assert_eq!(
        last_req.payload, original_bytes,
        "AC-P2-003 violated: signer payload does not match original call bytes"
    );
}

// ---------------------------------------------------------------------------
// 3. Missing signer returns NotInitialized error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_signer_returns_not_initialized() {
    let svc = make_service_without_signer();
    let request = make_explain_request(vec![0x01, 0x02, 0x03]);

    let result = svc.explain_and_sign(request).await;
    assert!(result.is_err(), "should fail without signer");

    let err = result.unwrap_err();
    assert!(
        matches!(err, ServiceError::NotInitialized { ref component } if component == "signer"),
        "expected NotInitialized{{signer}}, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. Missing chain client returns NotInitialized error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_chain_client_returns_not_initialized() {
    let svc = make_service_without_chain_client();
    let request = make_explain_request(vec![0x01, 0x02, 0x03]);

    let result = svc.explain_and_sign(request).await;
    assert!(result.is_err(), "should fail without chain client");

    let err = result.unwrap_err();
    assert!(
        matches!(err, ServiceError::NotInitialized { ref component } if component == "chain_client"),
        "expected NotInitialized{{chain_client}}, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 5. Chain client fault during decode returns ExplainPipeline error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chain_fault_during_pipeline_returns_explain_error() {
    let signer = Arc::new(FakeSigner::new());
    // fail_next_n(1): the very first chain call (fetch_metadata in stage 1)
    // will fail, which should propagate as an ExplainPipeline error.
    let chain = Arc::new(
        FakeChainClientBuilder::new("polkadot")
            .fail_next_n(1)
            .build(),
    );
    let svc = make_full_service(signer, chain);

    let request = make_explain_request(vec![0x01, 0x02]);
    let result = svc.explain_and_sign(request).await;

    assert!(result.is_err(), "pipeline should fail when chain faults");
    let err = result.unwrap_err();
    assert!(
        matches!(err, ServiceError::ExplainPipeline { .. }),
        "expected ExplainPipeline error, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. Rejecting signer returns ExplainPipeline error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rejecting_signer_returns_explain_pipeline_error() {
    let signer = Arc::new(FakeSigner::rejecting());
    let chain = Arc::new(FakeChainClientBuilder::new("polkadot").build());
    let svc = make_full_service(signer, chain);

    let request = make_explain_request(vec![0x04, 0x00, 0x01, 0x02]);
    let result = svc.explain_and_sign(request).await;

    assert!(result.is_err(), "pipeline should fail when signer rejects");
    let err = result.unwrap_err();
    assert!(
        matches!(err, ServiceError::ExplainPipeline { .. }),
        "expected ExplainPipeline error from rejected signer, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. Result contains a valid tx hash
// ---------------------------------------------------------------------------

#[tokio::test]
async fn result_contains_tx_hash_with_hex_prefix() {
    let signer = Arc::new(FakeSigner::new());
    let chain = Arc::new(FakeChainClientBuilder::new("polkadot").build());
    let svc = make_full_service(signer, chain);

    let request = make_explain_request(vec![0xAA, 0xBB, 0xCC, 0xDD]);
    let result = svc.explain_and_sign(request).await.expect("should succeed");

    // FakeChainClient produces a 32-byte hex hash = "0x" + 64 hex chars = 66.
    assert!(
        result.tx_hash.0.starts_with("0x"),
        "tx hash must start with 0x"
    );
    assert_eq!(
        result.tx_hash.0.len(),
        66,
        "tx hash should be 66 chars (0x + 64 hex digits)"
    );
}

// ---------------------------------------------------------------------------
// 8. Decode-and-explain stage produces a valid action card (direct call)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn decode_and_explain_produces_action_card() {
    let chain = FakeChainClientBuilder::new("polkadot").build();

    let call_bytes = vec![0x04, 0x00, 0x01, 0x02, 0x03];
    let profile = ChainProfileId::new("polkadot");

    let result = polkagent_service::explain::decode_and_explain(&chain, &call_bytes, profile).await;
    assert!(
        result.is_ok(),
        "decode_and_explain should succeed, got: {result:?}"
    );

    let explain = result.unwrap();

    // The action card title should contain the pallet and call name.
    assert!(
        explain.action_card.title.contains("Balances"),
        "card title should mention the pallet"
    );
    assert!(
        explain.action_card.title.contains("transfer_keep_alive"),
        "card title should mention the call name"
    );

    // The decoded call should match FakeChainClient's hardcoded response.
    assert_eq!(explain.decoded_call.pallet, "Balances");
    assert_eq!(explain.decoded_call.call_name, "transfer_keep_alive");
}

// ---------------------------------------------------------------------------
// 9. Sign-and-submit with FakeSigner and FakeChainClient
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sign_and_submit_returns_tx_hash_and_finality() {
    let signer = FakeSigner::new();
    let chain = FakeChainClientBuilder::new("polkadot").build();

    let original_bytes = vec![0x01, 0x02, 0x03, 0x04, 0x05];
    let profile = ChainProfileId::new("polkadot");

    let result =
        polkagent_service::explain::sign_and_submit(&signer, &chain, &original_bytes, profile)
            .await;
    assert!(
        result.is_ok(),
        "sign_and_submit should succeed, got: {result:?}"
    );

    let sign_result = result.unwrap();
    assert!(
        sign_result.tx_hash.0.starts_with("0x"),
        "tx hash should start with 0x"
    );

    // FakeChainClient returns Unknown finality for unseeded hashes.
    assert!(
        matches!(sign_result.finality, FinalityObservation::Unknown { .. }),
        "finality should be Unknown for unseeded hash, got: {:?}",
        sign_result.finality
    );
}

// ---------------------------------------------------------------------------
// 10. Multiple sequential pipeline runs produce unique tx hashes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_pipeline_runs_produce_unique_tx_hashes() {
    let signer = Arc::new(FakeSigner::new());
    let chain = Arc::new(FakeChainClientBuilder::new("polkadot").build());
    let svc = make_full_service(signer, chain);

    let mut tx_hashes = Vec::new();
    for i in 0u8..3 {
        // Use different call bytes so the FakeChainClient produces different
        // deterministic hashes.
        let call_bytes = vec![0x04, 0x00, i, i + 1, i + 2];
        let request = make_explain_request(call_bytes);
        let result = svc.explain_and_sign(request).await.expect("should succeed");
        tx_hashes.push(result.tx_hash.0.clone());
    }

    // All tx hashes should be distinct (since the call bytes differ).
    let unique: std::collections::HashSet<&String> = tx_hashes.iter().collect();
    assert_eq!(
        unique.len(),
        3,
        "each pipeline run with different bytes should produce a unique tx hash"
    );
}

// ---------------------------------------------------------------------------
// 11. Disconnected chain client fails the pipeline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disconnected_chain_client_fails_pipeline() {
    let signer = Arc::new(FakeSigner::new());
    let chain = Arc::new(
        FakeChainClientBuilder::new("polkadot")
            .disconnected()
            .build(),
    );
    let svc = make_full_service(signer, chain);

    let request = make_explain_request(vec![0x01, 0x02]);
    let result = svc.explain_and_sign(request).await;

    assert!(
        result.is_err(),
        "pipeline should fail with disconnected chain"
    );
    assert!(
        matches!(result.unwrap_err(), ServiceError::ExplainPipeline { .. }),
        "should return ExplainPipeline error"
    );
}

// ---------------------------------------------------------------------------
// 12. Signer call count reflects pipeline invocations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn signer_call_count_matches_pipeline_invocations() {
    let signer = Arc::new(FakeSigner::new());
    let chain = Arc::new(FakeChainClientBuilder::new("polkadot").build());
    let signer_ref = Arc::clone(&signer);
    let svc = make_full_service(signer, chain);

    assert_eq!(signer_ref.sign_call_count(), 0, "no calls yet");

    let req1 = make_explain_request(vec![0x01, 0x02]);
    svc.explain_and_sign(req1).await.expect("run 1 ok");
    assert_eq!(signer_ref.sign_call_count(), 1, "one call after first run");

    let req2 = make_explain_request(vec![0x03, 0x04]);
    svc.explain_and_sign(req2).await.expect("run 2 ok");
    assert_eq!(
        signer_ref.sign_call_count(),
        2,
        "two calls after second run"
    );
}

// ---------------------------------------------------------------------------
// 13. Chain client records decode and submit calls
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chain_client_records_decode_and_submit_calls() {
    let signer = Arc::new(FakeSigner::new());
    let fake_chain = FakeChainClientBuilder::new("polkadot").build();
    let chain_ref = fake_chain.clone();
    let chain: Arc<dyn ChainClient> = Arc::new(fake_chain);
    let svc = make_full_service(signer, chain);

    let call_bytes = vec![0x04, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
    let request = make_explain_request(call_bytes);
    svc.explain_and_sign(request).await.expect("pipeline ok");

    let calls = chain_ref.calls();
    // The pipeline should have made at least these calls:
    // - fetch_metadata (stage 1)
    // - decode_call (stage 1)
    // - fetch_metadata (stage 2 validation)
    // - sign -> submit_extrinsic (stage 3)
    // - watch_finality (stage 3)
    assert!(
        calls.len() >= 4,
        "chain should have recorded at least 4 calls, got: {}",
        calls.len()
    );
    assert!(
        chain_ref.call_count() >= 4,
        "call_count should be at least 4"
    );
}

// ---------------------------------------------------------------------------
// 14. Empty call bytes still go through the pipeline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_call_bytes_pipeline_succeeds() {
    // FakeChainClient handles empty bytes gracefully (returns hardcoded
    // Balances.transfer_keep_alive for any input).
    let signer = Arc::new(FakeSigner::new());
    let chain = Arc::new(FakeChainClientBuilder::new("polkadot").build());
    let signer_ref = Arc::clone(&signer);
    let svc = make_full_service(signer, chain);

    let request = make_explain_request(vec![]);
    let result = svc.explain_and_sign(request).await;

    assert!(
        result.is_ok(),
        "pipeline should handle empty bytes, got: {result:?}"
    );

    // Even with empty call bytes, the signer must receive exactly those bytes.
    let last_req = signer_ref.last_request().expect("request recorded");
    assert!(
        last_req.payload.is_empty(),
        "AC-P2-003: signer payload should be empty vec for empty call bytes"
    );
}

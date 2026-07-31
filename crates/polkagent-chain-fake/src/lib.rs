//! Deterministic fake [`ChainClient`] adapter for testing.
//!
//! This crate provides [`FakeChainClient`] — a fully in-memory, configurable
//! implementation of the [`ChainClient`] trait intended for unit and
//! integration tests. It never makes network calls and never requires a live
//! Substrate/Polkadot node.
//!
//! # Quick start
//!
//! ```rust
//! use polkagent_chain_fake::FakeChainClientBuilder;
//! use polkagent_chain_trait::{ChainClient, ChainProfileId};
//!
//! # #[tokio::main]
//! # async fn main() {
//! let client = FakeChainClientBuilder::new("polkadot")
//!     .with_block_number(1_000_000)
//!     .with_runtime_version(1_003_000, 0)
//!     .build();
//!
//! // Health check always succeeds by default.
//! client.health().await.expect("health ok");
//!
//! // Query a missing storage key returns None.
//! let result = client
//!     .query_storage(b"no-such-key", None, ChainProfileId::new("polkadot"))
//!     .await
//!     .expect("query ok");
//! assert!(result.is_none());
//! # }
//! ```
//!
//! # Fault injection
//!
//! ```rust
//! use polkagent_chain_fake::FakeChainClientBuilder;
//! use polkagent_chain_trait::{ChainClient, ChainProfileId};
//!
//! # #[tokio::main]
//! # async fn main() {
//! // First 2 calls return errors; subsequent calls succeed.
//! let client = FakeChainClientBuilder::new("test")
//!     .fail_next_n(2)
//!     .build();
//!
//! let r1 = client.health().await;
//! let r2 = client.health().await;
//! let r3 = client.health().await;
//! assert!(r1.is_err());
//! assert!(r2.is_err());
//! assert!(r3.is_ok());
//! # }
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use polkagent_chain_trait::{
    BlockRef, ChainClient, ChainError, ChainProfileId, DecodedCall, FinalityObservation,
    MetadataDigest, PinnedMetadata, SimulationResult, TxHash,
};
use polkagent_core::now;

pub mod builder;
pub mod error;

mod state;

pub use builder::FakeChainClientBuilder;
pub use state::CallRecord;

use state::{FakeChainState, hex_encode};

// ---------------------------------------------------------------------------
// FakeChainClient
// ---------------------------------------------------------------------------

/// A deterministic fake implementation of [`ChainClient`].
///
/// Construct via [`FakeChainClientBuilder`].
///
/// `FakeChainClient` is `Clone` and `Send + Sync`. All clones share the same
/// underlying state so that test scenarios can hold multiple handles.
#[derive(Clone)]
pub struct FakeChainClient {
    state: Arc<FakeChainState>,
}

impl FakeChainClient {
    pub(crate) fn from_state(state: FakeChainState) -> Self {
        Self { state: Arc::new(state) }
    }

    // -----------------------------------------------------------------------
    // Assertion helpers
    // -----------------------------------------------------------------------

    /// Return a snapshot of all recorded call invocations.
    ///
    /// Calls are appended in the order they were received, before the result
    /// is computed. This makes it safe to assert on calls even when the method
    /// returns an error.
    #[must_use]
    pub fn calls(&self) -> Vec<CallRecord> {
        self.state.calls.lock().clone()
    }

    /// Return the total number of calls recorded so far.
    #[must_use]
    pub fn call_count(&self) -> usize {
        self.state.calls.lock().len()
    }

    /// Return the current block number.
    #[must_use]
    pub fn current_block_number(&self) -> u64 {
        self.state.current_block_number()
    }

    /// Advance the current block number by `delta`.
    ///
    /// Useful for simulating chain progression within a test.
    pub fn advance_block(&self, delta: u64) {
        self.state
            .block_number
            .fetch_add(delta, std::sync::atomic::Ordering::SeqCst);
    }

    /// Seed an additional storage value after construction.
    pub fn insert_storage(&self, key: Vec<u8>, value: Vec<u8>) {
        self.state.storage_values.lock().insert(key, value);
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    async fn maybe_sleep(&self) {
        if self.state.latency_ms > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(self.state.latency_ms)).await;
        }
    }

    fn fault_error(&self) -> ChainError {
        if self.state.disconnected {
            ChainError::Rpc {
                endpoint: "fake://localhost".into(),
                message: "simulated network disconnection".into(),
                retryable: true,
            }
        } else {
            ChainError::Rpc {
                endpoint: "fake://localhost".into(),
                message: "simulated fault (fail_next_n)".into(),
                retryable: true,
            }
        }
    }

    fn current_block_ref(&self) -> BlockRef {
        let n = self.state.current_block_number();
        BlockRef { number: n, hash: format!("0x{n:016x}") }
    }

    fn finalized_block_ref(&self) -> BlockRef {
        let n = self.state.finalized_block_number();
        BlockRef { number: n, hash: format!("0x{n:016x}") }
    }

}

// ---------------------------------------------------------------------------
// ChainClient implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ChainClient for FakeChainClient {
    /// Return pinned metadata for the requested chain profile.
    ///
    /// The metadata bytes are a minimal fake payload (32 zero bytes). The
    /// genesis hash is verified against the seeded value; a mismatch returns
    /// [`ChainError::GenesisHashMismatch`].
    async fn fetch_metadata(
        &self,
        chain_profile: ChainProfileId,
    ) -> Result<PinnedMetadata, ChainError> {
        self.maybe_sleep().await;

        let record = CallRecord::FetchMetadata { chain_profile: chain_profile.0.clone() };
        if self.state.record_and_check_fault(record) {
            return Err(self.fault_error());
        }

        let block_ref = self.current_block_ref();
        Ok(PinnedMetadata {
            chain_profile,
            spec_version: self.state.spec_version,
            metadata_digest: MetadataDigest(self.state.metadata_digest_hex()),
            metadata_bytes: vec![0u8; 32],
            block_ref,
            fetched_at: now(),
        })
    }

    /// Simulate (dry-run) a signed extrinsic.
    ///
    /// The fake always reports success with a fee estimate of 1_000_000
    /// planck and an empty storage-change preview.
    async fn simulate(
        &self,
        signed_extrinsic: &[u8],
        block_ref: &BlockRef,
        _metadata: &PinnedMetadata,
    ) -> Result<SimulationResult, ChainError> {
        self.maybe_sleep().await;

        let record = CallRecord::Simulate {
            extrinsic_len: signed_extrinsic.len(),
            block_number: block_ref.number,
        };
        if self.state.record_and_check_fault(record) {
            return Err(self.fault_error());
        }

        Ok(SimulationResult {
            success: true,
            fee_estimate: Some(1_000_000),
            error_message: None,
            storage_changes_preview: vec![],
            block_ref: block_ref.clone(),
        })
    }

    /// Submit a signed extrinsic.
    ///
    /// Returns a deterministic tx hash derived from the first bytes of the
    /// extrinsic. Does not wait for inclusion or finality.
    async fn submit_extrinsic(
        &self,
        signed_extrinsic: &[u8],
        chain_profile: ChainProfileId,
    ) -> Result<TxHash, ChainError> {
        self.maybe_sleep().await;

        let record = CallRecord::SubmitExtrinsic {
            extrinsic_len: signed_extrinsic.len(),
            chain_profile: chain_profile.0.clone(),
        };
        if self.state.record_and_check_fault(record) {
            return Err(self.fault_error());
        }

        // Build a deterministic tx hash from the first up to 32 bytes.
        let mut hash_bytes = [0u8; 32];
        let copy_len = signed_extrinsic.len().min(32);
        hash_bytes[..copy_len].copy_from_slice(&signed_extrinsic[..copy_len]);
        let tx_hash = format!("0x{}", hex_encode(&hash_bytes));
        Ok(TxHash::new(tx_hash))
    }

    /// Watch for finality of a previously submitted transaction.
    ///
    /// If the tx hash was seeded via [`FakeChainClientBuilder::with_extrinsic_result`],
    /// returns [`FinalityObservation::Finalized`] or `Failed` accordingly.
    /// Otherwise returns [`FinalityObservation::Unknown`].
    async fn watch_finality(
        &self,
        tx_hash: TxHash,
        chain_profile: ChainProfileId,
        timeout_ms: u64,
    ) -> Result<FinalityObservation, ChainError> {
        self.maybe_sleep().await;

        let record = CallRecord::WatchFinality {
            tx_hash: tx_hash.0.clone(),
            chain_profile: chain_profile.0.clone(),
            timeout_ms,
        };
        if self.state.record_and_check_fault(record) {
            return Err(self.fault_error());
        }

        // Try to parse the first 32 bytes of the hex hash for seeded lookup.
        let seeded = lookup_seeded_extrinsic_result(&self.state, &tx_hash.0);

        let finalized_ref = self.finalized_block_ref();
        match seeded {
            Some(true) => Ok(FinalityObservation::Finalized {
                block_ref: finalized_ref,
                tx_index: 1,
            }),
            Some(false) => Ok(FinalityObservation::Failed {
                block_ref: finalized_ref,
                error_message: "seeded failure result".into(),
            }),
            None => Ok(FinalityObservation::Unknown {
                last_checked_block: self.current_block_ref(),
            }),
        }
    }

    /// Decode a SCALE-encoded call using the provided pinned metadata.
    ///
    /// The fake returns a hard-coded `Balances.transfer_keep_alive` decoded
    /// call. In real usage the metadata bytes would drive decoding; here we
    /// return a predictable value so tests can assert on the structure.
    async fn decode_call(
        &self,
        call_bytes: &[u8],
        _metadata: &PinnedMetadata,
    ) -> Result<DecodedCall, ChainError> {
        self.maybe_sleep().await;

        let record = CallRecord::DecodeCall { call_len: call_bytes.len() };
        if self.state.record_and_check_fault(record) {
            return Err(self.fault_error());
        }

        Ok(DecodedCall {
            pallet: "Balances".into(),
            call_name: "transfer_keep_alive".into(),
            arguments_json: format!(
                r#"{{"call_bytes_hex":"0x{}","length":{}}}"#,
                hex_encode(&call_bytes[..call_bytes.len().min(8)]),
                call_bytes.len()
            ),
            metadata_digest: MetadataDigest(self.state.metadata_digest_hex()),
        })
    }

    /// Query a raw storage key.
    ///
    /// Returns the pre-seeded value if the key was configured via
    /// [`FakeChainClientBuilder::with_storage_value`] or
    /// [`FakeChainClientBuilder::with_balance`]; otherwise returns `None`.
    async fn query_storage(
        &self,
        storage_key: &[u8],
        _block_ref: Option<&BlockRef>,
        chain_profile: ChainProfileId,
    ) -> Result<Option<Vec<u8>>, ChainError> {
        self.maybe_sleep().await;

        let record = CallRecord::QueryStorage {
            key: storage_key.to_vec(),
            chain_profile: chain_profile.0.clone(),
        };
        if self.state.record_and_check_fault(record) {
            return Err(self.fault_error());
        }

        let guard = self.state.storage_values.lock();
        Ok(guard.get(storage_key).cloned())
    }

    /// Health check.
    ///
    /// Returns `Ok(())` unless fault injection or the disconnected flag is
    /// active.
    async fn health(&self) -> Result<(), ChainError> {
        self.maybe_sleep().await;

        if self.state.record_and_check_fault(CallRecord::Health) {
            return Err(self.fault_error());
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Internal lookup helpers
// ---------------------------------------------------------------------------

/// Attempt to decode a hex tx hash and look it up in the seeded extrinsic map.
fn lookup_seeded_extrinsic_result(
    state: &FakeChainState,
    tx_hash_hex: &str,
) -> Option<bool> {
    // Strip optional "0x" prefix.
    let stripped = tx_hash_hex.strip_prefix("0x").unwrap_or(tx_hash_hex);
    let bytes = hex_decode_32(stripped)?;
    state.extrinsic_results.lock().get(&bytes).copied()
}

/// Decode up to 32 bytes from a hex string. Returns None if the string is not
/// valid hex or is shorter than 64 nibbles.
fn hex_decode_32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() < 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        let byte_str = &hex[i * 2..i * 2 + 2];
        out[i] = u8::from_str_radix(byte_str, 16).ok()?;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_chain_trait::ChainProfileId;

    // -----------------------------------------------------------------------
    // Helper
    // -----------------------------------------------------------------------

    fn polkadot_profile() -> ChainProfileId {
        ChainProfileId::new("polkadot")
    }

    fn fake_metadata() -> PinnedMetadata {
        PinnedMetadata {
            chain_profile: polkadot_profile(),
            spec_version: 1_003_000,
            metadata_digest: MetadataDigest("0xdeadbeef".into()),
            metadata_bytes: vec![0u8; 32],
            block_ref: BlockRef { number: 1_000_000, hash: "0xabc".into() },
            fetched_at: now(),
        }
    }

    fn builder() -> FakeChainClientBuilder {
        FakeChainClientBuilder::new("polkadot")
    }

    // -----------------------------------------------------------------------
    // 1. Builder creates a valid client with defaults
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn builder_creates_valid_client_with_defaults() {
        let client = builder().build();
        assert!(client.health().await.is_ok());
    }

    // -----------------------------------------------------------------------
    // 2. Query storage returns pre-seeded values
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn query_storage_returns_seeded_value() {
        let key = vec![0xDE, 0xAD];
        let value = vec![0xBE, 0xEF];
        let client = builder().with_storage_value(key.clone(), value.clone()).build();

        let result = client
            .query_storage(&key, None, polkadot_profile())
            .await
            .expect("query ok");
        assert_eq!(result, Some(value));
    }

    // -----------------------------------------------------------------------
    // 3. Missing storage key returns None
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn missing_storage_key_returns_none() {
        let client = builder().build();
        let result = client
            .query_storage(b"no-such-key", None, polkadot_profile())
            .await
            .expect("query ok");
        assert!(result.is_none());
    }

    // -----------------------------------------------------------------------
    // 4. Runtime version matches configured values
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn runtime_version_matches_configured() {
        let client = builder().with_runtime_version(9_430, 3).build();
        let meta = client.fetch_metadata(polkadot_profile()).await.expect("metadata ok");
        assert_eq!(meta.spec_version, 9_430);
    }

    // -----------------------------------------------------------------------
    // 5. Submit extrinsic returns a deterministic tx hash
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn submit_extrinsic_returns_deterministic_hash() {
        let client = builder().build();
        let extrinsic = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let tx = client
            .submit_extrinsic(&extrinsic, polkadot_profile())
            .await
            .expect("submit ok");
        assert!(tx.0.starts_with("0x"), "tx hash should be hex");
        assert_eq!(tx.0.len(), 66, "32-byte hash = 64 hex chars + 0x");
    }

    // -----------------------------------------------------------------------
    // 6. Same extrinsic bytes produce the same hash
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn submit_extrinsic_same_bytes_same_hash() {
        let client = builder().build();
        let extrinsic = vec![1u8, 2, 3, 4];
        let tx1 = client
            .submit_extrinsic(&extrinsic, polkadot_profile())
            .await
            .expect("submit 1");
        let tx2 = client
            .submit_extrinsic(&extrinsic, polkadot_profile())
            .await
            .expect("submit 2");
        assert_eq!(tx1.0, tx2.0);
    }

    // -----------------------------------------------------------------------
    // 7. Fault injection: fail_next_n causes N errors then succeeds
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fail_next_n_causes_n_errors_then_succeeds() {
        let client = builder().fail_next_n(3).build();

        assert!(client.health().await.is_err(), "call 1 should fail");
        assert!(client.health().await.is_err(), "call 2 should fail");
        assert!(client.health().await.is_err(), "call 3 should fail");
        assert!(client.health().await.is_ok(), "call 4 should succeed");
        assert!(client.health().await.is_ok(), "call 5 should succeed");
    }

    // -----------------------------------------------------------------------
    // 8. fail_next_n works across different methods
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fail_next_n_counts_across_different_methods() {
        let client = builder().fail_next_n(2).build();

        let _ = client
            .query_storage(b"key", None, polkadot_profile())
            .await; // call 1 → fail
        let _ = client.health().await; // call 2 → fail
        // call 3 → should succeed
        assert!(client.health().await.is_ok());
    }

    // -----------------------------------------------------------------------
    // 9. Latency simulation: with_latency does not error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn latency_simulation_does_not_error() {
        let client = builder().with_latency(10).build();
        let start = std::time::Instant::now();
        client.health().await.expect("health ok");
        // We slept at least 10 ms.
        assert!(start.elapsed().as_millis() >= 10);
    }

    // -----------------------------------------------------------------------
    // 10. Call tracking records all invocations
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn call_tracking_records_all_invocations() {
        let client = builder().build();
        client.health().await.ok();
        client.health().await.ok();
        client
            .query_storage(b"k", None, polkadot_profile())
            .await
            .ok();
        assert_eq!(client.call_count(), 3);
    }

    // -----------------------------------------------------------------------
    // 11. calls() returns the correct method names
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn calls_returns_correct_records() {
        let client = builder().build();
        client.health().await.ok();
        client.fetch_metadata(polkadot_profile()).await.ok();

        let calls = client.calls();
        assert_eq!(calls.len(), 2);
        assert!(matches!(calls[0], CallRecord::Health));
        assert!(matches!(calls[1], CallRecord::FetchMetadata { .. }));
    }

    // -----------------------------------------------------------------------
    // 12. Multiple storage values
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn multiple_storage_values_all_accessible() {
        let client = builder()
            .with_storage_value(vec![1], vec![10])
            .with_storage_value(vec![2], vec![20])
            .with_storage_value(vec![3], vec![30])
            .build();

        for (key, expected) in [(vec![1u8], 10u8), (vec![2], 20), (vec![3], 30)] {
            let val = client
                .query_storage(&key, None, polkadot_profile())
                .await
                .expect("ok")
                .expect("should exist");
            assert_eq!(val[0], expected);
        }
    }

    // -----------------------------------------------------------------------
    // 13. Genesis hash matches configured value
    // -----------------------------------------------------------------------

    #[test]
    fn genesis_hash_matches_configured() {
        let hash = [0xABu8; 32];
        let client = builder().with_genesis_hash(hash).build();
        let hex = client.state.genesis_hex();
        assert!(hex.starts_with("0x"));
        // All bytes should be 0xab.
        assert!(hex[2..].chars().all(|c| c == 'a' || c == 'b'));
    }

    // -----------------------------------------------------------------------
    // 14. Network name matches configured value
    // -----------------------------------------------------------------------

    #[test]
    fn network_name_matches_configured() {
        let client = FakeChainClientBuilder::new("my-test-net").build();
        assert_eq!(client.state.network_name, "my-test-net");
    }

    // -----------------------------------------------------------------------
    // 15. Balance queries return pre-seeded balances via storage
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn balance_query_returns_seeded_balance() {
        let account = [0x01u8; 32];
        let free: u128 = 1_000_000_000_000;
        let reserved: u128 = 500_000_000_000;

        let client = builder().with_balance(account, free, reserved).build();

        let raw = client
            .query_storage(&account, None, polkadot_profile())
            .await
            .expect("ok")
            .expect("balance should exist");

        // The builder encodes free || reserved as little-endian u128.
        let decoded_free = u128::from_le_bytes(raw[0..16].try_into().expect("16 bytes"));
        let decoded_reserved = u128::from_le_bytes(raw[16..32].try_into().expect("16 bytes"));
        assert_eq!(decoded_free, free);
        assert_eq!(decoded_reserved, reserved);
    }

    // -----------------------------------------------------------------------
    // 16. Block number matches configured value
    // -----------------------------------------------------------------------

    #[test]
    fn block_number_matches_configured() {
        let client = builder().with_block_number(42_000).build();
        assert_eq!(client.current_block_number(), 42_000);
    }

    // -----------------------------------------------------------------------
    // 17. advance_block increments block number
    // -----------------------------------------------------------------------

    #[test]
    fn advance_block_increments_block_number() {
        let client = builder().with_block_number(100).build();
        client.advance_block(10);
        assert_eq!(client.current_block_number(), 110);
    }

    // -----------------------------------------------------------------------
    // 18. Default Polkadot network configuration
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn polkadot_builder_defaults() {
        let client = FakeChainClientBuilder::polkadot().build();
        assert_eq!(client.state.network_name, "polkadot");
        assert_eq!(client.state.genesis_hash, builder::POLKADOT_GENESIS);
        assert!(client.health().await.is_ok());
    }

    // -----------------------------------------------------------------------
    // 19. Default Kusama network configuration
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn kusama_builder_defaults() {
        let client = FakeChainClientBuilder::kusama().build();
        assert_eq!(client.state.network_name, "kusama");
        assert_eq!(client.state.genesis_hash, builder::KUSAMA_GENESIS);
        assert!(client.health().await.is_ok());
    }

    // -----------------------------------------------------------------------
    // 20. Thread-safety: FakeChainClient is Send + Sync
    // -----------------------------------------------------------------------

    #[test]
    fn fake_chain_client_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FakeChainClient>();
    }

    // -----------------------------------------------------------------------
    // 21. watch_finality returns Finalized for seeded success result
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn watch_finality_returns_finalized_for_seeded_success() {
        let hash = [0x01u8; 32];
        let client = builder().with_extrinsic_result(hash, true).build();
        let tx_hash = TxHash::new(format!("0x{}", hex_encode(&hash)));

        let obs = client
            .watch_finality(tx_hash, polkadot_profile(), 5000)
            .await
            .expect("watch ok");

        assert!(matches!(obs, FinalityObservation::Finalized { .. }));
    }

    // -----------------------------------------------------------------------
    // 22. watch_finality returns Failed for seeded failure result
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn watch_finality_returns_failed_for_seeded_failure() {
        let hash = [0x02u8; 32];
        let client = builder().with_extrinsic_result(hash, false).build();
        let tx_hash = TxHash::new(format!("0x{}", hex_encode(&hash)));

        let obs = client
            .watch_finality(tx_hash, polkadot_profile(), 5000)
            .await
            .expect("watch ok");

        assert!(matches!(obs, FinalityObservation::Failed { .. }));
    }

    // -----------------------------------------------------------------------
    // 23. watch_finality returns Unknown for unseeded hash
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn watch_finality_returns_unknown_for_unseeded_hash() {
        let client = builder().build();
        let tx_hash = TxHash::new(format!("0x{}", "ff".repeat(32)));

        let obs = client
            .watch_finality(tx_hash, polkadot_profile(), 5000)
            .await
            .expect("watch ok");

        assert!(matches!(obs, FinalityObservation::Unknown { .. }));
    }

    // -----------------------------------------------------------------------
    // 24. decode_call returns a valid DecodedCall
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn decode_call_returns_valid_decoded_call() {
        let client = builder().build();
        let meta = fake_metadata();
        let call_bytes = vec![0x04, 0x00, 0x01, 0x02];
        let decoded = client.decode_call(&call_bytes, &meta).await.expect("decode ok");
        assert_eq!(decoded.pallet, "Balances");
        assert_eq!(decoded.call_name, "transfer_keep_alive");
    }

    // -----------------------------------------------------------------------
    // 25. simulate returns a successful SimulationResult
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn simulate_returns_successful_result() {
        let client = builder().build();
        let meta = fake_metadata();
        let block_ref = BlockRef { number: 1_000_000, hash: "0xabc".into() };
        let result = client
            .simulate(&[1, 2, 3, 4], &block_ref, &meta)
            .await
            .expect("simulate ok");
        assert!(result.success);
        assert_eq!(result.fee_estimate, Some(1_000_000));
    }

    // -----------------------------------------------------------------------
    // 26. simulate preserves the block_ref in the result
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn simulate_preserves_block_ref() {
        let client = builder().build();
        let meta = fake_metadata();
        let block_ref = BlockRef { number: 7777, hash: "0xbeef".into() };
        let result = client
            .simulate(&[0xAA], &block_ref, &meta)
            .await
            .expect("simulate ok");
        assert_eq!(result.block_ref.number, 7777);
        assert_eq!(result.block_ref.hash, "0xbeef");
    }

    // -----------------------------------------------------------------------
    // 27. fetch_metadata embeds the configured spec_version
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fetch_metadata_embeds_spec_version() {
        let client = builder().with_runtime_version(12_345, 0).build();
        let meta = client.fetch_metadata(polkadot_profile()).await.expect("ok");
        assert_eq!(meta.spec_version, 12_345);
    }

    // -----------------------------------------------------------------------
    // 28. fetch_metadata embeds the configured metadata digest
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fetch_metadata_embeds_metadata_digest() {
        let digest_bytes = [0xCAu8; 32];
        let client = builder().with_metadata_hash(digest_bytes).build();
        let meta = client.fetch_metadata(polkadot_profile()).await.expect("ok");
        let expected_hex = format!("0x{}", "ca".repeat(32));
        assert_eq!(meta.metadata_digest.0, expected_hex);
    }

    // -----------------------------------------------------------------------
    // 29. disconnected client always returns Rpc error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn disconnected_client_always_errors() {
        let client = builder().disconnected().build();
        assert!(client.health().await.is_err());
        assert!(client.health().await.is_err());
        assert!(client
            .query_storage(b"k", None, polkadot_profile())
            .await
            .is_err());
    }

    // -----------------------------------------------------------------------
    // 30. disconnected error is retryable
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn disconnected_error_is_retryable() {
        let client = builder().disconnected().build();
        let err = client.health().await.expect_err("should fail");
        assert!(err.is_retryable());
    }

    // -----------------------------------------------------------------------
    // 31. Clone shares state between handles
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn clone_shares_state() {
        let client1 = builder().build();
        let client2 = client1.clone();

        client1.health().await.ok();
        // Both handles should see the same calls.
        assert_eq!(client2.call_count(), 1);
    }

    // -----------------------------------------------------------------------
    // 32. insert_storage after construction is visible
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn insert_storage_after_construction_is_visible() {
        let client = builder().build();
        client.insert_storage(vec![0xFF], vec![0x42]);
        let val = client
            .query_storage(&[0xFF], None, polkadot_profile())
            .await
            .expect("ok");
        assert_eq!(val, Some(vec![0x42]));
    }

    // -----------------------------------------------------------------------
    // 33. Call records include the chain profile name
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn query_storage_call_record_includes_profile_name() {
        let client = builder().build();
        client
            .query_storage(b"key", None, ChainProfileId::new("my-chain"))
            .await
            .ok();
        let calls = client.calls();
        assert!(matches!(
            &calls[0],
            CallRecord::QueryStorage { chain_profile, .. } if chain_profile == "my-chain"
        ));
    }

    // -----------------------------------------------------------------------
    // 34. submit_extrinsic records the extrinsic length
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn submit_extrinsic_records_length() {
        let client = builder().build();
        let extrinsic = vec![0u8; 128];
        client.submit_extrinsic(&extrinsic, polkadot_profile()).await.ok();
        let calls = client.calls();
        assert!(matches!(
            &calls[0],
            CallRecord::SubmitExtrinsic { extrinsic_len: 128, .. }
        ));
    }

    // -----------------------------------------------------------------------
    // 35. watch_finality records the timeout_ms
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn watch_finality_records_timeout_ms() {
        let client = builder().build();
        client
            .watch_finality(TxHash::new("0x".to_string() + &"00".repeat(32)), polkadot_profile(), 9999)
            .await
            .ok();
        let calls = client.calls();
        assert!(matches!(
            &calls[0],
            CallRecord::WatchFinality { timeout_ms: 9999, .. }
        ));
    }

    // -----------------------------------------------------------------------
    // 36. fail_next_n(0) means no failures
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fail_next_n_zero_means_no_failures() {
        let client = builder().fail_next_n(0).build();
        assert!(client.health().await.is_ok());
        assert!(client.health().await.is_ok());
    }

    // -----------------------------------------------------------------------
    // 37. Fault error is an RPC error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fault_error_is_rpc_error() {
        let client = builder().fail_next_n(1).build();
        let err = client.health().await.expect_err("should error");
        assert!(matches!(err, ChainError::Rpc { .. }));
    }

    // -----------------------------------------------------------------------
    // 38. FakeChainClient implements the ChainClient trait object
    // -----------------------------------------------------------------------

    #[test]
    fn fake_chain_client_is_usable_as_trait_object() {
        let client: Arc<dyn ChainClient> = Arc::new(builder().build());
        // Just ensure it compiles and the trait object is valid.
        let _ = client;
    }

    // -----------------------------------------------------------------------
    // 39. Multiple balances can coexist
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn multiple_balances_coexist() {
        let acc1 = [0x01u8; 32];
        let acc2 = [0x02u8; 32];
        let client = builder()
            .with_balance(acc1, 1_000, 0)
            .with_balance(acc2, 2_000, 500)
            .build();

        let raw1 = client
            .query_storage(&acc1, None, polkadot_profile())
            .await
            .expect("ok")
            .expect("exists");
        let raw2 = client
            .query_storage(&acc2, None, polkadot_profile())
            .await
            .expect("ok")
            .expect("exists");

        let free1 = u128::from_le_bytes(raw1[0..16].try_into().expect("16"));
        let free2 = u128::from_le_bytes(raw2[0..16].try_into().expect("16"));
        assert_eq!(free1, 1_000);
        assert_eq!(free2, 2_000);
    }

    // -----------------------------------------------------------------------
    // 40. call_count returns 0 on fresh client
    // -----------------------------------------------------------------------

    #[test]
    fn call_count_zero_on_fresh_client() {
        let client = builder().build();
        assert_eq!(client.call_count(), 0);
    }

    // -----------------------------------------------------------------------
    // 41. decode_call length is captured in CallRecord
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn decode_call_records_length() {
        let client = builder().build();
        let meta = fake_metadata();
        let bytes = vec![0u8; 55];
        client.decode_call(&bytes, &meta).await.ok();
        let calls = client.calls();
        assert!(matches!(&calls[0], CallRecord::DecodeCall { call_len: 55 }));
    }

    // -----------------------------------------------------------------------
    // 42. simulate records extrinsic length and block number
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn simulate_records_extrinsic_and_block_info() {
        let client = builder().build();
        let meta = fake_metadata();
        let block_ref = BlockRef { number: 12_345, hash: "0xfeed".into() };
        client.simulate(&[0u8; 77], &block_ref, &meta).await.ok();
        let calls = client.calls();
        assert!(matches!(
            &calls[0],
            CallRecord::Simulate { extrinsic_len: 77, block_number: 12_345 }
        ));
    }
}

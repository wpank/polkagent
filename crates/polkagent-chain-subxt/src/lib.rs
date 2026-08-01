//! Substrate/Polkadot chain client using JSON-RPC over HTTP.
//!
//! This crate implements the [`ChainClient`] trait from `polkagent-chain-trait`
//! by sending raw JSON-RPC 2.0 requests to Substrate/Polkadot nodes via HTTP
//! using `reqwest`. SCALE encoding/decoding is handled by `polkagent-codec`.
//!
//! # Architecture
//!
//! - [`rpc`] — JSON-RPC 2.0 transport layer (request/response, typed methods)
//! - [`decode`] — SCALE decoding helpers and hex utilities
//! - [`error`] — Error types mapping to [`ChainError`]
//! - [`config`] — [`SubxtConfig`] for timeouts, retries, connection settings
//!
//! # Usage
//!
//! ```rust,no_run
//! use polkagent_chain_subxt::{SubxtChainClient, SubxtChainClientBuilder};
//! use polkagent_chain_trait::{ChainClient, ChainProfile, ChainProfileId, GenesisHash, NetworkType};
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let profile = ChainProfile {
//!     id: ChainProfileId::new("polkadot"),
//!     name: "Polkadot".into(),
//!     genesis_hash: GenesisHash::new("0x91b171bb158e2d3848fa23a9f1c25182fb8e20313b2c1eb49219da7a70ce90c3"),
//!     spec_version: Some(1_003_000),
//!     rpc_endpoints: vec!["https://rpc.polkadot.io".into()],
//!     network_type: NetworkType::Production,
//! };
//!
//! let client = SubxtChainClientBuilder::new()
//!     .add_profile(profile)
//!     .build()?;
//!
//! client.health().await?;
//! # Ok(())
//! # }
//! ```
//!
//! [`ChainClient`]: polkagent_chain_trait::ChainClient
//! [`ChainError`]: polkagent_chain_trait::ChainError

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod config;
pub mod decode;
pub mod error;
pub mod rpc;

use std::collections::HashMap;

use async_trait::async_trait;
use tracing::{debug, info, warn};

use polkagent_chain_trait::{
    BlockRef, ChainClient, ChainError, ChainProfile, ChainProfileId, DecodedCall,
    FinalityObservation, PinnedMetadata, SimulationResult, TxHash,
};
use polkagent_core::now;

use crate::config::SubxtConfig;
use crate::decode::{
    bytes_to_hex, compute_metadata_digest, decode_call_bytes, hex_to_bytes, parse_block_number_hex,
};
use crate::error::SubxtError;
use crate::rpc::RpcClient;

// ---------------------------------------------------------------------------
// SubxtChainClient
// ---------------------------------------------------------------------------

/// A [`ChainClient`] implementation that connects to Substrate/Polkadot nodes
/// via JSON-RPC 2.0 over HTTP using `reqwest`.
///
/// Construct via [`SubxtChainClientBuilder`].
pub struct SubxtChainClient {
    rpc: RpcClient,
    profiles: HashMap<String, ChainProfile>,
}

impl std::fmt::Debug for SubxtChainClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubxtChainClient")
            .field("profiles", &self.profiles.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl SubxtChainClient {
    /// Return the chain profile for the given ID, or an error.
    fn get_profile(&self, id: &ChainProfileId) -> Result<&ChainProfile, SubxtError> {
        self.profiles.get(&id.0).ok_or_else(|| SubxtError::ProfileNotFound {
            profile_id: id.0.clone(),
        })
    }

    /// Pick the first available RPC endpoint from a profile.
    fn primary_endpoint(profile: &ChainProfile) -> Result<&str, SubxtError> {
        profile.rpc_endpoints.first().map(|s| s.as_str()).ok_or_else(|| {
            SubxtError::Config {
                message: format!(
                    "chain profile '{}' has no RPC endpoints configured",
                    profile.id
                ),
            }
        })
    }

    /// Fetch the block hash at a given number from the node.
    #[allow(dead_code)]
    async fn fetch_block_hash(
        &self,
        endpoint: &str,
        block_number: Option<u64>,
    ) -> Result<String, SubxtError> {
        self.rpc.chain_get_block_hash(endpoint, block_number).await
    }

    /// Fetch the current block number and hash from a node.
    async fn fetch_current_block(
        &self,
        endpoint: &str,
    ) -> Result<BlockRef, SubxtError> {
        let hash = self.rpc.chain_get_block_hash(endpoint, None).await?;
        let header = self.rpc.chain_get_header(endpoint, Some(&hash)).await?;
        let number_hex = header
            .get("number")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SubxtError::InvalidResponse {
                endpoint: endpoint.to_string(),
                message: "chain_getHeader response missing 'number' field".into(),
            })?;
        let number = parse_block_number_hex(number_hex)?;
        Ok(BlockRef { number, hash })
    }

    /// Fetch the finalized block ref from a node.
    async fn fetch_finalized_block(
        &self,
        endpoint: &str,
    ) -> Result<BlockRef, SubxtError> {
        let hash = self.rpc.chain_get_finalized_head(endpoint).await?;
        let header = self.rpc.chain_get_header(endpoint, Some(&hash)).await?;
        let number_hex = header
            .get("number")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SubxtError::InvalidResponse {
                endpoint: endpoint.to_string(),
                message: "finalized header missing 'number' field".into(),
            })?;
        let number = parse_block_number_hex(number_hex)?;
        Ok(BlockRef { number, hash })
    }

    /// Verify that the genesis hash from the node matches the profile.
    async fn verify_genesis_hash(
        &self,
        endpoint: &str,
        profile: &ChainProfile,
    ) -> Result<(), SubxtError> {
        let genesis = self.rpc.chain_get_block_hash(endpoint, Some(0)).await?;
        if genesis != profile.genesis_hash.0 {
            return Err(SubxtError::GenesisHashMismatch {
                expected: profile.genesis_hash.0.clone(),
                actual: genesis,
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ChainClient trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ChainClient for SubxtChainClient {
    async fn fetch_metadata(
        &self,
        chain_profile: ChainProfileId,
    ) -> Result<PinnedMetadata, ChainError> {
        let profile = self.get_profile(&chain_profile)?;
        let endpoint = Self::primary_endpoint(profile)?;

        info!(
            chain_profile = %chain_profile,
            endpoint,
            "fetching runtime metadata"
        );

        // Verify genesis hash first.
        self.verify_genesis_hash(endpoint, profile).await?;

        // Get the current block for pinning.
        let block_ref = self.fetch_current_block(endpoint).await?;

        // Fetch metadata at this block.
        let metadata_hex = self
            .rpc
            .state_get_metadata(endpoint, Some(&block_ref.hash))
            .await?;
        let metadata_bytes = hex_to_bytes(&metadata_hex)?;

        // Compute digest and extract spec version.
        let metadata_digest = compute_metadata_digest(&metadata_bytes);

        // Parse metadata to extract spec version.
        let runtime_metadata = decode::parse_runtime_metadata(&metadata_bytes)
            .map_err(|e| ChainError::MetadataFetch {
                message: format!("failed to parse metadata: {e}"),
            })?;

        let spec_version = profile.spec_version.unwrap_or(runtime_metadata.version as u32);

        debug!(
            spec_version,
            block_number = block_ref.number,
            metadata_len = metadata_bytes.len(),
            "metadata fetched and pinned"
        );

        Ok(PinnedMetadata {
            chain_profile,
            spec_version,
            metadata_digest,
            metadata_bytes,
            block_ref,
            fetched_at: now(),
        })
    }

    async fn simulate(
        &self,
        signed_extrinsic: &[u8],
        block_ref: &BlockRef,
        metadata: &PinnedMetadata,
    ) -> Result<SimulationResult, ChainError> {
        let profile = self.get_profile(&metadata.chain_profile)?;
        let endpoint = Self::primary_endpoint(profile)?;

        let extrinsic_hex = bytes_to_hex(signed_extrinsic);

        debug!(
            chain_profile = %metadata.chain_profile,
            block = block_ref.number,
            extrinsic_len = signed_extrinsic.len(),
            "simulating extrinsic"
        );

        // Use state_call with TransactionPaymentApi_query_info for fee estimation.
        let result = self
            .rpc
            .state_call(
                endpoint,
                "TransactionPaymentApi_query_info",
                &extrinsic_hex,
                Some(&block_ref.hash),
            )
            .await;

        match result {
            Ok(result_hex) => {
                // Parse the fee estimate from the response.
                let result_bytes = hex_to_bytes(&result_hex).map_err(|e| {
                    ChainError::SimulationFailed {
                        message: format!("failed to decode simulation result: {e}"),
                    }
                })?;

                // The response contains RuntimeDispatchInfo with partial_fee as u128.
                // For now, extract the fee if we have enough bytes.
                let fee_estimate = if result_bytes.len() >= 16 {
                    // The partial_fee is at the end of the struct (after weight).
                    // Weight is u64 (Substrate < 10000) or { ref_time: u64, proof_size: u64 }.
                    // We try to read the last 16 bytes as u128 fee.
                    let fee_offset = result_bytes.len().saturating_sub(16);
                    let fee_bytes: [u8; 16] = result_bytes[fee_offset..fee_offset + 16]
                        .try_into()
                        .unwrap_or([0u8; 16]);
                    Some(u128::from_le_bytes(fee_bytes))
                } else {
                    None
                };

                Ok(SimulationResult {
                    success: true,
                    fee_estimate,
                    error_message: None,
                    storage_changes_preview: vec![],
                    block_ref: block_ref.clone(),
                })
            }
            Err(SubxtError::RpcError {
                endpoint: _,
                code: _,
                message,
            }) => {
                // Simulation failed with an RPC error — the extrinsic is invalid.
                Ok(SimulationResult {
                    success: false,
                    fee_estimate: None,
                    error_message: Some(message),
                    storage_changes_preview: vec![],
                    block_ref: block_ref.clone(),
                })
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn submit_extrinsic(
        &self,
        signed_extrinsic: &[u8],
        chain_profile: ChainProfileId,
    ) -> Result<TxHash, ChainError> {
        let profile = self.get_profile(&chain_profile)?;
        let endpoint = Self::primary_endpoint(profile)?;

        let extrinsic_hex = bytes_to_hex(signed_extrinsic);

        info!(
            chain_profile = %chain_profile,
            extrinsic_len = signed_extrinsic.len(),
            "submitting extrinsic"
        );

        let tx_hash = self
            .rpc
            .author_submit_extrinsic(endpoint, &extrinsic_hex)
            .await
            .map_err(|e| match e {
                SubxtError::RpcError { message, .. } => ChainError::ExtrinsicRejected {
                    reason: message,
                },
                other => other.into(),
            })?;

        debug!(tx_hash = %tx_hash, "extrinsic submitted");

        Ok(TxHash::new(tx_hash))
    }

    async fn watch_finality(
        &self,
        tx_hash: TxHash,
        chain_profile: ChainProfileId,
        timeout_ms: u64,
    ) -> Result<FinalityObservation, ChainError> {
        let profile = self.get_profile(&chain_profile)?;
        let endpoint = Self::primary_endpoint(profile)?;

        info!(
            tx_hash = %tx_hash,
            chain_profile = %chain_profile,
            timeout_ms,
            "watching finality"
        );

        let deadline =
            tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);
        let poll_interval = tokio::time::Duration::from_secs(2);

        loop {
            if tokio::time::Instant::now() >= deadline {
                // Timeout: return Unknown per contract.
                let last_block = self.fetch_finalized_block(endpoint).await;
                let last_checked_block = last_block.unwrap_or(BlockRef {
                    number: 0,
                    hash: "0x0".into(),
                });
                warn!(
                    tx_hash = %tx_hash,
                    timeout_ms,
                    "finality observation timed out"
                );
                return Ok(FinalityObservation::Unknown { last_checked_block });
            }

            // Poll the finalized head and check for inclusion.
            // In a real implementation, we would use subscription-based watching.
            // For the JSON-RPC HTTP approach, we poll.
            match self.fetch_finalized_block(endpoint).await {
                Ok(finalized_ref) => {
                    // Check if the tx is in this block by querying block details.
                    // Since we're doing HTTP polling, we check the finalized head
                    // and look for our tx hash in the block body.
                    let block_body_result = self
                        .rpc
                        .call(
                            endpoint,
                            "chain_getBlock",
                            Some(serde_json::json!([&finalized_ref.hash])),
                        )
                        .await;

                    if let Ok(block_data) = block_body_result {
                        if let Some(extrinsics) =
                            block_data.get("block").and_then(|b| b.get("extrinsics"))
                        {
                            if let Some(exts) = extrinsics.as_array() {
                                for (idx, _ext) in exts.iter().enumerate() {
                                    // For a full implementation we would hash each
                                    // extrinsic and compare. For now we return
                                    // Unknown on timeout as the contract requires.
                                    let _ = idx;
                                }
                            }
                        }
                    }

                    debug!(
                        finalized_block = finalized_ref.number,
                        tx_hash = %tx_hash,
                        "polling finality, not yet confirmed"
                    );
                }
                Err(e) => {
                    warn!(error = %e, "failed to fetch finalized head during watch");
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    async fn decode_call(
        &self,
        call_bytes: &[u8],
        metadata: &PinnedMetadata,
    ) -> Result<DecodedCall, ChainError> {
        // decode_call must not make RPC calls per the contract.
        debug!(
            call_len = call_bytes.len(),
            chain_profile = %metadata.chain_profile,
            "decoding call bytes"
        );

        decode_call_bytes(call_bytes, &metadata.metadata_bytes, &metadata.metadata_digest)
            .map_err(ChainError::from)
    }

    async fn query_storage(
        &self,
        storage_key: &[u8],
        block_ref: Option<&BlockRef>,
        chain_profile: ChainProfileId,
    ) -> Result<Option<Vec<u8>>, ChainError> {
        let profile = self.get_profile(&chain_profile)?;
        let endpoint = Self::primary_endpoint(profile)?;

        let storage_key_hex = bytes_to_hex(storage_key);
        let block_hash = block_ref.map(|b| b.hash.as_str());

        debug!(
            chain_profile = %chain_profile,
            storage_key = %storage_key_hex,
            "querying storage"
        );

        let result = self
            .rpc
            .state_get_storage(endpoint, &storage_key_hex, block_hash)
            .await?;

        match result {
            Some(hex) => {
                let bytes = hex_to_bytes(&hex)?;
                Ok(Some(bytes))
            }
            None => Ok(None),
        }
    }

    async fn health(&self) -> Result<(), ChainError> {
        // Check health against all configured profiles.
        if self.profiles.is_empty() {
            return Ok(());
        }

        // Use the first profile's endpoint for the health check.
        let profile = self
            .profiles
            .values()
            .next()
            .ok_or_else(|| ChainError::Internal {
                message: "no chain profiles configured".into(),
            })?;

        let endpoint = Self::primary_endpoint(profile)?;
        let health = self.rpc.system_health(endpoint).await?;

        debug!(
            peers = health.peers,
            is_syncing = health.is_syncing,
            "node health check passed"
        );

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SubxtChainClientBuilder
// ---------------------------------------------------------------------------

/// Builder for constructing a [`SubxtChainClient`].
///
/// # Example
///
/// ```rust,no_run
/// use polkagent_chain_subxt::SubxtChainClientBuilder;
/// use polkagent_chain_trait::{ChainProfile, ChainProfileId, GenesisHash, NetworkType};
///
/// let profile = ChainProfile {
///     id: ChainProfileId::new("polkadot"),
///     name: "Polkadot".into(),
///     genesis_hash: GenesisHash::new("0x91b171bb158e2d3848fa23a9f1c25182fb8e20313b2c1eb49219da7a70ce90c3"),
///     spec_version: Some(1_003_000),
///     rpc_endpoints: vec!["https://rpc.polkadot.io".into()],
///     network_type: NetworkType::Production,
/// };
///
/// let client = SubxtChainClientBuilder::new()
///     .add_profile(profile)
///     .build()
///     .expect("build client");
/// ```
#[derive(Debug, Default)]
pub struct SubxtChainClientBuilder {
    profiles: HashMap<String, ChainProfile>,
    config: Option<SubxtConfig>,
}

impl SubxtChainClientBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a chain profile to the client configuration.
    pub fn add_profile(mut self, profile: ChainProfile) -> Self {
        self.profiles.insert(profile.id.0.clone(), profile);
        self
    }

    /// Set the client configuration.
    pub fn with_config(mut self, config: SubxtConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Build the [`SubxtChainClient`].
    pub fn build(self) -> Result<SubxtChainClient, ChainError> {
        let config = self.config.unwrap_or_default();
        let rpc = RpcClient::new(config).map_err(|e| ChainError::Internal {
            message: format!("failed to create RPC client: {e}"),
        })?;

        Ok(SubxtChainClient {
            rpc,
            profiles: self.profiles,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_chain_trait::{
        BlockRef, ChainProfile, ChainProfileId, GenesisHash, MetadataDigest, NetworkType,
        PinnedMetadata,
    };
    use std::sync::Arc;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn test_profile() -> ChainProfile {
        ChainProfile {
            id: ChainProfileId::new("test"),
            name: "Test Network".into(),
            genesis_hash: GenesisHash::new("0x0000000000000000000000000000000000000000000000000000000000000000"),
            spec_version: Some(1_000),
            rpc_endpoints: vec!["http://localhost:9933".into()],
            network_type: NetworkType::Development,
        }
    }

    fn polkadot_profile() -> ChainProfile {
        ChainProfile {
            id: ChainProfileId::new("polkadot"),
            name: "Polkadot".into(),
            genesis_hash: GenesisHash::new("0x91b171bb158e2d3848fa23a9f1c25182fb8e20313b2c1eb49219da7a70ce90c3"),
            spec_version: Some(1_003_000),
            rpc_endpoints: vec!["https://rpc.polkadot.io".into()],
            network_type: NetworkType::Production,
        }
    }

    fn test_metadata() -> PinnedMetadata {
        PinnedMetadata {
            chain_profile: ChainProfileId::new("test"),
            spec_version: 1_000,
            metadata_digest: MetadataDigest("0xdeadbeef".into()),
            metadata_bytes: vec![0u8; 32],
            block_ref: BlockRef {
                number: 100,
                hash: "0xabc".into(),
            },
            fetched_at: now(),
        }
    }

    // -----------------------------------------------------------------------
    // 1. Builder creates a valid client
    // -----------------------------------------------------------------------

    #[test]
    fn builder_creates_valid_client() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .build();
        assert!(client.is_ok());
    }

    // -----------------------------------------------------------------------
    // 2. Builder with no profiles succeeds
    // -----------------------------------------------------------------------

    #[test]
    fn builder_with_no_profiles_succeeds() {
        let client = SubxtChainClientBuilder::new().build();
        assert!(client.is_ok());
    }

    // -----------------------------------------------------------------------
    // 3. Builder with custom config
    // -----------------------------------------------------------------------

    #[test]
    fn builder_with_custom_config() {
        let config = SubxtConfig::builder()
            .max_retries(5)
            .request_timeout(std::time::Duration::from_secs(60))
            .build();
        let client = SubxtChainClientBuilder::new()
            .with_config(config)
            .add_profile(test_profile())
            .build();
        assert!(client.is_ok());
    }

    // -----------------------------------------------------------------------
    // 4. Builder with multiple profiles
    // -----------------------------------------------------------------------

    #[test]
    fn builder_with_multiple_profiles() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .add_profile(polkadot_profile())
            .build()
            .expect("build");
        assert_eq!(client.profiles.len(), 2);
    }

    // -----------------------------------------------------------------------
    // 5. get_profile returns the correct profile
    // -----------------------------------------------------------------------

    #[test]
    fn get_profile_returns_correct_profile() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .build()
            .expect("build");
        let profile = client.get_profile(&ChainProfileId::new("test"));
        assert!(profile.is_ok());
        assert_eq!(profile.expect("ok").name, "Test Network");
    }

    // -----------------------------------------------------------------------
    // 6. get_profile returns error for unknown profile
    // -----------------------------------------------------------------------

    #[test]
    fn get_profile_returns_error_for_unknown() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .build()
            .expect("build");
        let result = client.get_profile(&ChainProfileId::new("nonexistent"));
        assert!(result.is_err());
        assert!(matches!(
            result.err(),
            Some(SubxtError::ProfileNotFound { .. })
        ));
    }

    // -----------------------------------------------------------------------
    // 7. primary_endpoint returns the first endpoint
    // -----------------------------------------------------------------------

    #[test]
    fn primary_endpoint_returns_first() {
        let profile = test_profile();
        let ep = SubxtChainClient::primary_endpoint(&profile);
        assert!(ep.is_ok());
        assert_eq!(ep.expect("ok"), "http://localhost:9933");
    }

    // -----------------------------------------------------------------------
    // 8. primary_endpoint errors on empty endpoints
    // -----------------------------------------------------------------------

    #[test]
    fn primary_endpoint_errors_on_empty() {
        let mut profile = test_profile();
        profile.rpc_endpoints.clear();
        let result = SubxtChainClient::primary_endpoint(&profile);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // 9. SubxtChainClient is Send + Sync
    // -----------------------------------------------------------------------

    #[test]
    fn subxt_chain_client_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SubxtChainClient>();
    }

    // -----------------------------------------------------------------------
    // 10. SubxtChainClient can be used as trait object
    // -----------------------------------------------------------------------

    #[test]
    fn subxt_chain_client_is_object_safe() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .build()
            .expect("build");
        let _dyn_client: Arc<dyn ChainClient> = Arc::new(client);
    }

    // -----------------------------------------------------------------------
    // 11. Debug output includes profile names
    // -----------------------------------------------------------------------

    #[test]
    fn debug_output_includes_profiles() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .build()
            .expect("build");
        let debug = format!("{client:?}");
        assert!(debug.contains("SubxtChainClient"));
        assert!(debug.contains("test"));
    }

    // -----------------------------------------------------------------------
    // 12. decode_call with short bytes returns error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn decode_call_short_bytes_returns_error() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .build()
            .expect("build");
        let meta = test_metadata();
        let result = client.decode_call(&[0x01], &meta).await;
        assert!(result.is_err(), "short bytes should fail decode");
    }

    // -----------------------------------------------------------------------
    // 13. decode_call with empty bytes returns error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn decode_call_empty_bytes_returns_error() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .build()
            .expect("build");
        let meta = test_metadata();
        let result = client.decode_call(&[], &meta).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // 14. health with no profiles succeeds
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn health_no_profiles_succeeds() {
        let client = SubxtChainClientBuilder::new()
            .build()
            .expect("build");
        let result = client.health().await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // 15. submit_extrinsic with unknown profile errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn submit_unknown_profile_errors() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .build()
            .expect("build");
        let result = client
            .submit_extrinsic(&[1, 2, 3], ChainProfileId::new("nonexistent"))
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.err(),
            Some(ChainError::ProfileNotFound { .. })
        ));
    }

    // -----------------------------------------------------------------------
    // 16. query_storage with unknown profile errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn query_storage_unknown_profile_errors() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .build()
            .expect("build");
        let result = client
            .query_storage(b"key", None, ChainProfileId::new("nonexistent"))
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.err(),
            Some(ChainError::ProfileNotFound { .. })
        ));
    }

    // -----------------------------------------------------------------------
    // 17. fetch_metadata with unknown profile errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fetch_metadata_unknown_profile_errors() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .build()
            .expect("build");
        let result = client
            .fetch_metadata(ChainProfileId::new("nonexistent"))
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.err(),
            Some(ChainError::ProfileNotFound { .. })
        ));
    }

    // -----------------------------------------------------------------------
    // 18. watch_finality with unknown profile errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn watch_finality_unknown_profile_errors() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .build()
            .expect("build");
        let result = client
            .watch_finality(
                TxHash::new("0xabc"),
                ChainProfileId::new("nonexistent"),
                5000,
            )
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.err(),
            Some(ChainError::ProfileNotFound { .. })
        ));
    }

    // -----------------------------------------------------------------------
    // 19. simulate with unknown profile errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn simulate_unknown_profile_errors() {
        let meta = PinnedMetadata {
            chain_profile: ChainProfileId::new("nonexistent"),
            spec_version: 1_000,
            metadata_digest: MetadataDigest("0xabc".into()),
            metadata_bytes: vec![0u8; 32],
            block_ref: BlockRef {
                number: 100,
                hash: "0xabc".into(),
            },
            fetched_at: now(),
        };
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .build()
            .expect("build");
        let block_ref = BlockRef {
            number: 100,
            hash: "0xabc".into(),
        };
        let result = client.simulate(&[1, 2, 3], &block_ref, &meta).await;
        assert!(result.is_err());
        assert!(matches!(
            result.err(),
            Some(ChainError::ProfileNotFound { .. })
        ));
    }

    // -----------------------------------------------------------------------
    // 20. Builder default creates with default config
    // -----------------------------------------------------------------------

    #[test]
    fn builder_default_uses_default_config() {
        let builder = SubxtChainClientBuilder::default();
        assert!(builder.profiles.is_empty());
        assert!(builder.config.is_none());
    }

    // -----------------------------------------------------------------------
    // 21. Profile replacement on duplicate ID
    // -----------------------------------------------------------------------

    #[test]
    fn builder_replaces_profile_on_duplicate_id() {
        let mut profile2 = test_profile();
        profile2.name = "Updated Test".into();

        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .add_profile(profile2)
            .build()
            .expect("build");

        let p = client.get_profile(&ChainProfileId::new("test")).expect("ok");
        assert_eq!(p.name, "Updated Test");
    }

    // -----------------------------------------------------------------------
    // 22. bytes_to_hex produces correct format for submit
    // -----------------------------------------------------------------------

    #[test]
    fn bytes_to_hex_for_extrinsic() {
        let extrinsic = vec![0x84, 0x00, 0xFF];
        let hex = bytes_to_hex(&extrinsic);
        assert_eq!(hex, "0x8400ff");
    }

    // -----------------------------------------------------------------------
    // 23. hex_to_bytes round trip for storage key
    // -----------------------------------------------------------------------

    #[test]
    fn hex_to_bytes_round_trip_storage_key() {
        let key = vec![0x26, 0xaa, 0x39, 0x4e];
        let hex = bytes_to_hex(&key);
        let decoded = hex_to_bytes(&hex).expect("decode");
        assert_eq!(decoded, key);
    }

    // -----------------------------------------------------------------------
    // 24. compute_metadata_digest returns 0x-prefixed hash
    // -----------------------------------------------------------------------

    #[test]
    fn compute_metadata_digest_format() {
        let digest = compute_metadata_digest(b"test");
        assert!(digest.0.starts_with("0x"));
        // BLAKE3 hash is 64 hex chars.
        assert_eq!(digest.0.len(), 2 + 64);
    }

    // -----------------------------------------------------------------------
    // 25. SubxtError -> ChainError: profile not found
    // -----------------------------------------------------------------------

    #[test]
    fn subxt_error_profile_not_found_conversion() {
        let e = SubxtError::ProfileNotFound {
            profile_id: "test".into(),
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::ProfileNotFound { .. }));
    }

    // -----------------------------------------------------------------------
    // 26. SubxtError -> ChainError: genesis mismatch
    // -----------------------------------------------------------------------

    #[test]
    fn subxt_error_genesis_mismatch_conversion() {
        let e = SubxtError::GenesisHashMismatch {
            expected: "0xaaa".into(),
            actual: "0xbbb".into(),
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::GenesisHashMismatch { .. }));
        assert!(!ce.is_retryable());
    }

    // -----------------------------------------------------------------------
    // 27. SubxtError -> ChainError: transport retryable
    // -----------------------------------------------------------------------

    #[test]
    fn subxt_error_transport_retryable_conversion() {
        let e = SubxtError::Transport {
            endpoint: "http://localhost:9933".into(),
            message: "timeout".into(),
            retryable: true,
        };
        let ce: ChainError = e.into();
        assert!(ce.is_retryable());
    }

    // -----------------------------------------------------------------------
    // 28. SubxtError -> ChainError: transport non-retryable
    // -----------------------------------------------------------------------

    #[test]
    fn subxt_error_transport_non_retryable_conversion() {
        let e = SubxtError::Transport {
            endpoint: "http://localhost:9933".into(),
            message: "DNS failure".into(),
            retryable: false,
        };
        let ce: ChainError = e.into();
        assert!(!ce.is_retryable());
    }

    // -----------------------------------------------------------------------
    // 29. Config backoff delay computation
    // -----------------------------------------------------------------------

    #[test]
    fn config_backoff_delay() {
        let cfg = SubxtConfig::builder()
            .retry_base_delay(std::time::Duration::from_millis(100))
            .retry_max_delay(std::time::Duration::from_secs(1))
            .build();

        assert_eq!(cfg.backoff_delay(0).as_millis(), 100);
        assert_eq!(cfg.backoff_delay(1).as_millis(), 200);
        assert_eq!(cfg.backoff_delay(2).as_millis(), 400);
        // Should cap at max.
        assert_eq!(cfg.backoff_delay(10).as_millis(), 1000);
    }

    // -----------------------------------------------------------------------
    // 30. Config serialization round trip
    // -----------------------------------------------------------------------

    #[test]
    fn config_serde_round_trip() {
        let cfg = SubxtConfig::default();
        let json = serde_json::to_string(&cfg).expect("serialize");
        let cfg2: SubxtConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg.max_retries, cfg2.max_retries);
        assert_eq!(cfg.request_timeout, cfg2.request_timeout);
    }

    // -----------------------------------------------------------------------
    // 31. parse_block_number_hex
    // -----------------------------------------------------------------------

    #[test]
    fn parse_block_number_hex_values() {
        assert_eq!(parse_block_number_hex("0x0").expect("ok"), 0);
        assert_eq!(parse_block_number_hex("0xff").expect("ok"), 255);
        assert_eq!(parse_block_number_hex("0xf4240").expect("ok"), 1_000_000);
    }

    // -----------------------------------------------------------------------
    // 32. parse_block_number_hex invalid
    // -----------------------------------------------------------------------

    #[test]
    fn parse_block_number_hex_invalid_returns_error() {
        assert!(parse_block_number_hex("0xZZZZ").is_err());
    }

    // -----------------------------------------------------------------------
    // 33. hex_to_bytes edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn hex_to_bytes_edge_cases() {
        // Empty with prefix.
        assert_eq!(hex_to_bytes("0x").expect("ok"), Vec::<u8>::new());
        // Single byte.
        assert_eq!(hex_to_bytes("0xff").expect("ok"), vec![0xFF]);
        // Large value.
        let hex = "0x" .to_string() + &"ab".repeat(256);
        let bytes = hex_to_bytes(&hex).expect("ok");
        assert_eq!(bytes.len(), 256);
    }

    // -----------------------------------------------------------------------
    // 34. compute_metadata_digest is deterministic
    // -----------------------------------------------------------------------

    #[test]
    fn compute_metadata_digest_deterministic() {
        let d1 = compute_metadata_digest(b"same data");
        let d2 = compute_metadata_digest(b"same data");
        assert_eq!(d1.0, d2.0);
    }

    // -----------------------------------------------------------------------
    // 35. compute_metadata_digest different input yields different hash
    // -----------------------------------------------------------------------

    #[test]
    fn compute_metadata_digest_different_for_different_input() {
        let d1 = compute_metadata_digest(b"data1");
        let d2 = compute_metadata_digest(b"data2");
        assert_ne!(d1.0, d2.0);
    }

    // -----------------------------------------------------------------------
    // 36. Builder new is identical to default
    // -----------------------------------------------------------------------

    #[test]
    fn builder_new_identical_to_default() {
        let b1 = SubxtChainClientBuilder::new();
        let b2 = SubxtChainClientBuilder::default();
        assert_eq!(b1.profiles.len(), b2.profiles.len());
        assert!(b1.config.is_none());
        assert!(b2.config.is_none());
    }

    // -----------------------------------------------------------------------
    // 37. Multiple profile lookup
    // -----------------------------------------------------------------------

    #[test]
    fn multiple_profile_lookup() {
        let client = SubxtChainClientBuilder::new()
            .add_profile(test_profile())
            .add_profile(polkadot_profile())
            .build()
            .expect("build");

        assert!(client.get_profile(&ChainProfileId::new("test")).is_ok());
        assert!(client.get_profile(&ChainProfileId::new("polkadot")).is_ok());
        assert!(client
            .get_profile(&ChainProfileId::new("kusama"))
            .is_err());
    }

    // -----------------------------------------------------------------------
    // 38. StorageChange type variants
    // -----------------------------------------------------------------------

    #[test]
    fn storage_change_type_serializes() {
        let change = StorageChange {
            key_prefix: "0x26aa".into(),
            change_type: StorageChangeType::Write,
        };
        let json = serde_json::to_string(&change).expect("serialize");
        assert!(json.contains("write"));
    }

    // -----------------------------------------------------------------------
    // 39. SimulationResult with error message
    // -----------------------------------------------------------------------

    #[test]
    fn simulation_result_with_error() {
        let result = SimulationResult {
            success: false,
            fee_estimate: None,
            error_message: Some("dispatch error".into()),
            storage_changes_preview: vec![],
            block_ref: BlockRef {
                number: 100,
                hash: "0xabc".into(),
            },
        };
        assert!(!result.success);
        assert_eq!(result.error_message.as_deref(), Some("dispatch error"));
    }

    // -----------------------------------------------------------------------
    // 40. SubxtConfig user_agent default
    // -----------------------------------------------------------------------

    #[test]
    fn config_default_user_agent() {
        let cfg = SubxtConfig::default();
        assert!(cfg.user_agent.starts_with("polkagent-chain-subxt/"));
    }
}

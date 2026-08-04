//! Chain client port trait for the Polkagent platform.
//!
//! This crate defines the [`ChainClient`] trait — the narrow adapter boundary
//! between the Polkagent kernel and a concrete Polkadot/Substrate node
//! connection (e.g. via `subxt`, JSON-RPC, or a light client).
//!
//! # Contract
//!
//! - Implementations must be `Send + Sync + 'static`.
//! - [`ChainClient::fetch_metadata`] must pin to the block specified in the
//!   chain profile and verify the genesis hash.
//! - [`ChainClient::decode_call`] must use only the provided metadata bytes;
//!   it must not make additional RPC calls.
//! - [`ChainClient::simulate`] produces evidence for decision-making, not
//!   authorization.
//! - [`ChainClient::submit`] sends the signed extrinsic and returns the tx
//!   hash. It does NOT wait for finality.
//! - [`ChainClient::watch_finality`] returns [`FinalityObservation::Unknown`]
//!   on timeout rather than erroring. **Never** return `Finalized` without
//!   verified on-chain evidence.

#[cfg(feature = "test-contracts")]
pub mod conformance;

pub mod xcm;

pub use xcm::{
    estimate_xcm_fees, resolve_xcm_mechanism, FeeEstimate, XcmError, XcmHop, XcmMechanism, XcmPlan,
    XcmRoute, XcmVersionCompat,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use polkagent_core::Timestamp;

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// An identifier for a chain profile configuration.
///
/// A chain profile binds a genesis hash, runtime version, RPC endpoints,
/// and metadata snapshot for a specific Polkadot network.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChainProfileId(pub String);

impl ChainProfileId {
    /// Construct a `ChainProfileId` from a string identifier.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for ChainProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Hex-encoded genesis hash of a Polkadot network.
///
/// Used to verify that the node connected to is the expected network before
/// accepting any metadata or submitting transactions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GenesisHash(pub String);

impl GenesisHash {
    #[must_use]
    pub fn new(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }
}

impl std::fmt::Display for GenesisHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A hex-encoded BLAKE3 or SHA-256 digest of pinned runtime metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetadataDigest(pub String);

/// A transaction hash on a Polkadot network (hex-encoded).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TxHash(pub String);

impl TxHash {
    #[must_use]
    pub fn new(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }
}

impl std::fmt::Display for TxHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A reference to a specific block by number and hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRef {
    /// Block number (height).
    pub number: u64,
    /// Hex-encoded block hash.
    pub hash: String,
}

/// Named, pinned configuration for interacting with a specific chain.
///
/// The `ChainProfile` records the genesis hash, spec version, and RPC
/// endpoints that the client must use for this chain. It is pinned at
/// run creation time and must not change during a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainProfile {
    pub id: ChainProfileId,
    pub name: String,
    pub genesis_hash: GenesisHash,
    pub spec_version: Option<u32>,
    pub rpc_endpoints: Vec<String>,
    pub network_type: NetworkType,
}

/// Deployment tier of a Polkadot network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkType {
    Production,
    Testnet,
    Development,
    Local,
}

/// Pinned runtime metadata fetched from a specific block.
///
/// The adapter fetches this once and the application layer caches it for the
/// duration of the chain action saga.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedMetadata {
    /// The chain profile this metadata was fetched for.
    pub chain_profile: ChainProfileId,
    /// Runtime spec version.
    pub spec_version: u32,
    /// BLAKE3 digest of `metadata_bytes`.
    pub metadata_digest: MetadataDigest,
    /// Raw SCALE-encoded metadata bytes.
    pub metadata_bytes: Vec<u8>,
    /// The block at which metadata was fetched.
    pub block_ref: BlockRef,
    /// When the metadata was fetched.
    pub fetched_at: Timestamp,
}

/// A decoded call with human-readable rendering of arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedCall {
    /// The pallet name (e.g. "Balances").
    pub pallet: String,
    /// The call name within the pallet (e.g. "transfer_keep_alive").
    pub call_name: String,
    /// JSON-encoded decoded arguments.
    pub arguments_json: String,
    /// The metadata digest used for decoding.
    pub metadata_digest: MetadataDigest,
}

/// Result of a DryRunApi dry-run call.
///
/// Captures whether execution succeeded, the emitted events, and an optional
/// destination weight/fee estimate (populated when the extrinsic triggers an
/// XCM message).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DryRunResult {
    /// Whether the dry-run execution succeeded.
    pub execution_ok: bool,
    /// Events emitted during the dry-run (JSON-encoded).
    pub events: Vec<serde_json::Value>,
    /// Estimated weight/fee for the destination leg, if applicable.
    pub dest_weight_fee: Option<u128>,
}

/// Result of a dry-run / simulation of a transaction.
///
/// Provides fee estimates and storage change previews for the approval
/// evidence chain. A successful simulation is **not** authorization to submit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationResult {
    /// Whether the simulation succeeded.
    pub success: bool,
    /// Estimated fee in the chain's native token (in planck units).
    pub fee_estimate: Option<u128>,
    /// Human-readable error message if `success` is false.
    pub error_message: Option<String>,
    /// Preview of storage changes (may be empty for complex calls).
    pub storage_changes_preview: Vec<StorageChange>,
    /// The block the simulation was run against.
    pub block_ref: BlockRef,
}

/// A single storage slot change observed during simulation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageChange {
    /// Hex-encoded storage key prefix.
    pub key_prefix: String,
    /// Type of change.
    pub change_type: StorageChangeType,
}

/// The kind of storage change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageChangeType {
    Write,
    Delete,
    Read,
}

/// The result of watching finality for a submitted transaction.
///
/// **Invariant:** `Finalized` must never be returned without verified
/// on-chain evidence. If the observation times out, return `Unknown`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FinalityObservation {
    /// The transaction was finalized in the given block.
    Finalized {
        block_ref: BlockRef,
        /// Index of the extrinsic within the block.
        tx_index: u32,
    },
    /// The transaction failed on-chain (e.g. dispatch error).
    Failed {
        block_ref: BlockRef,
        error_message: String,
    },
    /// The observation timed out without confirming inclusion.
    ///
    /// The caller must treat this as unknown, not failure. A new attempt
    /// requires fresh evidence (see PRD-02 INV-04 / INV-12).
    Unknown { last_checked_block: BlockRef },
}

/// Status of a submitted extrinsic.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExtrinsicStatus {
    /// The extrinsic has been broadcast but not yet included.
    Broadcast,
    /// The extrinsic is in a best-effort (not yet finalized) block.
    InBestBlock { block_ref: BlockRef, tx_index: u32 },
    /// The extrinsic is in a finalized block.
    Finalized { block_ref: BlockRef, tx_index: u32 },
    /// The extrinsic was dropped or rejected by the network.
    Dropped { reason: String },
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that a [`ChainClient`] implementation may return.
#[derive(Debug, Error)]
pub enum ChainError {
    /// The chain profile is not recognized or configured.
    #[error("chain profile not found: {chain_profile}")]
    ProfileNotFound { chain_profile: ChainProfileId },

    /// An RPC call to the node failed.
    #[error("RPC error from {endpoint}: {message} (retryable: {retryable})")]
    Rpc {
        endpoint: String,
        message: String,
        retryable: bool,
    },

    /// The node's genesis hash does not match the expected profile hash.
    ///
    /// This indicates the client is connected to the wrong network.
    #[error("genesis hash mismatch: expected {expected}, got {actual}")]
    GenesisHashMismatch {
        expected: GenesisHash,
        actual: GenesisHash,
    },

    /// Fetching or verifying runtime metadata failed.
    #[error("metadata fetch failed: {message}")]
    MetadataFetch { message: String },

    /// Simulation (dry-run) of the transaction failed.
    #[error("simulation failed: {message}")]
    SimulationFailed { message: String },

    /// Decoding the SCALE-encoded call bytes failed.
    #[error("call decode failed: {message}")]
    DecodeFailed { message: String },

    /// Finality observation timed out without confirmation.
    ///
    /// Callers must treat this as [`FinalityObservation::Unknown`]; it is
    /// not a failure of the transaction itself.
    #[error("finality observation timed out after {elapsed_ms}ms")]
    FinalityTimeout { elapsed_ms: u64 },

    /// The submitted extrinsic was rejected by the node.
    #[error("extrinsic rejected: {reason}")]
    ExtrinsicRejected { reason: String },

    /// The operation is not supported by this adapter.
    #[error("unsupported operation: {operation}")]
    Unsupported { operation: String },

    /// An unexpected internal error.
    #[error("chain client internal error: {message}")]
    Internal { message: String },
}

impl ChainError {
    /// Returns `true` if the caller may safely retry the operation.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Rpc {
                retryable: true,
                ..
            }
        )
    }
}

// ---------------------------------------------------------------------------
// Trait definition
// ---------------------------------------------------------------------------

/// The chain client port.
///
/// Provides read-only chain state access, metadata pinning, call decoding,
/// simulation, submission, and finality observation for Polkadot networks.
///
/// One implementation exists per connection technology (subxt, JSON-RPC light
/// client, mock). The kernel depends only on this trait.
///
/// # Contract
///
/// - Implementations must be `Send + Sync + 'static`.
/// - [`fetch_metadata`] must verify the genesis hash against the chain profile
///   before returning. A mismatch returns [`ChainError::GenesisHashMismatch`].
/// - [`decode_call`] must use only the provided `metadata` bytes; no
///   additional RPC calls may be made.
/// - [`simulate`] runs a dry-run. The result is evidence, not authorization.
/// - [`submit`] sends the signed extrinsic and returns the tx hash. It does
///   NOT wait for inclusion or finality.
/// - [`watch_finality`] returns [`FinalityObservation::Unknown`] on timeout.
///   It must **never** return `Finalized` without verified on-chain evidence.
///
/// [`fetch_metadata`]: ChainClient::fetch_metadata
/// [`decode_call`]: ChainClient::decode_call
/// [`simulate`]: ChainClient::simulate
/// [`submit`]: ChainClient::submit
/// [`watch_finality`]: ChainClient::watch_finality
#[async_trait]
pub trait ChainClient: Send + Sync + 'static {
    /// Fetch and pin runtime metadata for the given chain profile.
    ///
    /// Verifies that the node's genesis hash matches the profile's
    /// `genesis_hash`. Returns [`ChainError::GenesisHashMismatch`] on
    /// mismatch.
    async fn fetch_metadata(
        &self,
        chain_profile: ChainProfileId,
    ) -> Result<PinnedMetadata, ChainError>;

    /// Simulate (dry-run) a signed extrinsic against the specified block.
    ///
    /// The result is evidence for the approval flow — not authorization to
    /// submit.
    async fn simulate(
        &self,
        signed_extrinsic: &[u8],
        block_ref: &BlockRef,
        metadata: &PinnedMetadata,
    ) -> Result<SimulationResult, ChainError>;

    /// Submit a signed extrinsic to the network.
    ///
    /// Returns the transaction hash immediately. Does NOT wait for inclusion
    /// or finality. Use [`watch_finality`] to observe outcome.
    ///
    /// [`watch_finality`]: ChainClient::watch_finality
    async fn submit_extrinsic(
        &self,
        signed_extrinsic: &[u8],
        chain_profile: ChainProfileId,
    ) -> Result<TxHash, ChainError>;

    /// Watch for finality of a previously submitted transaction.
    ///
    /// Returns [`FinalityObservation::Unknown`] on timeout rather than
    /// erroring. Callers must not treat `Unknown` as failure.
    async fn watch_finality(
        &self,
        tx_hash: TxHash,
        chain_profile: ChainProfileId,
        timeout_ms: u64,
    ) -> Result<FinalityObservation, ChainError>;

    /// Decode a SCALE-encoded call using the provided pinned metadata.
    ///
    /// Must not make additional RPC calls; all information required for
    /// decoding must be present in `metadata`.
    async fn decode_call(
        &self,
        call_bytes: &[u8],
        metadata: &PinnedMetadata,
    ) -> Result<DecodedCall, ChainError>;

    /// Query a raw storage key from the chain state.
    ///
    /// Returns `None` if the storage key has no value at the given block.
    async fn query_storage(
        &self,
        storage_key: &[u8],
        block_ref: Option<&BlockRef>,
        chain_profile: ChainProfileId,
    ) -> Result<Option<Vec<u8>>, ChainError>;

    /// Execute a DryRunApi dry-run against the given extrinsic bytes.
    ///
    /// Returns execution outcome, events, and an optional destination fee
    /// estimate. Adapters that do not support this runtime API should return
    /// [`ChainError::Unsupported`].
    async fn dry_run_call(&self, extrinsic: &[u8]) -> Result<DryRunResult, ChainError>;

    /// Query the XCM payment assets accepted by the runtime.
    ///
    /// Returns a list of asset identifiers (JSON-encoded) that the runtime
    /// accepts for XCM fee payment at the given XCM version.
    async fn xcm_query_acceptable_payment_assets(
        &self,
        version: u8,
    ) -> Result<Vec<String>, ChainError>;

    /// Query the delivery fee for sending an XCM message to `dest`.
    ///
    /// Returns the fee in the chain's native token (planck units).
    async fn xcm_query_delivery_fee(
        &self,
        dest: &GenesisHash,
        message: &[u8],
    ) -> Result<u128, ChainError>;

    /// Check whether `dest` is a trusted teleporter for the given asset.
    async fn is_trusted_teleporter(
        &self,
        dest: &ChainProfileId,
        asset: &str,
    ) -> Result<bool, ChainError>;

    /// Check whether reserve-backed transfers are supported to `dest` for
    /// the given asset.
    async fn is_reserve_transfer_supported(
        &self,
        dest: &ChainProfileId,
        asset: &str,
    ) -> Result<bool, ChainError>;

    /// Health check: return `Ok(())` if the chain client is connected and
    /// the node is responsive.
    async fn health(&self) -> Result<(), ChainError>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_profile_id_display() {
        let id = ChainProfileId::new("polkadot-mainnet");
        assert_eq!(format!("{id}"), "polkadot-mainnet");
    }

    #[test]
    fn tx_hash_display() {
        let hash = TxHash::new("0xdeadbeef");
        assert_eq!(format!("{hash}"), "0xdeadbeef");
    }

    #[test]
    fn chain_error_rpc_retryable() {
        let e = ChainError::Rpc {
            endpoint: "wss://rpc.example.com".into(),
            message: "timeout".into(),
            retryable: true,
        };
        assert!(e.is_retryable());
    }

    #[test]
    fn chain_error_genesis_mismatch_not_retryable() {
        let e = ChainError::GenesisHashMismatch {
            expected: GenesisHash::new("0xaaa"),
            actual: GenesisHash::new("0xbbb"),
        };
        assert!(!e.is_retryable());
    }

    #[test]
    fn finality_observation_unknown_serializes() {
        let obs = FinalityObservation::Unknown {
            last_checked_block: BlockRef {
                number: 1000,
                hash: "0xabc".into(),
            },
        };
        let json = serde_json::to_string(&obs).expect("serialize");
        assert!(json.contains("unknown"));
    }

    #[test]
    fn simulation_result_serializes() {
        let result = SimulationResult {
            success: true,
            fee_estimate: Some(1_000_000),
            error_message: None,
            storage_changes_preview: vec![StorageChange {
                key_prefix: "0x26aa".into(),
                change_type: StorageChangeType::Write,
            }],
            block_ref: BlockRef {
                number: 500,
                hash: "0xfeed".into(),
            },
        };
        let json = serde_json::to_string(&result).expect("serialize");
        assert!(json.contains("1000000"));
    }

    #[test]
    fn dry_run_result_serializes() {
        let result = DryRunResult {
            execution_ok: true,
            events: vec![serde_json::json!({"pallet": "Balances", "event": "Transfer"})],
            dest_weight_fee: Some(500_000),
        };
        let json = serde_json::to_string(&result).expect("serialize");
        assert!(json.contains("execution_ok"));
        assert!(json.contains("500000"));
    }

    #[test]
    fn dry_run_result_without_dest_fee() {
        let result = DryRunResult {
            execution_ok: false,
            events: vec![],
            dest_weight_fee: None,
        };
        let json = serde_json::to_string(&result).expect("serialize");
        assert!(json.contains("\"execution_ok\":false"));
    }

    #[test]
    fn unsupported_error_display() {
        let e = ChainError::Unsupported {
            operation: "dry_run_call".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("unsupported"));
        assert!(msg.contains("dry_run_call"));
        assert!(!e.is_retryable());
    }

    #[test]
    fn decoded_call_fields() {
        let call = DecodedCall {
            pallet: "Balances".into(),
            call_name: "transfer_keep_alive".into(),
            arguments_json: r#"{"dest":"5GrwvaEF","value":1000}"#.into(),
            metadata_digest: MetadataDigest("abc123".into()),
        };
        assert_eq!(call.pallet, "Balances");
        assert_eq!(call.call_name, "transfer_keep_alive");
    }

    /// Compile-time check: `ChainClient` can be used as a `dyn` trait object.
    #[allow(dead_code)]
    fn _chain_client_is_object_safe(_c: &dyn ChainClient) {}
}

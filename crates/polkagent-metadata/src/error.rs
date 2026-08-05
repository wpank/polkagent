//! Error types for the metadata service.

use crate::types::{ChainId, MetadataHash};
use thiserror::Error;

/// Errors that can occur during metadata operations.
#[derive(Debug, Error)]
pub enum MetadataError {
    /// No metadata snapshot found for the given chain.
    #[error("no metadata found for chain: {chain_id}")]
    NotFound {
        /// The chain that was queried.
        chain_id: ChainId,
    },

    /// The metadata cache is full and the entry could not be inserted.
    #[error("cache is full (max {max_entries} entries)")]
    CacheFull {
        /// Maximum number of entries allowed.
        max_entries: usize,
    },

    /// A metadata drift was detected: the current metadata does not match
    /// any pinned hash.
    #[error("metadata drift detected for chain {chain_id}: pinned {pinned_hash}, current {current_hash}")]
    DriftDetected {
        /// The chain where drift was detected.
        chain_id: ChainId,
        /// The pinned (expected) hash.
        pinned_hash: MetadataHash,
        /// The current (observed) hash.
        current_hash: MetadataHash,
    },

    /// No pin exists for the given chain and hash.
    #[error("no pin found for chain {chain_id} with hash {hash}")]
    PinNotFound {
        /// The chain that was queried.
        chain_id: ChainId,
        /// The hash that was looked up.
        hash: MetadataHash,
    },

    /// No cached metadata available to pin.
    #[error("no cached metadata to pin for chain: {chain_id}")]
    NothingToPin {
        /// The chain that was queried.
        chain_id: ChainId,
    },

    /// Hash computation or verification failed.
    #[error("hash error: {message}")]
    HashError {
        /// Description of what went wrong.
        message: String,
    },

    /// Serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// The extrinsic targets a different network than the chain profile.
    ///
    /// This is returned when the genesis hash embedded in the extrinsic's
    /// signed extensions does not match the expected genesis hash for the
    /// chain (AC-P2-005).
    #[error(
        "extrinsic targets network {extrinsic_genesis} but chain profile \"{chain_name}\" \
         expects genesis {expected_genesis}"
    )]
    WrongNetwork {
        /// Human-readable name of the expected chain.
        chain_name: String,
        /// Hex-encoded genesis hash found in the extrinsic.
        extrinsic_genesis: String,
        /// Hex-encoded genesis hash that was expected for this chain.
        expected_genesis: String,
    },

    /// The cached metadata is too old to be used for safe decoding.
    ///
    /// Returned by [`crate::DecodeService`] when `is_stale` returns `true` for the
    /// chain (AC-P2-004).
    #[error("metadata for chain \"{chain_name}\" is stale; refresh metadata before decoding")]
    StaleMetadata {
        /// The chain whose metadata is stale.
        chain_name: String,
    },

    /// A codec error occurred during extrinsic decoding.
    #[error("codec error: {0}")]
    Codec(String),
}

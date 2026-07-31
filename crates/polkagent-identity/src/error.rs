//! Error types for the identity crate.

use thiserror::Error;

/// Errors that can occur during identity operations.
#[derive(Debug, Error)]
pub enum IdentityError {
    /// The SS58 address string is invalid (wrong format, characters, or length).
    #[error("invalid SS58 address: {0}")]
    InvalidSS58Address(String),

    /// The checksum in an SS58 address does not match the computed value.
    #[error("invalid SS58 checksum")]
    InvalidChecksum,

    /// The network prefix is not recognized or is out of range.
    #[error("invalid network prefix: {0}")]
    InvalidPrefix(u16),

    /// The account ID bytes are the wrong length.
    #[error("invalid account ID: expected 32 bytes, got {0}")]
    InvalidAccountIdLength(usize),

    /// Failed to decode hex string.
    #[error("invalid hex encoding: {0}")]
    InvalidHex(String),

    /// Serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(String),
}

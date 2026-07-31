//! Error types for the fake chain client.
//!
//! This module re-exports [`ChainError`] from `polkagent-chain-trait` so that
//! consumers of this crate can use a single import path without depending on
//! the trait crate directly.

pub use polkagent_chain_trait::ChainError;

/// Construct a generic [`ChainError::Internal`] with a message.
#[must_use]
pub fn internal(message: impl Into<String>) -> ChainError {
    ChainError::Internal { message: message.into() }
}

/// Construct a simulated [`ChainError::Rpc`] error for fault injection.
#[must_use]
pub fn simulated_rpc_error(retryable: bool) -> ChainError {
    ChainError::Rpc {
        endpoint: "fake://localhost".into(),
        message: "simulated RPC error (fault injection)".into(),
        retryable,
    }
}

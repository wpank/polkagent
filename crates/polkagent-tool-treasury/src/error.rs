//! Error types for the treasury tool suite.
//!
//! [`TreasuryToolError`] covers the domain-specific failure modes that can
//! occur when querying chain state for balances, staking positions, vesting
//! schedules, and portfolio aggregation.

use thiserror::Error;

use polkagent_tool::ToolError;

// ---------------------------------------------------------------------------
// TreasuryToolError
// ---------------------------------------------------------------------------

/// Errors raised by treasury and portfolio analysis tools.
#[derive(Debug, Error)]
pub enum TreasuryToolError {
    /// The provided account address is not a valid SS58 string.
    #[error("invalid account address: {address}")]
    InvalidAddress {
        /// The address string that failed validation.
        address: String,
    },

    /// The specified chain is not recognized or supported.
    #[error("unsupported chain: {chain}")]
    UnsupportedChain {
        /// The chain identifier that was not recognized.
        chain: String,
    },

    /// A required storage query returned no data.
    #[error("no data found for account {account} on {chain}")]
    AccountNotFound {
        /// The SS58 account address.
        account: String,
        /// The chain identifier.
        chain: String,
    },

    /// The chain client returned an error during a storage query.
    #[error("chain query failed: {message}")]
    ChainQueryFailed {
        /// Description of the chain-level failure.
        message: String,
    },

    /// An error occurred while decoding on-chain storage values.
    #[error("storage decode error: {message}")]
    DecodeFailed {
        /// What went wrong during decoding.
        message: String,
    },

    /// The tool received invalid or missing input parameters.
    #[error("invalid input: {reason}")]
    InvalidInput {
        /// What is wrong with the input.
        reason: String,
    },
}

impl From<TreasuryToolError> for ToolError {
    fn from(err: TreasuryToolError) -> Self {
        match err {
            TreasuryToolError::InvalidAddress { .. } | TreasuryToolError::InvalidInput { .. } => {
                ToolError::InvalidInput {
                    reason: err.to_string(),
                }
            }
            TreasuryToolError::UnsupportedChain { .. }
            | TreasuryToolError::AccountNotFound { .. }
            | TreasuryToolError::ChainQueryFailed { .. }
            | TreasuryToolError::DecodeFailed { .. } => ToolError::ExecutionFailed {
                reason: err.to_string(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_address_display() {
        let err = TreasuryToolError::InvalidAddress {
            address: "not-an-address".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("invalid account address"));
        assert!(msg.contains("not-an-address"));
    }

    #[test]
    fn unsupported_chain_display() {
        let err = TreasuryToolError::UnsupportedChain {
            chain: "bitcoin".to_string(),
        };
        assert!(err.to_string().contains("unsupported chain"));
        assert!(err.to_string().contains("bitcoin"));
    }

    #[test]
    fn account_not_found_display() {
        let err = TreasuryToolError::AccountNotFound {
            account: "5GrwvaEF...".to_string(),
            chain: "polkadot".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("no data found"));
        assert!(msg.contains("polkadot"));
    }

    #[test]
    fn chain_query_failed_display() {
        let err = TreasuryToolError::ChainQueryFailed {
            message: "RPC timeout".to_string(),
        };
        assert!(err.to_string().contains("chain query failed"));
    }

    #[test]
    fn decode_failed_display() {
        let err = TreasuryToolError::DecodeFailed {
            message: "unexpected SCALE prefix".to_string(),
        };
        assert!(err.to_string().contains("storage decode error"));
    }

    #[test]
    fn invalid_input_display() {
        let err = TreasuryToolError::InvalidInput {
            reason: "missing account_id".to_string(),
        };
        assert!(err.to_string().contains("invalid input"));
    }

    #[test]
    fn converts_to_tool_error_invalid_input() {
        let err = TreasuryToolError::InvalidAddress {
            address: "bad".to_string(),
        };
        let tool_err: ToolError = err.into();
        assert!(matches!(tool_err, ToolError::InvalidInput { .. }));
    }

    #[test]
    fn converts_to_tool_error_execution_failed() {
        let err = TreasuryToolError::ChainQueryFailed {
            message: "timeout".to_string(),
        };
        let tool_err: ToolError = err.into();
        assert!(matches!(tool_err, ToolError::ExecutionFailed { .. }));
    }
}

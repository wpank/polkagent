//! Error types for governance tools.
//!
//! [`GovernanceError`] unifies domain-specific failures (unknown referendum,
//! invalid track, etc.) with chain-level errors surfaced by the
//! [`ChainClient`](polkagent_chain_trait::ChainClient).

use thiserror::Error;

use polkagent_chain_trait::ChainError;

// ---------------------------------------------------------------------------
// GovernanceError
// ---------------------------------------------------------------------------

/// Errors raised by governance tool operations.
#[derive(Debug, Error)]
pub enum GovernanceError {
    /// The requested referendum does not exist or has not been created yet.
    #[error("referendum not found: index {index}")]
    ReferendumNotFound {
        /// The referendum index that was requested.
        index: u32,
    },

    /// The requested governance track does not exist.
    #[error("track not found: id {id}")]
    TrackNotFound {
        /// The track ID that was requested.
        id: u16,
    },

    /// The provided account address is malformed.
    #[error("invalid account address: {address}")]
    InvalidAccount {
        /// The malformed address string.
        address: String,
    },

    /// No voting history exists for the given account.
    #[error("no voting history for account: {account}")]
    NoVotingHistory {
        /// The account that has no voting records.
        account: String,
    },

    /// No delegation information exists for the given account.
    #[error("no delegation info for account: {account}")]
    NoDelegationInfo {
        /// The account that has no delegation records.
        account: String,
    },

    /// The treasury query failed due to missing or inaccessible state.
    #[error("treasury state unavailable: {reason}")]
    TreasuryUnavailable {
        /// Description of why the treasury state could not be read.
        reason: String,
    },

    /// An error occurred while querying the chain.
    #[error("chain query failed: {0}")]
    Chain(#[from] ChainError),

    /// Failed to decode chain state response.
    #[error("decode error: {message}")]
    Decode {
        /// Description of the decoding failure.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referendum_not_found_display() {
        let e = GovernanceError::ReferendumNotFound { index: 42 };
        assert_eq!(e.to_string(), "referendum not found: index 42");
    }

    #[test]
    fn track_not_found_display() {
        let e = GovernanceError::TrackNotFound { id: 10 };
        assert_eq!(e.to_string(), "track not found: id 10");
    }

    #[test]
    fn invalid_account_display() {
        let e = GovernanceError::InvalidAccount {
            address: "bad-addr".to_string(),
        };
        assert!(e.to_string().contains("bad-addr"));
    }

    #[test]
    fn chain_error_converts() {
        let chain_err = ChainError::Internal {
            message: "test".to_string(),
        };
        let gov_err: GovernanceError = chain_err.into();
        assert!(matches!(gov_err, GovernanceError::Chain(_)));
        assert!(gov_err.to_string().contains("chain query failed"));
    }

    #[test]
    fn decode_error_display() {
        let e = GovernanceError::Decode {
            message: "bad scale bytes".to_string(),
        };
        assert!(e.to_string().contains("bad scale bytes"));
    }

    #[test]
    fn no_voting_history_display() {
        let e = GovernanceError::NoVotingHistory {
            account: "5GrwvaEF".to_string(),
        };
        assert!(e.to_string().contains("5GrwvaEF"));
    }

    #[test]
    fn treasury_unavailable_display() {
        let e = GovernanceError::TreasuryUnavailable {
            reason: "pallet not available".to_string(),
        };
        assert!(e.to_string().contains("pallet not available"));
    }
}

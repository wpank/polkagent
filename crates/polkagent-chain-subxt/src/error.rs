//! Error types for the subxt chain client, mapping to [`ChainError`].

use polkagent_chain_trait::{ChainError, ChainProfileId, GenesisHash};
use thiserror::Error;

/// Errors specific to the subxt/JSON-RPC chain client layer.
///
/// These are converted into [`ChainError`] variants at the trait boundary.
#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum SubxtError {
    /// A JSON-RPC transport error (network, timeout, etc.).
    #[error("RPC transport error: {message}")]
    Transport {
        endpoint: String,
        message: String,
        retryable: bool,
    },

    /// The JSON-RPC response contained an error object.
    #[error("RPC error response (code {code}): {message}")]
    RpcError {
        endpoint: String,
        code: i64,
        message: String,
    },

    /// Failed to parse a JSON-RPC response.
    #[error("invalid RPC response: {message}")]
    InvalidResponse {
        endpoint: String,
        message: String,
    },

    /// The chain profile was not found in the client configuration.
    #[error("chain profile not found: {profile_id}")]
    ProfileNotFound { profile_id: String },

    /// Genesis hash mismatch between the profile and the connected node.
    #[error("genesis hash mismatch: expected {expected}, got {actual}")]
    GenesisHashMismatch { expected: String, actual: String },

    /// SCALE decoding failed.
    #[error("SCALE decode error: {message}")]
    ScaleDecode { message: String },

    /// Metadata fetch or parse failed.
    #[error("metadata error: {message}")]
    Metadata { message: String },

    /// Hex decoding failed.
    #[error("hex decode error: {message}")]
    HexDecode { message: String },

    /// Extrinsic submission was rejected.
    #[error("extrinsic rejected: {reason}")]
    ExtrinsicRejected { reason: String },

    /// Simulation / dry-run failed.
    #[error("simulation failed: {message}")]
    SimulationFailed { message: String },

    /// Finality observation timed out.
    #[error("finality timeout after {elapsed_ms}ms")]
    FinalityTimeout { elapsed_ms: u64 },

    /// Configuration error.
    #[error("configuration error: {message}")]
    Config { message: String },
}

impl From<SubxtError> for ChainError {
    fn from(e: SubxtError) -> Self {
        match e {
            SubxtError::Transport {
                endpoint,
                message,
                retryable,
            } => ChainError::Rpc {
                endpoint,
                message,
                retryable,
            },
            SubxtError::RpcError {
                endpoint,
                code: _,
                message,
            } => ChainError::Rpc {
                endpoint,
                message,
                retryable: false,
            },
            SubxtError::InvalidResponse { endpoint, message } => ChainError::Rpc {
                endpoint,
                message,
                retryable: false,
            },
            SubxtError::ProfileNotFound { profile_id } => ChainError::ProfileNotFound {
                chain_profile: ChainProfileId::new(profile_id),
            },
            SubxtError::GenesisHashMismatch { expected, actual } => {
                ChainError::GenesisHashMismatch {
                    expected: GenesisHash::new(expected),
                    actual: GenesisHash::new(actual),
                }
            }
            SubxtError::ScaleDecode { message } => ChainError::DecodeFailed { message },
            SubxtError::Metadata { message } => ChainError::MetadataFetch { message },
            SubxtError::HexDecode { message } => ChainError::DecodeFailed { message },
            SubxtError::ExtrinsicRejected { reason } => ChainError::ExtrinsicRejected { reason },
            SubxtError::SimulationFailed { message } => ChainError::SimulationFailed { message },
            SubxtError::FinalityTimeout { elapsed_ms } => {
                ChainError::FinalityTimeout { elapsed_ms }
            }
            SubxtError::Config { message } => ChainError::Internal { message },
        }
    }
}

impl From<reqwest::Error> for SubxtError {
    fn from(e: reqwest::Error) -> Self {
        let url = e
            .url()
            .map(|u| u.to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        let retryable = e.is_timeout() || e.is_connect();
        SubxtError::Transport {
            endpoint: url,
            message: e.to_string(),
            retryable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_error_converts_to_rpc_chain_error() {
        let e = SubxtError::Transport {
            endpoint: "http://localhost:9933".into(),
            message: "connection refused".into(),
            retryable: true,
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::Rpc { retryable: true, .. }));
        assert!(ce.is_retryable());
    }

    #[test]
    fn rpc_error_converts_to_non_retryable() {
        let e = SubxtError::RpcError {
            endpoint: "http://localhost:9933".into(),
            code: -32600,
            message: "invalid request".into(),
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::Rpc { retryable: false, .. }));
        assert!(!ce.is_retryable());
    }

    #[test]
    fn profile_not_found_converts() {
        let e = SubxtError::ProfileNotFound {
            profile_id: "polkadot".into(),
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::ProfileNotFound { .. }));
    }

    #[test]
    fn genesis_mismatch_converts() {
        let e = SubxtError::GenesisHashMismatch {
            expected: "0xaaa".into(),
            actual: "0xbbb".into(),
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::GenesisHashMismatch { .. }));
    }

    #[test]
    fn scale_decode_converts_to_decode_failed() {
        let e = SubxtError::ScaleDecode {
            message: "unexpected EOF".into(),
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::DecodeFailed { .. }));
    }

    #[test]
    fn metadata_error_converts() {
        let e = SubxtError::Metadata {
            message: "parse failed".into(),
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::MetadataFetch { .. }));
    }

    #[test]
    fn hex_decode_error_converts() {
        let e = SubxtError::HexDecode {
            message: "invalid hex".into(),
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::DecodeFailed { .. }));
    }

    #[test]
    fn extrinsic_rejected_converts() {
        let e = SubxtError::ExtrinsicRejected {
            reason: "bad signature".into(),
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::ExtrinsicRejected { .. }));
    }

    #[test]
    fn simulation_failed_converts() {
        let e = SubxtError::SimulationFailed {
            message: "out of gas".into(),
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::SimulationFailed { .. }));
    }

    #[test]
    fn finality_timeout_converts() {
        let e = SubxtError::FinalityTimeout { elapsed_ms: 5000 };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::FinalityTimeout { elapsed_ms: 5000 }));
    }

    #[test]
    fn config_error_converts_to_internal() {
        let e = SubxtError::Config {
            message: "bad config".into(),
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::Internal { .. }));
    }

    #[test]
    fn invalid_response_converts_to_rpc() {
        let e = SubxtError::InvalidResponse {
            endpoint: "http://localhost:9933".into(),
            message: "malformed JSON".into(),
        };
        let ce: ChainError = e.into();
        assert!(matches!(ce, ChainError::Rpc { retryable: false, .. }));
    }
}

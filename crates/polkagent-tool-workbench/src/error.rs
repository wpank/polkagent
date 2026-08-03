//! Error types for workbench tools.
//!
//! [`WorkbenchError`] unifies domain-specific failures (invalid WASM path,
//! migration failure, etc.) with chain-level errors surfaced by the
//! [`ChainClient`](polkagent_chain_trait::ChainClient).

use thiserror::Error;

use polkagent_chain_trait::ChainError;

// ---------------------------------------------------------------------------
// WorkbenchError
// ---------------------------------------------------------------------------

/// Errors raised by workbench tool operations.
#[derive(Debug, Error)]
pub enum WorkbenchError {
    /// The specified WASM blob path does not exist or is not readable.
    #[error("wasm blob not found or unreadable: {path}")]
    WasmNotFound {
        /// The path that was requested.
        path: String,
    },

    /// The WASM blob is empty or too small to be a valid runtime.
    #[error("invalid wasm blob: {reason}")]
    InvalidWasm {
        /// Description of why the WASM is invalid.
        reason: String,
    },

    /// The specified block number is invalid or not available.
    #[error("invalid block number: {block_number}")]
    InvalidBlockNumber {
        /// The block number that was requested.
        block_number: u64,
    },

    /// An error occurred during the migration rehearsal.
    #[error("migration rehearsal failed: {reason}")]
    MigrationFailed {
        /// Description of the failure.
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
    fn wasm_not_found_display() {
        let e = WorkbenchError::WasmNotFound {
            path: "/tmp/runtime.wasm".to_string(),
        };
        assert!(e.to_string().contains("/tmp/runtime.wasm"));
    }

    #[test]
    fn invalid_wasm_display() {
        let e = WorkbenchError::InvalidWasm {
            reason: "file is empty".to_string(),
        };
        assert!(e.to_string().contains("file is empty"));
    }

    #[test]
    fn invalid_block_number_display() {
        let e = WorkbenchError::InvalidBlockNumber { block_number: 999 };
        assert!(e.to_string().contains("999"));
    }

    #[test]
    fn migration_failed_display() {
        let e = WorkbenchError::MigrationFailed {
            reason: "storage key collision".to_string(),
        };
        assert!(e.to_string().contains("storage key collision"));
    }

    #[test]
    fn chain_error_converts() {
        let chain_err = ChainError::Internal {
            message: "test".to_string(),
        };
        let wb_err: WorkbenchError = chain_err.into();
        assert!(matches!(wb_err, WorkbenchError::Chain(_)));
        assert!(wb_err.to_string().contains("chain query failed"));
    }

    #[test]
    fn decode_error_display() {
        let e = WorkbenchError::Decode {
            message: "bad bytes".to_string(),
        };
        assert!(e.to_string().contains("bad bytes"));
    }
}

//! Error types for JAM client operations.

use thiserror::Error;

/// Errors returned by JAM client operations.
#[derive(Debug, Error)]
pub enum JamError {
    /// The RPC endpoint returned a transport-level error.
    #[error("JAM RPC transport error: {message} (endpoint: {endpoint})")]
    Transport { endpoint: String, message: String },

    /// The RPC response could not be parsed.
    #[error("JAM RPC invalid response: {message}")]
    InvalidResponse { message: String },

    /// The requested block was not found.
    #[error("JAM block not found: slot {slot}")]
    BlockNotFound { slot: u64 },

    /// The client is not connected to a JAM node.
    #[error("JAM client not connected")]
    NotConnected,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_error_display() {
        let e = JamError::Transport {
            endpoint: "http://localhost:9955".into(),
            message: "connection refused".into(),
        };
        let msg = format!("{e}");
        assert!(msg.contains("connection refused"));
        assert!(msg.contains("localhost:9955"));
    }

    #[test]
    fn block_not_found_display() {
        let e = JamError::BlockNotFound { slot: 42 };
        let msg = format!("{e}");
        assert!(msg.contains("42"));
    }

    #[test]
    fn not_connected_display() {
        let e = JamError::NotConnected;
        let msg = format!("{e}");
        assert!(msg.contains("not connected"));
    }
}

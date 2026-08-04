//! PCA-specific error types that map to [`TransportError`].
//!
//! Each variant captures a domain-specific failure mode of the PCA transport
//! layer and provides a deterministic conversion into the generic
//! [`TransportError`] used by the kernel.

use polkagent_transport_trait::TransportError;

/// Errors specific to the PCA encrypted transport layer.
#[derive(Debug, thiserror::Error)]
pub enum PcaError {
    /// The X25519 key exchange failed.
    #[error("key exchange failed: {reason}")]
    KeyExchangeFailed {
        /// Human-readable description.
        reason: String,
    },

    /// AEAD encryption or decryption failed.
    #[error("cipher error: {reason}")]
    CipherError {
        /// Human-readable description.
        reason: String,
    },

    /// The remote peer could not be authenticated via its SS58 address.
    #[error("peer authentication failed for {peer_address}: {reason}")]
    PeerAuthenticationFailed {
        /// The SS58 address of the peer that failed verification.
        peer_address: String,
        /// Human-readable description.
        reason: String,
    },

    /// The encrypted session has expired or been invalidated.
    #[error("session expired: {session_id}")]
    SessionExpired {
        /// The identifier of the expired session.
        session_id: String,
    },

    /// A key rotation operation failed.
    #[error("key rotation failed: {reason}")]
    KeyRotationFailed {
        /// Human-readable description.
        reason: String,
    },

    /// The message sequence was violated (out-of-order or duplicate).
    #[error("sequence error: expected {expected}, got {actual}")]
    SequenceError {
        /// The expected sequence number.
        expected: u64,
        /// The actual sequence number received.
        actual: u64,
    },

    /// The nonce has been exhausted for the current session key.
    #[error("nonce exhausted for session {session_id}")]
    NonceExhausted {
        /// The session whose nonce space is exhausted.
        session_id: String,
    },

    /// The internal message channel is full or closed.
    #[error("channel error: {reason}")]
    ChannelError {
        /// Human-readable description.
        reason: String,
    },

    /// A configuration error was detected.
    #[error("configuration error: {reason}")]
    ConfigError {
        /// Human-readable description.
        reason: String,
    },

    /// The transport has been shut down.
    #[error("transport is shut down")]
    Shutdown,

    /// Serialization or deserialization of a message envelope failed.
    #[error("envelope serde error: {reason}")]
    EnvelopeError {
        /// Human-readable description.
        reason: String,
    },

    // -- C2: Group messaging errors ------------------------------------------

    /// A group operation failed (e.g. member already present, group not found).
    #[error("group error: {reason}")]
    GroupError {
        /// Human-readable description.
        reason: String,
    },

    /// Multi-party key agreement failed.
    #[error("group key agreement failed: {reason}")]
    GroupKeyAgreementFailed {
        /// Human-readable description.
        reason: String,
    },

    // -- C3: Sync / statement errors -----------------------------------------

    /// A cross-device sync operation failed.
    #[error("sync error: {reason}")]
    SyncError {
        /// Human-readable description.
        reason: String,
    },

    /// A statement store operation failed.
    #[error("statement store error: {reason}")]
    StatementStoreError {
        /// Human-readable description.
        reason: String,
    },
}

impl From<PcaError> for TransportError {
    fn from(err: PcaError) -> Self {
        match err {
            PcaError::PeerAuthenticationFailed { reason, .. } => {
                TransportError::Authentication { message: reason }
            }
            PcaError::Shutdown => TransportError::ConnectionLost,
            PcaError::ChannelError { reason } => TransportError::Internal { message: reason },
            PcaError::SessionExpired { session_id } => TransportError::Internal {
                message: format!("session expired: {session_id}"),
            },
            PcaError::NonceExhausted { session_id } => TransportError::Internal {
                message: format!("nonce exhausted for session: {session_id}"),
            },
            other => TransportError::Internal {
                message: other.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_exchange_error_maps_to_internal() {
        let err = PcaError::KeyExchangeFailed {
            reason: "bad public key".into(),
        };
        let te: TransportError = err.into();
        assert!(matches!(te, TransportError::Internal { .. }));
    }

    #[test]
    fn peer_auth_error_maps_to_authentication() {
        let err = PcaError::PeerAuthenticationFailed {
            peer_address: "5GrwvaEF...".into(),
            reason: "signature mismatch".into(),
        };
        let te: TransportError = err.into();
        assert!(matches!(te, TransportError::Authentication { .. }));
    }

    #[test]
    fn shutdown_maps_to_connection_lost() {
        let err = PcaError::Shutdown;
        let te: TransportError = err.into();
        assert!(matches!(te, TransportError::ConnectionLost));
    }

    #[test]
    fn channel_error_maps_to_internal() {
        let err = PcaError::ChannelError {
            reason: "queue full".into(),
        };
        let te: TransportError = err.into();
        assert!(matches!(te, TransportError::Internal { .. }));
    }

    #[test]
    fn session_expired_maps_to_internal() {
        let err = PcaError::SessionExpired {
            session_id: "sess-1".into(),
        };
        let te: TransportError = err.into();
        match te {
            TransportError::Internal { message } => {
                assert!(message.contains("session expired"));
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn sequence_error_display() {
        let err = PcaError::SequenceError {
            expected: 5,
            actual: 3,
        };
        let msg = format!("{err}");
        assert!(msg.contains("expected 5"));
        assert!(msg.contains("got 3"));
    }

    #[test]
    fn config_error_maps_to_internal() {
        let err = PcaError::ConfigError {
            reason: "missing peer endpoint".into(),
        };
        let te: TransportError = err.into();
        assert!(matches!(te, TransportError::Internal { .. }));
    }

    #[test]
    fn envelope_error_maps_to_internal() {
        let err = PcaError::EnvelopeError {
            reason: "invalid JSON".into(),
        };
        let te: TransportError = err.into();
        assert!(matches!(te, TransportError::Internal { .. }));
    }
}

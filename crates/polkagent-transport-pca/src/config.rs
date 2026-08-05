//! PCA transport configuration.
//!
//! [`PcaConfig`] captures all tunable parameters for a PCA transport instance:
//! peer endpoints, key material paths, timeouts, and queue sizes.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Default key rotation interval: 1 hour.
const DEFAULT_KEY_ROTATION_SECS: u64 = 3600;

/// Default session timeout: 5 minutes.
const DEFAULT_SESSION_TIMEOUT_SECS: u64 = 300;

/// Default maximum in-flight (unacked) messages.
const DEFAULT_MAX_IN_FLIGHT: usize = 1024;

/// Default maximum message size: 1 MiB.
const DEFAULT_MAX_MESSAGE_BYTES: u64 = 1024 * 1024;

/// A configured peer endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerEndpoint {
    /// SS58 address of the peer for identity verification.
    pub ss58_address: String,
    /// Human-readable label for the peer.
    pub label: Option<String>,
}

/// Configuration for the PCA transport layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcaConfig {
    /// The local node's SS58 address used as the sender identity.
    pub local_ss58_address: String,

    /// Configured peer endpoints that this transport can communicate with.
    pub peers: Vec<PeerEndpoint>,

    /// How often to rotate session keys.
    #[serde(with = "humantime_duration", default = "default_key_rotation_interval")]
    pub key_rotation_interval: Duration,

    /// How long a session remains valid without activity before expiring.
    #[serde(with = "humantime_duration", default = "default_session_timeout")]
    pub session_timeout: Duration,

    /// Maximum number of unacknowledged messages in the delivery queue.
    #[serde(default = "default_max_in_flight")]
    pub max_in_flight: usize,

    /// Maximum message size in bytes.
    #[serde(default = "default_max_message_bytes")]
    pub max_message_bytes: u64,
}

fn default_key_rotation_interval() -> Duration {
    Duration::from_secs(DEFAULT_KEY_ROTATION_SECS)
}

fn default_session_timeout() -> Duration {
    Duration::from_secs(DEFAULT_SESSION_TIMEOUT_SECS)
}

fn default_max_in_flight() -> usize {
    DEFAULT_MAX_IN_FLIGHT
}

fn default_max_message_bytes() -> u64 {
    DEFAULT_MAX_MESSAGE_BYTES
}

impl Default for PcaConfig {
    fn default() -> Self {
        Self {
            local_ss58_address: String::new(),
            peers: Vec::new(),
            key_rotation_interval: default_key_rotation_interval(),
            session_timeout: default_session_timeout(),
            max_in_flight: default_max_in_flight(),
            max_message_bytes: default_max_message_bytes(),
        }
    }
}

impl PcaConfig {
    /// Create a new config with the given local SS58 address.
    pub fn new(local_ss58_address: impl Into<String>) -> Self {
        Self {
            local_ss58_address: local_ss58_address.into(),
            ..Default::default()
        }
    }

    /// Add a peer endpoint to the configuration.
    #[must_use]
    pub fn with_peer(mut self, ss58_address: impl Into<String>, label: Option<String>) -> Self {
        self.peers.push(PeerEndpoint {
            ss58_address: ss58_address.into(),
            label,
        });
        self
    }

    /// Set the key rotation interval.
    #[must_use]
    pub fn with_key_rotation_interval(mut self, interval: Duration) -> Self {
        self.key_rotation_interval = interval;
        self
    }

    /// Set the session timeout.
    #[must_use]
    pub fn with_session_timeout(mut self, timeout: Duration) -> Self {
        self.session_timeout = timeout;
        self
    }

    /// Set the maximum in-flight messages.
    #[must_use]
    pub fn with_max_in_flight(mut self, max: usize) -> Self {
        self.max_in_flight = max;
        self
    }

    /// Set the maximum message size.
    #[must_use]
    pub fn with_max_message_bytes(mut self, max: u64) -> Self {
        self.max_message_bytes = max;
        self
    }

    /// Validate the configuration, returning an error if invalid.
    pub fn validate(&self) -> Result<(), crate::error::PcaError> {
        if self.local_ss58_address.is_empty() {
            return Err(crate::error::PcaError::ConfigError {
                reason: "local_ss58_address must not be empty".into(),
            });
        }
        if self.max_in_flight == 0 {
            return Err(crate::error::PcaError::ConfigError {
                reason: "max_in_flight must be greater than 0".into(),
            });
        }
        if self.key_rotation_interval.is_zero() {
            return Err(crate::error::PcaError::ConfigError {
                reason: "key_rotation_interval must be greater than 0".into(),
            });
        }
        Ok(())
    }
}

/// Serde helper for `Duration` using human-readable strings (e.g. "1h", "30s").
///
/// Falls back to seconds as an integer when the string parse fails.
mod humantime_duration {
    use std::time::Duration;

    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "configuration tests intentionally fail fast when expected validation results are absent"
)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sane_defaults() {
        let config = PcaConfig::default();
        assert_eq!(config.key_rotation_interval, Duration::from_secs(3600));
        assert_eq!(config.session_timeout, Duration::from_secs(300));
        assert_eq!(config.max_in_flight, 1024);
        assert_eq!(config.max_message_bytes, 1024 * 1024);
    }

    #[test]
    fn builder_pattern_works() {
        let config = PcaConfig::new("5GrwvaEF...")
            .with_peer("5FHneW46...", Some("Alice".into()))
            .with_key_rotation_interval(Duration::from_secs(1800))
            .with_session_timeout(Duration::from_secs(60))
            .with_max_in_flight(512)
            .with_max_message_bytes(2048);

        assert_eq!(config.local_ss58_address, "5GrwvaEF...");
        assert_eq!(config.peers.len(), 1);
        assert_eq!(config.peers[0].ss58_address, "5FHneW46...");
        assert_eq!(config.peers[0].label.as_deref(), Some("Alice"));
        assert_eq!(config.key_rotation_interval, Duration::from_secs(1800));
        assert_eq!(config.session_timeout, Duration::from_secs(60));
        assert_eq!(config.max_in_flight, 512);
        assert_eq!(config.max_message_bytes, 2048);
    }

    #[test]
    fn validate_rejects_empty_local_address() {
        let config = PcaConfig::default();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("local_ss58_address"));
    }

    #[test]
    fn validate_rejects_zero_max_in_flight() {
        let config = PcaConfig::new("5GrwvaEF...").with_max_in_flight(0);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("max_in_flight"));
    }

    #[test]
    fn validate_rejects_zero_key_rotation() {
        let config =
            PcaConfig::new("5GrwvaEF...").with_key_rotation_interval(Duration::from_secs(0));
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("key_rotation_interval"));
    }

    #[test]
    fn validate_accepts_valid_config() {
        let config = PcaConfig::new("5GrwvaEF...");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn config_serializes_to_json() {
        let config = PcaConfig::new("5GrwvaEF...").with_peer("5FHneW46...", Some("Bob".into()));
        let json = serde_json::to_string(&config).expect("serialize");
        assert!(json.contains("5GrwvaEF..."));
        assert!(json.contains("5FHneW46..."));
        assert!(json.contains("Bob"));
    }

    #[test]
    fn config_roundtrips_through_json() {
        let config = PcaConfig::new("5GrwvaEF...")
            .with_peer("5FHneW46...", None)
            .with_max_in_flight(256);
        let json = serde_json::to_string(&config).expect("serialize");
        let back: PcaConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.local_ss58_address, "5GrwvaEF...");
        assert_eq!(back.peers.len(), 1);
        assert_eq!(back.max_in_flight, 256);
    }
}

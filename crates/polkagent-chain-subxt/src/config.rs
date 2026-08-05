//! Configuration types for the subxt chain client.
//!
//! [`SubxtConfig`] holds endpoint URLs, timeout settings, and retry policies
//! used by [`SubxtChainClient`](crate::SubxtChainClient).

use std::time::Duration;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SubxtConfig
// ---------------------------------------------------------------------------

/// Configuration for the subxt chain client.
///
/// Controls how the client connects to Substrate/Polkadot nodes, including
/// timeouts, retry behaviour, and connection pooling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubxtConfig {
    /// Timeout for individual JSON-RPC requests.
    #[serde(with = "duration_millis")]
    pub request_timeout: Duration,

    /// Timeout for establishing a connection.
    #[serde(with = "duration_millis")]
    pub connect_timeout: Duration,

    /// Maximum number of retry attempts for retryable errors.
    pub max_retries: u32,

    /// Base delay between retries (exponential backoff is applied).
    #[serde(with = "duration_millis")]
    pub retry_base_delay: Duration,

    /// Maximum delay between retries (caps exponential backoff).
    #[serde(with = "duration_millis")]
    pub retry_max_delay: Duration,

    /// Maximum number of concurrent connections per endpoint.
    pub max_connections_per_endpoint: usize,

    /// User-Agent header sent with HTTP requests.
    pub user_agent: String,
}

impl Default for SubxtConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            max_retries: 3,
            retry_base_delay: Duration::from_millis(500),
            retry_max_delay: Duration::from_secs(10),
            max_connections_per_endpoint: 4,
            user_agent: format!("polkagent-chain-subxt/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

impl SubxtConfig {
    /// Create a new configuration builder.
    pub fn builder() -> SubxtConfigBuilder {
        SubxtConfigBuilder::default()
    }

    /// Calculate the backoff delay for the given attempt number (0-indexed).
    ///
    /// Uses exponential backoff: `base_delay * 2^attempt`, capped at
    /// `retry_max_delay`.
    pub fn backoff_delay(&self, attempt: u32) -> Duration {
        let multiplier = 2u32.saturating_pow(attempt);
        let delay = self.retry_base_delay.saturating_mul(multiplier);
        std::cmp::min(delay, self.retry_max_delay)
    }
}

// ---------------------------------------------------------------------------
// SubxtConfigBuilder
// ---------------------------------------------------------------------------

/// Builder for [`SubxtConfig`].
#[derive(Debug, Default)]
#[must_use]
pub struct SubxtConfigBuilder {
    config: SubxtConfig,
}

impl SubxtConfigBuilder {
    /// Set the request timeout.
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.config.request_timeout = timeout;
        self
    }

    /// Set the connection timeout.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.config.connect_timeout = timeout;
        self
    }

    /// Set the maximum number of retries.
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.config.max_retries = retries;
        self
    }

    /// Set the base delay for retries.
    pub fn retry_base_delay(mut self, delay: Duration) -> Self {
        self.config.retry_base_delay = delay;
        self
    }

    /// Set the maximum retry delay.
    pub fn retry_max_delay(mut self, delay: Duration) -> Self {
        self.config.retry_max_delay = delay;
        self
    }

    /// Set the user agent string.
    pub fn user_agent(mut self, agent: impl Into<String>) -> Self {
        self.config.user_agent = agent.into();
        self
    }

    /// Build the configuration.
    pub fn build(self) -> SubxtConfig {
        self.config
    }
}

// ---------------------------------------------------------------------------
// Serde helper for Duration as milliseconds
// ---------------------------------------------------------------------------

mod duration_millis {
    use std::time::Duration;

    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        serializer.serialize_u64(millis)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(millis))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sane_values() {
        let cfg = SubxtConfig::default();
        assert_eq!(cfg.request_timeout, Duration::from_secs(30));
        assert_eq!(cfg.connect_timeout, Duration::from_secs(10));
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.retry_base_delay, Duration::from_millis(500));
        assert_eq!(cfg.retry_max_delay, Duration::from_secs(10));
        assert_eq!(cfg.max_connections_per_endpoint, 4);
        assert!(cfg.user_agent.starts_with("polkagent-chain-subxt/"));
    }

    #[test]
    fn builder_overrides_defaults() {
        let cfg = SubxtConfig::builder()
            .request_timeout(Duration::from_secs(60))
            .max_retries(5)
            .user_agent("test-agent/1.0")
            .build();

        assert_eq!(cfg.request_timeout, Duration::from_secs(60));
        assert_eq!(cfg.max_retries, 5);
        assert_eq!(cfg.user_agent, "test-agent/1.0");
        // Other fields remain default.
        assert_eq!(cfg.connect_timeout, Duration::from_secs(10));
    }

    #[test]
    fn backoff_delay_exponential() {
        let cfg = SubxtConfig {
            retry_base_delay: Duration::from_millis(100),
            retry_max_delay: Duration::from_secs(10),
            ..SubxtConfig::default()
        };

        assert_eq!(cfg.backoff_delay(0), Duration::from_millis(100));
        assert_eq!(cfg.backoff_delay(1), Duration::from_millis(200));
        assert_eq!(cfg.backoff_delay(2), Duration::from_millis(400));
        assert_eq!(cfg.backoff_delay(3), Duration::from_millis(800));
    }

    #[test]
    fn backoff_delay_capped_at_max() {
        let cfg = SubxtConfig {
            retry_base_delay: Duration::from_secs(1),
            retry_max_delay: Duration::from_secs(5),
            ..SubxtConfig::default()
        };

        // 1 * 2^3 = 8s, but capped at 5s.
        assert_eq!(cfg.backoff_delay(3), Duration::from_secs(5));
    }

    #[test]
    fn config_serializes_to_json() {
        let cfg = SubxtConfig::default();
        let json = serde_json::to_string(&cfg).expect("serialize");
        assert!(json.contains("request_timeout"));
        assert!(json.contains("max_retries"));
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = SubxtConfig::default();
        let json = serde_json::to_string(&cfg).expect("serialize");
        let cfg2: SubxtConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg.request_timeout, cfg2.request_timeout);
        assert_eq!(cfg.max_retries, cfg2.max_retries);
        assert_eq!(cfg.user_agent, cfg2.user_agent);
    }

    #[test]
    fn builder_connect_timeout() {
        let cfg = SubxtConfig::builder()
            .connect_timeout(Duration::from_secs(5))
            .build();
        assert_eq!(cfg.connect_timeout, Duration::from_secs(5));
    }

    #[test]
    fn builder_retry_delays() {
        let cfg = SubxtConfig::builder()
            .retry_base_delay(Duration::from_millis(250))
            .retry_max_delay(Duration::from_secs(3))
            .build();
        assert_eq!(cfg.retry_base_delay, Duration::from_millis(250));
        assert_eq!(cfg.retry_max_delay, Duration::from_secs(3));
    }
}

//! Webhook configuration types.
//!
//! [`WebhookConfig`] defines the full configuration for a single webhook
//! subscription: target URL, HMAC signing secret, event type filter, retry
//! policy, and request timeout.

use std::time::Duration;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// RetryPolicy
// ---------------------------------------------------------------------------

/// Retry policy for failed webhook deliveries.
///
/// Uses exponential backoff with full jitter, capped at [`RetryPolicy::max_delay`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of delivery attempts (including the initial attempt).
    ///
    /// Setting this to `1` means no retries.
    pub max_attempts: u32,

    /// Base delay for exponential backoff.
    pub base_delay: Duration,

    /// Upper bound on the computed backoff delay.
    pub max_delay: Duration,
}

impl RetryPolicy {
    /// Compute the delay before the `attempt`-th retry (0-indexed, where 0 is
    /// the first *retry*, not the initial attempt).
    ///
    /// Uses full-jitter: `rand(0, min(cap, base * 2^attempt))`.
    #[must_use]
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let shift = u64::from(attempt.min(63));
        let cap_nanos = self.max_delay.as_nanos();
        let base_nanos = self.base_delay.as_nanos();
        let multiplier: u128 = 1u128 << shift;
        let upper = cap_nanos.min(base_nanos.saturating_mul(multiplier));

        if upper == 0 {
            return Duration::ZERO;
        }

        // Lightweight jitter using system time.
        let seed = system_nanos() ^ (u128::from(attempt).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let jittered = seed % upper;
        // Safe truncation: `jittered < upper <= max_delay` which fits in u64
        // nanos (max ~584 years).
        #[allow(clippy::cast_possible_truncation)]
        Duration::from_nanos(jittered as u64)
    }

    /// Returns `true` when `attempt_count` (number of attempts already made)
    /// has reached or exceeded the maximum.
    #[must_use]
    pub fn is_exhausted(&self, attempt_count: u32) -> bool {
        attempt_count >= self.max_attempts
    }
}

impl Default for RetryPolicy {
    /// Sensible defaults: 3 attempts, 1 s base, 30 s cap.
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

// ---------------------------------------------------------------------------
// WebhookConfig
// ---------------------------------------------------------------------------

/// Full configuration for a webhook subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// The target URL to POST webhook payloads to.
    pub url: String,

    /// HMAC-SHA256 signing secret.
    ///
    /// Used to compute the `X-Polkagent-Signature` header so that receivers
    /// can verify payload authenticity.
    pub secret: String,

    /// Which event types this webhook should receive.
    ///
    /// An empty list means *all* events. Each entry uses the dot-notation
    /// format (e.g. `"run.started"`, `"effect.executed"`).
    pub events: Vec<String>,

    /// Retry policy for failed deliveries.
    #[serde(default)]
    pub retry_policy: RetryPolicy,

    /// HTTP request timeout for each delivery attempt.
    #[serde(default = "default_timeout")]
    #[serde(with = "duration_secs")]
    pub timeout: Duration,
}

impl WebhookConfig {
    /// Create a new webhook config with the given URL and secret.
    ///
    /// By default all events are subscribed and the default retry policy and
    /// timeout are used.
    #[must_use]
    pub fn new(url: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            secret: secret.into(),
            events: Vec::new(),
            retry_policy: RetryPolicy::default(),
            timeout: default_timeout(),
        }
    }

    /// Returns `true` if this config subscribes to the given event type.
    ///
    /// An empty `events` list matches all event types.
    #[must_use]
    pub fn accepts_event(&self, event_type: &str) -> bool {
        self.events.is_empty() || self.events.iter().any(|e| e == event_type)
    }
}

fn default_timeout() -> Duration {
    Duration::from_secs(10)
}

/// Serde helper to serialize/deserialize [`Duration`] as integer seconds.
mod duration_secs {
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

/// Returns system time nanoseconds since the UNIX epoch.
fn system_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "configuration tests intentionally panic when static serialization fixtures fail"
)]
mod tests {
    use super::*;

    #[test]
    fn default_retry_policy_has_sensible_values() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_attempts, 3);
        assert!(p.max_delay >= p.base_delay);
    }

    #[test]
    fn retry_delay_is_within_cap() {
        let p = RetryPolicy::default();
        for attempt in 0..20u32 {
            let d = p.delay_for_attempt(attempt);
            assert!(
                d <= p.max_delay,
                "attempt={attempt}: delay {d:?} exceeded cap {:?}",
                p.max_delay
            );
        }
    }

    #[test]
    fn retry_is_exhausted_at_max_attempts() {
        let p = RetryPolicy {
            max_attempts: 3,
            ..RetryPolicy::default()
        };
        assert!(!p.is_exhausted(0));
        assert!(!p.is_exhausted(2));
        assert!(p.is_exhausted(3));
        assert!(p.is_exhausted(10));
    }

    #[test]
    fn webhook_config_accepts_all_events_when_empty() {
        let cfg = WebhookConfig::new("https://example.com/hook", "secret");
        assert!(cfg.accepts_event("run.started"));
        assert!(cfg.accepts_event("anything"));
    }

    #[test]
    fn webhook_config_filters_events_when_specified() {
        let mut cfg = WebhookConfig::new("https://example.com/hook", "secret");
        cfg.events = vec!["run.started".into(), "run.completed".into()];
        assert!(cfg.accepts_event("run.started"));
        assert!(cfg.accepts_event("run.completed"));
        assert!(!cfg.accepts_event("effect.executed"));
    }

    #[test]
    fn webhook_config_serde_round_trip() {
        let cfg = WebhookConfig::new("https://example.com/hook", "s3cret");
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: WebhookConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.url, cfg.url);
        assert_eq!(back.secret, cfg.secret);
        assert_eq!(back.timeout, cfg.timeout);
    }
}

//! Rate-limit configuration with tier-based limits and per-endpoint overrides.
//!
//! [`RateLimitConfig`] describes rate limits for different service tiers
//! (free, basic, premium) and allows per-endpoint overrides.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::RateLimitError;
use crate::quota::Quota;

// ---------------------------------------------------------------------------
// Tier
// ---------------------------------------------------------------------------

/// Service tier for rate limiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Free tier: most restrictive limits.
    Free,
    /// Basic paid tier: moderate limits.
    Basic,
    /// Premium tier: generous limits.
    Premium,
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Free => write!(f, "free"),
            Self::Basic => write!(f, "basic"),
            Self::Premium => write!(f, "premium"),
        }
    }
}

// ---------------------------------------------------------------------------
// Strategy
// ---------------------------------------------------------------------------

/// Which rate-limiting algorithm to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Token bucket: allows bursts up to capacity.
    TokenBucket,
    /// Sliding window: smooth counting over a rolling window.
    SlidingWindow,
    /// Leaky bucket: strict average-rate enforcement.
    LeakyBucket,
}

// ---------------------------------------------------------------------------
// TierLimits
// ---------------------------------------------------------------------------

/// Rate limits for a single tier.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TierLimits {
    /// Maximum requests per second.
    pub requests_per_second: u32,
    /// Maximum requests per minute.
    pub requests_per_minute: u32,
    /// Maximum requests per hour.
    pub requests_per_hour: u32,
    /// Burst capacity (for token-bucket strategy).
    #[serde(default)]
    pub burst: u32,
}

impl TierLimits {
    /// Convert the per-second limit to a [`Quota`].
    pub fn per_second_quota(&self) -> Quota {
        Quota::new(self.requests_per_second, Duration::from_secs(1)).with_burst(self.burst)
    }

    /// Convert the per-minute limit to a [`Quota`].
    pub fn per_minute_quota(&self) -> Quota {
        Quota::new(self.requests_per_minute, Duration::from_secs(60))
    }

    /// Convert the per-hour limit to a [`Quota`].
    pub fn per_hour_quota(&self) -> Quota {
        Quota::new(self.requests_per_hour, Duration::from_secs(3600))
    }
}

// ---------------------------------------------------------------------------
// EndpointOverride
// ---------------------------------------------------------------------------

/// Per-endpoint rate limit override.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EndpointOverride {
    /// The endpoint pattern (e.g., "/api/v1/execute", "/api/v1/chat").
    pub endpoint: String,
    /// Override limits. If `None`, uses the tier default.
    pub limits: Option<TierLimits>,
    /// Whether this endpoint is exempt from rate limiting.
    #[serde(default)]
    pub exempt: bool,
}

// ---------------------------------------------------------------------------
// RateLimitConfig
// ---------------------------------------------------------------------------

/// Top-level rate-limiting configuration.
///
/// # Example (TOML)
///
/// ```toml
/// strategy = "token_bucket"
///
/// [tiers.free]
/// requests_per_second = 5
/// requests_per_minute = 100
/// requests_per_hour = 1000
/// burst = 10
///
/// [tiers.premium]
/// requests_per_second = 50
/// requests_per_minute = 2000
/// requests_per_hour = 50000
/// burst = 100
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Which algorithm to use.
    pub strategy: Strategy,
    /// Per-tier limits.
    pub tiers: HashMap<Tier, TierLimits>,
    /// Per-endpoint overrides.
    #[serde(default)]
    pub endpoint_overrides: Vec<EndpointOverride>,
    /// Whether rate limiting is enabled globally.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl RateLimitConfig {
    /// Look up the limits for a given tier.
    pub fn limits_for_tier(&self, tier: Tier) -> Option<&TierLimits> {
        self.tiers.get(&tier)
    }

    /// Look up an endpoint override.
    pub fn override_for_endpoint(&self, endpoint: &str) -> Option<&EndpointOverride> {
        self.endpoint_overrides
            .iter()
            .find(|o| o.endpoint == endpoint)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), RateLimitError> {
        if self.tiers.is_empty() {
            return Err(RateLimitError::InvalidConfig {
                reason: "at least one tier must be configured".into(),
            });
        }
        for (tier, limits) in &self.tiers {
            if limits.requests_per_second == 0
                && limits.requests_per_minute == 0
                && limits.requests_per_hour == 0
            {
                return Err(RateLimitError::InvalidConfig {
                    reason: format!("tier '{tier}' has all zero limits"),
                });
            }
        }
        Ok(())
    }

    /// Create a default configuration suitable for development.
    pub fn development() -> Self {
        let mut tiers = HashMap::new();
        tiers.insert(
            Tier::Free,
            TierLimits {
                requests_per_second: 10,
                requests_per_minute: 200,
                requests_per_hour: 5000,
                burst: 20,
            },
        );
        tiers.insert(
            Tier::Basic,
            TierLimits {
                requests_per_second: 50,
                requests_per_minute: 1000,
                requests_per_hour: 20000,
                burst: 100,
            },
        );
        tiers.insert(
            Tier::Premium,
            TierLimits {
                requests_per_second: 200,
                requests_per_minute: 5000,
                requests_per_hour: 100_000,
                burst: 500,
            },
        );

        Self {
            strategy: Strategy::TokenBucket,
            tiers,
            endpoint_overrides: Vec::new(),
            enabled: true,
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self::development()
    }
}

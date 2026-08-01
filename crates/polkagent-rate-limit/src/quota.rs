//! Quota definitions and result types.
//!
//! A [`Quota`] describes the allowed request budget: maximum requests within a
//! time window, plus an optional burst allowance.  A [`QuotaResult`] is
//! returned after each rate-limit check and tells the caller whether the
//! request was allowed, how many requests remain, and when the window resets.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Quota
// ---------------------------------------------------------------------------

/// Describes a rate-limit budget.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Quota {
    /// Maximum requests allowed within `window`.
    pub max_requests: u32,
    /// The time window over which `max_requests` are counted.
    #[serde(with = "duration_serde")]
    pub window: Duration,
    /// Extra burst capacity above `max_requests` for short spikes.
    /// Defaults to 0 (no burst beyond `max_requests`).
    #[serde(default)]
    pub burst: u32,
}

impl Quota {
    /// Create a new quota.
    pub fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            burst: 0,
        }
    }

    /// Builder method: set the burst capacity.
    #[must_use]
    pub fn with_burst(mut self, burst: u32) -> Self {
        self.burst = burst;
        self
    }

    /// Effective capacity: `max_requests + burst`.
    pub fn capacity(&self) -> u32 {
        self.max_requests.saturating_add(self.burst)
    }

    /// Refill rate in tokens per second.
    pub fn refill_rate(&self) -> f64 {
        if self.window.is_zero() {
            return 0.0;
        }
        f64::from(self.max_requests) / self.window.as_secs_f64()
    }
}

// ---------------------------------------------------------------------------
// QuotaResult
// ---------------------------------------------------------------------------

/// The outcome of a rate-limit check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaResult {
    /// Whether the request was allowed.
    pub allowed: bool,
    /// How many requests remain in the current window.
    pub remaining: u32,
    /// When the current rate-limit window resets (if known).
    pub reset_at: Option<DateTime<Utc>>,
    /// If rejected, how long to wait before retrying.
    pub retry_after: Option<Duration>,
}

impl QuotaResult {
    /// Create an "allowed" result.
    pub fn allowed(remaining: u32, reset_at: Option<DateTime<Utc>>) -> Self {
        Self {
            allowed: true,
            remaining,
            reset_at,
            retry_after: None,
        }
    }

    /// Create a "denied" result.
    pub fn denied(
        remaining: u32,
        reset_at: Option<DateTime<Utc>>,
        retry_after: Option<Duration>,
    ) -> Self {
        Self {
            allowed: false,
            remaining,
            reset_at,
            retry_after,
        }
    }
}

// ---------------------------------------------------------------------------
// Duration serde helpers
// ---------------------------------------------------------------------------

mod duration_serde {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Serialize a `Duration` as whole seconds (u64).
    pub fn serialize<S: Serializer>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        duration.as_secs().serialize(serializer)
    }

    /// Deserialize a `Duration` from whole seconds (u64).
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

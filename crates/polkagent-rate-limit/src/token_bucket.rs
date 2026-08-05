//! Token-bucket rate limiter.
//!
//! A classic token-bucket: the bucket holds up to `capacity` tokens and is
//! refilled at `refill_rate` tokens per `refill_interval`.  Each request
//! consumes one or more tokens.  When the bucket is empty, requests are
//! rejected until enough tokens have been replenished.
//!
//! # Thread safety
//!
//! This implementation uses [`parking_lot::Mutex`] for interior mutability,
//! making it safe to share across threads via `Arc`.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use tracing::trace;

use crate::quota::QuotaResult;
use crate::{floor_to_u32, RateLimiter};

// ---------------------------------------------------------------------------
// Inner state
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Inner {
    /// Current number of tokens (fractional for precision).
    tokens: f64,
    /// Maximum token capacity.
    capacity: f64,
    /// Tokens added per refill.
    refill_amount: f64,
    /// How often tokens are added.
    refill_interval: Duration,
    /// Last time tokens were refilled.
    last_refill: Instant,
}

impl Inner {
    /// Refill tokens based on elapsed time.
    fn refill(&mut self, now: Instant) {
        if self.refill_interval.is_zero() {
            return;
        }
        let elapsed = now.duration_since(self.last_refill);
        let intervals = elapsed.as_secs_f64() / self.refill_interval.as_secs_f64();
        let added = intervals * self.refill_amount;
        if added > 0.0 {
            self.tokens = (self.tokens + added).min(self.capacity);
            self.last_refill = now;
        }
    }

    /// Try to consume `cost` tokens. Returns `true` if successful.
    fn try_consume(&mut self, cost: u32) -> bool {
        let cost_f = f64::from(cost);
        if self.tokens >= cost_f {
            self.tokens -= cost_f;
            true
        } else {
            false
        }
    }

    /// Peek at available tokens (with virtual refill, no mutation).
    fn available(&self) -> f64 {
        if self.refill_interval.is_zero() {
            return self.tokens;
        }
        let elapsed = Instant::now().duration_since(self.last_refill);
        let intervals = elapsed.as_secs_f64() / self.refill_interval.as_secs_f64();
        let added = intervals * self.refill_amount;
        (self.tokens + added).min(self.capacity)
    }

    /// Estimate when enough tokens will be available for `cost`.
    fn time_until_available(&self, cost: u32) -> Duration {
        let cost_f = f64::from(cost);
        let deficit = cost_f - self.tokens;
        if deficit <= 0.0 {
            return Duration::ZERO;
        }
        if self.refill_interval.is_zero() || self.refill_amount <= 0.0 {
            // Will never refill.
            return Duration::from_secs(3600);
        }
        let intervals_needed = deficit / self.refill_amount;
        let secs = intervals_needed * self.refill_interval.as_secs_f64();
        Duration::from_secs_f64(secs)
    }
}

// ---------------------------------------------------------------------------
// TokenBucket
// ---------------------------------------------------------------------------

/// A token-bucket rate limiter.
///
/// # Example
///
/// ```rust,ignore
/// use std::time::Duration;
/// use polkagent_rate_limit::TokenBucket;
///
/// // 10 tokens capacity, refill 1 token every 100ms.
/// let bucket = TokenBucket::new(10, 1.0, Duration::from_millis(100));
/// let result = bucket.try_acquire("any-key", 1);
/// assert!(result.allowed);
/// ```
#[derive(Debug)]
pub struct TokenBucket {
    inner: Mutex<Inner>,
}

impl TokenBucket {
    /// Create a new token bucket.
    ///
    /// # Arguments
    ///
    /// * `capacity` -- maximum tokens the bucket can hold.
    /// * `refill_rate` -- tokens added per `refill_interval`.
    /// * `refill_interval` -- how often `refill_rate` tokens are added.
    pub fn new(capacity: u32, refill_rate: f64, refill_interval: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner {
                tokens: f64::from(capacity),
                capacity: f64::from(capacity),
                refill_amount: refill_rate,
                refill_interval,
                last_refill: Instant::now(),
            }),
        }
    }

    /// Create a token bucket from a tokens-per-second rate.
    ///
    /// Convenience constructor: `capacity` tokens burst, refilling at
    /// `tokens_per_second` with a 1-second interval.
    pub fn per_second(capacity: u32, tokens_per_second: f64) -> Self {
        Self::new(capacity, tokens_per_second, Duration::from_secs(1))
    }
}

impl RateLimiter for TokenBucket {
    fn try_acquire(&self, key: &str, cost: u32) -> QuotaResult {
        let mut inner = self.inner.lock();
        let now = Instant::now();
        inner.refill(now);

        if inner.try_consume(cost) {
            let remaining = floor_to_u32(inner.tokens);
            trace!(key, cost, remaining, "token bucket: allowed");
            QuotaResult::allowed(remaining, None)
        } else {
            let retry_after = inner.time_until_available(cost);
            let remaining = 0;
            trace!(key, cost, ?retry_after, "token bucket: denied");
            QuotaResult::denied(remaining, None, Some(retry_after))
        }
    }

    fn remaining(&self, _key: &str) -> u32 {
        let inner = self.inner.lock();
        floor_to_u32(inner.available())
    }

    fn reset_at(&self, _key: &str) -> Option<DateTime<Utc>> {
        // Token buckets refill continuously; there is no discrete reset.
        None
    }
}

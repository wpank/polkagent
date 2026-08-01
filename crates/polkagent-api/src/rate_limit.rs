//! Rate-limiter types for the Polkagent API.
//!
//! This module provides an in-memory, per-key **token-bucket** rate limiter.
//! It is intentionally a data-layer only — it does not integrate with Tower
//! middleware. Route handlers call [`RateLimiter::check`] directly when they
//! need to enforce a per-client or per-resource limit.
//!
//! # Algorithm
//!
//! Each key gets its own [`TokenBucket`].  The bucket starts full (at the
//! configured `burst` capacity).  On every request:
//!
//! 1. Tokens are refilled based on elapsed time since the last refill
//!    (`rate` tokens per second, up to `burst`).
//! 2. If at least one token is available it is consumed and the request is
//!    allowed.
//! 3. If no tokens remain the request is rejected with
//!    [`RateLimitError::TooManyRequests`].
//!
//! # Thread safety
//!
//! [`RateLimiter`] uses a [`dashmap::DashMap`] for lock-free concurrent
//! per-key access, making it safe to share across Axum handlers via `Arc`.
//!
//! # Example
//!
//! ```rust
//! use std::sync::Arc;
//! use polkagent_api::rate_limit::RateLimiter;
//!
//! let limiter = Arc::new(RateLimiter::new(10.0, 20));
//! assert!(limiter.check("client-a").is_ok());
//! assert_eq!(limiter.remaining("client-a"), 19);
//! ```

use std::time::Instant;

use dashmap::DashMap;
use thiserror::Error;

// ---------------------------------------------------------------------------
// RateLimitError
// ---------------------------------------------------------------------------

/// Error returned when a rate limit is exceeded.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RateLimitError {
    /// The caller has exhausted its token budget.
    #[error("too many requests: rate limit exceeded for key '{key}'")]
    TooManyRequests {
        /// The rate-limit key that was checked.
        key: String,
    },
}

// ---------------------------------------------------------------------------
// TokenBucket
// ---------------------------------------------------------------------------

/// A single token-bucket instance for one rate-limit key.
///
/// Not exposed publicly; managed internally by [`RateLimiter`].
#[derive(Debug)]
pub(crate) struct TokenBucket {
    /// Current number of available tokens (fractional to avoid drift).
    pub(crate) tokens: f64,
    /// Monotonic timestamp of the last refill calculation.
    pub(crate) last_refill: Instant,
    /// Token replenishment rate in tokens per second.
    pub(crate) rate: f64,
    /// Maximum number of tokens (burst capacity).
    pub(crate) burst: u32,
}

impl TokenBucket {
    /// Create a new, full bucket.
    fn new(rate: f64, burst: u32) -> Self {
        Self {
            tokens: f64::from(burst),
            last_refill: Instant::now(),
            rate,
            burst,
        }
    }

    /// Refill tokens based on elapsed time, then try to consume one.
    ///
    /// Returns `true` if a token was consumed (request allowed).
    fn try_consume(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;

        // Refill: add tokens proportional to elapsed time, cap at burst.
        self.tokens = (self.tokens + elapsed * self.rate).min(f64::from(self.burst));

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Return the current floor of available tokens without consuming any.
    fn available(&self) -> u32 {
        // Peek: compute what we would have after a refill, but do not mutate.
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        let projected = (self.tokens + elapsed * self.rate).min(f64::from(self.burst));
        projected.floor() as u32
    }
}

// ---------------------------------------------------------------------------
// RateLimiter
// ---------------------------------------------------------------------------

/// An in-memory, per-key token-bucket rate limiter.
///
/// # Construction
///
/// ```rust
/// use polkagent_api::rate_limit::RateLimiter;
///
/// // Allow 10 requests per second with a burst of 20.
/// let limiter = RateLimiter::new(10.0, 20);
/// ```
#[derive(Debug)]
pub struct RateLimiter {
    /// Per-key token buckets.
    buckets: DashMap<String, TokenBucket>,
    /// Token replenishment rate (tokens / second).
    requests_per_second: f64,
    /// Maximum token capacity (burst size).
    burst: u32,
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// # Arguments
    ///
    /// - `requests_per_second`: steady-state throughput, e.g. `10.0`.
    /// - `burst`: maximum tokens available at once (allows short spikes).
    ///
    /// # Panics
    ///
    /// Does not panic.  Negative or zero `requests_per_second` simply means
    /// no tokens are ever refilled — all requests after the initial burst
    /// will be rejected.
    pub fn new(requests_per_second: f64, burst: u32) -> Self {
        Self {
            buckets: DashMap::new(),
            requests_per_second,
            burst,
        }
    }

    /// Check whether the `key` is within its rate limit.
    ///
    /// If the key's bucket has at least one token it is consumed and
    /// `Ok(())` is returned.  Otherwise [`RateLimitError::TooManyRequests`]
    /// is returned.
    ///
    /// # Errors
    ///
    /// Returns [`RateLimitError::TooManyRequests`] when the bucket is empty.
    pub fn check(&self, key: &str) -> Result<(), RateLimitError> {
        let mut bucket = self.buckets
            .entry(key.to_owned())
            .or_insert_with(|| TokenBucket::new(self.requests_per_second, self.burst));

        if bucket.try_consume() {
            Ok(())
        } else {
            Err(RateLimitError::TooManyRequests {
                key: key.to_owned(),
            })
        }
    }

    /// Return the number of tokens currently available for `key`.
    ///
    /// Returns the burst capacity when the key has not yet been seen.
    /// This is a snapshot and may change by the time `check` is called.
    pub fn remaining(&self, key: &str) -> u32 {
        match self.buckets.get(key) {
            Some(bucket) => bucket.available(),
            None => self.burst,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn within_limit_passes() {
        let limiter = RateLimiter::new(10.0, 5);
        assert!(limiter.check("user-a").is_ok());
    }

    #[test]
    fn over_limit_rejected() {
        // Burst of 2 → first two allowed, third rejected immediately.
        let limiter = RateLimiter::new(0.0, 2);
        assert!(limiter.check("user-b").is_ok());
        assert!(limiter.check("user-b").is_ok());
        let result = limiter.check("user-b");
        assert!(result.is_err());
        match result.unwrap_err() {
            RateLimitError::TooManyRequests { key } => assert_eq!(key, "user-b"),
        }
    }

    #[test]
    fn tokens_refill_over_time() {
        // Start with burst=1, rate=100/s.  Exhaust the bucket, then sleep
        // 20 ms — at 100 tokens/s that refills at least 1 token.
        let limiter = RateLimiter::new(100.0, 1);
        assert!(limiter.check("user-c").is_ok());
        assert!(limiter.check("user-c").is_err(), "should be exhausted");

        std::thread::sleep(Duration::from_millis(20));

        assert!(
            limiter.check("user-c").is_ok(),
            "tokens should have refilled after sleep"
        );
    }

    #[test]
    fn remaining_starts_at_burst_for_unseen_key() {
        let limiter = RateLimiter::new(10.0, 15);
        assert_eq!(limiter.remaining("brand-new-key"), 15);
    }

    #[test]
    fn remaining_decreases_after_check() {
        let limiter = RateLimiter::new(0.0, 10);
        limiter.check("key").unwrap();
        assert_eq!(limiter.remaining("key"), 9);
    }

    #[test]
    fn different_keys_have_independent_buckets() {
        let limiter = RateLimiter::new(0.0, 1);
        // Key A: use up the bucket.
        assert!(limiter.check("key-a").is_ok());
        assert!(limiter.check("key-a").is_err());
        // Key B is still full.
        assert!(limiter.check("key-b").is_ok());
    }

    #[test]
    fn rate_limit_error_display_contains_key() {
        let err = RateLimitError::TooManyRequests {
            key: "test-key".into(),
        };
        assert!(err.to_string().contains("test-key"));
    }
}

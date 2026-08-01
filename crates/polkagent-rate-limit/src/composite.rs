//! Composite rate limiter that applies multiple strategies simultaneously.
//!
//! A [`CompositeRateLimiter`] holds a list of inner [`RateLimiter`] instances
//! and requires **all** of them to allow a request.  This enables layered
//! policies such as "10 requests per second AND 1000 requests per hour".
//!
//! If any inner limiter denies the request, the composite returns the denial
//! with the longest `retry_after` duration (i.e., the most restrictive).

use std::fmt;

use chrono::{DateTime, Utc};
use tracing::trace;

use crate::quota::QuotaResult;
use crate::RateLimiter;

/// A rate limiter that enforces multiple inner limits simultaneously.
///
/// A request is allowed only if **every** inner limiter allows it.
///
/// # Example
///
/// ```rust,ignore
/// use std::time::Duration;
/// use polkagent_rate_limit::{CompositeRateLimiter, TokenBucket};
///
/// let per_second = TokenBucket::per_second(10, 10.0);
/// let per_minute = TokenBucket::new(100, 100.0, Duration::from_secs(60));
///
/// let composite = CompositeRateLimiter::new(vec![
///     Box::new(per_second),
///     Box::new(per_minute),
/// ]);
/// ```
pub struct CompositeRateLimiter {
    limiters: Vec<Box<dyn RateLimiter>>,
}

impl fmt::Debug for CompositeRateLimiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompositeRateLimiter")
            .field("limiters_count", &self.limiters.len())
            .finish()
    }
}

impl CompositeRateLimiter {
    /// Create a composite limiter from a list of inner limiters.
    pub fn new(limiters: Vec<Box<dyn RateLimiter>>) -> Self {
        Self { limiters }
    }

    /// Builder method: add an inner limiter.
    #[must_use]
    pub fn with(mut self, limiter: Box<dyn RateLimiter>) -> Self {
        self.limiters.push(limiter);
        self
    }
}

impl RateLimiter for CompositeRateLimiter {
    fn try_acquire(&self, key: &str, cost: u32) -> QuotaResult {
        // First pass: check all limiters.  Collect results so we can find
        // the most restrictive denial if any limiter rejects.
        let results: Vec<QuotaResult> = self
            .limiters
            .iter()
            .map(|l| l.try_acquire(key, cost))
            .collect();

        let any_denied = results.iter().any(|r| !r.allowed);

        if any_denied {
            // Find the most restrictive denial (longest retry_after).
            let mut worst_retry = None;
            let mut earliest_reset = None;

            for r in &results {
                if !r.allowed {
                    if let Some(ra) = r.retry_after {
                        worst_retry = Some(match worst_retry {
                            Some(existing) if ra > existing => ra,
                            Some(existing) => existing,
                            None => ra,
                        });
                    }
                    if let Some(reset) = r.reset_at {
                        earliest_reset = Some(match earliest_reset {
                            Some(existing) if reset < existing => reset,
                            Some(existing) => existing,
                            None => reset,
                        });
                    }
                }
            }

            trace!(key, cost, "composite: denied by at least one limiter");
            QuotaResult::denied(0, earliest_reset, worst_retry)
        } else {
            // All allowed: remaining is the minimum across all limiters.
            let min_remaining = results.iter().map(|r| r.remaining).min().unwrap_or(0);
            let earliest_reset = results.iter().filter_map(|r| r.reset_at).min();
            trace!(key, cost, min_remaining, "composite: allowed by all limiters");
            QuotaResult::allowed(min_remaining, earliest_reset)
        }
    }

    fn remaining(&self, key: &str) -> u32 {
        self.limiters
            .iter()
            .map(|l| l.remaining(key))
            .min()
            .unwrap_or(0)
    }

    fn reset_at(&self, key: &str) -> Option<DateTime<Utc>> {
        self.limiters
            .iter()
            .filter_map(|l| l.reset_at(key))
            .min()
    }
}

// Send + Sync are auto-derived because Vec<Box<dyn RateLimiter>> is
// Send + Sync when the trait bound includes Send + Sync.

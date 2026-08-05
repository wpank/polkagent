//! Retry policy for the durable outbox.
//!
//! [`ExponentialBackoff`] computes the duration to wait before the next
//! delivery attempt using the standard exponential-backoff-with-jitter
//! algorithm. The "full jitter" variant is used, which provides better
//! load-distribution under high-concurrency bursts than the "decorrelated"
//! or "equal jitter" alternatives.
//!
//! Dead-lettering: once a message has exhausted all retries (i.e.
//! `attempt_count > max_retries`) it is moved to the dead-letter set
//! by [`ExponentialBackoff::should_dead_letter`] and will never be
//! re-queued automatically.

use std::time::Duration;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ExponentialBackoff
// ---------------------------------------------------------------------------

/// Exponential-backoff-with-full-jitter policy.
///
/// # Algorithm
///
/// ```text
/// sleep = rand(0, min(cap, base * 2^attempt))
/// ```
///
/// where `cap` is [`max_delay`](Self::max_delay) and `base` is
/// [`base_delay`](Self::base_delay).
///
/// # Dead-lettering
///
/// When `attempt >= max_attempts` the message is considered exhausted and
/// [`should_dead_letter`](Self::should_dead_letter) returns `true`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExponentialBackoff {
    /// Duration of the first retry interval (before scaling).
    pub base_delay: Duration,
    /// Upper bound placed on the computed delay.
    pub max_delay: Duration,
    /// Maximum number of attempts before the message is dead-lettered.
    ///
    /// An attempt count of `0` corresponds to the original delivery try.
    /// Setting `max_attempts = 3` allows the original attempt plus up to three
    /// retries.
    pub max_attempts: u32,
}

impl ExponentialBackoff {
    /// Create a policy with the given parameters.
    #[must_use]
    pub fn new(base_delay: Duration, max_delay: Duration, max_attempts: u32) -> Self {
        Self {
            base_delay,
            max_delay,
            max_attempts,
        }
    }

    /// Compute the next delay for the given `attempt` number (0-indexed).
    ///
    /// Uses full-jitter: `rand(0, min(cap, base * 2^attempt))`.
    ///
    /// The PRNG used here is a lightweight deterministic xorshift derived from
    /// the `attempt` value combined with [`std::time::SystemTime`] nanoseconds
    /// so that there is no external dependency on `rand` while still providing
    /// reasonable statistical jitter in production.
    #[must_use]
    pub fn next_delay(&self, attempt: u32) -> Duration {
        // Cap the exponent to avoid overflow on large attempt numbers.
        // u128 has 128 bits; we cap shift at 63 so 1 << shift fits in u64
        // (and the product fits in u128 after widening).
        let shift = u64::from(attempt.min(63));
        let cap_nanos = self.max_delay.as_nanos();
        let base_nanos = self.base_delay.as_nanos();
        // 2^shift as u128 — safe because shift ≤ 63 < 128.
        let multiplier: u128 = 1u128 << shift;
        let upper = cap_nanos.min(base_nanos.saturating_mul(multiplier));

        if upper == 0 {
            return Duration::ZERO;
        }

        // Lightweight jitter: mix SystemTime nanos with the attempt index.
        let seed = system_nanos() ^ (u128::from(attempt).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let jittered = seed % upper;
        Duration::from_nanos(u64::try_from(jittered).unwrap_or(u64::MAX))
    }

    /// Returns `true` when `attempt_count` has exceeded the allowed maximum,
    /// meaning the message should be moved to the dead-letter set.
    ///
    /// `attempt_count` is the number of delivery attempts already made
    /// (incremented *after* each attempt). Therefore the message should be
    /// dead-lettered when `attempt_count > max_attempts`.
    #[must_use]
    pub fn should_dead_letter(&self, attempt_count: u32) -> bool {
        attempt_count > self.max_attempts
    }
}

impl Default for ExponentialBackoff {
    /// Sensible defaults: 1 s base, 5 min cap, up to 5 attempts.
    fn default() -> Self {
        Self {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(300),
            max_attempts: 5,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns [`std::time::SystemTime`] elapsed since the UNIX epoch in
/// nanoseconds, falling back to 0 on the (unlikely) error path.
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
mod tests {
    use super::*;

    fn policy(base_secs: u64, max_secs: u64, max_attempts: u32) -> ExponentialBackoff {
        ExponentialBackoff::new(
            Duration::from_secs(base_secs),
            Duration::from_secs(max_secs),
            max_attempts,
        )
    }

    #[test]
    fn next_delay_is_within_cap() {
        let pol = policy(1, 60, 5);
        for attempt in 0..20u32 {
            let d = pol.next_delay(attempt);
            assert!(
                d <= pol.max_delay,
                "attempt={attempt}: delay {d:?} exceeded cap {:?}",
                pol.max_delay
            );
        }
    }

    #[test]
    fn next_delay_zero_base_is_always_zero() {
        let pol = ExponentialBackoff::new(Duration::ZERO, Duration::from_secs(60), 5);
        for attempt in 0..10u32 {
            assert_eq!(pol.next_delay(attempt), Duration::ZERO);
        }
    }

    #[test]
    fn should_dead_letter_threshold() {
        let pol = policy(1, 60, 3);
        assert!(!pol.should_dead_letter(0));
        assert!(!pol.should_dead_letter(3));
        assert!(pol.should_dead_letter(4));
        assert!(pol.should_dead_letter(100));
    }

    #[test]
    fn default_policy_has_sensible_values() {
        let pol = ExponentialBackoff::default();
        assert_eq!(pol.max_attempts, 5);
        assert!(pol.max_delay >= pol.base_delay);
    }

    #[test]
    fn large_attempt_number_does_not_panic() {
        let pol = policy(1, 300, 10);
        // Should not panic or overflow
        let _ = pol.next_delay(200);
    }
}

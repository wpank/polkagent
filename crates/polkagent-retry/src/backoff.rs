use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Strategy for computing the delay between retry attempts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackoffStrategy {
    /// Constant delay between retries.
    Fixed(Duration),

    /// Exponential backoff: delay = base * 2^attempt, capped at max.
    /// Optional jitter adds a random component to avoid thundering herd.
    Exponential {
        /// Base delay (used for the first retry).
        base: Duration,
        /// Maximum delay cap.
        max: Duration,
        /// Whether to add random jitter to the computed delay.
        jitter: bool,
    },

    /// Linear backoff: delay = step * (attempt + 1), capped at max.
    Linear {
        /// Increment per attempt.
        step: Duration,
        /// Maximum delay cap.
        max: Duration,
    },
}

impl BackoffStrategy {
    /// Compute the delay for the given zero-based attempt number.
    ///
    /// For `Exponential` with `jitter`, the jitter is deterministic
    /// based on `jitter_seed` (used for testing reproducibility).
    /// In production, callers should pass a random seed.
    pub fn delay(&self, attempt: u32, jitter_seed: u64) -> Duration {
        match self {
            Self::Fixed(d) => *d,

            Self::Exponential { base, max, jitter } => {
                let multiplier = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
                let base_ms = duration_millis_saturating(*base);
                let delay_ms = base_ms.saturating_mul(multiplier);
                let max_ms = duration_millis_saturating(*max);
                let capped = delay_ms.min(max_ms);

                if *jitter {
                    // Simple deterministic jitter: delay in [capped/2, capped]
                    let half = capped / 2;
                    let jitter_range = capped - half;
                    let jitter_value = if jitter_range == 0 {
                        0
                    } else {
                        jitter_seed % jitter_range
                    };
                    Duration::from_millis(half + jitter_value)
                } else {
                    Duration::from_millis(capped)
                }
            }

            Self::Linear { step, max } => {
                let step_ms = duration_millis_saturating(*step);
                let delay_ms = step_ms.saturating_mul(u64::from(attempt) + 1);
                let max_ms = duration_millis_saturating(*max);
                Duration::from_millis(delay_ms.min(max_ms))
            }
        }
    }

    /// Compute the delay using a random jitter seed from the system.
    pub fn delay_with_random_jitter(&self, attempt: u32) -> Duration {
        // Use a simple entropy source for jitter
        let seed = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            u64::try_from(now.as_nanos()).unwrap_or(u64::MAX) ^ u64::from(attempt)
        };
        self.delay(attempt, seed)
    }
}

fn duration_millis_saturating(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

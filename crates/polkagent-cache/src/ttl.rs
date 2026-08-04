use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Describes the time-to-live policy for cached entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum TtlPolicy {
    /// No expiration -- entries live until explicitly removed or evicted.
    #[default]
    None,

    /// A fixed duration after insertion.
    Fixed(Duration),

    /// A sliding window: the TTL resets on every access.
    Sliding(Duration),

    /// Adaptive TTL that adjusts between `min` and `max` based on access
    /// frequency.  Each access multiplies the remaining TTL by `factor`
    /// (capped at `max`).  Entries that are rarely accessed decay toward
    /// `min`.
    Adaptive {
        min: Duration,
        max: Duration,
        factor: f64,
    },
}

impl TtlPolicy {
    /// Compute the initial expiration instant for a newly inserted entry.
    pub fn initial_expiry(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Self::None => Option::None,
            Self::Fixed(d) | Self::Sliding(d) => {
                let millis = i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
                Some(now + chrono::Duration::milliseconds(millis))
            }
            Self::Adaptive { min, .. } => {
                let millis = i64::try_from(min.as_millis()).unwrap_or(i64::MAX);
                Some(now + chrono::Duration::milliseconds(millis))
            }
        }
    }

    /// Recompute the expiration after an access.
    ///
    /// For `Sliding`, the window resets from `now`.
    /// For `Adaptive`, the remaining TTL is extended by `factor` up to `max`.
    /// For `Fixed` or `None`, the current expiry is returned unchanged.
    pub fn on_access(
        &self,
        now: DateTime<Utc>,
        current_expiry: Option<DateTime<Utc>>,
    ) -> Option<DateTime<Utc>> {
        match self {
            Self::None => Option::None,
            Self::Fixed(_) => current_expiry,
            Self::Sliding(d) => {
                let millis = i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
                Some(now + chrono::Duration::milliseconds(millis))
            }
            Self::Adaptive { max, factor, .. } => {
                let remaining = current_expiry.map_or_else(chrono::Duration::zero, |e| e - now);
                #[allow(clippy::cast_precision_loss)]
                let remaining_ms = remaining.num_milliseconds().max(0) as f64;
                let extended_ms = remaining_ms * factor;
                #[allow(clippy::cast_precision_loss)]
                let max_ms = max.as_millis() as f64;
                let capped_ms = extended_ms.min(max_ms);
                #[allow(clippy::cast_possible_truncation)]
                let capped_i64 = capped_ms as i64;
                Some(now + chrono::Duration::milliseconds(capped_i64))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_ttl_sets_initial_expiry() {
        let now = Utc::now();
        let policy = TtlPolicy::Fixed(Duration::from_secs(60));
        let expiry = policy.initial_expiry(now).expect("should have expiry");
        let diff = expiry - now;
        assert_eq!(diff.num_seconds(), 60);
    }

    #[test]
    fn fixed_ttl_does_not_change_on_access() {
        let now = Utc::now();
        let policy = TtlPolicy::Fixed(Duration::from_secs(60));
        let expiry = policy.initial_expiry(now);
        let later = now + chrono::Duration::seconds(30);
        let refreshed = policy.on_access(later, expiry);
        assert_eq!(refreshed, expiry);
    }

    #[test]
    fn sliding_ttl_resets_on_access() {
        let now = Utc::now();
        let policy = TtlPolicy::Sliding(Duration::from_secs(60));
        let expiry = policy.initial_expiry(now);
        let later = now + chrono::Duration::seconds(30);
        let refreshed = policy.on_access(later, expiry).expect("should have expiry");
        let diff = refreshed - later;
        assert_eq!(diff.num_seconds(), 60);
    }

    #[test]
    fn adaptive_starts_at_min() {
        let now = Utc::now();
        let policy = TtlPolicy::Adaptive {
            min: Duration::from_secs(10),
            max: Duration::from_secs(300),
            factor: 2.0,
        };
        let expiry = policy.initial_expiry(now).expect("should have expiry");
        let diff = expiry - now;
        assert_eq!(diff.num_seconds(), 10);
    }

    #[test]
    fn adaptive_extends_on_access_capped_at_max() {
        let now = Utc::now();
        let policy = TtlPolicy::Adaptive {
            min: Duration::from_secs(10),
            max: Duration::from_secs(30),
            factor: 5.0,
        };
        let expiry = policy.initial_expiry(now);
        // Access immediately (remaining ~10s, * 5 = 50s, capped to 30s)
        let refreshed = policy.on_access(now, expiry).expect("should have expiry");
        let diff = refreshed - now;
        assert!(diff.num_seconds() <= 30);
    }

    #[test]
    fn none_policy_never_expires() {
        let now = Utc::now();
        let policy = TtlPolicy::None;
        assert!(policy.initial_expiry(now).is_none());
        assert!(policy.on_access(now, None).is_none());
    }
}

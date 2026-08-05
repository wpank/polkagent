//! Provider-level health status tracking.
//!
//! [`ProviderHealthStatus`] captures real-time health metrics for a model
//! provider endpoint, including latency percentiles, error rates, and
//! circuit breaker state. This is distinct from the infrastructure-level
//! health checks in `polkagent-health`.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::circuit_breaker::{CircuitBreaker, CircuitState};

/// High-level health state of a provider endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// The provider is operating normally.
    Healthy,
    /// The provider is experiencing elevated errors or latency but is still
    /// partially usable.
    Degraded,
    /// The provider is not usable (circuit open, all requests failing).
    Unhealthy,
    /// No data has been collected yet.
    Unknown,
}

/// Provider-level health status with latency and error metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealthStatus {
    /// Current high-level health state.
    pub state: HealthState,
    /// When the last successful request completed, if ever.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_instant_epoch"
    )]
    pub last_success: Option<Instant>,
    /// When the last failed request occurred, if ever.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_instant_epoch"
    )]
    pub last_failure: Option<Instant>,
    /// Median latency (p50) of recent requests.
    #[serde(with = "duration_millis")]
    pub latency_p50: Duration,
    /// 99th percentile latency of recent requests.
    #[serde(with = "duration_millis")]
    pub latency_p99: Duration,
    /// Error rate as a fraction in [0.0, 1.0].
    pub error_rate: f64,
    /// Current circuit breaker state description.
    pub circuit: CircuitSnapshot,
}

/// A serializable snapshot of a circuit breaker's state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitSnapshot {
    /// Current state name.
    pub state: String,
    /// Number of consecutive failures (only meaningful when closed).
    pub failure_count: u32,
    /// The failure threshold that triggers opening.
    pub failure_threshold: u32,
}

impl CircuitSnapshot {
    /// Create a snapshot from a circuit breaker.
    pub fn from_circuit_breaker(cb: &CircuitBreaker) -> Self {
        let state_name = match cb.state() {
            CircuitState::Closed => "closed".to_string(),
            CircuitState::Open { .. } => "open".to_string(),
            CircuitState::HalfOpen => "half_open".to_string(),
        };

        Self {
            state: state_name,
            failure_count: 0, // not directly exposed; tracked internally
            failure_threshold: cb.config().failure_threshold,
        }
    }
}

/// Tracks latency samples using a fixed-size circular buffer.
pub struct LatencyTracker {
    inner: Mutex<LatencyTrackerInner>,
}

struct LatencyTrackerInner {
    samples: Vec<Duration>,
    head: usize,
    count: usize,
    successes: u64,
    failures: u64,
    last_success: Option<Instant>,
    last_failure: Option<Instant>,
}

impl LatencyTracker {
    /// Create a new tracker with the given window size.
    pub fn new(window_size: usize) -> Self {
        Self {
            inner: Mutex::new(LatencyTrackerInner {
                samples: vec![Duration::ZERO; window_size],
                head: 0,
                count: 0,
                successes: 0,
                failures: 0,
                last_success: None,
                last_failure: None,
            }),
        }
    }

    /// Record a successful request with the given latency.
    pub fn record_success(&self, latency: Duration) {
        let mut inner = self.inner.lock();
        let cap = inner.samples.len();
        let head = inner.head;
        inner.samples[head] = latency;
        inner.head = (head + 1) % cap;
        if inner.count < cap {
            inner.count += 1;
        }
        inner.successes += 1;
        inner.last_success = Some(Instant::now());
    }

    /// Record a failed request.
    pub fn record_failure(&self) {
        let mut inner = self.inner.lock();
        inner.failures += 1;
        inner.last_failure = Some(Instant::now());
    }

    /// Compute the current health status.
    pub fn snapshot(&self, cb: Option<&CircuitBreaker>) -> ProviderHealthStatus {
        let inner = self.inner.lock();

        let (p50, p99) = if inner.count == 0 {
            (Duration::ZERO, Duration::ZERO)
        } else {
            let mut sorted: Vec<Duration> = inner.samples[..inner.count].to_vec();
            sorted.sort();
            let p50_idx = inner.count / 2;
            let p99_idx = inner.count - inner.count.div_ceil(100);
            (sorted[p50_idx], sorted[p99_idx])
        };

        let total = inner.successes + inner.failures;
        // Precision beyond f64's integer mantissa is immaterial for a health ratio.
        #[allow(clippy::cast_precision_loss)]
        let error_rate = if total == 0 {
            0.0
        } else {
            inner.failures as f64 / total as f64
        };

        let circuit = cb.map_or(
            CircuitSnapshot {
                state: "none".to_string(),
                failure_count: 0,
                failure_threshold: 0,
            },
            CircuitSnapshot::from_circuit_breaker,
        );

        let state = if let Some(cb) = cb {
            match cb.state() {
                CircuitState::Open { .. } => HealthState::Unhealthy,
                CircuitState::HalfOpen => HealthState::Degraded,
                CircuitState::Closed => {
                    if total == 0 {
                        HealthState::Unknown
                    } else if error_rate > 0.5 {
                        HealthState::Unhealthy
                    } else if error_rate > 0.1 {
                        HealthState::Degraded
                    } else {
                        HealthState::Healthy
                    }
                }
            }
        } else if total == 0 {
            HealthState::Unknown
        } else if error_rate > 0.5 {
            HealthState::Unhealthy
        } else if error_rate > 0.1 {
            HealthState::Degraded
        } else {
            HealthState::Healthy
        };

        ProviderHealthStatus {
            state,
            last_success: inner.last_success,
            last_failure: inner.last_failure,
            latency_p50: p50,
            latency_p99: p99,
            error_rate,
            circuit,
        }
    }

    /// Reset all tracked data.
    pub fn reset(&self) {
        let mut inner = self.inner.lock();
        let cap = inner.samples.len();
        inner.samples = vec![Duration::ZERO; cap];
        inner.head = 0;
        inner.count = 0;
        inner.successes = 0;
        inner.failures = 0;
        inner.last_success = None;
        inner.last_failure = None;
    }

    /// Return the total number of requests tracked.
    pub fn total_requests(&self) -> u64 {
        let inner = self.inner.lock();
        inner.successes + inner.failures
    }
}

impl std::fmt::Debug for LatencyTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock();
        f.debug_struct("LatencyTracker")
            .field("count", &inner.count)
            .field("successes", &inner.successes)
            .field("failures", &inner.failures)
            .finish()
    }
}

/// Serde helper for `Duration` as milliseconds (u64).
mod duration_millis {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        u64::try_from(value.as_millis())
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let ms = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(ms))
    }
}

/// Serde helper for `Option<Instant>` — serializes as epoch millis (u64).
/// Note: `Instant` has no absolute meaning, so this is approximate and only
/// useful for debugging/display, not for deserialization across processes.
mod optional_instant_epoch {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Instant;

    // Serde's `with` callback ABI passes the field as `&Option<T>`.
    #[allow(clippy::ref_option)]
    pub fn serialize<S: Serializer>(
        value: &Option<Instant>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(instant) => {
                let elapsed = instant.elapsed();
                // Represent as "milliseconds ago" for debugging.
                elapsed.as_millis().serialize(serializer)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Instant>, D::Error> {
        let opt: Option<u128> = Option::deserialize(deserializer)?;
        Ok(opt.and_then(|ms_ago| {
            let milliseconds = u64::try_from(ms_ago).ok()?;
            Instant::now().checked_sub(std::time::Duration::from_millis(milliseconds))
        }))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn health_state_serde_variants() {
        let variants = vec![
            (HealthState::Healthy, "\"healthy\""),
            (HealthState::Degraded, "\"degraded\""),
            (HealthState::Unhealthy, "\"unhealthy\""),
            (HealthState::Unknown, "\"unknown\""),
        ];
        for (state, expected) in variants {
            let json = serde_json::to_string(&state).expect("serialize");
            assert_eq!(json, expected, "failed for {state:?}");
            let back: HealthState = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, state);
        }
    }

    #[test]
    fn latency_tracker_empty_snapshot_is_unknown() {
        let tracker = LatencyTracker::new(100);
        let snapshot = tracker.snapshot(None);
        assert_eq!(snapshot.state, HealthState::Unknown);
        assert_eq!(snapshot.latency_p50, Duration::ZERO);
        assert_eq!(snapshot.latency_p99, Duration::ZERO);
        assert_eq!(snapshot.error_rate, 0.0);
    }

    #[test]
    fn latency_tracker_all_successes_is_healthy() {
        let tracker = LatencyTracker::new(100);
        for i in 0..10 {
            tracker.record_success(Duration::from_millis(i * 10));
        }
        let snapshot = tracker.snapshot(None);
        assert_eq!(snapshot.state, HealthState::Healthy);
        assert_eq!(snapshot.error_rate, 0.0);
        assert!(snapshot.latency_p50 > Duration::ZERO);
        assert!(snapshot.last_success.is_some());
    }

    #[test]
    fn latency_tracker_high_error_rate_is_unhealthy() {
        let tracker = LatencyTracker::new(100);
        // 2 successes, 8 failures = 80% error rate
        tracker.record_success(Duration::from_millis(10));
        tracker.record_success(Duration::from_millis(20));
        for _ in 0..8 {
            tracker.record_failure();
        }
        let snapshot = tracker.snapshot(None);
        assert_eq!(snapshot.state, HealthState::Unhealthy);
        assert!(snapshot.error_rate > 0.5);
        assert!(snapshot.last_failure.is_some());
    }

    #[test]
    fn latency_tracker_moderate_error_rate_is_degraded() {
        let tracker = LatencyTracker::new(100);
        // 8 successes, 2 failures = 20% error rate
        for _ in 0..8 {
            tracker.record_success(Duration::from_millis(10));
        }
        for _ in 0..2 {
            tracker.record_failure();
        }
        let snapshot = tracker.snapshot(None);
        assert_eq!(snapshot.state, HealthState::Degraded);
        assert!(snapshot.error_rate > 0.1);
        assert!(snapshot.error_rate <= 0.5);
    }

    #[test]
    fn latency_tracker_with_open_circuit_is_unhealthy() {
        let tracker = LatencyTracker::new(100);
        tracker.record_success(Duration::from_millis(10));

        let cb = CircuitBreaker::new(1, 1, Duration::from_secs(60));
        cb.record_failure(); // opens the circuit

        let snapshot = tracker.snapshot(Some(&cb));
        assert_eq!(snapshot.state, HealthState::Unhealthy);
        assert_eq!(snapshot.circuit.state, "open");
    }

    #[test]
    fn latency_tracker_with_half_open_circuit_is_degraded() {
        let tracker = LatencyTracker::new(100);
        tracker.record_success(Duration::from_millis(10));

        let cb = CircuitBreaker::new(1, 1, Duration::from_millis(1));
        cb.record_failure(); // opens
        std::thread::sleep(Duration::from_millis(10)); // wait for half-open

        let snapshot = tracker.snapshot(Some(&cb));
        assert_eq!(snapshot.state, HealthState::Degraded);
        assert_eq!(snapshot.circuit.state, "half_open");
    }

    #[test]
    fn latency_tracker_with_closed_circuit_uses_error_rate() {
        let tracker = LatencyTracker::new(100);
        for _ in 0..10 {
            tracker.record_success(Duration::from_millis(10));
        }

        let cb = CircuitBreaker::new(5, 1, Duration::from_secs(60));
        let snapshot = tracker.snapshot(Some(&cb));
        assert_eq!(snapshot.state, HealthState::Healthy);
        assert_eq!(snapshot.circuit.state, "closed");
    }

    #[test]
    fn latency_tracker_percentiles_correct() {
        let tracker = LatencyTracker::new(100);
        // Insert sorted values: 10, 20, 30, ..., 100
        for i in 1..=10 {
            tracker.record_success(Duration::from_millis(i * 10));
        }

        let snapshot = tracker.snapshot(None);
        // p50 should be around 50-60ms
        assert!(
            snapshot.latency_p50 >= Duration::from_millis(50),
            "p50 = {:?}",
            snapshot.latency_p50
        );
        // p99 should be 100ms (last sample)
        assert!(
            snapshot.latency_p99 >= Duration::from_millis(90),
            "p99 = {:?}",
            snapshot.latency_p99
        );
    }

    #[test]
    fn latency_tracker_circular_buffer_wraps() {
        let tracker = LatencyTracker::new(5);
        // Fill beyond capacity
        for i in 0..10 {
            tracker.record_success(Duration::from_millis(i * 10));
        }
        assert_eq!(tracker.total_requests(), 10);

        let snapshot = tracker.snapshot(None);
        // Should still compute valid percentiles from the last 5 samples
        assert!(snapshot.latency_p50 > Duration::ZERO);
    }

    #[test]
    fn latency_tracker_reset_clears_data() {
        let tracker = LatencyTracker::new(100);
        tracker.record_success(Duration::from_millis(50));
        tracker.record_failure();
        assert_eq!(tracker.total_requests(), 2);

        tracker.reset();
        assert_eq!(tracker.total_requests(), 0);

        let snapshot = tracker.snapshot(None);
        assert_eq!(snapshot.state, HealthState::Unknown);
    }

    #[test]
    fn circuit_snapshot_from_closed_breaker() {
        let cb = CircuitBreaker::new(5, 2, Duration::from_secs(30));
        let snap = CircuitSnapshot::from_circuit_breaker(&cb);
        assert_eq!(snap.state, "closed");
        assert_eq!(snap.failure_threshold, 5);
    }

    #[test]
    fn circuit_snapshot_from_open_breaker() {
        let cb = CircuitBreaker::new(1, 1, Duration::from_secs(60));
        cb.record_failure();
        let snap = CircuitSnapshot::from_circuit_breaker(&cb);
        assert_eq!(snap.state, "open");
    }

    #[test]
    fn provider_health_status_serde_roundtrip() {
        let status = ProviderHealthStatus {
            state: HealthState::Healthy,
            last_success: None,
            last_failure: None,
            latency_p50: Duration::from_millis(42),
            latency_p99: Duration::from_millis(200),
            error_rate: 0.05,
            circuit: CircuitSnapshot {
                state: "closed".to_string(),
                failure_count: 0,
                failure_threshold: 5,
            },
        };
        let json = serde_json::to_string(&status).expect("serialize");
        let back: ProviderHealthStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.state, HealthState::Healthy);
        assert_eq!(back.latency_p50, Duration::from_millis(42));
        assert_eq!(back.error_rate, 0.05);
    }

    #[test]
    fn latency_tracker_debug_impl() {
        let tracker = LatencyTracker::new(10);
        tracker.record_success(Duration::from_millis(5));
        let debug = format!("{tracker:?}");
        assert!(debug.contains("LatencyTracker"));
        assert!(debug.contains("successes"));
    }
}

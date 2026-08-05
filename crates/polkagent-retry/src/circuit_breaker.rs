use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::CircuitOpen;

/// The state of a circuit breaker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation; requests are allowed through.
    Closed,
    /// The circuit is open; requests are rejected until the given instant.
    Open {
        /// When the circuit breaker should transition to half-open.
        until: Instant,
    },
    /// The circuit is testing whether the downstream service has recovered.
    HalfOpen,
}

/// Configuration for a circuit breaker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// Number of consecutive successes in half-open state before closing.
    pub success_threshold: u32,
    /// How long the circuit stays open before transitioning to half-open.
    #[serde(with = "duration_secs")]
    pub open_duration: Duration,
}

/// A circuit breaker that tracks failures and opens/closes based on thresholds.
///
/// Thread-safe via internal `Mutex`.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    inner: Mutex<CircuitBreakerInner>,
}

struct CircuitBreakerInner {
    state: CircuitState,
    consecutive_failures: u32,
    consecutive_successes: u32,
}

impl CircuitBreaker {
    /// Create a new circuit breaker.
    ///
    /// - `failure_threshold`: consecutive failures before opening
    /// - `success_threshold`: consecutive successes in half-open before closing
    /// - `open_duration`: how long the circuit stays open
    pub fn new(failure_threshold: u32, success_threshold: u32, open_duration: Duration) -> Self {
        Self {
            config: CircuitBreakerConfig {
                failure_threshold,
                success_threshold,
                open_duration,
            },
            inner: Mutex::new(CircuitBreakerInner {
                state: CircuitState::Closed,
                consecutive_failures: 0,
                consecutive_successes: 0,
            }),
        }
    }

    /// Create a circuit breaker from a config.
    pub fn from_config(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            inner: Mutex::new(CircuitBreakerInner {
                state: CircuitState::Closed,
                consecutive_failures: 0,
                consecutive_successes: 0,
            }),
        }
    }

    /// Return the current state of the circuit breaker.
    pub fn state(&self) -> CircuitState {
        let mut inner = self.inner.lock();
        Self::maybe_transition_to_half_open(&self.config, &mut inner);
        inner.state.clone()
    }

    /// Check whether a request is allowed to proceed.
    ///
    /// Returns `Ok(())` if the request is allowed, or `Err(CircuitOpen)` if not.
    pub fn check(&self) -> Result<(), CircuitOpen> {
        let mut inner = self.inner.lock();
        Self::maybe_transition_to_half_open(&self.config, &mut inner);

        match &inner.state {
            CircuitState::Closed | CircuitState::HalfOpen => {
                debug!(state = ?inner.state, "circuit breaker allowing request");
                Ok(())
            }
            CircuitState::Open { until } => {
                let remaining = until.saturating_duration_since(Instant::now());
                warn!(?remaining, "circuit breaker rejecting request");
                Err(CircuitOpen { remaining })
            }
        }
    }

    /// Record a successful operation.
    pub fn record_success(&self) {
        let mut inner = self.inner.lock();
        inner.consecutive_failures = 0;
        inner.consecutive_successes += 1;

        match &inner.state {
            CircuitState::HalfOpen => {
                if inner.consecutive_successes >= self.config.success_threshold {
                    info!(
                        successes = inner.consecutive_successes,
                        "circuit breaker closing after successful probes"
                    );
                    inner.state = CircuitState::Closed;
                    inner.consecutive_successes = 0;
                } else {
                    debug!(
                        successes = inner.consecutive_successes,
                        threshold = self.config.success_threshold,
                        "circuit breaker half-open, accumulating successes"
                    );
                }
            }
            CircuitState::Closed => {
                // Already closed; just reset counters.
                inner.consecutive_successes = 0;
            }
            CircuitState::Open { .. } => {
                // Shouldn't normally happen, but reset if it does.
            }
        }
    }

    /// Record a failed operation.
    pub fn record_failure(&self) {
        let mut inner = self.inner.lock();
        inner.consecutive_successes = 0;
        inner.consecutive_failures += 1;

        match &inner.state {
            CircuitState::Closed => {
                if inner.consecutive_failures >= self.config.failure_threshold {
                    let until = Instant::now() + self.config.open_duration;
                    warn!(
                        failures = inner.consecutive_failures,
                        ?until,
                        "circuit breaker opening"
                    );
                    inner.state = CircuitState::Open { until };
                    inner.consecutive_failures = 0;
                }
            }
            CircuitState::HalfOpen => {
                // Any failure in half-open reopens the circuit.
                let until = Instant::now() + self.config.open_duration;
                warn!("circuit breaker reopening from half-open state");
                inner.state = CircuitState::Open { until };
                inner.consecutive_failures = 0;
                inner.consecutive_successes = 0;
            }
            CircuitState::Open { .. } => {
                // Already open; nothing to do.
            }
        }
    }

    /// Reset the circuit breaker to closed state.
    pub fn reset(&self) {
        let mut inner = self.inner.lock();
        inner.state = CircuitState::Closed;
        inner.consecutive_failures = 0;
        inner.consecutive_successes = 0;
        info!("circuit breaker manually reset to closed");
    }

    /// Check if an open circuit should transition to half-open.
    fn maybe_transition_to_half_open(
        _config: &CircuitBreakerConfig,
        inner: &mut CircuitBreakerInner,
    ) {
        if let CircuitState::Open { until } = &inner.state {
            if Instant::now() >= *until {
                debug!("circuit breaker transitioning to half-open");
                inner.state = CircuitState::HalfOpen;
                inner.consecutive_failures = 0;
                inner.consecutive_successes = 0;
            }
        }
    }

    /// Return the configuration.
    pub fn config(&self) -> &CircuitBreakerConfig {
        &self.config
    }
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock();
        f.debug_struct("CircuitBreaker")
            .field("config", &self.config)
            .field("state", &inner.state)
            .field("consecutive_failures", &inner.consecutive_failures)
            .field("consecutive_successes", &inner.consecutive_successes)
            .finish()
    }
}

/// Serde helper for `Duration` as seconds.
mod duration_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        value.as_secs().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

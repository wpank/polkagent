//! Timeout enforcement for runs.
//!
//! The `TimeoutEnforcer` checks whether a run has exceeded its configured
//! deadline and returns a `DeadlineExceeded` error if it has. It supports:
//!
//! - **Per-run deadlines**: set on the `Run` struct as `deadline: Option<DateTime<Utc>>`.
//! - **Global max duration**: a configurable ceiling applied to all runs
//!   regardless of their per-run deadline.
//!
//! The enforcer does **not** cancel runs itself — it only detects deadline
//! violations. Callers (`RunManager`) are responsible for acting on the result
//! (emitting a `Timeout` transition).

use std::time::Duration;

use chrono::{DateTime, Utc};
use polkagent_core::run::Run;
use tracing::warn;

use crate::error::RunError;

// ---------------------------------------------------------------------------
// TimeoutConfig
// ---------------------------------------------------------------------------

/// Configuration for the `TimeoutEnforcer`.
#[derive(Debug, Clone)]
pub struct TimeoutConfig {
    /// Hard ceiling on run duration applied to every run, regardless of any
    /// per-run `deadline`. If `None`, no global ceiling is enforced.
    pub global_max_duration: Option<Duration>,
}

impl TimeoutConfig {
    /// Create a config with no global max duration (only per-run deadlines are
    /// enforced).
    #[must_use]
    pub fn no_global_limit() -> Self {
        Self {
            global_max_duration: None,
        }
    }

    /// Create a config with a global max duration.
    #[must_use]
    pub fn with_global_max(max: Duration) -> Self {
        Self {
            global_max_duration: Some(max),
        }
    }
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        // Default: 10-minute ceiling on all runs.
        Self {
            global_max_duration: Some(Duration::from_secs(600)),
        }
    }
}

// ---------------------------------------------------------------------------
// TimeoutEnforcer
// ---------------------------------------------------------------------------

/// Enforces per-run and global timeout limits.
///
/// # Usage
///
/// ```no_run
/// use polkagent_run::timeout::{TimeoutEnforcer, TimeoutConfig};
/// use std::time::Duration;
///
/// let enforcer = TimeoutEnforcer::new(TimeoutConfig::with_global_max(Duration::from_secs(300)));
/// // enforcer.check(&run)?; // returns Err if the run has timed out
/// ```
#[derive(Debug, Clone)]
pub struct TimeoutEnforcer {
    config: TimeoutConfig,
}

impl TimeoutEnforcer {
    /// Create a new enforcer with the given configuration.
    #[must_use]
    pub fn new(config: TimeoutConfig) -> Self {
        Self { config }
    }

    /// Check whether `run` has exceeded its deadline or the global max duration.
    ///
    /// Returns `Ok(())` if the run is still within time limits.
    /// Returns `Err(RunError::DeadlineExceeded)` if either:
    /// - The run's `deadline` is `Some(t)` and `now >= t`.
    /// - The `global_max_duration` is set and the run has been executing longer
    ///   than that limit (measured from `started_at`).
    ///
    /// # Errors
    ///
    /// Returns `RunError::DeadlineExceeded` if the run has timed out.
    pub fn check(&self, run: &Run) -> Result<(), RunError> {
        let now = Utc::now();

        // Check per-run deadline.
        if let Some(deadline) = run.deadline {
            if now >= deadline {
                warn!(
                    run_id = %run.id,
                    deadline = %deadline,
                    now = %now,
                    "Run exceeded per-run deadline"
                );
                return Err(RunError::DeadlineExceeded(run.id));
            }
        }

        // Check global max duration.
        if let Some(max_dur) = self.config.global_max_duration {
            if let Some(started_at) = run.started_at {
                let elapsed = now
                    .signed_duration_since(started_at)
                    .to_std()
                    .unwrap_or(Duration::ZERO);

                if elapsed >= max_dur {
                    warn!(
                        run_id = %run.id,
                        elapsed_secs = elapsed.as_secs(),
                        max_secs = max_dur.as_secs(),
                        "Run exceeded global max duration"
                    );
                    return Err(RunError::DeadlineExceeded(run.id));
                }
            }
        }

        Ok(())
    }

    /// Compute the time remaining before either the per-run or global deadline
    /// expires. Returns `None` if neither is configured.
    #[must_use]
    pub fn time_remaining(&self, run: &Run) -> Option<Duration> {
        let now = Utc::now();
        let mut min_remaining: Option<Duration> = None;

        // Per-run deadline.
        if let Some(deadline) = run.deadline {
            let remaining = deadline
                .signed_duration_since(now)
                .to_std()
                .unwrap_or(Duration::ZERO);
            min_remaining = Some(match min_remaining {
                None => remaining,
                Some(prev) => prev.min(remaining),
            });
        }

        // Global max deadline.
        if let (Some(max_dur), Some(started_at)) = (self.config.global_max_duration, run.started_at)
        {
            let elapsed = now
                .signed_duration_since(started_at)
                .to_std()
                .unwrap_or(Duration::ZERO);
            let global_remaining = max_dur.saturating_sub(elapsed);
            min_remaining = Some(match min_remaining {
                None => global_remaining,
                Some(prev) => prev.min(global_remaining),
            });
        }

        min_remaining
    }

    /// Compute the wall-clock deadline for a run given its start time and
    /// the configured global max duration.
    ///
    /// Returns the earlier of:
    /// - The run's explicit `deadline` (if set).
    /// - `started_at + global_max_duration` (if both are set).
    /// - `None` if neither is configured.
    #[must_use]
    pub fn effective_deadline(&self, run: &Run) -> Option<DateTime<Utc>> {
        // Compute the deadline implied by the global max duration, if both
        // the max duration and a start time are available.
        let global = match (self.config.global_max_duration, run.started_at) {
            (Some(max_dur), Some(started_at)) => chrono::Duration::from_std(max_dur)
                .ok()
                .map(|dur| started_at + dur),
            _ => None,
        };

        match (run.deadline, global) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }
}

impl Default for TimeoutEnforcer {
    fn default() -> Self {
        Self::new(TimeoutConfig::default())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::{run::Run, AgentId, RunId};

    fn make_run() -> Run {
        Run::new(RunId::new(), AgentId::new())
    }

    fn enforcer_no_limit() -> TimeoutEnforcer {
        TimeoutEnforcer::new(TimeoutConfig::no_global_limit())
    }

    fn enforcer_with_limit(secs: u64) -> TimeoutEnforcer {
        TimeoutEnforcer::new(TimeoutConfig::with_global_max(Duration::from_secs(secs)))
    }

    // --- No timeout configured ---

    #[test]
    fn run_without_deadline_ok_with_no_limit() {
        let run = make_run();
        let result = enforcer_no_limit().check(&run);
        assert!(result.is_ok());
    }

    // --- Per-run deadline ---

    #[test]
    fn run_within_deadline_is_ok() {
        let mut run = make_run();
        // Deadline is 1 hour from now.
        run.deadline = Some(Utc::now() + chrono::Duration::hours(1));
        let result = enforcer_no_limit().check(&run);
        assert!(result.is_ok());
    }

    #[test]
    fn run_past_deadline_returns_deadline_exceeded() {
        let mut run = make_run();
        // Deadline was 1 second ago.
        run.deadline = Some(Utc::now() - chrono::Duration::seconds(1));
        let result = enforcer_no_limit().check(&run);
        assert!(
            matches!(result, Err(RunError::DeadlineExceeded(_))),
            "expected DeadlineExceeded, got {result:?}"
        );
    }

    // --- Global max duration ---

    #[test]
    fn run_within_global_max_is_ok() {
        let mut run = make_run();
        // Started 5 seconds ago; global max is 60 seconds.
        run.started_at = Some(Utc::now() - chrono::Duration::seconds(5));
        let result = enforcer_with_limit(60).check(&run);
        assert!(result.is_ok());
    }

    #[test]
    fn run_past_global_max_returns_deadline_exceeded() {
        let mut run = make_run();
        // Started 120 seconds ago; global max is 60 seconds.
        run.started_at = Some(Utc::now() - chrono::Duration::seconds(120));
        let result = enforcer_with_limit(60).check(&run);
        assert!(
            matches!(result, Err(RunError::DeadlineExceeded(_))),
            "expected DeadlineExceeded, got {result:?}"
        );
    }

    #[test]
    fn run_without_started_at_skips_global_limit_check() {
        let run = make_run(); // started_at is None
                              // Global max is 60 seconds but no started_at → no timeout
        let result = enforcer_with_limit(60).check(&run);
        assert!(result.is_ok());
    }

    // --- time_remaining ---

    #[test]
    fn time_remaining_none_when_no_limits() {
        let run = make_run();
        let remaining = enforcer_no_limit().time_remaining(&run);
        assert!(remaining.is_none());
    }

    #[test]
    fn time_remaining_reflects_per_run_deadline() {
        let mut run = make_run();
        run.deadline = Some(Utc::now() + chrono::Duration::seconds(30));
        let remaining = enforcer_no_limit().time_remaining(&run);
        // Should be ~30 seconds; allow small timing slack
        assert!(remaining.is_some());
        let r = remaining.unwrap();
        assert!(r.as_secs() <= 30, "remaining={r:?}");
    }

    #[test]
    fn time_remaining_zero_when_deadline_passed() {
        let mut run = make_run();
        run.deadline = Some(Utc::now() - chrono::Duration::seconds(5));
        let remaining = enforcer_no_limit().time_remaining(&run);
        assert_eq!(remaining, Some(Duration::ZERO));
    }

    // --- Error carries run_id ---

    #[test]
    fn deadline_exceeded_error_carries_run_id() {
        let run_id = RunId::new();
        let mut run = Run::new(run_id, AgentId::new());
        run.deadline = Some(Utc::now() - chrono::Duration::seconds(1));
        let err = enforcer_no_limit().check(&run).unwrap_err();
        match err {
            RunError::DeadlineExceeded(id) => assert_eq!(id, run_id),
            other => panic!("unexpected error: {other}"),
        }
    }

    // --- Config defaults ---

    #[test]
    fn default_config_has_600_second_global_max() {
        let config = TimeoutConfig::default();
        assert_eq!(config.global_max_duration, Some(Duration::from_secs(600)));
    }

    #[test]
    fn no_global_limit_config_has_none() {
        let config = TimeoutConfig::no_global_limit();
        assert!(config.global_max_duration.is_none());
    }
}

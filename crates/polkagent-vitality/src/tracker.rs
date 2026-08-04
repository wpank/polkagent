//! Derives affect state from run-level metrics.

use std::collections::VecDeque;
use std::time::Duration;

use chrono::Utc;

use crate::affect::{AffectState, Confidence, Engagement, Fatigue};

/// Rolling window size for metric history.
const WINDOW_CAP: usize = 64;

/// A snapshot of metrics captured at the end of a single run.
#[derive(Debug, Clone)]
pub struct RunSnapshot {
    /// Whether the run completed successfully.
    pub succeeded: bool,
    /// Wall-clock latency of the run.
    pub latency: Duration,
    /// Number of errors encountered during the run.
    pub error_count: u32,
    /// Fraction of the budget consumed by this run (`[0.0, 1.0]`).
    pub budget_used_fraction: f64,
}

/// Tracks recent run metrics and derives an [`AffectState`] projection.
///
/// The tracker maintains a bounded rolling window of [`RunSnapshot`]s and
/// computes engagement, confidence, and fatigue from aggregate statistics.
///
/// This is a pure data projection — it reads metrics but never writes to
/// the run lifecycle, grant system, or policy layer.
#[derive(Debug)]
pub struct VitalityTracker {
    window: VecDeque<RunSnapshot>,
}

impl VitalityTracker {
    pub fn new() -> Self {
        Self {
            window: VecDeque::with_capacity(WINDOW_CAP),
        }
    }

    /// Record a completed run's metrics.
    pub fn record(&mut self, snapshot: RunSnapshot) {
        if self.window.len() >= WINDOW_CAP {
            self.window.pop_front();
        }
        self.window.push_back(snapshot);
    }

    /// Compute the current affect state from the rolling window.
    ///
    /// Returns [`AffectState::default`] if no snapshots have been recorded.
    pub fn current(&self) -> AffectState {
        if self.window.is_empty() {
            return AffectState::default();
        }

        #[allow(clippy::cast_precision_loss)]
        let n = self.window.len() as f64;

        // -- Engagement: derived from budget utilisation momentum.
        // Higher recent budget usage → higher engagement.
        let avg_budget: f64 = self
            .window
            .iter()
            .map(|s| s.budget_used_fraction)
            .sum::<f64>()
            / n;
        let engagement = Engagement::clamped(avg_budget);

        // -- Confidence: derived from success rate.
        #[allow(clippy::cast_precision_loss)]
        let successes = self.window.iter().filter(|s| s.succeeded).count() as f64;
        let confidence = Confidence::clamped(successes / n);

        // -- Fatigue: composite of error frequency, latency, and budget burn.
        // Each factor is normalised to [0, 1] and then averaged.
        let avg_errors: f64 = self
            .window
            .iter()
            .map(|s| f64::from(s.error_count))
            .sum::<f64>()
            / n;
        let error_factor = (avg_errors / 5.0).min(1.0); // 5+ errors/run → saturated

        let avg_latency_ms: f64 = self
            .window
            .iter()
            .map(|s| {
                #[allow(clippy::cast_precision_loss)]
                let ms = s.latency.as_millis() as f64;
                ms
            })
            .sum::<f64>()
            / n;
        let latency_factor = (avg_latency_ms / 10_000.0).min(1.0); // 10s+ → saturated

        let fatigue = Fatigue::clamped((error_factor + latency_factor + avg_budget) / 3.0);

        AffectState {
            engagement,
            confidence,
            fatigue,
            updated_at: Utc::now(),
        }
    }

    /// Number of snapshots currently in the window.
    pub fn len(&self) -> usize {
        self.window.len()
    }

    /// Whether the window is empty.
    pub fn is_empty(&self) -> bool {
        self.window.is_empty()
    }
}

impl Default for VitalityTracker {
    fn default() -> Self {
        Self::new()
    }
}

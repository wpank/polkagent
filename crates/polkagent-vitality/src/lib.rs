//! Experimental affect/vitality state tracking for Polkagent agents.
//!
//! **Maturity: EXPERIMENTAL** — this module is not part of core. Core
//! correctness must never depend on it. It is disabled by default and must
//! be explicitly opted-in via feature flag.
//!
//! Inspired by PRD-09 §10.1 (Roko-style emotional/motivational modeling),
//! this crate tracks three internal affect dimensions derived from run
//! metrics:
//!
//! - **Engagement** — budget utilisation momentum (maps to energy level).
//! - **Confidence** — recent gate-pass / success rate (maps to focus).
//! - **Fatigue** — cost-consumption rate relative to budget, error frequency,
//!   and response latency (maps to stress indicators).
//!
//! These values are **informational only** and must never influence safety
//! decisions, grant resolution, or policy evaluation. They may be surfaced
//! in the TUI cognition panel when the `affect_vitality` feature is active.
//!
//! # Design constraints (PRD-09 §10.4)
//!
//! 1. Affect state is a separate projection — not part of memory or grants.
//! 2. Cannot influence safety, grants, or policy.
//! 3. May hint at task prioritisation / response verbosity within bounds.
//! 4. Disabled by default; clearly labelled experimental.
//! 5. All affect state is visible and inspectable by the user.

pub mod affect;
pub mod error;
pub mod tracker;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use affect::{AffectState, Confidence, Engagement, Fatigue};
pub use error::VitalityError;
pub use tracker::{RunSnapshot, VitalityTracker};

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        reason = "round-trip tests use fail-fast assertions for deterministic serialization fixtures"
    )]

    use super::*;
    use std::time::Duration;

    #[test]
    fn default_affect_state_is_neutral() {
        let state = AffectState::default();
        assert!((state.engagement.0 - 0.5).abs() < f64::EPSILON);
        assert!((state.confidence.0 - 0.5).abs() < f64::EPSILON);
        assert!((state.fatigue.0 - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn tracker_derives_state_from_snapshots() {
        let mut tracker = VitalityTracker::new();

        // Simulate a successful, fast run.
        tracker.record(RunSnapshot {
            succeeded: true,
            latency: Duration::from_millis(200),
            error_count: 0,
            budget_used_fraction: 0.1,
        });

        let state = tracker.current();
        // Success should raise confidence above neutral.
        assert!(state.confidence.0 > 0.5);
        // Low budget usage → low fatigue.
        assert!(state.fatigue.0 < 0.5);
    }

    #[test]
    fn tracker_reflects_errors_as_stress() {
        let mut tracker = VitalityTracker::new();

        for _ in 0..5 {
            tracker.record(RunSnapshot {
                succeeded: false,
                latency: Duration::from_secs(5),
                error_count: 3,
                budget_used_fraction: 0.8,
            });
        }

        let state = tracker.current();
        // Persistent failures should lower confidence.
        assert!(state.confidence.0 < 0.5);
        // High budget use + errors → elevated fatigue.
        assert!(state.fatigue.0 > 0.5);
    }

    #[test]
    fn affect_state_serializes_roundtrip() {
        let state = AffectState {
            engagement: Engagement(0.7),
            confidence: Confidence(0.9),
            fatigue: Fatigue(0.2),
            updated_at: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&state).expect("serialize");
        let deser: AffectState = serde_json::from_str(&json).expect("deserialize");
        assert!((deser.engagement.0 - 0.7).abs() < f64::EPSILON);
        assert!((deser.confidence.0 - 0.9).abs() < f64::EPSILON);
        assert!((deser.fatigue.0 - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn clamped_values_stay_in_range() {
        assert!((Engagement::clamped(1.5).0 - 1.0).abs() < f64::EPSILON);
        assert!((Confidence::clamped(-0.3).0 - 0.0).abs() < f64::EPSILON);
        assert!((Fatigue::clamped(0.6).0 - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn tracker_empty_returns_default() {
        let tracker = VitalityTracker::new();
        let state = tracker.current();
        assert!((state.engagement.0 - 0.5).abs() < f64::EPSILON);
    }
}

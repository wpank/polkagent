//! Affect state types: engagement, confidence, and fatigue.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single affect dimension clamped to `[0.0, 1.0]`.
///
/// - **Engagement** — budget utilisation momentum / energy level.
///   0.0 = idle, 1.0 = fully active.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Engagement(pub f64);

impl Engagement {
    /// Create a value clamped to `[0.0, 1.0]`.
    pub fn clamped(v: f64) -> Self {
        Self(v.clamp(0.0, 1.0))
    }
}

/// - **Confidence** — recent success / gate-pass rate.
///   0.0 = no confidence, 1.0 = full confidence.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Confidence(pub f64);

impl Confidence {
    /// Create a value clamped to `[0.0, 1.0]`.
    pub fn clamped(v: f64) -> Self {
        Self(v.clamp(0.0, 1.0))
    }
}

/// - **Fatigue** — stress indicator derived from cost consumption rate,
///   error frequency, and response latency.
///   0.0 = rested, 1.0 = exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Fatigue(pub f64);

impl Fatigue {
    /// Create a value clamped to `[0.0, 1.0]`.
    pub fn clamped(v: f64) -> Self {
        Self(v.clamp(0.0, 1.0))
    }
}

/// Combined affect state at a point in time.
///
/// This is informational only — it must never influence safety decisions,
/// grant resolution, or policy evaluation (PRD-09 §10.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffectState {
    pub engagement: Engagement,
    pub confidence: Confidence,
    pub fatigue: Fatigue,
    pub updated_at: DateTime<Utc>,
}

impl Default for AffectState {
    fn default() -> Self {
        Self {
            engagement: Engagement(0.5),
            confidence: Confidence(0.5),
            fatigue: Fatigue(0.0),
            updated_at: Utc::now(),
        }
    }
}

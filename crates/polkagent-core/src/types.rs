//! Low-level type aliases and enumerations used across the Polkagent
//! execution model.
//!
//! These types are kept in this module (rather than inline in their first-use
//! site) so that the rest of the workspace can import them from a single,
//! stable location without creating cyclic dependencies.
//!
//! # Canonical definitions for `RetryClass` and `RunState`
//!
//! These types were originally defined here but have been moved to their
//! canonical modules for better code organisation:
//!
//! - [`RetryClass`] is defined in [`crate::effect`] and re-exported here.
//! - [`RunState`] is defined in [`crate::run`] and re-exported here.
//!
//! Callers using `polkagent_core::types::RetryClass` or
//! `polkagent_core::types::RunState` continue to work unchanged.

use serde::{Deserialize, Serialize};

// Re-export canonical definitions for backwards compatibility.
pub use crate::effect::RetryClass;
pub use crate::run::RunState;

// ---------------------------------------------------------------------------
// Timestamp
// ---------------------------------------------------------------------------

/// Wall-clock timestamp with millisecond resolution, serialised as an ISO-8601
/// string. Stored as a UTC `DateTime` from `chrono`.
pub type Timestamp = chrono::DateTime<chrono::Utc>;

/// Return the current UTC timestamp.
#[must_use]
pub fn now() -> Timestamp {
    chrono::Utc::now()
}

// ---------------------------------------------------------------------------
// DurabilityClass
// ---------------------------------------------------------------------------

/// Determines how (and whether) an event is persisted and delivered.
///
/// See the backpressure contract in the event architecture PRD:
/// - [`Durable`]: blocked until persisted; never dropped under backpressure.
/// - [`Diagnostic`]: ring-buffer semantics; oldest entry dropped on overflow.
/// - [`Ephemeral`]: dropped immediately on overflow; never written to storage.
///
/// [`Durable`]: DurabilityClass::Durable
/// [`Diagnostic`]: DurabilityClass::Diagnostic
/// [`Ephemeral`]: DurabilityClass::Ephemeral
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DurabilityClass {
    /// Must survive crash. Persisted synchronously before being broadcast.
    #[default]
    Durable,
    /// Useful for debugging. Persisted asynchronously with a bounded lifetime.
    Diagnostic,
    /// Never persisted. Delivered best-effort over in-process channels only.
    Ephemeral,
}

impl std::fmt::Display for DurabilityClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Durable => write!(f, "durable"),
            Self::Diagnostic => write!(f, "diagnostic"),
            Self::Ephemeral => write!(f, "ephemeral"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- RetryClass (re-exported from crate::effect) ---

    #[test]
    fn retry_class_allows_auto_retry() {
        assert!(RetryClass::Idempotent.allows_auto_retry());
        assert!(RetryClass::CheckBeforeRetry.allows_auto_retry());
    }

    #[test]
    fn no_auto_retry_disallows_retry() {
        assert!(!RetryClass::NoAutoRetry.allows_auto_retry());
    }

    #[test]
    fn retry_class_default_is_idempotent() {
        assert_eq!(RetryClass::default(), RetryClass::Idempotent);
    }

    #[test]
    fn retry_class_serde_round_trip() {
        let c = RetryClass::NoAutoRetry;
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, r#""no_auto_retry""#);
        let back: RetryClass = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    // --- Timestamp ---

    #[test]
    fn now_is_recent() {
        let t = now();
        let delta = chrono::Utc::now() - t;
        assert!(delta.num_seconds() < 2, "now() should be very recent");
    }

    // --- DurabilityClass ---

    #[test]
    fn durability_class_display() {
        assert_eq!(DurabilityClass::Durable.to_string(), "durable");
        assert_eq!(DurabilityClass::Diagnostic.to_string(), "diagnostic");
        assert_eq!(DurabilityClass::Ephemeral.to_string(), "ephemeral");
    }

    #[test]
    fn durability_class_serde_round_trip() {
        let d = DurabilityClass::Diagnostic;
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, r#""diagnostic""#);
        let back: DurabilityClass = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn durability_class_default_is_durable() {
        assert_eq!(DurabilityClass::default(), DurabilityClass::Durable);
    }

    // --- RunState (re-exported from crate::run) ---

    #[test]
    fn run_state_terminal_detection() {
        assert!(RunState::Completed.is_terminal());
        assert!(RunState::Failed { reason: String::new() }.is_terminal());
        assert!(RunState::Cancelled { reason: String::new() }.is_terminal());
        assert!(RunState::TimedOut.is_terminal());
        assert!(!RunState::Created.is_terminal());
        assert!(!RunState::Queued.is_terminal());
        assert!(!RunState::Running.is_terminal());
        assert!(!RunState::AwaitingApproval { request_id: String::new() }.is_terminal());
        assert!(!RunState::Completing.is_terminal());
    }
}

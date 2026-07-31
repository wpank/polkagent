//! Core fault injection registry.
//!
//! [`FaultInjector`] holds a named set of fault points. Components wrap
//! themselves with a `FaultInjector` reference and call [`FaultInjector::check`]
//! at their injection points to decide whether to fire a fault.

use std::collections::HashMap;

use parking_lot::RwLock;

use crate::types::{Fault, FaultPoint, FaultSchedule};

// ---------------------------------------------------------------------------
// FaultInjector
// ---------------------------------------------------------------------------

/// A thread-safe registry of named [`FaultPoint`]s.
///
/// Multiple wrapper types ([`crate::executor::FaultExecutor`],
/// [`crate::signer::FaultSigner`], etc.) can share a single `FaultInjector`
/// (via `Arc<FaultInjector>`) so that a test can coordinate faults across
/// several components simultaneously.
pub struct FaultInjector {
    fault_points: RwLock<HashMap<String, FaultPoint>>,
}

impl std::fmt::Debug for FaultInjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let guard = self.fault_points.read();
        f.debug_struct("FaultInjector")
            .field("fault_points", &guard.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Default for FaultInjector {
    fn default() -> Self {
        Self::new()
    }
}

impl FaultInjector {
    /// Create a new, empty `FaultInjector`.
    pub fn new() -> Self {
        Self { fault_points: RwLock::new(HashMap::new()) }
    }

    /// Register a fault at the named injection point.
    ///
    /// If a fault with the same `name` already exists it is replaced.
    pub fn add_fault(&self, name: &str, fault: Fault, schedule: FaultSchedule) {
        let mut guard = self.fault_points.write();
        guard.insert(name.to_owned(), FaultPoint::new(fault, schedule));
    }

    /// Remove the fault at the named injection point, if any.
    ///
    /// This is a no-op if no fault with `name` is registered.
    pub fn remove_fault(&self, name: &str) {
        let mut guard = self.fault_points.write();
        guard.remove(name);
    }

    /// Check whether a fault fires at the given injection point.
    ///
    /// If a fault is registered at `point_name` and its schedule says it
    /// should fire, returns a reference to the [`Fault`] stored there.
    /// Otherwise returns `None`.
    ///
    /// This is `fn` (not `async`) so callers can use the result in a sync
    /// context before deciding to do async work (e.g. `tokio::time::sleep`).
    pub fn check(&self, point_name: &str) -> Option<Fault> {
        let guard = self.fault_points.read();
        guard.get(point_name).and_then(|fp| {
            if fp.should_fire() {
                Some(fp.fault.clone())
            } else {
                None
            }
        })
    }

    /// Reset the evaluation counters on all registered fault points.
    ///
    /// Useful between test runs to restart schedule policies (e.g. `Once`)
    /// without re-configuring faults.
    pub fn reset_counters(&self) {
        let guard = self.fault_points.read();
        for fp in guard.values() {
            fp.reset();
        }
    }

    /// Return the number of registered fault points.
    pub fn len(&self) -> usize {
        self.fault_points.read().len()
    }

    /// Return `true` if no fault points are registered.
    pub fn is_empty(&self) -> bool {
        self.fault_points.read().is_empty()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FaultSchedule;

    #[test]
    fn new_injector_is_empty() {
        let fi = FaultInjector::new();
        assert!(fi.is_empty());
        assert_eq!(fi.len(), 0);
    }

    #[test]
    fn add_fault_registers_point() {
        let fi = FaultInjector::new();
        fi.add_fault("test_point", Fault::Crash, FaultSchedule::Always);
        assert_eq!(fi.len(), 1);
        assert!(!fi.is_empty());
    }

    #[test]
    fn remove_fault_deregisters_point() {
        let fi = FaultInjector::new();
        fi.add_fault("test_point", Fault::Crash, FaultSchedule::Always);
        fi.remove_fault("test_point");
        assert!(fi.is_empty());
    }

    #[test]
    fn remove_nonexistent_fault_is_noop() {
        let fi = FaultInjector::new();
        fi.remove_fault("does_not_exist"); // should not panic
        assert!(fi.is_empty());
    }

    #[test]
    fn check_returns_some_when_fault_fires() {
        let fi = FaultInjector::new();
        fi.add_fault("p", Fault::Crash, FaultSchedule::Always);
        let result = fi.check("p");
        assert!(matches!(result, Some(Fault::Crash)));
    }

    #[test]
    fn check_returns_none_for_unknown_point() {
        let fi = FaultInjector::new();
        let result = fi.check("nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn check_returns_none_when_schedule_suppresses() {
        let fi = FaultInjector::new();
        // AfterN(5) — first 5 calls return None.
        fi.add_fault("p", Fault::Crash, FaultSchedule::AfterN(5));
        for _ in 0..5 {
            assert!(fi.check("p").is_none());
        }
        // 6th call fires.
        assert!(fi.check("p").is_some());
    }

    #[test]
    fn reset_counters_restarts_once_schedule() {
        let fi = FaultInjector::new();
        fi.add_fault("p", Fault::Crash, FaultSchedule::Once);
        assert!(fi.check("p").is_some()); // fires first time
        assert!(fi.check("p").is_none()); // suppressed
        fi.reset_counters();
        assert!(fi.check("p").is_some()); // fires again after reset
    }

    #[test]
    fn multiple_fault_points_can_be_active_simultaneously() {
        let fi = FaultInjector::new();
        fi.add_fault("point_a", Fault::Crash, FaultSchedule::Always);
        fi.add_fault(
            "point_b",
            Fault::Error { message: "boom".into() },
            FaultSchedule::Always,
        );
        fi.add_fault("point_c", Fault::Crash, FaultSchedule::AfterN(10));

        assert!(fi.check("point_a").is_some());
        assert!(fi.check("point_b").is_some());
        assert!(fi.check("point_c").is_none()); // not yet reached N
        assert_eq!(fi.len(), 3);
    }

    #[test]
    fn removing_a_fault_stops_injection() {
        let fi = FaultInjector::new();
        fi.add_fault("p", Fault::Crash, FaultSchedule::Always);
        assert!(fi.check("p").is_some());
        fi.remove_fault("p");
        // After removal, the point is gone entirely.
        assert!(fi.check("p").is_none());
    }

    #[test]
    fn replacing_fault_replaces_schedule_and_resets_counter() {
        let fi = FaultInjector::new();
        fi.add_fault("p", Fault::Crash, FaultSchedule::Once);
        fi.check("p"); // consume the once
        fi.check("p"); // now suppressed
        // Replace with Always.
        fi.add_fault("p", Fault::Crash, FaultSchedule::Always);
        assert!(fi.check("p").is_some());
    }

    #[test]
    fn default_injector_is_empty() {
        let fi = FaultInjector::default();
        assert!(fi.is_empty());
    }
}

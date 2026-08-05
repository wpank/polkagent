//! Fallback chain for model routing.
//!
//! A [`FallbackChain`] defines a primary model route and an ordered list of
//! fallback alternatives. When the primary provider fails with a
//! [`RetryClass::Fallback`] error, the chain moves to the next entry.
//!
//! **IMPORTANT CONSTRAINT**: Fallback is only permitted *before* any side
//! effect has been committed. Once a tool call, on-chain signature, or chain
//! effect has begun execution, the caller must **not** fall back to an
//! alternate provider for the same step.

use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::retry_class::RetryClass;

/// An opaque identifier for a model route (provider + model pair).
///
/// The actual route resolution happens in the router layer; the retry
/// crate only stores and compares these identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelRoute(pub String);

impl ModelRoute {
    /// Create a new model route identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Return the underlying route identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which error classes should trigger a fallback to this entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FallbackTrigger {
    /// Fall back on any error classified as `Fallback`.
    #[default]
    AnyFallback,
    /// Fall back only for specific provider error types.
    Specific(Vec<String>),
}

/// A single entry in a fallback chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackEntry {
    /// The model route to fall back to.
    pub route: ModelRoute,
    /// Which error classes trigger fallback to this entry.
    #[serde(default)]
    pub trigger_on: FallbackTrigger,
    /// Maximum number of times this fallback can be used in a rolling window.
    /// `None` means unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u32>,
}

impl FallbackEntry {
    /// Create a new fallback entry that triggers on any fallback error.
    pub fn new(route: ModelRoute) -> Self {
        Self {
            route,
            trigger_on: FallbackTrigger::AnyFallback,
            max_uses: None,
        }
    }

    /// Set the maximum number of uses.
    #[must_use]
    pub fn with_max_uses(mut self, max: u32) -> Self {
        self.max_uses = Some(max);
        self
    }

    /// Set the trigger condition.
    #[must_use]
    pub fn with_trigger(mut self, trigger: FallbackTrigger) -> Self {
        self.trigger_on = trigger;
        self
    }
}

/// An ordered list of model routes to try when the primary fails.
///
/// # Safety constraint
///
/// The caller is responsible for ensuring that fallback is only attempted
/// **before** any side effect has been committed. The chain itself does not
/// track side-effect state; it relies on the orchestrator to enforce this
/// invariant.
pub struct FallbackChain {
    /// The primary model route.
    primary: ModelRoute,
    /// Ordered fallback alternatives.
    fallbacks: Vec<FallbackEntry>,
    /// Per-entry usage counters (for `max_uses` enforcement).
    usage_counts: Vec<AtomicU32>,
    /// Whether a side effect has begun (set externally by the orchestrator).
    /// When `true`, `next_route` always returns `None`.
    side_effect_started: std::sync::atomic::AtomicBool,
}

impl FallbackChain {
    /// Create a new fallback chain with the given primary route and fallback entries.
    pub fn new(primary: ModelRoute, fallbacks: Vec<FallbackEntry>) -> Self {
        let usage_counts = fallbacks.iter().map(|_| AtomicU32::new(0)).collect();
        Self {
            primary,
            fallbacks,
            usage_counts,
            side_effect_started: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Return the primary route.
    pub fn primary(&self) -> &ModelRoute {
        &self.primary
    }

    /// Return the fallback entries.
    pub fn fallbacks(&self) -> &[FallbackEntry] {
        &self.fallbacks
    }

    /// Return the number of fallback entries.
    pub fn len(&self) -> usize {
        self.fallbacks.len()
    }

    /// Return `true` if there are no fallback entries.
    pub fn is_empty(&self) -> bool {
        self.fallbacks.is_empty()
    }

    /// Mark that a side effect has begun. After this, `next_route` will
    /// always return `None` -- no fallback is safe.
    pub fn mark_side_effect(&self) {
        self.side_effect_started.store(true, Ordering::Release);
    }

    /// Return whether a side effect has started.
    pub fn side_effect_started(&self) -> bool {
        self.side_effect_started.load(Ordering::Acquire)
    }

    /// Given the current error classification, return the next fallback
    /// route to try, or `None` if no fallback is available.
    ///
    /// Returns `None` if:
    /// - A side effect has started
    /// - The retry class is not `Fallback`
    /// - All fallback entries have been exhausted or exceeded `max_uses`
    ///
    /// `current_index` is 0-based into the fallback list. Pass `0` to get
    /// the first fallback, `1` for the second, etc.
    pub fn next_route(&self, current_index: usize, class: &RetryClass) -> Option<&ModelRoute> {
        // No fallback after side effects
        if self.side_effect_started.load(Ordering::Acquire) {
            return None;
        }

        // Only fall back on Fallback class
        if !class.is_fallback() {
            return None;
        }

        // Find the next eligible fallback
        for i in current_index..self.fallbacks.len() {
            let entry = &self.fallbacks[i];

            // Check max_uses
            if let Some(max) = entry.max_uses {
                let current = self.usage_counts[i].load(Ordering::Acquire);
                if current >= max {
                    continue;
                }
            }

            // Record usage
            self.usage_counts[i].fetch_add(1, Ordering::Release);
            return Some(&entry.route);
        }

        None
    }

    /// Reset all usage counters.
    pub fn reset_usage(&self) {
        for counter in &self.usage_counts {
            counter.store(0, Ordering::Release);
        }
    }

    /// Return the usage count for a specific fallback entry.
    pub fn usage_count(&self, index: usize) -> Option<u32> {
        self.usage_counts
            .get(index)
            .map(|c| c.load(Ordering::Acquire))
    }
}

impl std::fmt::Debug for FallbackChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let usage_counts = self
            .usage_counts
            .iter()
            .map(|count| count.load(Ordering::Acquire))
            .collect::<Vec<_>>();
        f.debug_struct("FallbackChain")
            .field("primary", &self.primary)
            .field("fallbacks", &self.fallbacks)
            .field("usage_counts", &usage_counts)
            .field("side_effect_started", &self.side_effect_started())
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn fallback_chain_returns_primary() {
        let chain = FallbackChain::new(
            ModelRoute::new("primary"),
            vec![FallbackEntry::new(ModelRoute::new("fallback-1"))],
        );
        assert_eq!(chain.primary().as_str(), "primary");
        assert_eq!(chain.len(), 1);
        assert!(!chain.is_empty());
    }

    #[test]
    fn fallback_chain_empty() {
        let chain = FallbackChain::new(ModelRoute::new("primary"), vec![]);
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);
    }

    #[test]
    fn next_route_returns_fallback_on_fallback_class() {
        let chain = FallbackChain::new(
            ModelRoute::new("primary"),
            vec![
                FallbackEntry::new(ModelRoute::new("fb-1")),
                FallbackEntry::new(ModelRoute::new("fb-2")),
            ],
        );

        let class = RetryClass::Fallback;
        let route = chain.next_route(0, &class).expect("should return fb-1");
        assert_eq!(route.as_str(), "fb-1");

        let route = chain.next_route(1, &class).expect("should return fb-2");
        assert_eq!(route.as_str(), "fb-2");
    }

    #[test]
    fn next_route_returns_none_on_retry_immediate() {
        let chain = FallbackChain::new(
            ModelRoute::new("primary"),
            vec![FallbackEntry::new(ModelRoute::new("fb-1"))],
        );
        assert!(chain.next_route(0, &RetryClass::RetryImmediate).is_none());
    }

    #[test]
    fn next_route_returns_none_on_no_retry() {
        let chain = FallbackChain::new(
            ModelRoute::new("primary"),
            vec![FallbackEntry::new(ModelRoute::new("fb-1"))],
        );
        assert!(chain.next_route(0, &RetryClass::NoRetry).is_none());
    }

    #[test]
    fn next_route_returns_none_after_side_effect() {
        let chain = FallbackChain::new(
            ModelRoute::new("primary"),
            vec![FallbackEntry::new(ModelRoute::new("fb-1"))],
        );
        assert!(!chain.side_effect_started());

        chain.mark_side_effect();
        assert!(chain.side_effect_started());

        let class = RetryClass::Fallback;
        assert!(
            chain.next_route(0, &class).is_none(),
            "must not fall back after side effect"
        );
    }

    #[test]
    fn next_route_respects_max_uses() {
        let chain = FallbackChain::new(
            ModelRoute::new("primary"),
            vec![
                FallbackEntry::new(ModelRoute::new("fb-1")).with_max_uses(2),
                FallbackEntry::new(ModelRoute::new("fb-2")),
            ],
        );

        let class = RetryClass::Fallback;

        // Use fb-1 twice (its max)
        assert_eq!(chain.next_route(0, &class).unwrap().as_str(), "fb-1");
        assert_eq!(chain.next_route(0, &class).unwrap().as_str(), "fb-1");

        // Third attempt should skip fb-1 and go to fb-2
        assert_eq!(chain.next_route(0, &class).unwrap().as_str(), "fb-2");
    }

    #[test]
    fn next_route_returns_none_when_all_exhausted() {
        let chain = FallbackChain::new(
            ModelRoute::new("primary"),
            vec![FallbackEntry::new(ModelRoute::new("fb-1")).with_max_uses(1)],
        );

        let class = RetryClass::Fallback;
        assert!(chain.next_route(0, &class).is_some());
        assert!(chain.next_route(0, &class).is_none());
    }

    #[test]
    fn next_route_returns_none_past_end() {
        let chain = FallbackChain::new(
            ModelRoute::new("primary"),
            vec![FallbackEntry::new(ModelRoute::new("fb-1"))],
        );
        assert!(chain.next_route(5, &RetryClass::Fallback).is_none());
    }

    #[test]
    fn reset_usage_clears_counters() {
        let chain = FallbackChain::new(
            ModelRoute::new("primary"),
            vec![FallbackEntry::new(ModelRoute::new("fb-1")).with_max_uses(1)],
        );

        let class = RetryClass::Fallback;
        chain.next_route(0, &class); // uses it
        assert_eq!(chain.usage_count(0), Some(1));

        chain.reset_usage();
        assert_eq!(chain.usage_count(0), Some(0));

        // Should be usable again
        assert!(chain.next_route(0, &class).is_some());
    }

    #[test]
    fn model_route_equality() {
        let a = ModelRoute::new("route-a");
        let b = ModelRoute::new("route-a");
        let c = ModelRoute::new("route-b");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn fallback_entry_serde_roundtrip() {
        let entry = FallbackEntry::new(ModelRoute::new("fb"))
            .with_max_uses(10)
            .with_trigger(FallbackTrigger::AnyFallback);
        let json = serde_json::to_string(&entry).expect("serialize");
        let back: FallbackEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.route.as_str(), "fb");
        assert_eq!(back.max_uses, Some(10));
    }

    #[test]
    fn model_route_serde_roundtrip() {
        let route = ModelRoute::new("anthropic/claude-opus-4-6");
        let json = serde_json::to_string(&route).expect("serialize");
        let back: ModelRoute = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, route);
    }

    #[test]
    fn fallback_chain_debug_shows_fields() {
        let chain = FallbackChain::new(
            ModelRoute::new("primary"),
            vec![FallbackEntry::new(ModelRoute::new("fb-1"))],
        );
        let debug = format!("{chain:?}");
        assert!(debug.contains("primary"));
        assert!(debug.contains("fb-1"));
    }

    #[test]
    fn next_route_returns_none_on_unknown_class() {
        let chain = FallbackChain::new(
            ModelRoute::new("primary"),
            vec![FallbackEntry::new(ModelRoute::new("fb-1"))],
        );
        assert!(chain.next_route(0, &RetryClass::Unknown).is_none());
    }

    #[test]
    fn next_route_returns_none_on_retry_after_class() {
        let chain = FallbackChain::new(
            ModelRoute::new("primary"),
            vec![FallbackEntry::new(ModelRoute::new("fb-1"))],
        );
        assert!(chain
            .next_route(
                0,
                &RetryClass::RetryAfter(std::time::Duration::from_secs(5))
            )
            .is_none());
    }
}

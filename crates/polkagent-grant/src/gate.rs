//! Gate system: independent, deterministic checks that allow, deny, or
//! escalate a proposed action.
//!
//! Each [`Gate`] receives a [`GateRequest`] and returns a [`GateResult`].
//! Gates are composable via [`ComposedGate`]:
//!
//! - `And` — all children must `Allow`; the first non-`Allow` short-circuits.
//! - `Or` — the first `Allow` wins; escalates or denies only if all children
//!   non-`Allow`.
//! - `Sequential` — children evaluated left-to-right; the first `Deny` or
//!   `Escalate` short-circuits.
//!
//! # Built-in gates
//!
//! | Gate | Check |
//! |------|-------|
//! | [`BudgetGate`] | Rejects the request when the cumulative spend would exceed the configured ceiling. |
//! | [`AllowlistGate`] | Permits only addresses/accounts that appear in a static allowlist. |
//! | [`RateLimitGate`] | Token-bucket rate limiter; denies when the bucket is exhausted. |

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::debug;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A request submitted to a gate for evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRequest {
    /// The principal making the request (agent ID, user ID, etc.).
    pub principal: String,

    /// The action being requested (e.g. `"chain/transfer"`).
    pub action: String,

    /// The target resource (e.g. an SS58 address or a tool path).
    pub resource: String,

    /// Amount being spent/transferred in the asset's smallest unit.
    /// `None` means the request has no monetary dimension.
    pub amount: Option<u64>,

    /// Arbitrary key-value metadata for gate-specific checks.
    pub metadata: std::collections::HashMap<String, String>,
}

/// The outcome of a gate evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum GateResult {
    /// The gate permits this request to proceed.
    Allow,
    /// The gate denies this request.
    Deny {
        /// Human-readable explanation of the denial.
        reason: String,
    },
    /// The gate requires the action to be escalated to a human or quorum
    /// approver before proceeding.
    Escalate {
        /// Human-readable explanation of why escalation is needed.
        reason: String,
    },
}

/// A deterministic check applied to a [`GateRequest`].
///
/// Implementations must be `Send + Sync` so that they can be shared across
/// async tasks and stored behind `Arc`.
#[async_trait]
pub trait Gate: Send + Sync {
    /// Evaluate the gate and return a [`GateResult`].
    async fn check(&self, request: &GateRequest) -> GateResult;

    /// A short human-readable label for this gate (used in logs and error
    /// messages).
    fn name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// BudgetGate
// ---------------------------------------------------------------------------

/// Denies any request whose `amount` (or cumulative spend) would exceed a
/// per-principal ceiling.
///
/// This is a lightweight, in-memory gate. For production deployments use
/// [`crate::budget::BudgetTracker`] for persistent, multi-run tracking; this
/// gate is suitable for single-run or integration-test scenarios.
#[derive(Debug)]
pub struct BudgetGate {
    /// Maximum spend per principal across all requests seen by this gate
    /// instance.
    pub max_spend: u64,

    /// Accumulated spend per principal.
    spent: Arc<Mutex<std::collections::HashMap<String, u64>>>,
}

impl BudgetGate {
    /// Create a gate that denies when cumulative spend exceeds `max_spend`.
    #[must_use]
    pub fn new(max_spend: u64) -> Self {
        Self {
            max_spend,
            spent: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }
}

#[async_trait]
impl Gate for BudgetGate {
    async fn check(&self, request: &GateRequest) -> GateResult {
        let amount = match request.amount {
            Some(a) => a,
            // No monetary dimension — this gate does not apply.
            None => return GateResult::Allow,
        };

        let mut spent = self.spent.lock().await;
        let current = spent.entry(request.principal.clone()).or_insert(0);
        let new_total = current.saturating_add(amount);

        if new_total > self.max_spend {
            let remaining = self.max_spend.saturating_sub(*current);
            debug!(
                principal = %request.principal,
                amount,
                remaining,
                max = self.max_spend,
                "budget gate: denied"
            );
            return GateResult::Deny {
                reason: format!(
                    "budget gate: requested {} exceeds remaining budget {} (max {})",
                    amount, remaining, self.max_spend
                ),
            };
        }

        // Tentatively record this spend. The gate is part of a check pipeline
        // and callers are expected to only call `check` when they intend to
        // proceed, so recording here is appropriate.
        *current = new_total;
        debug!(
            principal = %request.principal,
            amount,
            total = new_total,
            "budget gate: allowed"
        );
        GateResult::Allow
    }

    fn name(&self) -> &str {
        "BudgetGate"
    }
}

// ---------------------------------------------------------------------------
// AllowlistGate
// ---------------------------------------------------------------------------

/// Permits only requests whose `resource` (or `principal`, configurable)
/// appears in a static allowlist.
///
/// If the allowlist is empty the gate denies everything (fail-closed).
#[derive(Debug)]
pub struct AllowlistGate {
    /// The set of allowed values.
    allowed: HashSet<String>,

    /// Which field of the request to check.
    pub field: AllowlistField,
}

/// The field of [`GateRequest`] that [`AllowlistGate`] checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowlistField {
    /// Check `request.resource`.
    Resource,
    /// Check `request.principal`.
    Principal,
}

impl AllowlistGate {
    /// Create an allowlist gate checking the given `field` against `allowed`.
    ///
    /// Strings in `allowed` are matched literally (no glob expansion).
    #[must_use]
    pub fn new(field: AllowlistField, allowed: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
            field,
        }
    }
}

#[async_trait]
impl Gate for AllowlistGate {
    async fn check(&self, request: &GateRequest) -> GateResult {
        let value = match self.field {
            AllowlistField::Resource => &request.resource,
            AllowlistField::Principal => &request.principal,
        };

        if self.allowed.contains(value.as_str()) {
            GateResult::Allow
        } else {
            GateResult::Deny {
                reason: format!(
                    "allowlist gate: '{}' is not on the allowlist for field {:?}",
                    value, self.field
                ),
            }
        }
    }

    fn name(&self) -> &str {
        "AllowlistGate"
    }
}

// ---------------------------------------------------------------------------
// RateLimitGate
// ---------------------------------------------------------------------------

/// Token-bucket rate limiter.
///
/// Starts with `capacity` tokens. Each request consumes one token. Tokens
/// refill at `refill_rate` per `refill_interval`. When the bucket is empty
/// the gate returns [`GateResult::Deny`].
#[derive(Debug)]
pub struct RateLimitGate {
    /// Maximum number of tokens (= maximum burst).
    pub capacity: u64,

    /// Number of tokens added per refill.
    pub refill_rate: u64,

    /// How often tokens are added.
    pub refill_interval: Duration,

    /// Per-principal bucket state.
    buckets: Arc<Mutex<std::collections::HashMap<String, TokenBucket>>>,
}

#[derive(Debug)]
struct TokenBucket {
    tokens: u64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(capacity: u64) -> Self {
        Self {
            tokens: capacity,
            last_refill: Instant::now(),
        }
    }

    /// Refill tokens based on elapsed time, then consume one token.
    /// Returns `true` if a token was successfully consumed.
    fn try_consume(&mut self, capacity: u64, refill_rate: u64, refill_interval: Duration) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill);

        if elapsed >= refill_interval {
            let periods = elapsed.as_secs_f64() / refill_interval.as_secs_f64();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let new_tokens = (periods as u64).saturating_mul(refill_rate);
            self.tokens = self.tokens.saturating_add(new_tokens).min(capacity);
            self.last_refill = now;
        }

        if self.tokens > 0 {
            self.tokens -= 1;
            true
        } else {
            false
        }
    }
}

impl RateLimitGate {
    /// Create a rate limit gate with the given parameters.
    #[must_use]
    pub fn new(capacity: u64, refill_rate: u64, refill_interval: Duration) -> Self {
        Self {
            capacity,
            refill_rate,
            refill_interval,
            buckets: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Create a simple per-second rate limiter.
    ///
    /// `requests_per_second` tokens are granted per second with a burst of
    /// the same size.
    #[must_use]
    pub fn per_second(requests_per_second: u64) -> Self {
        Self::new(
            requests_per_second,
            requests_per_second,
            Duration::from_secs(1),
        )
    }
}

#[async_trait]
impl Gate for RateLimitGate {
    async fn check(&self, request: &GateRequest) -> GateResult {
        let mut buckets = self.buckets.lock().await;
        let bucket = buckets
            .entry(request.principal.clone())
            .or_insert_with(|| TokenBucket::new(self.capacity));

        if bucket.try_consume(self.capacity, self.refill_rate, self.refill_interval) {
            debug!(
                principal = %request.principal,
                tokens_remaining = bucket.tokens,
                "rate limit gate: allowed"
            );
            GateResult::Allow
        } else {
            debug!(
                principal = %request.principal,
                "rate limit gate: denied (bucket empty)"
            );
            GateResult::Deny {
                reason: format!(
                    "rate limit gate: request rate exceeded for principal '{}'",
                    request.principal
                ),
            }
        }
    }

    fn name(&self) -> &str {
        "RateLimitGate"
    }
}

// ---------------------------------------------------------------------------
// ComposedGate
// ---------------------------------------------------------------------------

/// Composite gate that combines multiple child gates with a logical strategy.
pub enum ComposedGate {
    /// All children must return [`GateResult::Allow`].
    ///
    /// The first non-Allow result short-circuits evaluation.
    And(Vec<Box<dyn Gate>>),

    /// The first [`GateResult::Allow`] wins.
    ///
    /// If all children return non-Allow the last result is returned.
    Or(Vec<Box<dyn Gate>>),

    /// Children are evaluated left-to-right.
    ///
    /// The first [`GateResult::Deny`] or [`GateResult::Escalate`] short-
    /// circuits; if all Allow, returns Allow.
    Sequential(Vec<Box<dyn Gate>>),
}

impl std::fmt::Debug for ComposedGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::And(v) => write!(f, "ComposedGate::And({} gates)", v.len()),
            Self::Or(v) => write!(f, "ComposedGate::Or({} gates)", v.len()),
            Self::Sequential(v) => write!(f, "ComposedGate::Sequential({} gates)", v.len()),
        }
    }
}

#[async_trait]
impl Gate for ComposedGate {
    async fn check(&self, request: &GateRequest) -> GateResult {
        match self {
            Self::And(gates) => {
                for gate in gates {
                    let result = gate.check(request).await;
                    if result != GateResult::Allow {
                        debug!(gate = gate.name(), "And gate: child denied/escalated");
                        return result;
                    }
                }
                GateResult::Allow
            }

            Self::Or(gates) => {
                let mut last = GateResult::Deny {
                    reason: "Or gate: no children".to_string(),
                };
                for gate in gates {
                    let result = gate.check(request).await;
                    if result == GateResult::Allow {
                        debug!(gate = gate.name(), "Or gate: child allowed");
                        return GateResult::Allow;
                    }
                    last = result;
                }
                last
            }

            Self::Sequential(gates) => {
                for gate in gates {
                    let result = gate.check(request).await;
                    match &result {
                        GateResult::Allow => {
                            // Continue to next gate.
                        }
                        GateResult::Deny { .. } | GateResult::Escalate { .. } => {
                            debug!(gate = gate.name(), "Sequential gate: short-circuit");
                            return result;
                        }
                    }
                }
                GateResult::Allow
            }
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::And(_) => "ComposedGate::And",
            Self::Or(_) => "ComposedGate::Or",
            Self::Sequential(_) => "ComposedGate::Sequential",
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn req(principal: &str, resource: &str, amount: Option<u64>) -> GateRequest {
        GateRequest {
            principal: principal.to_string(),
            action: "chain/transfer".to_string(),
            resource: resource.to_string(),
            amount,
            metadata: Default::default(),
        }
    }

    // ---- BudgetGate --------------------------------------------------------

    #[tokio::test]
    async fn budget_gate_allows_within_limit() {
        let gate = BudgetGate::new(1000);
        let r = gate.check(&req("alice", "account/bob", Some(500))).await;
        assert_eq!(r, GateResult::Allow);
    }

    #[tokio::test]
    async fn budget_gate_denies_over_limit() {
        let gate = BudgetGate::new(100);
        let r = gate.check(&req("alice", "account/bob", Some(200))).await;
        assert!(
            matches!(r, GateResult::Deny { .. }),
            "should deny over limit"
        );
    }

    #[tokio::test]
    async fn budget_gate_accumulates_across_calls() {
        let gate = BudgetGate::new(100);
        // First spend: 60, second: 60 — cumulative 120 > 100.
        gate.check(&req("alice", "account/bob", Some(60))).await;
        let r = gate.check(&req("alice", "account/bob", Some(60))).await;
        assert!(
            matches!(r, GateResult::Deny { .. }),
            "cumulative spend should exceed budget"
        );
    }

    #[tokio::test]
    async fn budget_gate_allows_no_amount() {
        let gate = BudgetGate::new(0);
        // No monetary dimension; the gate should not interfere.
        let r = gate.check(&req("alice", "account/bob", None)).await;
        assert_eq!(r, GateResult::Allow);
    }

    // ---- AllowlistGate -----------------------------------------------------

    #[tokio::test]
    async fn allowlist_gate_permits_listed_resource() {
        let gate = AllowlistGate::new(
            AllowlistField::Resource,
            vec!["5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string()],
        );
        let r = gate
            .check(&req(
                "alice",
                "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
                None,
            ))
            .await;
        assert_eq!(r, GateResult::Allow);
    }

    #[tokio::test]
    async fn allowlist_gate_denies_unlisted_resource() {
        let gate = AllowlistGate::new(AllowlistField::Resource, vec!["known_address".to_string()]);
        let r = gate.check(&req("alice", "unknown_address", None)).await;
        assert!(matches!(r, GateResult::Deny { .. }));
    }

    #[tokio::test]
    async fn allowlist_gate_empty_list_denies_all() {
        let gate = AllowlistGate::new(AllowlistField::Resource, std::iter::empty::<String>());
        let r = gate.check(&req("alice", "any_resource", None)).await;
        assert!(matches!(r, GateResult::Deny { .. }));
    }

    // ---- RateLimitGate -----------------------------------------------------

    #[tokio::test]
    async fn rate_limit_gate_allows_within_capacity() {
        let gate = RateLimitGate::new(3, 3, Duration::from_secs(60));
        for _ in 0..3 {
            let r = gate.check(&req("alice", "res", None)).await;
            assert_eq!(r, GateResult::Allow);
        }
    }

    #[tokio::test]
    async fn rate_limit_gate_denies_when_exhausted() {
        let gate = RateLimitGate::new(2, 2, Duration::from_secs(60));
        gate.check(&req("alice", "res", None)).await; // 1
        gate.check(&req("alice", "res", None)).await; // 2
        let r = gate.check(&req("alice", "res", None)).await; // 3rd — exhausted
        assert!(
            matches!(r, GateResult::Deny { .. }),
            "rate limit should deny after capacity exhausted"
        );
    }

    #[tokio::test]
    async fn rate_limit_gate_different_principals_independent() {
        let gate = RateLimitGate::new(1, 1, Duration::from_secs(60));
        let r_alice = gate.check(&req("alice", "res", None)).await;
        let r_bob = gate.check(&req("bob", "res", None)).await;
        assert_eq!(r_alice, GateResult::Allow);
        assert_eq!(r_bob, GateResult::Allow);
    }

    // ---- ComposedGate::And -------------------------------------------------

    #[tokio::test]
    async fn and_gate_all_allow() {
        let gate = ComposedGate::And(vec![
            Box::new(AllowlistGate::new(
                AllowlistField::Resource,
                vec!["res".to_string()],
            )),
            Box::new(BudgetGate::new(1000)),
        ]);
        let r = gate.check(&req("alice", "res", Some(100))).await;
        assert_eq!(r, GateResult::Allow);
    }

    #[tokio::test]
    async fn and_gate_short_circuits_on_deny() {
        let gate = ComposedGate::And(vec![
            Box::new(AllowlistGate::new(
                AllowlistField::Resource,
                vec![], // denies everything
            )),
            Box::new(BudgetGate::new(0)), // would also deny
        ]);
        let r = gate.check(&req("alice", "res", None)).await;
        assert!(matches!(r, GateResult::Deny { .. }));
    }

    // ---- ComposedGate::Or --------------------------------------------------

    #[tokio::test]
    async fn or_gate_first_allow_wins() {
        let gate = ComposedGate::Or(vec![
            Box::new(AllowlistGate::new(
                AllowlistField::Resource,
                vec!["res".to_string()],
            )),
            Box::new(BudgetGate::new(0)), // would deny
        ]);
        let r = gate.check(&req("alice", "res", None)).await;
        assert_eq!(r, GateResult::Allow);
    }

    #[tokio::test]
    async fn or_gate_all_deny_returns_last_deny() {
        let gate = ComposedGate::Or(vec![
            Box::new(AllowlistGate::new(AllowlistField::Resource, vec![])),
            Box::new(AllowlistGate::new(AllowlistField::Principal, vec![])),
        ]);
        let r = gate.check(&req("alice", "res", None)).await;
        assert!(matches!(r, GateResult::Deny { .. }));
    }

    // ---- ComposedGate::Sequential ------------------------------------------

    #[tokio::test]
    async fn sequential_gate_all_allow() {
        let gate = ComposedGate::Sequential(vec![
            Box::new(AllowlistGate::new(
                AllowlistField::Principal,
                vec!["alice".to_string()],
            )),
            Box::new(AllowlistGate::new(
                AllowlistField::Resource,
                vec!["res".to_string()],
            )),
        ]);
        let r = gate.check(&req("alice", "res", None)).await;
        assert_eq!(r, GateResult::Allow);
    }

    #[tokio::test]
    async fn sequential_gate_short_circuits_on_first_deny() {
        let gate = ComposedGate::Sequential(vec![
            Box::new(AllowlistGate::new(AllowlistField::Principal, vec![])), // deny
            Box::new(AllowlistGate::new(
                AllowlistField::Resource,
                vec!["res".to_string()],
            )), // would allow
        ]);
        let r = gate.check(&req("alice", "res", None)).await;
        assert!(matches!(r, GateResult::Deny { .. }));
    }

    // ---- Nested composition ------------------------------------------------

    #[tokio::test]
    async fn nested_and_inside_or() {
        // Or( And(deny, allow), allow_all ) → the And fails, but the Or's
        // second child allows.
        let inner_and = ComposedGate::And(vec![
            Box::new(AllowlistGate::new(AllowlistField::Resource, vec![])), // deny
            Box::new(AllowlistGate::new(
                AllowlistField::Resource,
                vec!["res".to_string()],
            )),
        ]);
        let gate = ComposedGate::Or(vec![
            Box::new(inner_and),
            Box::new(AllowlistGate::new(
                AllowlistField::Resource,
                vec!["res".to_string()],
            )),
        ]);
        let r = gate.check(&req("alice", "res", None)).await;
        assert_eq!(r, GateResult::Allow);
    }
}

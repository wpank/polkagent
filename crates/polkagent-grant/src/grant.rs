//! Grant resolution: maps a `(principal, action, resource, context)` tuple to
//! an authorization decision.
//!
//! # Algorithm (Phase 1)
//!
//! ```text
//! 1. Check context freshness — stale context is denied.
//! 2. Evaluate the policy set via deny-overrides rules.
//! 3. Check active (non-expired) grants for the principal.
//! 4. Check the budget gate.
//! 5. Return a GrantDecision.
//! ```
//!
//! The [`GrantResolver`] is the entry-point. It holds a [`PolicySet`], a
//! collection of active [`ActiveGrant`]s, and an optional [`BudgetGate`].
//!
//! # Default deny
//!
//! When the policy set contains no matching rules the resolver returns
//! [`GrantDecision::Deny`] consistent with INV-POLICY-03.
//!
//! # Time-bounded grants
//!
//! Active grants carry an `expires_at` field. Any grant past its expiry is
//! treated as absent (REQ-POLICY-02: "Expired grants must be denied
//! immediately; no grace period.").

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use polkagent_core::ids::{AgentId, GrantId, RunId};

use crate::budget::{BudgetError, BudgetTracker};
use crate::gate::{Gate, GateRequest, GateResult};
use crate::policy::{self, EvaluationContext, PolicyDecision, PolicySet};

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// The set of effects that a [`ResolvedGrant`] permits.
///
/// Effect names follow the dot-separated convention used in the PRD
/// (e.g. `"chain.transfer"`, `"model.inference"`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EffectSet {
    pub effects: Vec<String>,
}

impl EffectSet {
    pub fn new(effects: impl IntoIterator<Item = String>) -> Self {
        Self {
            effects: effects.into_iter().collect(),
        }
    }

    pub fn contains(&self, effect: &str) -> bool {
        self.effects.iter().any(|e| e == effect)
    }
}

/// Quantitative limits attached to a resolved grant.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GrantLimits {
    /// Maximum number of requests allowed under this grant.
    pub max_requests: Option<u64>,
    /// Maximum total spend in the asset's smallest unit.
    pub max_spend: Option<u64>,
    /// Hard deadline by which the granted operation must complete.
    pub deadline: Option<DateTime<Utc>>,
}

/// An immutable, time-bounded authorization for a specific operation.
///
/// Corresponds to `ResolvedGrant` in PRD-07 §8.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedGrant {
    /// Stable identifier for this grant instance.
    pub grant_id: GrantId,

    /// Which principal holds this grant.
    pub principal: String,

    /// The action this grant covers.
    pub action: String,

    /// The resource selector this grant covers.
    pub resource: String,

    /// Effects the principal may perform under this grant.
    pub allowed_effects: EffectSet,

    /// Quantitative limits.
    pub limits: GrantLimits,

    /// When this grant expires. Access after this time is denied.
    pub expires_at: DateTime<Utc>,
}

impl ResolvedGrant {
    /// Returns `true` if the grant has not yet expired.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        Utc::now() < self.expires_at
    }
}

/// A record of why a policy denied the request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDenial {
    /// Human-readable explanation.
    pub reason: String,
    /// The stage that produced the denial.
    pub stage: String,
}

/// The reason an approval is required before the request may proceed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequirement {
    pub reason: String,
}

/// The reason grant resolution was deferred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeferReason {
    pub reason: String,
}

/// The outcome of grant resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum GrantDecision {
    /// The request is permitted and a resolved grant has been produced.
    Permit(ResolvedGrant),

    /// The request is denied by policy.
    Deny(PolicyDenial),

    /// The request requires explicit human or quorum approval.
    RequireApproval(ApprovalRequirement),

    /// Resolution must be deferred (e.g. policy service unreachable).
    Defer(DeferReason),
}

/// An active (pre-issued) grant stored in the resolver's grant registry.
///
/// When a resolver has a matching active grant the policy evaluation may be
/// satisfied without producing a new [`ResolvedGrant`] — the existing grant is
/// re-used (with its original expiry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveGrant {
    pub grant_id: GrantId,
    pub principal: String,
    /// Glob pattern for actions this grant covers.
    pub action_pattern: String,
    /// Glob pattern for resources this grant covers.
    pub resource_pattern: String,
    pub allowed_effects: EffectSet,
    pub limits: GrantLimits,
    pub expires_at: DateTime<Utc>,
}

impl ActiveGrant {
    /// Returns `true` if this grant applies to `(principal, action, resource)`
    /// and has not expired.
    #[must_use]
    pub fn applies_to(&self, principal: &str, action: &str, resource: &str) -> bool {
        if self.principal != principal {
            return false;
        }
        if Utc::now() >= self.expires_at {
            return false;
        }
        policy::pattern_matches(&self.action_pattern, action)
            && policy::pattern_matches(&self.resource_pattern, resource)
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur during grant resolution.
#[derive(Debug, Error)]
pub enum ResolverError {
    #[error("budget error during grant resolution: {0}")]
    Budget(#[from] BudgetError),

    #[error("context freshness check failed: {0}")]
    StaleContext(String),
}

// ---------------------------------------------------------------------------
// GrantResolver
// ---------------------------------------------------------------------------

/// Configuration for the [`GrantResolver`].
#[derive(Debug, Clone)]
pub struct ResolverConfig {
    /// Maximum age of the evaluation context before it is considered stale
    /// and the request is denied. `None` disables the freshness check.
    pub max_context_age: Option<Duration>,

    /// Default expiry duration applied to grants produced by the resolver.
    pub default_grant_ttl: Duration,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            max_context_age: Some(Duration::minutes(5)),
            default_grant_ttl: Duration::hours(1),
        }
    }
}

/// The grant resolver.
///
/// Holds a [`PolicySet`], an in-memory registry of pre-issued [`ActiveGrant`]s,
/// and an optional [`BudgetTracker`] for budget enforcement.
///
/// Wrap in `Arc<GrantResolver>` to share across async tasks.
pub struct GrantResolver {
    policy_set: RwLock<PolicySet>,
    active_grants: RwLock<Vec<ActiveGrant>>,
    budget_tracker: Option<Arc<BudgetTracker>>,
    extra_gates: Vec<Box<dyn Gate>>,
    config: ResolverConfig,
}

impl GrantResolver {
    /// Create a resolver with the given policy set and configuration.
    #[must_use]
    pub fn new(policy_set: PolicySet, config: ResolverConfig) -> Arc<Self> {
        Arc::new(Self {
            policy_set: RwLock::new(policy_set),
            active_grants: RwLock::new(Vec::new()),
            budget_tracker: None,
            extra_gates: Vec::new(),
            config,
        })
    }

    /// Attach a [`BudgetTracker`] that will be consulted during resolution.
    pub async fn with_budget_tracker(self: Arc<Self>, tracker: Arc<BudgetTracker>) -> Arc<Self> {
        // Safety: we hold the only `Arc` to a freshly created resolver at
        // builder-call time, so this is safe. In production, call this before
        // sharing the `Arc`.
        //
        // We use `Arc::into_inner` / reconstruct if we need mutation after
        // creation; for the builder pattern it's simpler to rebuild.
        Arc::new(Self {
            policy_set: RwLock::new(self.policy_set.read().await.clone()),
            active_grants: RwLock::new(self.active_grants.read().await.clone()),
            budget_tracker: Some(tracker),
            extra_gates: Vec::new(),
            config: self.config.clone(),
        })
    }

    /// Register a pre-issued [`ActiveGrant`] in this resolver.
    pub async fn add_active_grant(&self, grant: ActiveGrant) {
        let mut grants = self.active_grants.write().await;
        grants.push(grant);
        debug!(total = grants.len(), "active grant added");
    }

    /// Replace the current policy set.
    pub async fn update_policy(&self, new_set: PolicySet) {
        *self.policy_set.write().await = new_set;
        debug!("policy set updated");
    }

    /// Resolve authorization for `(principal, action, resource, context)`.
    ///
    /// # Algorithm
    ///
    /// 1. **Context freshness.** If the context carries an `evaluated_at`
    ///    timestamp and `max_context_age` is configured, stale contexts are
    ///    denied.
    /// 2. **Policy evaluation.** Deny-overrides rule evaluation.
    /// 3. **Active grant check.** If a matching, non-expired active grant
    ///    exists, skip to step 5.
    /// 4. **Synthesize grant.** Build a [`ResolvedGrant`] from the policy
    ///    decision.
    /// 5. **Budget check.** If a [`BudgetTracker`] is attached and the request
    ///    carries an amount, verify remaining budget.
    /// 6. Return the [`GrantDecision`].
    ///
    /// # Errors
    ///
    /// Returns [`ResolverError`] only for infrastructure-level failures (e.g.
    /// budget tracker I/O). Policy denials are returned as
    /// [`GrantDecision::Deny`], not as errors.
    pub async fn resolve(
        &self,
        principal: &str,
        action: &str,
        resource: &str,
        ctx: &EvaluationContext,
        amount: Option<u64>,
        run_id: Option<RunId>,
    ) -> Result<GrantDecision, ResolverError> {
        // Step 1: Context freshness.
        if let (Some(max_age), Some(evaluated_at_str)) =
            (self.config.max_context_age, &ctx.evaluated_at)
        {
            match DateTime::parse_from_rfc3339(evaluated_at_str) {
                Ok(evaluated_at) => {
                    let age = Utc::now() - evaluated_at.with_timezone(&Utc);
                    if age > max_age {
                        warn!(
                            age_secs = age.num_seconds(),
                            max_secs = max_age.num_seconds(),
                            "context is stale"
                        );
                        return Ok(GrantDecision::Deny(PolicyDenial {
                            reason: format!(
                                "context is stale: age {}s exceeds max {}s",
                                age.num_seconds(),
                                max_age.num_seconds()
                            ),
                            stage: "context_freshness".to_string(),
                        }));
                    }
                }
                Err(e) => {
                    return Err(ResolverError::StaleContext(format!(
                        "invalid evaluated_at timestamp: {e}"
                    )));
                }
            }
        }

        // Step 2: Policy evaluation.
        let policy_decision = {
            let policy_set = self.policy_set.read().await;
            policy::evaluate(&policy_set, action, resource, ctx)
        };

        match &policy_decision {
            PolicyDecision::Deny { reason } => {
                debug!(action, resource, %reason, "policy denied");
                return Ok(GrantDecision::Deny(PolicyDenial {
                    reason: reason.clone(),
                    stage: "policy_evaluation".to_string(),
                }));
            }
            PolicyDecision::RequireApproval { reason } => {
                debug!(action, resource, %reason, "policy requires approval");
                return Ok(GrantDecision::RequireApproval(ApprovalRequirement {
                    reason: reason.clone(),
                }));
            }
            PolicyDecision::Allow => {
                debug!(action, resource, "policy allowed");
            }
        }

        // Step 3: Check active grants.
        let active_grant = {
            let grants = self.active_grants.read().await;
            grants
                .iter()
                .find(|g| g.applies_to(principal, action, resource))
                .cloned()
        };

        // Step 4: Produce a ResolvedGrant.
        let resolved = if let Some(ag) = active_grant {
            debug!(grant_id = %ag.grant_id, "using existing active grant");
            ResolvedGrant {
                grant_id: ag.grant_id,
                principal: principal.to_string(),
                action: action.to_string(),
                resource: resource.to_string(),
                allowed_effects: ag.allowed_effects,
                limits: ag.limits,
                expires_at: ag.expires_at,
            }
        } else {
            let expires_at = Utc::now() + self.config.default_grant_ttl;
            let grant_id = GrantId::new();
            debug!(%grant_id, "synthesized new resolved grant");
            ResolvedGrant {
                grant_id,
                principal: principal.to_string(),
                action: action.to_string(),
                resource: resource.to_string(),
                allowed_effects: EffectSet::new(vec![action.to_string()]),
                limits: GrantLimits::default(),
                expires_at,
            }
        };

        // Step 5: Budget check.
        if let Some(amount) = amount {
            if let Some(tracker) = &self.budget_tracker {
                // Derive an AgentId from the principal string. If the
                // principal is already a UUID it round-trips directly;
                // otherwise a deterministic blake3-based ID is produced.
                let agent_id = principal_to_agent_id(principal);
                let within_budget = tracker.check_budget(agent_id, amount).await?;
                if !within_budget {
                    return Ok(GrantDecision::Deny(PolicyDenial {
                        reason: format!(
                            "budget gate: requested {} exceeds remaining budget for agent '{}'",
                            amount, principal
                        ),
                        stage: "budget_check".to_string(),
                    }));
                }

                // Record the spend.
                if let Some(rid) = run_id {
                    tracker.record_spend(agent_id, rid, amount).await?;
                }
            }
        }

        // Step 6: Extra gates.
        for gate in &self.extra_gates {
            let gate_req = GateRequest {
                principal: principal.to_string(),
                action: action.to_string(),
                resource: resource.to_string(),
                amount,
                metadata: ctx.attributes.clone(),
            };
            match gate.check(&gate_req).await {
                GateResult::Allow => {}
                GateResult::Deny { reason } => {
                    return Ok(GrantDecision::Deny(PolicyDenial {
                        reason,
                        stage: format!("gate:{}", gate.name()),
                    }));
                }
                GateResult::Escalate { reason } => {
                    return Ok(GrantDecision::RequireApproval(ApprovalRequirement {
                        reason,
                    }));
                }
            }
        }

        Ok(GrantDecision::Permit(resolved))
    }
}

/// Map a string principal name to a deterministic [`AgentId`] for budget
/// lookups.
///
/// Two code paths:
///
/// 1. **Typed `AgentId`**: If the caller passes a UUID string (e.g.
///    `agent_id.to_string()`), it is parsed back into an `AgentId` directly.
///    This is the common path when the caller already has a typed ID.
///
/// 2. **Opaque principal string**: When the caller only has a human-readable
///    principal name (e.g. `"alice"`) — such as from external policy
///    descriptors — we derive a deterministic UUID v5-style identifier via
///    blake3 hashing.  This ensures a stable, collision-resistant mapping
///    from arbitrary principal strings to `AgentId` values suitable for
///    budget tracking.
fn principal_to_agent_id(principal: &str) -> AgentId {
    use std::str::FromStr;
    // Fast path: the principal is already a valid UUID (typed AgentId).
    if let Ok(id) = AgentId::from_str(principal) {
        return id;
    }
    // Deterministic derivation for opaque principal strings.
    // blake3 produces 32 bytes; we take the first 16 and set the UUID
    // version/variant bits to produce a valid v5 UUID.
    let hash = blake3::hash(principal.as_bytes());
    let bytes = hash.as_bytes();
    let mut uuid_bytes = [0u8; 16];
    uuid_bytes.copy_from_slice(&bytes[..16]);
    // Force UUID version 5 bits.
    uuid_bytes[6] = (uuid_bytes[6] & 0x0f) | 0x50;
    uuid_bytes[8] = (uuid_bytes[8] & 0x3f) | 0x80;
    AgentId::from_uuid(uuid::Uuid::from_bytes(uuid_bytes))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{Effect, PolicyRule};

    fn allow_rule(id: &str, actions: &[&str], resources: &[&str]) -> PolicyRule {
        PolicyRule {
            id: id.to_string(),
            effect: Effect::Allow,
            action_patterns: actions.iter().map(|s| s.to_string()).collect(),
            resource_patterns: resources.iter().map(|s| s.to_string()).collect(),
            conditions: Default::default(),
            abac_condition: None,
        }
    }

    fn deny_rule(id: &str, actions: &[&str], resources: &[&str]) -> PolicyRule {
        PolicyRule {
            id: id.to_string(),
            effect: Effect::Deny,
            action_patterns: actions.iter().map(|s| s.to_string()).collect(),
            resource_patterns: resources.iter().map(|s| s.to_string()).collect(),
            conditions: Default::default(),
            abac_condition: None,
        }
    }

    fn empty_ctx() -> EvaluationContext {
        EvaluationContext::default()
    }

    // ---- Default deny -------------------------------------------------------

    #[tokio::test]
    async fn default_deny_no_rules() {
        let resolver = GrantResolver::new(PolicySet::default(), ResolverConfig::default());
        let decision = resolver
            .resolve(
                "alice",
                "chain/transfer",
                "account/bob",
                &empty_ctx(),
                None,
                None,
            )
            .await
            .unwrap();
        assert!(
            matches!(decision, GrantDecision::Deny(_)),
            "empty policy must default-deny"
        );
    }

    // ---- Allow / deny pipeline ----------------------------------------------

    #[tokio::test]
    async fn allow_rule_produces_permit() {
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("r1", &["chain/**"], &["account/**"]));

        let resolver = GrantResolver::new(set, ResolverConfig::default());
        let decision = resolver
            .resolve(
                "alice",
                "chain/transfer",
                "account/bob",
                &empty_ctx(),
                None,
                None,
            )
            .await
            .unwrap();
        assert!(
            matches!(decision, GrantDecision::Permit(_)),
            "allow rule should produce Permit"
        );
    }

    #[tokio::test]
    async fn deny_overrides_allow_in_grant_resolver() {
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("allow-all", &["**"], &["**"]));
        set.add_rule(deny_rule("deny-transfer", &["chain/transfer"], &["**"]));

        let resolver = GrantResolver::new(set, ResolverConfig::default());
        let decision = resolver
            .resolve(
                "alice",
                "chain/transfer",
                "account/bob",
                &empty_ctx(),
                None,
                None,
            )
            .await
            .unwrap();
        assert!(
            matches!(decision, GrantDecision::Deny(_)),
            "deny rule must override allow via resolver"
        );
    }

    // ---- Active grants -------------------------------------------------------

    #[tokio::test]
    async fn active_grant_used_when_policy_allows() {
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("r1", &["chain/**"], &["**"]));

        let resolver = GrantResolver::new(set, ResolverConfig::default());

        let pre_grant_id = GrantId::new();
        resolver
            .add_active_grant(ActiveGrant {
                grant_id: pre_grant_id,
                principal: "alice".to_string(),
                action_pattern: "chain/**".to_string(),
                resource_pattern: "**".to_string(),
                allowed_effects: EffectSet::new(vec!["chain.transfer".to_string()]),
                limits: GrantLimits::default(),
                expires_at: Utc::now() + Duration::hours(1),
            })
            .await;

        let decision = resolver
            .resolve(
                "alice",
                "chain/transfer",
                "account/bob",
                &empty_ctx(),
                None,
                None,
            )
            .await
            .unwrap();

        if let GrantDecision::Permit(grant) = decision {
            assert_eq!(
                grant.grant_id, pre_grant_id,
                "should re-use the pre-issued active grant"
            );
        } else {
            panic!("expected Permit, got {decision:?}");
        }
    }

    #[tokio::test]
    async fn expired_active_grant_ignored() {
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("r1", &["chain/**"], &["**"]));

        let resolver = GrantResolver::new(set, ResolverConfig::default());

        // Insert a grant that expired in the past.
        resolver
            .add_active_grant(ActiveGrant {
                grant_id: GrantId::new(),
                principal: "alice".to_string(),
                action_pattern: "chain/**".to_string(),
                resource_pattern: "**".to_string(),
                allowed_effects: EffectSet::new(vec!["chain.transfer".to_string()]),
                limits: GrantLimits::default(),
                expires_at: Utc::now() - Duration::hours(1), // already expired
            })
            .await;

        let decision = resolver
            .resolve(
                "alice",
                "chain/transfer",
                "account/bob",
                &empty_ctx(),
                None,
                None,
            )
            .await
            .unwrap();

        // Policy allows, so we should get a freshly synthesized grant, not the
        // expired one.
        if let GrantDecision::Permit(grant) = &decision {
            assert!(
                grant.expires_at > Utc::now(),
                "synthesized grant must have future expiry"
            );
        } else {
            panic!("expected Permit after expired grant ignored, got {decision:?}");
        }
    }

    // ---- Budget integration -------------------------------------------------

    #[tokio::test]
    async fn budget_check_blocks_over_limit_spend() {
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("r1", &["**"], &["**"]));

        let config = ResolverConfig {
            max_context_age: None,
            default_grant_ttl: Duration::hours(1),
        };
        let resolver = GrantResolver::new(set, config);

        let tracker = BudgetTracker::new();
        let agent_id = AgentId::new();
        tracker.configure(agent_id, 100).await;

        let resolver = resolver.with_budget_tracker(Arc::clone(&tracker)).await;

        // Resolve using the agent_id string as the principal so
        // principal_to_agent_id round-trips correctly.
        let principal = agent_id.to_string();
        let run_id = RunId::new();

        // Spend 101 — exceeds the 100 ceiling.
        let decision = resolver
            .resolve(
                &principal,
                "chain/transfer",
                "account/bob",
                &empty_ctx(),
                Some(101),
                Some(run_id),
            )
            .await
            .unwrap();

        assert!(
            matches!(decision, GrantDecision::Deny(_)),
            "over-budget request should be denied"
        );
    }

    #[tokio::test]
    async fn budget_check_permits_within_limit() {
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("r1", &["**"], &["**"]));

        let config = ResolverConfig {
            max_context_age: None,
            default_grant_ttl: Duration::hours(1),
        };
        let resolver = GrantResolver::new(set, config);

        let tracker = BudgetTracker::new();
        let agent_id = AgentId::new();
        tracker.configure(agent_id, 1000).await;

        let resolver = resolver.with_budget_tracker(Arc::clone(&tracker)).await;

        let principal = agent_id.to_string();
        let run_id = RunId::new();

        let decision = resolver
            .resolve(
                &principal,
                "chain/transfer",
                "account/bob",
                &empty_ctx(),
                Some(500),
                Some(run_id),
            )
            .await
            .unwrap();

        assert!(
            matches!(decision, GrantDecision::Permit(_)),
            "within-budget request should be permitted"
        );
    }

    // ---- Stale context -------------------------------------------------------

    #[tokio::test]
    async fn stale_context_is_denied() {
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("r1", &["**"], &["**"]));

        let config = ResolverConfig {
            max_context_age: Some(Duration::seconds(60)),
            default_grant_ttl: Duration::hours(1),
        };
        let resolver = GrantResolver::new(set, config);

        // Set evaluated_at to 10 minutes ago.
        let stale_ts =
            (Utc::now() - Duration::minutes(10)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

        let ctx = EvaluationContext {
            attributes: Default::default(),
            evaluated_at: Some(stale_ts),
            ..Default::default()
        };

        let decision = resolver
            .resolve("alice", "chain/transfer", "account/bob", &ctx, None, None)
            .await
            .unwrap();

        assert!(
            matches!(decision, GrantDecision::Deny(_)),
            "stale context must produce Deny"
        );
    }

    // ---- Full pipeline -------------------------------------------------------

    #[tokio::test]
    async fn full_pipeline_allow_with_budget_and_active_grant() {
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("r1", &["chain/**"], &["account/**"]));
        set.add_rule(deny_rule("no-governance", &["governance/**"], &["**"]));

        let config = ResolverConfig {
            max_context_age: None,
            default_grant_ttl: Duration::hours(1),
        };
        let resolver = GrantResolver::new(set, config);

        let tracker = BudgetTracker::new();
        let agent_id = AgentId::new();
        tracker.configure(agent_id, 1_000_000).await;

        let resolver = resolver.with_budget_tracker(Arc::clone(&tracker)).await;

        let principal = agent_id.to_string();
        let run_id = RunId::new();

        // 1. Allowed action within budget.
        let d1 = resolver
            .resolve(
                &principal,
                "chain/transfer",
                "account/bob",
                &empty_ctx(),
                Some(100),
                Some(run_id),
            )
            .await
            .unwrap();
        assert!(matches!(d1, GrantDecision::Permit(_)), "step 1 must permit");

        // 2. Denied action (governance is blocked).
        let d2 = resolver
            .resolve(
                &principal,
                "governance/vote",
                "referendum/1",
                &empty_ctx(),
                None,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(d2, GrantDecision::Deny(_)), "step 2 must deny");

        // 3. Budget status should reflect the spend from step 1.
        let status = tracker.get_remaining(agent_id).await.unwrap();
        assert_eq!(status.spent, 100, "budget should reflect step-1 spend");
    }
}

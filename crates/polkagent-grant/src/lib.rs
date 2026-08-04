//! Policy evaluation, grant resolution, and gate system for the Polkagent
//! platform.
//!
//! # Overview
//!
//! This crate implements the deterministic authorization layer described in
//! PRD-07 §8. It is intentionally free of any LLM interaction: the only inputs
//! are typed Rust values (principals, actions, resources, contexts) and the
//! outputs are typed decisions ([`grant::GrantDecision`]). An LLM cannot
//! influence or bypass this layer.
//!
//! # Module structure
//!
//! | Module | Responsibility |
//! |--------|---------------|
//! | [`policy`] | [`policy::PolicyRule`], [`policy::PolicySet`], and deny-overrides [`policy::evaluate`]. |
//! | [`grant`] | [`grant::GrantResolver`] that runs the full resolution pipeline. |
//! | [`gate`] | [`gate::Gate`] trait and built-in implementations: [`gate::BudgetGate`], [`gate::AllowlistGate`], [`gate::RateLimitGate`], [`gate::ComposedGate`]. |
//! | [`budget`] | [`budget::BudgetTracker`] for per-agent, per-run spend accounting. |
//!
//! # Quick start
//!
//! ```rust,no_run
//! use polkagent_grant::policy::{Effect, PolicyRule, PolicySet};
//! use polkagent_grant::grant::{GrantDecision, GrantResolver, ResolverConfig};
//! use polkagent_grant::policy::EvaluationContext;
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut set = PolicySet::default();
//!     set.add_rule(PolicyRule {
//!         id: "allow-chain".to_string(),
//!         effect: Effect::Allow,
//!         action_patterns: vec!["chain/**".to_string()],
//!         resource_patterns: vec!["account/**".to_string()],
//!         conditions: Default::default(),
//!         abac_condition: None,
//!     });
//!
//!     let resolver = GrantResolver::new(set, ResolverConfig::default());
//!     let ctx = EvaluationContext::default();
//!
//!     let decision = resolver
//!         .resolve("alice", "chain/transfer", "account/bob", &ctx, None, None)
//!         .await
//!         .unwrap();
//!
//!     assert!(matches!(decision, GrantDecision::Permit(_)));
//! }
//! ```
//!
//! # Security invariants
//!
//! - **Deny by default.** A [`PolicySet`](policy::PolicySet) with no matching
//!   rules returns [`PolicyDecision::Deny`](policy::PolicyDecision::Deny).
//! - **Deny overrides allow.** Any matching `Deny` rule beats all `Allow`
//!   rules regardless of rule order.
//! - **Expired grants denied immediately.** [`grant::ResolvedGrant::is_valid`]
//!   checks the expiry; the resolver never returns a stale grant as valid.
//! - **Budget checked before permit.** When a [`budget::BudgetTracker`] is
//!   attached, a spend that would exceed the ceiling produces
//!   [`GrantDecision::Deny`](grant::GrantDecision::Deny).

pub mod budget;
pub mod gate;
pub mod grant;
#[cfg(any(feature = "identity", test))]
pub mod identity;
pub mod loader;
#[cfg(any(feature = "personhood", test))]
pub mod personhood;
pub mod policy;

// ---------------------------------------------------------------------------
// Convenience re-exports for common use cases.
// ---------------------------------------------------------------------------

pub use budget::{BudgetError, BudgetStatus, BudgetTracker};
pub use gate::{
    AllowlistField, AllowlistGate, BudgetGate, ComposedGate, Gate, GateRequest, GateResult,
    RateLimitGate,
};
pub use grant::{
    ActiveGrant, ApprovalRequirement, DeferReason, EffectSet, GrantDecision, GrantLimits,
    GrantResolver, PolicyDenial, ResolvedGrant, ResolverConfig, ResolverError,
};
pub use loader::{load_policy_dir, load_policy_file, merge_policy_sets, LoadError, PolicyFileRule};
pub use policy::{
    builtin_templates, evaluate, evaluate_condition, instantiate_template, pattern_matches,
    resolve_policy_chain, Condition, ContextAttribute, Effect, EvaluationContext, Policy,
    PolicyDecision, PolicyRule, PolicyRuleTemplate, PolicySet, PolicyTemplate, ResolvedPolicy,
    TemplateError, TemplateParam,
};
#[cfg(feature = "identity")]
pub use identity::{context_from_identity, IdentityPrincipal, SS58EncodedAccount};
#[cfg(feature = "personhood")]
pub use personhood::{
    check_personhood, PersonhoodDecision, PersonhoodPolicy, PersonhoodRequirement,
};

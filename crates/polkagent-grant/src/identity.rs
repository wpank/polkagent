//! Identity-based principal types and `EvaluationContext` helpers.
//!
//! This module bridges `polkagent-identity` into the grant/policy layer,
//! allowing policy rules to match against typed on-chain principals (SS58
//! addresses, DIDs) rather than raw strings.
//!
//! # Feature gate
//!
//! The runtime types (`IdentityPrincipal`, `context_from_identity`) require
//! the **`identity`** Cargo feature:
//!
//! ```toml
//! polkagent-grant = { ..., features = ["identity"] }
//! ```

use crate::policy::EvaluationContext;

// ---------------------------------------------------------------------------
// IdentityPrincipal
// ---------------------------------------------------------------------------

/// A typed principal derived from an `AgentIdentity`, used in grant / policy
/// evaluation.
///
/// Requires the `identity` crate feature.
#[cfg(feature = "identity")]
#[derive(Debug, Clone)]
pub struct IdentityPrincipal {
    /// The agent's unique identifier as a string.
    pub agent_id: String,
    /// SS58-encoded primary address (first account, if any).
    pub primary_ss58: Option<String>,
    /// All SS58-encoded addresses associated with this agent.
    pub addresses: Vec<SS58EncodedAccount>,
}

/// A single chain account rendered as an SS58 address string.
///
/// Requires the `identity` crate feature.
#[cfg(feature = "identity")]
#[derive(Debug, Clone)]
pub struct SS58EncodedAccount {
    /// The SS58-encoded address string.
    pub ss58: String,
    /// The network name (e.g. "Polkadot", "Kusama").
    pub network: String,
    /// Optional human-readable label.
    pub label: Option<String>,
}

#[cfg(feature = "identity")]
impl IdentityPrincipal {
    /// Construct an `IdentityPrincipal` from an `AgentIdentity`.
    #[must_use]
    pub fn from_identity(identity: &polkagent_identity::AgentIdentity) -> Self {
        let addresses: Vec<SS58EncodedAccount> = identity
            .accounts
            .iter()
            .map(|account| {
                let ss58 = account.ss58_address();
                SS58EncodedAccount {
                    ss58: ss58.as_str().to_owned(),
                    network: account.network.name().to_owned(),
                    label: account.label.clone(),
                }
            })
            .collect();

        let primary_ss58 = addresses.first().map(|a| a.ss58.clone());

        Self {
            agent_id: identity.agent_id.to_string(),
            primary_ss58,
            addresses,
        }
    }

    /// Returns the primary SS58 address, or the agent ID string when no
    /// on-chain accounts are registered.
    #[must_use]
    pub fn principal_string(&self) -> &str {
        self.primary_ss58.as_deref().unwrap_or(&self.agent_id)
    }
}

// ---------------------------------------------------------------------------
// context_from_identity
// ---------------------------------------------------------------------------

/// Build an [`EvaluationContext`] pre-populated with identity attributes.
///
/// | Key | Value |
/// |-----|-------|
/// | `"identity.agent_id"` | Agent ID string |
/// | `"identity.display_name"` | Agent display name |
/// | `"identity.primary_ss58"` | Primary SS58 address (if any account) |
/// | `"identity.network"` | Network name of primary account (if any) |
/// | `"identity.ss58.N"` | SS58 of N-th account (0-indexed) |
/// | `"identity.network.N"` | Network of N-th account |
///
/// Requires the `identity` crate feature.
#[cfg(feature = "identity")]
#[must_use]
pub fn context_from_identity(identity: &polkagent_identity::AgentIdentity) -> EvaluationContext {
    build_context_impl(
        &identity.agent_id.to_string(),
        &identity.display_name,
        &identity
            .accounts
            .iter()
            .map(|a| {
                (
                    a.ss58_address().as_str().to_owned(),
                    a.network.name().to_owned(),
                    a.label.clone(),
                )
            })
            .collect::<Vec<_>>(),
    )
}

// ---------------------------------------------------------------------------
// Internal shared builder
// ---------------------------------------------------------------------------

/// Build a context from pre-computed account data.
///
/// This function is used both by the feature-gated public API and by tests
/// that call into it via dev-dep helpers below.
fn build_context_impl(
    agent_id: &str,
    display_name: &str,
    accounts: &[(String, String, Option<String>)],
) -> EvaluationContext {
    let mut ctx = EvaluationContext::default();

    ctx.attributes
        .insert("identity.agent_id".to_owned(), agent_id.to_owned());
    ctx.attributes
        .insert("identity.display_name".to_owned(), display_name.to_owned());

    if let Some((ss58, network, _)) = accounts.first() {
        ctx.attributes
            .insert("identity.primary_ss58".to_owned(), ss58.clone());
        ctx.attributes
            .insert("identity.network".to_owned(), network.clone());
    }

    for (i, (ss58, network, _)) in accounts.iter().enumerate() {
        ctx.attributes
            .insert(format!("identity.ss58.{i}"), ss58.clone());
        ctx.attributes
            .insert(format!("identity.network.{i}"), network.clone());
    }

    ctx
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use polkagent_core::AgentId;
    use polkagent_identity::{AccountId32, AgentIdentity, ChainAccount, NetworkId};

    use super::*;

    // ---- Test helpers -------------------------------------------------------

    fn make_identity_with_accounts() -> AgentIdentity {
        let id = AgentId::new();
        AgentIdentity::new(id, "test-agent")
            .with_account(
                ChainAccount::new(AccountId32::from_bytes([1u8; 32]), NetworkId::Polkadot)
                    .with_label("primary"),
            )
            .with_account(
                ChainAccount::new(AccountId32::from_bytes([2u8; 32]), NetworkId::Kusama)
                    .with_label("kusama"),
            )
    }

    fn make_identity_no_accounts() -> AgentIdentity {
        AgentIdentity::new(AgentId::new(), "no-accounts-agent")
    }

    /// Build a context from an identity using the internal builder,
    /// without requiring the `identity` feature to be enabled.
    fn ctx_from(identity: &AgentIdentity) -> EvaluationContext {
        let accounts: Vec<(String, String, Option<String>)> = identity
            .accounts
            .iter()
            .map(|a| {
                (
                    a.ss58_address().as_str().to_owned(),
                    a.network.name().to_owned(),
                    a.label.clone(),
                )
            })
            .collect();
        build_context_impl(
            &identity.agent_id.to_string(),
            &identity.display_name,
            &accounts,
        )
    }

    // ---- EvaluationContext building -----------------------------------------

    #[test]
    fn context_sets_agent_id() {
        let identity = make_identity_with_accounts();
        let ctx = ctx_from(&identity);
        assert_eq!(
            ctx.attributes.get("identity.agent_id").map(String::as_str),
            Some(identity.agent_id.to_string().as_str())
        );
    }

    #[test]
    fn context_sets_display_name() {
        let identity = make_identity_with_accounts();
        let ctx = ctx_from(&identity);
        assert_eq!(
            ctx.attributes
                .get("identity.display_name")
                .map(String::as_str),
            Some("test-agent")
        );
    }

    #[test]
    fn context_sets_primary_ss58_when_accounts_present() {
        let identity = make_identity_with_accounts();
        let ctx = ctx_from(&identity);
        let ss58 = ctx.attributes.get("identity.primary_ss58");
        assert!(ss58.is_some(), "primary_ss58 should be set");
        assert!(!ss58.unwrap().is_empty());
    }

    #[test]
    fn context_no_accounts_omits_ss58() {
        let identity = make_identity_no_accounts();
        let ctx = ctx_from(&identity);
        assert!(!ctx.attributes.contains_key("identity.primary_ss58"));
        assert!(!ctx.attributes.contains_key("identity.network"));
    }

    #[test]
    fn context_sets_indexed_addresses_for_all_accounts() {
        let identity = make_identity_with_accounts();
        let ctx = ctx_from(&identity);

        assert!(ctx.attributes.contains_key("identity.ss58.0"));
        assert!(ctx.attributes.contains_key("identity.ss58.1"));
        assert!(ctx.attributes.contains_key("identity.network.0"));
        assert!(ctx.attributes.contains_key("identity.network.1"));

        assert_eq!(
            ctx.attributes.get("identity.network.0").map(String::as_str),
            Some("Polkadot")
        );
        assert_eq!(
            ctx.attributes.get("identity.network.1").map(String::as_str),
            Some("Kusama")
        );
    }

    #[test]
    fn multi_account_indexed_ss58_are_distinct() {
        let identity = make_identity_with_accounts();
        let ctx = ctx_from(&identity);

        let ss58_0 = ctx.attributes.get("identity.ss58.0").expect("ss58.0");
        let ss58_1 = ctx.attributes.get("identity.ss58.1").expect("ss58.1");
        assert_ne!(
            ss58_0, ss58_1,
            "different accounts must have different SS58s"
        );
    }

    #[test]
    fn identity_display_name_preserved_in_context() {
        let identity = AgentIdentity::new(AgentId::new(), "unique-name-xyz");
        let ctx = ctx_from(&identity);
        assert_eq!(
            ctx.attributes
                .get("identity.display_name")
                .map(String::as_str),
            Some("unique-name-xyz")
        );
    }

    // ---- Policy evaluation with identity context ---------------------------

    #[test]
    fn policy_allows_when_network_condition_matches() {
        use crate::policy::{evaluate, Effect, PolicyDecision, PolicyRule, PolicySet};

        let mut set = PolicySet::default();
        let mut rule = PolicyRule {
            id: "allow-polkadot".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["chain/**".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        };
        rule.conditions
            .insert("identity.network".to_string(), "Polkadot".to_string());
        set.add_rule(rule);

        let identity = make_identity_with_accounts();
        let ctx = ctx_from(&identity);
        let decision = evaluate(&set, "chain/transfer", "account/bob", &ctx);
        assert!(
            matches!(decision, PolicyDecision::Allow),
            "Polkadot identity should be allowed"
        );
    }

    #[test]
    fn policy_denies_when_network_condition_mismatches() {
        use crate::policy::{evaluate, Effect, PolicyDecision, PolicyRule, PolicySet};

        let mut set = PolicySet::default();
        let mut rule = PolicyRule {
            id: "allow-kusama-only".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["chain/**".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        };
        rule.conditions
            .insert("identity.network".to_string(), "Kusama".to_string());
        set.add_rule(rule);

        // Primary account is Polkadot, so identity.network = "Polkadot".
        let identity = make_identity_with_accounts();
        let ctx = ctx_from(&identity);
        let decision = evaluate(&set, "chain/transfer", "account/bob", &ctx);
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "Polkadot identity should be denied by Kusama-only rule"
        );
    }

    #[test]
    fn ss58_principal_matching_exact_address_allows() {
        use crate::policy::{evaluate, Effect, PolicyDecision, PolicyRule, PolicySet};

        let account = ChainAccount::new(AccountId32::from_bytes([0xAAu8; 32]), NetworkId::Polkadot);
        let ss58 = account.ss58_address().as_str().to_owned();

        let identity = AgentIdentity::new(AgentId::new(), "ss58-test").with_account(account);
        let ctx = ctx_from(&identity);

        let mut set = PolicySet::default();
        let mut rule = PolicyRule {
            id: "allow-by-ss58".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["chain/**".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        };
        rule.conditions
            .insert("identity.primary_ss58".to_string(), ss58);
        set.add_rule(rule);

        let decision = evaluate(&set, "chain/query", "account/any", &ctx);
        assert!(
            matches!(decision, PolicyDecision::Allow),
            "exact SS58 match should allow"
        );
    }

    #[test]
    fn ss58_principal_matching_wrong_address_denied() {
        use crate::policy::{evaluate, Effect, PolicyDecision, PolicyRule, PolicySet};

        // Policy allows [0xAA; 32].
        let allowed_account =
            ChainAccount::new(AccountId32::from_bytes([0xAAu8; 32]), NetworkId::Polkadot);
        let allowed_ss58 = allowed_account.ss58_address().as_str().to_owned();

        // But the identity uses [0xBB; 32].
        let other_account =
            ChainAccount::new(AccountId32::from_bytes([0xBBu8; 32]), NetworkId::Polkadot);
        let identity = AgentIdentity::new(AgentId::new(), "wrong-addr").with_account(other_account);
        let ctx = ctx_from(&identity);

        let mut set = PolicySet::default();
        let mut rule = PolicyRule {
            id: "allow-specific-ss58".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["chain/**".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        };
        rule.conditions
            .insert("identity.primary_ss58".to_string(), allowed_ss58);
        set.add_rule(rule);

        let decision = evaluate(&set, "chain/query", "account/any", &ctx);
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "wrong SS58 should be denied"
        );
    }
}

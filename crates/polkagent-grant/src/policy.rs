//! Policy types and deny-overrides evaluation.
//!
//! Polkagent uses a rule-based policy engine for Phase 1. Each [`PolicyRule`]
//! carries an [`Effect`] (Allow or Deny), a set of action patterns, a set of
//! resource patterns, and optional conditions. Evaluation follows the
//! _deny-overrides_ semantic: a single matching Deny rule beats all Allow
//! rules and produces an immediate [`PolicyDecision::Deny`].
//!
//! When no rules match the request at all the decision defaults to
//! [`PolicyDecision::Deny`] — consistent with the "default deny" invariant
//! stated in PRD-07 §8.1.

use serde::{Deserialize, Serialize};
use tracing::trace;

/// The effect of a policy rule: either permit or deny the action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    /// The rule explicitly permits the matched action.
    Allow,
    /// The rule explicitly denies the matched action.
    Deny,
}

/// A single rule inside a [`PolicySet`].
///
/// Patterns use a simple glob syntax where `*` matches any sequence of
/// characters that does not contain a `/`, and `**` matches any sequence
/// including `/`. The matching is performed by [`pattern_matches`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Stable, operator-assigned identifier for this rule (for audit logs).
    pub id: String,

    /// Whether this rule permits or denies the matched request.
    pub effect: Effect,

    /// Glob patterns for actions this rule applies to (e.g. `"chain/transfer"`
    /// or `"chain/**"`). An empty list matches no actions.
    pub action_patterns: Vec<String>,

    /// Glob patterns for resources this rule applies to (e.g.
    /// `"account/5GrwvaEF*"` or `"**"`). An empty list matches no resources.
    pub resource_patterns: Vec<String>,

    /// Free-form key/value conditions that the evaluation context must satisfy.
    /// Every entry must be present and equal in the context for the rule to
    /// match. An empty map means the rule applies unconditionally (subject to
    /// the action and resource patterns).
    #[serde(default)]
    pub conditions: std::collections::HashMap<String, String>,
}

impl PolicyRule {
    /// Returns `true` when this rule matches the given request.
    ///
    /// All three axes (action patterns, resource patterns, conditions) must
    /// match for the rule to apply.
    #[must_use]
    pub fn matches(&self, action: &str, resource: &str, ctx: &EvaluationContext) -> bool {
        let action_match = self
            .action_patterns
            .iter()
            .any(|p| pattern_matches(p, action));

        if !action_match {
            return false;
        }

        let resource_match = self
            .resource_patterns
            .iter()
            .any(|p| pattern_matches(p, resource));

        if !resource_match {
            return false;
        }

        // Every required condition must be present and equal.
        for (key, required_value) in &self.conditions {
            match ctx.attributes.get(key.as_str()) {
                Some(actual) if actual == required_value => {}
                _ => return false,
            }
        }

        true
    }
}

/// Context supplied to the policy evaluator alongside the principal, action,
/// and resource.
///
/// `attributes` holds arbitrary key/value pairs that rules may check through
/// their `conditions` map (e.g. `"network" -> "polkadot"` or
/// `"autonomy_level" -> "observe"`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvaluationContext {
    /// Arbitrary key/value attributes for condition matching.
    pub attributes: std::collections::HashMap<String, String>,

    /// ISO-8601 timestamp of the current evaluation. Used by the resolver to
    /// verify context freshness; not directly matched against rule conditions
    /// in Phase 1.
    pub evaluated_at: Option<String>,
}

/// An ordered collection of [`PolicyRule`]s evaluated with deny-overrides
/// semantics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PolicySet {
    /// Rules are evaluated in order, but deny-overrides means that position
    /// only matters for Allow rules (first match wins among Allows); any Deny
    /// beats all Allows regardless of position.
    pub rules: Vec<PolicyRule>,
}

impl PolicySet {
    /// Construct a [`PolicySet`] from a `Vec` of rules.
    #[must_use]
    pub fn new(rules: Vec<PolicyRule>) -> Self {
        Self { rules }
    }

    /// Add a rule to this set.
    pub fn add_rule(&mut self, rule: PolicyRule) {
        self.rules.push(rule);
    }
}

/// The outcome of evaluating a [`PolicySet`] against a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PolicyDecision {
    /// The request is explicitly permitted.
    Allow,
    /// The request is denied, with an auditable reason.
    Deny {
        /// Human-readable explanation citing the rule that caused the denial.
        reason: String,
    },
    /// The request requires explicit human or quorum approval before it may
    /// proceed.
    RequireApproval {
        /// Human-readable explanation of why approval is required.
        reason: String,
    },
}

/// Evaluate a [`PolicySet`] against `(action, resource, context)` using
/// deny-overrides semantics.
///
/// Algorithm:
/// 1. Iterate every rule in the set.
/// 2. If the rule matches and has `effect: Deny` → return
///    [`PolicyDecision::Deny`] immediately (short-circuit).
/// 3. Track whether any Allow rule matched.
/// 4. After all rules, if an Allow matched → return
///    [`PolicyDecision::Allow`].
/// 5. Otherwise → return [`PolicyDecision::Deny`] (default deny).
#[must_use]
pub fn evaluate(
    policy_set: &PolicySet,
    action: &str,
    resource: &str,
    ctx: &EvaluationContext,
) -> PolicyDecision {
    let mut any_allow = false;

    for rule in &policy_set.rules {
        if !rule.matches(action, resource, ctx) {
            continue;
        }

        trace!(rule_id = %rule.id, effect = ?rule.effect, action, resource, "rule matched");

        match rule.effect {
            Effect::Deny => {
                return PolicyDecision::Deny {
                    reason: format!(
                        "denied by rule '{}': action '{}' on resource '{}'",
                        rule.id, action, resource
                    ),
                };
            }
            Effect::Allow => {
                any_allow = true;
                // Do not break: a later Deny rule must still be able to win.
            }
        }
    }

    if any_allow {
        PolicyDecision::Allow
    } else {
        PolicyDecision::Deny {
            reason: format!(
                "default deny: no rule permitted action '{}' on resource '{}'",
                action, resource
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Glob pattern matching
// ---------------------------------------------------------------------------

/// Match `value` against `pattern` using simplified glob syntax.
///
/// - `**` matches any sequence of characters (including `/`).
/// - `*` matches any sequence of characters that does not contain `/`.
/// - All other characters are matched literally.
#[must_use]
pub fn pattern_matches(pattern: &str, value: &str) -> bool {
    glob_match(pattern.as_bytes(), value.as_bytes())
}

/// Recursive glob matcher.
///
/// We process the pattern left-to-right. The two wildcard cases are handled
/// explicitly before the character-by-character comparison.
fn glob_match(pat: &[u8], val: &[u8]) -> bool {
    // `**` — match zero or more arbitrary characters (including `/`).
    if pat.starts_with(b"**") {
        let rest_pat = &pat[2..];
        // Skip an optional `/` separator between `**` and the next segment.
        let rest_pat = rest_pat.strip_prefix(b"/").unwrap_or(rest_pat);

        // Try matching rest_pat at every position in val.
        for i in 0..=val.len() {
            if glob_match(rest_pat, &val[i..]) {
                return true;
            }
        }
        return false;
    }

    // `*` — match zero or more non-`/` characters.
    if pat.first() == Some(&b'*') {
        let rest_pat = &pat[1..];
        // Advance through val as long as we have not consumed a `/`.
        for i in 0..=val.len() {
            // A single `*` cannot cross a path separator.
            if i > 0 && val[i - 1] == b'/' {
                break;
            }
            if glob_match(rest_pat, &val[i..]) {
                return true;
            }
        }
        return false;
    }

    match (pat.first(), val.first()) {
        // Both exhausted: full match.
        (None, None) => true,
        // Pattern exhausted but value has remaining chars: no match.
        (None, Some(_)) => false,
        // Value exhausted but pattern has remaining chars: no match.
        (Some(_), None) => false,
        // Literal match: advance both pointers.
        (Some(&pc), Some(&vc)) if pc == vc => glob_match(&pat[1..], &val[1..]),
        // Literal mismatch.
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx(attrs: &[(&str, &str)]) -> EvaluationContext {
        EvaluationContext {
            attributes: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            evaluated_at: None,
        }
    }

    fn allow_rule(id: &str, actions: &[&str], resources: &[&str]) -> PolicyRule {
        PolicyRule {
            id: id.to_string(),
            effect: Effect::Allow,
            action_patterns: actions.iter().map(|s| s.to_string()).collect(),
            resource_patterns: resources.iter().map(|s| s.to_string()).collect(),
            conditions: Default::default(),
        }
    }

    fn deny_rule(id: &str, actions: &[&str], resources: &[&str]) -> PolicyRule {
        PolicyRule {
            id: id.to_string(),
            effect: Effect::Deny,
            action_patterns: actions.iter().map(|s| s.to_string()).collect(),
            resource_patterns: resources.iter().map(|s| s.to_string()).collect(),
            conditions: Default::default(),
        }
    }

    // ----- Glob tests -------------------------------------------------------

    #[test]
    fn glob_exact_match() {
        assert!(pattern_matches("chain/transfer", "chain/transfer"));
    }

    #[test]
    fn glob_exact_no_match() {
        assert!(!pattern_matches("chain/transfer", "chain/query"));
    }

    #[test]
    fn glob_star_matches_segment() {
        assert!(pattern_matches("chain/*", "chain/transfer"));
        assert!(!pattern_matches("chain/*", "chain/nested/transfer"));
    }

    #[test]
    fn glob_double_star_matches_across_slashes() {
        assert!(pattern_matches("chain/**", "chain/transfer/polkadot"));
        assert!(pattern_matches("**", "anything/at/all"));
        assert!(pattern_matches("chain/**/transfer", "chain/a/b/transfer"));
    }

    #[test]
    fn glob_empty_pattern_matches_empty_value() {
        assert!(pattern_matches("", ""));
    }

    // ----- Policy evaluation tests ------------------------------------------

    #[test]
    fn default_deny_no_rules() {
        let set = PolicySet::default();
        let ctx = make_ctx(&[]);
        let decision = evaluate(&set, "chain/transfer", "account/alice", &ctx);
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "empty policy set must deny"
        );
    }

    #[test]
    fn allow_rule_permits() {
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("r1", &["chain/transfer"], &["account/**"]));
        let ctx = make_ctx(&[]);
        let decision = evaluate(&set, "chain/transfer", "account/alice", &ctx);
        assert_eq!(decision, PolicyDecision::Allow);
    }

    #[test]
    fn deny_overrides_allow() {
        let mut set = PolicySet::default();
        // Allow first, deny second — deny must win.
        set.add_rule(allow_rule("allow-all", &["**"], &["**"]));
        set.add_rule(deny_rule("deny-transfer", &["chain/transfer"], &["**"]));

        let ctx = make_ctx(&[]);
        let decision = evaluate(&set, "chain/transfer", "account/alice", &ctx);
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "deny rule must override allow rule"
        );
    }

    #[test]
    fn deny_before_allow_also_overrides() {
        let mut set = PolicySet::default();
        // Deny first, allow second — deny must still win.
        set.add_rule(deny_rule("deny-transfer", &["chain/transfer"], &["**"]));
        set.add_rule(allow_rule("allow-all", &["**"], &["**"]));

        let ctx = make_ctx(&[]);
        let decision = evaluate(&set, "chain/transfer", "account/alice", &ctx);
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "deny rule in any position must override allow"
        );
    }

    #[test]
    fn non_matching_deny_does_not_affect_allow() {
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("allow-transfer", &["chain/transfer"], &["**"]));
        // This deny only covers "chain/query", not "chain/transfer".
        set.add_rule(deny_rule("deny-query", &["chain/query"], &["**"]));

        let ctx = make_ctx(&[]);
        let decision = evaluate(&set, "chain/transfer", "account/alice", &ctx);
        assert_eq!(decision, PolicyDecision::Allow);
    }

    #[test]
    fn condition_must_match() {
        let mut set = PolicySet::default();
        let mut rule = allow_rule("allow-with-cond", &["chain/**"], &["**"]);
        rule.conditions
            .insert("network".to_string(), "polkadot".to_string());
        set.add_rule(rule);

        // Context without matching condition → deny (no allow matched).
        let ctx_missing = make_ctx(&[]);
        assert!(matches!(
            evaluate(&set, "chain/transfer", "account/alice", &ctx_missing),
            PolicyDecision::Deny { .. }
        ));

        // Context with matching condition → allow.
        let ctx_match = make_ctx(&[("network", "polkadot")]);
        assert_eq!(
            evaluate(&set, "chain/transfer", "account/alice", &ctx_match),
            PolicyDecision::Allow
        );

        // Wrong value → deny.
        let ctx_wrong = make_ctx(&[("network", "kusama")]);
        assert!(matches!(
            evaluate(&set, "chain/transfer", "account/alice", &ctx_wrong),
            PolicyDecision::Deny { .. }
        ));
    }

    #[test]
    fn multiple_allow_rules_any_match_permits() {
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("allow-query", &["chain/query"], &["**"]));
        set.add_rule(allow_rule("allow-transfer", &["chain/transfer"], &["**"]));

        let ctx = make_ctx(&[]);
        assert_eq!(
            evaluate(&set, "chain/query", "account/alice", &ctx),
            PolicyDecision::Allow
        );
        assert_eq!(
            evaluate(&set, "chain/transfer", "account/alice", &ctx),
            PolicyDecision::Allow
        );
        // Unmatched action still denied.
        assert!(matches!(
            evaluate(&set, "chain/stake", "account/alice", &ctx),
            PolicyDecision::Deny { .. }
        ));
    }
}

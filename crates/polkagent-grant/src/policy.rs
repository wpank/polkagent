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
//!
//! # ABAC Conditions
//!
//! Rules may carry a structured [`Condition`] that is evaluated against the
//! [`EvaluationContext`]. Context attributes are typed via [`ContextAttribute`]
//! and support string equality, numeric comparisons, list membership, glob
//! patterns, and boolean composition (`And`, `Or`, `Not`).
//!
//! # Policy Templates
//!
//! [`PolicyTemplate`]s allow parameterized policy definitions with `{{param}}`
//! placeholders. Use [`instantiate_template`] to produce a concrete [`Policy`].
//!
//! # Policy Inheritance
//!
//! A [`Policy`] may declare `extends: Some(parent_name)`. The function
//! [`resolve_policy_chain`] merges a chain of policies where child rules are
//! appended after parent rules and deny-overrides still applies globally.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::trace;

// ---------------------------------------------------------------------------
// Typed context attributes
// ---------------------------------------------------------------------------

/// A typed value that can be stored in [`EvaluationContext`] attributes.
///
/// Using typed variants instead of raw strings allows numeric comparisons and
/// list membership tests in [`Condition`] expressions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ContextAttribute {
    /// A string scalar.
    String(String),
    /// A floating-point number (covers integers too).
    Number(f64),
    /// A boolean value.
    Bool(bool),
    /// An ordered list of strings.
    List(Vec<String>),
    /// An ISO-8601 timestamp string (stored as-is for external comparison).
    Timestamp(String),
}

impl ContextAttribute {
    /// Return the attribute as a `&str` if it is the `String` or `Timestamp` variant.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ContextAttribute::String(s) | ContextAttribute::Timestamp(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Return the attribute as an `f64` if it is the `Number` variant.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ContextAttribute::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Return the attribute as a `bool` if it is the `Bool` variant.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ContextAttribute::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Return the list slice if this is the `List` variant.
    #[must_use]
    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            ContextAttribute::List(v) => Some(v.as_slice()),
            _ => None,
        }
    }
}

impl From<&str> for ContextAttribute {
    fn from(s: &str) -> Self {
        ContextAttribute::String(s.to_string())
    }
}

impl From<String> for ContextAttribute {
    fn from(s: String) -> Self {
        ContextAttribute::String(s)
    }
}

impl From<f64> for ContextAttribute {
    fn from(n: f64) -> Self {
        ContextAttribute::Number(n)
    }
}

impl From<bool> for ContextAttribute {
    fn from(b: bool) -> Self {
        ContextAttribute::Bool(b)
    }
}

impl From<Vec<String>> for ContextAttribute {
    fn from(v: Vec<String>) -> Self {
        ContextAttribute::List(v)
    }
}

// ---------------------------------------------------------------------------
// ABAC Condition expression tree
// ---------------------------------------------------------------------------

/// A structured condition that can be evaluated against an [`EvaluationContext`].
///
/// Conditions form a recursive expression tree supporting:
/// - Atomic comparisons: [`Condition::Equals`], [`Condition::Contains`],
///   [`Condition::GreaterThan`], [`Condition::LessThan`], [`Condition::In`],
///   [`Condition::Matches`] (glob).
/// - Boolean composition: [`Condition::Not`], [`Condition::And`], [`Condition::Or`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Condition {
    /// The attribute named `attr` must equal `value` (case-sensitive string comparison).
    Equals { attr: String, value: String },

    /// The attribute named `attr` (a string or list) must contain `value` as a substring
    /// (for strings) or as an element (for lists).
    Contains { attr: String, value: String },

    /// The attribute named `attr` (a number) must be strictly greater than `value`.
    GreaterThan { attr: String, value: f64 },

    /// The attribute named `attr` (a number) must be strictly less than `value`.
    LessThan { attr: String, value: f64 },

    /// The attribute named `attr` must be one of the listed `values`.
    In { attr: String, values: Vec<String> },

    /// The attribute named `attr` (a string) must match the glob `pattern`.
    ///
    /// Uses the same glob semantics as [`pattern_matches`]: `*` does not cross
    /// `/`, `**` crosses `/`.
    Matches { attr: String, pattern: String },

    /// Negation: the inner condition must be false.
    Not { inner: Box<Condition> },

    /// Conjunction: all inner conditions must be true.
    And { conditions: Vec<Condition> },

    /// Disjunction: at least one inner condition must be true.
    Or { conditions: Vec<Condition> },
}

/// Evaluate a [`Condition`] against the given [`EvaluationContext`].
///
/// Returns `true` when the condition is satisfied, `false` otherwise.
/// Attributes not present in the context are treated as absent (the condition
/// is not satisfied).
#[must_use]
pub fn evaluate_condition(condition: &Condition, ctx: &EvaluationContext) -> bool {
    match condition {
        Condition::Equals { attr, value } => ctx
            .get_attribute(attr)
            .and_then(|a| a.as_str())
            .map(|s| s == value.as_str())
            .unwrap_or(false),

        Condition::Contains { attr, value } => match ctx.get_attribute(attr) {
            Some(ContextAttribute::String(s)) => s.contains(value.as_str()),
            Some(ContextAttribute::List(list)) => list.iter().any(|el| el == value),
            _ => false,
        },

        Condition::GreaterThan { attr, value } => ctx
            .get_attribute(attr)
            .and_then(|a| a.as_f64())
            .map(|n| n > *value)
            .unwrap_or(false),

        Condition::LessThan { attr, value } => ctx
            .get_attribute(attr)
            .and_then(|a| a.as_f64())
            .map(|n| n < *value)
            .unwrap_or(false),

        Condition::In { attr, values } => ctx
            .get_attribute(attr)
            .and_then(|a| a.as_str())
            .map(|s| values.iter().any(|v| v.as_str() == s))
            .unwrap_or(false),

        Condition::Matches { attr, pattern } => ctx
            .get_attribute(attr)
            .and_then(|a| a.as_str())
            .map(|s| pattern_matches(pattern, s))
            .unwrap_or(false),

        Condition::Not { inner } => !evaluate_condition(inner, ctx),

        Condition::And { conditions } => conditions.iter().all(|c| evaluate_condition(c, ctx)),

        Condition::Or { conditions } => conditions.iter().any(|c| evaluate_condition(c, ctx)),
    }
}

// ---------------------------------------------------------------------------
// Effect
// ---------------------------------------------------------------------------

/// The effect of a policy rule: either permit or deny the action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    /// The rule explicitly permits the matched action.
    Allow,
    /// The rule explicitly denies the matched action.
    Deny,
}

// ---------------------------------------------------------------------------
// PolicyRule
// ---------------------------------------------------------------------------

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
    ///
    /// These legacy string conditions are evaluated alongside [`Self::abac_condition`].
    #[serde(default)]
    pub conditions: HashMap<String, String>,

    /// Optional structured ABAC condition. When present, must evaluate to `true`
    /// in addition to the legacy `conditions` map for the rule to match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abac_condition: Option<Condition>,
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

        // Every required legacy condition must be present and equal.
        for (key, required_value) in &self.conditions {
            match ctx.attributes.get(key.as_str()) {
                Some(actual) if actual == required_value => {}
                _ => return false,
            }
        }

        // Evaluate the structured ABAC condition if present.
        if let Some(cond) = &self.abac_condition {
            if !evaluate_condition(cond, ctx) {
                return false;
            }
        }

        true
    }
}

// ---------------------------------------------------------------------------
// EvaluationContext
// ---------------------------------------------------------------------------

/// Context supplied to the policy evaluator alongside the principal, action,
/// and resource.
///
/// `attributes` holds typed [`ContextAttribute`] values keyed by a dot-separated
/// name (e.g. `"principal.role"`, `"resource.pallet"`, `"environment.time"`).
///
/// The legacy `string_attributes` map is kept for backward compatibility with
/// rules that carry a `conditions: HashMap<String, String>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvaluationContext {
    /// Legacy key/value string attributes for backward-compatible condition matching.
    ///
    /// This field is the same as the old `attributes` field, renamed for clarity.
    /// It is still serialized as `"attributes"` for file-format compatibility.
    #[serde(rename = "attributes")]
    pub attributes: HashMap<String, String>,

    /// Typed attributes used by structured [`Condition`] expressions.
    #[serde(default, rename = "typed_attributes")]
    pub typed_attributes: HashMap<String, ContextAttribute>,

    /// ISO-8601 timestamp of the current evaluation. Used by the resolver to
    /// verify context freshness.
    pub evaluated_at: Option<String>,
}

impl EvaluationContext {
    /// Builder: add a typed attribute and return `self`.
    ///
    /// ```rust
    /// use polkagent_grant::policy::{EvaluationContext, ContextAttribute};
    ///
    /// let ctx = EvaluationContext::default()
    ///     .with_attribute("principal.role", ContextAttribute::String("developer".to_string()))
    ///     .with_attribute("environment.budget", ContextAttribute::Number(500.0));
    /// ```
    #[must_use]
    pub fn with_attribute(mut self, key: impl Into<String>, value: impl Into<ContextAttribute>) -> Self {
        self.typed_attributes.insert(key.into(), value.into());
        self
    }

    /// Look up a typed attribute by key.
    ///
    /// Returns `None` if the key is absent from the typed attribute map.
    #[must_use]
    pub fn get_attribute(&self, key: &str) -> Option<&ContextAttribute> {
        self.typed_attributes.get(key)
    }
}

// ---------------------------------------------------------------------------
// Policy (named, inheritable)
// ---------------------------------------------------------------------------

/// A named policy that holds a set of rules and may optionally extend another
/// policy by name.
///
/// Unlike a raw [`PolicySet`], a `Policy` carries an identity (`name`) and an
/// optional parent reference (`extends`) used by [`resolve_policy_chain`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// Unique name for this policy.
    pub name: String,

    /// Optional description of what this policy permits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Optional name of a parent policy this one inherits from.
    ///
    /// When present, [`resolve_policy_chain`] prepends the parent's rules to
    /// this policy's rules before evaluation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,

    /// The rules defined directly on this policy.
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

impl Policy {
    /// Convert this named policy into an anonymous [`PolicySet`].
    #[must_use]
    pub fn into_policy_set(self) -> PolicySet {
        PolicySet::new(self.rules)
    }
}

/// A policy set that has been resolved through an inheritance chain. All
/// ancestor rules have been inlined (deny rules first, then allow rules).
#[derive(Debug, Clone)]
pub struct ResolvedPolicy {
    /// The final merged [`PolicySet`] with deny-overrides ordering.
    pub policy_set: PolicySet,
    /// The resolution order (outermost ancestor first, final policy last).
    pub resolution_order: Vec<String>,
}

/// Resolve a chain of [`Policy`] objects into a single [`ResolvedPolicy`].
///
/// The `policies` slice is treated as an inheritance chain: the first element
/// is the ultimate ancestor and the last element is the most-derived child.
/// Rules from each level are appended in order. Within the merged rule set
/// deny-overrides semantics still apply (deny rules are sorted before allow
/// rules).
///
/// This function does **not** follow `extends` references automatically. The
/// caller is responsible for ordering the slice correctly (e.g. by walking the
/// `extends` graph upward and reversing).
///
/// # Panics
///
/// Does not panic. An empty slice returns an empty [`ResolvedPolicy`].
#[must_use]
pub fn resolve_policy_chain(policies: &[Policy]) -> ResolvedPolicy {
    let mut all_deny = Vec::new();
    let mut all_allow = Vec::new();
    let mut resolution_order = Vec::with_capacity(policies.len());

    for policy in policies {
        resolution_order.push(policy.name.clone());
        for rule in &policy.rules {
            match rule.effect {
                Effect::Deny => all_deny.push(rule.clone()),
                Effect::Allow => all_allow.push(rule.clone()),
            }
        }
    }

    all_deny.append(&mut all_allow);
    ResolvedPolicy {
        policy_set: PolicySet::new(all_deny),
        resolution_order,
    }
}

// ---------------------------------------------------------------------------
// Policy templates
// ---------------------------------------------------------------------------

/// A parameter definition for a [`PolicyTemplate`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateParam {
    /// The parameter name as it appears in placeholders (without the `{{` `}}`
    /// delimiters).
    pub name: String,

    /// Human-readable description of what the parameter controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// An optional default value used when the parameter is not provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,

    /// Whether the parameter must be provided (no default).
    #[serde(default)]
    pub required: bool,
}

/// Error type returned when template instantiation fails.
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    /// A required parameter was not provided and has no default.
    #[error("missing required template parameter: {param}")]
    MissingParam { param: String },
}

/// A parameterized policy template.
///
/// Templates allow operators to define reusable policy skeletons with
/// `{{param_name}}` placeholders in `action_patterns`, `resource_patterns`, and
/// `conditions` values. [`instantiate_template`] substitutes concrete values to
/// produce a [`PolicySet`].
///
/// # Built-in templates
///
/// The following templates are pre-defined via [`builtin_templates`]:
///
/// | Name | Description |
/// |------|-------------|
/// | `read-only-agent` | Read-only access to chain queries and memory. |
/// | `pallet-scoped-agent` | Full access scoped to a specific pallet. |
/// | `time-limited-agent` | Actions permitted only within a time window. |
/// | `budget-capped-agent` | Actions permitted up to a budget ceiling. |
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyTemplate {
    /// Unique template identifier.
    pub name: String,

    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Declared parameters.
    #[serde(default)]
    pub parameters: Vec<TemplateParam>,

    /// Rule skeletons. Placeholders use `{{param_name}}` syntax.
    pub rules: Vec<PolicyRuleTemplate>,
}

/// A rule skeleton that may contain `{{param}}` placeholders.
///
/// After instantiation via [`instantiate_template`] all placeholders are
/// replaced with concrete values and the result is converted to a [`PolicyRule`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRuleTemplate {
    pub id: String,
    pub effect: Effect,
    pub action_patterns: Vec<String>,
    pub resource_patterns: Vec<String>,
    #[serde(default)]
    pub conditions: HashMap<String, String>,
}

/// Instantiate a [`PolicyTemplate`] with the given parameter map.
///
/// Parameters are substituted as literal string replacements of `{{name}}`
/// occurrences in `id`, `action_patterns`, `resource_patterns`, and `conditions`
/// values.
///
/// # Errors
///
/// Returns [`TemplateError::MissingParam`] if a required parameter is absent
/// from `params` and has no default value in the template definition.
pub fn instantiate_template(
    template: &PolicyTemplate,
    params: &HashMap<String, String>,
) -> Result<PolicySet, TemplateError> {
    // Build the final substitution map: provided params override defaults.
    let mut subs: HashMap<String, String> = HashMap::new();
    for param in &template.parameters {
        if let Some(provided) = params.get(&param.name) {
            subs.insert(param.name.clone(), provided.clone());
        } else if let Some(default) = &param.default {
            subs.insert(param.name.clone(), default.clone());
        } else if param.required {
            return Err(TemplateError::MissingParam {
                param: param.name.clone(),
            });
        }
        // Optional params with no value and no default are simply absent from
        // the substitution map; any placeholder for them is left unreplaced
        // (callers should not use such params in patterns).
    }

    let rules: Vec<PolicyRule> = template
        .rules
        .iter()
        .map(|rule_tpl| {
            let substitute = |s: &str| -> String {
                let mut out = s.to_string();
                for (k, v) in &subs {
                    out = out.replace(&format!("{{{{{k}}}}}"), v);
                }
                out
            };

            PolicyRule {
                id: substitute(&rule_tpl.id),
                effect: rule_tpl.effect.clone(),
                action_patterns: rule_tpl.action_patterns.iter().map(|p| substitute(p)).collect(),
                resource_patterns: rule_tpl.resource_patterns.iter().map(|p| substitute(p)).collect(),
                conditions: rule_tpl
                    .conditions
                    .iter()
                    .map(|(k, v)| (substitute(k), substitute(v)))
                    .collect(),
                abac_condition: None,
            }
        })
        .collect();

    Ok(PolicySet::new(rules))
}

/// Return the set of built-in [`PolicyTemplate`]s shipped with the crate.
///
/// Templates:
/// - `read-only-agent` — read-only access to chain queries and memory.
/// - `pallet-scoped-agent` — full access scoped to one pallet (param: `pallet`).
/// - `time-limited-agent` — actions permitted only within a named time window
///   (param: `window_attr`, `window_value`).
/// - `budget-capped-agent` — actions permitted up to a budget ceiling
///   (param: `max_amount`).
#[must_use]
pub fn builtin_templates() -> Vec<PolicyTemplate> {
    vec![
        // ------------------------------------------------------------------
        // read-only-agent
        // ------------------------------------------------------------------
        PolicyTemplate {
            name: "read-only-agent".to_string(),
            description: Some(
                "Read-only access to chain queries and memory reads. No writes permitted."
                    .to_string(),
            ),
            parameters: vec![],
            rules: vec![
                PolicyRuleTemplate {
                    id: "ro-allow-chain-query".to_string(),
                    effect: Effect::Allow,
                    action_patterns: vec!["chain.query".to_string(), "chain.decode".to_string(), "chain.balance".to_string()],
                    resource_patterns: vec!["**".to_string()],
                    conditions: Default::default(),
                },
                PolicyRuleTemplate {
                    id: "ro-allow-memory-read".to_string(),
                    effect: Effect::Allow,
                    action_patterns: vec!["memory.read".to_string(), "memory.search".to_string()],
                    resource_patterns: vec!["**".to_string()],
                    conditions: Default::default(),
                },
                PolicyRuleTemplate {
                    id: "ro-deny-writes".to_string(),
                    effect: Effect::Deny,
                    action_patterns: vec!["chain.submit".to_string(), "chain.sign".to_string(), "file.write".to_string(), "shell.execute".to_string()],
                    resource_patterns: vec!["**".to_string()],
                    conditions: Default::default(),
                },
            ],
        },

        // ------------------------------------------------------------------
        // pallet-scoped-agent
        // ------------------------------------------------------------------
        PolicyTemplate {
            name: "pallet-scoped-agent".to_string(),
            description: Some(
                "Full access to chain operations scoped to a specific pallet.".to_string(),
            ),
            parameters: vec![TemplateParam {
                name: "pallet".to_string(),
                description: Some("The pallet name to scope access to (e.g. 'balances')".to_string()),
                default: None,
                required: true,
            }],
            rules: vec![
                PolicyRuleTemplate {
                    id: "pallet-allow-{{pallet}}".to_string(),
                    effect: Effect::Allow,
                    action_patterns: vec!["chain.*".to_string()],
                    resource_patterns: vec!["pallet/{{pallet}}/**".to_string()],
                    conditions: Default::default(),
                },
                PolicyRuleTemplate {
                    id: "pallet-deny-other".to_string(),
                    effect: Effect::Deny,
                    action_patterns: vec!["chain.*".to_string()],
                    resource_patterns: vec!["**".to_string()],
                    conditions: HashMap::from([
                        ("__deny_other_pallets".to_string(), "true".to_string()),
                    ]),
                },
            ],
        },

        // ------------------------------------------------------------------
        // time-limited-agent
        // ------------------------------------------------------------------
        PolicyTemplate {
            name: "time-limited-agent".to_string(),
            description: Some(
                "Actions permitted only when a named time-window attribute is present.".to_string(),
            ),
            parameters: vec![
                TemplateParam {
                    name: "window_attr".to_string(),
                    description: Some("Context attribute name that signals the allowed window (e.g. 'environment.time_window')".to_string()),
                    default: Some("environment.time_window".to_string()),
                    required: false,
                },
                TemplateParam {
                    name: "window_value".to_string(),
                    description: Some("Expected value of the time-window attribute".to_string()),
                    default: Some("business_hours".to_string()),
                    required: false,
                },
            ],
            rules: vec![
                PolicyRuleTemplate {
                    id: "time-allow-in-window".to_string(),
                    effect: Effect::Allow,
                    action_patterns: vec!["chain.*".to_string(), "memory.*".to_string()],
                    resource_patterns: vec!["**".to_string()],
                    conditions: HashMap::from([
                        ("{{window_attr}}".to_string(), "{{window_value}}".to_string()),
                    ]),
                },
                PolicyRuleTemplate {
                    id: "time-deny-outside-window".to_string(),
                    effect: Effect::Deny,
                    action_patterns: vec!["chain.submit".to_string(), "chain.sign".to_string()],
                    resource_patterns: vec!["**".to_string()],
                    conditions: Default::default(),
                },
            ],
        },

        // ------------------------------------------------------------------
        // budget-capped-agent
        // ------------------------------------------------------------------
        PolicyTemplate {
            name: "budget-capped-agent".to_string(),
            description: Some(
                "Actions permitted, constrained by a maximum-amount condition.".to_string(),
            ),
            parameters: vec![TemplateParam {
                name: "max_amount".to_string(),
                description: Some("Maximum transfer amount allowed (as a string)".to_string()),
                default: None,
                required: true,
            }],
            rules: vec![
                PolicyRuleTemplate {
                    id: "budget-allow-chain".to_string(),
                    effect: Effect::Allow,
                    action_patterns: vec!["chain.*".to_string()],
                    resource_patterns: vec!["**".to_string()],
                    conditions: HashMap::from([
                        ("max_amount".to_string(), "{{max_amount}}".to_string()),
                    ]),
                },
                PolicyRuleTemplate {
                    id: "budget-allow-memory".to_string(),
                    effect: Effect::Allow,
                    action_patterns: vec!["memory.*".to_string()],
                    resource_patterns: vec!["**".to_string()],
                    conditions: Default::default(),
                },
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// PolicySet
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// PolicyDecision
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// evaluate
// ---------------------------------------------------------------------------

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

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    fn make_ctx(attrs: &[(&str, &str)]) -> EvaluationContext {
        EvaluationContext {
            attributes: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            evaluated_at: None,
            ..Default::default()
        }
    }

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

    fn typed_ctx() -> EvaluationContext {
        EvaluationContext::default()
            .with_attribute("principal.id", ContextAttribute::String("alice".to_string()))
            .with_attribute("principal.role", ContextAttribute::String("developer".to_string()))
            .with_attribute("action.kind", ContextAttribute::String("chain.query".to_string()))
            .with_attribute("resource.pallet", ContextAttribute::String("balances".to_string()))
            .with_attribute("resource.call", ContextAttribute::String("transfer".to_string()))
            .with_attribute("environment.time", ContextAttribute::Timestamp("2026-07-31T10:00:00Z".to_string()))
            .with_attribute("environment.network", ContextAttribute::String("westend".to_string()))
            .with_attribute("budget.remaining", ContextAttribute::Number(500.0))
            .with_attribute("budget.limit", ContextAttribute::Number(1000.0))
            .with_attribute("allowed.pallets", ContextAttribute::List(vec![
                "balances".to_string(),
                "staking".to_string(),
            ]))
    }

    // -------------------------------------------------------------------------
    // Glob tests (existing)
    // -------------------------------------------------------------------------

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

    // -------------------------------------------------------------------------
    // Policy evaluation tests (existing)
    // -------------------------------------------------------------------------

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

    // -------------------------------------------------------------------------
    // ContextAttribute tests
    // -------------------------------------------------------------------------

    #[test]
    fn context_attribute_as_str() {
        let a = ContextAttribute::String("hello".to_string());
        assert_eq!(a.as_str(), Some("hello"));
        let b = ContextAttribute::Timestamp("2026-01-01T00:00:00Z".to_string());
        assert_eq!(b.as_str(), Some("2026-01-01T00:00:00Z"));
        let c = ContextAttribute::Number(1.0);
        assert_eq!(c.as_str(), None);
    }

    #[test]
    fn context_attribute_as_f64() {
        let a = ContextAttribute::Number(42.5);
        assert_eq!(a.as_f64(), Some(42.5));
        let b = ContextAttribute::String("42".to_string());
        assert_eq!(b.as_f64(), None);
    }

    #[test]
    fn context_attribute_as_bool() {
        let a = ContextAttribute::Bool(true);
        assert_eq!(a.as_bool(), Some(true));
        let b = ContextAttribute::String("true".to_string());
        assert_eq!(b.as_bool(), None);
    }

    #[test]
    fn context_attribute_as_list() {
        let a = ContextAttribute::List(vec!["x".to_string(), "y".to_string()]);
        assert_eq!(a.as_list(), Some(["x".to_string(), "y".to_string()].as_slice()));
        let b = ContextAttribute::String("x".to_string());
        assert_eq!(b.as_list(), None);
    }

    #[test]
    fn evaluation_context_builder() {
        let ctx = EvaluationContext::default()
            .with_attribute("principal.role", ContextAttribute::String("admin".to_string()))
            .with_attribute("budget.remaining", ContextAttribute::Number(250.0));

        assert_eq!(
            ctx.get_attribute("principal.role").unwrap().as_str(),
            Some("admin")
        );
        assert_eq!(
            ctx.get_attribute("budget.remaining").unwrap().as_f64(),
            Some(250.0)
        );
        assert!(ctx.get_attribute("nonexistent").is_none());
    }

    // -------------------------------------------------------------------------
    // Condition evaluation: atomic operators
    // -------------------------------------------------------------------------

    #[test]
    fn condition_equals_string_match() {
        let ctx = typed_ctx();
        let cond = Condition::Equals {
            attr: "principal.role".to_string(),
            value: "developer".to_string(),
        };
        assert!(evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_equals_string_no_match() {
        let ctx = typed_ctx();
        let cond = Condition::Equals {
            attr: "principal.role".to_string(),
            value: "operator".to_string(),
        };
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_equals_missing_attr_is_false() {
        let ctx = EvaluationContext::default();
        let cond = Condition::Equals {
            attr: "principal.role".to_string(),
            value: "developer".to_string(),
        };
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_contains_substring() {
        let ctx = typed_ctx();
        let cond = Condition::Contains {
            attr: "resource.pallet".to_string(),
            value: "ance".to_string(), // "balances" contains "ance"
        };
        assert!(evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_contains_list_element() {
        let ctx = typed_ctx();
        let cond = Condition::Contains {
            attr: "allowed.pallets".to_string(),
            value: "staking".to_string(),
        };
        assert!(evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_contains_list_element_missing() {
        let ctx = typed_ctx();
        let cond = Condition::Contains {
            attr: "allowed.pallets".to_string(),
            value: "governance".to_string(),
        };
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_greater_than_passes() {
        let ctx = typed_ctx(); // budget.remaining = 500
        let cond = Condition::GreaterThan {
            attr: "budget.remaining".to_string(),
            value: 100.0,
        };
        assert!(evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_greater_than_fails_equal() {
        let ctx = typed_ctx(); // budget.remaining = 500
        let cond = Condition::GreaterThan {
            attr: "budget.remaining".to_string(),
            value: 500.0,
        };
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_less_than_passes() {
        let ctx = typed_ctx(); // budget.remaining = 500
        let cond = Condition::LessThan {
            attr: "budget.remaining".to_string(),
            value: 1000.0,
        };
        assert!(evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_less_than_fails_equal() {
        let ctx = typed_ctx(); // budget.remaining = 500
        let cond = Condition::LessThan {
            attr: "budget.remaining".to_string(),
            value: 500.0,
        };
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_in_passes() {
        let ctx = typed_ctx(); // environment.network = "westend"
        let cond = Condition::In {
            attr: "environment.network".to_string(),
            values: vec!["westend".to_string(), "rococo".to_string()],
        };
        assert!(evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_in_fails() {
        let ctx = typed_ctx();
        let cond = Condition::In {
            attr: "environment.network".to_string(),
            values: vec!["polkadot".to_string(), "kusama".to_string()],
        };
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_matches_glob_passes() {
        let ctx = typed_ctx(); // resource.pallet = "balances"
        let cond = Condition::Matches {
            attr: "resource.pallet".to_string(),
            pattern: "bal*".to_string(),
        };
        assert!(evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_matches_glob_fails() {
        let ctx = typed_ctx();
        let cond = Condition::Matches {
            attr: "resource.pallet".to_string(),
            pattern: "staking*".to_string(),
        };
        assert!(!evaluate_condition(&cond, &ctx));
    }

    // -------------------------------------------------------------------------
    // Condition composition: And, Or, Not
    // -------------------------------------------------------------------------

    #[test]
    fn condition_not_inverts() {
        let ctx = typed_ctx();
        let inner = Condition::Equals {
            attr: "principal.role".to_string(),
            value: "developer".to_string(),
        };
        // developer == developer → true; NOT(true) → false
        assert!(!evaluate_condition(&Condition::Not { inner: Box::new(inner.clone()) }, &ctx));

        let wrong = Condition::Equals {
            attr: "principal.role".to_string(),
            value: "operator".to_string(),
        };
        // operator != developer → false; NOT(false) → true
        assert!(evaluate_condition(&Condition::Not { inner: Box::new(wrong) }, &ctx));
    }

    #[test]
    fn condition_and_all_true() {
        let ctx = typed_ctx();
        let cond = Condition::And {
            conditions: vec![
                Condition::Equals {
                    attr: "principal.role".to_string(),
                    value: "developer".to_string(),
                },
                Condition::GreaterThan {
                    attr: "budget.remaining".to_string(),
                    value: 100.0,
                },
            ],
        };
        assert!(evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_and_one_false() {
        let ctx = typed_ctx();
        let cond = Condition::And {
            conditions: vec![
                Condition::Equals {
                    attr: "principal.role".to_string(),
                    value: "developer".to_string(),
                },
                Condition::GreaterThan {
                    attr: "budget.remaining".to_string(),
                    value: 9999.0, // fails: 500 is not > 9999
                },
            ],
        };
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_or_one_true() {
        let ctx = typed_ctx();
        let cond = Condition::Or {
            conditions: vec![
                Condition::Equals {
                    attr: "principal.role".to_string(),
                    value: "operator".to_string(), // false
                },
                Condition::Equals {
                    attr: "principal.role".to_string(),
                    value: "developer".to_string(), // true
                },
            ],
        };
        assert!(evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_or_all_false() {
        let ctx = typed_ctx();
        let cond = Condition::Or {
            conditions: vec![
                Condition::Equals {
                    attr: "principal.role".to_string(),
                    value: "operator".to_string(),
                },
                Condition::Equals {
                    attr: "principal.role".to_string(),
                    value: "admin".to_string(),
                },
            ],
        };
        assert!(!evaluate_condition(&cond, &ctx));
    }

    #[test]
    fn condition_and_empty_is_true() {
        let ctx = EvaluationContext::default();
        assert!(evaluate_condition(&Condition::And { conditions: vec![] }, &ctx));
    }

    #[test]
    fn condition_or_empty_is_false() {
        let ctx = EvaluationContext::default();
        assert!(!evaluate_condition(&Condition::Or { conditions: vec![] }, &ctx));
    }

    #[test]
    fn condition_nested_and_or_not() {
        let ctx = typed_ctx();
        // (role == developer OR role == operator) AND NOT (network == polkadot)
        let cond = Condition::And {
            conditions: vec![
                Condition::Or {
                    conditions: vec![
                        Condition::Equals {
                            attr: "principal.role".to_string(),
                            value: "developer".to_string(),
                        },
                        Condition::Equals {
                            attr: "principal.role".to_string(),
                            value: "operator".to_string(),
                        },
                    ],
                },
                Condition::Not {
                    inner: Box::new(Condition::Equals {
                        attr: "environment.network".to_string(),
                        value: "polkadot".to_string(),
                    }),
                },
            ],
        };
        // role IS developer → OR = true; network is westend NOT polkadot → NOT = true; AND = true
        assert!(evaluate_condition(&cond, &ctx));
    }

    // -------------------------------------------------------------------------
    // ABAC condition integrated into PolicyRule matching
    // -------------------------------------------------------------------------

    #[test]
    fn rule_with_abac_condition_matches_when_true() {
        let mut rule = allow_rule("abac-rule", &["chain.*"], &["**"]);
        rule.abac_condition = Some(Condition::Equals {
            attr: "principal.role".to_string(),
            value: "developer".to_string(),
        });

        let ctx = typed_ctx(); // principal.role = developer
        assert!(rule.matches("chain.query", "pallet/balances", &ctx));
    }

    #[test]
    fn rule_with_abac_condition_no_match_when_false() {
        let mut rule = allow_rule("abac-rule", &["chain.*"], &["**"]);
        rule.abac_condition = Some(Condition::Equals {
            attr: "principal.role".to_string(),
            value: "operator".to_string(),
        });

        let ctx = typed_ctx(); // principal.role = developer, not operator
        assert!(!rule.matches("chain.query", "pallet/balances", &ctx));
    }

    // -------------------------------------------------------------------------
    // Policy template tests
    // -------------------------------------------------------------------------

    #[test]
    fn template_read_only_agent_instantiates() {
        let templates = builtin_templates();
        let tpl = templates.iter().find(|t| t.name == "read-only-agent").unwrap();
        let set = instantiate_template(tpl, &HashMap::new()).unwrap();
        assert_eq!(set.rules.len(), 3);

        let ctx = EvaluationContext::default();
        assert_eq!(evaluate(&set, "chain.query", "any", &ctx), PolicyDecision::Allow);
        assert!(matches!(evaluate(&set, "chain.submit", "any", &ctx), PolicyDecision::Deny { .. }));
    }

    #[test]
    fn template_pallet_scoped_agent_instantiates() {
        let templates = builtin_templates();
        let tpl = templates.iter().find(|t| t.name == "pallet-scoped-agent").unwrap();

        let mut params = HashMap::new();
        params.insert("pallet".to_string(), "balances".to_string());

        let set = instantiate_template(tpl, &params).unwrap();

        // Check that placeholder was substituted in rule id and resource pattern.
        let allow_rule = set.rules.iter().find(|r| r.effect == Effect::Allow).unwrap();
        assert!(allow_rule.id.contains("balances"), "rule id should contain 'balances'");
        assert!(allow_rule.resource_patterns.iter().any(|p| p.contains("balances")));
    }

    #[test]
    fn template_budget_capped_agent_instantiates() {
        let templates = builtin_templates();
        let tpl = templates.iter().find(|t| t.name == "budget-capped-agent").unwrap();

        let mut params = HashMap::new();
        params.insert("max_amount".to_string(), "500000".to_string());

        let set = instantiate_template(tpl, &params).unwrap();
        let chain_rule = set.rules.iter().find(|r| r.id == "budget-allow-chain").unwrap();
        assert_eq!(chain_rule.conditions.get("max_amount").map(String::as_str), Some("500000"));
    }

    #[test]
    fn template_missing_required_param_is_error() {
        let templates = builtin_templates();
        let tpl = templates.iter().find(|t| t.name == "pallet-scoped-agent").unwrap();

        // No params provided — 'pallet' is required.
        let result = instantiate_template(tpl, &HashMap::new());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TemplateError::MissingParam { .. }));
    }

    #[test]
    fn template_optional_param_uses_default() {
        let templates = builtin_templates();
        let tpl = templates.iter().find(|t| t.name == "time-limited-agent").unwrap();

        // No params → defaults should be used.
        let set = instantiate_template(tpl, &HashMap::new()).unwrap();
        assert!(!set.rules.is_empty());
    }

    #[test]
    fn template_param_override_replaces_default() {
        let templates = builtin_templates();
        let tpl = templates.iter().find(|t| t.name == "time-limited-agent").unwrap();

        let mut params = HashMap::new();
        params.insert("window_value".to_string(), "off_hours".to_string());

        let set = instantiate_template(tpl, &params).unwrap();
        // The allow rule's condition value should be "off_hours" not "business_hours".
        let allow_rule = set.rules.iter().find(|r| r.effect == Effect::Allow).unwrap();
        let has_off_hours = allow_rule.conditions.values().any(|v| v == "off_hours");
        assert!(has_off_hours, "override should replace default");
    }

    // -------------------------------------------------------------------------
    // Policy inheritance tests
    // -------------------------------------------------------------------------

    #[test]
    fn resolve_policy_chain_empty() {
        let resolved = resolve_policy_chain(&[]);
        assert!(resolved.policy_set.rules.is_empty());
        assert!(resolved.resolution_order.is_empty());
    }

    #[test]
    fn resolve_policy_chain_single() {
        let policy = Policy {
            name: "base".to_string(),
            description: None,
            extends: None,
            rules: vec![allow_rule("base-rule", &["chain.*"], &["**"])],
        };
        let resolved = resolve_policy_chain(&[policy]);
        assert_eq!(resolved.policy_set.rules.len(), 1);
        assert_eq!(resolved.resolution_order, vec!["base"]);
    }

    #[test]
    fn resolve_policy_chain_parent_child() {
        let parent = Policy {
            name: "parent".to_string(),
            description: None,
            extends: None,
            rules: vec![allow_rule("parent-allow", &["memory.*"], &["**"])],
        };
        let child = Policy {
            name: "child".to_string(),
            description: None,
            extends: Some("parent".to_string()),
            rules: vec![
                allow_rule("child-allow", &["chain.*"], &["**"]),
                deny_rule("child-deny", &["chain.sign"], &["**"]),
            ],
        };

        let resolved = resolve_policy_chain(&[parent, child]);
        // 1 deny + 2 allow = 3 rules; deny goes first
        assert_eq!(resolved.policy_set.rules.len(), 3);
        assert_eq!(resolved.resolution_order, vec!["parent", "child"]);
        // Deny rules precede allow rules.
        assert_eq!(resolved.policy_set.rules[0].effect, Effect::Deny);
    }

    #[test]
    fn resolve_policy_chain_deny_wins_globally() {
        // Parent allows everything; child adds a deny for a specific action.
        // Even if the child allow rule fires, the parent deny wins.
        let parent = Policy {
            name: "permissive".to_string(),
            description: None,
            extends: None,
            rules: vec![deny_rule("parent-deny-sign", &["chain.sign"], &["**"])],
        };
        let child = Policy {
            name: "override-attempt".to_string(),
            description: None,
            extends: Some("permissive".to_string()),
            rules: vec![allow_rule("child-allow-all", &["**"], &["**"])],
        };

        let resolved = resolve_policy_chain(&[parent, child]);
        let ctx = EvaluationContext::default();
        // Deny from parent should still win even with child's allow-all.
        assert!(matches!(
            evaluate(&resolved.policy_set, "chain.sign", "key/alice", &ctx),
            PolicyDecision::Deny { .. }
        ));
    }

    #[test]
    fn resolve_policy_chain_resolution_order() {
        let policies: Vec<Policy> = ["base", "middle", "derived"]
            .iter()
            .map(|name| Policy {
                name: name.to_string(),
                description: None,
                extends: None,
                rules: vec![],
            })
            .collect();

        let resolved = resolve_policy_chain(&policies);
        assert_eq!(
            resolved.resolution_order,
            vec!["base", "middle", "derived"]
        );
    }
}

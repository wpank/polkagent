//! TOML-based policy file loading.
//!
//! This module provides utilities to load [`PolicySet`]s from TOML files that
//! use a Cedar-inspired rule format. Each file contains a `[[rules]]` array
//! of table entries that map directly to [`PolicyRule`]s.
//!
//! # File format
//!
//! ```toml
//! [[rules]]
//! id = "allow-chain-query"
//! effect = "allow"
//! action_patterns = ["chain.query", "chain.decode"]
//! resource_patterns = ["**"]
//!
//! [[rules]]
//! id = "deny-writes"
//! effect = "deny"
//! action_patterns = ["chain.submit"]
//! resource_patterns = ["**"]
//! ```
//!
//! Rules may also carry a structured `[rules.abac_condition]` TOML table that
//! is deserialized into a [`Condition`] and evaluated by the policy engine.
//!
//! # Loading
//!
//! - [`load_policy_file`] loads a single `.toml` file into a [`PolicySet`].
//! - [`load_policy_dir`] loads every `.toml` file in a directory and merges
//!   them into one [`PolicySet`].
//! - [`merge_policy_sets`] combines multiple sets with deny-takes-precedence
//!   ordering (deny rules first, then allow rules).

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use tracing::debug;

use crate::policy::{Condition, Effect, PolicyRule, PolicySet};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur while loading policy files.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// Failed to read a policy file from disk.
    #[error("failed to read policy file {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },

    /// Failed to parse the TOML content of a policy file.
    #[error("failed to parse policy file {path}: {source}")]
    Parse {
        path: String,
        source: toml::de::Error,
    },

    /// The directory could not be read.
    #[error("failed to read policy directory {path}: {source}")]
    ReadDir {
        path: String,
        source: std::io::Error,
    },
}

// ---------------------------------------------------------------------------
// Serde structs matching the TOML format
// ---------------------------------------------------------------------------

/// Top-level structure of a policy TOML file.
///
/// The file is expected to contain a `[[rules]]` array of
/// [`PolicyFileRule`] entries.
#[derive(Debug, Deserialize)]
struct PolicyFile {
    /// The rules declared in this file.
    rules: Vec<PolicyFileRule>,
}

/// A single rule as represented in a TOML policy file.
///
/// This struct is the serde counterpart of [`PolicyRule`]; it uses the same
/// field names so that serialization round-trips cleanly.
#[derive(Debug, Deserialize)]
pub struct PolicyFileRule {
    /// Stable, operator-assigned identifier for this rule.
    pub id: String,

    /// Whether this rule permits or denies the matched request.
    pub effect: Effect,

    /// Glob patterns for actions this rule applies to.
    pub action_patterns: Vec<String>,

    /// Glob patterns for resources this rule applies to.
    pub resource_patterns: Vec<String>,

    /// Optional key/value conditions the evaluation context must satisfy.
    #[serde(default)]
    pub conditions: HashMap<String, String>,

    /// Optional structured ABAC condition expression.
    ///
    /// When present in TOML this is parsed as a nested table whose `op` key
    /// determines the [`Condition`] variant.
    #[serde(default)]
    pub abac_condition: Option<Condition>,
}

impl From<PolicyFileRule> for PolicyRule {
    fn from(file_rule: PolicyFileRule) -> Self {
        Self {
            id: file_rule.id,
            effect: file_rule.effect,
            action_patterns: file_rule.action_patterns,
            resource_patterns: file_rule.resource_patterns,
            conditions: file_rule.conditions,
            abac_condition: file_rule.abac_condition,
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load a single TOML policy file and return a [`PolicySet`].
///
/// The file must contain a `[[rules]]` array. Each entry is converted to a
/// [`PolicyRule`] and added to the resulting set in declaration order.
pub fn load_policy_file(path: &Path) -> Result<PolicySet, LoadError> {
    let content = std::fs::read_to_string(path).map_err(|e| LoadError::Io {
        path: path.display().to_string(),
        source: e,
    })?;

    let policy_file: PolicyFile = toml::from_str(&content).map_err(|e| LoadError::Parse {
        path: path.display().to_string(),
        source: e,
    })?;

    let rules: Vec<PolicyRule> = policy_file.rules.into_iter().map(Into::into).collect();

    debug!(
        path = %path.display(),
        rule_count = rules.len(),
        "loaded policy file"
    );

    Ok(PolicySet::new(rules))
}

/// Load all `.toml` files in the given directory and merge them into a single
/// [`PolicySet`].
///
/// Files are sorted by name to ensure deterministic ordering. The merge
/// strategy places deny rules before allow rules (see [`merge_policy_sets`]).
pub fn load_policy_dir(dir: &Path) -> Result<PolicySet, LoadError> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| LoadError::ReadDir {
            path: dir.display().to_string(),
            source: e,
        })?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    // Sort for deterministic ordering.
    entries.sort();

    let mut sets = Vec::with_capacity(entries.len());
    for path in &entries {
        sets.push(load_policy_file(path)?);
    }

    debug!(
        dir = %dir.display(),
        file_count = sets.len(),
        "loaded policy directory"
    );

    Ok(merge_policy_sets(sets))
}

/// Merge multiple [`PolicySet`]s into one.
///
/// The merge strategy applies **deny-takes-precedence** ordering: all deny
/// rules are placed before all allow rules. Within each group the original
/// order is preserved (deny rules from the first set come before deny rules
/// from the second set, and likewise for allow rules).
///
/// This ensures that even when merging policies from different files, any
/// deny rule will short-circuit evaluation before an allow rule from a
/// different file can match.
pub fn merge_policy_sets(sets: Vec<PolicySet>) -> PolicySet {
    let total_rules: usize = sets.iter().map(|s| s.rules.len()).sum();
    let mut deny_rules = Vec::with_capacity(total_rules);
    let mut allow_rules = Vec::with_capacity(total_rules);

    for set in sets {
        for rule in set.rules {
            match rule.effect {
                Effect::Deny => deny_rules.push(rule),
                Effect::Allow => allow_rules.push(rule),
            }
        }
    }

    deny_rules.append(&mut allow_rules);
    PolicySet::new(deny_rules)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{evaluate, Condition, ContextAttribute, EvaluationContext, PolicyDecision};
    use std::path::PathBuf;

    /// Helper to resolve the workspace root from the crate directory.
    fn fixtures_dir() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        // crates/polkagent-grant -> workspace root
        manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .expect("could not resolve workspace root")
            .join("fixtures")
            .join("policies")
    }

    fn empty_ctx() -> EvaluationContext {
        EvaluationContext::default()
    }

    fn ctx_with(attrs: &[(&str, &str)]) -> EvaluationContext {
        EvaluationContext {
            attributes: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            evaluated_at: None,
            ..Default::default()
        }
    }

    // ---- File loading tests ------------------------------------------------

    #[test]
    fn load_default_deny_file() {
        let path = fixtures_dir().join("default-deny.toml");
        let set = load_policy_file(&path).expect("should load default-deny.toml");
        assert_eq!(set.rules.len(), 1);
        assert_eq!(set.rules[0].id, "default-deny");
        assert_eq!(set.rules[0].effect, Effect::Deny);
    }

    #[test]
    fn load_read_only_file() {
        let path = fixtures_dir().join("read-only.toml");
        let set = load_policy_file(&path).expect("should load read-only.toml");
        assert_eq!(set.rules.len(), 3);
    }

    #[test]
    fn load_developer_file() {
        let path = fixtures_dir().join("developer.toml");
        let set = load_policy_file(&path).expect("should load developer.toml");
        assert_eq!(set.rules.len(), 4);
        // The shell rule should have a condition.
        let shell_rule = set
            .rules
            .iter()
            .find(|r| r.id == "allow-shell")
            .expect("should have allow-shell rule");
        assert_eq!(
            shell_rule.conditions.get("max_timeout"),
            Some(&"30".to_string())
        );
    }

    #[test]
    fn load_operator_file() {
        let path = fixtures_dir().join("operator.toml");
        let set = load_policy_file(&path).expect("should load operator.toml");
        assert_eq!(set.rules.len(), 3);
        let signing_rule = set
            .rules
            .iter()
            .find(|r| r.id == "allow-signing")
            .expect("should have allow-signing rule");
        assert_eq!(
            signing_rule.conditions.get("require_approval"),
            Some(&"true".to_string())
        );
        assert_eq!(
            signing_rule.conditions.get("max_amount"),
            Some(&"1000000000000".to_string())
        );
    }

    #[test]
    fn load_policy_dir_merges_all() {
        let dir = fixtures_dir();
        let set = load_policy_dir(&dir).expect("should load policy directory");
        // Total rules across all five files:
        //   default-deny=1, developer=4, operator=3, read-only=3, time-limited=2 → 13
        assert_eq!(set.rules.len(), 13);

        // Deny rules should come before allow rules after merge.
        let first_allow_idx = set.rules.iter().position(|r| r.effect == Effect::Allow);
        let last_deny_idx = set.rules.iter().rposition(|r| r.effect == Effect::Deny);

        if let (Some(first_allow), Some(last_deny)) = (first_allow_idx, last_deny_idx) {
            assert!(
                last_deny < first_allow,
                "all deny rules should precede all allow rules after merge"
            );
        }
    }

    // ---- Default deny policy evaluation ------------------------------------

    #[test]
    fn default_deny_blocks_everything() {
        let path = fixtures_dir().join("default-deny.toml");
        let set = load_policy_file(&path).expect("load");
        let ctx = empty_ctx();

        // Any action on any resource should be denied.
        assert!(matches!(
            evaluate(&set, "chain.query", "anything", &ctx),
            PolicyDecision::Deny { .. }
        ));
        assert!(matches!(
            evaluate(&set, "chain.submit", "polkadot/tx", &ctx),
            PolicyDecision::Deny { .. }
        ));
        assert!(matches!(
            evaluate(&set, "file.write", "workspace/foo.rs", &ctx),
            PolicyDecision::Deny { .. }
        ));
        assert!(matches!(
            evaluate(&set, "memory.read", "store/123", &ctx),
            PolicyDecision::Deny { .. }
        ));
    }

    // ---- Read-only policy evaluation ---------------------------------------

    #[test]
    fn read_only_allows_queries() {
        let path = fixtures_dir().join("read-only.toml");
        let set = load_policy_file(&path).expect("load");
        let ctx = empty_ctx();

        assert_eq!(
            evaluate(&set, "chain.query", "any/resource", &ctx),
            PolicyDecision::Allow
        );
        assert_eq!(
            evaluate(&set, "chain.decode", "any/resource", &ctx),
            PolicyDecision::Allow
        );
        assert_eq!(
            evaluate(&set, "chain.balance", "any/resource", &ctx),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn read_only_allows_memory_reads() {
        let path = fixtures_dir().join("read-only.toml");
        let set = load_policy_file(&path).expect("load");
        let ctx = empty_ctx();

        assert_eq!(
            evaluate(&set, "memory.read", "store/123", &ctx),
            PolicyDecision::Allow
        );
        assert_eq!(
            evaluate(&set, "memory.search", "store/query", &ctx),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn read_only_blocks_writes() {
        let path = fixtures_dir().join("read-only.toml");
        let set = load_policy_file(&path).expect("load");
        let ctx = empty_ctx();

        assert!(matches!(
            evaluate(&set, "chain.submit", "polkadot/tx", &ctx),
            PolicyDecision::Deny { .. }
        ));
        assert!(matches!(
            evaluate(&set, "chain.sign", "key/alice", &ctx),
            PolicyDecision::Deny { .. }
        ));
        assert!(matches!(
            evaluate(&set, "file.write", "workspace/foo.rs", &ctx),
            PolicyDecision::Deny { .. }
        ));
        assert!(matches!(
            evaluate(&set, "shell.execute", "workspace/cmd", &ctx),
            PolicyDecision::Deny { .. }
        ));
    }

    #[test]
    fn read_only_denies_unlisted_actions() {
        let path = fixtures_dir().join("read-only.toml");
        let set = load_policy_file(&path).expect("load");
        let ctx = empty_ctx();

        // An action not covered by any rule should be denied (default deny).
        assert!(matches!(
            evaluate(&set, "chain.stake", "polkadot/pool", &ctx),
            PolicyDecision::Deny { .. }
        ));
    }

    // ---- Developer policy evaluation ---------------------------------------

    #[test]
    fn developer_allows_testnet_chain_ops() {
        let path = fixtures_dir().join("developer.toml");
        let set = load_policy_file(&path).expect("load");
        let ctx = empty_ctx();

        assert_eq!(
            evaluate(&set, "chain.query", "westend/some-resource", &ctx),
            PolicyDecision::Allow
        );
        assert_eq!(
            evaluate(&set, "chain.query", "rococo/some-resource", &ctx),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn developer_blocks_mainnet_submit() {
        let path = fixtures_dir().join("developer.toml");
        let set = load_policy_file(&path).expect("load");
        let ctx = empty_ctx();

        assert!(matches!(
            evaluate(&set, "chain.submit", "polkadot/tx", &ctx),
            PolicyDecision::Deny { .. }
        ));
        assert!(matches!(
            evaluate(&set, "chain.sign", "kusama/key", &ctx),
            PolicyDecision::Deny { .. }
        ));
    }

    #[test]
    fn developer_allows_file_ops_in_workspace() {
        let path = fixtures_dir().join("developer.toml");
        let set = load_policy_file(&path).expect("load");
        let ctx = empty_ctx();

        assert_eq!(
            evaluate(&set, "file.read", "workspace/src/main.rs", &ctx),
            PolicyDecision::Allow
        );
        assert_eq!(
            evaluate(&set, "file.write", "workspace/src/main.rs", &ctx),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn developer_denies_file_ops_outside_workspace() {
        let path = fixtures_dir().join("developer.toml");
        let set = load_policy_file(&path).expect("load");
        let ctx = empty_ctx();

        assert!(matches!(
            evaluate(&set, "file.read", "/etc/passwd", &ctx),
            PolicyDecision::Deny { .. }
        ));
    }

    #[test]
    fn developer_shell_requires_condition() {
        let path = fixtures_dir().join("developer.toml");
        let set = load_policy_file(&path).expect("load");

        // Without the condition, shell should be denied (condition doesn't match).
        let ctx_no_cond = empty_ctx();
        assert!(matches!(
            evaluate(&set, "shell.execute", "workspace/cmd", &ctx_no_cond),
            PolicyDecision::Deny { .. }
        ));

        // With the matching condition, shell should be allowed.
        let ctx_with_cond = ctx_with(&[("max_timeout", "30")]);
        assert_eq!(
            evaluate(&set, "shell.execute", "workspace/cmd", &ctx_with_cond),
            PolicyDecision::Allow
        );
    }

    // ---- Operator policy evaluation ----------------------------------------

    #[test]
    fn operator_allows_all_chain_ops() {
        let path = fixtures_dir().join("operator.toml");
        let set = load_policy_file(&path).expect("load");
        let ctx = empty_ctx();

        assert_eq!(
            evaluate(&set, "chain.query", "polkadot/any", &ctx),
            PolicyDecision::Allow
        );
        assert_eq!(
            evaluate(&set, "chain.submit", "kusama/tx", &ctx),
            PolicyDecision::Allow
        );
        assert_eq!(
            evaluate(&set, "chain.transfer", "westend/account", &ctx),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn operator_signing_requires_conditions() {
        let path = fixtures_dir().join("operator.toml");
        let set = load_policy_file(&path).expect("load");

        // Without conditions, signing should be denied.
        let ctx_none = empty_ctx();
        assert!(matches!(
            evaluate(&set, "signer.sign", "key/alice", &ctx_none),
            PolicyDecision::Deny { .. }
        ));

        // With matching conditions, signing should be allowed.
        let ctx_match = ctx_with(&[
            ("require_approval", "true"),
            ("max_amount", "1000000000000"),
        ]);
        assert_eq!(
            evaluate(&set, "signer.sign", "key/alice", &ctx_match),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn operator_allows_memory_ops() {
        let path = fixtures_dir().join("operator.toml");
        let set = load_policy_file(&path).expect("load");
        let ctx = empty_ctx();

        assert_eq!(
            evaluate(&set, "memory.read", "store/123", &ctx),
            PolicyDecision::Allow
        );
        assert_eq!(
            evaluate(&set, "memory.write", "store/456", &ctx),
            PolicyDecision::Allow
        );
        assert_eq!(
            evaluate(&set, "memory.search", "store/query", &ctx),
            PolicyDecision::Allow
        );
    }

    // ---- Time-limited policy evaluation ------------------------------------

    #[test]
    fn time_limited_loads_successfully() {
        let path = fixtures_dir().join("time-limited.toml");
        let set = load_policy_file(&path).expect("should load time-limited.toml");
        assert_eq!(set.rules.len(), 2, "time-limited should have 2 rules");
    }

    #[test]
    fn time_limited_abac_condition_is_loaded() {
        let path = fixtures_dir().join("time-limited.toml");
        let set = load_policy_file(&path).expect("load time-limited.toml");
        let allow_rule = set
            .rules
            .iter()
            .find(|r| r.effect == Effect::Allow)
            .expect("should have an allow rule");
        assert!(
            allow_rule.abac_condition.is_some(),
            "time-limited allow rule should have an ABAC condition"
        );
    }

    #[test]
    fn time_limited_allows_in_business_hours() {
        let path = fixtures_dir().join("time-limited.toml");
        let set = load_policy_file(&path).expect("load");

        let ctx = EvaluationContext::default().with_attribute(
            "environment.time_window",
            ContextAttribute::String("business_hours".to_string()),
        );

        assert_eq!(
            evaluate(&set, "chain.query", "any", &ctx),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn time_limited_denies_outside_business_hours() {
        let path = fixtures_dir().join("time-limited.toml");
        let set = load_policy_file(&path).expect("load");

        // No time_window set → chain.submit should be denied.
        let ctx = EvaluationContext::default();
        assert!(matches!(
            evaluate(&set, "chain.submit", "any", &ctx),
            PolicyDecision::Deny { .. }
        ));

        // Wrong window → submit still denied (deny rule fires).
        let ctx_wrong = EvaluationContext::default().with_attribute(
            "environment.time_window",
            ContextAttribute::String("off_hours".to_string()),
        );
        assert!(matches!(
            evaluate(&set, "chain.submit", "any", &ctx_wrong),
            PolicyDecision::Deny { .. }
        ));
    }

    // ---- ABAC condition in TOML (inline table) ------------------------------

    #[test]
    fn load_rule_with_abac_condition_from_toml() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("abac.toml");

        let content = r#"
[[rules]]
id = "abac-allow"
effect = "allow"
action_patterns = ["chain.*"]
resource_patterns = ["**"]

[rules.abac_condition]
op = "equals"
attr = "principal.role"
value = "developer"
"#;
        std::fs::write(&path, content).expect("write toml");

        let set = load_policy_file(&path).expect("load abac toml");
        assert_eq!(set.rules.len(), 1);
        let rule = &set.rules[0];
        assert!(rule.abac_condition.is_some());
        assert!(matches!(
            rule.abac_condition.as_ref().unwrap(),
            Condition::Equals { attr, value }
            if attr == "principal.role" && value == "developer"
        ));
    }

    // ---- Merge tests -------------------------------------------------------

    #[test]
    fn merge_empty_sets() {
        let merged = merge_policy_sets(vec![]);
        assert!(merged.rules.is_empty());
    }

    #[test]
    fn merge_single_set() {
        let set = PolicySet::new(vec![PolicyRule {
            id: "test".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["**".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        }]);
        let merged = merge_policy_sets(vec![set]);
        assert_eq!(merged.rules.len(), 1);
    }

    #[test]
    fn merge_deny_before_allow() {
        let set_a = PolicySet::new(vec![PolicyRule {
            id: "allow-a".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["a.*".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        }]);
        let set_b = PolicySet::new(vec![PolicyRule {
            id: "deny-b".to_string(),
            effect: Effect::Deny,
            action_patterns: vec!["b.*".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        }]);

        let merged = merge_policy_sets(vec![set_a, set_b]);
        assert_eq!(merged.rules.len(), 2);
        // Deny rule should be first despite coming from the second set.
        assert_eq!(merged.rules[0].effect, Effect::Deny);
        assert_eq!(merged.rules[1].effect, Effect::Allow);
    }

    // ---- Error handling tests ----------------------------------------------

    #[test]
    fn load_nonexistent_file_returns_io_error() {
        let result = load_policy_file(Path::new("/nonexistent/policy.toml"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, LoadError::Io { .. }));
    }

    #[test]
    fn load_nonexistent_dir_returns_error() {
        let result = load_policy_dir(Path::new("/nonexistent/policies"));
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LoadError::ReadDir { .. }));
    }

    #[test]
    fn load_invalid_toml_returns_parse_error() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is not valid { toml").expect("write");

        let result = load_policy_file(&path);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LoadError::Parse { .. }));
    }
}

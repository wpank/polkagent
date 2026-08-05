//! Skill resolution integration tests.
//!
//! Exercises TOML manifest loading, dependency resolution (topological sort),
//! circular dependency detection, and missing/version-conflict dependency errors.

// This assertion-oriented integration target uses `expect`/`unwrap` to identify
// the exact cross-crate fixture step or behavioral contract that failed.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;

use polkagent_skill::error::SkillError;
use polkagent_skill::manifest::{
    CapabilitiesSection, DependencySpec, PromptsSection, SkillId, SkillManifest, SkillSection,
};
use polkagent_skill::resolver::resolve;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `SkillManifest` programmatically with optional dependencies.
fn make_manifest(name: &str, version: &str, deps: &[(&str, &str)]) -> SkillManifest {
    let mut dependencies = HashMap::new();
    for (dep_name, ver_req) in deps {
        dependencies.insert(
            dep_name.to_string(),
            DependencySpec {
                version: ver_req.to_string(),
            },
        );
    }
    SkillManifest {
        skill: SkillSection {
            name: name.to_string(),
            version: version.to_string(),
            description: String::new(),
            authors: vec![],
            license: String::new(),
        },
        capabilities: CapabilitiesSection::default(),
        prompts: PromptsSection::default(),
        config: HashMap::new(),
        dependencies,
    }
}

// ---------------------------------------------------------------------------
// TOML manifest loading
// ---------------------------------------------------------------------------

#[test]
fn parse_minimal_toml_manifest() {
    let toml = r#"
[skill]
name = "simple"
version = "1.0.0"
"#;
    let manifest = SkillManifest::from_toml(toml).expect("should parse");
    assert_eq!(manifest.skill.name, "simple");
    assert_eq!(manifest.skill.version, "1.0.0");
    assert!(manifest.capabilities.required_grants.is_empty());
    assert!(manifest.capabilities.tools.is_empty());
    assert!(manifest.dependencies.is_empty());
}

#[test]
fn parse_full_toml_manifest() {
    let toml = r#"
[skill]
name = "governance-researcher"
version = "0.2.0"
description = "Research OpenGov proposals"
authors = ["alice@example.com"]
license = "MIT"

[capabilities]
required_grants = ["chain.query", "memory.read"]
tools = ["chain_query", "search_memory"]

[prompts]
system = "You are a governance assistant."

[config]
default_chain = "kusama"
"#;
    let manifest = SkillManifest::from_toml(toml).expect("should parse");
    assert_eq!(manifest.skill.name, "governance-researcher");
    assert_eq!(manifest.skill.description, "Research OpenGov proposals");
    assert_eq!(
        manifest.capabilities.required_grants,
        vec!["chain.query", "memory.read"]
    );
    assert_eq!(
        manifest.capabilities.tools,
        vec!["chain_query", "search_memory"]
    );
    assert_eq!(manifest.prompts.system, "You are a governance assistant.");
    assert_eq!(
        manifest
            .config
            .get("default_chain")
            .and_then(|v| v.as_str()),
        Some("kusama")
    );
}

#[test]
fn parse_manifest_with_dependencies() {
    let toml = r#"
[skill]
name = "my-skill"
version = "1.0.0"

[dependencies]
chain-utils = { version = ">=0.2.0" }
memory-helper = { version = "^1.0" }
"#;
    let manifest = SkillManifest::from_toml(toml).expect("should parse");
    assert_eq!(manifest.dependencies.len(), 2);
    assert_eq!(manifest.dependencies["chain-utils"].version, ">=0.2.0");
    assert_eq!(manifest.dependencies["memory-helper"].version, "^1.0");
}

#[test]
fn parse_manifest_missing_name_fails() {
    let toml = r#"
[skill]
version = "1.0.0"
"#;
    assert!(
        SkillManifest::from_toml(toml).is_err(),
        "manifest without name must fail to parse"
    );
}

#[test]
fn parse_manifest_empty_name_fails() {
    let toml = r#"
[skill]
name = ""
version = "1.0.0"
"#;
    let result = SkillManifest::from_toml(toml);
    assert!(result.is_err(), "empty skill name must fail validation");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("must not be empty"),
        "error must mention empty name: {msg}"
    );
}

#[test]
fn parse_manifest_invalid_name_characters_fails() {
    let toml = r#"
[skill]
name = "My Skill!"
version = "1.0.0"
"#;
    let result = SkillManifest::from_toml(toml);
    assert!(
        result.is_err(),
        "uppercase / special chars in name must fail"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("lowercase ASCII"),
        "error must mention character restriction: {msg}"
    );
}

#[test]
fn parse_manifest_invalid_version_fails() {
    let toml = r#"
[skill]
name = "good-name"
version = "not-semver"
"#;
    let result = SkillManifest::from_toml(toml);
    assert!(result.is_err(), "invalid semver version must fail");
}

#[test]
fn parse_manifest_invalid_dependency_version_req_fails() {
    let toml = r#"
[skill]
name = "my-skill"
version = "1.0.0"

[dependencies]
bad-dep = { version = "not a valid semver requirement !!!" }
"#;
    let result = SkillManifest::from_toml(toml);
    assert!(result.is_err(), "invalid dep version requirement must fail");
}

#[test]
fn parse_invalid_toml_fails() {
    let result = SkillManifest::from_toml("{{{{ not valid toml");
    assert!(result.is_err(), "invalid TOML must fail");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("parse"),
        "error must mention parse failure: {msg}"
    );
}

// ---------------------------------------------------------------------------
// SkillId
// ---------------------------------------------------------------------------

#[test]
fn skill_id_parse_valid() {
    let id = SkillId::parse("my-skill", "1.2.3").expect("should parse");
    assert_eq!(id.name, "my-skill");
    assert_eq!(id.version.major, 1);
    assert_eq!(id.version.minor, 2);
    assert_eq!(id.version.patch, 3);
}

#[test]
fn skill_id_parse_invalid_version_fails() {
    let result = SkillId::parse("my-skill", "not-semver");
    assert!(result.is_err(), "invalid version must fail SkillId::parse");
}

#[test]
fn skill_id_display_format() {
    let id = SkillId::parse("my-skill", "2.0.0").expect("parse");
    assert_eq!(id.to_string(), "my-skill@2.0.0");
}

#[test]
fn skill_id_equality() {
    let a = SkillId::parse("x", "1.0.0").expect("a");
    let b = SkillId::parse("x", "1.0.0").expect("b");
    let c = SkillId::parse("x", "2.0.0").expect("c");
    assert_eq!(a, b, "same name and version must be equal");
    assert_ne!(a, c, "different versions must not be equal");
}

#[test]
fn manifest_id_extracts_correct_skill_id() {
    let manifest = make_manifest("my-skill", "3.1.4", &[]);
    let id = manifest.id().expect("id");
    assert_eq!(id.name, "my-skill");
    assert_eq!(id.to_string(), "my-skill@3.1.4");
}

// ---------------------------------------------------------------------------
// Dependency resolution — success cases
// ---------------------------------------------------------------------------

#[test]
fn resolve_empty_list_returns_empty() {
    let order = resolve(&[]).expect("resolve empty");
    assert!(order.is_empty());
}

#[test]
fn resolve_single_skill_no_dependencies() {
    let a = make_manifest("alpha", "1.0.0", &[]);
    let order = resolve(&[a]).expect("resolve");
    assert_eq!(order.len(), 1);
    assert_eq!(order[0].name, "alpha");
}

#[test]
fn resolve_two_independent_skills_both_present() {
    let a = make_manifest("alpha", "1.0.0", &[]);
    let b = make_manifest("beta", "2.0.0", &[]);

    let order = resolve(&[a, b]).expect("resolve");
    assert_eq!(order.len(), 2);

    let names: Vec<&str> = order.iter().map(|id| id.name.as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
}

#[test]
fn resolve_two_independent_skills_lexicographic_order() {
    // When there are no dependencies the resolver uses lexicographic order.
    let a = make_manifest("alpha", "1.0.0", &[]);
    let b = make_manifest("beta", "2.0.0", &[]);

    let order = resolve(&[b, a]).expect("resolve"); // note: reversed input
    assert_eq!(
        order[0].name, "alpha",
        "alpha must come before beta lexicographically"
    );
}

#[test]
fn resolve_linear_chain_produces_dependency_first_order() {
    // c depends on b, b depends on a.
    let a = make_manifest("a", "1.0.0", &[]);
    let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);
    let c = make_manifest("c", "1.0.0", &[("b", "^1.0")]);

    let order = resolve(&[c, a, b]).expect("resolve");
    let names: Vec<&str> = order.iter().map(|id| id.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["a", "b", "c"],
        "must resolve in dependency-first order"
    );
}

#[test]
fn resolve_diamond_dependency_graph() {
    //   a
    //  / \
    // b   c
    //  \ /
    //   d
    let a = make_manifest("a", "1.0.0", &[]);
    let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);
    let c = make_manifest("c", "1.0.0", &[("a", "^1.0")]);
    let d = make_manifest("d", "1.0.0", &[("b", "^1.0"), ("c", "^1.0")]);

    let order = resolve(&[d, b, c, a]).expect("resolve diamond");

    let names: Vec<&str> = order.iter().map(|id| id.name.as_str()).collect();
    let pos_a = names.iter().position(|&n| n == "a").expect("a");
    let pos_b = names.iter().position(|&n| n == "b").expect("b");
    let pos_c = names.iter().position(|&n| n == "c").expect("c");
    let pos_d = names.iter().position(|&n| n == "d").expect("d");

    assert!(pos_a < pos_b, "a must come before b");
    assert!(pos_a < pos_c, "a must come before c");
    assert!(pos_b < pos_d, "b must come before d");
    assert!(pos_c < pos_d, "c must come before d");
}

#[test]
fn resolve_version_requirement_satisfied_by_compatible_version() {
    // b requires a ^1.0; 1.5.0 satisfies ^1.0.
    let a = make_manifest("a", "1.5.0", &[]);
    let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);

    let order = resolve(&[a, b]).expect("resolve");
    assert_eq!(order.len(), 2);
    assert_eq!(order[0].name, "a");
    assert_eq!(order[1].name, "b");
}

// ---------------------------------------------------------------------------
// Dependency resolution — error cases
// ---------------------------------------------------------------------------

#[test]
fn resolve_missing_dependency_returns_skill_not_found_error() {
    let a = make_manifest("a", "1.0.0", &[("nonexistent-dep", "^1.0")]);

    let result = resolve(&[a]);
    assert!(result.is_err(), "missing dependency must fail resolution");

    let err = result.unwrap_err();
    assert!(
        matches!(err, SkillError::SkillNotFound { .. }),
        "error must be SkillNotFound, got: {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("nonexistent-dep") || msg.contains("not found"),
        "error must name the missing skill: {msg}"
    );
}

#[test]
fn resolve_version_conflict_returns_version_conflict_error() {
    // b requires a >= 2.0.0 but only 1.0.0 is available.
    let a = make_manifest("a", "1.0.0", &[]);
    let b = make_manifest("b", "1.0.0", &[("a", ">=2.0.0")]);

    let result = resolve(&[a, b]);
    assert!(result.is_err(), "version conflict must fail resolution");

    let err = result.unwrap_err();
    assert!(
        matches!(err, SkillError::VersionConflict { .. }),
        "error must be VersionConflict, got: {err:?}"
    );
}

#[test]
fn resolve_simple_two_node_cycle_detected() {
    // a -> b -> a
    let a = make_manifest("a", "1.0.0", &[("b", "^1.0")]);
    let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);

    let result = resolve(&[a, b]);
    assert!(result.is_err(), "two-node cycle must fail resolution");

    let err = result.unwrap_err();
    assert!(
        matches!(err, SkillError::CyclicDependency { .. }),
        "error must be CyclicDependency, got: {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("cyclic") || msg.contains("cycle"),
        "error message must mention cycle: {msg}"
    );
}

#[test]
fn resolve_three_node_cycle_detected() {
    // a -> b -> c -> a
    let a = make_manifest("a", "1.0.0", &[("c", "^1.0")]);
    let b = make_manifest("b", "1.0.0", &[("a", "^1.0")]);
    let c = make_manifest("c", "1.0.0", &[("b", "^1.0")]);

    let result = resolve(&[a, b, c]);
    assert!(result.is_err(), "three-node cycle must fail resolution");

    let err = result.unwrap_err();
    assert!(
        matches!(err, SkillError::CyclicDependency { .. }),
        "error must be CyclicDependency: {err:?}"
    );
}

#[test]
fn resolve_self_dependency_cycle_detected() {
    // a depends on itself.
    let a = make_manifest("a", "1.0.0", &[("a", "^1.0")]);

    let result = resolve(&[a]);
    assert!(
        result.is_err(),
        "self-dependency must be detected as a cycle"
    );
}

#[test]
fn resolve_partial_graph_with_cycle_identifies_cyclic_nodes() {
    // normal -> a and b -> c -> b (cycle in subset)
    let normal = make_manifest("normal", "1.0.0", &[]);
    let a = make_manifest("a", "1.0.0", &[("normal", "^1.0")]);
    let b = make_manifest("b", "1.0.0", &[("c", "^1.0")]);
    let c = make_manifest("c", "1.0.0", &[("b", "^1.0")]);

    let result = resolve(&[normal, a, b, c]);
    assert!(result.is_err(), "partial cycle must still fail resolution");
}

// ---------------------------------------------------------------------------
// Manifest round-trip from TOML
// ---------------------------------------------------------------------------

#[test]
fn toml_manifest_id_matches_expected_skill_id() {
    let toml = r#"
[skill]
name = "my-skill"
version = "2.1.0"
"#;
    let manifest = SkillManifest::from_toml(toml).expect("parse");
    let id = manifest.id().expect("id");
    assert_eq!(id.name, "my-skill");
    assert_eq!(id.version.to_string(), "2.1.0");
}

#[test]
fn resolve_toml_manifests_end_to_end() {
    let lib_toml = r#"
[skill]
name = "lib-a"
version = "1.0.0"
description = "A shared library skill"
"#;

    let app_toml = r#"
[skill]
name = "app-b"
version = "1.0.0"
description = "Application that depends on lib-a"

[dependencies]
lib-a = { version = "^1.0" }
"#;

    let lib = SkillManifest::from_toml(lib_toml).expect("parse lib");
    let app = SkillManifest::from_toml(app_toml).expect("parse app");

    let order = resolve(&[app, lib]).expect("resolve end-to-end");
    let names: Vec<&str> = order.iter().map(|id| id.name.as_str()).collect();

    assert_eq!(names[0], "lib-a", "lib must come before app");
    assert_eq!(names[1], "app-b");
}

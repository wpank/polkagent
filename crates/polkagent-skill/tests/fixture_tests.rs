//! Integration tests that load skill manifest fixture files from the workspace
//! `fixtures/skills/` directory and verify all fields are parsed correctly.

use std::path::PathBuf;

use polkagent_skill::manifest::SkillManifest;

/// Return the absolute path to a fixture file under `fixtures/skills/`.
fn fixture_path(rel: &str) -> PathBuf {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate is inside crates/")
        .parent()
        .expect("crates/ is inside workspace root")
        .to_path_buf();
    workspace_root.join("fixtures").join("skills").join(rel)
}

/// Read and parse a skill manifest TOML file.
fn load_manifest(rel: &str) -> SkillManifest {
    let path = fixture_path(rel);
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    SkillManifest::from_toml(&content)
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// balance-checker/skill.toml
// ---------------------------------------------------------------------------

#[test]
fn balance_checker_manifest_parses() {
    let manifest = load_manifest("balance-checker/skill.toml");

    // Skill identity.
    assert_eq!(manifest.skill.name, "balance-checker");
    assert_eq!(manifest.skill.version, "0.1.0");
    assert_eq!(
        manifest.skill.description,
        "Check token balances across chains"
    );
    assert_eq!(manifest.skill.authors, vec!["team@polkagent.io"]);
    assert_eq!(manifest.skill.license, "Apache-2.0");
}

#[test]
fn balance_checker_capabilities_are_correct() {
    let manifest = load_manifest("balance-checker/skill.toml");

    assert_eq!(manifest.capabilities.required_grants, vec!["chain.query"]);
    assert_eq!(manifest.capabilities.tools, vec!["chain_query"]);
}

#[test]
fn balance_checker_prompts_are_correct() {
    let manifest = load_manifest("balance-checker/skill.toml");

    assert_eq!(
        manifest.prompts.system,
        "You check token balances. Query the chain for balance information and report it clearly."
    );
}

#[test]
fn balance_checker_config_has_default_chain() {
    let manifest = load_manifest("balance-checker/skill.toml");

    assert_eq!(
        manifest.config.get("default_chain").and_then(|v| v.as_str()),
        Some("polkadot")
    );
}

#[test]
fn balance_checker_skill_id_is_correct() {
    let manifest = load_manifest("balance-checker/skill.toml");
    let id = manifest.id().expect("should produce a valid SkillId");

    assert_eq!(id.name, "balance-checker");
    assert_eq!(id.version, semver::Version::new(0, 1, 0));
    assert_eq!(id.to_string(), "balance-checker@0.1.0");
}

#[test]
fn balance_checker_has_no_dependencies() {
    let manifest = load_manifest("balance-checker/skill.toml");
    assert!(
        manifest.dependencies.is_empty(),
        "balance-checker should have no dependencies"
    );
}

// ---------------------------------------------------------------------------
// Round-trip serialization
// ---------------------------------------------------------------------------

#[test]
fn balance_checker_round_trips_through_toml() {
    let manifest = load_manifest("balance-checker/skill.toml");

    // Serialize back to TOML.
    let serialized = toml::to_string_pretty(&manifest).expect("serialize to TOML");

    // Parse again from the serialized string.
    let reparsed = SkillManifest::from_toml(&serialized).expect("reparse from serialized TOML");

    // All fields should match.
    assert_eq!(manifest.skill.name, reparsed.skill.name);
    assert_eq!(manifest.skill.version, reparsed.skill.version);
    assert_eq!(manifest.skill.description, reparsed.skill.description);
    assert_eq!(manifest.skill.authors, reparsed.skill.authors);
    assert_eq!(manifest.skill.license, reparsed.skill.license);
    assert_eq!(
        manifest.capabilities.required_grants,
        reparsed.capabilities.required_grants
    );
    assert_eq!(manifest.capabilities.tools, reparsed.capabilities.tools);
    assert_eq!(manifest.prompts.system, reparsed.prompts.system);
    assert_eq!(manifest.config, reparsed.config);
}

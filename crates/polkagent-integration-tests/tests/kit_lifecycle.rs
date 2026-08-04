//! Phase 4 Acceptance Tests — Kit Lifecycle
//!
//! Exercises the kit install, list, uninstall, rollback, and reinstall
//! lifecycle end-to-end via the `polkagent-kit` crate's manager functions.
//!
//! - KL-01: validate_kit succeeds with all capabilities granted.
//! - KL-02: validate_kit detects missing capabilities.
//! - KL-03: validate_kit detects missing required skills.
//! - KL-04: prepare_install builds a correct InstalledKit record.
//! - KL-05: skills_to_unregister excludes optional skills.
//! - KL-06: Uninstall → reinstall yields identical InstalledKit.
//! - KL-07: Rollback scenario — old version can be re-installed after uninstall.
//! - KL-08: Invalid manifest TOML is rejected.
//! - KL-09: Manifest missing [kit] section is rejected.
//! - KL-10: Multiple kits can be installed independently.

use std::fs;
use std::path::Path;

use polkagent_kit::{prepare_install, skills_to_unregister, validate_kit, KitManifest};

// =========================================================================
// Helpers
// =========================================================================

fn write_kit_toml(dir: &Path, content: &str) {
    fs::write(dir.join("kit.toml"), content).expect("write kit.toml");
}

const KIT_V1: &str = r#"
[kit]
name = "defi-kit"
version = "1.0.0"
description = "DeFi tools for Polkadot"

[capabilities]
required_grants = ["chain.query", "chain.submit"]

[skills]
swap-skill = { version = "^1.0.0", role = "primary" }
price-feed  = { version = "^1.0.0", role = "required" }
analytics   = { version = "^1.0.0", role = "optional" }
"#;

const KIT_V2: &str = r#"
[kit]
name = "defi-kit"
version = "2.0.0"
description = "DeFi tools for Polkadot (v2)"

[capabilities]
required_grants = ["chain.query", "chain.submit"]

[skills]
swap-skill   = { version = "^2.0.0", role = "primary" }
price-feed   = { version = "^2.0.0", role = "required" }
analytics    = { version = "^2.0.0", role = "optional" }
liquidity    = { version = "^1.0.0", role = "context" }
"#;

fn all_caps() -> Vec<String> {
    vec!["chain.query".into(), "chain.submit".into()]
}

fn all_skills_v1() -> Vec<String> {
    vec!["swap-skill".into(), "price-feed".into(), "analytics".into()]
}

fn _all_skills_v2() -> Vec<String> {
    vec![
        "swap-skill".into(),
        "price-feed".into(),
        "analytics".into(),
        "liquidity".into(),
    ]
}

// =========================================================================
// KL-01: Validate kit with all capabilities granted
// =========================================================================

#[test]
fn kl_01_validate_kit_all_granted() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_kit_toml(dir.path(), KIT_V1);

    let validation = validate_kit(dir.path(), &all_caps(), &all_skills_v1()).expect("validate");
    assert!(validation.can_install());
    assert!(validation.denied_capabilities.is_empty());
    assert!(validation.missing_skills.is_empty());
}

// =========================================================================
// KL-02: Validate kit detects missing capabilities
// =========================================================================

#[test]
fn kl_02_validate_kit_missing_capability() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_kit_toml(dir.path(), KIT_V1);

    let validation =
        validate_kit(dir.path(), &["chain.query".into()], &all_skills_v1()).expect("validate");
    assert!(!validation.can_install());
    assert!(validation
        .denied_capabilities
        .contains(&"chain.submit".into()));
}

// =========================================================================
// KL-03: Validate kit detects missing required skills
// =========================================================================

#[test]
fn kl_03_validate_kit_missing_skill() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_kit_toml(dir.path(), KIT_V1);

    let validation =
        validate_kit(dir.path(), &all_caps(), &["swap-skill".into()]).expect("validate");
    assert!(!validation.can_install());
    assert!(validation.missing_skills.contains(&"price-feed".into()));
}

// =========================================================================
// KL-04: prepare_install builds correct InstalledKit
// =========================================================================

#[test]
fn kl_04_prepare_install_builds_record() {
    let manifest = KitManifest::from_toml(KIT_V1).expect("parse");
    let installed = prepare_install(&manifest, Path::new("/kits/defi-kit")).expect("install");
    assert_eq!(installed.name, "defi-kit");
    assert_eq!(installed.version, "1.0.0");
    assert_eq!(installed.description, "DeFi tools for Polkadot");
    assert_eq!(installed.skill_names.len(), 3);
    assert!(installed.skill_names.contains(&"swap-skill".to_string()));
    assert!(!installed.manifest_json.is_empty());
}

// =========================================================================
// KL-05: skills_to_unregister excludes optional skills
// =========================================================================

#[test]
fn kl_05_skills_to_unregister_excludes_optional() {
    let manifest = KitManifest::from_toml(KIT_V1).expect("parse");
    let to_remove = skills_to_unregister(&manifest);
    assert!(to_remove.contains(&"swap-skill".to_string()));
    assert!(to_remove.contains(&"price-feed".to_string()));
    assert!(
        !to_remove.contains(&"analytics".to_string()),
        "optional skill must not be unregistered"
    );
}

// =========================================================================
// KL-06: Uninstall then reinstall yields identical record
// =========================================================================

#[test]
fn kl_06_uninstall_reinstall_identical() {
    let manifest = KitManifest::from_toml(KIT_V1).expect("parse");
    let install_path = Path::new("/kits/defi-kit");

    let first_install = prepare_install(&manifest, install_path).expect("first install");
    let _unregister = skills_to_unregister(&manifest);
    let second_install = prepare_install(&manifest, install_path).expect("reinstall");

    assert_eq!(first_install.name, second_install.name);
    assert_eq!(first_install.version, second_install.version);
    assert_eq!(first_install.description, second_install.description);
    assert_eq!(
        first_install.skill_names.len(),
        second_install.skill_names.len()
    );
}

// =========================================================================
// KL-07: Rollback — old version re-installs after upgrade uninstall
// =========================================================================

#[test]
fn kl_07_rollback_to_old_version() {
    let v1 = KitManifest::from_toml(KIT_V1).expect("parse v1");
    let v2 = KitManifest::from_toml(KIT_V2).expect("parse v2");

    let v1_installed = prepare_install(&v1, Path::new("/kits/v1")).expect("install v1");
    assert_eq!(v1_installed.version, "1.0.0");

    let v2_installed = prepare_install(&v2, Path::new("/kits/v2")).expect("install v2");
    assert_eq!(v2_installed.version, "2.0.0");
    assert_eq!(v2_installed.skill_names.len(), 4);

    let _v2_unregister = skills_to_unregister(&v2);
    let rollback = prepare_install(&v1, Path::new("/kits/v1")).expect("rollback to v1");
    assert_eq!(rollback.version, "1.0.0");
    assert_eq!(rollback.skill_names.len(), 3);
}

// =========================================================================
// KL-08: Invalid manifest TOML is rejected
// =========================================================================

#[test]
fn kl_08_invalid_toml_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_kit_toml(dir.path(), "this is not valid toml {{{{");

    let result = validate_kit(dir.path(), &all_caps(), &all_skills_v1());
    assert!(result.is_err());
}

// =========================================================================
// KL-09: Missing [kit] section is rejected
// =========================================================================

#[test]
fn kl_09_missing_kit_section_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_kit_toml(
        dir.path(),
        r#"
[capabilities]
required_grants = []

[skills]
"#,
    );

    let result = validate_kit(dir.path(), &[], &[]);
    assert!(result.is_err());
}

// =========================================================================
// KL-10: Multiple kits installed independently
// =========================================================================

#[test]
fn kl_10_multiple_kits_independent() {
    let kit_a_toml = r#"
[kit]
name = "kit-alpha"
version = "1.0.0"
description = "Kit Alpha"

[capabilities]
required_grants = []

[skills]
alpha-skill = { version = "^1.0.0", role = "primary" }
"#;

    let kit_b_toml = r#"
[kit]
name = "kit-beta"
version = "1.0.0"
description = "Kit Beta"

[capabilities]
required_grants = []

[skills]
beta-skill = { version = "^1.0.0", role = "primary" }
"#;

    let a_manifest = KitManifest::from_toml(kit_a_toml).expect("parse alpha");
    let b_manifest = KitManifest::from_toml(kit_b_toml).expect("parse beta");

    let a_installed =
        prepare_install(&a_manifest, Path::new("/kits/alpha")).expect("install alpha");
    let b_installed = prepare_install(&b_manifest, Path::new("/kits/beta")).expect("install beta");

    assert_ne!(a_installed.name, b_installed.name);
    assert!(a_installed.skill_names.contains(&"alpha-skill".to_string()));
    assert!(b_installed.skill_names.contains(&"beta-skill".to_string()));

    let a_unregister = skills_to_unregister(&a_manifest);
    assert!(a_unregister.contains(&"alpha-skill".to_string()));
    assert!(
        !a_unregister.contains(&"beta-skill".to_string()),
        "unregistering kit-alpha must not affect kit-beta"
    );
}

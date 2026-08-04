//! Kit manifest: the declarative description of a product kit.
//!
//! A kit manifest is read from a `kit.toml` file at the root of a kit
//! directory. It bundles multiple skills (and other packages) into a single
//! installable unit with shared policy and configuration defaults.
//!
//! # Example TOML
//!
//! ```toml
//! [kit]
//! name = "governance-scout-kit"
//! version = "1.0.0"
//! description = "Complete governance research agent for Polkadot OpenGov."
//! authors = ["author@example.com"]
//! license = "Apache-2.0"
//!
//! [capabilities]
//! required_grants = ["chain.query", "memory.read"]
//!
//! [skills]
//! governance-analysis-skill = { version = "^1.3.0", role = "primary" }
//! referendum-summary-skill  = { version = "^1.0.0", role = "primary" }
//! chain-reader-tool         = { version = "^2.0.0", role = "required" }
//! document-formatter-tool   = { version = "^1.0.0", role = "optional" }
//!
//! [defaults]
//! target_networks = ["polkadot", "kusama"]
//! auto_activate = true
//!
//! [policy]
//! chain_write = false
//! data_classification = "Public"
//!
//! [ux]
//! display_name = "Governance Scout"
//! short_description = "Research OpenGov referenda with cited evidence."
//! category = "governance"
//! ```

use std::collections::HashMap;
use std::fmt;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::error::KitError;

// ---------------------------------------------------------------------------
// KitId
// ---------------------------------------------------------------------------

/// A unique identifier for a kit, composed of a name and a semver version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KitId {
    pub name: String,
    pub version: Version,
}

impl KitId {
    pub fn new(name: impl Into<String>, version: Version) -> Self {
        Self {
            name: name.into(),
            version,
        }
    }

    pub fn parse(name: impl Into<String>, version: &str) -> Result<Self, KitError> {
        let name = name.into();
        let version = Version::parse(version).map_err(|e| KitError::InvalidVersion {
            version: version.to_string(),
            reason: e.to_string(),
        })?;
        Ok(Self { name, version })
    }
}

impl fmt::Display for KitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

// ---------------------------------------------------------------------------
// SkillRole
// ---------------------------------------------------------------------------

/// The role of a skill within a kit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillRole {
    /// Core skill — the kit's primary functionality.
    Primary,
    /// Must be present for the kit to function.
    #[default]
    Required,
    /// Enhances the kit but is not mandatory.
    Optional,
    /// Provides context data (schemas, profiles, etc.).
    Context,
}

impl fmt::Display for SkillRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primary => write!(f, "primary"),
            Self::Required => write!(f, "required"),
            Self::Optional => write!(f, "optional"),
            Self::Context => write!(f, "context"),
        }
    }
}

// ---------------------------------------------------------------------------
// KitManifest — the full manifest structure
// ---------------------------------------------------------------------------

/// The top-level kit manifest structure, deserialized from `kit.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KitManifest {
    /// Core identity section.
    pub kit: KitSection,

    /// Capability requirements (grants the kit needs).
    #[serde(default)]
    pub capabilities: KitCapabilities,

    /// Skills bundled in this kit.
    #[serde(default)]
    pub skills: HashMap<String, SkillEntry>,

    /// Default configuration values.
    #[serde(default)]
    pub defaults: KitDefaults,

    /// Policy constraints.
    #[serde(default)]
    pub policy: KitPolicy,

    /// UX / display metadata.
    #[serde(default)]
    pub ux: KitUx,
}

/// The `[kit]` section of the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KitSection {
    /// The package name (kebab-case).
    pub name: String,

    /// The semver version string.
    pub version: String,

    /// A human-readable description.
    #[serde(default)]
    pub description: String,

    /// Author email addresses or names.
    #[serde(default)]
    pub authors: Vec<String>,

    /// SPDX license identifier.
    #[serde(default)]
    pub license: String,
}

/// The `[capabilities]` section of the manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KitCapabilities {
    /// Grant patterns the kit requires (e.g. `"chain.query"`, `"memory.read"`).
    #[serde(default)]
    pub required_grants: Vec<String>,
}

/// A skill entry in the `[skills]` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntry {
    /// Semver version requirement for this skill.
    pub version: String,

    /// The role of this skill within the kit.
    #[serde(default)]
    pub role: SkillRole,
}

/// The `[defaults]` section of the manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KitDefaults {
    /// Target networks for the kit.
    #[serde(default)]
    pub target_networks: Vec<String>,

    /// Whether to auto-activate on install.
    #[serde(default)]
    pub auto_activate: bool,
}

/// The `[policy]` section of the manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KitPolicy {
    /// Whether the kit is allowed to write on-chain.
    #[serde(default)]
    pub chain_write: bool,

    /// Data classification floor.
    #[serde(default)]
    pub data_classification: String,
}

/// The `[ux]` section of the manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KitUx {
    /// Display name shown in the UI.
    #[serde(default)]
    pub display_name: String,

    /// Short description for listings.
    #[serde(default)]
    pub short_description: String,

    /// Category tag (e.g. "governance", "treasury").
    #[serde(default)]
    pub category: String,
}

// ---------------------------------------------------------------------------
// Parsing and validation
// ---------------------------------------------------------------------------

impl KitManifest {
    /// Parse a manifest from a TOML string.
    pub fn from_toml(content: &str) -> Result<Self, KitError> {
        let manifest: Self = toml::from_str(content).map_err(|e| KitError::ManifestParse {
            reason: e.to_string(),
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Extract the [`KitId`] from this manifest.
    pub fn id(&self) -> Result<KitId, KitError> {
        KitId::parse(&self.kit.name, &self.kit.version)
    }

    /// Parse the version string into a [`semver::Version`].
    pub fn version(&self) -> Result<Version, KitError> {
        Version::parse(&self.kit.version).map_err(|e| KitError::InvalidVersion {
            version: self.kit.version.clone(),
            reason: e.to_string(),
        })
    }

    /// Return skill entries that are required (non-optional) for the kit.
    pub fn required_skills(&self) -> Vec<(&String, &SkillEntry)> {
        self.skills
            .iter()
            .filter(|(_, entry)| entry.role != SkillRole::Optional)
            .collect()
    }

    /// Validate the manifest for semantic correctness.
    fn validate(&self) -> Result<(), KitError> {
        let name = &self.kit.name;

        if name.trim().is_empty() {
            return Err(KitError::validation(
                "<unknown>",
                "kit name must not be empty",
            ));
        }

        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(KitError::validation(
                name,
                "kit name must contain only lowercase ASCII letters, digits, and hyphens",
            ));
        }

        Version::parse(&self.kit.version).map_err(|e| KitError::InvalidVersion {
            version: self.kit.version.clone(),
            reason: e.to_string(),
        })?;

        if self.skills.is_empty() {
            return Err(KitError::validation(
                name,
                "kit must contain at least one skill",
            ));
        }

        for (skill_name, entry) in &self.skills {
            VersionReq::parse(&entry.version).map_err(|e| KitError::InvalidVersionReq {
                requirement: format!("{skill_name}: {}", entry.version),
                reason: e.to_string(),
            })?;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_MANIFEST: &str = r#"
[kit]
name = "governance-scout-kit"
version = "1.0.0"
description = "Complete governance research agent for Polkadot OpenGov."
authors = ["author@example.com"]
license = "Apache-2.0"

[capabilities]
required_grants = ["chain.query", "memory.read"]

[skills]
governance-analysis-skill = { version = "^1.3.0", role = "primary" }
referendum-summary-skill  = { version = "^1.0.0", role = "primary" }
chain-reader-tool         = { version = "^2.0.0", role = "required" }
document-formatter-tool   = { version = "^1.0.0", role = "optional" }

[defaults]
target_networks = ["polkadot", "kusama"]
auto_activate = true

[policy]
chain_write = false
data_classification = "Public"

[ux]
display_name = "Governance Scout"
short_description = "Research OpenGov referenda with cited evidence."
category = "governance"
"#;

    const MINIMAL_MANIFEST: &str = r#"
[kit]
name = "simple-kit"
version = "1.0.0"

[skills]
my-skill = { version = "^1.0.0" }
"#;

    #[test]
    fn parse_full_manifest() {
        let manifest = KitManifest::from_toml(FULL_MANIFEST).expect("should parse");
        assert_eq!(manifest.kit.name, "governance-scout-kit");
        assert_eq!(manifest.kit.version, "1.0.0");
        assert_eq!(
            manifest.kit.description,
            "Complete governance research agent for Polkadot OpenGov."
        );
        assert_eq!(manifest.kit.authors, vec!["author@example.com"]);
        assert_eq!(manifest.kit.license, "Apache-2.0");
        assert_eq!(
            manifest.capabilities.required_grants,
            vec!["chain.query", "memory.read"]
        );
        assert_eq!(manifest.skills.len(), 4);
        assert_eq!(
            manifest.skills["governance-analysis-skill"].role,
            SkillRole::Primary
        );
        assert_eq!(
            manifest.skills["document-formatter-tool"].role,
            SkillRole::Optional
        );
        assert_eq!(
            manifest.defaults.target_networks,
            vec!["polkadot", "kusama"]
        );
        assert!(manifest.defaults.auto_activate);
        assert!(!manifest.policy.chain_write);
        assert_eq!(manifest.policy.data_classification, "Public");
        assert_eq!(manifest.ux.display_name, "Governance Scout");
        assert_eq!(manifest.ux.category, "governance");
    }

    #[test]
    fn parse_minimal_manifest() {
        let manifest = KitManifest::from_toml(MINIMAL_MANIFEST).expect("should parse");
        assert_eq!(manifest.kit.name, "simple-kit");
        assert_eq!(manifest.kit.version, "1.0.0");
        assert!(manifest.capabilities.required_grants.is_empty());
        assert_eq!(manifest.skills.len(), 1);
        assert_eq!(manifest.skills["my-skill"].role, SkillRole::Required);
    }

    #[test]
    fn kit_id_from_manifest() {
        let manifest = KitManifest::from_toml(FULL_MANIFEST).expect("should parse");
        let id = manifest.id().expect("should build id");
        assert_eq!(id.name, "governance-scout-kit");
        assert_eq!(id.version, Version::new(1, 0, 0));
        assert_eq!(id.to_string(), "governance-scout-kit@1.0.0");
    }

    #[test]
    fn kit_id_parse() {
        let id = KitId::parse("my-kit", "2.3.4").expect("should parse");
        assert_eq!(id.name, "my-kit");
        assert_eq!(id.version, Version::new(2, 3, 4));
    }

    #[test]
    fn kit_id_parse_invalid_version() {
        let result = KitId::parse("my-kit", "not.a.version");
        assert!(result.is_err());
    }

    #[test]
    fn reject_empty_name() {
        let toml_str = r#"
[kit]
name = ""
version = "1.0.0"

[skills]
x = { version = "^1.0.0" }
"#;
        let result = KitManifest::from_toml(toml_str);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("must not be empty"));
    }

    #[test]
    fn reject_invalid_name_characters() {
        let toml_str = r#"
[kit]
name = "My Kit!"
version = "1.0.0"

[skills]
x = { version = "^1.0.0" }
"#;
        let result = KitManifest::from_toml(toml_str);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("lowercase ASCII"));
    }

    #[test]
    fn reject_invalid_version() {
        let toml_str = r#"
[kit]
name = "good-name"
version = "bad"

[skills]
x = { version = "^1.0.0" }
"#;
        let result = KitManifest::from_toml(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn reject_no_skills() {
        let toml_str = r#"
[kit]
name = "empty-kit"
version = "1.0.0"
"#;
        let result = KitManifest::from_toml(toml_str);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("at least one skill"));
    }

    #[test]
    fn reject_invalid_skill_version_req() {
        let toml_str = r#"
[kit]
name = "bad-deps"
version = "1.0.0"

[skills]
bad-skill = { version = "not a semver range" }
"#;
        let result = KitManifest::from_toml(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn reject_invalid_toml() {
        let result = KitManifest::from_toml("this is not valid TOML {{{");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("parse"));
    }

    #[test]
    fn required_skills_excludes_optional() {
        let manifest = KitManifest::from_toml(FULL_MANIFEST).expect("should parse");
        let required = manifest.required_skills();
        assert_eq!(required.len(), 3);
        assert!(required.iter().all(|(_, e)| e.role != SkillRole::Optional));
    }

    #[test]
    fn kit_id_display() {
        let id = KitId::new("foo-bar", Version::new(1, 2, 3));
        assert_eq!(id.to_string(), "foo-bar@1.2.3");
    }

    #[test]
    fn kit_id_equality() {
        let a = KitId::new("x", Version::new(1, 0, 0));
        let b = KitId::new("x", Version::new(1, 0, 0));
        let c = KitId::new("x", Version::new(2, 0, 0));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}

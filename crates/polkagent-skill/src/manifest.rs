//! Skill manifest: the declarative description of a skill package.
//!
//! A manifest is typically read from a `skill.toml` file at the root of a
//! skill directory. It declares the skill's identity, version, required
//! capabilities (grants and tools), prompt templates, and default
//! configuration values.
//!
//! # Example TOML
//!
//! ```toml
//! [skill]
//! name = "governance-researcher"
//! version = "0.1.0"
//! description = "Research OpenGov proposals"
//! authors = ["author@example.com"]
//! license = "Apache-2.0"
//!
//! [capabilities]
//! required_grants = ["chain.query", "memory.read"]
//! tools = ["search_memory", "chain_query"]
//!
//! [prompts]
//! system = "You are a governance research assistant..."
//!
//! [config]
//! default_chain = "polkadot"
//! ```

use std::collections::HashMap;
use std::fmt;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::error::SkillError;

// ---------------------------------------------------------------------------
// SkillId
// ---------------------------------------------------------------------------

/// A unique identifier for a skill, composed of a name and a semver version.
///
/// Two `SkillId` values are considered equal when both their name and version
/// match exactly.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillId {
    /// The skill package name (kebab-case by convention).
    pub name: String,
    /// The skill version following semver.
    pub version: Version,
}

impl SkillId {
    /// Create a new `SkillId`.
    pub fn new(name: impl Into<String>, version: Version) -> Self {
        Self {
            name: name.into(),
            version,
        }
    }

    /// Parse a `SkillId` from name and version strings.
    pub fn parse(name: impl Into<String>, version: &str) -> Result<Self, SkillError> {
        let name = name.into();
        let version = Version::parse(version).map_err(|e| SkillError::InvalidVersion {
            version: version.to_string(),
            reason: e.to_string(),
        })?;
        Ok(Self { name, version })
    }
}

impl fmt::Display for SkillId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

// ---------------------------------------------------------------------------
// SkillManifest — the full manifest structure
// ---------------------------------------------------------------------------

/// The top-level manifest structure, deserialized from TOML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Core identity section.
    pub skill: SkillSection,

    /// Capability requirements (grants and tools).
    #[serde(default)]
    pub capabilities: CapabilitiesSection,

    /// Prompt templates.
    #[serde(default)]
    pub prompts: PromptsSection,

    /// Free-form configuration key-value pairs.
    #[serde(default)]
    pub config: HashMap<String, toml::Value>,

    /// Skill-level dependencies on other skills.
    #[serde(default)]
    pub dependencies: HashMap<String, DependencySpec>,
}

/// The `[skill]` section of the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSection {
    /// The package name (kebab-case).
    pub name: String,

    /// The semver version string (e.g. `"0.1.0"`).
    pub version: String,

    /// A human-readable description of what the skill does.
    #[serde(default)]
    pub description: String,

    /// A list of author email addresses or names.
    #[serde(default)]
    pub authors: Vec<String>,

    /// The SPDX license identifier.
    #[serde(default)]
    pub license: String,
}

/// The `[capabilities]` section of the manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilitiesSection {
    /// Grant patterns the skill requires (e.g. `"chain.query"`, `"memory.read"`).
    #[serde(default)]
    pub required_grants: Vec<String>,

    /// Tool names the skill expects to use.
    #[serde(default)]
    pub tools: Vec<String>,
}

/// The `[prompts]` section of the manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptsSection {
    /// The system prompt injected when the skill is activated.
    #[serde(default)]
    pub system: String,
}

/// A dependency specification from the `[dependencies]` table.
///
/// In TOML this looks like:
/// ```toml
/// [dependencies]
/// chain-utils = { version = ">=0.2.0" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencySpec {
    /// The semver version requirement string.
    pub version: String,
}

// ---------------------------------------------------------------------------
// Parsing and validation
// ---------------------------------------------------------------------------

impl SkillManifest {
    /// Parse a manifest from a TOML string.
    pub fn from_toml(content: &str) -> Result<Self, SkillError> {
        let manifest: Self = toml::from_str(content).map_err(|e| SkillError::ManifestParse {
            reason: e.to_string(),
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Extract the [`SkillId`] from this manifest.
    pub fn id(&self) -> Result<SkillId, SkillError> {
        SkillId::parse(&self.skill.name, &self.skill.version)
    }

    /// Parse the version string into a [`semver::Version`].
    pub fn version(&self) -> Result<Version, SkillError> {
        Version::parse(&self.skill.version).map_err(|e| SkillError::InvalidVersion {
            version: self.skill.version.clone(),
            reason: e.to_string(),
        })
    }

    /// Validate the manifest for semantic correctness.
    ///
    /// This checks:
    /// - Name is non-empty
    /// - Version is a valid semver string
    /// - All dependency version requirements are parseable
    fn validate(&self) -> Result<(), SkillError> {
        let name = &self.skill.name;

        // Name must be non-empty.
        if name.trim().is_empty() {
            return Err(SkillError::validation(
                "<unknown>",
                "skill name must not be empty",
            ));
        }

        // Name must be kebab-case-ish: lowercase alphanumeric + hyphens.
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(SkillError::validation(
                name,
                "skill name must contain only lowercase ASCII letters, digits, and hyphens",
            ));
        }

        // Version must be valid semver.
        Version::parse(&self.skill.version).map_err(|e| SkillError::InvalidVersion {
            version: self.skill.version.clone(),
            reason: e.to_string(),
        })?;

        // Validate dependency version requirements.
        for (dep_name, dep_spec) in &self.dependencies {
            VersionReq::parse(&dep_spec.version).map_err(|e| SkillError::InvalidVersionReq {
                requirement: format!("{dep_name}: {}", dep_spec.version),
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
[skill]
name = "governance-researcher"
version = "0.1.0"
description = "Research OpenGov proposals"
authors = ["author@example.com"]
license = "Apache-2.0"

[capabilities]
required_grants = ["chain.query", "memory.read"]
tools = ["search_memory", "chain_query"]

[prompts]
system = "You are a governance research assistant..."

[config]
default_chain = "polkadot"
"#;

    const MINIMAL_MANIFEST: &str = r#"
[skill]
name = "simple"
version = "1.0.0"
"#;

    #[test]
    fn parse_full_manifest() {
        let manifest = SkillManifest::from_toml(FULL_MANIFEST).expect("should parse");
        assert_eq!(manifest.skill.name, "governance-researcher");
        assert_eq!(manifest.skill.version, "0.1.0");
        assert_eq!(manifest.skill.description, "Research OpenGov proposals");
        assert_eq!(manifest.skill.authors, vec!["author@example.com"]);
        assert_eq!(manifest.skill.license, "Apache-2.0");
        assert_eq!(
            manifest.capabilities.required_grants,
            vec!["chain.query", "memory.read"]
        );
        assert_eq!(
            manifest.capabilities.tools,
            vec!["search_memory", "chain_query"]
        );
        assert_eq!(
            manifest.prompts.system,
            "You are a governance research assistant..."
        );
        assert_eq!(
            manifest
                .config
                .get("default_chain")
                .and_then(|v| v.as_str()),
            Some("polkadot")
        );
    }

    #[test]
    fn parse_minimal_manifest() {
        let manifest = SkillManifest::from_toml(MINIMAL_MANIFEST).expect("should parse");
        assert_eq!(manifest.skill.name, "simple");
        assert_eq!(manifest.skill.version, "1.0.0");
        assert!(manifest.capabilities.required_grants.is_empty());
        assert!(manifest.capabilities.tools.is_empty());
        assert!(manifest.prompts.system.is_empty());
        assert!(manifest.config.is_empty());
    }

    #[test]
    fn skill_id_from_manifest() {
        let manifest = SkillManifest::from_toml(FULL_MANIFEST).expect("should parse");
        let id = manifest.id().expect("should build id");
        assert_eq!(id.name, "governance-researcher");
        assert_eq!(id.version, Version::new(0, 1, 0));
        assert_eq!(id.to_string(), "governance-researcher@0.1.0");
    }

    #[test]
    fn skill_id_parse() {
        let id = SkillId::parse("my-skill", "2.3.4").expect("should parse");
        assert_eq!(id.name, "my-skill");
        assert_eq!(id.version, Version::new(2, 3, 4));
    }

    #[test]
    fn skill_id_parse_invalid_version() {
        let result = SkillId::parse("my-skill", "not.a.version");
        assert!(result.is_err());
    }

    #[test]
    fn reject_empty_name() {
        let toml = r#"
[skill]
name = ""
version = "1.0.0"
"#;
        let result = SkillManifest::from_toml(toml);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("must not be empty"));
    }

    #[test]
    fn reject_invalid_name_characters() {
        let toml = r#"
[skill]
name = "My Skill!"
version = "1.0.0"
"#;
        let result = SkillManifest::from_toml(toml);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("lowercase ASCII"));
    }

    #[test]
    fn reject_invalid_version() {
        let toml = r#"
[skill]
name = "good-name"
version = "bad"
"#;
        let result = SkillManifest::from_toml(toml);
        assert!(result.is_err());
    }

    #[test]
    fn reject_invalid_toml() {
        let result = SkillManifest::from_toml("this is not valid TOML {{{");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("parse"));
    }

    #[test]
    fn manifest_with_dependencies() {
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
    fn reject_invalid_dependency_version_req() {
        let toml = r#"
[skill]
name = "my-skill"
version = "1.0.0"

[dependencies]
bad-dep = { version = "not a semver range" }
"#;
        let result = SkillManifest::from_toml(toml);
        assert!(result.is_err());
    }

    #[test]
    fn skill_id_display() {
        let id = SkillId::new("foo-bar", Version::new(1, 2, 3));
        assert_eq!(id.to_string(), "foo-bar@1.2.3");
    }

    #[test]
    fn skill_id_equality() {
        let a = SkillId::new("x", Version::new(1, 0, 0));
        let b = SkillId::new("x", Version::new(1, 0, 0));
        let c = SkillId::new("x", Version::new(2, 0, 0));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}

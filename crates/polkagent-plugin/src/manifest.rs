//! Plugin manifest: the declarative description of a plugin package.
//!
//! A manifest is read from a `plugin.toml` file at the root of a plugin
//! directory. It declares the plugin's identity, version, required and
//! optional capabilities, entry point, dependencies on other plugins, and
//! metadata such as author and license.
//!
//! # Example TOML
//!
//! ```toml
//! [plugin]
//! name = "governance-tools"
//! version = "1.0.0"
//! description = "OpenGov governance research tools"
//! author = "Polkagent Team"
//! license = "Apache-2.0"
//! entry_point = "governance_tools::init"
//!
//! [capabilities]
//! required = ["chain.query", "memory.read"]
//! optional = ["network.http"]
//!
//! [dependencies]
//! polkagent-identity = ">=0.1.0"
//! ```

use std::collections::HashMap;
use std::fmt;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::capability::CapabilitySet;
use crate::error::PluginError;

// ---------------------------------------------------------------------------
// PluginId
// ---------------------------------------------------------------------------

/// A unique identifier for a plugin, composed of a name and a semver version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginId {
    /// The plugin package name (kebab-case by convention).
    pub name: String,
    /// The plugin version following semver.
    pub version: Version,
}

impl PluginId {
    /// Create a new `PluginId`.
    pub fn new(name: impl Into<String>, version: Version) -> Self {
        Self {
            name: name.into(),
            version,
        }
    }

    /// Parse a `PluginId` from name and version strings.
    pub fn parse(name: impl Into<String>, version: &str) -> Result<Self, PluginError> {
        let name = name.into();
        let version = Version::parse(version).map_err(|e| PluginError::InvalidVersion {
            version: version.to_string(),
            reason: e.to_string(),
        })?;
        Ok(Self { name, version })
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

// ---------------------------------------------------------------------------
// PluginManifest — the full manifest structure
// ---------------------------------------------------------------------------

/// The top-level manifest structure, deserialized from `plugin.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Core identity section.
    pub plugin: PluginSection,

    /// Capability requirements (required and optional).
    #[serde(default)]
    pub capabilities: CapabilitiesSection,

    /// Plugin-level dependencies on other plugins, keyed by name.
    ///
    /// Values are semver requirement strings (e.g. `">=0.1.0"`, `"^1.0"`).
    #[serde(default)]
    pub dependencies: HashMap<String, String>,

    /// Supply-chain provenance and signing information.
    #[serde(default)]
    pub provenance: ProvenanceSection,
}

/// The `[provenance]` section of the manifest.
///
/// Records supply-chain signing, SLSA provenance, and trust classification.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProvenanceSection {
    /// Cosign v3 keyless signature bundle (base64-encoded).
    #[serde(default)]
    pub cosign_bundle: Option<String>,

    /// The expected signer identity for cosign verification
    /// (e.g. a GitHub Actions workflow URI).
    #[serde(default)]
    pub signer_identity: Option<String>,

    /// Rekor transparency-log entry index (if signed via Sigstore).
    #[serde(default)]
    pub rekor_log_index: Option<u64>,

    /// SLSA provenance attestation (in-toto, base64-encoded JSON).
    #[serde(default)]
    pub slsa_provenance: Option<String>,

    /// SLSA Build Level claimed by the publisher (0-3).
    #[serde(default)]
    pub slsa_build_level: Option<u8>,

    /// Content digest of the package archive (hex-encoded blake3).
    #[serde(default)]
    pub content_digest: Option<String>,
}

/// Trust tier classification for a package.
///
/// See PRD-12 §4.7.1 for the full trust tier model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    /// No signature. Displayed with prominent warnings.
    Unsigned,
    /// Valid cosign v3 keyless signature + Rekor entry + in-toto attestation.
    Signed,
    /// Signed + verified publisher identity + SLSA Build L2 provenance.
    Verified,
    /// Verified plus explicit inclusion in a curated collection.
    Curated,
}

impl fmt::Display for TrustTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsigned => write!(f, "unsigned"),
            Self::Signed => write!(f, "signed"),
            Self::Verified => write!(f, "verified"),
            Self::Curated => write!(f, "curated"),
        }
    }
}

/// Sandbox tier classification for a package.
///
/// See PRD-12 §8.2 for sandbox tier selection logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxTier {
    /// No executable code. Content loaded into agent context.
    ContextOnly,
    /// Separate OS process with restricted capabilities.
    Process,
    /// WebAssembly module with fuel, memory, and host-call restrictions.
    Wasm,
    /// Container or microVM with full resource isolation.
    Container,
}

impl fmt::Display for SandboxTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContextOnly => write!(f, "context-only"),
            Self::Process => write!(f, "process"),
            Self::Wasm => write!(f, "wasm"),
            Self::Container => write!(f, "container"),
        }
    }
}

/// The `[plugin]` section of the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSection {
    /// The package name (kebab-case).
    pub name: String,

    /// The semver version string (e.g. `"1.0.0"`).
    pub version: String,

    /// A human-readable description of what the plugin does.
    #[serde(default)]
    pub description: String,

    /// The plugin author (name or email).
    #[serde(default)]
    pub author: String,

    /// The SPDX license identifier.
    #[serde(default)]
    pub license: String,

    /// The entry point for the plugin (e.g. module path or function name).
    #[serde(default)]
    pub entry_point: String,
}

/// The `[capabilities]` section of the manifest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilitiesSection {
    /// Capability strings the plugin requires (must be granted).
    #[serde(default)]
    pub required: Vec<String>,

    /// Capability strings the plugin can use if available (optional).
    #[serde(default)]
    pub optional: Vec<String>,
}

// ---------------------------------------------------------------------------
// Parsing and validation
// ---------------------------------------------------------------------------

impl PluginManifest {
    /// Parse a manifest from a TOML string.
    pub fn from_toml(content: &str) -> Result<Self, PluginError> {
        let manifest: Self =
            toml::from_str(content).map_err(|e| PluginError::manifest_parse(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Extract the [`PluginId`] from this manifest.
    pub fn id(&self) -> Result<PluginId, PluginError> {
        PluginId::parse(&self.plugin.name, &self.plugin.version)
    }

    /// Parse the version string into a [`semver::Version`].
    pub fn version(&self) -> Result<Version, PluginError> {
        Version::parse(&self.plugin.version).map_err(|e| PluginError::InvalidVersion {
            version: self.plugin.version.clone(),
            reason: e.to_string(),
        })
    }

    /// Parse the required capabilities into a [`CapabilitySet`].
    pub fn required_capabilities(&self) -> Result<CapabilitySet, PluginError> {
        CapabilitySet::parse_strings(&self.capabilities.required)
    }

    /// Parse the optional capabilities into a [`CapabilitySet`].
    pub fn optional_capabilities(&self) -> Result<CapabilitySet, PluginError> {
        CapabilitySet::parse_strings(&self.capabilities.optional)
    }

    /// Determine the trust tier of this package based on its provenance data.
    pub fn trust_tier(&self) -> TrustTier {
        let prov = &self.provenance;

        // Must have a cosign signature to be Signed or above.
        if prov.cosign_bundle.is_none() {
            return TrustTier::Unsigned;
        }

        // Must have SLSA provenance at Build Level 2+ to be Verified.
        let has_slsa_l2 = prov.slsa_provenance.is_some() && prov.slsa_build_level.unwrap_or(0) >= 2;

        if has_slsa_l2 {
            TrustTier::Verified
        } else {
            TrustTier::Signed
        }
    }

    /// Determine the sandbox tier for this package based on its trust tier.
    ///
    /// See PRD-12 §8.2: skills get `ContextOnly`, tools get a tier based
    /// on trust.
    pub fn sandbox_tier(&self) -> SandboxTier {
        match self.trust_tier() {
            TrustTier::Curated | TrustTier::Verified => SandboxTier::Process,
            TrustTier::Signed => SandboxTier::Wasm,
            TrustTier::Unsigned => SandboxTier::Container,
        }
    }

    /// Check whether this manifest has a cosign signature.
    pub fn is_signed(&self) -> bool {
        self.provenance.cosign_bundle.is_some()
    }

    /// Validate the manifest for semantic correctness.
    ///
    /// Checks:
    /// - Name is non-empty and kebab-case
    /// - Version is a valid semver string
    /// - All capability strings are recognized
    /// - All dependency version requirements are parseable
    fn validate(&self) -> Result<(), PluginError> {
        let name = &self.plugin.name;

        // Name must be non-empty.
        if name.trim().is_empty() {
            return Err(PluginError::manifest_invalid(
                "<unknown>",
                "plugin name must not be empty",
            ));
        }

        // Name must be kebab-case: lowercase alphanumeric + hyphens.
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(PluginError::manifest_invalid(
                name,
                "plugin name must contain only lowercase ASCII letters, digits, and hyphens",
            ));
        }

        // Version must be valid semver.
        Version::parse(&self.plugin.version).map_err(|e| PluginError::InvalidVersion {
            version: self.plugin.version.clone(),
            reason: e.to_string(),
        })?;

        // Validate capability strings.
        CapabilitySet::parse_strings(&self.capabilities.required)?;
        CapabilitySet::parse_strings(&self.capabilities.optional)?;

        // Validate dependency version requirements.
        for (dep_name, ver_req_str) in &self.dependencies {
            VersionReq::parse(ver_req_str).map_err(|e| PluginError::InvalidVersionReq {
                requirement: format!("{dep_name}: {ver_req_str}"),
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
[plugin]
name = "governance-tools"
version = "1.0.0"
description = "OpenGov governance research tools"
author = "Polkagent Team"
license = "Apache-2.0"
entry_point = "governance_tools::init"

[capabilities]
required = ["chain.query", "memory.read"]
optional = ["network.http"]

[dependencies]
polkagent-identity = ">=0.1.0"
"#;

    const MINIMAL_MANIFEST: &str = r#"
[plugin]
name = "simple"
version = "1.0.0"
"#;

    #[test]
    fn parse_full_manifest() {
        let manifest = PluginManifest::from_toml(FULL_MANIFEST).expect("should parse");
        assert_eq!(manifest.plugin.name, "governance-tools");
        assert_eq!(manifest.plugin.version, "1.0.0");
        assert_eq!(
            manifest.plugin.description,
            "OpenGov governance research tools"
        );
        assert_eq!(manifest.plugin.author, "Polkagent Team");
        assert_eq!(manifest.plugin.license, "Apache-2.0");
        assert_eq!(manifest.plugin.entry_point, "governance_tools::init");
        assert_eq!(
            manifest.capabilities.required,
            vec!["chain.query", "memory.read"]
        );
        assert_eq!(manifest.capabilities.optional, vec!["network.http"]);
        assert_eq!(
            manifest.dependencies.get("polkagent-identity"),
            Some(&">=0.1.0".to_string())
        );
    }

    #[test]
    fn parse_minimal_manifest() {
        let manifest = PluginManifest::from_toml(MINIMAL_MANIFEST).expect("should parse");
        assert_eq!(manifest.plugin.name, "simple");
        assert_eq!(manifest.plugin.version, "1.0.0");
        assert!(manifest.plugin.description.is_empty());
        assert!(manifest.capabilities.required.is_empty());
        assert!(manifest.capabilities.optional.is_empty());
        assert!(manifest.dependencies.is_empty());
    }

    #[test]
    fn plugin_id_from_manifest() {
        let manifest = PluginManifest::from_toml(FULL_MANIFEST).expect("should parse");
        let id = manifest.id().expect("should build id");
        assert_eq!(id.name, "governance-tools");
        assert_eq!(id.version, Version::new(1, 0, 0));
        assert_eq!(id.to_string(), "governance-tools@1.0.0");
    }

    #[test]
    fn plugin_id_parse() {
        let id = PluginId::parse("my-plugin", "2.3.4").expect("should parse");
        assert_eq!(id.name, "my-plugin");
        assert_eq!(id.version, Version::new(2, 3, 4));
    }

    #[test]
    fn plugin_id_parse_invalid_version() {
        let result = PluginId::parse("my-plugin", "not.a.version");
        assert!(result.is_err());
    }

    #[test]
    fn reject_empty_name() {
        let toml = r#"
[plugin]
name = ""
version = "1.0.0"
"#;
        let result = PluginManifest::from_toml(toml);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("must not be empty"));
    }

    #[test]
    fn reject_invalid_name_characters() {
        let toml = r#"
[plugin]
name = "My Plugin!"
version = "1.0.0"
"#;
        let result = PluginManifest::from_toml(toml);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("lowercase ASCII"));
    }

    #[test]
    fn reject_invalid_version() {
        let toml = r#"
[plugin]
name = "good-name"
version = "bad"
"#;
        let result = PluginManifest::from_toml(toml);
        assert!(result.is_err());
    }

    #[test]
    fn reject_invalid_toml() {
        let result = PluginManifest::from_toml("this is not valid TOML {{{");
        assert!(result.is_err());
    }

    #[test]
    fn reject_unknown_capability() {
        let toml = r#"
[plugin]
name = "bad-caps"
version = "1.0.0"

[capabilities]
required = ["chain.query", "fly.to.moon"]
"#;
        let result = PluginManifest::from_toml(toml);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("fly.to.moon"));
    }

    #[test]
    fn reject_invalid_dependency_version_req() {
        let toml = r#"
[plugin]
name = "bad-deps"
version = "1.0.0"

[dependencies]
bad-dep = "not a semver range"
"#;
        let result = PluginManifest::from_toml(toml);
        assert!(result.is_err());
    }

    #[test]
    fn required_capabilities_parsed() {
        let manifest = PluginManifest::from_toml(FULL_MANIFEST).expect("should parse");
        let caps = manifest.required_capabilities().expect("should parse caps");
        assert_eq!(caps.len(), 2);
        assert!(caps.contains(crate::capability::PluginCapability::ChainQuery));
        assert!(caps.contains(crate::capability::PluginCapability::MemoryAccess));
    }

    #[test]
    fn optional_capabilities_parsed() {
        let manifest = PluginManifest::from_toml(FULL_MANIFEST).expect("should parse");
        let caps = manifest.optional_capabilities().expect("should parse caps");
        assert_eq!(caps.len(), 1);
        assert!(caps.contains(crate::capability::PluginCapability::NetworkAccess));
    }

    #[test]
    fn plugin_id_display() {
        let id = PluginId::new("foo-bar", Version::new(1, 2, 3));
        assert_eq!(id.to_string(), "foo-bar@1.2.3");
    }

    #[test]
    fn plugin_id_equality() {
        let a = PluginId::new("x", Version::new(1, 0, 0));
        let b = PluginId::new("x", Version::new(1, 0, 0));
        let c = PluginId::new("x", Version::new(2, 0, 0));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn minimal_manifest_has_default_provenance() {
        let manifest = PluginManifest::from_toml(MINIMAL_MANIFEST).expect("should parse");
        assert!(manifest.provenance.cosign_bundle.is_none());
        assert!(manifest.provenance.slsa_provenance.is_none());
        assert!(manifest.provenance.slsa_build_level.is_none());
        assert!(manifest.provenance.content_digest.is_none());
    }

    #[test]
    fn trust_tier_unsigned_when_no_signature() {
        let manifest = PluginManifest::from_toml(MINIMAL_MANIFEST).expect("should parse");
        assert_eq!(manifest.trust_tier(), TrustTier::Unsigned);
        assert!(!manifest.is_signed());
    }

    #[test]
    fn trust_tier_signed_with_cosign_bundle() {
        let toml = r#"
[plugin]
name = "signed-pkg"
version = "1.0.0"

[provenance]
cosign_bundle = "eyJhbGciOi..."
signer_identity = "https://github.com/example/repo/.github/workflows/publish.yml@refs/heads/main"
"#;
        let manifest = PluginManifest::from_toml(toml).expect("should parse");
        assert_eq!(manifest.trust_tier(), TrustTier::Signed);
        assert!(manifest.is_signed());
    }

    #[test]
    fn trust_tier_verified_with_slsa_l2() {
        let toml = r#"
[plugin]
name = "verified-pkg"
version = "1.0.0"

[provenance]
cosign_bundle = "eyJhbGciOi..."
slsa_provenance = "eyJ2ZXJzaW9uIjoi..."
slsa_build_level = 2
"#;
        let manifest = PluginManifest::from_toml(toml).expect("should parse");
        assert_eq!(manifest.trust_tier(), TrustTier::Verified);
    }

    #[test]
    fn sandbox_tier_based_on_trust() {
        let unsigned = PluginManifest::from_toml(MINIMAL_MANIFEST).expect("should parse");
        assert_eq!(unsigned.sandbox_tier(), SandboxTier::Container);

        let signed_toml = r#"
[plugin]
name = "signed-pkg"
version = "1.0.0"

[provenance]
cosign_bundle = "eyJhbGciOi..."
"#;
        let signed = PluginManifest::from_toml(signed_toml).expect("should parse");
        assert_eq!(signed.sandbox_tier(), SandboxTier::Wasm);

        let verified_toml = r#"
[plugin]
name = "verified-pkg"
version = "1.0.0"

[provenance]
cosign_bundle = "eyJhbGciOi..."
slsa_provenance = "eyJ2ZXJzaW9uIjoi..."
slsa_build_level = 2
"#;
        let verified = PluginManifest::from_toml(verified_toml).expect("should parse");
        assert_eq!(verified.sandbox_tier(), SandboxTier::Process);
    }

    #[test]
    fn trust_tier_display() {
        assert_eq!(TrustTier::Unsigned.to_string(), "unsigned");
        assert_eq!(TrustTier::Signed.to_string(), "signed");
        assert_eq!(TrustTier::Verified.to_string(), "verified");
        assert_eq!(TrustTier::Curated.to_string(), "curated");
    }

    #[test]
    fn sandbox_tier_display() {
        assert_eq!(SandboxTier::ContextOnly.to_string(), "context-only");
        assert_eq!(SandboxTier::Process.to_string(), "process");
        assert_eq!(SandboxTier::Wasm.to_string(), "wasm");
        assert_eq!(SandboxTier::Container.to_string(), "container");
    }

    #[test]
    fn trust_tier_ordering() {
        assert!(TrustTier::Unsigned < TrustTier::Signed);
        assert!(TrustTier::Signed < TrustTier::Verified);
        assert!(TrustTier::Verified < TrustTier::Curated);
    }
}

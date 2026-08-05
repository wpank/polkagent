//! SLSA Build Level 2 attestation checking for marketplace packages.
//!
//! Implements the provenance attestation verification described in
//! PRD-12 §4.7.1. Every published package should carry an in-toto
//! attestation recording the full build provenance chain. The target
//! supply-chain assurance level is **SLSA Build Level 2**.
//!
//! SLSA L2 requires:
//! - A hosted build platform that generates signed provenance.
//! - The provenance attestation covers all build artifacts.
//! - The build platform identity is verifiable.
//!
//! This module provides the attestation validation policy and result
//! types. Full in-toto envelope parsing and signature verification
//! requires the `in-toto` crate at the application level.

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::error::PluginError;
use crate::manifest::PluginManifest;

// ---------------------------------------------------------------------------
// SlsaBuildLevel
// ---------------------------------------------------------------------------

/// SLSA Build Level as defined by the SLSA specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SlsaBuildLevel {
    /// No provenance information.
    None,
    /// SLSA Build Level 1: provenance exists but may not be signed.
    L1,
    /// SLSA Build Level 2: hosted build platform with signed provenance.
    L2,
    /// SLSA Build Level 3: hardened build platform resistant to tampering.
    L3,
}

impl SlsaBuildLevel {
    /// Parse from an integer (0-3).
    pub fn from_level(level: u8) -> Self {
        match level {
            0 => Self::None,
            1 => Self::L1,
            2 => Self::L2,
            _ => Self::L3,
        }
    }

    /// Return the numeric level.
    pub fn level(&self) -> u8 {
        match self {
            Self::None => 0,
            Self::L1 => 1,
            Self::L2 => 2,
            Self::L3 => 3,
        }
    }
}

impl std::fmt::Display for SlsaBuildLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::L1 => write!(f, "SLSA L1"),
            Self::L2 => write!(f, "SLSA L2"),
            Self::L3 => write!(f, "SLSA L3"),
        }
    }
}

// ---------------------------------------------------------------------------
// AttestationPolicy
// ---------------------------------------------------------------------------

/// Policy controlling SLSA attestation requirements.
#[derive(Debug, Clone)]
pub struct AttestationPolicy {
    /// Whether SLSA attestation is required.
    pub require_attestation: bool,
    /// Minimum required SLSA Build Level.
    pub minimum_build_level: SlsaBuildLevel,
}

impl Default for AttestationPolicy {
    fn default() -> Self {
        Self {
            require_attestation: true,
            minimum_build_level: SlsaBuildLevel::L2,
        }
    }
}

// ---------------------------------------------------------------------------
// AttestationResult
// ---------------------------------------------------------------------------

/// The result of checking a package's SLSA provenance attestation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationResult {
    /// The determined SLSA Build Level.
    pub build_level: SlsaBuildLevel,
    /// Whether the attestation check passed.
    pub accepted: bool,
    /// Human-readable summary of the attestation outcome.
    pub summary: String,
    /// Warnings generated during verification.
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// AttestationChecker
// ---------------------------------------------------------------------------

/// Checks SLSA provenance attestations on packages according to
/// the configured [`AttestationPolicy`].
#[derive(Debug, Clone)]
pub struct AttestationChecker {
    policy: AttestationPolicy,
}

impl AttestationChecker {
    /// Create a new checker with the given policy.
    pub fn new(policy: AttestationPolicy) -> Self {
        Self { policy }
    }

    /// Create a checker with the default policy (SLSA L2 required).
    pub fn strict() -> Self {
        Self::new(AttestationPolicy::default())
    }

    /// Create a checker that does not require attestation (for
    /// development and sideloading workflows).
    pub fn permissive() -> Self {
        Self::new(AttestationPolicy {
            require_attestation: false,
            minimum_build_level: SlsaBuildLevel::None,
        })
    }

    /// Get a reference to the active policy.
    pub fn policy(&self) -> &AttestationPolicy {
        &self.policy
    }

    /// Check a package manifest's SLSA provenance attestation.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::SlsaAttestationFailed`] if the package
    /// does not meet the required SLSA Build Level.
    pub fn check(&self, manifest: &PluginManifest) -> Result<AttestationResult, PluginError> {
        let name = &manifest.plugin.name;
        let prov = &manifest.provenance;
        let mut warnings = Vec::new();

        info!(plugin = %name, "checking SLSA attestation");

        // Determine the actual build level from manifest data.
        let actual_level = match (&prov.slsa_provenance, prov.slsa_build_level) {
            (Some(_), Some(level)) => SlsaBuildLevel::from_level(level),
            (Some(_), None) => {
                warnings.push(format!(
                    "Package '{name}' has SLSA provenance but no declared build level. \
                     Assuming L1."
                ));
                SlsaBuildLevel::L1
            }
            (None, _) => SlsaBuildLevel::None,
        };

        debug!(
            plugin = %name,
            actual_level = %actual_level,
            required_level = %self.policy.minimum_build_level,
            "SLSA attestation check"
        );

        // If attestation is not required, accept with warnings.
        if !self.policy.require_attestation {
            if actual_level == SlsaBuildLevel::None {
                warnings.push(format!(
                    "Package '{name}' has no SLSA provenance attestation."
                ));
            }
            return Ok(AttestationResult {
                build_level: actual_level,
                accepted: true,
                summary: format!("attestation not required; package '{name}' at {actual_level}"),
                warnings,
            });
        }

        // Attestation is required: check for presence.
        if prov.slsa_provenance.is_none() {
            return Err(PluginError::SlsaAttestationFailed {
                package_name: name.clone(),
                reason: "SLSA provenance attestation is missing".to_string(),
            });
        }

        // Check minimum build level.
        if actual_level < self.policy.minimum_build_level {
            return Err(PluginError::SlsaAttestationFailed {
                package_name: name.clone(),
                reason: format!(
                    "SLSA Build Level '{actual_level}' is below minimum '{}'",
                    self.policy.minimum_build_level
                ),
            });
        }

        // Validate content digest is present alongside attestation.
        if prov.content_digest.is_none() {
            warnings.push(format!(
                "Package '{name}' has SLSA provenance but no content digest. \
                 Artifact binding cannot be verified."
            ));
        }

        info!(
            plugin = %name,
            build_level = %actual_level,
            "SLSA attestation check passed"
        );

        Ok(AttestationResult {
            build_level: actual_level,
            accepted: true,
            summary: format!("package '{name}' passes SLSA attestation at {actual_level}"),
            warnings,
        })
    }
}

impl Default for AttestationChecker {
    fn default() -> Self {
        Self::strict()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "attestation tests intentionally panic at provenance and policy fixture boundaries"
)]
mod tests {
    use super::*;
    use crate::manifest::{CapabilitiesSection, PluginSection, ProvenanceSection};
    use std::collections::HashMap;

    fn bare_manifest(name: &str) -> PluginManifest {
        PluginManifest {
            plugin: PluginSection {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                description: String::new(),
                author: String::new(),
                license: String::new(),
                entry_point: String::new(),
            },
            capabilities: CapabilitiesSection::default(),
            dependencies: HashMap::new(),
            provenance: ProvenanceSection::default(),
        }
    }

    fn manifest_with_slsa(name: &str, level: u8) -> PluginManifest {
        let mut m = bare_manifest(name);
        m.provenance.slsa_provenance = Some("eyJ2ZXJzaW9uIjoi...".to_string());
        m.provenance.slsa_build_level = Some(level);
        m.provenance.content_digest = Some("abcdef".to_string());
        m
    }

    #[test]
    fn strict_checker_rejects_missing_attestation() {
        let checker = AttestationChecker::strict();
        let manifest = bare_manifest("no-attestation");

        let result = checker.check(&manifest);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("missing"));
    }

    #[test]
    fn strict_checker_rejects_low_build_level() {
        let checker = AttestationChecker::strict();
        let manifest = manifest_with_slsa("low-level", 1);

        let result = checker.check(&manifest);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("below minimum"));
    }

    #[test]
    fn strict_checker_accepts_l2() {
        let checker = AttestationChecker::strict();
        let manifest = manifest_with_slsa("l2-pkg", 2);

        let result = checker.check(&manifest).expect("should accept");
        assert!(result.accepted);
        assert_eq!(result.build_level, SlsaBuildLevel::L2);
    }

    #[test]
    fn strict_checker_accepts_l3() {
        let checker = AttestationChecker::strict();
        let manifest = manifest_with_slsa("l3-pkg", 3);

        let result = checker.check(&manifest).expect("should accept");
        assert!(result.accepted);
        assert_eq!(result.build_level, SlsaBuildLevel::L3);
    }

    #[test]
    fn permissive_checker_accepts_without_attestation() {
        let checker = AttestationChecker::permissive();
        let manifest = bare_manifest("no-slsa");

        let result = checker.check(&manifest).expect("should accept");
        assert!(result.accepted);
        assert_eq!(result.build_level, SlsaBuildLevel::None);
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn missing_content_digest_produces_warning() {
        let checker = AttestationChecker::strict();
        let mut manifest = manifest_with_slsa("no-digest", 2);
        manifest.provenance.content_digest = None;

        let result = checker.check(&manifest).expect("should still accept");
        assert!(result.accepted);
        assert!(result.warnings.iter().any(|w| w.contains("content digest")));
    }

    #[test]
    fn slsa_build_level_ordering() {
        assert!(SlsaBuildLevel::None < SlsaBuildLevel::L1);
        assert!(SlsaBuildLevel::L1 < SlsaBuildLevel::L2);
        assert!(SlsaBuildLevel::L2 < SlsaBuildLevel::L3);
    }

    #[test]
    fn slsa_build_level_display() {
        assert_eq!(SlsaBuildLevel::None.to_string(), "none");
        assert_eq!(SlsaBuildLevel::L1.to_string(), "SLSA L1");
        assert_eq!(SlsaBuildLevel::L2.to_string(), "SLSA L2");
        assert_eq!(SlsaBuildLevel::L3.to_string(), "SLSA L3");
    }

    #[test]
    fn slsa_build_level_roundtrip() {
        for level in 0..=3 {
            let parsed = SlsaBuildLevel::from_level(level);
            assert_eq!(parsed.level(), level);
        }
    }

    #[test]
    fn custom_policy_minimum_l1() {
        let checker = AttestationChecker::new(AttestationPolicy {
            require_attestation: true,
            minimum_build_level: SlsaBuildLevel::L1,
        });

        let l1 = manifest_with_slsa("l1-ok", 1);
        assert!(checker.check(&l1).expect("should accept").accepted);

        let none = bare_manifest("no-slsa");
        assert!(checker.check(&none).is_err());
    }

    #[test]
    fn default_checker_is_strict() {
        let checker = AttestationChecker::default();
        let manifest = bare_manifest("should-fail");
        assert!(checker.check(&manifest).is_err());
    }

    #[test]
    fn provenance_without_build_level_defaults_to_l1() {
        let checker = AttestationChecker::new(AttestationPolicy {
            require_attestation: true,
            minimum_build_level: SlsaBuildLevel::L1,
        });

        let mut manifest = bare_manifest("no-level");
        manifest.provenance.slsa_provenance = Some("eyJ2ZXJzaW9uIjoi...".to_string());

        let result = checker.check(&manifest).expect("should accept at L1");
        assert_eq!(result.build_level, SlsaBuildLevel::L1);
        assert!(result.warnings.iter().any(|w| w.contains("Assuming L1")));
    }
}

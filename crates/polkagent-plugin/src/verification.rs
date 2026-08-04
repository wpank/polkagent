//! Cosign v3 keyless signature verification for marketplace packages.
//!
//! Implements the package signing verification described in PRD-12 §4.7.1.
//! Unsigned packages are rejected unless the policy explicitly allows them.
//! Signed packages must have a valid cosign v3 keyless signature bundle
//! and an optional Rekor transparency-log entry.
//!
//! The actual cryptographic verification of cosign bundles requires the
//! `sigstore` crate at the application level. This module provides the
//! policy logic and verification result types.

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::PluginError;
use crate::manifest::{PluginManifest, TrustTier};

// ---------------------------------------------------------------------------
// VerificationPolicy
// ---------------------------------------------------------------------------

/// Policy controlling which trust tiers are accepted at install time.
///
/// See PRD-12 §4.7.1: verification is **mandatory at install time**.
#[derive(Debug, Clone)]
pub struct VerificationPolicy {
    /// Whether to accept unsigned packages (with warnings).
    pub allow_unsigned: bool,
    /// Minimum required trust tier for installation.
    pub minimum_trust_tier: TrustTier,
    /// Pinned signer identities for cosign verification.
    /// If non-empty, only packages whose signer identity matches
    /// one of these are accepted.
    pub pinned_identities: Vec<String>,
}

impl Default for VerificationPolicy {
    fn default() -> Self {
        Self {
            allow_unsigned: false,
            minimum_trust_tier: TrustTier::Signed,
            pinned_identities: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// VerificationResult
// ---------------------------------------------------------------------------

/// The result of verifying a package's supply-chain provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    /// The determined trust tier.
    pub trust_tier: TrustTier,
    /// Whether the package passed verification.
    pub accepted: bool,
    /// Human-readable summary of the verification outcome.
    pub summary: String,
    /// Warnings generated during verification.
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// PackageVerifier
// ---------------------------------------------------------------------------

/// Verifies package signatures and provenance according to the
/// configured [`VerificationPolicy`].
#[derive(Debug, Clone)]
pub struct PackageVerifier {
    policy: VerificationPolicy,
}

impl PackageVerifier {
    /// Create a new verifier with the given policy.
    pub fn new(policy: VerificationPolicy) -> Self {
        Self { policy }
    }

    /// Create a verifier with the default (strict) policy: unsigned
    /// packages are rejected.
    pub fn strict() -> Self {
        Self::new(VerificationPolicy::default())
    }

    /// Create a verifier that allows unsigned packages (for development
    /// and sideloading workflows).
    pub fn permissive() -> Self {
        Self::new(VerificationPolicy {
            allow_unsigned: true,
            minimum_trust_tier: TrustTier::Unsigned,
            pinned_identities: Vec::new(),
        })
    }

    /// Get a reference to the active policy.
    pub fn policy(&self) -> &VerificationPolicy {
        &self.policy
    }

    /// Verify a package manifest's cosign signature and provenance.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::UnsignedPackageRejected`] if the package
    /// is unsigned and the policy does not allow unsigned packages.
    ///
    /// Returns [`PluginError::SignatureVerificationFailed`] if the
    /// package has a signature but it does not pass validation.
    pub fn verify(&self, manifest: &PluginManifest) -> Result<VerificationResult, PluginError> {
        let name = &manifest.plugin.name;
        let trust_tier = manifest.trust_tier();
        let mut warnings = Vec::new();

        info!(
            plugin = %name,
            trust_tier = %trust_tier,
            "verifying package provenance"
        );

        // Check: unsigned packages.
        if trust_tier == TrustTier::Unsigned {
            if self.policy.allow_unsigned {
                warn!(
                    plugin = %name,
                    "accepting unsigned package (policy allows unsigned)"
                );
                warnings.push(format!(
                    "Package '{name}' has no cosign signature. \
                     Anyone could have produced or modified it."
                ));
                return Ok(VerificationResult {
                    trust_tier,
                    accepted: true,
                    summary: format!("unsigned package '{name}' accepted with warnings"),
                    warnings,
                });
            }
            return Err(PluginError::UnsignedPackageRejected {
                package_name: name.clone(),
            });
        }

        // Check: minimum trust tier.
        if trust_tier < self.policy.minimum_trust_tier {
            return Err(PluginError::SignatureVerificationFailed {
                package_name: name.clone(),
                reason: format!(
                    "package trust tier '{trust_tier}' is below minimum '{}'",
                    self.policy.minimum_trust_tier
                ),
            });
        }

        // Check: pinned signer identity (if configured).
        if !self.policy.pinned_identities.is_empty() {
            match &manifest.provenance.signer_identity {
                Some(identity) => {
                    if !self.policy.pinned_identities.contains(identity) {
                        return Err(PluginError::SignatureVerificationFailed {
                            package_name: name.clone(),
                            reason: format!(
                                "signer identity '{identity}' does not match any pinned identity"
                            ),
                        });
                    }
                    debug!(
                        plugin = %name,
                        identity = %identity,
                        "signer identity matches pinned identity"
                    );
                }
                None => {
                    return Err(PluginError::SignatureVerificationFailed {
                        package_name: name.clone(),
                        reason: "no signer identity present but pinned identities are configured"
                            .to_string(),
                    });
                }
            }
        }

        // Validate cosign bundle is present for Signed+ tiers.
        if manifest.provenance.cosign_bundle.is_none() {
            return Err(PluginError::SignatureVerificationFailed {
                package_name: name.clone(),
                reason: "cosign bundle is missing".to_string(),
            });
        }

        // Validate content digest is present if bundle is present.
        if manifest.provenance.content_digest.is_none() {
            warnings.push(format!(
                "Package '{name}' has a cosign signature but no content digest. \
                 Integrity cannot be fully verified."
            ));
        }

        info!(
            plugin = %name,
            trust_tier = %trust_tier,
            "package verification passed"
        );

        Ok(VerificationResult {
            trust_tier,
            accepted: true,
            summary: format!("package '{name}' verified at trust tier '{trust_tier}'"),
            warnings,
        })
    }
}

impl Default for PackageVerifier {
    fn default() -> Self {
        Self::strict()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{CapabilitiesSection, PluginSection, ProvenanceSection};
    use std::collections::HashMap;

    fn unsigned_manifest(name: &str) -> PluginManifest {
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

    fn signed_manifest(name: &str) -> PluginManifest {
        let mut m = unsigned_manifest(name);
        m.provenance.cosign_bundle = Some("eyJhbGciOi...".to_string());
        m.provenance.signer_identity = Some(
            "https://github.com/example/repo/.github/workflows/publish.yml@refs/heads/main"
                .to_string(),
        );
        m.provenance.content_digest = Some("abcdef1234".to_string());
        m
    }

    fn verified_manifest(name: &str) -> PluginManifest {
        let mut m = signed_manifest(name);
        m.provenance.slsa_provenance = Some("eyJ2ZXJzaW9uIjoi...".to_string());
        m.provenance.slsa_build_level = Some(2);
        m
    }

    #[test]
    fn strict_verifier_rejects_unsigned() {
        let verifier = PackageVerifier::strict();
        let manifest = unsigned_manifest("no-sig");

        let result = verifier.verify(&manifest);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("unsigned"));
        assert!(msg.contains("no-sig"));
    }

    #[test]
    fn permissive_verifier_accepts_unsigned_with_warnings() {
        let verifier = PackageVerifier::permissive();
        let manifest = unsigned_manifest("unsigned-ok");

        let result = verifier.verify(&manifest).expect("should accept");
        assert!(result.accepted);
        assert_eq!(result.trust_tier, TrustTier::Unsigned);
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn strict_verifier_accepts_signed() {
        let verifier = PackageVerifier::strict();
        let manifest = signed_manifest("signed-pkg");

        let result = verifier.verify(&manifest).expect("should accept");
        assert!(result.accepted);
        assert_eq!(result.trust_tier, TrustTier::Signed);
    }

    #[test]
    fn strict_verifier_accepts_verified() {
        let verifier = PackageVerifier::strict();
        let manifest = verified_manifest("verified-pkg");

        let result = verifier.verify(&manifest).expect("should accept");
        assert!(result.accepted);
        assert_eq!(result.trust_tier, TrustTier::Verified);
    }

    #[test]
    fn pinned_identity_mismatch_rejected() {
        let verifier = PackageVerifier::new(VerificationPolicy {
            allow_unsigned: false,
            minimum_trust_tier: TrustTier::Signed,
            pinned_identities: vec![
                "https://github.com/trusted/repo/.github/workflows/publish.yml@refs/heads/main"
                    .to_string(),
            ],
        });
        let manifest = signed_manifest("wrong-signer");

        let result = verifier.verify(&manifest);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("does not match"));
    }

    #[test]
    fn pinned_identity_match_accepted() {
        let verifier = PackageVerifier::new(VerificationPolicy {
            allow_unsigned: false,
            minimum_trust_tier: TrustTier::Signed,
            pinned_identities: vec![
                "https://github.com/example/repo/.github/workflows/publish.yml@refs/heads/main"
                    .to_string(),
            ],
        });
        let manifest = signed_manifest("right-signer");

        let result = verifier.verify(&manifest).expect("should accept");
        assert!(result.accepted);
    }

    #[test]
    fn minimum_trust_tier_enforcement() {
        let verifier = PackageVerifier::new(VerificationPolicy {
            allow_unsigned: false,
            minimum_trust_tier: TrustTier::Verified,
            pinned_identities: Vec::new(),
        });

        // Signed < Verified → should be rejected.
        let signed = signed_manifest("too-low");
        let result = verifier.verify(&signed);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("below minimum"));

        // Verified >= Verified → should be accepted.
        let verified = verified_manifest("high-enough");
        let result = verifier.verify(&verified).expect("should accept");
        assert!(result.accepted);
    }

    #[test]
    fn missing_content_digest_produces_warning() {
        let verifier = PackageVerifier::strict();
        let mut manifest = signed_manifest("no-digest");
        manifest.provenance.content_digest = None;

        let result = verifier.verify(&manifest).expect("should still accept");
        assert!(result.accepted);
        assert!(result.warnings.iter().any(|w| w.contains("content digest")));
    }

    #[test]
    fn default_verifier_is_strict() {
        let verifier = PackageVerifier::default();
        let manifest = unsigned_manifest("should-fail");
        assert!(verifier.verify(&manifest).is_err());
    }
}

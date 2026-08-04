//! W3C DID credential types and mock verification.
//!
//! This module provides experimental types for representing and verifying
//! W3C Decentralized Identifier (DID) credentials, as described in PRD-07
//! §6.2 (personhood gating).
//!
//! # Status
//!
//! This is a **mock implementation** suitable for integration testing and
//! feature-flag-gated experimentation. Production deployments will replace
//! the mock verifier with a real DID document resolver and cryptographic
//! proof verification.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// DID credential types
// ---------------------------------------------------------------------------

/// A W3C DID string (e.g. `"did:web:example.com"`, `"did:key:z6Mk..."`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Did(pub String);

impl Did {
    /// Returns `true` if the DID string starts with `"did:"` and contains
    /// at least one method segment (e.g. `"did:web:..."`, `"did:key:..."`).
    #[must_use]
    pub fn is_well_formed(&self) -> bool {
        let parts: Vec<&str> = self.0.splitn(3, ':').collect();
        parts.len() >= 3 && parts[0] == "did" && !parts[1].is_empty() && !parts[2].is_empty()
    }
}

impl std::fmt::Display for Did {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A simplified W3C Verifiable Credential carrying a personhood claim.
///
/// This is intentionally minimal — production implementations will carry
/// full JSON-LD `@context`, `proof`, and `credentialSubject` fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DidCredential {
    /// The DID of the credential subject (the entity claiming personhood).
    pub subject: Did,
    /// The DID of the issuer (the entity that verified personhood).
    pub issuer: Did,
    /// The type of credential (e.g. `"PersonhoodCredential"`).
    pub credential_type: String,
    /// ISO-8601 timestamp when the credential was issued.
    pub issued_at: String,
    /// ISO-8601 timestamp when the credential expires, if any.
    pub expires_at: Option<String>,
}

impl DidCredential {
    /// Create a new personhood credential.
    #[must_use]
    pub fn personhood(subject: Did, issuer: Did, issued_at: String) -> Self {
        Self {
            subject,
            issuer,
            credential_type: "PersonhoodCredential".to_string(),
            issued_at,
            expires_at: None,
        }
    }

    /// Set the expiry timestamp.
    #[must_use]
    pub fn with_expiry(mut self, expires_at: String) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// Returns `true` if this credential is a personhood credential.
    #[must_use]
    pub fn is_personhood(&self) -> bool {
        self.credential_type == "PersonhoodCredential"
    }
}

// ---------------------------------------------------------------------------
// Verification result
// ---------------------------------------------------------------------------

/// The outcome of verifying a [`DidCredential`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DidVerificationResult {
    /// The credential is valid.
    Valid,
    /// The credential is malformed (e.g. bad DID syntax).
    Malformed(String),
    /// The credential has expired.
    Expired,
    /// The issuer is not trusted.
    UntrustedIssuer(String),
}

impl DidVerificationResult {
    /// Returns `true` if verification succeeded.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }
}

// ---------------------------------------------------------------------------
// Mock verifier
// ---------------------------------------------------------------------------

/// A mock DID credential verifier for testing and experimentation.
///
/// Accepts any well-formed credential whose issuer DID is in the trusted
/// set. Does **not** perform cryptographic proof verification.
pub struct MockDidVerifier {
    trusted_issuers: Vec<Did>,
}

impl MockDidVerifier {
    /// Create a verifier that trusts the given issuer DIDs.
    #[must_use]
    pub fn new(trusted_issuers: Vec<Did>) -> Self {
        Self { trusted_issuers }
    }

    /// Create a verifier that trusts all issuers (for testing only).
    #[must_use]
    pub fn trust_all() -> Self {
        Self {
            trusted_issuers: Vec::new(),
        }
    }

    /// Verify the given credential.
    #[must_use]
    pub fn verify(&self, credential: &DidCredential) -> DidVerificationResult {
        if !credential.subject.is_well_formed() {
            return DidVerificationResult::Malformed("subject DID is not well-formed".to_string());
        }
        if !credential.issuer.is_well_formed() {
            return DidVerificationResult::Malformed("issuer DID is not well-formed".to_string());
        }

        // Check expiry if present.
        if let Some(expires_at) = &credential.expires_at {
            if let Ok(exp) = chrono::DateTime::parse_from_rfc3339(expires_at) {
                if exp < chrono::Utc::now() {
                    return DidVerificationResult::Expired;
                }
            }
        }

        // Trust-all mode: empty trusted_issuers means accept any issuer.
        if !self.trusted_issuers.is_empty() && !self.trusted_issuers.contains(&credential.issuer) {
            return DidVerificationResult::UntrustedIssuer(credential.issuer.0.clone());
        }

        DidVerificationResult::Valid
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_credential() -> DidCredential {
        DidCredential::personhood(
            Did("did:web:alice.example.com".to_string()),
            Did("did:web:issuer.example.com".to_string()),
            "2025-01-01T00:00:00Z".to_string(),
        )
    }

    #[test]
    fn did_well_formed() {
        assert!(Did("did:web:example.com".to_string()).is_well_formed());
        assert!(Did("did:key:z6MkTest".to_string()).is_well_formed());
        assert!(!Did("not-a-did".to_string()).is_well_formed());
        assert!(!Did("did:".to_string()).is_well_formed());
        assert!(!Did("did::missing".to_string()).is_well_formed());
    }

    #[test]
    fn credential_is_personhood() {
        let cred = sample_credential();
        assert!(cred.is_personhood());
    }

    #[test]
    fn mock_verifier_trust_all_accepts_valid() {
        let verifier = MockDidVerifier::trust_all();
        let result = verifier.verify(&sample_credential());
        assert!(result.is_valid());
    }

    #[test]
    fn mock_verifier_rejects_malformed_subject() {
        let verifier = MockDidVerifier::trust_all();
        let mut cred = sample_credential();
        cred.subject = Did("bad".to_string());
        assert!(matches!(
            verifier.verify(&cred),
            DidVerificationResult::Malformed(_)
        ));
    }

    #[test]
    fn mock_verifier_rejects_untrusted_issuer() {
        let verifier = MockDidVerifier::new(vec![Did("did:web:trusted.com".to_string())]);
        let result = verifier.verify(&sample_credential());
        assert!(matches!(result, DidVerificationResult::UntrustedIssuer(_)));
    }

    #[test]
    fn mock_verifier_accepts_trusted_issuer() {
        let verifier = MockDidVerifier::new(vec![Did("did:web:issuer.example.com".to_string())]);
        let result = verifier.verify(&sample_credential());
        assert!(result.is_valid());
    }

    #[test]
    fn mock_verifier_rejects_expired() {
        let verifier = MockDidVerifier::trust_all();
        let cred = sample_credential().with_expiry("2020-01-01T00:00:00Z".to_string());
        assert_eq!(verifier.verify(&cred), DidVerificationResult::Expired);
    }

    #[test]
    fn did_display() {
        let did = Did("did:web:example.com".to_string());
        assert_eq!(did.to_string(), "did:web:example.com");
    }

    #[test]
    fn credential_serde_round_trip() {
        let cred = sample_credential().with_expiry("2030-12-31T23:59:59Z".to_string());
        let json = serde_json::to_string(&cred).expect("serialize");
        let back: DidCredential = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.subject, cred.subject);
        assert_eq!(back.issuer, cred.issuer);
        assert_eq!(back.credential_type, cred.credential_type);
        assert_eq!(back.expires_at, cred.expires_at);
    }

    #[test]
    fn verification_result_serde_round_trip() {
        for result in [
            DidVerificationResult::Valid,
            DidVerificationResult::Expired,
            DidVerificationResult::Malformed("bad".to_string()),
            DidVerificationResult::UntrustedIssuer("did:web:bad.com".to_string()),
        ] {
            let json = serde_json::to_string(&result).expect("serialize");
            let back: DidVerificationResult = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(result, back);
        }
    }
}

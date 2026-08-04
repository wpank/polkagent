//! Experimental personhood gating (PRD-07 §6.2).
//!
//! This module is behind the **`personhood`** Cargo feature and provides:
//!
//! - [`PersonhoodRequirement`] — a Cedar-style policy annotation that
//!   declares a rule requires a W3C DID credential.
//! - [`PersonhoodPolicy`] — wraps a [`PolicyRule`] with an optional
//!   personhood requirement.
//! - [`check_personhood`] — verifies that a DID credential satisfies the
//!   requirement, intended to be called during grant resolution.
//!
//! # Design constraints (REQ-IDENT-07 / REQ-IDENT-08)
//!
//! - Personhood is **never** the sole authorization input.
//! - Non-personhood-gated flows are completely unaffected.
//! - The check is additive: it runs *after* the normal policy evaluation
//!   has already produced an `Allow` decision.

use serde::{Deserialize, Serialize};

use polkagent_identity::did::{DidCredential, DidVerificationResult, MockDidVerifier};

use crate::policy::{EvaluationContext, PolicyRule};

// ---------------------------------------------------------------------------
// Cedar-style personhood requirement
// ---------------------------------------------------------------------------

/// A Cedar-style policy annotation declaring that a rule requires the
/// principal to present a valid W3C DID credential proving personhood.
///
/// This is modelled after Cedar's `context has` expressions — the policy
/// states *what* credential type is required and optionally restricts the
/// set of acceptable issuers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonhoodRequirement {
    /// The credential type that must be presented (e.g.
    /// `"PersonhoodCredential"`).
    pub credential_type: String,
    /// If non-empty, the credential's issuer DID must appear in this list.
    #[serde(default)]
    pub trusted_issuers: Vec<String>,
}

impl PersonhoodRequirement {
    /// Create a requirement for a `PersonhoodCredential` from any issuer.
    #[must_use]
    pub fn any_issuer() -> Self {
        Self {
            credential_type: "PersonhoodCredential".to_string(),
            trusted_issuers: Vec::new(),
        }
    }

    /// Create a requirement for a `PersonhoodCredential` from specific
    /// trusted issuers.
    #[must_use]
    pub fn with_issuers(issuers: Vec<String>) -> Self {
        Self {
            credential_type: "PersonhoodCredential".to_string(),
            trusted_issuers: issuers,
        }
    }
}

// ---------------------------------------------------------------------------
// PersonhoodPolicy
// ---------------------------------------------------------------------------

/// A policy rule optionally annotated with a personhood requirement.
///
/// When `personhood` is `Some`, the rule's `Allow` effect is conditional on
/// the principal presenting a valid DID credential that satisfies the
/// requirement.  When `personhood` is `None`, the rule behaves identically
/// to a plain [`PolicyRule`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonhoodPolicy {
    /// The underlying policy rule.
    pub rule: PolicyRule,
    /// Optional personhood gating annotation.
    pub personhood: Option<PersonhoodRequirement>,
}

impl PersonhoodPolicy {
    /// Wrap a rule without personhood gating.
    #[must_use]
    pub fn ungated(rule: PolicyRule) -> Self {
        Self {
            rule,
            personhood: None,
        }
    }

    /// Wrap a rule with personhood gating.
    #[must_use]
    pub fn gated(rule: PolicyRule, requirement: PersonhoodRequirement) -> Self {
        Self {
            rule,
            personhood: Some(requirement),
        }
    }

    /// Returns `true` if this policy requires personhood verification.
    #[must_use]
    pub fn requires_personhood(&self) -> bool {
        self.personhood.is_some()
    }
}

// ---------------------------------------------------------------------------
// Personhood check result
// ---------------------------------------------------------------------------

/// The outcome of [`check_personhood`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersonhoodDecision {
    /// No personhood requirement — the check is a no-op.
    NotRequired,
    /// The credential satisfies the requirement.
    Satisfied,
    /// No credential was provided but one is required.
    MissingCredential,
    /// The credential was provided but failed verification.
    VerificationFailed(String),
    /// The credential type does not match the requirement.
    WrongCredentialType { expected: String, got: String },
}

impl PersonhoodDecision {
    /// Returns `true` if the decision allows the request to proceed.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::NotRequired | Self::Satisfied)
    }

    /// Returns a human-readable denial reason, or `None` if the decision
    /// is not a denial.
    #[must_use]
    pub fn denial_reason(&self) -> Option<String> {
        match self {
            Self::NotRequired | Self::Satisfied => None,
            Self::MissingCredential => {
                Some("personhood credential required but not provided".to_string())
            }
            Self::VerificationFailed(reason) => Some(format!(
                "personhood credential verification failed: {reason}"
            )),
            Self::WrongCredentialType { expected, got } => Some(format!(
                "expected credential type '{expected}', got '{got}'"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// check_personhood
// ---------------------------------------------------------------------------

/// Check whether the given credential satisfies the personhood requirement.
///
/// This function is intended to be called during grant resolution when a
/// matching [`PersonhoodPolicy`] has `personhood: Some(...)`.
///
/// # Arguments
///
/// * `requirement` — The personhood requirement from the policy. Pass
///   `None` for rules that do not require personhood; the function returns
///   [`PersonhoodDecision::NotRequired`] immediately.
/// * `credential` — The DID credential presented by the principal. `None`
///   when the principal did not provide one.
/// * `_ctx` — The evaluation context (reserved for future use, e.g.
///   cross-referencing context attributes with credential claims).
pub fn check_personhood(
    requirement: Option<&PersonhoodRequirement>,
    credential: Option<&DidCredential>,
    _ctx: &EvaluationContext,
) -> PersonhoodDecision {
    let req = match requirement {
        Some(r) => r,
        None => return PersonhoodDecision::NotRequired,
    };

    let cred = match credential {
        Some(c) => c,
        None => return PersonhoodDecision::MissingCredential,
    };

    // Check credential type matches.
    if cred.credential_type != req.credential_type {
        return PersonhoodDecision::WrongCredentialType {
            expected: req.credential_type.clone(),
            got: cred.credential_type.clone(),
        };
    }

    // Build a mock verifier from the requirement's trusted issuers.
    let trusted: Vec<polkagent_identity::Did> = req
        .trusted_issuers
        .iter()
        .map(|s| polkagent_identity::Did(s.clone()))
        .collect();

    let verifier = if trusted.is_empty() {
        MockDidVerifier::trust_all()
    } else {
        MockDidVerifier::new(trusted)
    };

    match verifier.verify(cred) {
        DidVerificationResult::Valid => PersonhoodDecision::Satisfied,
        DidVerificationResult::Malformed(msg) => PersonhoodDecision::VerificationFailed(msg),
        DidVerificationResult::Expired => {
            PersonhoodDecision::VerificationFailed("credential has expired".to_string())
        }
        DidVerificationResult::UntrustedIssuer(issuer) => {
            PersonhoodDecision::VerificationFailed(format!("untrusted issuer: {issuer}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use polkagent_identity::Did;

    use super::*;
    use crate::policy::{Effect, PolicyRule};

    fn sample_rule() -> PolicyRule {
        PolicyRule {
            id: "test-rule".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["chain/**".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        }
    }

    fn sample_credential() -> DidCredential {
        DidCredential::personhood(
            Did("did:web:alice.example.com".to_string()),
            Did("did:web:issuer.example.com".to_string()),
            "2025-01-01T00:00:00Z".to_string(),
        )
    }

    fn empty_ctx() -> EvaluationContext {
        EvaluationContext::default()
    }

    // ---- PersonhoodPolicy construction -----------------------------------

    #[test]
    fn ungated_policy_does_not_require_personhood() {
        let policy = PersonhoodPolicy::ungated(sample_rule());
        assert!(!policy.requires_personhood());
    }

    #[test]
    fn gated_policy_requires_personhood() {
        let policy = PersonhoodPolicy::gated(sample_rule(), PersonhoodRequirement::any_issuer());
        assert!(policy.requires_personhood());
    }

    // ---- check_personhood ------------------------------------------------

    #[test]
    fn no_requirement_returns_not_required() {
        let decision = check_personhood(None, None, &empty_ctx());
        assert_eq!(decision, PersonhoodDecision::NotRequired);
        assert!(decision.is_ok());
        assert!(decision.denial_reason().is_none());
    }

    #[test]
    fn requirement_without_credential_returns_missing() {
        let req = PersonhoodRequirement::any_issuer();
        let decision = check_personhood(Some(&req), None, &empty_ctx());
        assert_eq!(decision, PersonhoodDecision::MissingCredential);
        assert!(!decision.is_ok());
        assert!(decision.denial_reason().is_some());
    }

    #[test]
    fn valid_credential_satisfies_any_issuer() {
        let req = PersonhoodRequirement::any_issuer();
        let cred = sample_credential();
        let decision = check_personhood(Some(&req), Some(&cred), &empty_ctx());
        assert_eq!(decision, PersonhoodDecision::Satisfied);
        assert!(decision.is_ok());
    }

    #[test]
    fn valid_credential_satisfies_trusted_issuer() {
        let req =
            PersonhoodRequirement::with_issuers(vec!["did:web:issuer.example.com".to_string()]);
        let cred = sample_credential();
        let decision = check_personhood(Some(&req), Some(&cred), &empty_ctx());
        assert_eq!(decision, PersonhoodDecision::Satisfied);
    }

    #[test]
    fn untrusted_issuer_fails() {
        let req = PersonhoodRequirement::with_issuers(vec!["did:web:other-issuer.com".to_string()]);
        let cred = sample_credential();
        let decision = check_personhood(Some(&req), Some(&cred), &empty_ctx());
        assert!(!decision.is_ok());
        assert!(matches!(
            decision,
            PersonhoodDecision::VerificationFailed(_)
        ));
    }

    #[test]
    fn wrong_credential_type_fails() {
        let req = PersonhoodRequirement {
            credential_type: "SybilResistanceCredential".to_string(),
            trusted_issuers: Vec::new(),
        };
        let cred = sample_credential();
        let decision = check_personhood(Some(&req), Some(&cred), &empty_ctx());
        assert!(matches!(
            decision,
            PersonhoodDecision::WrongCredentialType { .. }
        ));
    }

    #[test]
    fn expired_credential_fails() {
        let req = PersonhoodRequirement::any_issuer();
        let cred = sample_credential().with_expiry("2020-01-01T00:00:00Z".to_string());
        let decision = check_personhood(Some(&req), Some(&cred), &empty_ctx());
        assert!(!decision.is_ok());
    }

    #[test]
    fn malformed_subject_fails() {
        let req = PersonhoodRequirement::any_issuer();
        let mut cred = sample_credential();
        cred.subject = Did("bad-did".to_string());
        let decision = check_personhood(Some(&req), Some(&cred), &empty_ctx());
        assert!(matches!(
            decision,
            PersonhoodDecision::VerificationFailed(_)
        ));
    }

    // ---- Serde round-trip ------------------------------------------------

    #[test]
    fn personhood_requirement_serde_round_trip() {
        let req =
            PersonhoodRequirement::with_issuers(vec!["did:web:issuer.example.com".to_string()]);
        let json = serde_json::to_string(&req).expect("serialize");
        let back: PersonhoodRequirement = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.credential_type, "PersonhoodCredential");
        assert_eq!(back.trusted_issuers.len(), 1);
    }

    #[test]
    fn personhood_policy_serde_round_trip() {
        let policy = PersonhoodPolicy::gated(sample_rule(), PersonhoodRequirement::any_issuer());
        let json = serde_json::to_string(&policy).expect("serialize");
        let back: PersonhoodPolicy = serde_json::from_str(&json).expect("deserialize");
        assert!(back.requires_personhood());
        assert_eq!(back.rule.id, "test-rule");
    }

    // ---- Non-personhood flows unaffected ---------------------------------

    #[test]
    fn ungated_check_is_noop() {
        let policy = PersonhoodPolicy::ungated(sample_rule());
        let decision = check_personhood(policy.personhood.as_ref(), None, &empty_ctx());
        assert_eq!(decision, PersonhoodDecision::NotRequired);
    }
}

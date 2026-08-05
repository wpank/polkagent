//! PRD-15 Security Tests: Data classification enforcement.
//!
//! These tests verify that the `DataClassification` system enforces access
//! boundaries correctly, including the critical `SecretForbidden` tier.

use polkagent_core::artifact::{Artifact, ArtifactKind};
use polkagent_core::config::DataClassification;
use polkagent_core::ids::ArtifactId;

// ===========================================================================
// SecretForbidden prevents data from reaching model context
// ===========================================================================

#[test]
fn secret_forbidden_artifact_is_not_context_safe() {
    let mut artifact = Artifact::from_bytes(
        ArtifactId::new(),
        ArtifactKind::Custom {
            type_uri: "secret://wallet-seed".to_string(),
        },
        b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    );
    artifact.classification = DataClassification::SecretForbidden;

    assert!(
        !artifact.is_context_safe(),
        "SecretForbidden artifact must NOT be context-safe"
    );
}

#[test]
fn secret_forbidden_rejects_is_context_safe() {
    // Directly test that is_context_safe returns false for SecretForbidden.
    let mut artifact = Artifact::from_bytes(
        ArtifactId::new(),
        ArtifactKind::Custom {
            type_uri: "secret://api-key".to_string(),
        },
        b"sk-ant-api03-SUPER_SECRET_KEY",
    );
    artifact.classification = DataClassification::SecretForbidden;

    let safe = artifact.is_context_safe();
    assert!(
        !safe,
        "is_context_safe() must return false for SecretForbidden"
    );
}

// ===========================================================================
// Other classifications allow context access appropriately
// ===========================================================================

#[test]
fn public_artifact_is_context_safe() {
    let artifact = Artifact::from_bytes(
        ArtifactId::new(),
        ArtifactKind::Plan {
            format: "json".to_string(),
        },
        b"{}",
    );
    assert_eq!(artifact.classification, DataClassification::Public);
    assert!(
        artifact.is_context_safe(),
        "Public artifact must be context-safe"
    );
}

#[test]
fn internal_artifact_is_context_safe() {
    let mut artifact = Artifact::from_bytes(
        ArtifactId::new(),
        ArtifactKind::Plan {
            format: "json".to_string(),
        },
        b"internal data",
    );
    artifact.classification = DataClassification::Internal;
    assert!(
        artifact.is_context_safe(),
        "Internal artifact must be context-safe (restricted access but allowed in model context)"
    );
}

#[test]
fn private_artifact_is_context_safe() {
    let mut artifact = Artifact::from_bytes(
        ArtifactId::new(),
        ArtifactKind::Plan {
            format: "json".to_string(),
        },
        b"private data",
    );
    artifact.classification = DataClassification::Private;
    assert!(
        artifact.is_context_safe(),
        "Private artifact must be context-safe (restricted but not forbidden from context)"
    );
}

#[test]
fn sensitive_artifact_is_context_safe() {
    let mut artifact = Artifact::from_bytes(
        ArtifactId::new(),
        ArtifactKind::Plan {
            format: "json".to_string(),
        },
        b"sensitive PII data",
    );
    artifact.classification = DataClassification::Sensitive;
    assert!(
        artifact.is_context_safe(),
        "Sensitive artifact must be context-safe (requires access controls, but not forbidden)"
    );
}

// ===========================================================================
// Classification ordering
// ===========================================================================

#[test]
fn classification_ordering_public_lt_internal() {
    assert!(DataClassification::Public < DataClassification::Internal);
}

#[test]
fn classification_ordering_internal_lt_private() {
    assert!(DataClassification::Internal < DataClassification::Private);
}

#[test]
fn classification_ordering_private_lt_sensitive() {
    assert!(DataClassification::Private < DataClassification::Sensitive);
}

#[test]
fn classification_ordering_sensitive_lt_secret_forbidden() {
    assert!(DataClassification::Sensitive < DataClassification::SecretForbidden);
}

#[test]
fn classification_ordering_full_chain() {
    // Verify the complete ordering: Public < Internal < Private < Sensitive < SecretForbidden
    let levels = [
        DataClassification::Public,
        DataClassification::Internal,
        DataClassification::Private,
        DataClassification::Sensitive,
        DataClassification::SecretForbidden,
    ];

    for i in 0..levels.len() {
        for j in (i + 1)..levels.len() {
            assert!(
                levels[i] < levels[j],
                "{:?} must be less than {:?}",
                levels[i],
                levels[j]
            );
        }
    }
}

#[test]
fn secret_forbidden_is_the_maximum_classification() {
    let all = [
        DataClassification::Public,
        DataClassification::Internal,
        DataClassification::Private,
        DataClassification::Sensitive,
        DataClassification::SecretForbidden,
    ];
    for cls in &all {
        assert!(
            *cls <= DataClassification::SecretForbidden,
            "{cls:?} must be <= SecretForbidden"
        );
    }
}

// ===========================================================================
// Classification serde round-trip integrity
// ===========================================================================

#[test]
fn classification_serde_round_trip_preserves_value() {
    let classifications = [
        DataClassification::Public,
        DataClassification::Internal,
        DataClassification::Private,
        DataClassification::Sensitive,
        DataClassification::SecretForbidden,
    ];

    for cls in &classifications {
        let json = serde_json::to_string(cls)
            .unwrap_or_else(|e| panic!("failed to serialize {cls:?}: {e}"));
        let back: DataClassification = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("failed to deserialize {cls:?}: {e}"));
        assert_eq!(
            *cls, back,
            "serde round-trip must preserve classification value for {cls:?}"
        );
    }
}

#[test]
fn classification_default_is_public() {
    assert_eq!(
        DataClassification::default(),
        DataClassification::Public,
        "default classification must be Public (least restrictive)"
    );
}

// ===========================================================================
// Classification violation error type
// ===========================================================================

#[test]
fn classification_violation_error_references_classification() {
    let err = polkagent_core::PolkagentError::ClassificationViolation {
        classification: "secret_forbidden".to_string(),
        reason: "item excluded from context assembly".to_string(),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("secret_forbidden"),
        "classification violation error must reference the classification tier"
    );
    assert!(
        msg.contains("context assembly"),
        "classification violation error must explain what boundary was violated"
    );
}

//! Configuration primitives shared across the Polkagent platform.
//!
//! This module defines fundamental configuration vocabulary types such as
//! autonomy levels and data classification tiers. These are intentionally
//! kept simple and free of heavy dependencies so that they can be embedded in
//! every layer of the stack.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// AutonomyLevel
// ---------------------------------------------------------------------------

/// The degree of autonomous decision-making granted to an agent.
///
/// Higher autonomy levels allow the agent to perform more effects without
/// requiring human approval. The platform's default is
/// [`AutonomyLevel::Supervised`].
///
/// **Security note:** Autonomy level is an *input* to grant resolution — it
/// cannot override explicit policy denials or approval requirements.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    /// Every effect requires explicit human approval before execution.
    FullySupervised,
    /// Most read/query effects are autonomous; write and chain effects require
    /// approval.
    #[default]
    Supervised,
    /// Routine effects execute autonomously; high-risk effects (chain
    /// transactions, signing) still require approval.
    AssistedAutonomous,
    /// All declared effects execute without per-call approval, subject to the
    /// agent's configured grant limits and budgets.
    FullyAutonomous,
}

impl std::fmt::Display for AutonomyLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::FullySupervised => "fully_supervised",
            Self::Supervised => "supervised",
            Self::AssistedAutonomous => "assisted_autonomous",
            Self::FullyAutonomous => "fully_autonomous",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// DataClassification
// ---------------------------------------------------------------------------

/// Classification tier for artifacts, events, and context items.
///
/// The ordering is `Public < Internal < Private < Sensitive < SecretForbidden`:
/// a higher-classified item is *more* restricted. The platform enforces
/// classification boundaries at every output boundary — context assembly,
/// telemetry sinks, and projection endpoints.
///
/// # Invariant
///
/// Items classified [`DataClassification::SecretForbidden`] must never appear
/// in model context, logs, telemetry, or any other externally visible output.
/// This is enforced by the `ClassificationGuard` in `polkagent-kernel`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum DataClassification {
    /// May appear in public projections, logs, and model context.
    #[default]
    Public,
    /// Visible within the organization; excluded from public-facing views.
    Internal,
    /// Visible to authorized users and operators; excluded from public views.
    Private,
    /// Requires additional access controls; excluded from default operator
    /// views. Suitable for PII and financial data.
    Sensitive,
    /// Must never contain secret material. Any item in this tier is excluded
    /// from context assembly, telemetry, and log export.
    ///
    /// Violated by: wallet seeds, raw API keys, unredacted bearer tokens.
    SecretForbidden,
}

impl std::fmt::Display for DataClassification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Private => "private",
            Self::Sensitive => "sensitive",
            Self::SecretForbidden => "secret_forbidden",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "configuration tests intentionally panic when static serialization fixtures fail"
)]
mod tests {
    use super::*;

    #[test]
    fn autonomy_level_ordering() {
        assert!(AutonomyLevel::FullySupervised < AutonomyLevel::Supervised);
        assert!(AutonomyLevel::Supervised < AutonomyLevel::AssistedAutonomous);
        assert!(AutonomyLevel::AssistedAutonomous < AutonomyLevel::FullyAutonomous);
    }

    #[test]
    fn autonomy_level_default_is_supervised() {
        assert_eq!(AutonomyLevel::default(), AutonomyLevel::Supervised);
    }

    #[test]
    fn autonomy_level_serde_round_trip() {
        let level = AutonomyLevel::AssistedAutonomous;
        let json = serde_json::to_string(&level).unwrap();
        assert_eq!(json, r#""assisted_autonomous""#);
        let back: AutonomyLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, level);
    }

    #[test]
    fn data_classification_ordering() {
        assert!(DataClassification::Public < DataClassification::Internal);
        assert!(DataClassification::Internal < DataClassification::Private);
        assert!(DataClassification::Private < DataClassification::Sensitive);
        assert!(DataClassification::Sensitive < DataClassification::SecretForbidden);
    }

    #[test]
    fn data_classification_default_is_public() {
        assert_eq!(DataClassification::default(), DataClassification::Public);
    }

    #[test]
    fn data_classification_serde_round_trip() {
        let cls = DataClassification::SecretForbidden;
        let json = serde_json::to_string(&cls).unwrap();
        assert_eq!(json, r#""secret_forbidden""#);
        let back: DataClassification = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cls);
    }

    #[test]
    fn secret_forbidden_is_max_classification() {
        let all = [
            DataClassification::Public,
            DataClassification::Internal,
            DataClassification::Private,
            DataClassification::Sensitive,
            DataClassification::SecretForbidden,
        ];
        for &cls in &all {
            assert!(cls <= DataClassification::SecretForbidden);
        }
    }
}

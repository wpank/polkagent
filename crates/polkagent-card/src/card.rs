//! The [`ActionCard`] type — the safety-critical UX surface for irreversible
//! on-chain effects.
//!
//! An action card is built by [`ActionCardBuilder`](crate::builder::ActionCardBuilder)
//! and rendered by [`render_text`](crate::render::render_text) or
//! [`render_tui`](crate::render::render_tui). It is never constructed directly
//! from model output; every field is either kernel-derived or explicitly
//! annotated as model-generated.
//!
//! # Design invariants
//!
//! 1. `canonical_sections` are always rendered *before* `narrative_sections`.
//! 2. `narrative_sections` always carry the disclaimer
//!    `"AI-generated explanation"`.
//! 3. `payload_hash` ties the approval decision to the exact signing payload;
//!    approving a card means approving that hash, not a model description.
//!
//! See PRD-13-UX-SURFACES §2.2, §2.3, and §7 for the full requirements.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::sections::{CanonicalSection, NarrativeSection, RiskFlag};

// ---------------------------------------------------------------------------
// RiskLevel
// ---------------------------------------------------------------------------

/// Overall risk classification of the proposed action.
///
/// The level is determined by the builder based on the union of risk flags
/// and any explicit override set by the caller (via
/// [`ActionCardBuilder::with_risk_level`](crate::builder::ActionCardBuilder::with_risk_level)).
///
/// Renderers and confirmation flows use this value to decide how many
/// confirmation steps are required before the user can approve:
///
/// - `Low` → single confirmation (e.g., `[y/N]`)
/// - `Medium` → explicit confirmation with payload hash display
/// - `High` → hold-to-approve or multi-step confirmation
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// No significant risk indicators. Standard confirmation required.
    #[default]
    Low,
    /// One or more medium-severity risk flags present. Enhanced confirmation
    /// required.
    Medium,
    /// One or more high-severity risk flags present. Maximum confirmation
    /// required (hold-to-approve or multi-step).
    High,
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "LOW"),
            Self::Medium => write!(f, "MEDIUM"),
            Self::High => write!(f, "HIGH"),
        }
    }
}

// ---------------------------------------------------------------------------
// SimulationSummary
// ---------------------------------------------------------------------------

/// Summary of a preflight dry-run simulation for the proposed action.
///
/// When available, simulation evidence is shown in the canonical section of
/// the card (PRD-13-UX-SURFACES §7.2). The presence of a `SimulationSummary`
/// means the action was successfully simulated; its absence (i.e.,
/// `Option::None`) means simulation was unavailable or was not performed.
///
/// Requirement UX-CARD-10: an absent or pending simulation must be explicitly
/// labeled — never silently omitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimulationSummary {
    /// Whether the simulated extrinsic succeeded.
    pub success: bool,

    /// Human-readable outcome message from the dry-run.
    pub outcome: String,

    /// Estimated fee from the simulation (e.g. `"0.0014 DOT"`).
    pub estimated_fee: Option<String>,

    /// Block number at which the simulation was performed. Allows the user
    /// to assess staleness.
    pub simulated_at_block: Option<u64>,

    /// The RPC endpoint or substrate node that performed the simulation.
    pub simulation_source: Option<String>,
}

impl SimulationSummary {
    /// Construct a minimal simulation summary.
    #[must_use]
    pub fn new(success: bool, outcome: impl Into<String>) -> Self {
        Self {
            success,
            outcome: outcome.into(),
            estimated_fee: None,
            simulated_at_block: None,
            simulation_source: None,
        }
    }

    /// Builder method: set the estimated fee.
    #[must_use]
    pub fn with_fee(mut self, fee: impl Into<String>) -> Self {
        self.estimated_fee = Some(fee.into());
        self
    }

    /// Builder method: set the block height of the simulation.
    #[must_use]
    pub fn at_block(mut self, block: u64) -> Self {
        self.simulated_at_block = Some(block);
        self
    }

    /// Builder method: set the simulation source (RPC endpoint).
    #[must_use]
    pub fn from_source(mut self, source: impl Into<String>) -> Self {
        self.simulation_source = Some(source.into());
        self
    }
}

// ---------------------------------------------------------------------------
// ActionCard
// ---------------------------------------------------------------------------

/// The canonical UX surface for an irreversible or value-moving on-chain
/// effect.
///
/// An `ActionCard` is the single source of truth for what the user is asked
/// to approve. It contains:
///
/// - **`canonical_sections`** — typed, metadata-derived facts shown first.
/// - **`narrative_sections`** — model-generated explanations shown second,
///   always with the "AI-generated explanation" disclaimer.
/// - **`risk_flags`** — canonical risk conditions derived from policy rules.
/// - **`risk_level`** — the overall risk classification.
/// - **`payload_hash`** — hex-encoded hash of the exact signing payload; the
///   approval decision is bound to this hash.
/// - **`simulation_result`** — optional preflight evidence.
/// - **`expires_at`** — optional deadline after which the card is invalid.
/// - **`created_at`** — when the card was constructed (UTC).
///
/// # Constructing a card
///
/// Always use [`ActionCardBuilder`](crate::builder::ActionCardBuilder):
///
/// ```rust
/// use polkagent_card::builder::ActionCardBuilder;
/// use polkagent_card::sections::{SectionSource, RiskFlag, RiskFlagType, Severity};
///
/// let card = ActionCardBuilder::new("Transfer 10 DOT")
///     .add_canonical("Amount", "10 DOT", SectionSource::Metadata)
///     .add_canonical("Recipient", "5GrwvaEF…", SectionSource::Chain)
///     .add_narrative("Why?", "The agent is rebalancing your staking position.")
///     .add_risk_flag(RiskFlag::new(
///         RiskFlagType::FirstTimeRecipient,
///         "Recipient not seen before",
///         Severity::Medium,
///     ))
///     .with_payload_hash("deadbeef1234")
///     .build();
///
/// assert!(!card.canonical_sections.is_empty());
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionCard {
    /// Stable identifier for this card instance (UUID v7 string).
    pub card_id: String,

    /// Short human-readable title describing the proposed action
    /// (e.g. `"Transfer 10 DOT"`).
    pub title: String,

    /// Structured key/value rows derived from typed metadata, chain queries,
    /// or simulation evidence. **Never populated from model output.**
    ///
    /// These are always rendered *before* `narrative_sections`.
    pub canonical_sections: Vec<CanonicalSection>,

    /// Model-generated prose explanations rendered *after* all canonical
    /// sections. Each entry always carries the disclaimer
    /// `"AI-generated explanation"`.
    pub narrative_sections: Vec<NarrativeSection>,

    /// Overall risk classification inferred from `risk_flags` or set
    /// explicitly by the caller.
    pub risk_level: RiskLevel,

    /// Individual risk conditions detected for this action. Derived from
    /// canonical data and policy rules; never from model assessment.
    pub risk_flags: Vec<RiskFlag>,

    /// Hex-encoded hash of the exact signing payload. Approving this card
    /// means approving this hash — not a model's description of the action.
    pub payload_hash: String,

    /// Preflight simulation result when available.
    pub simulation_result: Option<SimulationSummary>,

    /// Optional deadline. After `expires_at` the card must not be approved.
    pub expires_at: Option<DateTime<Utc>>,

    /// When this card was created (wall clock, UTC).
    pub created_at: DateTime<Utc>,
}

impl ActionCard {
    /// Returns `true` if the card has expired relative to `now`.
    ///
    /// A card with no expiry never expires.
    #[must_use]
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|exp| now >= exp)
    }

    /// Returns `true` if every canonical section is verified.
    #[must_use]
    pub fn all_canonical_verified(&self) -> bool {
        self.canonical_sections.iter().all(|s| s.verified)
    }

    /// Returns the highest severity among all risk flags, or `None` if there
    /// are no flags.
    #[must_use]
    pub fn max_severity(&self) -> Option<&crate::sections::Severity> {
        self.risk_flags.iter().map(|f| &f.severity).max()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::ActionCardBuilder;

    #[test]
    fn risk_level_ordering() {
        assert!(RiskLevel::High > RiskLevel::Medium);
        assert!(RiskLevel::Medium > RiskLevel::Low);
    }

    #[test]
    fn risk_level_display() {
        assert_eq!(RiskLevel::Low.to_string(), "LOW");
        assert_eq!(RiskLevel::Medium.to_string(), "MEDIUM");
        assert_eq!(RiskLevel::High.to_string(), "HIGH");
    }

    #[test]
    fn simulation_summary_builder_methods() {
        let sim = SimulationSummary::new(true, "ok")
            .with_fee("0.001 DOT")
            .at_block(12345)
            .from_source("wss://rpc.polkadot.io");

        assert!(sim.success);
        assert_eq!(sim.estimated_fee.as_deref(), Some("0.001 DOT"));
        assert_eq!(sim.simulated_at_block, Some(12345));
        assert_eq!(
            sim.simulation_source.as_deref(),
            Some("wss://rpc.polkadot.io")
        );
    }

    #[test]
    fn is_expired_with_past_deadline() {
        let card = ActionCardBuilder::new("Transfer 1 DOT")
            .with_payload_hash("aabbcc")
            .build();

        // Card with no expiry never expires.
        assert!(!card.is_expired(Utc::now()));
    }

    #[test]
    fn is_expired_with_deadline_in_past() {
        use chrono::Duration;

        let past = Utc::now() - Duration::seconds(60);
        let card = ActionCardBuilder::new("Transfer 1 DOT")
            .with_payload_hash("aabbcc")
            .with_expires_at(past)
            .build();

        assert!(card.is_expired(Utc::now()));
    }

    #[test]
    fn all_canonical_verified_false_when_any_unverified() {
        use crate::sections::{CanonicalSection, SectionSource};

        let card = ActionCardBuilder::new("Transfer 1 DOT")
            .add_canonical_raw(CanonicalSection::new(
                "Amount",
                "1 DOT",
                SectionSource::Metadata,
                false, // unverified
            ))
            .with_payload_hash("aabbcc")
            .build();

        assert!(!card.all_canonical_verified());
    }
}

//! [`ActionCardBuilder`] — the only way to construct an [`ActionCard`].
//!
//! The builder enforces the key display-order invariant required by
//! PRD-13-UX-SURFACES §2.2:
//!
//! > Canonical sections ALWAYS come first in display. Model narrative ALWAYS
//! > has a disclaimer header.
//!
//! # Example
//!
//! ```rust
//! use polkagent_card::builder::ActionCardBuilder;
//! use polkagent_card::sections::{RiskFlag, RiskFlagType, SectionSource, Severity};
//! use polkagent_card::card::RiskLevel;
//!
//! let card = ActionCardBuilder::new("Transfer 10 DOT")
//!     .add_canonical("Amount", "10 DOT", SectionSource::Metadata)
//!     .add_canonical("Recipient", "5GrwvaEF…", SectionSource::Chain)
//!     .add_narrative("Why?", "Rebalancing staking position.")
//!     .add_risk_flag(RiskFlag::new(
//!         RiskFlagType::HighValue,
//!         "Amount exceeds 5 DOT threshold",
//!         Severity::Medium,
//!     ))
//!     .with_payload_hash("deadbeef1234")
//!     .with_risk_level(RiskLevel::Medium)
//!     .build();
//!
//! // Canonical sections always precede narrative sections.
//! let positions: Vec<_> = card
//!     .canonical_sections
//!     .iter()
//!     .map(|s| s.label.as_str())
//!     .collect();
//! assert_eq!(positions, ["Amount", "Recipient"]);
//! assert_eq!(card.narrative_sections[0].label, "Why?");
//! ```

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::card::{ActionCard, RiskLevel, SimulationSummary};
use crate::sections::{CanonicalSection, NarrativeSection, RiskFlag, SectionSource};

// ---------------------------------------------------------------------------
// ActionCardBuilder
// ---------------------------------------------------------------------------

/// Fluent builder for [`ActionCard`].
///
/// The builder collects canonical sections, narrative sections, risk flags, and
/// metadata in separate buffers, then assembles a final [`ActionCard`] where
/// canonical sections are guaranteed to precede narrative sections.
///
/// # Invariants enforced at `build()`
///
/// 1. Canonical sections appear before narrative sections regardless of the
///    order in which builder methods were called.
/// 2. Every `NarrativeSection` carries the disclaimer `"AI-generated
///    explanation"` (set by [`NarrativeSection::new`]).
/// 3. `card_id` is a freshly generated UUID v7.
/// 4. `created_at` is set to the current UTC time.
#[derive(Debug)]
pub struct ActionCardBuilder {
    title: String,
    canonical_sections: Vec<CanonicalSection>,
    narrative_sections: Vec<NarrativeSection>,
    risk_flags: Vec<RiskFlag>,
    risk_level: Option<RiskLevel>,
    payload_hash: String,
    simulation_result: Option<SimulationSummary>,
    expires_at: Option<DateTime<Utc>>,
}

impl ActionCardBuilder {
    /// Begin building an [`ActionCard`] with the given `title`.
    ///
    /// The title should be a short action verb phrase such as
    /// `"Transfer 10 DOT"` or `"Vote on referendum #123"`.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            canonical_sections: Vec::new(),
            narrative_sections: Vec::new(),
            risk_flags: Vec::new(),
            risk_level: None,
            payload_hash: String::new(),
            simulation_result: None,
            expires_at: None,
        }
    }

    // -----------------------------------------------------------------------
    // Canonical sections
    // -----------------------------------------------------------------------

    /// Append a canonical section with the default `verified: true` flag.
    ///
    /// Use this when the value has been cross-checked against an authoritative
    /// source (metadata, chain storage, or simulation). For values that are
    /// canonical in origin but not yet verified, use
    /// [`add_canonical_unverified`](Self::add_canonical_unverified) or
    /// [`add_canonical_raw`](Self::add_canonical_raw).
    #[must_use]
    pub fn add_canonical(
        mut self,
        label: impl Into<String>,
        value: impl Into<String>,
        source: SectionSource,
    ) -> Self {
        self.canonical_sections
            .push(CanonicalSection::new(label, value, source, true));
        self
    }

    /// Append a canonical section with `verified: false`.
    ///
    /// Use this when the value is derived from a canonical source but has not
    /// been independently confirmed.
    #[must_use]
    pub fn add_canonical_unverified(
        mut self,
        label: impl Into<String>,
        value: impl Into<String>,
        source: SectionSource,
    ) -> Self {
        self.canonical_sections
            .push(CanonicalSection::new(label, value, source, false));
        self
    }

    /// Append a pre-constructed [`CanonicalSection`] directly.
    ///
    /// Useful when the caller needs full control over the `verified` flag or
    /// when constructing sections programmatically.
    #[must_use]
    pub fn add_canonical_raw(mut self, section: CanonicalSection) -> Self {
        self.canonical_sections.push(section);
        self
    }

    // -----------------------------------------------------------------------
    // Narrative sections
    // -----------------------------------------------------------------------

    /// Append a model-generated narrative section.
    ///
    /// The disclaimer `"AI-generated explanation"` is set automatically by
    /// [`NarrativeSection::new`].
    ///
    /// # Key rule
    ///
    /// All narrative sections are stored in a separate buffer and placed
    /// *after* all canonical sections in the final [`ActionCard`], regardless
    /// of when this method is called relative to
    /// [`add_canonical`](Self::add_canonical).
    #[must_use]
    pub fn add_narrative(mut self, label: impl Into<String>, content: impl Into<String>) -> Self {
        self.narrative_sections
            .push(NarrativeSection::new(label, content));
        self
    }

    // -----------------------------------------------------------------------
    // Risk
    // -----------------------------------------------------------------------

    /// Append a [`RiskFlag`] to the card.
    ///
    /// Risk flags are derived from canonical data and policy rules, never from
    /// model output.
    #[must_use]
    pub fn add_risk_flag(mut self, flag: RiskFlag) -> Self {
        self.risk_flags.push(flag);
        self
    }

    /// Override the overall risk level.
    ///
    /// If not called, [`build`](Self::build) infers the risk level from the
    /// highest severity among `risk_flags`: at least one `High` severity flag
    /// → `RiskLevel::High`; at least one `Medium` → `RiskLevel::Medium`;
    /// otherwise `RiskLevel::Low`.
    #[must_use]
    pub fn with_risk_level(mut self, level: RiskLevel) -> Self {
        self.risk_level = Some(level);
        self
    }

    // -----------------------------------------------------------------------
    // Payload & simulation
    // -----------------------------------------------------------------------

    /// Set the hex-encoded payload hash that this card's approval is bound to.
    ///
    /// The approval decision is tied to this exact hash; if the payload
    /// changes, a new card must be generated.
    #[must_use]
    pub fn with_payload_hash(mut self, hash: impl Into<String>) -> Self {
        self.payload_hash = hash.into();
        self
    }

    /// Attach a [`SimulationSummary`] (preflight dry-run evidence).
    #[must_use]
    pub fn with_simulation(mut self, result: SimulationSummary) -> Self {
        self.simulation_result = Some(result);
        self
    }

    // -----------------------------------------------------------------------
    // Metadata
    // -----------------------------------------------------------------------

    /// Set an expiry deadline for this card.
    ///
    /// After `expires_at` the card must not be approved
    /// (see [`ActionCard::is_expired`]).
    #[must_use]
    pub fn with_expires_at(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    // -----------------------------------------------------------------------
    // Build
    // -----------------------------------------------------------------------

    /// Finalise the builder and produce an [`ActionCard`].
    ///
    /// # Ordering guarantee
    ///
    /// `canonical_sections` are placed before `narrative_sections` in the
    /// returned card, unconditionally.
    ///
    /// # Risk level inference
    ///
    /// If [`with_risk_level`](Self::with_risk_level) was not called, the level
    /// is inferred from the `risk_flags` (highest severity wins).
    #[must_use]
    pub fn build(self) -> ActionCard {
        let risk_level = self
            .risk_level
            .unwrap_or_else(|| infer_risk_level(&self.risk_flags));

        ActionCard {
            card_id: Uuid::now_v7().to_string(),
            title: self.title,
            // KEY INVARIANT: canonical sections ALWAYS come first.
            canonical_sections: self.canonical_sections,
            narrative_sections: self.narrative_sections,
            risk_level,
            risk_flags: self.risk_flags,
            payload_hash: self.payload_hash,
            simulation_result: self.simulation_result,
            expires_at: self.expires_at,
            created_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Infer [`RiskLevel`] from the union of `risk_flags`.
///
/// - Any `Severity::High` flag → `RiskLevel::High`
/// - Any `Severity::Medium` flag (and no High) → `RiskLevel::Medium`
/// - No flags or all flags below Medium → `RiskLevel::Low`
fn infer_risk_level(flags: &[RiskFlag]) -> RiskLevel {
    use crate::sections::Severity;

    let max = flags.iter().map(|f| &f.severity).max();
    match max {
        Some(Severity::High) => RiskLevel::High,
        Some(Severity::Medium) => RiskLevel::Medium,
        None => RiskLevel::Low,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sections::{RiskFlagType, Severity};

    fn medium_flag() -> RiskFlag {
        RiskFlag::new(
            RiskFlagType::FirstTimeRecipient,
            "New recipient",
            Severity::Medium,
        )
    }

    fn high_flag() -> RiskFlag {
        RiskFlag::new(
            RiskFlagType::HighValue,
            "Amount is very large",
            Severity::High,
        )
    }

    #[test]
    fn builder_produces_valid_card() {
        let card = ActionCardBuilder::new("Transfer 10 DOT")
            .add_canonical("Amount", "10 DOT", SectionSource::Metadata)
            .add_canonical("Recipient", "5Grw…", SectionSource::Chain)
            .add_narrative("Why?", "Rebalancing.")
            .with_payload_hash("cafebabe")
            .build();

        assert_eq!(card.title, "Transfer 10 DOT");
        assert_eq!(card.payload_hash, "cafebabe");
        assert_eq!(card.canonical_sections.len(), 2);
        assert_eq!(card.narrative_sections.len(), 1);
        assert!(!card.card_id.is_empty());
    }

    #[test]
    fn canonical_sections_always_before_narrative() {
        // Even if we call add_narrative before add_canonical the final
        // card must put canonical sections first.
        let card = ActionCardBuilder::new("Test")
            .add_narrative("Explanation", "Some explanation.")
            .add_canonical("Amount", "1 DOT", SectionSource::Metadata)
            .add_narrative("Extra", "More text.")
            .add_canonical("Fee", "0.001 DOT", SectionSource::Simulation)
            .with_payload_hash("00")
            .build();

        // Canonical sections must be the first two in the ordered sense.
        // The card stores them separately so we check each Vec in isolation.
        assert_eq!(card.canonical_sections[0].label, "Amount");
        assert_eq!(card.canonical_sections[1].label, "Fee");
        assert_eq!(card.narrative_sections[0].label, "Explanation");
        assert_eq!(card.narrative_sections[1].label, "Extra");
    }

    #[test]
    fn narrative_disclaimer_always_set() {
        let card = ActionCardBuilder::new("Stake 50 DOT")
            .add_narrative("Context", "You are staking.")
            .with_payload_hash("abc")
            .build();

        for ns in &card.narrative_sections {
            assert_eq!(ns.disclaimer, "AI-generated explanation");
        }
    }

    #[test]
    fn risk_level_inferred_from_flags_none() {
        let card = ActionCardBuilder::new("Query")
            .with_payload_hash("00")
            .build();
        assert_eq!(card.risk_level, RiskLevel::Low);
    }

    #[test]
    fn risk_level_inferred_from_flags_medium() {
        let card = ActionCardBuilder::new("Transfer")
            .add_risk_flag(medium_flag())
            .with_payload_hash("00")
            .build();
        assert_eq!(card.risk_level, RiskLevel::Medium);
    }

    #[test]
    fn risk_level_inferred_from_flags_high() {
        let card = ActionCardBuilder::new("Transfer")
            .add_risk_flag(medium_flag())
            .add_risk_flag(high_flag())
            .with_payload_hash("00")
            .build();
        assert_eq!(card.risk_level, RiskLevel::High);
    }

    #[test]
    fn explicit_risk_level_overrides_inference() {
        // Even with a High flag, the caller can downgrade to Medium (not
        // recommended, but the API must be consistent).
        let card = ActionCardBuilder::new("Transfer")
            .add_risk_flag(high_flag())
            .with_risk_level(RiskLevel::Medium)
            .with_payload_hash("00")
            .build();
        assert_eq!(card.risk_level, RiskLevel::Medium);
    }

    #[test]
    fn simulation_result_attached() {
        let sim = SimulationSummary::new(true, "dry run ok").with_fee("0.002 DOT");
        let card = ActionCardBuilder::new("Transfer")
            .with_simulation(sim)
            .with_payload_hash("ff")
            .build();

        let result = card.simulation_result.expect("simulation should be set");
        assert!(result.success);
        assert_eq!(result.estimated_fee.as_deref(), Some("0.002 DOT"));
    }

    #[test]
    fn serialization_round_trip() {
        let card = ActionCardBuilder::new("Vote yes on #42")
            .add_canonical("Referendum", "#42", SectionSource::Chain)
            .add_canonical("Vote", "Aye (100 DOT)", SectionSource::Metadata)
            .add_narrative("Context", "This referendum proposes fee reduction.")
            .add_risk_flag(medium_flag())
            .with_payload_hash("deadbeef")
            .build();

        let json = serde_json::to_string(&card).expect("serialize");
        let back: ActionCard = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(card.card_id, back.card_id);
        assert_eq!(card.title, back.title);
        assert_eq!(card.payload_hash, back.payload_hash);
        assert_eq!(card.canonical_sections.len(), back.canonical_sections.len());
        assert_eq!(card.narrative_sections.len(), back.narrative_sections.len());
        assert_eq!(card.risk_level, back.risk_level);
    }

    #[test]
    fn risk_level_affects_confirmation_flow_description() {
        // High risk cards should convey the need for extra confirmation.
        // We test the RiskLevel value that a UI / CLI would branch on.
        let high_card = ActionCardBuilder::new("Proxy add")
            .add_risk_flag(high_flag())
            .with_payload_hash("00")
            .build();

        assert_eq!(high_card.risk_level, RiskLevel::High);

        // A confirmation flow would require multi-step / hold-to-approve for High.
        let requires_extra = matches!(high_card.risk_level, RiskLevel::High);
        assert!(requires_extra, "High risk must trigger extra confirmation");
    }
}

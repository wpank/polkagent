//! Section types for [`ActionCard`](crate::ActionCard).
//!
//! An action card contains two fundamentally different kinds of sections:
//!
//! - **Canonical sections** — derived from typed kernel records, chain
//!   metadata, or verifiable on-chain evidence. They are *never* populated
//!   from model output. Each field carries a [`SectionSource`] that records
//!   exactly where the value came from, and a `verified` flag that indicates
//!   whether the value was validated against an authoritative source.
//!
//! - **Narrative sections** — model-generated explanations that provide
//!   context to the user. They are always displayed *after* all canonical
//!   sections and always carry the disclaimer `"AI-generated explanation"`.
//!   They are read-only and must never contain interactive elements or
//!   contradict canonical data.
//!
//! See PRD-13-UX-SURFACES §2.2 (canonical/narrative separation) and §7
//! (action card specification) for the full requirements.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SectionSource
// ---------------------------------------------------------------------------

/// Records the authoritative origin of a canonical field value.
///
/// This enum drives both display (showing the user where a datum came from)
/// and trust scoring (metadata-derived values may be verified against the
/// chain; simulation-derived values are best-effort).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionSource {
    /// Decoded from SCALE-encoded chain metadata (pallet/call/event ABI).
    Metadata,
    /// Queried directly from a chain node (storage, RPC).
    Chain,
    /// Derived from a dry-run / preflight simulation.
    Simulation,
}

impl std::fmt::Display for SectionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Metadata => write!(f, "metadata"),
            Self::Chain => write!(f, "chain"),
            Self::Simulation => write!(f, "simulation"),
        }
    }
}

// ---------------------------------------------------------------------------
// CanonicalSection
// ---------------------------------------------------------------------------

/// A single key/value row in the canonical section of an [`ActionCard`](crate::ActionCard).
///
/// Canonical sections are populated exclusively from typed kernel records and
/// chain-derived evidence. They are never written by the AI model.
///
/// # Ordering guarantee
///
/// The [`ActionCardBuilder`](crate::builder::ActionCardBuilder) always places
/// all canonical sections before any narrative sections in the final card.
/// Renderers must preserve this order.
///
/// # Safety
///
/// A `verified: true` flag means the value was cross-checked against an
/// authoritative source (e.g., the on-chain storage root). An unverified
/// value is still canonical in origin but may not yet have been confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalSection {
    /// Human-readable label (e.g. `"Source account"`, `"Amount"`, `"Fee"`).
    pub label: String,

    /// The value to display (e.g. `"5GrwvaEF…"`, `"10 DOT"`, `"0.002 DOT"`).
    pub value: String,

    /// Where this value was obtained from.
    pub source: SectionSource,

    /// Whether the value has been verified against an authoritative source.
    ///
    /// Renderers should visually distinguish verified from unverified values
    /// (e.g., a checkmark vs. an open circle) to help the user assess trust.
    pub verified: bool,
}

impl CanonicalSection {
    /// Construct a new canonical section.
    #[must_use]
    pub fn new(
        label: impl Into<String>,
        value: impl Into<String>,
        source: SectionSource,
        verified: bool,
    ) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            source,
            verified,
        }
    }
}

// ---------------------------------------------------------------------------
// NarrativeSection
// ---------------------------------------------------------------------------

/// A model-generated explanation block appended to an [`ActionCard`](crate::ActionCard).
///
/// Narrative sections are always:
///
/// - Placed *after* all [`CanonicalSection`]s in display order.
/// - Visually distinguished from canonical sections (dimmed/mist styling in
///   the TUI, indented prose in plain text).
/// - Accompanied by the fixed disclaimer `"AI-generated explanation"`.
///
/// They are read-only. They must never contain interactive elements, links
/// that trigger actions, or data that contradicts the canonical sections above
/// them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NarrativeSection {
    /// Optional sub-heading for this narrative block (e.g. `"Why this action?"`).
    pub label: String,

    /// Prose text generated by the model.
    pub content: String,

    /// Mandatory disclaimer that must appear before or alongside `content`.
    ///
    /// Always `"AI-generated explanation"`. The field is public so renderers
    /// can position it independently, but its value is fixed at construction
    /// time and must not be altered.
    pub disclaimer: String,
}

impl NarrativeSection {
    /// Construct a new narrative section.
    ///
    /// The `disclaimer` field is always set to `"AI-generated explanation"`.
    #[must_use]
    pub fn new(label: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            content: content.into(),
            disclaimer: "AI-generated explanation".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// RiskFlag
// ---------------------------------------------------------------------------

/// The machine-readable type of a risk condition detected on the card.
///
/// Each variant corresponds to a specific, canonically defined trigger
/// condition (PRD-13-UX-SURFACES §7.5). Risk flags are derived from canonical
/// data and policy rules — never from model assessment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskFlagType {
    /// Transfer amount exceeds the operator-configured high-value threshold.
    HighValue,
    /// The recipient account has not been seen in this user's address history.
    FirstTimeRecipient,
    /// The destination address could not be verified against a known-address
    /// set or naming system.
    UnverifiedAddress,
    /// Chain metadata is older than the operator-configured staleness
    /// threshold; decoded call data may be incorrect.
    StaleMetadata,
    /// A token approval (e.g., ERC-20/PSP22 `approve`) grants a large or
    /// unlimited spending allowance.
    LargeApproval,
}

impl std::fmt::Display for RiskFlagType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HighValue => write!(f, "high_value"),
            Self::FirstTimeRecipient => write!(f, "first_time_recipient"),
            Self::UnverifiedAddress => write!(f, "unverified_address"),
            Self::StaleMetadata => write!(f, "stale_metadata"),
            Self::LargeApproval => write!(f, "large_approval"),
        }
    }
}

/// Severity level of a [`RiskFlag`].
///
/// Renderers use severity to select color: Medium → amber, High → red.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Notable but unlikely to result in loss if approved. Displayed in amber.
    Medium,
    /// Significant risk of loss or irreversible harm if approved. Displayed in
    /// red; may require additional confirmation step.
    High,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Medium => write!(f, "MEDIUM"),
            Self::High => write!(f, "HIGH"),
        }
    }
}

/// A single risk condition detected for an [`ActionCard`](crate::ActionCard).
///
/// Risk flags are derived from canonical kernel data and policy rules, not
/// from model output (PRD-13-UX-SURFACES §7.5 and requirement UX-CARD-07).
///
/// The combination of `flag_type`, `message`, and `severity` allows both
/// automated test assertions and human-readable display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskFlag {
    /// Machine-readable risk type used for programmatic handling and testing.
    pub flag_type: RiskFlagType,

    /// Human-readable explanation of why this flag was raised.
    pub message: String,

    /// How serious this risk is (affects display color and confirmation flow).
    pub severity: Severity,
}

impl RiskFlag {
    /// Construct a new risk flag.
    #[must_use]
    pub fn new(flag_type: RiskFlagType, message: impl Into<String>, severity: Severity) -> Self {
        Self {
            flag_type,
            message: message.into(),
            severity,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_section_new_fields_preserved() {
        let s = CanonicalSection::new("Amount", "10 DOT", SectionSource::Metadata, true);
        assert_eq!(s.label, "Amount");
        assert_eq!(s.value, "10 DOT");
        assert_eq!(s.source, SectionSource::Metadata);
        assert!(s.verified);
    }

    #[test]
    fn narrative_section_disclaimer_is_fixed() {
        let n = NarrativeSection::new("Why?", "This transfers funds.");
        assert_eq!(n.disclaimer, "AI-generated explanation");
    }

    #[test]
    fn risk_flag_serde_round_trip() {
        let flag = RiskFlag::new(
            RiskFlagType::HighValue,
            "Amount exceeds 1000 DOT threshold",
            Severity::High,
        );
        let json = serde_json::to_string(&flag).expect("serialize");
        let back: RiskFlag = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(flag, back);
    }

    #[test]
    fn section_source_display() {
        assert_eq!(SectionSource::Metadata.to_string(), "metadata");
        assert_eq!(SectionSource::Chain.to_string(), "chain");
        assert_eq!(SectionSource::Simulation.to_string(), "simulation");
    }

    #[test]
    fn severity_ordering() {
        assert!(Severity::High > Severity::Medium);
    }
}

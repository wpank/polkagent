//! Conversion from [`EffectIntent`] to [`ActionCard`].
//!
//! This module provides the [`ActionCard::from_effect_intent`] constructor
//! which derives the canonical card fields directly from a durable
//! [`EffectIntent`].  Because every field is pulled from kernel-owned data
//! (not from model output), the resulting card satisfies the canonical-section
//! invariants defined in PRD-13-UX-SURFACES §2.2.
//!
//! # Risk-level mapping
//!
//! | `EffectKind`         | `RiskLevel` |
//! |----------------------|-------------|
//! | `SignatureRequest`   | `High`      |
//! | `Broadcast`          | `High`      |
//! | `FinalityWatch`      | `High`      |
//! | `ToolCall`           | `Medium`    |
//! | `Delivery`           | `Medium`    |
//! | `HarnessOperation`   | `Medium`    |
//! | `ModelCall`          | `Low`       |
//! | `ChainRead`          | `Low`       |
//! | `Simulation`         | `Low`       |
//!
//! # Payload hash
//!
//! The payload hash is the BLAKE3 digest of the canonical JSON serialisation
//! of `intent.payload`, hex-encoded.  Because `serde_json` does not guarantee
//! map key order, we serialise through [`serde_json::to_vec`] and rely on the
//! fact that within a single process the same JSON value always serialises to
//! the same bytes (deterministic within a run).

use blake3::Hasher;
use serde_json;

use crate::builder::ActionCardBuilder;
use crate::card::{ActionCard, RiskLevel};
use crate::sections::SectionSource;

// Re-export the EffectIntent types we need without creating a circular dep:
// polkagent-card depends on polkagent-effect? No — that would be circular since
// polkagent-effect already depends on polkagent-card.
//
// Instead we define a minimal mirror of the fields we need via a local trait /
// struct so the conversion can be unit-tested inside polkagent-card without
// importing polkagent-effect. The orchestrator (in polkagent-run) calls
// ActionCard::from_effect_intent with the concrete polkagent_effect::EffectIntent.
//
// To avoid the circular dependency we expose the constructor as a *free function*
// that takes the fields we care about, and the orchestrator calls it.
// The ActionCard type lives in polkagent-card. The EffectIntent type lives in
// polkagent-effect which depends on polkagent-card. polkagent-card MUST NOT
// depend on polkagent-effect.
//
// Therefore: polkagent-card does NOT import polkagent-effect types.
// The `from_effect_intent` function lives in polkagent-effect (the crate that
// already knows about both types) or in polkagent-run, which depends on both.
//
// Concretely, this module defines:
//   - `EffectKindTag` — a mirror enum that callers fill in.
//   - `IntentCardSpec` — the minimal struct that the orchestrator populates
//     from an EffectIntent and passes to `ActionCard::from_spec`.
//
// This keeps polkagent-card dependency-free of polkagent-effect.

// ---------------------------------------------------------------------------
// EffectKindTag — mirror of polkagent_effect::EffectKind
// ---------------------------------------------------------------------------

/// Mirror of `polkagent_effect::EffectKind` used by the card constructor so
/// that `polkagent-card` does not need to depend on `polkagent-effect`.
///
/// Callers (e.g. the orchestrator in `polkagent-run`) convert from the real
/// `EffectKind` before calling [`ActionCard::from_spec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKindTag {
    ModelCall,
    ToolCall,
    SignatureRequest,
    Broadcast,
    FinalityWatch,
    Delivery,
    ChainRead,
    Simulation,
    HarnessOperation,
}

impl EffectKindTag {
    /// Human-readable label used in the canonical section.
    #[must_use]
    pub fn display_label(self) -> &'static str {
        match self {
            Self::ModelCall => "Model call",
            Self::ToolCall => "Tool call",
            Self::SignatureRequest => "Sign request",
            Self::Broadcast => "Broadcast",
            Self::FinalityWatch => "Finality watch",
            Self::Delivery => "Delivery",
            Self::ChainRead => "Chain read",
            Self::Simulation => "Simulation",
            Self::HarnessOperation => "Harness operation",
        }
    }

    /// Whether this effect kind requires an action card at all.
    ///
    /// Read-only, idempotent effects (`ChainRead`, `Simulation`) do not move
    /// value and do not require user approval, so no card is generated.
    #[must_use]
    pub fn requires_card(self) -> bool {
        !matches!(self, Self::ChainRead | Self::Simulation | Self::ModelCall)
    }

    /// The risk level for this effect kind.
    #[must_use]
    pub fn risk_level(self) -> RiskLevel {
        match self {
            Self::SignatureRequest | Self::Broadcast | Self::FinalityWatch => RiskLevel::High,
            Self::ToolCall | Self::Delivery | Self::HarnessOperation => RiskLevel::Medium,
            Self::ModelCall | Self::ChainRead | Self::Simulation => RiskLevel::Low,
        }
    }
}

// ---------------------------------------------------------------------------
// IntentCardSpec — minimal spec the orchestrator assembles from EffectIntent
// ---------------------------------------------------------------------------

/// The minimal set of fields the orchestrator extracts from an
/// [`EffectIntent`](polkagent_effect::EffectIntent) to build an
/// [`ActionCard`].
///
/// Using this intermediate struct keeps `polkagent-card` free of any
/// dependency on `polkagent-effect`.
#[derive(Debug, Clone)]
pub struct IntentCardSpec {
    /// Discriminant of the effect kind.
    pub kind: EffectKindTag,

    /// Stable string ID of the effect intent (UUID).
    pub effect_id: String,

    /// Run ID the intent belongs to.
    pub run_id: String,

    /// Pallet name extracted from the payload, if applicable.
    pub pallet: Option<String>,

    /// Call name extracted from the payload, if applicable.
    pub call: Option<String>,

    /// Human-readable argument summary derived from the payload.
    pub arguments_summary: Option<String>,

    /// Model-generated rationale for the action (from the assistant turn text).
    pub model_explanation: Option<String>,

    /// Estimated cost / fee, if known at card-generation time.
    pub estimated_cost: Option<String>,

    /// The raw JSON payload of the effect intent (used to compute the hash).
    pub payload_json: serde_json::Value,
}

impl IntentCardSpec {
    /// Build an [`ActionCard`] from this spec.
    ///
    /// Returns `None` if this effect kind does not require a card
    /// (i.e., `kind.requires_card()` is `false`).
    #[must_use]
    pub fn build_card(&self) -> Option<ActionCard> {
        if !self.kind.requires_card() {
            return None;
        }

        // Compute the payload hash — deterministic BLAKE3 of the canonical JSON.
        let payload_bytes = serde_json::to_vec(&self.payload_json).unwrap_or_default();
        let payload_hash = compute_payload_hash(&payload_bytes);

        // Build the title from kind + target if available.
        let title = build_title(self);

        let mut builder = ActionCardBuilder::new(title)
            .with_risk_level(self.kind.risk_level())
            .with_payload_hash(payload_hash);

        // Canonical sections — all from kernel-derived data.
        builder = builder.add_canonical(
            "Effect kind",
            self.kind.display_label(),
            SectionSource::Metadata,
        );

        builder = builder.add_canonical("Effect ID", &self.effect_id, SectionSource::Metadata);

        builder = builder.add_canonical("Run ID", &self.run_id, SectionSource::Metadata);

        if let Some(pallet) = &self.pallet {
            builder = builder.add_canonical("Pallet", pallet, SectionSource::Metadata);
        }

        if let Some(call) = &self.call {
            builder = builder.add_canonical("Call", call, SectionSource::Metadata);
        }

        if let Some(args) = &self.arguments_summary {
            builder = builder.add_canonical_unverified("Arguments", args, SectionSource::Metadata);
        }

        if let Some(cost) = &self.estimated_cost {
            builder = builder.add_canonical("Estimated cost", cost, SectionSource::Simulation);
        }

        // Narrative section — model-generated text, always after canonical.
        if let Some(explanation) = &self.model_explanation {
            if !explanation.trim().is_empty() {
                builder = builder.add_narrative("Model explanation", explanation);
            }
        }

        Some(builder.build())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute a hex-encoded BLAKE3 digest of `bytes`.
fn compute_payload_hash(bytes: &[u8]) -> String {
    let mut hasher = Hasher::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    // Format as lowercase hex string.
    digest
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Build a short human-readable card title from the spec.
fn build_title(spec: &IntentCardSpec) -> String {
    match (&spec.pallet, &spec.call) {
        (Some(pallet), Some(call)) => format!("{}: {}.{}", spec.kind.display_label(), pallet, call),
        (Some(pallet), None) => format!("{}: {}", spec.kind.display_label(), pallet),
        _ => spec.kind.display_label().to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_spec(kind: EffectKindTag) -> IntentCardSpec {
        IntentCardSpec {
            kind,
            effect_id: "eff-001".to_owned(),
            run_id: "run-001".to_owned(),
            pallet: Some("Balances".to_owned()),
            call: Some("transfer_keep_alive".to_owned()),
            arguments_summary: Some("dest=5GrwvaEF…, value=1000000".to_owned()),
            model_explanation: Some("Sending 1 DOT to alice.".to_owned()),
            estimated_cost: Some("0.0014 DOT".to_owned()),
            payload_json: serde_json::json!({"pallet_index": 4, "call_index": 3}),
        }
    }

    // ── Card generated for signable effects ─────────────────────────────────

    #[test]
    fn card_generated_for_signature_request() {
        let spec = make_spec(EffectKindTag::SignatureRequest);
        let card = spec.build_card();
        assert!(card.is_some(), "SignatureRequest must produce a card");
    }

    #[test]
    fn card_generated_for_broadcast() {
        let spec = make_spec(EffectKindTag::Broadcast);
        let card = spec.build_card();
        assert!(card.is_some(), "Broadcast must produce a card");
    }

    #[test]
    fn card_generated_for_tool_call() {
        let spec = make_spec(EffectKindTag::ToolCall);
        let card = spec.build_card();
        assert!(card.is_some(), "ToolCall must produce a card");
    }

    // ── Card NOT generated for read-only effects ─────────────────────────────

    #[test]
    fn card_not_generated_for_chain_read() {
        let spec = make_spec(EffectKindTag::ChainRead);
        let card = spec.build_card();
        assert!(card.is_none(), "ChainRead must not produce a card");
    }

    #[test]
    fn card_not_generated_for_simulation() {
        let spec = make_spec(EffectKindTag::Simulation);
        let card = spec.build_card();
        assert!(card.is_none(), "Simulation must not produce a card");
    }

    #[test]
    fn card_not_generated_for_model_call() {
        let spec = make_spec(EffectKindTag::ModelCall);
        let card = spec.build_card();
        assert!(card.is_none(), "ModelCall must not produce a card");
    }

    // ── Canonical fields match effect data ──────────────────────────────────

    #[test]
    fn card_canonical_fields_match_spec() {
        let spec = make_spec(EffectKindTag::SignatureRequest);
        let card = spec.build_card().unwrap();

        let labels: Vec<&str> = card
            .canonical_sections
            .iter()
            .map(|s| s.label.as_str())
            .collect();
        assert!(labels.contains(&"Effect kind"), "missing Effect kind");
        assert!(labels.contains(&"Effect ID"), "missing Effect ID");
        assert!(labels.contains(&"Pallet"), "missing Pallet");
        assert!(labels.contains(&"Call"), "missing Call");
        assert!(labels.contains(&"Arguments"), "missing Arguments");
        assert!(labels.contains(&"Estimated cost"), "missing Estimated cost");

        let kind_section = card
            .canonical_sections
            .iter()
            .find(|s| s.label == "Effect kind")
            .unwrap();
        assert_eq!(kind_section.value, "Sign request");

        let pallet_section = card
            .canonical_sections
            .iter()
            .find(|s| s.label == "Pallet")
            .unwrap();
        assert_eq!(pallet_section.value, "Balances");

        let call_section = card
            .canonical_sections
            .iter()
            .find(|s| s.label == "Call")
            .unwrap();
        assert_eq!(call_section.value, "transfer_keep_alive");
    }

    // ── Payload hash is deterministic ────────────────────────────────────────

    #[test]
    fn payload_hash_is_deterministic() {
        let payload = serde_json::json!({"pallet_index": 4, "call_index": 3});
        let bytes = serde_json::to_vec(&payload).unwrap();

        let hash1 = compute_payload_hash(&bytes);
        let hash2 = compute_payload_hash(&bytes);
        assert_eq!(
            hash1, hash2,
            "hash must be deterministic for the same input"
        );
        assert_eq!(hash1.len(), 64, "BLAKE3 hex is 64 chars");
    }

    #[test]
    fn different_payloads_produce_different_hashes() {
        let p1 = serde_json::to_vec(&serde_json::json!({"a": 1})).unwrap();
        let p2 = serde_json::to_vec(&serde_json::json!({"a": 2})).unwrap();
        assert_ne!(compute_payload_hash(&p1), compute_payload_hash(&p2));
    }

    // ── Card survives serde round-trip ───────────────────────────────────────

    #[test]
    fn card_serde_round_trip() {
        let spec = make_spec(EffectKindTag::SignatureRequest);
        let card = spec.build_card().unwrap();

        let json = serde_json::to_string(&card).expect("serialize");
        let back: crate::ActionCard = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(card.card_id, back.card_id);
        assert_eq!(card.title, back.title);
        assert_eq!(card.payload_hash, back.payload_hash);
        assert_eq!(card.risk_level, back.risk_level);
        assert_eq!(card.canonical_sections.len(), back.canonical_sections.len());
    }

    // ── Risk level correct for each effect kind ──────────────────────────────

    #[test]
    fn risk_level_high_for_signable() {
        for kind in [
            EffectKindTag::SignatureRequest,
            EffectKindTag::Broadcast,
            EffectKindTag::FinalityWatch,
        ] {
            assert_eq!(kind.risk_level(), RiskLevel::High, "{kind:?} must be High");
        }
    }

    #[test]
    fn risk_level_medium_for_tool_delivery_harness() {
        for kind in [
            EffectKindTag::ToolCall,
            EffectKindTag::Delivery,
            EffectKindTag::HarnessOperation,
        ] {
            assert_eq!(
                kind.risk_level(),
                RiskLevel::Medium,
                "{kind:?} must be Medium"
            );
        }
    }

    #[test]
    fn risk_level_low_for_read_only() {
        for kind in [
            EffectKindTag::ModelCall,
            EffectKindTag::ChainRead,
            EffectKindTag::Simulation,
        ] {
            assert_eq!(kind.risk_level(), RiskLevel::Low, "{kind:?} must be Low");
        }
    }

    // ── Title is built correctly ──────────────────────────────────────────────

    #[test]
    fn title_includes_pallet_and_call() {
        let spec = make_spec(EffectKindTag::Broadcast);
        let card = spec.build_card().unwrap();
        assert!(card.title.contains("Balances"), "title must include pallet");
        assert!(
            card.title.contains("transfer_keep_alive"),
            "title must include call"
        );
    }

    #[test]
    fn title_falls_back_to_kind_label_when_no_pallet() {
        let mut spec = make_spec(EffectKindTag::Broadcast);
        spec.pallet = None;
        spec.call = None;
        let card = spec.build_card().unwrap();
        assert_eq!(card.title, "Broadcast");
    }

    // ── Narrative section is present when explanation is provided ────────────

    #[test]
    fn narrative_section_present_for_explanation() {
        let spec = make_spec(EffectKindTag::SignatureRequest);
        let card = spec.build_card().unwrap();
        assert!(
            !card.narrative_sections.is_empty(),
            "explanation must produce a narrative"
        );
        assert_eq!(
            card.narrative_sections[0].disclaimer,
            "AI-generated explanation"
        );
    }

    #[test]
    fn no_narrative_section_when_explanation_is_none() {
        let mut spec = make_spec(EffectKindTag::SignatureRequest);
        spec.model_explanation = None;
        let card = spec.build_card().unwrap();
        assert!(
            card.narrative_sections.is_empty(),
            "no explanation → no narrative"
        );
    }

    // ── Canonical sections always precede narrative sections ─────────────────

    #[test]
    fn canonical_sections_not_empty_for_signable_effect() {
        let spec = make_spec(EffectKindTag::SignatureRequest);
        let card = spec.build_card().unwrap();
        // All fields provided: Effect kind, Effect ID, Run ID, Pallet, Call, Arguments, Estimated cost
        assert!(
            card.canonical_sections.len() >= 4,
            "expected at least 4 canonical sections, got {}",
            card.canonical_sections.len()
        );
    }
}

//! Property-based tests for `polkagent-card`.
//!
//! These tests use `proptest` to verify invariants of ActionCard construction,
//! risk level ordering, rendering, and section stability.

use proptest::prelude::*;

use polkagent_card::builder::ActionCardBuilder;
use polkagent_card::card::RiskLevel;
use polkagent_card::render::{render_text, render_tui};
use polkagent_card::sections::{CanonicalSection, RiskFlag, RiskFlagType, SectionSource, Severity};

// =========================================================================
// Helpers: strategies for generating domain values
// =========================================================================

/// Strategy for arbitrary non-empty strings (for labels, values, etc.).
fn arb_nonempty_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_ ]{1,40}"
}

/// Strategy for arbitrary `SectionSource`.
fn arb_section_source() -> impl Strategy<Value = SectionSource> {
    prop_oneof![
        Just(SectionSource::Metadata),
        Just(SectionSource::Chain),
        Just(SectionSource::Simulation),
    ]
}

/// Strategy for arbitrary `RiskLevel`.
fn arb_risk_level() -> impl Strategy<Value = RiskLevel> {
    prop_oneof![
        Just(RiskLevel::Low),
        Just(RiskLevel::Medium),
        Just(RiskLevel::High),
    ]
}

/// Strategy for arbitrary `CanonicalSection`.
fn arb_canonical_section() -> impl Strategy<Value = CanonicalSection> {
    (
        arb_nonempty_string(),
        arb_nonempty_string(),
        arb_section_source(),
        any::<bool>(),
    )
        .prop_map(|(label, value, source, verified)| {
            CanonicalSection::new(label, value, source, verified)
        })
}

// =========================================================================
// 1. Action card builder always produces a valid card
// =========================================================================

proptest! {
    /// Any combination of canonical sections, narrative sections, risk flags,
    /// and a payload hash produces a card with a non-empty card_id and the
    /// correct section counts.
    #[test]
    fn builder_always_produces_valid_card(
        title in arb_nonempty_string(),
        canonical_count in 0..5usize,
        narrative_count in 0..5usize,
        flag_count in 0..4usize,
        payload_hash in "[0-9a-f]{8,64}",
    ) {
        let mut builder = ActionCardBuilder::new(title.clone());

        for i in 0..canonical_count {
            builder = builder.add_canonical(
                format!("label_{i}"),
                format!("value_{i}"),
                SectionSource::Metadata,
            );
        }

        for i in 0..narrative_count {
            builder = builder.add_narrative(
                format!("heading_{i}"),
                format!("content_{i}"),
            );
        }

        for _ in 0..flag_count {
            builder = builder.add_risk_flag(RiskFlag::new(
                RiskFlagType::HighValue,
                "test flag",
                Severity::Medium,
            ));
        }

        let card = builder.with_payload_hash(payload_hash.clone()).build();

        // Card ID is always set (UUID v7 string).
        prop_assert!(!card.card_id.is_empty(), "card_id must be non-empty");

        // Title preserved.
        prop_assert_eq!(&card.title, &title);

        // Payload hash preserved.
        prop_assert_eq!(&card.payload_hash, &payload_hash);

        // Section counts match.
        prop_assert_eq!(card.canonical_sections.len(), canonical_count);
        prop_assert_eq!(card.narrative_sections.len(), narrative_count);
        prop_assert_eq!(card.risk_flags.len(), flag_count);

        // All narrative sections carry the fixed disclaimer.
        for ns in &card.narrative_sections {
            prop_assert_eq!(
                &ns.disclaimer, "AI-generated explanation",
                "narrative disclaimer must be fixed"
            );
        }
    }
}

// =========================================================================
// 2. Risk level ordering: Low < Medium < High
// =========================================================================

proptest! {
    /// The RiskLevel ordering invariant holds for all generated pairs.
    #[test]
    fn risk_level_ordering(a in arb_risk_level(), b in arb_risk_level()) {
        // Verify consistency with the derive(PartialOrd, Ord).
        match (&a, &b) {
            (RiskLevel::Low, RiskLevel::Medium) => prop_assert!(a < b),
            (RiskLevel::Low, RiskLevel::High) => prop_assert!(a < b),
            (RiskLevel::Medium, RiskLevel::High) => prop_assert!(a < b),
            (RiskLevel::Medium, RiskLevel::Low) => prop_assert!(a > b),
            (RiskLevel::High, RiskLevel::Low) => prop_assert!(a > b),
            (RiskLevel::High, RiskLevel::Medium) => prop_assert!(a > b),
            _ => prop_assert!(a == b, "equal variants must be equal"),
        }
    }

    /// Low is always strictly less than High (transitive through Medium).
    #[test]
    fn risk_level_low_always_less_than_high(_ in 0..1u8) {
        prop_assert!(RiskLevel::Low < RiskLevel::Medium);
        prop_assert!(RiskLevel::Medium < RiskLevel::High);
        prop_assert!(RiskLevel::Low < RiskLevel::High);
    }
}

// =========================================================================
// 3. Rendered text is non-empty for any valid card
// =========================================================================

proptest! {
    /// `render_text` always produces a non-empty string for any card.
    #[test]
    fn rendered_text_is_non_empty(
        title in arb_nonempty_string(),
        hash in "[0-9a-f]{8}",
    ) {
        let card = ActionCardBuilder::new(title.clone())
            .with_payload_hash(hash)
            .build();

        let text = render_text(&card);
        prop_assert!(!text.is_empty(), "render_text must produce non-empty output");
        prop_assert!(
            text.contains(&title),
            "rendered text must contain the card title"
        );
    }

    /// `render_tui` always produces at least one line for any card.
    #[test]
    fn rendered_tui_is_non_empty(
        title in arb_nonempty_string(),
        hash in "[0-9a-f]{8}",
    ) {
        let card = ActionCardBuilder::new(title)
            .with_payload_hash(hash)
            .build();

        let lines = render_tui(&card);
        prop_assert!(!lines.is_empty(), "render_tui must produce at least one line");
    }
}

// =========================================================================
// 4. Canonical section count never changes after construction
// =========================================================================

proptest! {
    /// The number of canonical sections in the built card exactly matches
    /// the number passed to the builder, and does not change when the card
    /// is serialized and deserialized.
    #[test]
    fn canonical_section_count_stable(
        sections in prop::collection::vec(arb_canonical_section(), 0..8),
    ) {
        let expected_count = sections.len();
        let mut builder = ActionCardBuilder::new("Test");
        for section in sections {
            builder = builder.add_canonical_raw(section);
        }
        let card = builder.with_payload_hash("aa").build();

        prop_assert_eq!(
            card.canonical_sections.len(),
            expected_count,
            "canonical section count must match builder input"
        );

        // Round-trip through serde to verify stability.
        let json = serde_json::to_string(&card).expect("serialize");
        let back: polkagent_card::ActionCard =
            serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(
            back.canonical_sections.len(),
            expected_count,
            "section count must survive serde round-trip"
        );
    }
}

// =========================================================================
// 5. Card with no sections renders without panic
// =========================================================================

proptest! {
    /// A card with zero canonical sections, zero narrative sections, and
    /// zero risk flags renders successfully via both renderers.
    #[test]
    fn empty_card_renders_without_panic(
        title in arb_nonempty_string(),
        hash in "[0-9a-f]{8}",
    ) {
        let card = ActionCardBuilder::new(title)
            .with_payload_hash(hash)
            .build();

        prop_assert!(card.canonical_sections.is_empty());
        prop_assert!(card.narrative_sections.is_empty());
        prop_assert!(card.risk_flags.is_empty());

        // Must not panic.
        let text = render_text(&card);
        let tui_lines = render_tui(&card);

        prop_assert!(!text.is_empty());
        prop_assert!(!tui_lines.is_empty());
    }
}

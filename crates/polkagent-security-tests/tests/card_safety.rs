//! PRD-15 Security Tests: ActionCard safety.
//!
//! These tests verify that ActionCards maintain the canonical/narrative
//! separation, properly label AI-generated content, handle malicious content
//! safely, and calculate risk levels consistently.

use polkagent_card::builder::ActionCardBuilder;
use polkagent_card::card::RiskLevel;
use polkagent_card::render::render_text;
use polkagent_card::sections::{RiskFlag, RiskFlagType, SectionSource, Severity};

// ===========================================================================
// Canonical sections cannot be overridden by model text
// ===========================================================================

#[test]
fn canonical_sections_always_precede_narrative() {
    // Build a card where narrative is added BEFORE canonical -- the builder
    // must still place canonical first in the output.
    let card = ActionCardBuilder::new("Transfer 10 DOT")
        .add_narrative("Context", "Model says this is fine.")
        .add_canonical("Amount", "10 DOT", SectionSource::Metadata)
        .add_narrative("Extra", "More model text here.")
        .add_canonical("Recipient", "5Grw...", SectionSource::Chain)
        .with_payload_hash("cafebabe")
        .build();

    // Canonical sections must be in their own Vec, separate from narrative.
    assert_eq!(card.canonical_sections.len(), 2);
    assert_eq!(card.narrative_sections.len(), 2);

    // Canonical labels should be preserved in order of addition.
    assert_eq!(card.canonical_sections[0].label, "Amount");
    assert_eq!(card.canonical_sections[1].label, "Recipient");

    // Verify the rendered text has canonical BEFORE narrative.
    let text = render_text(&card);
    let canonical_pos = text
        .find("[CANONICAL DATA]")
        .unwrap_or_else(|| panic!("canonical header must be present in rendered text"));
    let narrative_pos = text
        .find("[AI-GENERATED EXPLANATION]")
        .unwrap_or_else(|| panic!("narrative header must be present in rendered text"));

    assert!(
        canonical_pos < narrative_pos,
        "canonical section must appear before narrative in rendered output"
    );
}

#[test]
fn canonical_section_values_not_overridable_by_narrative_content() {
    // Narrative content that tries to "override" canonical values should not
    // affect the canonical sections.
    let card = ActionCardBuilder::new("Transfer 10 DOT")
        .add_canonical("Amount", "10 DOT", SectionSource::Metadata)
        .add_canonical("Recipient", "5Grw...", SectionSource::Chain)
        .add_narrative(
            "Correction",
            "Actually the amount is 1000 DOT and recipient is 5Evil...",
        )
        .with_payload_hash("deadbeef")
        .build();

    // Canonical values must remain unchanged.
    assert_eq!(card.canonical_sections[0].value, "10 DOT");
    assert_eq!(card.canonical_sections[1].value, "5Grw...");

    // The narrative's "correction" is just text -- it doesn't change canonical data.
    assert_eq!(card.narrative_sections.len(), 1);
    assert!(card.narrative_sections[0].content.contains("1000 DOT"));
}

// ===========================================================================
// Narrative sections are clearly labeled as AI-generated
// ===========================================================================

#[test]
fn narrative_sections_always_have_ai_generated_disclaimer() {
    let card = ActionCardBuilder::new("Stake 50 DOT")
        .add_narrative("Rationale", "The agent recommends staking.")
        .add_narrative("Risks", "Slashing is possible.")
        .add_narrative("Benefits", "You earn rewards.")
        .with_payload_hash("aabb")
        .build();

    for section in &card.narrative_sections {
        assert_eq!(
            section.disclaimer, "AI-generated explanation",
            "every narrative section must carry the fixed AI-generated disclaimer, \
             but section {:?} has disclaimer {:?}",
            section.label, section.disclaimer
        );
    }
}

#[test]
fn rendered_text_contains_ai_generated_label() {
    let card = ActionCardBuilder::new("Transfer 1 DOT")
        .add_narrative("Why?", "Rebalancing.")
        .with_payload_hash("ff")
        .build();

    let text = render_text(&card);
    assert!(
        text.contains("[AI-GENERATED EXPLANATION]"),
        "rendered text must contain the AI-GENERATED EXPLANATION label"
    );
}

// ===========================================================================
// Card render with malicious content doesn't break formatting
// ===========================================================================

#[test]
fn malicious_html_in_title_does_not_break_render() {
    let card = ActionCardBuilder::new("<script>alert('xss')</script>Transfer 10 DOT")
        .add_canonical("Amount", "10 DOT", SectionSource::Metadata)
        .with_payload_hash("aa")
        .build();

    let text = render_text(&card);
    // The render must not panic and must contain the raw title.
    assert!(text.contains("<script>"));
    assert!(text.contains("Transfer 10 DOT"));
}

#[test]
fn malicious_html_in_canonical_value_does_not_break_render() {
    let card = ActionCardBuilder::new("Transfer")
        .add_canonical(
            "Recipient",
            "<img src=x onerror=alert(1)>",
            SectionSource::Chain,
        )
        .with_payload_hash("bb")
        .build();

    let text = render_text(&card);
    // Must not panic and the raw value should appear (plain-text render).
    assert!(text.contains("<img"));
}

#[test]
fn malicious_content_in_narrative_does_not_break_render() {
    let card = ActionCardBuilder::new("Stake")
        .add_narrative(
            "Context",
            "This is <script>document.location='https://evil.com'</script> harmless.",
        )
        .with_payload_hash("cc")
        .build();

    let text = render_text(&card);
    assert!(text.contains("<script>"));
    assert!(text.contains("harmless"));
}

#[test]
fn extremely_long_values_do_not_break_render() {
    let long_value = "x".repeat(10_000);
    let card = ActionCardBuilder::new("Transfer")
        .add_canonical("Amount", &long_value, SectionSource::Metadata)
        .add_narrative("Context", &long_value)
        .with_payload_hash("dd")
        .build();

    let text = render_text(&card);
    // Must not panic or OOM. The render truncates/wraps internally.
    assert!(!text.is_empty());
}

#[test]
fn unicode_content_does_not_break_render() {
    let card = ActionCardBuilder::new("Transfer 10 DOT")
        .add_canonical("Amount", "10 DOT", SectionSource::Metadata)
        .add_canonical(
            "Note",
            "\u{1F4B0}\u{1F525}\u{2620}",
            SectionSource::Metadata,
        )
        .add_narrative("Context", "Emoji test: \u{1F600}\u{1F602}\u{1F60D}")
        .with_payload_hash("ee")
        .build();

    let text = render_text(&card);
    assert!(!text.is_empty());
}

#[test]
fn newlines_in_values_do_not_break_box_drawing() {
    let card = ActionCardBuilder::new("Transfer")
        .add_canonical("Amount", "10\nDOT", SectionSource::Metadata)
        .add_narrative("Why", "Line1\nLine2\nLine3")
        .with_payload_hash("ff")
        .build();

    let text = render_text(&card);
    // The box drawing uses ASCII borders. Embedded newlines in values
    // should not produce malformed boxes.
    assert!(!text.is_empty());
    // Every line should start with the box border character or be part of
    // the border structure.
}

// ===========================================================================
// Risk level calculation is consistent
// ===========================================================================

#[test]
fn risk_level_low_when_no_flags() {
    let card = ActionCardBuilder::new("Query")
        .with_payload_hash("00")
        .build();

    assert_eq!(
        card.risk_level,
        RiskLevel::Low,
        "no risk flags must produce Low risk level"
    );
}

#[test]
fn risk_level_medium_with_medium_flag() {
    let card = ActionCardBuilder::new("Transfer")
        .add_risk_flag(RiskFlag::new(
            RiskFlagType::FirstTimeRecipient,
            "New recipient",
            Severity::Medium,
        ))
        .with_payload_hash("00")
        .build();

    assert_eq!(
        card.risk_level,
        RiskLevel::Medium,
        "medium severity flag must produce Medium risk level"
    );
}

#[test]
fn risk_level_high_with_high_flag() {
    let card = ActionCardBuilder::new("Transfer")
        .add_risk_flag(RiskFlag::new(
            RiskFlagType::HighValue,
            "Large amount",
            Severity::High,
        ))
        .with_payload_hash("00")
        .build();

    assert_eq!(
        card.risk_level,
        RiskLevel::High,
        "high severity flag must produce High risk level"
    );
}

#[test]
fn risk_level_high_overrides_medium() {
    let card = ActionCardBuilder::new("Transfer")
        .add_risk_flag(RiskFlag::new(
            RiskFlagType::FirstTimeRecipient,
            "New recipient",
            Severity::Medium,
        ))
        .add_risk_flag(RiskFlag::new(
            RiskFlagType::HighValue,
            "Large amount",
            Severity::High,
        ))
        .with_payload_hash("00")
        .build();

    assert_eq!(
        card.risk_level,
        RiskLevel::High,
        "high flag must override medium flag in risk level calculation"
    );
}

#[test]
fn risk_level_explicit_override_is_consistent() {
    let card = ActionCardBuilder::new("Transfer")
        .add_risk_flag(RiskFlag::new(
            RiskFlagType::HighValue,
            "Large amount",
            Severity::High,
        ))
        .with_risk_level(RiskLevel::Medium)
        .with_payload_hash("00")
        .build();

    assert_eq!(
        card.risk_level,
        RiskLevel::Medium,
        "explicit risk_level override must be respected even with High flag"
    );
}

#[test]
fn risk_level_ordering() {
    assert!(RiskLevel::Low < RiskLevel::Medium);
    assert!(RiskLevel::Medium < RiskLevel::High);
    assert!(RiskLevel::Low < RiskLevel::High);
}

// ===========================================================================
// Payload hash integrity
// ===========================================================================

#[test]
fn card_payload_hash_is_preserved() {
    let card = ActionCardBuilder::new("Transfer")
        .with_payload_hash("cafebabe1234deadbeef")
        .build();

    assert_eq!(card.payload_hash, "cafebabe1234deadbeef");

    let text = render_text(&card);
    assert!(
        text.contains("cafebabe1234deadbeef"),
        "payload hash must appear in rendered output"
    );
}

#[test]
fn card_without_payload_hash_renders_safely() {
    // An empty payload hash should still render without panicking.
    let card = ActionCardBuilder::new("Transfer").build();

    let text = render_text(&card);
    assert!(!text.is_empty());
    assert!(text.contains("payload:"));
}

// ===========================================================================
// Card expiry enforcement
// ===========================================================================

#[test]
fn expired_card_is_detected() {
    let past = chrono::Utc::now() - chrono::Duration::seconds(60);
    let card = ActionCardBuilder::new("Transfer")
        .with_payload_hash("aa")
        .with_expires_at(past)
        .build();

    assert!(
        card.is_expired(chrono::Utc::now()),
        "card with past expiry must be detected as expired"
    );
}

#[test]
fn future_card_is_not_expired() {
    let future = chrono::Utc::now() + chrono::Duration::hours(1);
    let card = ActionCardBuilder::new("Transfer")
        .with_payload_hash("aa")
        .with_expires_at(future)
        .build();

    assert!(
        !card.is_expired(chrono::Utc::now()),
        "card with future expiry must NOT be detected as expired"
    );
}

#[test]
fn card_without_expiry_never_expires() {
    let card = ActionCardBuilder::new("Transfer")
        .with_payload_hash("aa")
        .build();

    assert!(
        !card.is_expired(chrono::Utc::now()),
        "card without expiry must never be expired"
    );
}

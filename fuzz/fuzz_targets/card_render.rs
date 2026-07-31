#![no_main]

use libfuzzer_sys::fuzz_target;
use polkagent_card::builder::ActionCardBuilder;
use polkagent_card::render::{render_text, render_tui};
use polkagent_card::sections::{RiskFlag, RiskFlagType, SectionSource, Severity};
use polkagent_card::card::RiskLevel;

fuzz_target!(|data: &[u8]| {
    // We need at least a few bytes to drive the builder choices.
    if data.len() < 4 {
        return;
    }

    // Use bytes to derive field values and builder decisions.
    let title = lossy_string(&data[..core::cmp::min(data.len(), 64)]);
    let mut offset = core::cmp::min(64, data.len());

    let mut builder = ActionCardBuilder::new(&title);

    // Add canonical sections based on remaining bytes.
    let num_canonical = data.get(0).copied().unwrap_or(0) % 8;
    for i in 0..num_canonical {
        let label = format!("field_{i}");
        let value = if offset < data.len() {
            let end = core::cmp::min(offset + 32, data.len());
            let v = lossy_string(&data[offset..end]);
            offset = end;
            v
        } else {
            format!("val_{i}")
        };

        let source = match i % 3 {
            0 => SectionSource::Metadata,
            1 => SectionSource::Chain,
            _ => SectionSource::Simulation,
        };

        builder = builder.add_canonical(&label, &value, source);
    }

    // Add narrative sections.
    let num_narrative = data.get(1).copied().unwrap_or(0) % 6;
    for i in 0..num_narrative {
        let label = format!("narrative_{i}");
        let content = if offset < data.len() {
            let end = core::cmp::min(offset + 128, data.len());
            let c = lossy_string(&data[offset..end]);
            offset = end;
            c
        } else {
            format!("content_{i}")
        };

        builder = builder.add_narrative(&label, &content);
    }

    // Add risk flags.
    let num_flags = data.get(2).copied().unwrap_or(0) % 5;
    for i in 0..num_flags {
        let flag_type = match i % 5 {
            0 => RiskFlagType::HighValue,
            1 => RiskFlagType::FirstTimeRecipient,
            2 => RiskFlagType::UnverifiedAddress,
            3 => RiskFlagType::StaleMetadata,
            _ => RiskFlagType::LargeApproval,
        };
        let severity = if i % 2 == 0 {
            Severity::Medium
        } else {
            Severity::High
        };
        let message = if offset < data.len() {
            let end = core::cmp::min(offset + 48, data.len());
            let m = lossy_string(&data[offset..end]);
            offset = end;
            m
        } else {
            format!("risk_{i}")
        };

        builder = builder.add_risk_flag(RiskFlag::new(flag_type, &message, severity));
    }

    // Optionally set risk level override.
    if data.get(3).copied().unwrap_or(0) % 4 == 0 {
        let level = match data.get(3).copied().unwrap_or(0) % 3 {
            0 => RiskLevel::Low,
            1 => RiskLevel::Medium,
            _ => RiskLevel::High,
        };
        builder = builder.with_risk_level(level);
    }

    // Payload hash from remaining data.
    let hash = if offset < data.len() {
        lossy_string(&data[offset..])
    } else {
        "fuzz_hash".to_string()
    };
    builder = builder.with_payload_hash(&hash);

    let card = builder.build();

    // render_text must never panic.
    let _text = render_text(&card);

    // render_tui must never panic.
    let _tui_lines = render_tui(&card);

    // ActionCard methods must never panic.
    let _ = card.is_expired(chrono::Utc::now());
    let _ = card.all_canonical_verified();
    let _ = card.max_severity();
});

/// Convert bytes to a string, replacing invalid UTF-8 with the replacement
/// character. This produces interesting inputs including multi-byte sequences
/// and control characters.
fn lossy_string(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

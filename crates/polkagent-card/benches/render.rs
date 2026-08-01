//! Criterion benchmarks for ActionCard construction and rendering.
//!
//! Covers:
//! - ActionCard creation via builder
//! - Terminal (TUI ratatui) rendering via `render_tui`
//! - JSON serialization via serde_json
//!
//! PRD-15 performance benchmarks.

use criterion::{criterion_group, criterion_main, Criterion};

use polkagent_card::{
    builder::ActionCardBuilder,
    card::RiskLevel,
    render::{render_text, render_tui},
    sections::{RiskFlag, RiskFlagType, SectionSource, Severity},
};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn make_card() -> polkagent_card::card::ActionCard {
    ActionCardBuilder::new("Transfer 10 DOT")
        .add_canonical("Amount", "10 DOT", SectionSource::Metadata)
        .add_canonical("Recipient", "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY", SectionSource::Chain)
        .add_canonical("Nonce", "42", SectionSource::Chain)
        .add_canonical("Fee", "0.001 DOT (estimated)", SectionSource::Metadata)
        .add_narrative(
            "Why?",
            "The agent is rebalancing the staking position to reach the target \
             allocation of 70% staked. This transfer moves funds to the staking \
             account for bonding.",
        )
        .add_narrative("Context", "Triggered by the weekly rebalance policy.")
        .add_risk_flag(RiskFlag::new(
            RiskFlagType::FirstTimeRecipient,
            "Recipient has not been seen in previous transactions",
            Severity::Medium,
        ))
        .add_risk_flag(RiskFlag::new(
            RiskFlagType::HighValue,
            "Transfer amount exceeds 5 DOT daily threshold",
            Severity::High,
        ))
        .with_payload_hash("0xdeadbeef1234abcd5678ef901234abcdef567890abcdef1234567890abcdef12")
        .with_risk_level(RiskLevel::High)
        .build()
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_card_creation(c: &mut Criterion) {
    c.bench_function("action_card_create_via_builder", |b| {
        b.iter(|| {
            let card = make_card();
            criterion::black_box(card);
        });
    });

    // Minimal card (no risk flags, one canonical, no narrative)
    c.bench_function("action_card_create_minimal", |b| {
        b.iter(|| {
            let card = ActionCardBuilder::new("Query balance")
                .add_canonical("Account", "5GrwvaEF…", SectionSource::Chain)
                .with_payload_hash("aabbcc")
                .build();
            criterion::black_box(card);
        });
    });
}

fn bench_card_text_render(c: &mut Criterion) {
    let card = make_card();

    c.bench_function("action_card_render_text", |b| {
        b.iter(|| {
            let text = render_text(&card);
            criterion::black_box(text);
        });
    });
}

fn bench_card_tui_render(c: &mut Criterion) {
    let card = make_card();

    c.bench_function("action_card_render_tui", |b| {
        b.iter(|| {
            let lines = render_tui(&card);
            criterion::black_box(lines);
        });
    });
}

fn bench_card_json_serialize(c: &mut Criterion) {
    let card = make_card();

    c.bench_function("action_card_serde_json_serialize", |b| {
        b.iter(|| {
            let json = serde_json::to_string(&card).expect("serialize");
            criterion::black_box(json);
        });
    });

    let json = serde_json::to_string(&card).expect("serialize");

    c.bench_function("action_card_serde_json_deserialize", |b| {
        b.iter(|| {
            let card: polkagent_card::card::ActionCard =
                serde_json::from_str(&json).expect("deserialize");
            criterion::black_box(card);
        });
    });
}

criterion_group!(
    benches,
    bench_card_creation,
    bench_card_text_render,
    bench_card_tui_render,
    bench_card_json_serialize,
);
criterion_main!(benches);

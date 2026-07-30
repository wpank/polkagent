//! Text and TUI rendering for [`ActionCard`].
//!
//! Two rendering targets are provided:
//!
//! - [`render_text`] — plain-text output suitable for CLI (`--plain-text`)
//!   and logging. Uses ASCII box-drawing and prefix characters to distinguish
//!   canonical from narrative content without relying on ANSI color.
//!
//! - [`render_tui`] — ratatui [`Line`] output with [`Span`] styling for
//!   terminal UIs. Color conventions:
//!   - **Canonical sections** → bold white ("frost / bright") — the most
//!     legible, high-contrast foreground.
//!   - **Narrative sections** → dim white ("mist") — subordinate, clearly
//!     secondary.
//!   - **Risk flags** → color-coded by severity: amber (`Yellow`) for
//!     `Medium`, red (`Red`) for `High`.
//!   - **Section headers and metadata** → cyan for canonical headers, dark
//!     gray for narrative headers and labels.
//!
//! Both renderers respect the canonical-before-narrative ordering invariant
//! established by [`ActionCardBuilder`](crate::builder::ActionCardBuilder).
//!
//! See PRD-13-UX-SURFACES §7, requirement UX-A11Y-05 (plain-text mode), and
//! requirement UX-SEP-01 (visual distinguishability).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::card::{ActionCard, RiskLevel};
use crate::sections::Severity;

// ---------------------------------------------------------------------------
// Plain-text renderer
// ---------------------------------------------------------------------------

/// Render an [`ActionCard`] as a plain-text string for CLI output.
///
/// The output uses ASCII borders and prefix characters (`[C]` / `[N]` /
/// `[!]`) to distinguish section types without relying on ANSI color codes,
/// satisfying requirement UX-A11Y-05 (plain-text / `--no-color` mode).
///
/// Layout:
/// ```text
/// ╔══════════════════════════════╗
/// ║  Transfer 10 DOT             ║
/// ╠══════════════════════════════╣
/// ║ [CANONICAL]                  ║
/// ║   Amount           10 DOT   [metadata] ✓
/// ║   Recipient        5Grw…    [chain]    ✓
/// ╠══════════════════════════════╣
/// ║ [RISK] MEDIUM                ║
/// ║   [!M] New recipient         ║
/// ╠══════════════════════════════╣
/// ║ [AI-GENERATED EXPLANATION]   ║
/// ║   Why?                       ║
/// ║     Rebalancing staking…     ║
/// ╠══════════════════════════════╣
/// ║ payload: deadbeef1234        ║
/// ╚══════════════════════════════╝
/// ```
#[must_use]
pub fn render_text(card: &ActionCard) -> String {
    let mut out = String::new();
    let width = 60usize;
    let bar = "═".repeat(width);

    // Title
    out.push_str(&format!("╔{}╗\n", bar));
    out.push_str(&format!("║  {:<width$}║\n", card.title, width = width - 2));
    out.push_str(&format!("╠{}╣\n", bar));

    // Canonical sections
    if !card.canonical_sections.is_empty() {
        out.push_str(&format!("║ {:<width$}║\n", "[CANONICAL DATA]", width = width - 1));
        for section in &card.canonical_sections {
            let verified_mark = if section.verified { "✓" } else { "?" };
            let source_tag = format!("[{}]", section.source);
            let row = format!(
                "  {label:<24} {value:<18} {source} {verified}",
                label = section.label,
                value = section.value,
                source = source_tag,
                verified = verified_mark,
            );
            // Truncate to fit the box width.
            let truncated = truncate_to(&row, width - 1);
            out.push_str(&format!("║{:<width$}║\n", truncated, width = width));
        }
        out.push_str(&format!("╠{}╣\n", bar));
    }

    // Simulation result
    if let Some(ref sim) = card.simulation_result {
        let status = if sim.success { "PASS" } else { "FAIL" };
        out.push_str(&format!(
            "║ {:<width$}║\n",
            format!("[SIMULATION: {}] {}", status, sim.outcome),
            width = width - 1
        ));
        if let Some(ref fee) = sim.estimated_fee {
            out.push_str(&format!(
                "║   {:<width$}║\n",
                format!("Estimated fee: {}", fee),
                width = width - 3
            ));
        }
        if let Some(block) = sim.simulated_at_block {
            out.push_str(&format!(
                "║   {:<width$}║\n",
                format!("At block: #{}", block),
                width = width - 3
            ));
        }
        out.push_str(&format!("╠{}╣\n", bar));
    } else {
        out.push_str(&format!(
            "║ {:<width$}║\n",
            "[SIMULATION: NOT AVAILABLE]",
            width = width - 1
        ));
        out.push_str(&format!("╠{}╣\n", bar));
    }

    // Risk flags
    if !card.risk_flags.is_empty() {
        out.push_str(&format!(
            "║ {:<width$}║\n",
            format!("[RISK: {}]", card.risk_level),
            width = width - 1
        ));
        for flag in &card.risk_flags {
            let prefix = match flag.severity {
                Severity::High => "[!H]",
                Severity::Medium => "[!M]",
            };
            let row = format!("  {} {}", prefix, flag.message);
            let truncated = truncate_to(&row, width - 1);
            out.push_str(&format!("║{:<width$}║\n", truncated, width = width));
        }
        out.push_str(&format!("╠{}╣\n", bar));
    } else {
        out.push_str(&format!(
            "║ {:<width$}║\n",
            format!("[RISK: {}] No flags", card.risk_level),
            width = width - 1
        ));
        out.push_str(&format!("╠{}╣\n", bar));
    }

    // Narrative sections
    if !card.narrative_sections.is_empty() {
        out.push_str(&format!(
            "║ {:<width$}║\n",
            "[AI-GENERATED EXPLANATION]",
            width = width - 1
        ));
        for section in &card.narrative_sections {
            // Label
            let label_row = format!("  {}:", section.label);
            let truncated = truncate_to(&label_row, width - 1);
            out.push_str(&format!("║{:<width$}║\n", truncated, width = width));
            // Content — wrap at width-4 characters
            for line in word_wrap(&section.content, width - 4) {
                let content_row = format!("    {}", line);
                let truncated = truncate_to(&content_row, width - 1);
                out.push_str(&format!("║{:<width$}║\n", truncated, width = width));
            }
        }
        out.push_str(&format!("╠{}╣\n", bar));
    }

    // Payload hash
    out.push_str(&format!(
        "║ {:<width$}║\n",
        format!("payload: {}", card.payload_hash),
        width = width - 1
    ));

    // Expiry
    if let Some(exp) = card.expires_at {
        out.push_str(&format!(
            "║ {:<width$}║\n",
            format!("expires: {}", exp.format("%Y-%m-%dT%H:%M:%SZ")),
            width = width - 1
        ));
    }

    // Created at
    out.push_str(&format!(
        "║ {:<width$}║\n",
        format!("created: {}", card.created_at.format("%Y-%m-%dT%H:%M:%SZ")),
        width = width - 1
    ));

    out.push_str(&format!("╚{}╝\n", bar));
    out
}

// ---------------------------------------------------------------------------
// TUI renderer
// ---------------------------------------------------------------------------

/// Render an [`ActionCard`] as a `Vec<Line>` of ratatui styled spans.
///
/// Callers can render the returned lines directly into a ratatui paragraph or
/// list widget.
///
/// # Color conventions
///
/// | Element | Style |
/// |---|---|
/// | Card title | Bold white |
/// | Section header (canonical) | Bold cyan |
/// | Canonical label | Bold white ("frost / bright") |
/// | Canonical value | White |
/// | Canonical source | Dark gray |
/// | Verified mark (✓) | Green |
/// | Unverified mark (?) | Yellow |
/// | Section header (narrative) | Dark gray |
/// | Narrative content | Dim white ("mist") |
/// | Risk header | Color by level |
/// | Risk flag (Medium) | Yellow |
/// | Risk flag (High) | Red |
/// | Payload hash | Cyan |
#[must_use]
pub fn render_tui(card: &ActionCard) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

    let divider = Line::from(Span::styled(
        "─".repeat(62),
        Style::default().fg(Color::DarkGray),
    ));

    // ── Title ─────────────────────────────────────────────────────────────
    lines.push(Line::from(Span::styled(
        format!("  {}", card.title),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(divider.clone());

    // ── Canonical sections ─────────────────────────────────────────────────
    if !card.canonical_sections.is_empty() {
        lines.push(Line::from(Span::styled(
            "  CANONICAL DATA",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));

        for section in &card.canonical_sections {
            let verified_span = if section.verified {
                Span::styled(" ✓", Style::default().fg(Color::Green))
            } else {
                Span::styled(" ?", Style::default().fg(Color::Yellow))
            };

            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    format!("{:<24}", section.label),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<20}", section.value),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!(" [{}]", section.source),
                    Style::default().fg(Color::DarkGray),
                ),
                verified_span,
            ]));
        }
        lines.push(divider.clone());
    }

    // ── Simulation result ─────────────────────────────────────────────────
    match &card.simulation_result {
        Some(sim) => {
            let (status_text, status_color) = if sim.success {
                ("SIMULATION: PASS", Color::Green)
            } else {
                ("SIMULATION: FAIL", Color::Red)
            };
            lines.push(Line::from(Span::styled(
                format!("  {}", status_text),
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(sim.outcome.clone(), Style::default().fg(Color::Gray)),
            ]));
            if let Some(ref fee) = sim.estimated_fee {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled("Estimated fee: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(fee.clone(), Style::default().fg(Color::White)),
                ]));
            }
            if let Some(block) = sim.simulated_at_block {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled("At block: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("#{}", block),
                        Style::default().fg(Color::White),
                    ),
                ]));
            }
        }
        None => {
            lines.push(Line::from(Span::styled(
                "  SIMULATION: NOT AVAILABLE",
                Style::default().fg(Color::Yellow),
            )));
        }
    }
    lines.push(divider.clone());

    // ── Risk flags ─────────────────────────────────────────────────────────
    let risk_color = risk_level_color(&card.risk_level);
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("RISK: {}", card.risk_level),
            Style::default()
                .fg(risk_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    for flag in &card.risk_flags {
        let flag_color = severity_color(&flag.severity);
        let prefix = match flag.severity {
            Severity::High => "  [!!] ",
            Severity::Medium => "  [!]  ",
        };
        lines.push(Line::from(vec![
            Span::styled(prefix, Style::default().fg(flag_color)),
            Span::styled(flag.message.clone(), Style::default().fg(flag_color)),
        ]));
    }

    if card.risk_flags.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No risk flags",
            Style::default().fg(Color::Green),
        )));
    }
    lines.push(divider.clone());

    // ── Narrative sections ─────────────────────────────────────────────────
    if !card.narrative_sections.is_empty() {
        lines.push(Line::from(Span::styled(
            "  AI-GENERATED EXPLANATION",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )));

        for section in &card.narrative_sections {
            // Sub-label
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    format!("{}:", section.label),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            // Content (mist: dim white)
            for content_line in word_wrap(&section.content, 54) {
                lines.push(Line::from(vec![
                    Span::raw("      "),
                    Span::styled(
                        content_line,
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::DIM),
                    ),
                ]));
            }
        }
        lines.push(divider.clone());
    }

    // ── Payload hash ───────────────────────────────────────────────────────
    lines.push(Line::from(vec![
        Span::styled("  payload: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            card.payload_hash.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // ── Expiry ─────────────────────────────────────────────────────────────
    if let Some(exp) = card.expires_at {
        lines.push(Line::from(vec![
            Span::styled("  expires: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                exp.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                Style::default().fg(Color::Yellow),
            ),
        ]));
    }

    // ── Created at ─────────────────────────────────────────────────────────
    lines.push(Line::from(vec![
        Span::styled("  created: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            card.created_at.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    lines
}

// ---------------------------------------------------------------------------
// Color helpers
// ---------------------------------------------------------------------------

fn risk_level_color(level: &RiskLevel) -> Color {
    match level {
        RiskLevel::Low => Color::Green,
        RiskLevel::Medium => Color::Yellow,
        RiskLevel::High => Color::Red,
    }
}

fn severity_color(severity: &Severity) -> Color {
    match severity {
        Severity::Medium => Color::Yellow,
        Severity::High => Color::Red,
    }
}

// ---------------------------------------------------------------------------
// Text utilities
// ---------------------------------------------------------------------------

/// Truncate `s` to at most `max_chars` characters (by char count, not bytes).
fn truncate_to(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "…"
    }
}

/// Naive word-wrap: split `text` into lines of at most `max_width` characters.
fn word_wrap(text: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.len() + 1 + word.len() <= max_width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current.clone());
            current = word.to_string();
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::ActionCardBuilder;
    use crate::sections::{RiskFlag, RiskFlagType, SectionSource, Severity};

    fn sample_card() -> ActionCard {
        ActionCardBuilder::new("Transfer 10 DOT")
            .add_canonical("Amount", "10 DOT", SectionSource::Metadata)
            .add_canonical("Recipient", "5GrwvaEF…", SectionSource::Chain)
            .add_narrative("Why?", "The agent is rebalancing the staking position.")
            .add_risk_flag(RiskFlag::new(
                RiskFlagType::FirstTimeRecipient,
                "Recipient not seen before",
                Severity::Medium,
            ))
            .with_payload_hash("deadbeef1234")
            .build()
    }

    #[test]
    fn render_text_contains_title() {
        let card = sample_card();
        let text = render_text(&card);
        assert!(text.contains("Transfer 10 DOT"), "title must appear in output");
    }

    #[test]
    fn render_text_contains_canonical_marker() {
        let card = sample_card();
        let text = render_text(&card);
        assert!(
            text.contains("[CANONICAL DATA]"),
            "canonical section header must be present"
        );
    }

    #[test]
    fn render_text_canonical_before_narrative() {
        let card = sample_card();
        let text = render_text(&card);
        let canonical_pos = text.find("[CANONICAL DATA]").expect("canonical header");
        let narrative_pos = text
            .find("[AI-GENERATED EXPLANATION]")
            .expect("narrative header");
        assert!(
            canonical_pos < narrative_pos,
            "canonical section must appear before narrative in text output"
        );
    }

    #[test]
    fn render_text_contains_payload_hash() {
        let card = sample_card();
        let text = render_text(&card);
        assert!(
            text.contains("deadbeef1234"),
            "payload hash must appear in text output"
        );
    }

    #[test]
    fn render_text_contains_risk_flag() {
        let card = sample_card();
        let text = render_text(&card);
        assert!(
            text.contains("Recipient not seen before"),
            "risk flag message must appear in output"
        );
    }

    #[test]
    fn render_text_simulation_not_available_when_absent() {
        let card = sample_card();
        let text = render_text(&card);
        assert!(
            text.contains("[SIMULATION: NOT AVAILABLE]"),
            "absent simulation must be labeled explicitly (UX-CARD-10)"
        );
    }

    #[test]
    fn render_text_simulation_shown_when_present() {
        use crate::card::SimulationSummary;

        let card = ActionCardBuilder::new("Transfer 1 DOT")
            .add_canonical("Amount", "1 DOT", SectionSource::Metadata)
            .with_simulation(
                SimulationSummary::new(true, "extrinsic dispatched").with_fee("0.001 DOT"),
            )
            .with_payload_hash("ff")
            .build();

        let text = render_text(&card);
        assert!(text.contains("[SIMULATION: PASS]"));
        assert!(text.contains("0.001 DOT"));
    }

    #[test]
    fn render_tui_returns_non_empty_lines() {
        let card = sample_card();
        let lines = render_tui(&card);
        assert!(!lines.is_empty(), "TUI renderer must return at least one line");
    }

    #[test]
    fn render_tui_canonical_line_before_narrative_line() {
        let card = sample_card();
        let lines = render_tui(&card);

        // Convert lines to plain text for position comparison.
        let plain: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .collect();

        let canonical_idx = plain
            .iter()
            .position(|l| l.contains("CANONICAL DATA"))
            .expect("canonical header line must be present");

        let narrative_idx = plain
            .iter()
            .position(|l| l.contains("AI-GENERATED"))
            .expect("narrative header line must be present");

        assert!(
            canonical_idx < narrative_idx,
            "CANONICAL DATA line must precede AI-GENERATED EXPLANATION line in TUI output"
        );
    }

    #[test]
    fn render_tui_risk_high_uses_red_style() {
        use crate::sections::RiskFlagType;

        let card = ActionCardBuilder::new("Proxy add")
            .add_risk_flag(RiskFlag::new(
                RiskFlagType::HighValue,
                "Very large transfer",
                Severity::High,
            ))
            .with_payload_hash("aa")
            .build();

        let lines = render_tui(&card);

        // Find any span with Red foreground.
        let has_red = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.style.fg == Some(Color::Red))
        });

        assert!(has_red, "High severity risk must produce a Red-colored span");
    }

    #[test]
    fn word_wrap_short_text() {
        let result = word_wrap("hello world", 20);
        assert_eq!(result, vec!["hello world"]);
    }

    #[test]
    fn word_wrap_splits_long_text() {
        let result = word_wrap("one two three four five", 10);
        // Each line must be at most 10 chars.
        for line in &result {
            assert!(
                line.len() <= 10,
                "line '{line}' exceeds max_width=10"
            );
        }
    }

    #[test]
    fn truncate_to_short_string_unchanged() {
        assert_eq!(truncate_to("hello", 20), "hello");
    }

    #[test]
    fn truncate_to_long_string_shortened() {
        let result = truncate_to("hello world this is long", 10);
        assert!(result.chars().count() <= 10);
    }
}

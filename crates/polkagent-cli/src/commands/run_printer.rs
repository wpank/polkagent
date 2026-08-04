//! Styled inline output for `polkagent run`.
//!
//! [`RunPrinter`] renders coloured, box-drawn output to stdout using crossterm
//! ANSI sequences. When stdout is not a terminal (piped / redirected) or
//! `NO_COLOR` is set, all styling is suppressed while keeping the same textual
//! content.

use std::io::{IsTerminal, Write};
use std::ops::ControlFlow;
use std::time::Instant;

use crossterm::style::{
    Attribute, Color as CtColor, ResetColor, SetAttribute, SetForegroundColor,
};
use ratatui::style::Color as RatColor;

use polkagent_core::event::{EventKind, LogLevel, RunEvent};

use crate::error_explainer;
use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Unicode block characters (reused from tui/widgets/progress_bar.rs)
// ---------------------------------------------------------------------------

const BLOCKS: [char; 8] = [
    '\u{258F}', // ▏  1/8
    '\u{258E}', // ▎  2/8
    '\u{258D}', // ▍  3/8
    '\u{258C}', // ▌  4/8
    '\u{258B}', // ▋  5/8
    '\u{258A}', // ▊  6/8
    '\u{2589}', // ▉  7/8
    '\u{2588}', // █  8/8
];

// ---------------------------------------------------------------------------
// RunPrinter
// ---------------------------------------------------------------------------

/// Styled inline output for the `polkagent run` command.
///
/// Converts [`RunEvent`]s into coloured terminal output using crossterm.
/// Falls back to plain text when stdout is not a terminal or `NO_COLOR` is
/// set.
pub struct RunPrinter {
    theme: Theme,
    styled: bool,
    stream_tokens: bool,
    start_time: Instant,
    current_turn: u32,
    final_text: String,
    active_tool: Option<(String, Instant)>,
    tool_count: u32,
    effect_count: u32,
}

impl RunPrinter {
    /// Create a new printer.
    ///
    /// Styling is enabled when stdout is a terminal AND `NO_COLOR` is not set.
    /// `stream_tokens` controls whether streaming tokens are printed
    /// immediately or buffered for display at run completion.
    pub fn new(theme: Theme, stream_tokens: bool) -> Self {
        let is_tty = std::io::stdout().is_terminal();
        let no_color = std::env::var_os("NO_COLOR").is_some();
        Self {
            theme,
            styled: is_tty && !no_color,
            stream_tokens,
            start_time: Instant::now(),
            current_turn: 0,
            final_text: String::new(),
            active_tool: None,
            tool_count: 0,
            effect_count: 0,
        }
    }

    /// Return any buffered final text (non-streaming mode).
    #[allow(dead_code)]
    pub fn final_text(&self) -> &str {
        &self.final_text
    }

    // ── Public API ───────────────────────────────────────────────────────

    /// Print the startup banner.
    pub fn print_header(
        &self,
        w: &mut impl Write,
        run_id: &impl std::fmt::Display,
        agent_name: &str,
        model: &str,
    ) -> std::io::Result<()> {
        let run_short = {
            let s = run_id.to_string();
            s[..8.min(s.len())].to_string()
        };
        let version = env!("CARGO_PKG_VERSION");

        // Compute inner width: widest content line + padding.
        let line_title = format!("  POLKAGENT RUN                    v{version}");
        let line_info = format!("  Agent: {agent_name}  Model: {model}");
        let line_run = format!("  Run:   {run_short}");
        let inner_w = line_title.len().max(line_info.len()).max(line_run.len()) + 2;

        let top = format!(" ┌{}┐", "─".repeat(inner_w));
        let bot = format!(" └{}┘", "─".repeat(inner_w));

        // Box border.
        self.set_fg(w, self.theme.rose_ember)?;
        writeln!(w, "{top}")?;

        // Title line.
        write!(w, " │")?;
        self.set_fg(w, self.theme.rose_bright)?;
        self.set_bold(w)?;
        write!(w, "  POLKAGENT RUN")?;
        self.reset(w)?;
        self.set_fg(w, self.theme.text_dim)?;
        let pad = inner_w - line_title.len();
        write!(w, "{:>width$}v{version}", "", width = pad.saturating_sub(0))?;
        self.reset(w)?;
        self.set_fg(w, self.theme.rose_ember)?;
        writeln!(w, "│")?;

        // Agent/model line.
        write!(w, " │")?;
        self.set_fg(w, self.theme.bone)?;
        write!(w, "  Agent: {agent_name}  Model: {model}")?;
        let pad2 = inner_w - line_info.len();
        write!(w, "{:width$}", "", width = pad2)?;
        self.reset(w)?;
        self.set_fg(w, self.theme.rose_ember)?;
        writeln!(w, "│")?;

        // Run ID line.
        write!(w, " │")?;
        self.set_fg(w, self.theme.rose_dim)?;
        write!(w, "  Run:   {run_short}")?;
        let pad3 = inner_w - line_run.len();
        write!(w, "{:width$}", "", width = pad3)?;
        self.reset(w)?;
        self.set_fg(w, self.theme.rose_ember)?;
        writeln!(w, "│")?;

        // Bottom border.
        writeln!(w, "{bot}")?;
        self.reset(w)?;

        Ok(())
    }

    /// Handle a single event, producing styled output.
    ///
    /// Returns `ControlFlow::Break(())` for terminal events (completed,
    /// failed, cancelled, timed out) so the caller can exit the event loop.
    pub fn handle_event(
        &mut self,
        w: &mut impl Write,
        event: &RunEvent,
    ) -> ControlFlow<()> {
        match &event.kind {
            // -- Suppressed / silent ----------------------------------------
            EventKind::RunCreated
            | EventKind::RunQueued
            | EventKind::TurnCompleted { .. }
            | EventKind::StepStarted { .. }
            | EventKind::StepCompleted { .. } => {}

            // -- Run lifecycle ----------------------------------------------
            EventKind::RunStarted => {
                let _ = self.styled_line(w, "  ▶ Running", self.theme.success, false);
            }

            // -- Turns ------------------------------------------------------
            EventKind::TurnStarted { turn_number, .. } => {
                self.current_turn = *turn_number;
                let _ = self.styled_line(
                    w,
                    &format!("\n ── Turn {} ──", turn_number + 1),
                    self.theme.rose,
                    false,
                );
            }

            // -- Streaming tokens -------------------------------------------
            EventKind::StreamingToken { text } => {
                if self.stream_tokens {
                    let _ = self.set_fg(w, self.theme.text_primary);
                    let _ = write!(w, "{text}");
                    let _ = self.reset(w);
                    let _ = w.flush();
                } else {
                    self.final_text.push_str(text);
                }
            }

            // -- Tool calls -------------------------------------------------
            EventKind::ToolCallStarted { tool_name } => {
                self.active_tool = Some((tool_name.clone(), Instant::now()));
                self.tool_count += 1;
                let _ = self.set_fg(w, self.theme.bone);
                let _ = self.set_bold(w);
                let _ = writeln!(w, "\n  ◉ {tool_name}");
                let _ = self.reset(w);
            }
            EventKind::ToolCallCompleted { tool_name } => {
                let elapsed = self
                    .active_tool
                    .take()
                    .map(|(_, t)| format_elapsed(t.elapsed()))
                    .unwrap_or_default();
                let msg = if elapsed.is_empty() {
                    format!("  ✓ {tool_name}")
                } else {
                    format!("  ✓ {tool_name} ({elapsed})")
                };
                let _ = self.styled_line(w, &msg, self.theme.success, false);
            }

            // -- Effects ----------------------------------------------------
            EventKind::EffectIntentCreated { intent_id } => {
                self.effect_count += 1;
                let id_str = intent_id.to_string();
                let short = short_id(&id_str);
                let _ = self.styled_line(
                    w,
                    &format!("  ▲ Effect pending: {short}"),
                    self.theme.warning,
                    false,
                );
            }
            EventKind::EffectAttemptStarted { attempt_id } => {
                let id_str = attempt_id.to_string();
                let short = short_id(&id_str);
                let _ = self.styled_line(
                    w,
                    &format!("    ▶ Attempt: {short}"),
                    self.theme.rose_dim,
                    false,
                );
            }
            EventKind::EffectOutcomeRecorded { outcome_id } => {
                let id_str = outcome_id.to_string();
                let short = short_id(&id_str);
                let _ = self.styled_line(
                    w,
                    &format!("    ✓ Outcome: {short}"),
                    self.theme.success,
                    false,
                );
            }
            EventKind::EffectsResolved => {
                let _ = self.styled_line(w, "  ✓ Effects resolved", self.theme.success, false);
            }

            // -- Artifacts --------------------------------------------------
            EventKind::ArtifactCreated { artifact_id } => {
                let id_str = artifact_id.to_string();
                let short = short_id(&id_str);
                let _ = self.styled_line(
                    w,
                    &format!("  ◆ Artifact: {short}"),
                    self.theme.dream,
                    false,
                );
            }

            // -- Approvals --------------------------------------------------
            EventKind::ApprovalRequested { request_id } => {
                let short = short_id(request_id);
                let _ = self.styled_line(
                    w,
                    &format!("  ⏸ Awaiting approval: {short}"),
                    self.theme.warning,
                    true,
                );
            }
            EventKind::ApprovalGranted { .. } => {
                let _ = self.styled_line(w, "  ✓ Approved", self.theme.success, false);
            }
            EventKind::ApprovalDenied { reason } => {
                let _ = self.styled_line(
                    w,
                    &format!("  ✗ Denied: {reason}"),
                    self.theme.danger,
                    false,
                );
            }

            // -- Progress ---------------------------------------------------
            EventKind::ProgressUpdate {
                message,
                percentage,
            } => {
                let _ = self.print_progress(w, message, *percentage);
            }

            // -- Delivery ---------------------------------------------------
            EventKind::DeliveryStarted => {
                let _ = self.styled_line(w, "  ▶ Delivering...", self.theme.rose_dim, false);
            }
            EventKind::DeliveryCompleted => {
                let _ = self.styled_line(w, "  ✓ Delivered", self.theme.success, false);
            }

            // -- Diagnostics ------------------------------------------------
            EventKind::DiagnosticLog { level, message } => {
                let color = match level {
                    LogLevel::Error => self.theme.danger,
                    LogLevel::Warn => self.theme.warning,
                    LogLevel::Info => self.theme.text_primary,
                    LogLevel::Debug | LogLevel::Trace => self.theme.text_dim,
                };
                let label = match level {
                    LogLevel::Error => "ERROR",
                    LogLevel::Warn => "WARN",
                    LogLevel::Info => "INFO",
                    LogLevel::Debug => "DEBUG",
                    LogLevel::Trace => "TRACE",
                };
                let _ = self.styled_line(
                    w,
                    &format!("  [{label}] {message}"),
                    color,
                    false,
                );
            }

            // -- Budget -----------------------------------------------------
            EventKind::BudgetConsumed {
                resource,
                amount_str,
            } => {
                let _ = self.styled_line(
                    w,
                    &format!("  $ {resource}: {amount_str}"),
                    self.theme.text_dim,
                    false,
                );
            }
            EventKind::BudgetWarning {
                resource,
                remaining_str,
            } => {
                let _ = self.styled_line(
                    w,
                    &format!("  ⚠ Budget low: {resource} ({remaining_str} remaining)"),
                    self.theme.warning,
                    true,
                );
            }

            // -- Terminal events --------------------------------------------
            EventKind::RunCompleting => {
                let _ = self.styled_line(w, "  Completing...", self.theme.text_dim, false);
            }
            EventKind::RunCompleted { .. } => {
                if !self.stream_tokens && !self.final_text.is_empty() {
                    let _ = self.set_fg(w, self.theme.text_primary);
                    let _ = writeln!(w, "{}", self.final_text);
                    let _ = self.reset(w);
                }
                let _ = self.print_completion_card(w);
                return ControlFlow::Break(());
            }
            EventKind::RunFailed { reason } => {
                let _ = self.print_failure_card(w, reason);
                return ControlFlow::Break(());
            }
            EventKind::RunCancelled { reason } => {
                let cancel_reason = format!("cancelled: {reason}");
                let _ = self.print_failure_card(w, &cancel_reason);
                return ControlFlow::Break(());
            }
            EventKind::RunTimedOut => {
                let _ = self.print_failure_card(w, "run timed out");
                return ControlFlow::Break(());
            }
            EventKind::RunRetryQueued => {
                let _ = self.styled_line(w, "  ↻ Retry queued", self.theme.warning, false);
            }
            EventKind::MetadataDriftDetected { chain_id, pinned_hash, current_hash } => {
                let msg = format!(
                    "  ⚠ Metadata drift on {chain_id}: pinned={pinned_hash} current={current_hash}"
                );
                let _ = self.styled_line(w, &msg, self.theme.warning, false);
            }
        }

        ControlFlow::Continue(())
    }

    // ── Private helpers ──────────────────────────────────────────────────

    /// Write a single styled line with optional bold.
    fn styled_line(
        &self,
        w: &mut impl Write,
        text: &str,
        color: RatColor,
        bold: bool,
    ) -> std::io::Result<()> {
        self.set_fg(w, color)?;
        if bold {
            self.set_bold(w)?;
        }
        writeln!(w, "{text}")?;
        self.reset(w)
    }

    /// Set foreground colour (no-op when unstyled).
    fn set_fg(&self, w: &mut impl Write, color: RatColor) -> std::io::Result<()> {
        if self.styled {
            crossterm::queue!(w, SetForegroundColor(rat_to_ct(color)))?;
        }
        Ok(())
    }

    /// Set bold attribute (no-op when unstyled).
    fn set_bold(&self, w: &mut impl Write) -> std::io::Result<()> {
        if self.styled {
            crossterm::queue!(w, SetAttribute(Attribute::Bold))?;
        }
        Ok(())
    }

    /// Reset all attributes (no-op when unstyled).
    fn reset(&self, w: &mut impl Write) -> std::io::Result<()> {
        if self.styled {
            crossterm::queue!(w, ResetColor, SetAttribute(Attribute::Reset))?;
        }
        Ok(())
    }

    /// Render the completion summary card.
    fn print_completion_card(&self, w: &mut impl Write) -> std::io::Result<()> {
        let elapsed = format_elapsed(self.start_time.elapsed());
        let turns = self.current_turn + 1;

        let line1 = format!("  ✓ Run completed                    {elapsed}");
        let line2 = format!(
            "  Turns: {}   Tools: {}   Effects: {}",
            turns, self.tool_count, self.effect_count
        );
        let inner_w = line1.len().max(line2.len()) + 2;

        let top = format!(" ┌{}┐", "─".repeat(inner_w));
        let bot = format!(" └{}┘", "─".repeat(inner_w));

        writeln!(w)?;
        self.set_fg(w, self.theme.rose_ember)?;
        writeln!(w, "{top}")?;

        // Line 1: status + elapsed.
        write!(w, " │")?;
        self.set_fg(w, self.theme.success)?;
        self.set_bold(w)?;
        write!(w, "{line1}")?;
        self.reset(w)?;
        let pad = inner_w - line1.len();
        write!(w, "{:width$}", "", width = pad)?;
        self.set_fg(w, self.theme.rose_ember)?;
        writeln!(w, "│")?;

        // Line 2: stats.
        write!(w, " │")?;
        self.set_fg(w, self.theme.text_primary)?;
        write!(w, "{line2}")?;
        self.reset(w)?;
        let pad2 = inner_w - line2.len();
        write!(w, "{:width$}", "", width = pad2)?;
        self.set_fg(w, self.theme.rose_ember)?;
        writeln!(w, "│")?;

        writeln!(w, "{bot}")?;
        self.reset(w)?;

        Ok(())
    }

    /// Render the failure card with structured error explanation.
    fn print_failure_card(&self, w: &mut impl Write, reason: &str) -> std::io::Result<()> {
        let elapsed = format_elapsed(self.start_time.elapsed());
        let explanation = error_explainer::explain(reason);

        let line1 = format!(
            "  ✗ Run failed: {:<26}{elapsed}",
            explanation.category.label()
        );
        let line2 = format!("  {reason}");
        let inner_w = line1.len().max(line2.len()) + 2;

        let top = format!(" ┌{}┐", "─".repeat(inner_w));
        let mid = format!(" ├{}┤", "─".repeat(inner_w));
        let bot = format!(" └{}┘", "─".repeat(inner_w));

        writeln!(w)?;
        self.set_fg(w, self.theme.rose_ember)?;
        writeln!(w, "{top}")?;

        // Line 1: status + category + elapsed.
        write!(w, " │")?;
        self.set_fg(w, self.theme.danger)?;
        self.set_bold(w)?;
        write!(w, "{line1}")?;
        self.reset(w)?;
        let pad = inner_w - line1.len();
        write!(w, "{:width$}", "", width = pad)?;
        self.set_fg(w, self.theme.rose_ember)?;
        writeln!(w, "│")?;

        // Line 2: raw reason.
        write!(w, " │")?;
        self.set_fg(w, self.theme.text_primary)?;
        write!(w, "{line2}")?;
        self.reset(w)?;
        let pad2 = inner_w - line2.len();
        write!(w, "{:width$}", "", width = pad2)?;
        self.set_fg(w, self.theme.rose_ember)?;
        writeln!(w, "│")?;

        // Separator.
        self.set_fg(w, self.theme.rose_ember)?;
        writeln!(w, "{mid}")?;

        // What happened.
        self.print_box_line(w, explanation.what_happened, inner_w, self.theme.text_primary)?;

        // Next step.
        let next = format!("→ {}", explanation.next_step_summary());
        self.print_box_line(w, &next, inner_w, self.theme.bone)?;

        writeln!(w, "{bot}")?;
        self.reset(w)?;

        Ok(())
    }

    /// Write a single line inside a box-drawn card.
    fn print_box_line(
        &self,
        w: &mut impl Write,
        text: &str,
        inner_w: usize,
        color: RatColor,
    ) -> std::io::Result<()> {
        let content = format!("  {text}");
        write!(w, " │")?;
        self.set_fg(w, color)?;
        write!(w, "{content}")?;
        self.reset(w)?;
        let pad = inner_w - content.len().min(inner_w);
        write!(w, "{:width$}", "", width = pad)?;
        self.set_fg(w, self.theme.rose_ember)?;
        writeln!(w, "│")?;
        Ok(())
    }

    /// Render an inline progress bar with message.
    fn print_progress(
        &self,
        w: &mut impl Write,
        message: &str,
        percentage: Option<f32>,
    ) -> std::io::Result<()> {
        let Some(pct) = percentage else {
            // No percentage — just show message.
            return self.styled_line(w, &format!("  ▶ {message}"), self.theme.rose_dim, false);
        };

        let ratio = (pct / 100.0).clamp(0.0, 1.0) as f64;
        let bar_width = 20usize;
        let total_units = bar_width * 8;
        let filled_units = (ratio * total_units as f64).round() as usize;
        let full_cells = filled_units / 8;
        let partial_idx = filled_units % 8;

        let fill_color = self.theme.progress_color(ratio);

        let mut bar = String::with_capacity(bar_width);
        for _ in 0..full_cells {
            bar.push(BLOCKS[7]);
        }
        if partial_idx > 0 && full_cells < bar_width {
            bar.push(BLOCKS[partial_idx - 1]);
        }
        let filled_chars = full_cells + usize::from(partial_idx > 0);
        for _ in 0..bar_width.saturating_sub(filled_chars) {
            bar.push('░');
        }

        write!(w, "  ")?;
        self.set_fg(w, fill_color)?;
        write!(w, "{bar}")?;
        self.reset(w)?;
        self.set_fg(w, self.theme.text_dim)?;
        writeln!(w, " {pct:5.1}%  {message}")?;
        self.reset(w)?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

/// Convert `ratatui::style::Color` → `crossterm::style::Color`.
///
/// The ROSEDUST palette exclusively uses `Color::Rgb` and `Color::Reset`, so
/// this mapping is safe and complete for our use case.
fn rat_to_ct(color: RatColor) -> CtColor {
    match color {
        RatColor::Rgb(r, g, b) => CtColor::Rgb { r, g, b },
        RatColor::Reset => CtColor::Reset,
        // Fallback for any other variant (shouldn't happen with our theme).
        _ => CtColor::Reset,
    }
}

/// Truncate a UUID string to its first 8 characters.
fn short_id(id: &str) -> &str {
    &id[..8.min(id.len())]
}

/// Format a `Duration` as a compact human-readable string.
fn format_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let mins = secs as u64 / 60;
        let rem = secs as u64 % 60;
        format!("{mins}m{rem}s")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rat_to_ct_rgb() {
        let ct = rat_to_ct(RatColor::Rgb(170, 112, 136));
        assert_eq!(ct, CtColor::Rgb { r: 170, g: 112, b: 136 });
    }

    #[test]
    fn rat_to_ct_reset() {
        assert_eq!(rat_to_ct(RatColor::Reset), CtColor::Reset);
    }

    #[test]
    fn short_id_truncates() {
        assert_eq!(short_id("abcdef01-2345-6789"), "abcdef01");
    }

    #[test]
    fn short_id_short_input() {
        assert_eq!(short_id("abc"), "abc");
    }

    #[test]
    fn format_elapsed_seconds() {
        let s = format_elapsed(std::time::Duration::from_secs_f64(12.34));
        assert_eq!(s, "12.3s");
    }

    #[test]
    fn format_elapsed_minutes() {
        let s = format_elapsed(std::time::Duration::from_secs(125));
        assert_eq!(s, "2m5s");
    }

    #[test]
    fn printer_unstyled_header_contains_key_info() {
        // Force NO_COLOR so output is plain text.
        let theme = Theme::no_color();
        let printer = RunPrinter {
            theme,
            styled: false,
            stream_tokens: true,
            start_time: Instant::now(),
            current_turn: 0,
            final_text: String::new(),
            active_tool: None,
            tool_count: 0,
            effect_count: 0,
        };

        let mut buf = Vec::new();
        printer
            .print_header(&mut buf, &"abcd1234-5678-9abc-def0-123456789abc", "dev-helper", "claude-opus-4-6")
            .unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("POLKAGENT RUN"), "should contain title");
        assert!(output.contains("dev-helper"), "should contain agent name");
        assert!(output.contains("claude-opus-4-6"), "should contain model");
        assert!(output.contains("abcd1234"), "should contain short run ID");
    }

    #[test]
    fn handle_event_run_completed_breaks() {
        let theme = Theme::no_color();
        let mut printer = RunPrinter {
            theme,
            styled: false,
            stream_tokens: true,
            start_time: Instant::now(),
            current_turn: 0,
            final_text: String::new(),
            active_tool: None,
            tool_count: 0,
            effect_count: 0,
        };

        let event = RunEvent::new_durable(
            polkagent_core::ids::EventId::new(),
            polkagent_core::ids::RunId::new(),
            1,
            EventKind::RunCompleted {
                output_artifact_id: None,
                input_tokens: 0,
                output_tokens: 0,
            },
            polkagent_core::event::EventCorrelation::default(),
        );

        let mut buf = Vec::new();
        let flow = printer.handle_event(&mut buf, &event);
        assert!(flow.is_break());
    }

    #[test]
    fn handle_event_run_failed_breaks() {
        let theme = Theme::no_color();
        let mut printer = RunPrinter {
            theme,
            styled: false,
            stream_tokens: true,
            start_time: Instant::now(),
            current_turn: 0,
            final_text: String::new(),
            active_tool: None,
            tool_count: 0,
            effect_count: 0,
        };

        let event = RunEvent::new_durable(
            polkagent_core::ids::EventId::new(),
            polkagent_core::ids::RunId::new(),
            1,
            EventKind::RunFailed {
                reason: "something broke".into(),
            },
            polkagent_core::event::EventCorrelation::default(),
        );

        let mut buf = Vec::new();
        let flow = printer.handle_event(&mut buf, &event);
        assert!(flow.is_break());

        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("something broke"));
    }

    #[test]
    fn handle_event_tool_call_tracks_count() {
        let theme = Theme::no_color();
        let mut printer = RunPrinter {
            theme,
            styled: false,
            stream_tokens: true,
            start_time: Instant::now(),
            current_turn: 0,
            final_text: String::new(),
            active_tool: None,
            tool_count: 0,
            effect_count: 0,
        };

        let run_id = polkagent_core::ids::RunId::new();
        let start_event = RunEvent::new_ephemeral(
            polkagent_core::ids::EventId::new(),
            run_id.clone(),
            1,
            EventKind::ToolCallStarted {
                tool_name: "read_file".into(),
            },
        );
        let end_event = RunEvent::new_ephemeral(
            polkagent_core::ids::EventId::new(),
            run_id,
            2,
            EventKind::ToolCallCompleted {
                tool_name: "read_file".into(),
            },
        );

        let mut buf = Vec::new();
        let _ = printer.handle_event(&mut buf, &start_event);
        let _ = printer.handle_event(&mut buf, &end_event);

        assert_eq!(printer.tool_count, 1);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("read_file"));
    }

    #[test]
    fn handle_event_streaming_token_buffers_when_not_streaming() {
        let theme = Theme::no_color();
        let mut printer = RunPrinter {
            theme,
            styled: false,
            stream_tokens: false,
            start_time: Instant::now(),
            current_turn: 0,
            final_text: String::new(),
            active_tool: None,
            tool_count: 0,
            effect_count: 0,
        };

        let event = RunEvent::new_ephemeral(
            polkagent_core::ids::EventId::new(),
            polkagent_core::ids::RunId::new(),
            1,
            EventKind::StreamingToken {
                text: "hello world".into(),
            },
        );

        let mut buf = Vec::new();
        let _ = printer.handle_event(&mut buf, &event);

        // Nothing written to output.
        assert!(buf.is_empty());
        // Text buffered internally.
        assert_eq!(printer.final_text(), "hello world");
    }
}

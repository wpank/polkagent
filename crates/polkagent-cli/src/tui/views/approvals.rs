//! Approval Queue view (F6).
//!
//! Lists pending effects awaiting approval. Each entry shows the effect kind,
//! run context, and state. Color indicators:
//!
//! - Amber (warning) — pending
//! - Jade (success) — approved / succeeded
//! - Crimson (danger) — denied / failed
//!
//! Keyboard: Enter to view detail, 'a' to approve, 'd' to deny.
//! First press shows a confirmation dialog; second press executes.
//!
//! ## Widget integration
//!
//! The detail panel on the right side now uses `action_card::render` to show
//! a structured action card for the selected effect intent, with pallet/call
//! information parsed from the effect kind and a generated narrative.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table},
    Frame,
};

use crate::tui::state::{ApprovalItem, ConfirmDialog, ScrollState, TuiState};
use crate::tui::theme::Theme;
use crate::tui::widgets::action_card;
use crate::tui::widgets::action_card::{ActionCardData, RiskLevel};

// ---------------------------------------------------------------------------
// Public render entry point
// ---------------------------------------------------------------------------

/// Render the approval queue into `area`.
pub fn render(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    if area.width >= 100 && state.approvals_scroll.selected.is_some() {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        render_list(
            frame,
            cols[0],
            &state.pending_approvals,
            &state.approvals_scroll,
            theme,
        );
        if let Some(sel) = state.approvals_scroll.selected {
            if let Some(item) = state.pending_approvals.get(sel) {
                render_detail(frame, cols[1], item, theme);
            }
        }
    } else {
        render_list(
            frame,
            area,
            &state.pending_approvals,
            &state.approvals_scroll,
            theme,
        );
    }

    // Render confirmation dialog overlay if active.
    match &state.confirm_dialog {
        ConfirmDialog::None => {}
        ConfirmDialog::ConfirmApprove(effect_id) => {
            render_confirm_dialog(frame, area, effect_id, true, theme);
        }
        ConfirmDialog::ConfirmDeny(effect_id) => {
            render_confirm_dialog(frame, area, effect_id, false, theme);
        }
    }
}

// ---------------------------------------------------------------------------
// Approval list table
// ---------------------------------------------------------------------------

fn render_list(
    frame: &mut Frame,
    area: Rect,
    items: &[ApprovalItem],
    scroll: &ScrollState,
    theme: &Theme,
) {
    let pending_count = items.iter().filter(|i| i.state == "pending").count();
    let title = format!(
        " APPROVAL QUEUE ({pending_count} pending, {} total) ",
        items.len()
    );

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if items.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No pending effects.",
                Style::default().fg(theme.text_dim),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Effects awaiting approval will appear here.",
                Style::default().fg(theme.text_dim),
            )),
        ]);
        frame.render_widget(msg, inner);
        return;
    }

    // Header row.
    let header = Row::new(vec![
        Cell::from(Span::styled(" ST", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("Kind", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("Agent", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("State", Style::default().fg(theme.text_dim))),
        Cell::from(Span::styled("Created", Style::default().fg(theme.text_dim))),
    ])
    .height(1)
    .style(Style::default().add_modifier(Modifier::UNDERLINED));

    let selected_idx = scroll.selected.unwrap_or(usize::MAX);

    let rows: Vec<Row> = items
        .iter()
        .skip(scroll.offset)
        .take(inner.height.saturating_sub(1) as usize)
        .enumerate()
        .map(|(vis_idx, item)| {
            let abs_idx = vis_idx + scroll.offset;
            let is_selected = abs_idx == selected_idx;

            let row_style = if is_selected {
                Style::default()
                    .bg(theme.bg_highlight)
                    .fg(theme.rose_bright)
            } else {
                Style::default().fg(theme.text_primary)
            };

            let state_color = approval_state_color(&item.state, theme);
            let glyph = approval_glyph(&item.state);
            let created = item.created_at.format("%m-%d %H:%M").to_string();

            // Truncate agent name.
            let agent = if item.agent_name.len() > 16 {
                format!("{}…", &item.agent_name[..15])
            } else {
                item.agent_name.clone()
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    format!(" {glyph}"),
                    Style::default().fg(state_color),
                )),
                Cell::from(Span::styled(
                    item.kind.clone(),
                    Style::default().fg(kind_color(&item.kind, theme)),
                )),
                Cell::from(agent),
                Cell::from(Span::styled(
                    item.state.clone(),
                    Style::default().fg(state_color),
                )),
                Cell::from(Span::styled(created, Style::default().fg(theme.text_dim))),
            ])
            .height(1)
            .style(row_style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Min(12),
            Constraint::Min(12),
            Constraint::Length(10),
            Constraint::Length(12),
        ],
    )
    .header(header);

    frame.render_widget(table, inner);

    // Footer with key hints.
    let footer_y = inner.y + inner.height.saturating_sub(1);
    if footer_y < inner.y + inner.height {
        let footer_area = Rect {
            y: footer_y,
            height: 1,
            x: inner.x,
            width: inner.width,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                " Enter: detail  a: approve  d: deny  j/k: navigate  Esc: back",
                Style::default().fg(theme.text_dim),
            )),
            footer_area,
        );
    }
}

// ---------------------------------------------------------------------------
// Approval detail panel — uses action_card widget
// ---------------------------------------------------------------------------

fn render_detail(frame: &mut Frame, area: Rect, item: &ApprovalItem, theme: &Theme) {
    // Parse pallet and call from the effect kind (e.g. "sign", "broadcast",
    // "tool:balance_transfer").  We produce a best-effort breakdown.
    let (pallet, call) = parse_kind_to_pallet_call(&item.kind);

    // Build a minimal params list from the effect metadata we have.
    let short_effect = &item.effect_id[..8.min(item.effect_id.len())];
    let short_run = &item.run_id[..8.min(item.run_id.len())];
    let params: Vec<(&str, &str)> = vec![
        ("effect_id", short_effect),
        ("run_id", short_run),
        ("agent", &item.agent_name),
        ("state", &item.state),
    ];

    // Determine risk level from the effect kind.
    let risk = effect_risk_level(&item.kind);

    // Generate a simple narrative summary.
    let narrative = format!(
        "Effect '{}' from agent '{}' is in state '{}'. \
         This action was submitted by run {} and requires your review before it is executed on-chain.",
        item.kind, item.agent_name, item.state, short_run,
    );

    let card_data = ActionCardData {
        pallet: &pallet,
        call: &call,
        params: &params,
        narrative: &narrative,
        risk,
        hash: &item.effect_id,
    };

    action_card::render(frame, area, &card_data, theme);
}

/// Split an effect kind string into (pallet, call) for the action card.
fn parse_kind_to_pallet_call(kind: &str) -> (String, String) {
    // Kinds: "sign", "broadcast", "tool", "tool:balance_transfer", "model", etc.
    if let Some(sep) = kind.find(':') {
        let outer = &kind[..sep];
        let inner = &kind[sep + 1..];
        // Try to derive pallet/call from inner if it contains a dot or underscore.
        if let Some(dot) = inner.find('.') {
            return (inner[..dot].to_owned(), inner[dot + 1..].to_owned());
        }
        (outer.to_owned(), inner.to_owned())
    } else {
        // Map well-known kinds to pallet::call.
        let (pallet, call) = match kind {
            "sign" => ("Crypto", "sign"),
            "broadcast" => ("Chain", "broadcast"),
            "tool" => ("Tool", "execute"),
            "model" => ("Model", "infer"),
            _ => ("Effect", kind),
        };
        (pallet.to_owned(), call.to_owned())
    }
}

/// Derive a `RiskLevel` from an effect kind string.
fn effect_risk_level(kind: &str) -> RiskLevel {
    match kind {
        "sign" | "broadcast" => RiskLevel::High,
        "tool" => RiskLevel::Medium,
        _ => RiskLevel::Low,
    }
}

// ---------------------------------------------------------------------------
// Confirmation dialog
// ---------------------------------------------------------------------------

/// Render a modal confirmation dialog centered in `area`.
fn render_confirm_dialog(
    frame: &mut Frame,
    area: Rect,
    effect_id: &str,
    is_approve: bool,
    theme: &Theme,
) {
    // Center the dialog: 50 cols x 8 rows.
    let dialog_w: u16 = 52;
    let dialog_h: u16 = 8;
    let x = area.x + area.width.saturating_sub(dialog_w) / 2;
    let y = area.y + area.height.saturating_sub(dialog_h) / 2;
    let dialog_area = Rect {
        x,
        y,
        width: dialog_w,
        height: dialog_h,
    };

    let (action, action_color, key_hint) = if is_approve {
        (
            "APPROVE",
            theme.success,
            "'a' again to confirm  Esc to cancel",
        )
    } else {
        ("DENY", theme.danger, "'d' again to confirm  Esc to cancel")
    };

    let short_eid = &effect_id[..8.min(effect_id.len())];

    let block = Block::default()
        .title(Span::styled(
            format!(" Confirm: {action} "),
            Style::default()
                .fg(action_color)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(action_color))
        .style(Style::default().bg(theme.bg_void));

    let inner = block.inner(dialog_area);

    // Clear the area first so the dialog appears on top.
    frame.render_widget(Clear, dialog_area);
    frame.render_widget(block, dialog_area);

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Effect: ", Style::default().fg(theme.text_dim)),
            Span::styled(short_eid.to_owned(), Style::default().fg(theme.bone)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!("  Press the key again to {action}."),
            Style::default()
                .fg(action_color)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {key_hint}"),
            Style::default().fg(theme.text_dim),
        )),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Color for the approval state indicator.
fn approval_state_color(state: &str, theme: &Theme) -> ratatui::style::Color {
    match state {
        "pending" => theme.warning,
        "claimed" => theme.rose,
        "success" | "approved" => theme.success,
        "failure" | "denied" | "timeout" => theme.danger,
        _ => theme.text_dim,
    }
}

/// Glyph for an approval state.
fn approval_glyph(state: &str) -> &'static str {
    match state {
        "pending" => "◦",
        "claimed" => "▶",
        "success" | "approved" => "✓",
        "failure" | "denied" => "✗",
        "timeout" => "⏱",
        _ => "?",
    }
}

/// Color for the effect kind.
fn kind_color(kind: &str, theme: &Theme) -> ratatui::style::Color {
    match kind {
        "sign" | "broadcast" => theme.warning,
        "tool" => theme.dream,
        "model" => theme.rose,
        _ => theme.text_primary,
    }
}

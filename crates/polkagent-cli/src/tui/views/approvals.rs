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

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table},
    Frame,
};

use crate::tui::state::{ApprovalItem, ConfirmDialog, ScrollState, TuiState};
use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Public render entry point
// ---------------------------------------------------------------------------

/// Render the approval queue into `area`.
pub fn render(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    if area.width >= 100 && state.approvals_scroll.selected.is_some() {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);

        render_list(frame, cols[0], &state.pending_approvals, &state.approvals_scroll, theme);
        if let Some(sel) = state.approvals_scroll.selected {
            if let Some(item) = state.pending_approvals.get(sel) {
                render_detail(frame, cols[1], item, theme);
            }
        }
    } else {
        render_list(frame, area, &state.pending_approvals, &state.approvals_scroll, theme);
    }

    // Render confirmation dialog overlay if active.
    match &state.confirm_dialog {
        ConfirmDialog::None => {}
        ConfirmDialog::ConfirmApprove(effect_id) => {
            render_confirm_dialog(
                frame,
                area,
                effect_id,
                true,
                theme,
            );
        }
        ConfirmDialog::ConfirmDeny(effect_id) => {
            render_confirm_dialog(
                frame,
                area,
                effect_id,
                false,
                theme,
            );
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
    let title = format!(" APPROVAL QUEUE ({pending_count} pending, {} total) ", items.len());

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme.rose)
                .add_modifier(Modifier::BOLD),
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
                Cell::from(Span::styled(
                    created,
                    Style::default().fg(theme.text_dim),
                )),
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
    let footer_y = area.y + area.height.saturating_sub(1);
    if footer_y < area.y + area.height {
        let footer_area = Rect {
            y: footer_y,
            height: 1,
            x: area.x + 1,
            width: area.width.saturating_sub(2),
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
// Approval detail panel
// ---------------------------------------------------------------------------

fn render_detail(frame: &mut Frame, area: Rect, item: &ApprovalItem, theme: &Theme) {
    let state_color = approval_state_color(&item.state, theme);

    let block = Block::default()
        .title(Span::styled(
            format!(" Effect: {} ", item.kind),
            Style::default()
                .fg(theme.bone)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_active))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let glyph = approval_glyph(&item.state);
    let created = item.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let short_eid = &item.effect_id[..8.min(item.effect_id.len())];
    let short_rid = &item.run_id[..8.min(item.run_id.len())];

    let lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(glyph, Style::default().fg(state_color)),
            Span::raw("  "),
            Span::styled(
                item.state.clone(),
                Style::default()
                    .fg(state_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        kv_line("  Effect ID", short_eid, theme.text_primary, theme),
        kv_line("  Kind", &item.kind, kind_color(&item.kind, theme), theme),
        kv_line("  Agent", &item.agent_name, theme.text_primary, theme),
        kv_line("  Run", short_rid, theme.text_dim, theme),
        kv_line("  Created", &created, theme.text_primary, theme),
        Line::from(""),
        Line::from(Span::styled(
            "  Actions:",
            Style::default()
                .fg(theme.bone)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("    a", Style::default().fg(theme.success).add_modifier(Modifier::BOLD)),
            Span::styled("  Approve this effect", Style::default().fg(theme.text_dim)),
        ]),
        Line::from(vec![
            Span::styled("    d", Style::default().fg(theme.danger).add_modifier(Modifier::BOLD)),
            Span::styled("  Deny this effect", Style::default().fg(theme.text_dim)),
        ]),
        Line::from(vec![
            Span::styled("    Esc", Style::default().fg(theme.text_dim).add_modifier(Modifier::BOLD)),
            Span::styled("  Go back", Style::default().fg(theme.text_dim)),
        ]),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
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
    let dialog_area = Rect { x, y, width: dialog_w, height: dialog_h };

    let (action, action_color, key_hint) = if is_approve {
        ("APPROVE", theme.success, "'a' again to confirm  Esc to cancel")
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
            Style::default().fg(action_color).add_modifier(Modifier::BOLD),
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
        "pending"              => "◦",
        "claimed"              => "▶",
        "success" | "approved" => "✓",
        "failure" | "denied"   => "✗",
        "timeout"              => "⏱",
        _                      => "?",
    }
}

/// Color for the effect kind.
fn kind_color(kind: &str, theme: &Theme) -> ratatui::style::Color {
    match kind {
        "sign" | "broadcast" => theme.warning,
        "tool"               => theme.dream,
        "model"              => theme.rose,
        _                    => theme.text_primary,
    }
}

fn kv_line(
    key: &str,
    value: &str,
    value_color: ratatui::style::Color,
    theme: &Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{key:<14}"),
            Style::default().fg(theme.text_dim),
        ),
        Span::styled(value.to_owned(), Style::default().fg(value_color)),
    ])
}

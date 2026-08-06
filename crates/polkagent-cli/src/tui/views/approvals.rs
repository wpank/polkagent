//! Durable approval queue (F6).
//!
//! This view renders only the redaction-safe projection returned by the
//! shared interaction approval service. It never infers operations, risk, or
//! policy from effect storage rows.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
    Frame,
};

use crate::tui::state::{
    ApprovalActionTarget, ApprovalItem, ApprovalQueueStatus, ConfirmDialog, TuiState,
};
use crate::tui::theme::Theme;

/// Render the scoped approval queue and optional exact-identity confirmation.
pub fn render(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    if area.width >= 100 && state.approvals_scroll.selected.is_some() {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
            .split(area);
        render_list(frame, cols[0], state, theme);
        if let Some(item) = state
            .approvals_scroll
            .selected
            .and_then(|index| state.pending_approvals.get(index))
        {
            render_detail(frame, cols[1], item, theme);
        }
    } else {
        render_list(frame, area, state, theme);
    }

    match &state.confirm_dialog {
        ConfirmDialog::None => {}
        ConfirmDialog::ConfirmApprove(target) => {
            render_confirm_dialog(frame, area, target, true, theme);
        }
        ConfirmDialog::ConfirmDeny(target) => {
            render_confirm_dialog(frame, area, target, false, theme);
        }
    }
}

fn render_list(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let scope = state
        .approval_queue
        .conversation_id
        .as_deref()
        .map_or("no scope", short_id);
    let title = format!(
        " APPROVAL QUEUE ({} pending · conversation {scope}) ",
        state.pending_approvals.len()
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

    if state.pending_approvals.is_empty() {
        let (headline, guidance) = empty_guidance(state);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {headline}"),
                    Style::default().fg(status_color(state.approval_queue.status, theme)),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    format!("  {guidance}"),
                    Style::default().fg(theme.text_dim),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  r: refresh  F9: select/create/resume conversation",
                    Style::default().fg(theme.text_dim),
                )),
            ])
            .wrap(Wrap { trim: false }),
            inner,
        );
        return;
    }

    let header = Row::new(vec![
        Cell::from(" ST"),
        Cell::from("Title"),
        Cell::from("Status"),
        Cell::from("Run"),
        Cell::from("Expires"),
    ])
    .style(
        Style::default()
            .fg(theme.text_dim)
            .add_modifier(Modifier::UNDERLINED),
    );
    let selected = state.approvals_scroll.selected.unwrap_or(usize::MAX);
    let rows = state
        .pending_approvals
        .iter()
        .skip(state.approvals_scroll.offset)
        .take(inner.height.saturating_sub(2) as usize)
        .enumerate()
        .map(|(visible, item)| {
            let absolute = visible + state.approvals_scroll.offset;
            let style = if absolute == selected {
                Style::default()
                    .bg(theme.bg_highlight)
                    .fg(theme.rose_bright)
            } else {
                Style::default().fg(theme.text_primary)
            };
            Row::new(vec![
                Cell::from(Span::styled(
                    format!(" {}", approval_glyph(&item.status)),
                    Style::default().fg(approval_state_color(&item.status, theme)),
                )),
                Cell::from(item.title.clone()),
                Cell::from(Span::styled(
                    item.status.clone(),
                    Style::default().fg(approval_state_color(&item.status, theme)),
                )),
                Cell::from(short_id(&item.run_id).to_owned()),
                Cell::from(item.expires_at.map_or_else(
                    || "—".to_owned(),
                    |expiry| expiry.format("%m-%d %H:%M").to_string(),
                )),
            ])
            .style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(4),
                Constraint::Min(18),
                Constraint::Length(10),
                Constraint::Length(10),
                Constraint::Length(12),
            ],
        )
        .header(header),
        inner,
    );

    let footer = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(1),
        width: inner.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            " Enter: detail  a: approve  d: deny  r: refresh  j/k: navigate",
            Style::default().fg(theme.text_dim),
        )),
        footer,
    );
}

fn empty_guidance(state: &TuiState) -> (&'static str, String) {
    let message = state.approval_queue.message.clone().unwrap_or_default();
    match state.approval_queue.status {
        ApprovalQueueStatus::Unscoped => (
            "No durable conversation selected.",
            if message.is_empty() {
                "Open F9 and select, create, or resume a durable Console conversation.".to_owned()
            } else {
                message
            },
        ),
        ApprovalQueueStatus::Loading => (
            "Loading scoped approvals…",
            "The terminal remains interactive while the service responds.".to_owned(),
        ),
        ApprovalQueueStatus::Resolving => ("Resolving approval…", message),
        ApprovalQueueStatus::Unavailable => (
            "Approval authority unavailable.",
            if message.is_empty() {
                "Configure an authenticated durable approval authority and restart.".to_owned()
            } else {
                message
            },
        ),
        ApprovalQueueStatus::Failed => ("Approval request failed.", message),
        ApprovalQueueStatus::Ready => (
            "No pending approvals in this conversation.",
            if message.is_empty() {
                "Pending durable approval requests will appear here.".to_owned()
            } else {
                message
            },
        ),
    }
}

fn render_detail(frame: &mut Frame, area: Rect, item: &ApprovalItem, theme: &Theme) {
    let block = Block::default()
        .title(Span::styled(
            " APPROVAL DETAIL ",
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border));
    let expiry = item
        .expires_at
        .map_or_else(|| "—".to_owned(), |expiry| expiry.to_rfc3339());
    let lines = vec![
        detail_line("Approval", &item.approval_id, theme),
        detail_line("Effect", &item.effect_id, theme),
        detail_line("Run", &item.run_id, theme),
        detail_line(
            "Tool call",
            item.tool_call_id.as_deref().unwrap_or("—"),
            theme,
        ),
        detail_line("Status", &item.status, theme),
        Line::from(""),
        detail_line("Title", &item.title, theme),
        detail_line("Description", &item.description, theme),
        detail_line(
            "Policy",
            item.policy_reason.as_deref().unwrap_or("—"),
            theme,
        ),
        detail_line("Expires", &expiry, theme),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn detail_line<'a>(label: &'a str, value: &'a str, theme: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!(" {label}: "), Style::default().fg(theme.text_dim)),
        Span::styled(value, Style::default().fg(theme.bone)),
    ])
}

fn render_confirm_dialog(
    frame: &mut Frame,
    area: Rect,
    target: &ApprovalActionTarget,
    approve: bool,
    theme: &Theme,
) {
    let width = 58;
    let height = 8;
    let dialog = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let (action, color, key) = if approve {
        ("APPROVE", theme.success, 'a')
    } else {
        ("DENY", theme.danger, 'd')
    };
    let block = Block::default()
        .title(format!(" Confirm: {action} "))
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(theme.bg_void));
    let inner = block.inner(dialog);
    frame.render_widget(Clear, dialog);
    frame.render_widget(block, dialog);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            detail_line("Approval", &target.approval_id, theme),
            detail_line("Conversation", &target.conversation_id, theme),
            Line::from(""),
            Line::from(Span::styled(
                format!(" Press '{key}' again to {action}; Esc cancels."),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )),
        ]),
        inner,
    );
}

fn short_id(value: &str) -> &str {
    &value[..value.len().min(8)]
}

fn status_color(status: ApprovalQueueStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        ApprovalQueueStatus::Unavailable | ApprovalQueueStatus::Failed => theme.danger,
        ApprovalQueueStatus::Loading | ApprovalQueueStatus::Resolving => theme.warning,
        ApprovalQueueStatus::Ready => theme.success,
        ApprovalQueueStatus::Unscoped => theme.text_dim,
    }
}

fn approval_state_color(status: &str, theme: &Theme) -> ratatui::style::Color {
    match status {
        "pending" => theme.warning,
        "approved" => theme.success,
        "denied" | "expired" | "cancelled" => theme.danger,
        _ => theme.text_dim,
    }
}

fn approval_glyph(status: &str) -> &'static str {
    match status {
        "pending" => "◦",
        "approved" => "✓",
        "denied" => "✗",
        "expired" => "⏱",
        "cancelled" => "■",
        _ => "?",
    }
}

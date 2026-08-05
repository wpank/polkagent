//! Interactive console view (F9).

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::input::InputMode;
use crate::tui::interaction::ConsoleRunStatus;
use crate::tui::state::TuiState;
use crate::tui::theme::Theme;

pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &TuiState,
    input_mode: InputMode,
    theme: &Theme,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(area);

    render_session(frame, rows[0], state, theme);
    render_transcript(frame, rows[1], state, theme);
    render_composer(frame, rows[2], state, input_mode, theme);
}

fn render_session(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let agent = state
        .interaction
        .agent_name
        .as_deref()
        .unwrap_or("no active agent");
    let (status, color) = state
        .interaction
        .run
        .as_ref()
        .map_or(("idle", theme.text_dim), |run| {
            let color = match run.status {
                ConsoleRunStatus::Completed => theme.success,
                ConsoleRunStatus::Failed | ConsoleRunStatus::TimedOut => theme.danger,
                ConsoleRunStatus::Cancelled => theme.warning,
                _ => theme.rose_bright,
            };
            (run.status.label(), color)
        });
    let run_id = state
        .interaction
        .run
        .as_ref()
        .and_then(|run| run.run_id.as_deref())
        .map_or("--------", |id| &id[..8.min(id.len())]);

    let block = Block::default()
        .title(Span::styled(
            " SESSION ",
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_active))
        .style(Style::default().bg(theme.bg_raised));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" Agent  {agent}"), Style::default().fg(theme.bone)),
            Span::styled("   State  ", Style::default().fg(theme.text_dim)),
            Span::styled(
                status,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("   Run  ", Style::default().fg(theme.text_dim)),
            Span::styled(run_id, Style::default().fg(theme.text_primary)),
        ])),
        inner,
    );
}

fn render_transcript(frame: &mut Frame, area: Rect, state: &TuiState, theme: &Theme) {
    let block = Block::default()
        .title(Span::styled(
            " LIVE OUTPUT ",
            Style::default().fg(theme.rose).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.bg_raised));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(run) = &state.interaction.run {
        lines.push(Line::from(vec![
            Span::styled(" YOU  ", Style::default().fg(theme.rose_bright)),
            Span::styled(run.prompt.clone(), Style::default().fg(theme.text_primary)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " AGENT",
            Style::default().fg(theme.bone).add_modifier(Modifier::BOLD),
        )));
        if run.output.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("   {}", run.detail),
                Style::default().fg(theme.text_dim),
            )));
        } else {
            lines.extend(run.output.lines().map(|line| {
                Line::from(Span::styled(
                    format!(" {line}"),
                    Style::default().fg(theme.text_primary),
                ))
            }));
            lines.push(Line::from(""));
            let usage = if run.status == ConsoleRunStatus::Completed {
                format!(
                    " {} · {} input / {} output tokens",
                    run.detail, run.input_tokens, run.output_tokens
                )
            } else {
                format!(" {}", run.detail)
            };
            lines.push(Line::from(Span::styled(
                usage,
                Style::default().fg(theme.text_dim),
            )));
        }
    } else {
        lines.push(Line::from(Span::styled(
            " Choose an active agent in F2, then press p to compose a prompt.",
            Style::default().fg(theme.text_dim),
        )));
        lines.push(Line::from(Span::styled(
            " You can also press p here to use the first active agent.",
            Style::default().fg(theme.text_dim),
        )));
    }

    let height = usize::from(inner.height);
    let scroll = u16::try_from(lines.len().saturating_sub(height)).unwrap_or(u16::MAX);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        inner,
    );
}

fn render_composer(
    frame: &mut Frame,
    area: Rect,
    state: &TuiState,
    input_mode: InputMode,
    theme: &Theme,
) {
    let composing = input_mode == InputMode::Prompt;
    let title = if composing {
        " PROMPT · Enter send · Esc cancel "
    } else if state
        .interaction
        .run
        .as_ref()
        .is_some_and(|run| !run.status.is_terminal())
    {
        " RUNNING · x cancel "
    } else {
        " p compose · F3 runs · F5 timeline "
    };
    let border = if composing {
        theme.rose_bright
    } else {
        theme.border
    };
    let block = Block::default()
        .title(Span::styled(title, Style::default().fg(theme.text_dim)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(theme.bg_raised));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let text = if composing {
        format!("> {}", state.interaction.prompt_buffer)
    } else {
        "> press p to write the next prompt".to_owned()
    };
    frame.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(theme.text_primary))),
        inner,
    );

    if composing && inner.width > 0 {
        let cursor_x = inner
            .x
            .saturating_add(2)
            .saturating_add(
                u16::try_from(state.interaction.prompt_buffer.chars().count()).unwrap_or(u16::MAX),
            )
            .min(inner.x.saturating_add(inner.width.saturating_sub(1)));
        frame.set_cursor_position((cursor_x, inner.y));
    }
}

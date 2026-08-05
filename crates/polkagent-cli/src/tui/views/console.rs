//! Interactive console view (F9).

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::input::InputMode;
use crate::tui::interaction::{ConsoleRunStatus, SlashCommandMenu};
use crate::tui::state::TuiState;
use crate::tui::theme::Theme;

pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &TuiState,
    input_mode: InputMode,
    theme: &Theme,
) {
    let composer_height = if input_mode == InputMode::Prompt {
        if state.interaction.slash_command_menu().is_some() {
            11
        } else {
            7
        }
    } else {
        3
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(composer_height),
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
    let slash_menu = if composing {
        state.interaction.slash_command_menu()
    } else {
        None
    };
    let title = if slash_menu.is_some() {
        " SLASH HELP · ↑/↓ select · Tab complete · Esc dismiss "
    } else if composing {
        " PROMPT · Enter send · Shift+Enter newline · ↑/↓ line/history · Esc cancel "
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

    if !composing {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "> press p to write the next prompt",
                Style::default().fg(theme.text_primary),
            )),
            inner,
        );
        return;
    }

    let editor_area = if let Some(menu) = slash_menu.as_ref() {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);
        render_slash_menu(frame, rows[1], menu, theme);
        rows[0]
    } else {
        inner
    };

    let buffer = &state.interaction.prompt_buffer;
    let cursor = state.interaction.cursor();
    let before_cursor = &buffer[..cursor];
    let cursor_line = before_cursor.bytes().filter(|byte| *byte == b'\n').count();
    let line_start = before_cursor.rfind('\n').map_or(0, |index| index + 1);
    let cursor_column = 2 + Span::raw(&buffer[line_start..cursor]).width();

    let lines = buffer.split('\n').enumerate().map(|(index, line)| {
        let prefix = if index == 0 { "> " } else { "  " };
        Line::from(vec![
            Span::styled(prefix, Style::default().fg(theme.rose_bright)),
            Span::styled(line.to_owned(), Style::default().fg(theme.text_primary)),
        ])
    });
    let visible_height = usize::from(editor_area.height.max(1));
    let visible_width = usize::from(editor_area.width.max(1));
    let vertical_scroll = cursor_line.saturating_sub(visible_height.saturating_sub(1));
    let horizontal_scroll = cursor_column.saturating_sub(visible_width.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(lines.collect::<Text>()).scroll((
            u16::try_from(vertical_scroll).unwrap_or(u16::MAX),
            u16::try_from(horizontal_scroll).unwrap_or(u16::MAX),
        )),
        editor_area,
    );

    if editor_area.width > 0 && editor_area.height > 0 {
        let cursor_x = editor_area
            .x
            .saturating_add(
                u16::try_from(cursor_column.saturating_sub(horizontal_scroll)).unwrap_or(u16::MAX),
            )
            .min(
                editor_area
                    .x
                    .saturating_add(editor_area.width.saturating_sub(1)),
            );
        let cursor_y = editor_area
            .y
            .saturating_add(
                u16::try_from(cursor_line.saturating_sub(vertical_scroll)).unwrap_or(u16::MAX),
            )
            .min(
                editor_area
                    .y
                    .saturating_add(editor_area.height.saturating_sub(1)),
            );
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

fn render_slash_menu(frame: &mut Frame, area: Rect, menu: &SlashCommandMenu, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let visible_entries = usize::from(area.height.saturating_sub(1));
    let window_start = menu
        .selected
        .saturating_sub(visible_entries.saturating_sub(1))
        .min(menu.candidates.len().saturating_sub(visible_entries));
    let mut lines = menu
        .candidates
        .iter()
        .enumerate()
        .skip(window_start)
        .take(visible_entries)
        .map(|(index, candidate)| {
            let selected = index == menu.selected;
            let marker = if selected { "▸ " } else { "  " };
            let command_style = if selected {
                Style::default()
                    .fg(theme.rose_bright)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.bone)
            };
            let aliases = if selected && !candidate.aliases.is_empty() {
                format!(" · aliases: /{}", candidate.aliases.join(", /"))
            } else {
                String::new()
            };
            Line::from(vec![
                Span::styled(marker, command_style),
                Span::styled(candidate.usage(), command_style),
                Span::styled(
                    format!(" — {}{aliases}", candidate.description),
                    Style::default().fg(theme.text_dim),
                ),
            ])
        })
        .collect::<Vec<_>>();
    lines.push(Line::from(Span::styled(
        format!(
            "  Shared catalog · {}/{} · execution is not wired in Console yet",
            menu.selected + 1,
            menu.candidates.len()
        ),
        Style::default().fg(theme.warning),
    )));
    frame.render_widget(Paragraph::new(lines), area);
}

//! Keyboard input handling and action dispatch for the Polkagent TUI.
#![allow(dead_code)]
//!
//! All user input is translated to a [`TuiAction`] variant before being
//! applied to state via `App::apply_action`. The render path is never called
//! from inside an input handler.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::app::Tab;

// ---------------------------------------------------------------------------
// Input mode
// ---------------------------------------------------------------------------

/// Which input mode the TUI is currently in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// Normal navigation mode — all key bindings active.
    #[default]
    Normal,
    /// Text input mode — most bindings suppressed, characters go to the buffer.
    Insert,
    /// Interactive console composer — characters go to the agent prompt.
    Prompt,
    /// Durable Console session selector overlay.
    SessionPicker,
    /// Command-palette mode — input goes to the palette filter.
    Command,
}

// ---------------------------------------------------------------------------
// TuiAction
// ---------------------------------------------------------------------------

/// Every state-mutating event the TUI can respond to.
///
/// This is a simplified TEA (The Elm Architecture) message type. All mutations
/// to `TuiState` go through `App::apply_action`.
#[derive(Debug, Clone)]
pub enum TuiAction {
    // -- Navigation ----------------------------------------------------------
    /// Switch to a different region tab.
    NavigateTab(Tab),
    /// Move selection or focus up.
    NavigateUp,
    /// Move selection or focus down.
    NavigateDown,
    /// Select the focused item (drill-down).
    Select,
    /// Go back / close detail panel.
    Back,
    /// Quit the TUI.
    Quit,

    // -- Run detail / timeline / approvals -----------------------------------
    /// Open the run detail view for the currently selected run.
    SelectRun,
    /// Switch to the event timeline for the selected run.
    ViewTimeline,
    /// Switch to the approval queue view.
    ViewApprovals,
    /// Approve the selected effect in the approval queue.
    ApproveEffect,
    /// Deny the selected effect in the approval queue.
    DenyEffect,
    /// Toggle between sub-panels (e.g. info/turns in run detail).
    TogglePanel,

    // -- Interactive console ------------------------------------------------
    /// Open the console composer for the selected/default active agent.
    OpenPrompt,
    /// Open or refresh the durable Console session selector.
    OpenSessionPicker,
    /// Move the durable session selector up.
    SessionPickerUp,
    /// Move the durable session selector down.
    SessionPickerDown,
    /// Load the selected durable Console session.
    SessionPickerConfirm,
    /// Close the durable Console session selector.
    SessionPickerClose,
    /// Append a character to the console prompt.
    PromptInput(char),
    /// Insert one bracketed-paste payload as bounded composer content.
    PromptPaste(String),
    /// Insert a newline at the console prompt cursor.
    PromptNewline,
    /// Delete the character before the console prompt cursor.
    PromptBackspace,
    /// Delete the character at the console prompt cursor.
    PromptDelete,
    /// Move the console prompt cursor left by one extended grapheme cluster.
    PromptMoveLeft,
    /// Move the console prompt cursor right by one extended grapheme cluster.
    PromptMoveRight,
    /// Move to the previous line, or the previous history entry at the top.
    PromptMoveUp,
    /// Move to the next line, or the next history entry at the bottom.
    PromptMoveDown,
    /// Move to the start of the current prompt line.
    PromptMoveHome,
    /// Move to the end of the current prompt line.
    PromptMoveEnd,
    /// Accept the highlighted shared slash-command completion.
    PromptAcceptCompletion,
    /// Start a real run using the current prompt.
    PromptSubmit,
    /// Cancel the run currently owned by the console.
    CancelActiveRun,

    // -- Memory / audit ------------------------------------------------------
    /// Activate the memory search bar (switches to insert mode on the Memory tab).
    Search,
    /// Append a character to the memory search query (insert mode).
    SearchInput(char),
    /// Delete the last character from the memory search query (insert mode).
    SearchBackspace,
    /// Submit the current search query (exit insert mode, keep filter).
    SearchSubmit,
    /// Delete the selected memory entry.
    DeleteEntry,
    /// Scroll to the bottom of the active list (G in audit/timeline).
    ScrollToBottom,
    /// Scroll to the top of the active list (g in audit/timeline).
    ScrollToTop,
    /// Cycle the active filter (f in audit view).
    CycleFilter,

    // -- Scroll --------------------------------------------------------------
    /// Scroll the active list up by `n` rows.
    ScrollUp(usize),
    /// Scroll the active list down by `n` rows.
    ScrollDown(usize),

    // -- Data ----------------------------------------------------------------
    /// Trigger a data refresh from the database.
    Refresh,

    // -- Terminal resize -----------------------------------------------------
    /// Terminal window was resized.
    Resize(u16, u16),
}

// ---------------------------------------------------------------------------
// Key event translation
// ---------------------------------------------------------------------------

/// Translate a raw crossterm key event into a [`TuiAction`].
///
/// Returns `None` for key events that have no registered binding in the
/// current `input_mode`.
#[must_use]
pub fn key_to_action(key: KeyEvent, mode: InputMode) -> Option<TuiAction> {
    // Always handle quit / resize regardless of mode.
    match key.code {
        // Ctrl+C is always quit.
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Some(TuiAction::Quit);
        }
        _ => {}
    }

    match mode {
        InputMode::Normal => normal_mode_key(key),
        InputMode::Insert => match key.code {
            KeyCode::Esc => Some(TuiAction::Back),
            KeyCode::Enter => Some(TuiAction::SearchSubmit),
            KeyCode::Backspace => Some(TuiAction::SearchBackspace),
            KeyCode::Char(c) => Some(TuiAction::SearchInput(c)),
            _ => None,
        },
        InputMode::Prompt => prompt_mode_key(key),
        InputMode::SessionPicker => match key.code {
            KeyCode::Esc => Some(TuiAction::SessionPickerClose),
            KeyCode::Up | KeyCode::Char('k') => Some(TuiAction::SessionPickerUp),
            KeyCode::Down | KeyCode::Char('j') => Some(TuiAction::SessionPickerDown),
            KeyCode::Enter => Some(TuiAction::SessionPickerConfirm),
            _ => None,
        },
        InputMode::Command => match key.code {
            KeyCode::Esc => Some(TuiAction::Back),
            _ => None,
        },
    }
}

/// Key bindings active while editing the Console prompt.
fn prompt_mode_key(key: KeyEvent) -> Option<TuiAction> {
    match key.code {
        KeyCode::Esc => Some(TuiAction::Back),
        KeyCode::Enter
            if key
                .modifiers
                .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
        {
            Some(TuiAction::PromptNewline)
        }
        KeyCode::Enter => Some(TuiAction::PromptSubmit),
        KeyCode::Backspace => Some(TuiAction::PromptBackspace),
        KeyCode::Delete => Some(TuiAction::PromptDelete),
        KeyCode::Left => Some(TuiAction::PromptMoveLeft),
        KeyCode::Right => Some(TuiAction::PromptMoveRight),
        KeyCode::Up => Some(TuiAction::PromptMoveUp),
        KeyCode::Down => Some(TuiAction::PromptMoveDown),
        KeyCode::Home => Some(TuiAction::PromptMoveHome),
        KeyCode::End => Some(TuiAction::PromptMoveEnd),
        KeyCode::Tab => Some(TuiAction::PromptAcceptCompletion),
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::PromptMoveHome)
        }
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::PromptMoveEnd)
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::PromptMoveUp)
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::PromptMoveDown)
        }
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::PromptNewline)
        }
        KeyCode::Char(c)
            if !key.modifiers.intersects(
                KeyModifiers::CONTROL
                    | KeyModifiers::ALT
                    | KeyModifiers::SUPER
                    | KeyModifiers::HYPER
                    | KeyModifiers::META,
            ) =>
        {
            Some(TuiAction::PromptInput(c))
        }
        _ => None,
    }
}

/// Key bindings active in Normal mode.
fn normal_mode_key(key: KeyEvent) -> Option<TuiAction> {
    match key.code {
        // ── Tab navigation (F1–F9) ──────────────────────────────────────
        // Numeric keys mirror the F-key tab shortcuts.
        KeyCode::F(1) | KeyCode::Char('1') => Some(TuiAction::NavigateTab(Tab::Dashboard)),
        KeyCode::F(2) | KeyCode::Char('2') => Some(TuiAction::NavigateTab(Tab::Agents)),
        KeyCode::F(3) | KeyCode::Char('3') => Some(TuiAction::NavigateTab(Tab::Runs)),
        KeyCode::F(4) | KeyCode::Char('4') => Some(TuiAction::NavigateTab(Tab::System)),
        KeyCode::F(5) | KeyCode::Char('5') => Some(TuiAction::NavigateTab(Tab::Timeline)),
        KeyCode::F(6) | KeyCode::Char('6') => Some(TuiAction::NavigateTab(Tab::Approvals)),
        KeyCode::F(7) | KeyCode::Char('7') => Some(TuiAction::NavigateTab(Tab::Memory)),
        KeyCode::F(8) | KeyCode::Char('8') => Some(TuiAction::NavigateTab(Tab::Audit)),
        KeyCode::F(9) | KeyCode::Char('9') => Some(TuiAction::NavigateTab(Tab::Console)),

        // ── Quit ─────────────────────────────────────────────────────────
        KeyCode::Char('q' | 'Q') => Some(TuiAction::Quit),

        // ── Navigation ───────────────────────────────────────────────────
        KeyCode::Up | KeyCode::Char('k') => Some(TuiAction::NavigateUp),
        KeyCode::Down | KeyCode::Char('j') => Some(TuiAction::NavigateDown),

        // ── Scroll (page / half-page) ────────────────────────────────────
        KeyCode::PageUp => Some(TuiAction::ScrollUp(10)),
        KeyCode::PageDown => Some(TuiAction::ScrollDown(10)),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::ScrollUp(5))
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::ScrollDown(5))
        }

        // ── Jump to top / bottom (audit and timeline) ────────────────────
        KeyCode::Char('G') => Some(TuiAction::ScrollToBottom),
        KeyCode::Char('g') => Some(TuiAction::ScrollToTop),

        // ── Select / back ────────────────────────────────────────────────
        KeyCode::Enter => Some(TuiAction::Select),
        KeyCode::Esc | KeyCode::Backspace => Some(TuiAction::Back),

        // ── Panel toggle (Tab key) ───────────────────────────────────────
        KeyCode::Tab => Some(TuiAction::TogglePanel),

        // ── Interactive console ─────────────────────────────────────────
        KeyCode::Char('p') => Some(TuiAction::OpenPrompt),
        KeyCode::Char('s') => Some(TuiAction::OpenSessionPicker),
        KeyCode::Char('x') => Some(TuiAction::CancelActiveRun),

        // ── Approval actions ─────────────────────────────────────────────
        // NOTE: These fire globally but the handler in App::apply_action
        // guards with `if self.active_tab == Tab::Approvals`, so they are
        // effectively no-ops on other tabs.
        KeyCode::Char('a') => Some(TuiAction::ApproveEffect),
        KeyCode::Char('d') => Some(TuiAction::DenyEffect),

        // ── Memory / audit actions ───────────────────────────────────────
        KeyCode::Char('/') => Some(TuiAction::Search),
        KeyCode::Delete => Some(TuiAction::DeleteEntry),
        KeyCode::Char('f') => Some(TuiAction::CycleFilter),

        // ── Data refresh ─────────────────────────────────────────────────
        KeyCode::Char('r') => Some(TuiAction::Refresh),

        _ => None,
    }
}

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
        InputMode::Insert | InputMode::Command => {
            // In insert/command mode only Escape cancels back to normal.
            match key.code {
                KeyCode::Esc => Some(TuiAction::Back),
                _ => None,
            }
        }
    }
}

/// Key bindings active in Normal mode.
fn normal_mode_key(key: KeyEvent) -> Option<TuiAction> {
    match key.code {
        // ── Tab navigation (F1–F4) ──────────────────────────────────────
        KeyCode::F(1) => Some(TuiAction::NavigateTab(Tab::Dashboard)),
        KeyCode::F(2) => Some(TuiAction::NavigateTab(Tab::Agents)),
        KeyCode::F(3) => Some(TuiAction::NavigateTab(Tab::Runs)),
        KeyCode::F(4) => Some(TuiAction::NavigateTab(Tab::System)),

        // ── Quick tab shortcuts ──────────────────────────────────────────
        KeyCode::Char('1') => Some(TuiAction::NavigateTab(Tab::Dashboard)),
        KeyCode::Char('2') => Some(TuiAction::NavigateTab(Tab::Agents)),
        KeyCode::Char('3') => Some(TuiAction::NavigateTab(Tab::Runs)),
        KeyCode::Char('4') => Some(TuiAction::NavigateTab(Tab::System)),

        // ── Quit ─────────────────────────────────────────────────────────
        KeyCode::Char('q') | KeyCode::Char('Q') => Some(TuiAction::Quit),

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

        // ── Select / back ────────────────────────────────────────────────
        KeyCode::Enter => Some(TuiAction::Select),
        KeyCode::Esc | KeyCode::Backspace => Some(TuiAction::Back),

        // ── Data refresh ─────────────────────────────────────────────────
        KeyCode::Char('r') | KeyCode::F(5) => Some(TuiAction::Refresh),

        _ => None,
    }
}

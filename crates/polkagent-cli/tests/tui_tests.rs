//! TUI widget and view unit tests for the polkagent-cli crate.
//!
//! Tests cover the ROSEDUST theme, input/key mapping, state management,
//! widget rendering (using ratatui's TestBackend), and responsive layout
//! breakpoints.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};

use polkagent_cli::tui::{
    app::Tab,
    input::{InputMode, TuiAction, key_to_action},
    state::{ScrollState, TuiState},
    theme::Theme,
    widgets::{header_bar, status_bar},
};

// =========================================================================
// Helpers
// =========================================================================

/// Create a `KeyEvent` with no modifiers.
fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Create a `KeyEvent` with Ctrl held.
fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

/// Extract all text from a TestBackend buffer as a single string.
fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            text.push_str(cell.symbol());
        }
    }
    text
}

/// Helper to extract a `Color::Rgb(r,g,b)` tuple. Returns `None` for
/// non-RGB variants.
fn as_rgb(color: ratatui::style::Color) -> Option<(u8, u8, u8)> {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

/// Collect all colour fields from a `Theme` into a `Vec<Color>`.
fn all_theme_colors(t: &Theme) -> Vec<ratatui::style::Color> {
    vec![
        t.bg_void,
        t.bg_raised,
        t.bg_mid,
        t.bg_warm,
        t.bg_highlight,
        t.rose_deep,
        t.rose_ember,
        t.rose_dim,
        t.rose,
        t.rose_bright,
        t.bone,
        t.bone_dim,
        t.success,
        t.warning,
        t.danger,
        t.dream,
        t.text_phantom,
        t.text_ghost,
        t.text_dim,
        t.text_primary,
        t.border,
        t.border_active,
        t.border_dream,
        t.dot_pink,
        t.parachain_violet,
        t.finalized_teal,
        t.pending_amber,
        t.scanline_dark,
        t.phosphor_res,
        t.noise_warm,
        t.noise_cool,
    ]
}

// =========================================================================
// 1. Theme tests
// =========================================================================

#[test]
fn test_dark_theme_no_pure_black() {
    let t = Theme::dark();
    // bg_void is the deepest background; it must NOT be pure black.
    if let Some((r, g, b)) = as_rgb(t.bg_void) {
        assert!(
            !(r == 0 && g == 0 && b == 0),
            "bg_void should not be pure black (0,0,0), got ({r},{g},{b})"
        );
    } else {
        panic!("bg_void should be an Rgb colour in the dark theme");
    }
}

#[test]
fn test_dark_theme_no_pure_white() {
    let t = Theme::dark();
    for color in all_theme_colors(&t) {
        if let Some((r, g, b)) = as_rgb(color) {
            assert!(
                !(r == 255 && g == 255 && b == 255),
                "No colour in the dark theme should be pure white (255,255,255), \
                 but found ({r},{g},{b})"
            );
        }
    }
}

#[test]
fn test_no_color_theme_is_plain() {
    let t = Theme::no_color();
    for color in all_theme_colors(&t) {
        assert_eq!(
            color,
            ratatui::style::Color::Reset,
            "no_color theme must use only Color::Reset, found {color:?}"
        );
    }
}

#[test]
fn test_status_color_mapping() {
    let t = Theme::dark();

    // Every canonical state string must produce a valid colour (not Reset).
    let states = [
        "finalized",
        "succeeded",
        "active",
        "completed",
        "working",
        "signed",
        "submitted",
        "included",
        "started",
        "waiting_approval",
        "pending",
        "queued",
        "created",
        "unknown",
        "failed",
        "reverted",
        "denied",
        "expired",
        "timed_out",
        "cancelled",
        "stopped",
        "idle",
        "paused",
        "deactivated",
        "configured",
        // Fall-through (unknown states).
        "some_other_state",
    ];

    for &s in &states {
        let c = t.status_color(s);
        // All dark-theme status colours are Rgb.
        assert!(
            as_rgb(c).is_some(),
            "status_color({s:?}) should return an Rgb colour, got {c:?}"
        );
    }
}

#[test]
fn test_progress_color_gradient() {
    let t = Theme::dark();

    // 0.0 => danger (crimson)
    assert_eq!(t.progress_color(0.0), t.danger, "0.0 should map to danger");

    // 0.5 => warning (amber), since 0.4 <= 0.5 < 0.75
    assert_eq!(
        t.progress_color(0.5),
        t.warning,
        "0.5 should map to warning"
    );

    // 1.0 => success (jade)
    assert_eq!(
        t.progress_color(1.0),
        t.success,
        "1.0 should map to success"
    );

    // Boundary checks.
    assert_eq!(
        t.progress_color(0.39),
        t.danger,
        "0.39 should map to danger"
    );
    assert_eq!(
        t.progress_color(0.4),
        t.warning,
        "0.4 should map to warning"
    );
    assert_eq!(
        t.progress_color(0.74),
        t.warning,
        "0.74 should map to warning"
    );
    assert_eq!(
        t.progress_color(0.75),
        t.success,
        "0.75 should map to success"
    );
}

// =========================================================================
// 2. Input / key mapping tests
// =========================================================================

#[test]
fn test_quit_keys() {
    // 'q' in Normal mode produces Quit.
    let action_q = key_to_action(key(KeyCode::Char('q')), InputMode::Normal);
    assert!(
        matches!(action_q, Some(TuiAction::Quit)),
        "q should produce Quit, got {action_q:?}"
    );

    // Ctrl+C in Normal mode produces Quit.
    let action_ctrl_c = key_to_action(ctrl(KeyCode::Char('c')), InputMode::Normal);
    assert!(
        matches!(action_ctrl_c, Some(TuiAction::Quit)),
        "Ctrl+C should produce Quit, got {action_ctrl_c:?}"
    );

    // Ctrl+C works even in Insert mode.
    let action_insert_ctrl_c = key_to_action(ctrl(KeyCode::Char('c')), InputMode::Insert);
    assert!(
        matches!(action_insert_ctrl_c, Some(TuiAction::Quit)),
        "Ctrl+C in Insert mode should produce Quit, got {action_insert_ctrl_c:?}"
    );
}

#[test]
fn test_navigation_keys() {
    // j => NavigateDown
    let j = key_to_action(key(KeyCode::Char('j')), InputMode::Normal);
    assert!(
        matches!(j, Some(TuiAction::NavigateDown)),
        "j should produce NavigateDown, got {j:?}"
    );

    // k => NavigateUp
    let k = key_to_action(key(KeyCode::Char('k')), InputMode::Normal);
    assert!(
        matches!(k, Some(TuiAction::NavigateUp)),
        "k should produce NavigateUp, got {k:?}"
    );

    // Up arrow => NavigateUp
    let up = key_to_action(key(KeyCode::Up), InputMode::Normal);
    assert!(
        matches!(up, Some(TuiAction::NavigateUp)),
        "Up should produce NavigateUp, got {up:?}"
    );

    // Down arrow => NavigateDown
    let down = key_to_action(key(KeyCode::Down), InputMode::Normal);
    assert!(
        matches!(down, Some(TuiAction::NavigateDown)),
        "Down should produce NavigateDown, got {down:?}"
    );
}

#[test]
fn test_tab_keys() {
    let f1 = key_to_action(key(KeyCode::F(1)), InputMode::Normal);
    assert!(
        matches!(f1, Some(TuiAction::NavigateTab(Tab::Dashboard))),
        "F1 should switch to Dashboard, got {f1:?}"
    );

    let f2 = key_to_action(key(KeyCode::F(2)), InputMode::Normal);
    assert!(
        matches!(f2, Some(TuiAction::NavigateTab(Tab::Agents))),
        "F2 should switch to Agents, got {f2:?}"
    );

    let f3 = key_to_action(key(KeyCode::F(3)), InputMode::Normal);
    assert!(
        matches!(f3, Some(TuiAction::NavigateTab(Tab::Runs))),
        "F3 should switch to Runs, got {f3:?}"
    );

    let f4 = key_to_action(key(KeyCode::F(4)), InputMode::Normal);
    assert!(
        matches!(f4, Some(TuiAction::NavigateTab(Tab::System))),
        "F4 should switch to System, got {f4:?}"
    );
}

#[test]
fn test_any_key_no_panic() {
    // Exhaustive check: a broad range of key codes should never panic.
    // We test all printable ASCII, all F-keys, arrow keys, and modifiers.
    let modes = [InputMode::Normal, InputMode::Insert, InputMode::Command];
    let modifiers = [
        KeyModifiers::NONE,
        KeyModifiers::CONTROL,
        KeyModifiers::SHIFT,
        KeyModifiers::ALT,
    ];

    let key_codes: Vec<KeyCode> = {
        let mut codes = Vec::new();
        // All printable ASCII characters.
        for ch in ' '..='~' {
            codes.push(KeyCode::Char(ch));
        }
        // Function keys F1-F12.
        for n in 1..=12 {
            codes.push(KeyCode::F(n));
        }
        // Navigation keys.
        codes.extend_from_slice(&[
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Delete,
            KeyCode::Insert,
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Backspace,
            KeyCode::Null,
        ]);
        codes
    };

    for &mode in &modes {
        for &mods in &modifiers {
            for code in &key_codes {
                let event = KeyEvent::new(*code, mods);
                // This must not panic -- the return can be Some or None.
                let result = key_to_action(event, mode);
                // Ensure the result is either None or a valid TuiAction.
                match result {
                    None => {} // fine
                    Some(_action) => {} // fine
                }
            }
        }
    }
}

// =========================================================================
// 3. State tests
// =========================================================================

#[test]
fn test_default_state() {
    let state = TuiState::default();

    assert!(state.agents.is_empty(), "Default state should have no agents");
    assert!(state.runs.is_empty(), "Default state should have no runs");
    assert!(!state.health.db_ok, "Default health should report db_ok=false");
    assert!(state.selected_run.is_none(), "No run should be selected initially");
    assert!(!state.dirty, "Default state should not be dirty");
    assert!(state.last_refresh.is_none(), "No refresh should have occurred");
    assert!(state.last_error.is_none(), "No error should be present initially");
    assert_eq!(state.agents_scroll.offset, 0, "Scroll offset should start at 0");
    assert!(
        state.agents_scroll.selected.is_none(),
        "No item should be selected in agents scroll"
    );
}

#[test]
fn test_scroll_bounds() {
    let mut scroll = ScrollState::default();
    let total_items = 10usize;
    let visible = 5usize;

    // Scrolling up from zero should stay at zero (no negative).
    scroll.up();
    assert_eq!(
        scroll.selected,
        Some(0),
        "Scrolling up from 0 should clamp to 0"
    );
    assert_eq!(scroll.offset, 0, "Offset should remain 0");

    // Scroll down to the end.
    for _ in 0..20 {
        scroll.down(total_items, visible);
    }
    let selected = scroll.selected.expect("selected should be Some after scrolling");
    assert!(
        selected < total_items,
        "Selected ({selected}) should not exceed total items ({total_items})"
    );
    assert_eq!(
        selected,
        total_items - 1,
        "Selected should be clamped to last item"
    );

    // Scroll up all the way back.
    for _ in 0..20 {
        scroll.up();
    }
    assert_eq!(
        scroll.selected,
        Some(0),
        "Scrolling up should return to 0"
    );
    assert_eq!(scroll.offset, 0, "Offset should return to 0");

    // Edge case: scrolling with zero items.
    let mut empty_scroll = ScrollState::default();
    empty_scroll.down(0, visible);
    // With total_items=0, down() computes min(selected+1, 0-1) which is
    // 0.saturating_sub(1) = 0 via the saturating_sub, so selected stays 0.
    // This should not panic.
    assert_eq!(
        empty_scroll.selected.unwrap_or(0),
        0,
        "Scrolling down on empty list should clamp to 0"
    );

    // Reset should bring everything back to zero.
    scroll.reset();
    assert_eq!(scroll.offset, 0, "reset() should zero offset");
    assert_eq!(scroll.selected, Some(0), "reset() should set selected to 0");
}

#[test]
fn test_tab_navigation() {
    // Tab cycling should wrap around.
    let mut tab = Tab::Dashboard;

    // Go forward through all tabs.
    tab = tab.next(); // Agents
    assert_eq!(tab, Tab::Agents);
    tab = tab.next(); // Runs
    assert_eq!(tab, Tab::Runs);
    tab = tab.next(); // System
    assert_eq!(tab, Tab::System);
    tab = tab.next(); // Timeline
    assert_eq!(tab, Tab::Timeline);
    tab = tab.next(); // Approvals
    assert_eq!(tab, Tab::Approvals);
    tab = tab.next(); // Dashboard (wrap)
    assert_eq!(tab, Tab::Dashboard, "next() should wrap from Approvals to Dashboard");

    // Go backward through all tabs.
    tab = tab.prev(); // Approvals
    assert_eq!(tab, Tab::Approvals, "prev() should wrap from Dashboard to Approvals");
    tab = tab.prev(); // Timeline
    assert_eq!(tab, Tab::Timeline);
    tab = tab.prev(); // System
    assert_eq!(tab, Tab::System);
    tab = tab.prev(); // Runs
    assert_eq!(tab, Tab::Runs);
    tab = tab.prev(); // Agents
    assert_eq!(tab, Tab::Agents);
    tab = tab.prev(); // Dashboard
    assert_eq!(tab, Tab::Dashboard);

    // Tab::ALL should list all tabs.
    assert_eq!(Tab::ALL.len(), 6, "There should be exactly 6 tabs");
}

// =========================================================================
// 4. Widget rendering tests (TestBackend)
// =========================================================================

#[test]
fn test_header_bar_renders() {
    let backend = TestBackend::new(80, 1);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    let theme = Theme::dark();

    terminal
        .draw(|frame| {
            let area = frame.area();
            header_bar::render(frame, area, Tab::Dashboard, &theme);
        })
        .expect("draw failed");

    let text = buffer_text(&terminal);
    assert!(
        text.contains("POLKAGENT"),
        "Header bar should contain 'POLKAGENT', got: {text:?}"
    );
}

#[test]
fn test_status_bar_renders() {
    let backend = TestBackend::new(80, 1);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    let theme = Theme::dark();

    terminal
        .draw(|frame| {
            let area = frame.area();
            status_bar::render(frame, area, Tab::Dashboard, InputMode::Normal, None, &theme);
        })
        .expect("draw failed");

    let text = buffer_text(&terminal);
    // The dashboard status bar should show key hints.
    assert!(
        text.contains("q:quit") || text.contains("quit"),
        "Status bar should contain quit hint, got: {text:?}"
    );
}

#[test]
fn test_header_bar_narrow() {
    // 40 columns is below the standard breakpoint; should not panic.
    let backend = TestBackend::new(40, 1);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    let theme = Theme::dark();

    terminal
        .draw(|frame| {
            let area = frame.area();
            header_bar::render(frame, area, Tab::Agents, &theme);
        })
        .expect("header_bar should render without panic at 40 cols");

    // Just verify something was rendered.
    let text = buffer_text(&terminal);
    assert!(
        !text.trim().is_empty(),
        "Narrow header should still render some text"
    );
}

#[test]
fn test_status_bar_narrow() {
    // Very narrow terminal.
    let backend = TestBackend::new(30, 1);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    let theme = Theme::dark();

    terminal
        .draw(|frame| {
            let area = frame.area();
            status_bar::render(
                frame,
                area,
                Tab::Runs,
                InputMode::Normal,
                Some("test error"),
                &theme,
            );
        })
        .expect("status_bar should render without panic at 30 cols");

    let text = buffer_text(&terminal);
    assert!(
        !text.trim().is_empty(),
        "Narrow status bar should still render some text"
    );
}

// =========================================================================
// 5. Responsive layout tests
// =========================================================================

/// Render the dashboard into a terminal of the given size and return the
/// buffer text for inspection.
fn render_dashboard(width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    let theme = Theme::dark();
    let state = TuiState::default();

    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            polkagent_cli::tui::views::dashboard::render(frame, area, &state, &theme);
        })
        .expect("dashboard render failed");

    buffer_text(&terminal)
}

#[test]
fn test_dashboard_compact_layout() {
    // 60x24 is below the 80-col breakpoint => compact (single column).
    let text = render_dashboard(60, 24);

    // In compact layout, all three panels are stacked vertically.
    // The AGENTS panel should appear.
    assert!(
        text.contains("AGENTS"),
        "Compact dashboard should show AGENTS panel, got: {text:?}"
    );
    // The SYSTEM HEALTH panel should appear.
    assert!(
        text.contains("SYSTEM HEALTH") || text.contains("HEALTH"),
        "Compact dashboard should show health panel, got: {text:?}"
    );
}

#[test]
fn test_dashboard_standard_layout() {
    // 100x24 is >= 80 and < 120 => standard (two columns + health bar).
    let text = render_dashboard(100, 24);

    assert!(
        text.contains("AGENTS"),
        "Standard dashboard should show AGENTS panel, got: {text:?}"
    );
    assert!(
        text.contains("ACTIVE RUNS"),
        "Standard dashboard should show ACTIVE RUNS panel, got: {text:?}"
    );
}

#[test]
fn test_dashboard_wide_layout() {
    // 140x24 is >= 120 => wide (three columns).
    let text = render_dashboard(140, 24);

    assert!(
        text.contains("AGENTS"),
        "Wide dashboard should show AGENTS panel, got: {text:?}"
    );
    assert!(
        text.contains("ACTIVE RUNS"),
        "Wide dashboard should show ACTIVE RUNS panel, got: {text:?}"
    );
    // Wide layout adds a health sidebar.
    assert!(
        text.contains("HEALTH"),
        "Wide dashboard should show HEALTH sidebar, got: {text:?}"
    );
}

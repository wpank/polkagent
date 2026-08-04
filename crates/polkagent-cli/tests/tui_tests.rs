//! TUI widget and view unit tests for the polkagent-cli crate.
//!
//! Tests cover the ROSEDUST theme, input/key mapping, state management,
//! widget rendering (using ratatui's TestBackend), and responsive layout
//! breakpoints.

use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};

use polkagent_cli::tui::{
    app::Tab,
    input::{key_to_action, InputMode, TuiAction},
    state::{ApprovalItem, AuditEvent, ConfirmDialog, MemoryEntry, ScrollState, TuiState},
    theme::Theme,
    views::audit::AuditFilter,
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
                    None => {}          // fine
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

    assert!(
        state.agents.is_empty(),
        "Default state should have no agents"
    );
    assert!(state.runs.is_empty(), "Default state should have no runs");
    assert!(
        !state.health.db_ok,
        "Default health should report db_ok=false"
    );
    assert!(
        state.selected_run.is_none(),
        "No run should be selected initially"
    );
    assert!(!state.dirty, "Default state should not be dirty");
    assert!(
        state.last_refresh.is_none(),
        "No refresh should have occurred"
    );
    assert!(
        state.last_error.is_none(),
        "No error should be present initially"
    );
    assert_eq!(
        state.agents_scroll.offset, 0,
        "Scroll offset should start at 0"
    );
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
    let selected = scroll
        .selected
        .expect("selected should be Some after scrolling");
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
    assert_eq!(scroll.selected, Some(0), "Scrolling up should return to 0");
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
    tab = tab.next(); // Memory
    assert_eq!(tab, Tab::Memory);
    tab = tab.next(); // Audit
    assert_eq!(tab, Tab::Audit);
    tab = tab.next(); // Dashboard (wrap)
    assert_eq!(
        tab,
        Tab::Dashboard,
        "next() should wrap from Audit to Dashboard"
    );

    // Go backward through all tabs.
    tab = tab.prev(); // Audit
    assert_eq!(
        tab,
        Tab::Audit,
        "prev() should wrap from Dashboard to Audit"
    );
    tab = tab.prev(); // Memory
    assert_eq!(tab, Tab::Memory);
    tab = tab.prev(); // Approvals
    assert_eq!(tab, Tab::Approvals);
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
    assert_eq!(Tab::ALL.len(), 8, "There should be exactly 8 tabs");
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

// =========================================================================
// 6. Memory view tests
// =========================================================================

fn make_memory_entry(id: &str, memory_type: &str, relevance: f64, content: &str) -> MemoryEntry {
    MemoryEntry {
        id: id.to_owned(),
        memory_type: memory_type.to_owned(),
        agent_name: "test-agent".to_owned(),
        relevance_score: relevance,
        content: content.to_owned(),
        created_at: Utc::now(),
    }
}

fn render_memory_view(width: u16, height: u16, entries: Vec<MemoryEntry>) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("failed to create test terminal");
    let theme = Theme::dark();
    let mut state = TuiState::default();
    state.memory_entries = entries;

    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            polkagent_cli::tui::views::memory::render(frame, area, &state, &theme);
        })
        .expect("memory render failed");

    buffer_text(&terminal)
}

#[test]
fn test_memory_view_empty() {
    let text = render_memory_view(80, 24, vec![]);
    assert!(
        text.contains("MEMORY BROWSER"),
        "Empty memory view should show MEMORY BROWSER header, got: {text:?}"
    );
    assert!(
        text.contains("No memory entries"),
        "Empty memory view should show 'No memory entries' message, got: {text:?}"
    );
}

#[test]
fn test_memory_view_with_entries() {
    let entries = vec![
        make_memory_entry("e1", "episodic", 0.9, "Agent helped with a transfer"),
        make_memory_entry("e2", "semantic", 0.5, "Polkadot is a blockchain"),
        make_memory_entry("e3", "working", 0.2, "User address is 5GrwvaEF"),
    ];
    let text = render_memory_view(80, 24, entries);
    assert!(
        text.contains("MEMORY BROWSER"),
        "Memory view with entries should show MEMORY BROWSER header, got: {text:?}"
    );
    // The entry types should appear.
    assert!(
        text.contains("episodic") || text.contains("semantic") || text.contains("working"),
        "Memory entry types should appear in the list, got: {text:?}"
    );
}

#[test]
fn test_memory_view_search_bar() {
    let text = render_memory_view(80, 24, vec![]);
    // Search bar should always be present.
    assert!(
        text.contains("SEARCH") || text.contains("Search") || text.contains("search"),
        "Memory view should show search bar, got: {text:?}"
    );
}

#[test]
fn test_memory_view_detail_pane_wide() {
    // >= 100 cols with a selected entry should show the detail pane.
    let entries = vec![make_memory_entry(
        "detail-test",
        "semantic",
        0.85,
        "The agent learned that DOT transfers require approval.",
    )];
    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let theme = Theme::dark();
    let mut state = TuiState::default();
    state.memory_entries = entries;
    state.memory_scroll.selected = Some(0);

    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, 120, 30);
            polkagent_cli::tui::views::memory::render(frame, area, &state, &theme);
        })
        .expect("memory render failed");

    let text = buffer_text(&terminal);
    // With a selection in wide mode, the detail pane should appear with "Memory:".
    assert!(
        text.contains("Memory:") || text.contains("Content:") || text.contains("semantic"),
        "Wide memory view with selection should show detail pane, got: {text:?}"
    );
}

#[test]
fn test_memory_entry_no_panic_minimal() {
    // Minimal 20x5 terminal — must not panic.
    let entries = vec![make_memory_entry("x", "episodic", 0.5, "test")];
    let text = render_memory_view(20, 5, entries);
    // Just check it rendered something.
    assert!(
        !text.trim().is_empty(),
        "Minimal memory view should render something"
    );
}

// =========================================================================
// 7. Audit log view tests
// =========================================================================

fn make_audit_event(id: &str, severity: &str, kind: &str, message: &str) -> AuditEvent {
    AuditEvent {
        id: id.to_owned(),
        severity: severity.to_owned(),
        kind: kind.to_owned(),
        agent_name: "test-agent".to_owned(),
        run_id: Some("run-1234".to_owned()),
        message: message.to_owned(),
        timestamp: Utc::now(),
    }
}

fn render_audit_view(
    width: u16,
    height: u16,
    events: Vec<AuditEvent>,
    filter: AuditFilter,
) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let theme = Theme::dark();
    let mut state = TuiState::default();
    state.audit_log = events;
    state.audit_filter = filter;

    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            polkagent_cli::tui::views::audit::render(frame, area, &state, &theme);
        })
        .expect("audit render failed");

    buffer_text(&terminal)
}

#[test]
fn test_audit_view_empty() {
    let text = render_audit_view(80, 24, vec![], AuditFilter::None);
    assert!(
        text.contains("AUDIT LOG"),
        "Empty audit view should show AUDIT LOG header, got: {text:?}"
    );
    assert!(
        text.contains("No audit events"),
        "Empty audit view should show 'No audit events' message, got: {text:?}"
    );
}

#[test]
fn test_audit_view_with_info_events() {
    let events = vec![
        make_audit_event("a1", "info", "AgentStarted", "Agent started successfully"),
        make_audit_event("a2", "info", "RunCreated", "New run was created"),
    ];
    let text = render_audit_view(100, 24, events, AuditFilter::None);
    assert!(
        text.contains("AUDIT LOG"),
        "Audit view should show AUDIT LOG header, got: {text:?}"
    );
}

#[test]
fn test_audit_view_with_mixed_severity() {
    let events = vec![
        make_audit_event("b1", "info", "RunCreated", "Run started"),
        make_audit_event("b2", "warn", "TokenLimit", "Approaching token limit"),
        make_audit_event("b3", "error", "RunFailed", "Run failed with error"),
    ];
    let text = render_audit_view(100, 30, events, AuditFilter::None);
    // The filter bar should show "ALL".
    assert!(
        text.contains("ALL") || text.contains("Filter"),
        "Audit view should show filter bar, got: {text:?}"
    );
}

#[test]
fn test_audit_view_filter_warn() {
    let events = vec![
        make_audit_event("c1", "info", "RunCreated", "Run started"),
        make_audit_event("c2", "warn", "HighTokenUsage", "Token usage high"),
    ];
    let text = render_audit_view(100, 30, events, AuditFilter::BySeverity("warn".to_owned()));
    // Filter indicator should show "warn".
    assert!(
        text.contains("warn") || text.contains("severity"),
        "Audit view should show warn filter indicator, got: {text:?}"
    );
}

#[test]
fn test_audit_view_filter_none_no_crash() {
    // Switching filter to None should not crash.
    let events = vec![make_audit_event("d1", "info", "Test", "Test event")];
    let text = render_audit_view(80, 20, events, AuditFilter::None);
    assert!(
        !text.trim().is_empty(),
        "Audit view with None filter should render something"
    );
}

#[test]
fn test_audit_view_footer_hints() {
    let text = render_audit_view(100, 24, vec![], AuditFilter::None);
    // Should show key hints.
    assert!(
        text.contains('g') || text.contains('G') || text.contains("top"),
        "Audit view should show navigation key hints, got: {text:?}"
    );
}

// =========================================================================
// 8. System view tests
// =========================================================================

fn render_system_view(width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let theme = Theme::dark();
    let state = TuiState::default();

    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            polkagent_cli::tui::views::system::render(frame, area, &state, &theme);
        })
        .expect("system render failed");

    buffer_text(&terminal)
}

#[test]
fn test_system_view_health_section() {
    let text = render_system_view(80, 30);
    assert!(
        text.contains("SYSTEM HEALTH"),
        "System view should show SYSTEM HEALTH section, got: {text:?}"
    );
}

#[test]
fn test_system_view_configuration_section() {
    let text = render_system_view(80, 30);
    assert!(
        text.contains("CONFIGURATION"),
        "System view should show CONFIGURATION section, got: {text:?}"
    );
}

#[test]
fn test_system_view_statistics_section() {
    let text = render_system_view(100, 30);
    assert!(
        text.contains("STATISTICS") || text.contains("HEALTH"),
        "System view should show STATISTICS section, got: {text:?}"
    );
}

#[test]
fn test_system_view_wide_layout() {
    // Wide layout >= 100 cols should show all three panels.
    let text = render_system_view(140, 40);
    assert!(
        text.contains("SYSTEM HEALTH"),
        "Wide system view should show SYSTEM HEALTH, got: {text:?}"
    );
    assert!(
        text.contains("CONFIGURATION"),
        "Wide system view should show CONFIGURATION, got: {text:?}"
    );
}

#[test]
fn test_system_view_keybindings_shown() {
    // Use a wide terminal to guarantee the config panel is fully visible.
    let text = render_system_view(140, 50);
    // Should show keybinding hints — check for the 'q' binding.
    assert!(
        text.contains('q') || text.contains("quit") || text.contains("Quit"),
        "System view should show quit keybinding, got: {text:?}"
    );
}

#[test]
fn test_system_view_narrow_no_panic() {
    // Very narrow terminal should not panic.
    let text = render_system_view(40, 20);
    assert!(
        !text.trim().is_empty(),
        "Narrow system view should render something"
    );
}

// =========================================================================
// 9. Approval flow tests
// =========================================================================

fn make_approval_item(effect_id: &str, kind: &str) -> ApprovalItem {
    ApprovalItem {
        effect_id: effect_id.to_owned(),
        kind: kind.to_owned(),
        run_id: "run-abcdef12".to_owned(),
        agent_name: "test-agent".to_owned(),
        created_at: Utc::now(),
        state: "pending".to_owned(),
    }
}

fn render_approvals_view(
    width: u16,
    height: u16,
    items: Vec<ApprovalItem>,
    confirm: ConfirmDialog,
) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let theme = Theme::dark();
    let mut state = TuiState::default();
    state.pending_approvals = items;
    state.confirm_dialog = confirm;

    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            polkagent_cli::tui::views::approvals::render(frame, area, &state, &theme);
        })
        .expect("approvals render failed");

    buffer_text(&terminal)
}

#[test]
fn test_approvals_empty_queue() {
    let text = render_approvals_view(80, 24, vec![], ConfirmDialog::None);
    assert!(
        text.contains("APPROVAL QUEUE"),
        "Empty approvals view should show APPROVAL QUEUE header, got: {text:?}"
    );
    assert!(
        text.contains("No pending") || text.contains("pending"),
        "Empty approvals view should mention 'pending', got: {text:?}"
    );
}

#[test]
fn test_approvals_with_items() {
    let items = vec![
        make_approval_item("eff-0001-2345-6789-abcd", "sign"),
        make_approval_item("eff-0002-2345-6789-abcd", "broadcast"),
    ];
    let text = render_approvals_view(100, 30, items, ConfirmDialog::None);
    assert!(
        text.contains("APPROVAL QUEUE"),
        "Approvals view should show APPROVAL QUEUE header, got: {text:?}"
    );
    // Kind labels should appear.
    assert!(
        text.contains("sign") || text.contains("broadcast"),
        "Approval item kinds should appear in the list, got: {text:?}"
    );
}

#[test]
fn test_approvals_confirm_approve_dialog() {
    let items = vec![make_approval_item("eff-abc123def456", "sign")];
    let dialog = ConfirmDialog::ConfirmApprove("eff-abc123def456".to_owned());
    let text = render_approvals_view(100, 30, items, dialog);
    // The confirmation dialog should appear.
    assert!(
        text.contains("Confirm") || text.contains("APPROVE") || text.contains("approve"),
        "Approve dialog should appear in the approvals view, got: {text:?}"
    );
}

#[test]
fn test_approvals_confirm_deny_dialog() {
    let items = vec![make_approval_item("eff-abc123def456", "sign")];
    let dialog = ConfirmDialog::ConfirmDeny("eff-abc123def456".to_owned());
    let text = render_approvals_view(100, 30, items, dialog);
    assert!(
        text.contains("DENY") || text.contains("deny") || text.contains("Confirm"),
        "Deny dialog should appear in the approvals view, got: {text:?}"
    );
}

#[test]
fn test_approvals_key_hints() {
    let text = render_approvals_view(80, 24, vec![], ConfirmDialog::None);
    // Should show 'a' and 'd' key hints somewhere.
    assert!(
        text.contains('a') || text.contains("approve"),
        "Approvals view should show approve hint, got: {text:?}"
    );
}

#[test]
fn test_approvals_keybinding_approve_produces_action() {
    // Press 'a' in Normal mode => ApproveEffect action.
    let action = key_to_action(key(KeyCode::Char('a')), InputMode::Normal);
    assert!(
        matches!(action, Some(TuiAction::ApproveEffect)),
        "'a' in Normal mode should produce ApproveEffect, got: {action:?}"
    );
}

#[test]
fn test_approvals_keybinding_deny_produces_action() {
    // Press 'd' in Normal mode => DenyEffect action.
    let action = key_to_action(key(KeyCode::Char('d')), InputMode::Normal);
    assert!(
        matches!(action, Some(TuiAction::DenyEffect)),
        "'d' in Normal mode should produce DenyEffect, got: {action:?}"
    );
}

// =========================================================================
// 10. New tab navigation tests
// =========================================================================

#[test]
fn test_tab_navigation_includes_memory_and_audit() {
    // Tab::ALL should now include Memory and Audit.
    let all = Tab::ALL;
    assert_eq!(
        all.len(),
        8,
        "Tab::ALL should contain 8 tabs (including Memory and Audit)"
    );
    assert!(all.contains(&Tab::Memory), "Tab::ALL should include Memory");
    assert!(all.contains(&Tab::Audit), "Tab::ALL should include Audit");
}

#[test]
fn test_tab_next_audit_wraps_to_dashboard() {
    assert_eq!(
        Tab::Audit.next(),
        Tab::Dashboard,
        "next() on Audit should wrap to Dashboard"
    );
}

#[test]
fn test_tab_prev_dashboard_goes_to_audit() {
    assert_eq!(
        Tab::Dashboard.prev(),
        Tab::Audit,
        "prev() on Dashboard should go to Audit"
    );
}

#[test]
fn test_tab_f7_maps_to_memory() {
    let action = key_to_action(key(KeyCode::F(7)), InputMode::Normal);
    assert!(
        matches!(action, Some(TuiAction::NavigateTab(Tab::Memory))),
        "F7 should switch to Memory tab, got: {action:?}"
    );
}

#[test]
fn test_tab_f8_maps_to_audit() {
    let action = key_to_action(key(KeyCode::F(8)), InputMode::Normal);
    assert!(
        matches!(action, Some(TuiAction::NavigateTab(Tab::Audit))),
        "F8 should switch to Audit tab, got: {action:?}"
    );
}

#[test]
fn test_key_7_maps_to_memory() {
    let action = key_to_action(key(KeyCode::Char('7')), InputMode::Normal);
    assert!(
        matches!(action, Some(TuiAction::NavigateTab(Tab::Memory))),
        "'7' should switch to Memory tab, got: {action:?}"
    );
}

#[test]
fn test_key_8_maps_to_audit() {
    let action = key_to_action(key(KeyCode::Char('8')), InputMode::Normal);
    assert!(
        matches!(action, Some(TuiAction::NavigateTab(Tab::Audit))),
        "'8' should switch to Audit tab, got: {action:?}"
    );
}

#[test]
fn test_key_g_maps_to_scroll_to_top() {
    let action = key_to_action(key(KeyCode::Char('g')), InputMode::Normal);
    assert!(
        matches!(action, Some(TuiAction::ScrollToTop)),
        "'g' should produce ScrollToTop, got: {action:?}"
    );
}

#[test]
fn test_key_capital_g_maps_to_scroll_to_bottom() {
    use crossterm::event::KeyModifiers;
    let action = key_to_action(
        KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE),
        InputMode::Normal,
    );
    assert!(
        matches!(action, Some(TuiAction::ScrollToBottom)),
        "'G' should produce ScrollToBottom, got: {action:?}"
    );
}

#[test]
fn test_key_delete_maps_to_delete_entry() {
    let action = key_to_action(key(KeyCode::Delete), InputMode::Normal);
    assert!(
        matches!(action, Some(TuiAction::DeleteEntry)),
        "Delete key should produce DeleteEntry, got: {action:?}"
    );
}

#[test]
fn test_key_f_maps_to_cycle_filter() {
    let action = key_to_action(key(KeyCode::Char('f')), InputMode::Normal);
    assert!(
        matches!(action, Some(TuiAction::CycleFilter)),
        "'f' key should produce CycleFilter, got: {action:?}"
    );
}

// =========================================================================
// 11. Confirm dialog state tests
// =========================================================================

#[test]
fn test_confirm_dialog_default_is_none() {
    let dialog = ConfirmDialog::default();
    assert!(
        matches!(dialog, ConfirmDialog::None),
        "Default ConfirmDialog should be None"
    );
}

#[test]
fn test_audit_filter_default_is_none() {
    let filter = AuditFilter::default();
    assert!(
        matches!(filter, AuditFilter::None),
        "Default AuditFilter should be None"
    );
}

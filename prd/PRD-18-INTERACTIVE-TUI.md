# PRD-18 — Interactive TUI: Prompt Bar, Run Control, and Agent Management

**Status:** draft
**Owner:** unassigned
**Last updated:** 2026-08-03
**Depends on:** PRD-03 (execution model), PRD-13 (UX surfaces), PRD-16 (TUI diagnostics)
**Implementation status:** design requirements only

---

## 1. Problem

The ROSEDUST TUI (`polkagent tui`) is a passive monitoring dashboard. Users can
view agents, scroll runs, approve/deny effects, and browse memory — but cannot
**do** anything. Creating an agent, starting a run, or submitting a prompt
requires leaving the TUI and switching to a separate `polkagent run -a ... -p ...`
terminal. This context-switch breaks flow and makes the TUI feel like a read-only
afterthought rather than the primary operating surface.

The TUI should be the place where operators live. They should be able to start
work, watch it happen, intervene when needed, and start more work — all without
leaving the interface.

### 1.1 Current state

| Capability | TUI today | Target |
|---|---|---|
| View agents, runs, events | Yes | Yes |
| Approve/deny effects | Yes (F6) | Yes |
| Delete memory entries | Yes (F7) | Yes |
| Type text (search, prompts) | No (Insert mode stub) | Yes |
| Create an agent | No | Yes |
| Start a run with a prompt | No | Yes |
| Cancel a running run | No | Yes |
| Quick-command palette | No | Yes |
| Switch model/provider on the fly | No | Yes |
| View streaming run output | No | Yes |

### 1.2 Design goals

1. **Zero learning curve.** A user who has never seen polkagent should be able to
   press `:` and discover what they can do. No manual required.
2. **Keyboard-first, fast.** Every action reachable in 1-3 keystrokes. Mouse
   support is a nice-to-have, not a requirement.
3. **Progressive disclosure.** The prompt bar is simple by default. Power-user
   flags are available but hidden until needed.
4. **Non-blocking.** Starting a run returns instantly. Output streams into the
   Timeline. The user can start another run while the first is still going.
5. **Consistent with CLI.** Every TUI action maps to an equivalent CLI command.
   The TUI is a wrapper over the same `AppService` methods.

---

## 2. Architecture overview

```
┌──────────────────────────────────────────────────────────────────┐
│  ROSEDUST TUI                                                    │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │  Tab views (F1-F8) — existing read-only views            │    │
│  │  + NEW: streaming output pane in Timeline/RunDetail      │    │
│  └──────────────────────────────────────────────────────────┘    │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │  Prompt Bar (bottom) — always available, `:` to focus    │    │
│  │  > ask dev-helper "What referendums are active?"         │    │
│  └──────────────────────────────────────────────────────────┘    │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │  Status Bar — key hints, mode indicator, clock           │    │
│  └──────────────────────────────────────────────────────────┘    │
│                                                                  │
│         │                                                        │
│         ▼                                                        │
│  ┌─────────────┐    ┌─────────────┐    ┌──────────────────┐     │
│  │ CommandParser│───>│ AppService  │───>│ RunManager       │     │
│  │ (TUI-local) │    │ (shared)    │    │ + Orchestrator   │     │
│  └─────────────┘    └─────────────┘    └──────────────────┘     │
│                            │                                     │
│                            ▼                                     │
│                     ┌─────────────┐                              │
│                     │ EventBus    │──> Timeline view updates     │
│                     └─────────────┘                              │
└──────────────────────────────────────────────────────────────────┘
```

### 2.1 Key principle: TUI wraps AppService

The TUI does not implement business logic. It calls the same `AppService`
methods that the REST API and CLI use:

- `app_service.create_agent(spec)` → agent creation
- `app_service.start_run(agent_id, prompt)` → run creation
- `app_service.cancel_run(run_id)` → cancellation
- `app_service.subscribe_events()` → live streaming

This means every TUI action is testable without a terminal, and the TUI never
diverges from the CLI in behavior.

---

## 3. Feature: Prompt Bar

The prompt bar is the primary interaction surface. It sits between the tab views
and the status bar, hidden by default (1 row) and expanding when focused.

### 3.1 Activation

| Key | Action |
|---|---|
| `:` | Open prompt bar in **command mode** |
| `/` | Open prompt bar in **search mode** (scoped to active tab) |
| `Enter` on agent list | Open prompt bar pre-filled with `ask <agent-name> ""` |
| `Esc` | Close prompt bar, return to Normal mode |

### 3.2 Command grammar

```
COMMAND := VERB [TARGET] [ARGS...]

# --- Run commands ---
ask   <agent> "<prompt>"           # Start a run (short form)
run   <agent> "<prompt>"           # Start a run (alias)
run   <agent> "<prompt>" --model claude-opus-4-6
run   <agent> "<prompt>" --provider anthropic --timeout 300

# --- Agent commands ---
agent create <name>                # Create agent with defaults
agent create <name> --model anthropic/claude-opus-4-6
agent create <name> --model ollama/llama3.1
agent list                         # Refresh and show agent list
agent delete <name>                # Archive agent (confirm dialog)

# --- Run control ---
cancel <run-id>                    # Cancel running/queued run
cancel all                         # Cancel all active runs (confirm)
retry  <run-id>                    # Re-run with same prompt

# --- Navigation shortcuts ---
goto agents | runs | timeline | approvals | memory | audit
goto run <run-id>                  # Jump to run detail

# --- Quick actions ---
approve <effect-id>                # Approve without switching tabs
deny    <effect-id>                # Deny without switching tabs
approve all                        # Approve all pending (confirm)

# --- System ---
refresh                            # Force refresh all data
provider list                      # Show configured providers
model list                         # Show available models
help                               # Show command reference
quit                               # Exit TUI
```

### 3.3 Short forms

For the most common action — asking an agent a question — the grammar is
deliberately minimal:

```
:ask dev-helper "What's the DOT price?"
```

If only one agent exists, the agent name is optional:

```
:ask "What's the DOT price?"
```

If the user just types a quoted string, it's treated as `ask <default-agent>`:

```
:"What's the DOT price?"
```

### 3.4 Autocomplete

The prompt bar supports tab-completion:

| Context | Completes from |
|---|---|
| First word | Command verbs: `ask`, `run`, `agent`, `cancel`, `goto`, etc. |
| After `ask`/`run` | Agent names from registry |
| After `--model` | Model slugs from BuiltInModelCatalog |
| After `--provider` | Provider names from config |
| After `goto` | Tab names |
| After `cancel`/`retry` | Active run IDs (prefix match) |
| After `approve`/`deny` | Pending effect IDs |

Tab cycles through matches. Shift+Tab cycles backward. The completion menu
appears inline below the prompt bar (max 5 items, scrollable).

### 3.5 Command history

- `Up`/`Down` arrows cycle through previous commands (per-session)
- History persists in `~/.polkagent/tui_history` (last 500 entries)
- `Ctrl+R` searches history (reverse incremental search)

### 3.6 Visual design

```
┌─ PROMPT ──────────────────────────────────────────────────────┐
│ > ask dev-helper "What referendums are active on Polkadot?"   │
└───────────────────────────────────────────────────────────────┘
 F1-F8:tabs  q:quit  Enter:submit  Tab:complete  Esc:close
```

- Border color: `theme.border_dream` (violet) when focused
- Input text: `theme.text_primary`
- Placeholder text: `theme.text_ghost` showing `"Type a command or :help"`
- Cursor: block cursor, blinking
- Error state: border flashes `theme.danger` with error message for 3s

---

## 4. Feature: Text input engine

The current `InputMode::Insert` is a stub that ignores all character input.
This must be replaced with a proper text input engine.

### 4.1 Requirements

| Feature | Description |
|---|---|
| Character insert | Any printable character appends at cursor position |
| Cursor movement | Left/Right arrows, Home/End, Ctrl+A/Ctrl+E |
| Word movement | Ctrl+Left/Right to jump by word |
| Deletion | Backspace, Delete, Ctrl+W (word), Ctrl+U (to start), Ctrl+K (to end) |
| Selection | Shift+arrows for selection (cut/copy) — stretch goal |
| Multi-byte | Full Unicode support (emoji, CJK, combining marks) |
| Paste | Ctrl+V or bracketed paste from terminal |
| Wrapping | Prompt bar grows to 3 lines max, then scrolls horizontally |

### 4.2 Implementation: `TextInput` struct

```rust
pub struct TextInput {
    /// The current text content.
    content: String,
    /// Byte offset of cursor within `content`.
    cursor: usize,
    /// Horizontal scroll offset (for long inputs).
    scroll: usize,
    /// Command history ring buffer.
    history: VecDeque<String>,
    /// Current history navigation index (None = editing new input).
    history_index: Option<usize>,
    /// Placeholder text shown when empty.
    placeholder: &'static str,
}
```

This struct is owned by `App` (not `TuiState`) because it manages transient
input state that should not be serialized or refreshed.

### 4.3 Key mapping in Insert/Command mode

Replace the current no-op handler in `input.rs`:

```rust
InputMode::Insert | InputMode::Command => {
    match key.code {
        KeyCode::Esc => Some(TuiAction::ExitInput),
        KeyCode::Enter => Some(TuiAction::SubmitInput),
        KeyCode::Backspace => Some(TuiAction::InputBackspace),
        KeyCode::Delete => Some(TuiAction::InputDelete),
        KeyCode::Left => Some(TuiAction::InputCursorLeft),
        KeyCode::Right => Some(TuiAction::InputCursorRight),
        KeyCode::Home => Some(TuiAction::InputCursorHome),
        KeyCode::End => Some(TuiAction::InputCursorEnd),
        KeyCode::Up => Some(TuiAction::InputHistoryPrev),
        KeyCode::Down => Some(TuiAction::InputHistoryNext),
        KeyCode::Tab => Some(TuiAction::InputComplete),
        KeyCode::BackTab => Some(TuiAction::InputCompleteBack),
        KeyCode::Char(c) => Some(TuiAction::InputChar(c)),
        _ => None,
    }
}
```

---

## 5. Feature: Run creation and streaming output

### 5.1 Starting a run

When the user submits `ask <agent> "<prompt>"`:

1. **Parse and validate.** Command parser extracts agent name and prompt.
2. **Resolve agent.** Look up agent in `AppService` registry. Show error if not found.
3. **Start run.** Call `app_service.start_run(agent_id, prompt)`. Returns `RunId` immediately.
4. **Subscribe to events.** `app_service.subscribe_events()` filtered to `run_id`.
5. **Navigate to run.** Auto-switch to Timeline (F5) or RunDetail view, focused on the new run.
6. **Show toast.** Status bar briefly shows: `"Run 019fc... started for dev-helper"` in success color.

### 5.2 Streaming output in Timeline/RunDetail

The Timeline view (F5) and RunDetail sub-view gain a **live output pane**:

```
┌─ RUN 019fc239 ─ dev-helper ─ running ─────────────────────┐
│                                                             │
│  [19:28:35] RunStarted                                      │
│  [19:28:36] TurnStarted  sequence=1                         │
│  [19:28:37] ░░ Querying active referendums on Polkadot...   │
│  [19:28:39] ░░ Found 12 active referendums                  │
│  [19:28:40] ░░ Referendum #1234: "Treasury Proposal for..." │
│  [19:28:41] TurnCompleted  tokens_in=1204 tokens_out=847    │
│  [19:28:41] EffectProposed  transfer 0.5 DOT                │
│  [19:28:41] ⏳ Waiting for approval...                       │
│                                                             │
│  ┌─ PENDING APPROVAL ─────────────────────────────────┐     │
│  │ Transfer 0.5 DOT to 5GrwvaEF...                    │     │
│  │ Press 'a' to approve, 'd' to deny                  │     │
│  └────────────────────────────────────────────────────┘     │
│                                                             │
└── j/k:scroll  a:approve  d:deny  c:cancel  Esc:back ───────┘
```

**Output rendering rules:**
- System events (RunStarted, TurnCompleted) render as dim, structured lines
- Model output renders as primary text with `░░` prefix
- Pending approvals render inline as action cards
- Auto-scroll to bottom unless user has scrolled up manually
- `Ctrl+L` clears scroll position and jumps to latest

### 5.3 Multiple concurrent runs

Users can start multiple runs. The Runs tab (F3) shows all of them. The
Timeline (F5) shows events from the currently selected run. Users switch
between runs with `j/k` on the Runs tab, then `Enter` to view details.

A notification badge appears on the tab header when a background run completes
or needs approval:

```
F5 Timeline    F6 Approvals (2)    F7 Memory
```

---

## 6. Feature: Agent creation dialog

### 6.1 Quick create

```
:agent create my-researcher
```

Creates an agent with defaults:
- Model: `anthropic/claude-sonnet-4-6`
- No tools, no system prompt
- Autonomy: supervised

### 6.2 Guided create (modal)

```
:agent new
```

Opens a multi-step modal dialog:

```
┌─ CREATE AGENT ─────────────────────────────────────────┐
│                                                         │
│  Name:     [my-researcher____________]                  │
│  Model:    [anthropic/claude-sonnet-4-6  ▾]             │
│  Prompt:   [You are a Polkadot governance researcher.]  │
│                                                         │
│  ── Advanced (Tab to expand) ──                         │
│                                                         │
│        [Create]     [Cancel]                            │
│                                                         │
└─ Tab:next field  Enter:submit  Esc:cancel ──────────────┘
```

**Fields:**

| Field | Type | Default | Required |
|---|---|---|---|
| Name | text input | — | yes |
| Model | dropdown (tab-complete) | `anthropic/claude-sonnet-4-6` | yes |
| System prompt | multi-line text | empty | no |
| Max turns | number | 25 | no |
| Timeout (seconds) | number | 0 (none) | no |
| Max tokens/turn | number | 4096 | no |
| Description | text | empty | no |

**Tab** moves between fields. **Enter** on `[Create]` submits. **Esc** cancels.

The dropdown for Model shows all models from `BuiltInModelCatalog` filtered by
which providers have valid API keys configured.

---

## 7. Feature: Quick actions

### 7.1 Run control from any tab

| Key | Action | Context |
|---|---|---|
| `c` | Cancel selected run | Runs tab with run selected |
| `Ctrl+C` (in run detail) | Cancel the viewed run | RunDetail view |
| `:cancel <id>` | Cancel by ID | Prompt bar |
| `:cancel all` | Cancel all active runs | Prompt bar (shows confirm) |

### 7.2 Inline approval from Timeline

When viewing a run's timeline and an effect is pending, the user can approve or
deny directly from the timeline view (without switching to F6 Approvals):

- `a` approves the highlighted pending effect
- `d` denies it
- Both show the standard two-stage confirmation dialog

### 7.3 Retry

```
:retry 019fc239
```

Looks up the original run's prompt and agent, creates a new run with the same
parameters. Navigates to the new run's timeline.

---

## 8. Feature: Search (fixing the existing stub)

The current `/` search on the Memory tab is broken because `InputMode::Insert`
ignores all character input. Fixing the text input engine (section 4) also
fixes search.

### 8.1 Tab-scoped search

`/` activates search scoped to the current tab:

| Tab | Search scope |
|---|---|
| F2 Agents | Filter agents by name |
| F3 Runs | Filter runs by agent name, state, or run ID prefix |
| F5 Timeline | Filter events by kind or content |
| F6 Approvals | Filter pending effects by type |
| F7 Memory | Full-text search of memory entries (existing) |
| F8 Audit | Filter audit events by kind |

The search bar appears at the top of the active view. Results filter in
real-time as the user types (debounced 150ms).

---

## 9. Implementation plan — actionable checklist

All paths are relative to `crates/polkagent-cli/src/`.

---

### Phase 1: Text input engine (foundation)

**Goal:** Make typing work everywhere — search, prompt bar, dialogs.

#### P1-1. Create `tui/text_input.rs` (new file)

```rust
// crates/polkagent-cli/src/tui/text_input.rs
use std::collections::VecDeque;
use unicode_segmentation::UnicodeSegmentation;

pub struct TextInput {
    content: String,
    /// Cursor position in **grapheme cluster** index (not bytes).
    cursor: usize,
    /// Horizontal scroll offset in grapheme clusters.
    scroll: usize,
    /// Saved content when navigating history.
    draft: String,
    history: VecDeque<String>,
    history_index: Option<usize>,
    placeholder: &'static str,
    /// Maximum history entries.
    max_history: usize,
}

impl TextInput {
    pub fn new(placeholder: &'static str) -> Self { /* ... */ }
    pub fn content(&self) -> &str { &self.content }
    pub fn cursor(&self) -> usize { self.cursor }
    pub fn is_empty(&self) -> bool { self.content.is_empty() }
    pub fn placeholder(&self) -> &str { self.placeholder }

    // ── Editing ─────────────────────────────────────────────
    pub fn insert_char(&mut self, c: char) { /* insert at cursor, advance cursor */ }
    pub fn backspace(&mut self) { /* delete grapheme before cursor */ }
    pub fn delete(&mut self) { /* delete grapheme at cursor */ }
    pub fn delete_word_back(&mut self) { /* Ctrl+W: delete back to word boundary */ }
    pub fn clear_to_start(&mut self) { /* Ctrl+U: delete from start to cursor */ }
    pub fn clear_to_end(&mut self) { /* Ctrl+K: delete from cursor to end */ }
    pub fn clear(&mut self) { /* reset content + cursor */ }

    // ── Cursor movement ─────────────────────────────────────
    pub fn move_left(&mut self) { /* clamp at 0 */ }
    pub fn move_right(&mut self) { /* clamp at grapheme count */ }
    pub fn move_word_left(&mut self) { /* jump to previous word boundary */ }
    pub fn move_word_right(&mut self) { /* jump to next word boundary */ }
    pub fn move_home(&mut self) { self.cursor = 0; }
    pub fn move_end(&mut self) { self.cursor = self.grapheme_count(); }

    // ── History ─────────────────────────────────────────────
    pub fn push_history(&mut self) { /* push content to history, clear */ }
    pub fn history_prev(&mut self) { /* save draft if first press, move index back */ }
    pub fn history_next(&mut self) { /* move index forward, restore draft at end */ }

    // ── Rendering helpers ───────────────────────────────────
    /// Visible slice for rendering, respecting `scroll` and `width`.
    pub fn visible_content(&self, width: usize) -> &str { /* ... */ }
    /// Cursor position relative to visible content.
    pub fn visible_cursor(&self, width: usize) -> usize { /* ... */ }

    // ── Internal ────────────────────────────────────────────
    fn grapheme_count(&self) -> usize {
        self.content.graphemes(true).count()
    }
    fn byte_offset_of_grapheme(&self, n: usize) -> usize {
        self.content.grapheme_indices(true).nth(n).map(|(i, _)| i).unwrap_or(self.content.len())
    }
}
```

**Dependencies to add** to `crates/polkagent-cli/Cargo.toml`:
```toml
unicode-segmentation = "1"
```

**Tests** (in `text_input.rs`):
- [ ] `insert_char` appends at cursor, advances cursor by 1
- [ ] `insert_char` in middle splits content correctly
- [ ] `backspace` at position 0 is no-op
- [ ] `backspace` removes one grapheme cluster (test with multi-byte: `"café"`)
- [ ] `delete_word_back` deletes `"hello world|"` → `"hello |"`
- [ ] `clear_to_start` with cursor at 5 clears first 5 graphemes
- [ ] `move_word_left` on `"foo bar baz"` with cursor at 11 → 8 → 4 → 0
- [ ] `history_prev`/`next` cycle through entries, preserving draft
- [ ] `visible_content` with width=10 on 20-char string returns correct slice

#### P1-2. Add `TuiAction` variants to `tui/input.rs`

**Location:** `tui/input.rs:37-91` — add to the `TuiAction` enum:

```rust
// -- Text input (phase 1) ------------------------------------------------
/// Insert a character at the cursor position.
InputChar(char),
/// Delete the grapheme before the cursor (Backspace).
InputBackspace,
/// Delete the grapheme at the cursor (Delete key).
InputDelete,
/// Move cursor left by one grapheme.
InputCursorLeft,
/// Move cursor right by one grapheme.
InputCursorRight,
/// Move cursor to start of input (Home / Ctrl+A).
InputCursorHome,
/// Move cursor to end of input (End / Ctrl+E).
InputCursorEnd,
/// Move cursor left by one word (Ctrl+Left).
InputWordLeft,
/// Move cursor right by one word (Ctrl+Right).
InputWordRight,
/// Delete word before cursor (Ctrl+W).
InputDeleteWord,
/// Clear from cursor to start (Ctrl+U).
InputClearToStart,
/// Clear from cursor to end (Ctrl+K).
InputClearToEnd,
/// Submit the current input (Enter).
SubmitInput,
/// Close input mode (Esc).
ExitInput,
/// Navigate to previous history entry (Up arrow).
InputHistoryPrev,
/// Navigate to next history entry (Down arrow).
InputHistoryNext,
/// Trigger tab-completion (Tab).
InputComplete,
/// Trigger reverse tab-completion (Shift+Tab).
InputCompleteBack,
/// Open the command prompt bar (`:` key in Normal mode).
OpenPrompt,
```

#### P1-3. Wire Insert/Command mode key handling in `tui/input.rs`

**Location:** `tui/input.rs:113-119` — replace the existing stub:

```rust
// CURRENT (broken stub):
InputMode::Insert | InputMode::Command => {
    match key.code {
        KeyCode::Esc => Some(TuiAction::Back),
        _ => None,  // ← ALL input silently ignored
    }
}
```

**Replace with:**

```rust
InputMode::Insert | InputMode::Command => {
    match key.code {
        KeyCode::Esc => Some(TuiAction::ExitInput),
        KeyCode::Enter => Some(TuiAction::SubmitInput),
        KeyCode::Backspace => Some(TuiAction::InputBackspace),
        KeyCode::Delete => Some(TuiAction::InputDelete),
        KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::InputWordLeft)
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::InputWordRight)
        }
        KeyCode::Left => Some(TuiAction::InputCursorLeft),
        KeyCode::Right => Some(TuiAction::InputCursorRight),
        KeyCode::Home => Some(TuiAction::InputCursorHome),
        KeyCode::End => Some(TuiAction::InputCursorEnd),
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::InputCursorHome)
        }
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::InputCursorEnd)
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::InputDeleteWord)
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::InputClearToStart)
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::InputClearToEnd)
        }
        KeyCode::Up => Some(TuiAction::InputHistoryPrev),
        KeyCode::Down => Some(TuiAction::InputHistoryNext),
        KeyCode::Tab => Some(TuiAction::InputComplete),
        KeyCode::BackTab => Some(TuiAction::InputCompleteBack),
        KeyCode::Char(c) => Some(TuiAction::InputChar(c)),
        _ => None,
    }
}
```

Also add to `normal_mode_key()` at `tui/input.rs:189` (before the `_ => None`):
```rust
KeyCode::Char(':') => Some(TuiAction::OpenPrompt),
```

#### P1-4. Add `TextInput` field to `App` struct and wire action handlers

**Location:** `tui/app.rs:171-193` — add fields to `pub struct App`:

```rust
pub struct App {
    // ... existing fields ...

    // -- Text input (phase 1) ------------------------------------------------
    /// Search input (used for / search on any tab).
    pub search_input: TextInput,
    /// Command prompt input (used for : command bar).
    pub prompt_input: TextInput,
    /// Whether the prompt bar is visible.
    pub prompt_visible: bool,
}
```

**Location:** `tui/app.rs:197-212` — update `App::new()`:
```rust
pub fn new(theme: Theme, pool: SqlitePool) -> Self {
    Self {
        // ... existing fields ...
        search_input: TextInput::new("Search..."),
        prompt_input: TextInput::new("Type a command or :help"),
        prompt_visible: false,
    }
}
```

**Location:** `tui/app.rs:273` — add handlers in `apply_action()`:

```rust
TuiAction::OpenPrompt => {
    self.prompt_visible = true;
    self.input_mode = InputMode::Command;
    self.prompt_input.clear();
}
TuiAction::ExitInput => {
    if self.input_mode == InputMode::Command {
        self.prompt_visible = false;
    }
    self.input_mode = InputMode::Normal;
}
TuiAction::InputChar(c) => {
    self.active_text_input_mut().insert_char(c);
    self.tui_state.mark_dirty();
    if self.input_mode == InputMode::Insert {
        // Live search: update memory_search_query
        self.tui_state.memory_search_query = self.search_input.content().to_string();
    }
}
TuiAction::InputBackspace => {
    self.active_text_input_mut().backspace();
    self.tui_state.mark_dirty();
    if self.input_mode == InputMode::Insert {
        self.tui_state.memory_search_query = self.search_input.content().to_string();
    }
}
// ... delegate other Input* actions to active_text_input_mut() ...
TuiAction::SubmitInput => {
    match self.input_mode {
        InputMode::Insert => { /* search already live, just exit */ }
        InputMode::Command => {
            let cmd = self.prompt_input.content().to_string();
            self.prompt_input.push_history();
            self.prompt_visible = false;
            self.input_mode = InputMode::Normal;
            self.execute_command(&cmd);
        }
        _ => {}
    }
}
```

Add helper method:
```rust
fn active_text_input_mut(&mut self) -> &mut TextInput {
    match self.input_mode {
        InputMode::Insert => &mut self.search_input,
        InputMode::Command => &mut self.prompt_input,
        InputMode::Normal => &mut self.search_input, // unreachable in practice
    }
}
```

#### P1-5. Fix memory search to use `TextInput`

**Location:** `tui/views/memory.rs:65-95` — update `render_search_bar()`:

Replace reading from `state.memory_search_query` with reading from `app.search_input`:
```rust
fn render_search_bar(frame: &mut Frame, area: Rect, input: &TextInput, theme: &Theme) {
    let query = input.content();
    // ... existing render logic, but use input.visible_content(area.width) ...
    // Show cursor: frame.set_cursor_position(area.x + 2 + input.visible_cursor(w), area.y + 1)
}
```

**Location:** `tui/app.rs` — update the `TuiAction::Search` handler (currently at ~line 540):

```rust
TuiAction::Search => {
    self.input_mode = InputMode::Insert;
    self.search_input.clear();
    // Navigate to Memory tab if not already there
    if self.active_tab == Tab::Memory {
        // stay
    }
    // For other tabs, set a search_filter field on TuiState
}
```

#### P1-6. Update main layout in `App::run()` to include prompt bar

**Location:** `tui/app.rs:217` — in the render closure, update layout:

Currently the layout is:
```
[header: Length(2)] [content: Min(4)] [status: Length(1)]
```

Change to:
```
[header: Length(2)] [content: Min(4)] [prompt: Length(if visible 3 else 0)] [status: Length(1)]
```

---

### Phase 2: Prompt bar and command parser

**Goal:** `:` opens a command bar. Users can type and submit commands.

#### P2-1. Create `tui/widgets/prompt_bar.rs` (new file)

Stateless render function following existing widget patterns:

```rust
pub fn render_prompt_bar(
    frame: &mut Frame,
    area: Rect,
    input: &TextInput,
    theme: &Theme,
    has_error: bool,
) {
    let border_color = if has_error { theme.danger } else { theme.border_dream };
    let block = Block::bordered()
        .title(" PROMPT ")
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Render "> " prefix + visible content
    let prefix = "> ";
    let available_width = inner.width.saturating_sub(prefix.len() as u16) as usize;
    let visible = input.visible_content(available_width);
    let text = format!("{prefix}{visible}");
    // ... render Paragraph with text, set cursor position ...
}
```

Register in `tui/widgets/mod.rs`:
```rust
pub mod prompt_bar;
```

#### P2-2. Create `tui/command.rs` (new file)

```rust
/// Parsed TUI command.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// :ask <agent> "<prompt>" [--model M] [--provider P] [--timeout T]
    Ask {
        agent: String,
        prompt: String,
        model: Option<String>,
        provider: Option<String>,
        timeout: Option<u64>,
    },
    /// :agent create <name> [--model M]
    AgentCreate { name: String, model: Option<String> },
    /// :agent list
    AgentList,
    /// :agent delete <name>
    AgentDelete { name: String },
    /// :cancel <run-id> | :cancel all
    Cancel { target: CancelTarget },
    /// :retry <run-id>
    Retry { run_id: String },
    /// :goto <tab> | :goto run <run-id>
    Goto { target: GotoTarget },
    /// :approve <effect-id> | :approve all
    Approve { target: String },
    /// :deny <effect-id>
    Deny { effect_id: String },
    /// :refresh
    Refresh,
    /// :help
    Help,
    /// :quit
    Quit,
    /// :provider list
    ProviderList,
    /// :model list
    ModelList,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CancelTarget { RunId(String), All }

#[derive(Debug, Clone, PartialEq)]
pub enum GotoTarget { Tab(String), Run(String) }

/// Parse a raw command string into a `Command`.
/// Returns `Err(message)` for malformed input.
pub fn parse_command(input: &str) -> Result<Command, String> {
    let input = input.trim();
    let mut tokens = Tokenizer::new(input);
    let verb = tokens.next().ok_or("empty command")?;

    match verb.to_lowercase().as_str() {
        "ask" | "run" => {
            let agent = tokens.next().ok_or("missing agent name")?;
            let prompt = tokens.next_quoted().ok_or("missing prompt (wrap in quotes)")?;
            let model = tokens.flag("--model");
            let provider = tokens.flag("--provider");
            let timeout = tokens.flag("--timeout").and_then(|v| v.parse().ok());
            Ok(Command::Ask { agent, prompt, model, provider, timeout })
        }
        "agent" => { /* parse subcommand */ }
        "cancel" => { /* parse target */ }
        "goto" => { /* parse tab name or "run <id>" */ }
        "approve" => { /* parse effect-id or "all" */ }
        "deny" => { /* parse effect-id */ }
        "retry" => { /* parse run-id */ }
        "refresh" => Ok(Command::Refresh),
        "help" => Ok(Command::Help),
        "quit" | "q" => Ok(Command::Quit),
        "provider" => { /* parse "list" */ }
        "model" => { /* parse "list" */ }
        _ => Err(format!("unknown command: {verb}")),
    }
}

/// Simple tokenizer that handles quoted strings.
struct Tokenizer { /* splits on whitespace, handles "..." and '...' */ }
```

**Tests:**
- [ ] `parse_command("ask dev-helper \"hello\"")` → `Command::Ask { agent: "dev-helper", prompt: "hello", .. }`
- [ ] `parse_command("agent create my-bot")` → `Command::AgentCreate { name: "my-bot", .. }`
- [ ] `parse_command("agent create my-bot --model ollama/llama3.1")` → model is Some
- [ ] `parse_command("goto runs")` → `Command::Goto { target: Tab("runs") }`
- [ ] `parse_command("cancel all")` → `Command::Cancel { target: All }`
- [ ] `parse_command("cancel 019fc239")` → `Command::Cancel { target: RunId("019fc239") }`
- [ ] `parse_command("")` → Err
- [ ] `parse_command("xyzzy")` → Err("unknown command: xyzzy")
- [ ] Quoted string with spaces: `"ask bot \"hello world\""` → prompt is `"hello world"`

#### P2-3. Create `tui/complete.rs` (new file)

```rust
pub struct Completer {
    candidates: Vec<String>,
    index: usize,
    prefix: String,
}

impl Completer {
    pub fn new() -> Self { /* ... */ }

    /// Set candidates based on parser context.
    pub fn update_candidates(&mut self, input: &str, sources: &CompletionSources) {
        let parts: Vec<&str> = input.split_whitespace().collect();
        self.candidates = match parts.as_slice() {
            [] => vec!["ask", "agent", "cancel", "goto", "help", "quit", ...]
                .iter().map(|s| s.to_string()).collect(),
            ["ask" | "run"] => sources.agent_names.clone(),
            ["agent", "create", _, "--model"] | ["ask", _, _, "--model"] => {
                sources.model_slugs.clone()
            }
            ["goto"] => vec!["agents", "runs", "timeline", "approvals", "memory", "audit"]
                .iter().map(|s| s.to_string()).collect(),
            ["cancel"] | ["retry"] => sources.active_run_ids.clone(),
            ["approve" | "deny"] => sources.pending_effect_ids.clone(),
            _ => vec![],
        };
        self.prefix = parts.last().copied().unwrap_or("").to_string();
        self.index = 0;
        self.filter_by_prefix();
    }

    pub fn next(&mut self) -> Option<&str> { /* cycle forward */ }
    pub fn prev(&mut self) -> Option<&str> { /* cycle backward */ }
}

pub struct CompletionSources {
    pub agent_names: Vec<String>,
    pub model_slugs: Vec<String>,
    pub active_run_ids: Vec<String>,
    pub pending_effect_ids: Vec<String>,
}
```

#### P2-4. Wire `App::execute_command()` in `tui/app.rs`

Add method to `impl App`:

```rust
fn execute_command(&mut self, raw: &str) {
    match command::parse_command(raw) {
        Ok(Command::Goto { target: GotoTarget::Tab(name) }) => {
            match name.as_str() {
                "dashboard" => self.active_tab = Tab::Dashboard,
                "agents" => self.active_tab = Tab::Agents,
                "runs" => self.active_tab = Tab::Runs,
                // ... etc
                _ => self.show_error(&format!("Unknown tab: {name}")),
            }
        }
        Ok(Command::Refresh) => {
            self.last_refresh = Instant::now()
                .checked_sub(Duration::from_secs(REFRESH_INTERVAL_SECS + 1))
                .unwrap_or_else(Instant::now);
        }
        Ok(Command::Help) => {
            // Set a help_visible flag, render help overlay
            self.help_visible = true;
        }
        Ok(Command::Quit) => {
            self.running = false;
        }
        Ok(Command::Ask { .. }) => {
            // Phase 3 — show "Not yet implemented" for now
            self.show_error("Run commands require AppService (Phase 3)");
        }
        Ok(_) => self.show_error("Command not yet implemented"),
        Err(msg) => self.show_error(&msg),
    }
    self.tui_state.mark_dirty();
}

fn show_error(&mut self, msg: &str) {
    self.tui_state.last_error = Some(msg.to_string());
    self.toast_expires = Some(Instant::now() + Duration::from_secs(3));
}
```

Register `tui/command.rs` and `tui/complete.rs` in `tui/mod.rs`:
```rust
pub mod command;
pub mod complete;
pub mod text_input;
```

---

### Phase 3: AppService integration

**Goal:** TUI can create agents and start runs.

#### P3-1. Pass `Arc<AppService>` into TUI App

**The problem:** Currently `App::new()` takes `(Theme, SqlitePool)`. The TUI launch path
in `main.rs:224-239` (`launch_tui`) only passes the pool. The TUI has no access to
`AppService`, which provides `start_run()`, `cancel_run()`, `create_agent()`,
`subscribe_events()`.

**Step 1 — Change `App` struct** (`tui/app.rs:171`):

```rust
pub struct App {
    // ... existing fields ...
    /// Optional AppService for interactive commands (None = read-only mode).
    pub app_service: Option<Arc<AppService>>,
    /// Event receiver for live run updates.
    pub event_rx: Option<EventReceiver>,
}
```

**Step 2 — Change `App::new()` signature** (`tui/app.rs:197`):

```rust
pub fn new(
    theme: Theme,
    pool: SqlitePool,
    app_service: Option<Arc<AppService>>,
) -> Self {
    let event_rx = app_service.as_ref().map(|svc| svc.subscribe_events());
    Self {
        // ... existing fields ...
        app_service,
        event_rx,
    }
}
```

**Step 3 — Build AppService in `launch_tui()`** (`main.rs:224`):

```rust
fn launch_tui(pool: SqlitePool) -> Result<()> {
    use crate::tui::app::{App, enter_tui, exit_tui};
    use crate::tui::theme::Theme;

    let theme = Theme::from_env();

    // Build AppService for interactive mode.
    let config = polkagent_config::ConfigLoader::new().load()?;
    let app_service = build_app_service(&pool, &config)?;  // new helper function

    let mut app = App::new(theme, pool, Some(Arc::new(app_service)));
    // ... rest unchanged ...
}
```

The `build_app_service` helper follows the same pattern as `commands/run.rs:330-500`:
- Load config with `ConfigLoader`
- Build provider registry from env vars
- Create `EventBus::new(256)`, `EventRecorder`
- Create `RunManager` from pool's `RunStore`
- Build `AppServiceBuilder` with executor, event_bus, run_store, etc.
- Return `AppService`

**New deps for polkagent-cli** (`Cargo.toml`):
```toml
polkagent-service = { path = "../polkagent-service" }
polkagent-event = { path = "../polkagent-event" }
```

#### P3-2. Wire `Command::Ask` handler

In `App::execute_command()`, replace the placeholder:

```rust
Ok(Command::Ask { agent, prompt, model, provider, timeout }) => {
    let Some(svc) = &self.app_service else {
        self.show_error("No AppService configured (read-only mode)");
        return;
    };
    // Resolve agent name → AgentId from TuiState
    let agent_id = match self.tui_state.agents.iter().find(|a| a.name == agent) {
        Some(a) => a.id.clone(),
        None => {
            self.show_error(&format!("Agent '{}' not found", agent));
            return;
        }
    };
    // Parse AgentId
    let agent_id = match agent_id.parse::<polkagent_core::AgentId>() {
        Ok(id) => id,
        Err(_) => { self.show_error("Invalid agent ID"); return; }
    };
    // Start run (blocking in the event loop — fine because start_run returns immediately)
    let svc = svc.clone();
    let prompt_owned = prompt.clone();
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            let run_id = handle.block_on(svc.start_run(agent_id, &prompt_owned));
            match run_id {
                Ok(run_id) => {
                    self.show_toast(&format!("Run {} started", &run_id.to_string()[..8]));
                    // Navigate to timeline, select this run
                    self.tui_state.selected_run = Some(run_id.to_string());
                    self.active_tab = Tab::Timeline;
                    // Force refresh to pick up the new run
                    self.force_refresh();
                }
                Err(e) => self.show_error(&format!("Failed to start run: {e}")),
            }
        }
        Err(_) => self.show_error("No async runtime available"),
    }
}
```

#### P3-3. Wire `Command::AgentCreate` handler

```rust
Ok(Command::AgentCreate { name, model }) => {
    let Some(svc) = &self.app_service else {
        self.show_error("No AppService configured");
        return;
    };
    let model = model.unwrap_or_else(|| "anthropic/claude-sonnet-4-6".to_string());
    // Build AgentSpec and call svc.create_agent()
    // Follow pattern from commands/agent.rs:80-120
    // ...
    self.show_toast(&format!("Agent '{}' created", name));
    self.force_refresh();
    self.active_tab = Tab::Agents;
}
```

#### P3-4. Wire `Command::Cancel` handler

```rust
Ok(Command::Cancel { target: CancelTarget::RunId(id) }) => {
    let Some(svc) = &self.app_service else {
        self.show_error("No AppService configured");
        return;
    };
    match id.parse::<polkagent_core::RunId>() {
        Ok(run_id) => {
            let svc = svc.clone();
            match tokio::runtime::Handle::try_current() {
                Ok(handle) => match handle.block_on(svc.cancel_run(run_id)) {
                    Ok(()) => self.show_toast(&format!("Run {} cancelled", &id[..8])),
                    Err(e) => self.show_error(&format!("Cancel failed: {e}")),
                },
                Err(_) => self.show_error("No async runtime available"),
            }
        }
        Err(_) => self.show_error(&format!("Invalid run ID: {id}")),
    }
}
```

#### P3-5. Poll EventBus in the event loop

**Location:** `tui/app.rs:217` — inside `App::run()`, in the main loop after
processing input events:

```rust
// Poll event bus for live updates (non-blocking).
if let Some(ref mut rx) = self.event_rx {
    while let Ok(event) = rx.try_recv() {
        self.on_bus_event(event);
    }
}
```

Add handler:
```rust
fn on_bus_event(&mut self, event: polkagent_event::DurableEvent) {
    // Append to timeline if event matches selected run
    if let Some(ref selected) = self.tui_state.selected_run {
        if event.run_id.to_string() == *selected {
            self.tui_state.run_events.push(EventSummary {
                id: event.id.to_string(),
                timestamp: event.timestamp,
                event_type: event.kind.clone(),
                description: extract_description(&event),
                payload: event.data_json.clone().unwrap_or_default(),
            });
            self.tui_state.mark_dirty();
        }
    }
    // Check for terminal events to show toast
    if event.kind.starts_with("run_completed") || event.kind.starts_with("run_failed") {
        let short = &event.run_id.to_string()[..8];
        self.show_toast(&format!("Run {short} {}", event.kind));
    }
    // Update notification badges
    if event.kind == "effect_proposed" {
        self.tui_state.pending_approval_count += 1;
    }
}
```

#### P3-6. Add toast system to status bar

**Location:** `tui/app.rs` struct — add fields:

```rust
pub toast_message: Option<String>,
pub toast_is_error: bool,
pub toast_expires: Option<Instant>,
```

**Location:** `tui/widgets/status_bar.rs` — modify render to check toast:

```rust
// In the centre column, prefer toast_message over last_error
let centre_text = if let Some(toast) = &app.toast_message {
    Span::styled(toast, if app.toast_is_error {
        theme.status_error()
    } else {
        theme.status_active()
    })
} else if let Some(err) = &state.last_error {
    Span::styled(err, theme.status_error())
} else {
    Span::styled("Ready", theme.dim_style())
};
```

**Location:** `tui/app.rs:run()` — in the loop, check toast expiry:

```rust
if let Some(expires) = self.toast_expires {
    if Instant::now() >= expires {
        self.toast_message = None;
        self.toast_expires = None;
        self.tui_state.mark_dirty();
    }
}
```

---

### Phase 4: Streaming output and inline approval

**Goal:** Watch runs execute in real-time. Approve effects without tab-switching.

#### P4-1. Add live output pane to RunDetail view

**Location:** `tui/views/run_detail.rs` — after the existing info/turns panels,
add a third panel that shows streaming events:

```rust
fn render_live_output(
    frame: &mut Frame,
    area: Rect,
    events: &[EventSummary],
    scroll: &ScrollState,
    theme: &Theme,
    auto_scroll: bool,
) {
    let block = Block::bordered().title(" Live Output ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible_height = inner.height as usize;
    let offset = if auto_scroll {
        events.len().saturating_sub(visible_height)
    } else {
        scroll.offset
    };

    for (i, event) in events.iter().skip(offset).take(visible_height).enumerate() {
        let y = inner.y + i as u16;
        let time = event.timestamp.format("%H:%M:%S");
        let (prefix, style) = match event.event_type.as_str() {
            k if k.starts_with("turn_") => ("", theme.dim_style()),
            k if k.starts_with("run_") => ("", theme.dim_style()),
            _ => ("░░ ", theme.primary_style()),
        };
        let line = format!("[{time}] {prefix}{}", event.description);
        let span = Span::styled(&line, style);
        frame.render_widget(Paragraph::new(span), Rect::new(inner.x, y, inner.width, 1));
    }
}
```

#### P4-2. Auto-scroll with manual lock

Add to `TuiState`:
```rust
/// If true, auto-scroll timeline to bottom. Set false when user scrolls up.
pub auto_scroll_timeline: bool,
```

Default `true`. Set to `false` when `TuiAction::ScrollUp` is received on Timeline.
Set back to `true` on `TuiAction::ScrollToBottom` or when a new run is selected.

#### P4-3. Inline approval in Timeline

When an `effect_proposed` event appears in the timeline, render it with an
inline action card and enable `a`/`d` keys in the Timeline tab:

**Location:** `tui/app.rs` — extend the `TuiAction::ApproveEffect` guard:

```rust
// Current (line ~547):
TuiAction::ApproveEffect => {
    if self.active_tab == Tab::Approvals { /* ... */ }
}

// Change to:
TuiAction::ApproveEffect => {
    if self.active_tab == Tab::Approvals || self.active_tab == Tab::Timeline {
        /* ... same confirm dialog logic ... */
    }
}
```

#### P4-4. Notification badges

**Location:** `tui/widgets/header_bar.rs` — when rendering tab labels, append
badge count:

```rust
let label = if tab == Tab::Approvals && state.pending_approval_count > 0 {
    format!("F6 Approvals ({})", state.pending_approval_count)
} else {
    format!("{} {}", tab.fkey_label(), tab.label())
};
```

Add `pending_approval_count: usize` to `TuiState`.

---

### Phase 5: Agent creation dialog and polish

#### P5-1. Create `tui/widgets/modal_form.rs` (new file)

A multi-field form widget rendered as a centered overlay (similar to
`approvals.rs:300-360` confirm dialog pattern, but with multiple fields).

```rust
pub struct FormField {
    pub label: &'static str,
    pub input: TextInput,
    pub required: bool,
}

pub struct ModalForm {
    pub title: String,
    pub fields: Vec<FormField>,
    pub focused_field: usize,
}

pub fn render_modal_form(
    frame: &mut Frame,
    area: Rect,
    form: &ModalForm,
    theme: &Theme,
) {
    // Center: 60w x (fields.len() * 2 + 6)h
    // Use Clear widget to render on top of content
    // Render each field as "Label:  [input__________]"
    // Highlight focused field with border_dream color
    // Bottom: [Submit] [Cancel] buttons
}
```

#### P5-2. Wire agent creation form in `App`

Add to `App`:
```rust
pub agent_form: Option<ModalForm>,
```

`Command::AgentCreate` with no name opens the form:
```rust
Ok(Command::AgentCreate { name: None, .. }) => {
    self.agent_form = Some(ModalForm {
        title: "Create Agent".to_string(),
        fields: vec![
            FormField { label: "Name", input: TextInput::new("my-agent"), required: true },
            FormField { label: "Model", input: TextInput::with_default("anthropic/claude-sonnet-4-6", ""), required: true },
            FormField { label: "System Prompt", input: TextInput::new("Optional system prompt"), required: false },
        ],
        focused_field: 0,
    });
    self.input_mode = InputMode::Insert;
}
```

#### P5-3. Command history persistence

In `TextInput`:
```rust
pub fn save_history(&self, path: &Path) -> io::Result<()> {
    let content: String = self.history.iter().map(|s| format!("{s}\n")).collect();
    std::fs::write(path, content)
}

pub fn load_history(&mut self, path: &Path) -> io::Result<()> {
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        self.history = content.lines().map(String::from).collect();
        if self.history.len() > self.max_history {
            self.history.drain(0..self.history.len() - self.max_history);
        }
    }
    Ok(())
}
```

Call `load_history` in `App::new()` and `save_history` in `exit_tui()`:
```rust
let history_path = dirs::home_dir().unwrap().join(".polkagent/tui_history");
app.prompt_input.load_history(&history_path).ok();
// ... on exit:
app.prompt_input.save_history(&history_path).ok();
```

---

## 10. Complete file manifest

### New files to create

| # | Full path | LOC est. | Phase |
|---|---|---|---|
| 1 | `crates/polkagent-cli/src/tui/text_input.rs` | ~250 | P1 |
| 2 | `crates/polkagent-cli/src/tui/command.rs` | ~200 | P2 |
| 3 | `crates/polkagent-cli/src/tui/complete.rs` | ~120 | P2 |
| 4 | `crates/polkagent-cli/src/tui/widgets/prompt_bar.rs` | ~80 | P2 |
| 5 | `crates/polkagent-cli/src/tui/widgets/modal_form.rs` | ~150 | P5 |

### Existing files to modify

| # | Full path | What changes | Phase |
|---|---|---|---|
| 1 | `crates/polkagent-cli/src/tui/input.rs` | Add 18 `TuiAction` variants, replace Insert/Command stub (lines 37-91, 113-119), add `:` binding (line 189) | P1 |
| 2 | `crates/polkagent-cli/src/tui/app.rs` | Add `TextInput`, `prompt_visible`, `app_service`, `event_rx`, `toast_*` fields to `App` (line 171). Add `execute_command()`, `on_bus_event()`, `active_text_input_mut()`, `show_toast()`, `show_error()` methods. Add input action handlers to `apply_action()` (line 273). Add event bus polling to `run()` loop (line 217). Update layout to include prompt bar row. | P1-P4 |
| 3 | `crates/polkagent-cli/src/tui/state.rs` | Add `toast_message`, `toast_is_error`, `pending_approval_count`, `auto_scroll_timeline`, `search_filter` fields to `TuiState` (line 388). | P1-P4 |
| 4 | `crates/polkagent-cli/src/tui/mod.rs` | Add `pub mod text_input; pub mod command; pub mod complete;` | P1-P2 |
| 5 | `crates/polkagent-cli/src/tui/widgets/mod.rs` | Add `pub mod prompt_bar; pub mod modal_form;` | P2, P5 |
| 6 | `crates/polkagent-cli/src/tui/views/memory.rs` | Change `render_search_bar` to accept `&TextInput` instead of `&str` query (line 65). Render cursor position. | P1 |
| 7 | `crates/polkagent-cli/src/tui/views/run_detail.rs` | Add `render_live_output()` pane for streaming events. | P4 |
| 8 | `crates/polkagent-cli/src/tui/views/timeline.rs` | Render inline approval cards for `effect_proposed` events. | P4 |
| 9 | `crates/polkagent-cli/src/tui/widgets/header_bar.rs` | Add notification badge count next to Approvals tab label. | P4 |
| 10 | `crates/polkagent-cli/src/tui/widgets/status_bar.rs` | Render toast messages in centre column, auto-dismiss after 3s. | P3 |
| 11 | `crates/polkagent-cli/src/main.rs` | Update `launch_tui()` (line 224) to build `AppService` and pass `Arc<AppService>` to `App::new()`. Add `build_app_service()` helper. | P3 |
| 12 | `crates/polkagent-cli/Cargo.toml` | Add `unicode-segmentation = "1"`, `polkagent-service`, `polkagent-event` deps. | P1, P3 |

---

## 11. Data flow: `ask` command end-to-end

```
1. User types:  :ask dev-helper "Check referendum 1234"
                    │
2. CommandParser    │  (tui/command.rs:parse_command)
   verb=ask, target="dev-helper", prompt="Check referendum 1234"
                    │
3. App::execute_command()               (tui/app.rs)
   ├── resolve agent: tui_state.agents.find(name == "dev-helper") → AgentId
   ├── start run:     app_service.start_run(agent_id, prompt) → RunId
   │                  (polkagent-service/src/app.rs:796)
   │                  └── RunManager::create_run → RunId
   │                  └── RunManager::enqueue_run → Created→Queued
   │                  └── tokio::spawn(orchestrator.execute_run)
   ├── navigate:      active_tab = Tab::Timeline, selected_run = run_id
   └── toast:         "Run 019fc... started"
                    │
4. Background       │  orchestrator.execute_run() runs in spawned task
   turn loop        │  (polkagent-run/src/orchestrator.rs)
                    │
5. EventBus         │  (polkagent-event/src/bus.rs)
   RunStarted, TurnStarted, TurnCompleted, ...
   └── App::on_bus_event()              (tui/app.rs)
       ├── append EventSummary to tui_state.run_events
       ├── mark_dirty() → triggers re-render
       └── auto-scroll to bottom
                    │
6. Terminal state   │  RunCompleted | RunFailed | RunTimedOut
   └── App::on_bus_event()
       ├── toast: "Run 019fc... completed (14 turns, 12.4k tokens)"
       └── force_refresh() to update Runs list
```

---

## 13. Key binding summary

### Normal mode (existing + new)

| Key | Action |
|---|---|
| F1-F8 | Switch tab |
| j/k | Navigate up/down |
| Enter | Select / drill down |
| Esc | Back |
| a/d | Approve/deny (Approvals + Timeline) |
| `/` | **Search** (scoped to active tab) |
| `:` | **Open prompt bar** |
| `c` | **Cancel selected run** (Runs tab) |
| `r` | Refresh |
| `q` | Quit |

### Insert mode (search — fixed)

| Key | Action |
|---|---|
| Any char | Insert at cursor |
| Backspace | Delete before cursor |
| Left/Right | Move cursor |
| Ctrl+W | Delete word |
| Ctrl+U | Clear to start |
| Enter | Execute search (stays in search mode) |
| Esc | Exit search, return to Normal |

### Command mode (prompt bar — new)

| Key | Action |
|---|---|
| Any char | Insert at cursor |
| Backspace | Delete before cursor |
| Left/Right | Move cursor |
| Up/Down | History navigation |
| Tab | Autocomplete |
| Enter | Submit command |
| Esc | Close prompt bar, return to Normal |
| Ctrl+R | Reverse history search |

---

## 14. Error handling

| Scenario | Behavior |
|---|---|
| Unknown command | Status bar shows `"Unknown command: foo"` in danger color for 3s |
| Agent not found | Status bar shows `"Agent 'xyz' not found. Use :agent list"` |
| Run start fails | Status bar shows error message from `AppService` |
| No providers configured | Status bar shows `"No AI providers configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY"` |
| Parse error | Prompt bar border flashes red, cursor stays at error position |

All errors are non-fatal. The TUI never crashes from a bad command. Errors
display for 3 seconds and then auto-dismiss, or the user can press any key to
dismiss immediately.

---

## 15. Testing strategy

| Test type | Coverage |
|---|---|
| Unit: `TextInput` | Cursor math, Unicode grapheme handling, word boundaries, history |
| Unit: `CommandParser` | All command forms, quoted strings, flags, edge cases |
| Unit: `Completer` | Agent name completion, model completion, prefix matching |
| Integration: command → AppService | Mock `AppService`, verify correct method calls |
| Integration: event → render | Inject events, verify TuiState updates and render output |
| Manual: end-to-end | Start TUI, create agent, run prompt, approve effect, verify output |

---

## 16. Out of scope (future PRD)

- Multi-line prompt editor (vim/emacs keybindings)
- Agent-to-agent conversation chains from TUI
- TUI-based agent spec YAML editor
- Mouse support for clicking tab headers / list items
- Clipboard integration for copying run output
- Split-pane views (two runs side by side)
- TUI themes / color scheme selection
- Remote TUI (SSH-forwarded rendering)

---

## 17. Success criteria

1. A new user can install polkagent, run `polkagent tui`, and within 30 seconds
   start their first agent run by typing `:ask <agent> "..."`.
2. The full run lifecycle — creation, execution, approval, completion — is
   visible and controllable from within the TUI without switching terminals.
3. Zero regressions to existing TUI functionality (monitoring, navigation,
   approve/deny).
4. All 5 phases can be implemented incrementally. Each phase is independently
   shippable and useful.

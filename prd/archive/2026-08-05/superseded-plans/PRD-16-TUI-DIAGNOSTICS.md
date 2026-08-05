# PRD: TUI Diagnostics, Claude Integration, and Bug Triage

**Version:** 1.0.0
**Date:** 2026-08-03
**Status:** Draft

---

## 1. Context & Motivation

Polkagent v0.1.0 has a working ROSEDUST TUI, but a first-run session reveals multiple issues that block productive use. Additionally, there is no workflow for an AI agent (Claude Code, Codex, etc.) to run polkagent, inspect its output, diagnose problems, and iterate on fixes. This PRD addresses both:

1. **Triage and fix the observed TUI bugs** (from real screenshots)
2. **Design a Claude-in-the-loop diagnostic workflow** so that Claude Code can run polkagent, collect feedback, and course-correct without requiring manual screenshots

---

## 2. Current State — Observed Issues

Analysis of 8 TUI screenshots (all tabs, v0.1.0, 2026-08-03):

### BUG-01: Audit SQL Schema Mismatch (P0 — blocks every tab)

**Symptom:** Every tab displays the error: `! audit: no such column: event_type in SELECT id, run_id, event_type, payload_json, created_at FROM run_events`

**Root cause:** The TUI's `audit_events()` query in `crates/polkagent-cli/src/tui/db.rs:498-505` selects columns `event_type`, `payload_json`, and `created_at` from `run_events`. But the production schema in `crates/polkagent-store-sqlite/src/schema.sql:194-204` defines `run_events` with columns `kind`, `data_json`, and `timestamp`. The TUI has a test-only schema (`tui/db.rs:954-961`) that uses the old column names — confirming this is a rename that was applied to the production schema but not propagated to the TUI query.

**Column mapping:**
| TUI query (wrong) | Production schema (correct) |
|---|---|
| `event_type` | `kind` |
| `payload_json` | `data_json` |
| `created_at` | `timestamp` |

**Fix:** Update the SELECT in `tui/db.rs:500` and the test schema in `tui/db.rs:954-961` to use the correct column names.

**Scope (wider than initially thought):** The same column name mismatch exists in **3 files**:
- `crates/polkagent-cli/src/tui/db.rs` — lines 272, 500, 570
- `crates/polkagent-cli/src/commands/export.rs` — lines 692, 739 (also references non-existent `audit_log` table)
- `crates/polkagent-cli/src/commands/logs.rs` — lines 127, 135, 145, 153

### BUG-02: Runs Stuck in "running" for 25+ Hours (P0)

**Symptom:** F3 Runs tab shows 3 runs in `running` state for 25h 2m, 25h 8m, 25h 14m respectively, all with 0 tokens.

**Root cause (confirmed by research):**
- **Timeout is in-memory only:** The timeout enforcer (`crates/polkagent-run/src/timeout.rs`) checks deadlines inside the orchestrator's turn loop (`orchestrator.rs:571-584`) using `tokio::time::Instant`. If the orchestrator process exits/crashes, the deadline is lost — it is never persisted to the DB.
- **No stale-run reaper exists:** No background process scans for runs stuck in `running` state. The effect system has a lease reaper, but runs do not.
- **`runs` table missing columns:** The schema (`schema.sql:38-48`) has no `started_at`, no `token_usage`, and no `deadline` column — making crash recovery impossible.
- **Token usage never persisted:** Total usage is accumulated in-memory in the orchestrator (`orchestrator.rs:454`) but never written to the database. When the orchestrator exits, token data is lost.
- **Codex harness handles missing binary correctly** (`ExecutableNotFound` error) but the run may hang if the binary starts but the process stalls.

### BUG-03: Run Stuck in "queued" for 45+ Hours (P1)

**Symptom:** F3 shows run `019fbdd6` in `queued` state for 45h 20m.

**Likely cause:** The scheduler (`crates/polkagent-scheduler/`) enforces `max_concurrent_runs` — since 3 runs are permanently "running", the queue is permanently blocked. Fixing BUG-02 (unsticking or cancelling zombie runs) would unblock this.

### BUG-04: All Token Counts are 0 / "no data" (P1)

**Symptom:** F1 Dashboard shows "Token Usage: no data". F3 Runs shows "Tokens: 0" for every run, including the 2 that completed in 3-4 seconds.

**Root cause (confirmed — two independent bugs):**

1. **Dashboard sparkline never populated:** The `TuiState::recompute_widget_data()` method (`state.rs:514-541`) computes `token_history` from turn data, but is **never called** in `app.rs`. It's only used in unit tests (`state.rs:604-666`). This causes the dashboard to always show "no data".
   - **Fix:** Add `self.tui_state.recompute_widget_data();` after updating `run_detail` in `app.rs` (lines ~744 and ~784).

2. **Token usage never persisted to DB:** The orchestrator accumulates `total_usage` in-memory (`orchestrator.rs:454, 676-677`) but never writes it to the `runs` table. The `runs` schema has no token columns. Per-turn tokens ARE stored in the `turns` table (`store.rs:548`) and the TUI can query them (`tui/db.rs:548-559`), but the dashboard code path doesn't call `recompute_widget_data()` to actually use them.

### BUG-05: Config Sources "(none found)" (P2)

**Symptom:** F4 System tab shows `Config Sources: Loaded from: (none found)`.

**Root cause (confirmed):** The TUI's `detect_config_sources()` function (`tui/views/system.rs:418-442`) only checks 3 hardcoded paths:
- `~/.config/polkagent/polkagent.toml`
- `/etc/polkagent/polkagent.toml`
- `./polkagent.toml` (current directory only)

But the real `ConfigLoader::load()` (`polkagent-config/src/loader.rs:97-145`) walks **up ancestor directories** looking for `.polkagent/polkagent.toml`. The TUI display doesn't replicate this walk, so it misses project-local configs created by `polkagent init`.

### BUG-06: Polkadot Disconnected, Best/Final #0 (P2)

**Symptom:** F4 System shows `Polkadot: disconnected, Best: #0, Final: #0, Metadata v14, stale`.

**Expected behavior:** Chain connection is optional — polkagent should work without a live Polkadot RPC. The TUI should display this as "not configured" rather than "disconnected" with stale metadata, which looks like an error.

### BUG-07: Budget Remaining 0.0% (P2)

**Symptom:** F4 System shows `Budget: Remaining 0.0%` in red.

**Root cause (confirmed):** The `budget_remaining` field in `TuiState` (`state.rs:491`) is initialized to `0.0` by `Default` and **is never updated**. The `TuiDb::budget_status()` method exists (`tui/db.rs:585-600`, marked `#[allow(dead_code)]`) and correctly calculates spend from the `turns` table, but it is never called in `app.rs::refresh_data()`. The fix is to call `db.budget_status()` during refresh and set `state.budget_remaining`.

### BUG-08: Completed Runs with 0s Duration (P3)

**Symptom:** F3 shows runs `019fbddf` and `019fbdd9` completed in 0s with 0 tokens.

**Likely cause:** These runs failed immediately (e.g., Codex binary not found, executor error) and were marked completed without actually executing. The state should be `failed` or `error`, not `completed`.

---

## 3. Claude-in-the-Loop Diagnostic Workflow

### 3.1 Problem

Claude Code cannot:
- See the TUI (it's a full-screen ratatui app that takes over the terminal)
- Take screenshots of the TUI
- Interact with the TUI (send keypresses)

This means Claude cannot independently verify TUI rendering, spot visual bugs, or iterate on TUI development.

### 3.2 Diagnostic Channels (What Claude CAN Do)

#### Channel 1: CLI Commands with `--json` Output

Claude can run polkagent CLI commands and parse their output:

```bash
# Health check — verify provider, DB, system status
polkagent doctor

# Status — agent counts, active runs, queue depth
polkagent status

# Agent inspection
polkagent agent list
polkagent agent show <AGENT_ID>

# Run inspection
polkagent run list             # or similar subcommand
polkagent logs --tail 50       # recent event log

# Effect inspection
polkagent inbox list
polkagent inbox show <EFFECT_ID>

# Config validation
polkagent config               # show or validate config
```

**TODO:** Add `--json` flag to all read commands for machine-parseable output. Priority: `status`, `agent list`, `agent show`, `doctor`.

#### Channel 2: Direct SQLite Queries

Claude can query the database directly to inspect state:

```bash
# Database path
DB="$HOME/.local/share/polkagent/polkagent.db"

# List agents
sqlite3 "$DB" "SELECT id, name, state, model FROM agents;"

# List runs with status
sqlite3 "$DB" "SELECT id, agent_id, state, created_at FROM runs ORDER BY created_at DESC LIMIT 20;"

# Check for stuck runs
sqlite3 "$DB" "SELECT id, state, created_at FROM runs WHERE state IN ('running', 'queued');"

# Check events for a run
sqlite3 "$DB" "SELECT id, run_id, kind, timestamp FROM run_events WHERE run_id = '<RUN_ID>' ORDER BY sequence;"

# Check effect pipeline
sqlite3 "$DB" "SELECT id, run_id, kind, state FROM effect_intents WHERE state != 'resolved';"

# Database size and table counts
sqlite3 "$DB" "SELECT name, (SELECT COUNT(*) FROM pragma_table_info(name)) as cols FROM sqlite_master WHERE type='table' ORDER BY name;"
```

**TODO:** Create a `polkagent debug dump` command that outputs a JSON snapshot of system state (agents, runs, effects, config, schema version) for diagnostic purposes.

#### Channel 3: REST API Queries

When the API server is running, Claude can query it:

```bash
# Start server
polkagent serve --port 9090 &

# Health probes
curl -s http://127.0.0.1:9090/health/ready | jq .
curl -s http://127.0.0.1:9090/health/live | jq .

# System info
curl -s http://127.0.0.1:9090/api/v1alpha1/system/info | jq .

# List agents
curl -s http://127.0.0.1:9090/api/v1alpha1/agents | jq .

# List runs
curl -s http://127.0.0.1:9090/api/v1alpha1/runs | jq .

# Get run details
curl -s http://127.0.0.1:9090/api/v1alpha1/runs/<RUN_ID> | jq .

# Check pending effects
curl -s http://127.0.0.1:9090/api/v1alpha1/effects | jq '.[] | select(.state == "pending")'

# Prometheus metrics
curl -s http://127.0.0.1:9090/metrics
```

#### Channel 4: TUI Snapshot Testing (ratatui TestBackend)

Ratatui provides a `TestBackend` that renders to an in-memory buffer instead of the terminal. This enables:

- **Headless rendering:** Render any TUI tab to a string buffer without a terminal
- **Snapshot assertions:** Compare rendered output against expected snapshots
- **Regression testing:** Detect visual regressions automatically

**TODO:** Add a `polkagent tui --render-tab <TAB> --headless` mode that:
1. Initializes the app with a `TestBackend`
2. Renders the specified tab
3. Prints the rendered buffer as text to stdout
4. Exits

This lets Claude "see" any TUI tab by reading the text output.

#### Channel 5: Terminal Recording with VHS/asciinema

For CI and visual verification:

```bash
# Record a TUI session with VHS (Charm CLI)
vhs < script.tape    # produces a GIF

# Record with asciinema
asciinema rec --command "polkagent tui" recording.cast

# Replay as text
asciinema cat recording.cast
```

**VHS tape example** for automated TUI testing:
```tape
Output tui-screenshot.gif
Set Shell "bash"
Set FontSize 14
Set Width 1200
Set Height 800

Type "polkagent tui"
Enter
Sleep 2s
Screenshot tui-dashboard.png

Type "2"   # F2 Agents
Sleep 1s
Screenshot tui-agents.png

Type "3"   # F3 Runs
Sleep 1s
Screenshot tui-runs.png

Type "q"
```

#### Channel 6: Log Tailing and Event Stream

```bash
# Tail structured logs
RUST_LOG=polkagent=debug polkagent tui 2>debug.log &
tail -f debug.log | jq .

# Or use the logs command
polkagent logs --level debug --follow

# WebSocket event stream (when API server running)
wscat -c ws://127.0.0.1:9090/api/v1alpha1/events/stream
```

### 3.3 Recommended Claude Diagnostic Workflow

```
┌─────────────────────────────────────────────────────────┐
│                  Claude Diagnostic Loop                  │
├─────────────────────────────────────────────────────────┤
│                                                         │
│  1. BUILD                                               │
│     cargo build --release -p polkagent-cli              │
│                                                         │
│  2. HEALTH CHECK                                        │
│     polkagent doctor                                    │
│     polkagent status --json                             │
│                                                         │
│  3. INSPECT STATE                                       │
│     sqlite3 $DB "SELECT ..."  (direct DB queries)       │
│     curl localhost:9090/api/... (API queries)            │
│     polkagent agent list / run list / inbox list         │
│                                                         │
│  4. TUI SNAPSHOT (after headless mode is implemented)    │
│     polkagent tui --render-tab F1 --headless             │
│     polkagent tui --render-tab F4 --headless             │
│                                                         │
│  5. RUN TESTS                                           │
│     cargo test -p polkagent-cli --test tui_tests         │
│     cargo test -p polkagent-integration-tests            │
│                                                         │
│  6. MAKE FIX                                            │
│     Edit code → rebuild → re-check                      │
│                                                         │
│  7. VERIFY                                              │
│     Re-run health check + state inspection              │
│     Re-run TUI snapshot → compare before/after          │
│     Run eval suites for regression                      │
│                                                         │
│  8. USER SCREENSHOT (fallback)                          │
│     Ask user to take screenshot + share                 │
│     Claude reads the image with vision                  │
│                                                         │
└─────────────────────────────────────────────────────────┘
```

---

## 4. Implementation Plan

### Phase 1: Critical Bug Fixes (P0)

#### 4.1 Fix Audit SQL Schema Mismatch

**File:** `crates/polkagent-cli/src/tui/db.rs`

- [ ] Line 500: Change `event_type` → `kind`, `payload_json` → `data_json`, `created_at` → `timestamp`
- [ ] Lines 954-961: Update test schema to match production column names
- [ ] Search for ALL other references to the old column names in tui/db.rs
- [ ] Run `cargo test -p polkagent-cli` to verify
- [ ] Verify the error disappears from the TUI status bar

#### 4.2 Fix Stuck Runs / Add Timeout Enforcement

- [ ] Investigate `crates/polkagent-run/src/` for run timeout logic
- [ ] Investigate `crates/polkagent-scheduler/src/` for stale run detection
- [ ] Add a stale-run reaper: any run in `running` state longer than `2 * timeout_secs` should be moved to `failed` with reason `timeout_exceeded`
- [ ] Add CLI command to force-cancel zombie runs: `polkagent run cancel <RUN_ID> --force`
- [ ] Verify the queued run unblocks after zombie runs are cleaned up

#### 4.3 Fix Codex Harness Error Handling

- [ ] Check `crates/polkagent-harness-codex/src/` — does it handle "binary not found" gracefully?
- [ ] If the codex binary isn't installed, the run should fail immediately with a clear error, not hang forever
- [ ] Add a pre-flight check: `which codex` before attempting dispatch
- [ ] Return proper error to the run manager so the run transitions to `failed`

### Phase 2: Data Quality Fixes (P1)

#### 4.4 Fix Token Count Tracking

- [ ] Investigate how tokens flow from executor → run → database
- [ ] Check if the Codex harness reports token usage
- [ ] If not, at minimum track that "token data unavailable" vs showing 0 (which implies the run used zero tokens)
- [ ] For completed runs that actually hit the LLM, ensure input/output tokens are recorded

#### 4.5 Fix Run State Accuracy

- [ ] Runs that fail immediately (executor error, missing harness) should be `failed`, not `completed` with 0s duration
- [ ] Add a `failure_reason` column or field to the run record
- [ ] TUI should show failed runs with a distinct indicator (red `x` instead of green `✓`)

### Phase 3: UX Polish (P2)

#### 4.6 Fix Config Source Display

- [ ] Track which config file(s) were loaded and display paths
- [ ] If auto-synthesized from env vars, show "auto-detected from ANTHROPIC_API_KEY" or similar
- [ ] If no config file exists, show "default (no config file found)" not "(none found)"

#### 4.7 Fix Chain Connection Display

- [ ] When no chain endpoint is configured, show "not configured" instead of "disconnected"
- [ ] Only show "disconnected" when an endpoint IS configured but can't be reached
- [ ] Remove stale metadata display when no connection was ever established

#### 4.8 Fix Budget Display

- [ ] When no budget is configured, show "unlimited" or "not configured" instead of "0.0%"
- [ ] Only show percentage when a budget IS configured
- [ ] Distinguish between "budget not set" and "budget depleted"

### Phase 4: Claude Diagnostic Tooling

#### 4.9 Add `--json` Output to CLI Read Commands

- [ ] `polkagent status --json` → JSON object with agents, runs, effects, health
- [ ] `polkagent agent list --json` → JSON array of agents
- [ ] `polkagent agent show <ID> --json` → JSON agent object
- [ ] `polkagent doctor --json` → JSON health check results
- [ ] `polkagent inbox list --json` → JSON array of pending effects

#### 4.10 Add `polkagent debug dump` Command

- [ ] Output a comprehensive JSON snapshot:
  - Schema version
  - Agent count and states
  - Run count by state (running, queued, completed, failed, cancelled)
  - Pending effect count
  - Memory entry count
  - Config sources
  - Provider status
  - Recent errors
  - Database size
- [ ] Useful for bug reports and automated diagnostics

#### 4.11 Add TUI Headless Render Mode

- [ ] `polkagent tui --render-tab <TAB> --headless`
- [ ] Uses ratatui `TestBackend` to render to a string buffer
- [ ] Prints the rendered frame to stdout as text
- [ ] Claude can read this to "see" the TUI without a terminal
- [ ] Enables snapshot testing in CI

#### 4.12 Add TUI Snapshot Tests

- [ ] Create `crates/polkagent-cli/tests/tui_snapshots/` directory
- [ ] Write tests that:
  1. Create a `TestBackend` with known dimensions
  2. Populate mock data (agents, runs, effects)
  3. Render each tab
  4. Compare against saved snapshots (insta or similar)
- [ ] Run in CI to catch visual regressions

#### 4.13 VHS Tape for Automated Screenshots

- [ ] Create `tests/tui/dashboard.tape` — records each tab
- [ ] Add to CI as an optional job
- [ ] Produces GIF artifacts for PR review

### Phase 5: Eval & Feedback Loop

#### 4.14 Add TUI-Specific Eval Suite

- [ ] Create `fixtures/evals/tui/suite.json` with cases like:
  - "Start with no agents → dashboard shows empty state correctly"
  - "Create agent → agents tab shows it"
  - "Start a run → runs tab updates, dashboard shows activity"
  - "Run completes → tokens populated, duration accurate"
- [ ] These evals verify the data pipeline end-to-end

#### 4.15 Add Integration Test for Full Run Lifecycle with TUI Data

- [ ] In `crates/polkagent-integration-tests/`:
  - Create agent → run → complete → verify DB state matches what TUI would display
  - Use fake executor to control response (including token counts)
  - Verify run_events table has correct columns and data

---

## 5. Implementation Checklist

### Immediate (do now)

- [ ] Fix `tui/db.rs` audit query column names (BUG-01)
- [ ] Fix `tui/db.rs` test schema to match production (BUG-01)
- [ ] Cancel or expire stuck runs in the database (BUG-02)
- [ ] Build and test: `cargo build -p polkagent-cli && cargo test -p polkagent-cli`

### Short-term (this week)

- [ ] Add timeout enforcement for runs
- [ ] Add stale-run reaper
- [ ] Fix Codex harness "binary not found" handling
- [ ] Fix "completed with 0s" → should be "failed"
- [ ] Add `polkagent status --json`
- [ ] Add `polkagent debug dump`

### Medium-term (next sprint)

- [ ] Add TUI headless render mode
- [ ] Add TUI snapshot tests
- [ ] Fix config source display
- [ ] Fix chain connection display
- [ ] Fix budget display
- [ ] Add `--json` to all read commands
- [ ] Token tracking through harness interface

### Long-term (backlog)

- [ ] VHS tape CI integration
- [ ] TUI eval suite
- [ ] Full Claude diagnostic loop documentation
- [ ] Screenshot comparison in CI (visual regression)

---

## 6. How to Run Polkagent with Codex

### Prerequisites

```bash
# Install Codex CLI
npm install -g @openai/codex

# Verify
codex --version

# Set API key
export OPENAI_API_KEY="sk-..."
```

### Configure the Harness

```toml
# .polkagent/polkagent.toml
[harness]
harness_type = "codex"
timeout_secs = 300

[harness.harnesses.codex]
binary_path = "codex"
transport = "stdio"
```

### Run with Codex

```bash
# Create an agent
polkagent agent create my-codex-agent \
  --model anthropic/claude-sonnet-4-6 \
  --description "Codex-powered agent"

# Run with Codex harness
polkagent run -a my-codex-agent \
  -p "Summarize referendum 1234" \
  --harness codex

# Or launch TUI (uses configured default harness)
polkagent tui
```

### Important Notes

1. The `--harness` flag overrides the configured default for a single run
2. Codex harness requires the `codex` binary on PATH
3. If Codex is not installed, the run will currently hang (BUG-02/BUG-03) — this PRD addresses fixing that
4. Token usage may not be reported through the Codex harness (BUG-04)

---

## 7. Claude Code Diagnostic Cheat Sheet

### Quick health check

```bash
polkagent doctor
polkagent status
```

### Inspect the database directly

```bash
DB="$HOME/.local/share/polkagent/polkagent.db"

# Schema check — list all tables and columns
sqlite3 "$DB" ".schema"

# Stuck runs
sqlite3 "$DB" "SELECT id, state, created_at FROM runs WHERE state IN ('running','queued');"

# Recent events
sqlite3 "$DB" "SELECT id, run_id, kind, timestamp FROM run_events ORDER BY timestamp DESC LIMIT 20;"

# Agent list
sqlite3 "$DB" "SELECT id, name, state, model FROM agents;"

# Pending effects
sqlite3 "$DB" "SELECT * FROM effect_intents WHERE state = 'pending';"
```

### Cancel stuck runs manually

```bash
sqlite3 "$DB" "UPDATE runs SET state = 'cancelled' WHERE state = 'running' AND created_at < datetime('now', '-1 hour');"
```

### Build and test after changes

```bash
cargo build -p polkagent-cli
cargo test -p polkagent-cli
cargo test -p polkagent-integration-tests
cargo test -p polkagent-store-sqlite
```

### Ask user for visual verification

When Claude needs to see the TUI but can't:
1. Ask the user to take a screenshot
2. User shares the image
3. Claude reads it with vision capabilities
4. Claude diagnoses visual issues and makes fixes
5. Repeat until correct

---

## 8. Files to Modify

| File | Change | Priority |
|------|--------|----------|
| `crates/polkagent-cli/src/tui/db.rs` | Fix audit query column names (BUG-01) | P0 |
| `crates/polkagent-run/src/` | Add timeout enforcement (BUG-02) | P0 |
| `crates/polkagent-scheduler/src/` | Add stale-run reaper (BUG-02) | P0 |
| `crates/polkagent-harness-codex/src/` | Fix missing binary handling (BUG-02) | P0 |
| `crates/polkagent-cli/src/tui/` | Fix budget/config/chain display (BUG-05/06/07) | P2 |
| `crates/polkagent-cli/src/cli.rs` | Add `--json` flags, `debug dump` command | P1 |
| `crates/polkagent-cli/src/tui/` | Add headless render mode | P2 |
| `crates/polkagent-cli/tests/` | Add TUI snapshot tests | P2 |
| `fixtures/evals/tui/suite.json` | Create TUI eval suite | P3 |

---

## 9. Success Criteria

1. TUI launches with no errors in the status bar
2. Runs complete (or fail) within the configured timeout — no zombie runs
3. Completed runs show accurate token counts and durations
4. System tab shows meaningful status (not placeholder "disconnected" / "0.0%" / "(none found)")
5. Claude Code can inspect polkagent state via `--json` CLI commands and direct DB queries
6. TUI snapshot tests exist and run in CI
7. `polkagent debug dump` provides a comprehensive diagnostic snapshot

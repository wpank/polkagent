# Polkagent Diagnostic Findings & Implementation Checklist

**Date:** 2026-08-03
**Updated:** 2026-08-04 (post-D0–P6 verification)
**Method:** 6 parallel exploration agents + live CLI/DB testing + 15 parallel fix agents
**Scope:** Full codebase audit — TUI, CLI, API, run lifecycle, effect pipeline, memory, eval, config, providers

---

## Executive Summary

A comprehensive audit of the polkagent codebase (74 crates) uncovered **70+ issues** across 9 subsystems.

### Fix Status (updated 2026-08-04)

Initial pass: **15 parallel fix agents** applied corrections across **33 files** (+636/-320 lines). Subsequent tasks D0–P6 fixed 20 additional items. **84/107 findings resolved (79%)**.

| Category | Found | Fixed | Remaining |
|----------|-------|-------|-----------|
| SQL Schema Mismatches | 17 | 17 | 0 |
| Run Lifecycle | 12 | 9 | 3 (TimeoutEnforcer, per-run timeout persistence, completed_at gap) |
| TUI Rendering | 19 | 16 | 3 (ScrollToBottom hardcode, FTS SQL injection, global a/d keys) |
| CLI Commands | 10 | 8 | 2 (output.rs dead, exit_codes dead) |
| REST/WS API | 14 | 7 | 7 (lower priority) |
| Effect Pipeline | 10 | 9 | 1 (idempotency O(N) scan) |
| Config & Providers | 12 | 10 | 2 (config merge, find_project_config boundary) |
| Memory System | 7 | 3 | 4 (lower priority) |
| Eval Framework | 6 | 5 | 1 (zero-check score inflation or min_delta noise) |

**Key fixes applied:**
- `polkagent logs` command now works (was crashing with `no such column: event_type`)
- TUI audit tab no longer errors on every render
- TUI sparkline/gauge widgets now populated via `recompute_widget_data()`
- TUI approve/deny now inserts `effect_outcomes` rows (was incorrectly updating `claimed_by`)
- Memory search '/' key now wired and functional
- Run timeout now transitions to `TimedOut` state (was incorrectly using `Failed`)
- Startup reaper added for zombie runs (`lifecycle::recover_stuck_runs`)
- Event sequence numbering fixed (was hardcoded to 0)
- API route ordering fixed (`/events/stream` before `/events/{id}`)
- Health endpoints now use real health checks (was hardcoded 200 OK)
- Cross-agent memory search now gives clear error instead of silent empty results
- Eval report underflow fixed with `saturating_sub`
- Effect pipeline recovery now applies actions automatically

---

## Table of Contents

1. [SQL Schema Mismatches (17 findings)](#1-sql-schema-mismatches)
2. [Run Lifecycle & State Management (12 findings)](#2-run-lifecycle--state-management)
3. [TUI Rendering & UX (19 findings)](#3-tui-rendering--ux)
4. [CLI Command Issues (10 findings)](#4-cli-command-issues)
5. [REST/WebSocket API (14 findings)](#5-restwebsocket-api)
6. [Effect Pipeline (10 findings)](#6-effect-pipeline)
7. [Config & Provider System (12 findings)](#7-config--provider-system)
8. [Memory System (7 findings)](#8-memory-system)
9. [Eval Framework (6 findings)](#9-eval-framework)
10. [Dead Code (16 findings)](#10-dead-code)
11. [Implementation Checklist](#11-implementation-checklist)

---

## 1. SQL Schema Mismatches

The production schema is in `crates/polkagent-store-sqlite/src/schema.sql`. Many SQL queries across the codebase reference old/wrong column names or non-existent tables.

### 1.1 `run_events` column rename not propagated (5 files)

The `run_events` table uses `kind`, `data_json`, `timestamp` — but queries use the old names `event_type`, `payload_json`, `created_at`.

| File | Lines | Operation |
|------|-------|-----------|
| `tui/db.rs` | 500–503 | SELECT in `audit_events()` |
| `tui/db.rs` | 570 | WHERE in `recent_error_count()` |
| `commands/logs.rs` | 127, 135, 145, 153 | All 4 SELECT variants |
| `commands/inbox.rs` | 209–210, 286–287 | INSERT for approve/deny |
| `commands/export.rs` | 692–693 | SELECT in `export_events()` |

### 1.2 Non-existent tables referenced

| File | Lines | Table | Reality |
|------|-------|-------|---------|
| `commands/export.rs` | 563–565 | `effects` | Should be `effect_intents` |
| `commands/export.rs` | 737–739 | `audit_log` | No such table exists |
| `event_store.rs` | 100–327 | `durable_events`, `diagnostic_events`, `global_sequence_counter` | None exist in schema |

### 1.3 Wrong columns on existing tables

| File | Lines | Table | Wrong Columns | Correct Columns |
|------|-------|-------|--------------|-----------------|
| `store.rs` | 1481–1482 | `effect_intents` | `state`, `retry_class` | Not in schema |
| `store.rs` | 1875–1877 | `effect_attempts` | `worker_id`, `payload_json` | Not in schema |
| `store.rs` | 1940–1943 | `effect_outcomes` | `attempt_id`, `run_id`, `consumed` | Not in schema |
| `commands/export.rs` | 518–520 | `runs` | `prompt` | Should use `params_json` |
| `commands/export.rs` | 623–624 | `artifacts` | `name`, `mime_type` | Should be `kind`, `metadata_json` |

### 1.4 `skills` table never created by any migration

| File | Lines | Operation |
|------|-------|-----------|
| `commands/skill.rs` | 48–64, 155–163, 205, 287, 316–319 | SELECT/INSERT/DELETE on `skills` table |

---

## 2. Run Lifecycle & State Management

### 2.1 Critical: All events emitted with hardcoded sequence=0
- **File:** `manager.rs:386`
- Every `emit_event` call passes `sequence = 0`, causing `NonMonotonicSequence` errors after the first event per run. The durable event log is permanently broken.

### 2.2 Critical: No crash recovery / startup reaper for zombie runs
- **File:** `lifecycle.rs:148`
- No startup sweep transitions stuck `Running`/`Queued`/`Completing` runs to `Failed`. Zombie runs accumulate permanently.
- **Confirmed live:** 3 runs stuck in `running` since Aug 2, 1 stuck in `queued` since Aug 1.

### 2.3 High: Orchestrator task errors don't persist Failed state
- **File:** `app.rs:827–873`
- When `execute_run` returns `Err`, only the in-memory event bus gets a `RunFailed` event. The DB state is never updated. Panics leave runs in `Running` forever.

### 2.4 High: Timeout transitions to Failed instead of TimedOut
- **File:** `orchestrator.rs:574–584`
- Wall-clock timeout calls `fail_run` → `RunState::Failed` instead of the dedicated `TimedOut` state. Queries for timed-out runs find nothing.

### 2.5 High: Token usage never persisted to DB
- **File:** `orchestrator.rs:454, 719`
- `total_usage` is in-memory only. `_completed_turn` result is discarded (never written to `turns` table). The `runs` table has no token columns.
- **Confirmed live:** `SELECT * FROM turns` returns 0 rows despite 4 completed runs.

### 2.6 High: AwaitingApproval/WaitingEffect states lose payload after DB read
- **File:** `manager.rs:430–438`
- `request_id` and `pending_intent_ids` are always empty strings/vecs after a reload. Approval routing breaks after process restart.

### 2.7 High: TimeoutEnforcer is dead code
- **File:** `timeout.rs` (all)
- Never instantiated in orchestrator or manager. The `runs` table has no `started_at` column. Runs blocked in approval/effect states can wait indefinitely.

### 2.8 Medium: Per-run timeout resets on process restart
- **File:** `orchestrator.rs:474–478`
- `Instant`-based deadline. Not persisted. Fresh timeout from every restart.

### 2.9 Medium: completed_at not set if crash between Completing→Completed
- **File:** `run_store_impl.rs:187–193`

### 2.10 Medium: Duplicate terminal events emitted
- **File:** `app.rs:832–854`
- Orchestrator emits `RunCompleted`, then AppService emits another one with hardcoded sequence=3.

### 2.11 Medium: CLI timeout cancels as Cancelled not TimedOut
- **File:** `run.rs:239–247`

### 2.12 Low: Schema comment uses stale state names
- **File:** `schema.sql:43`

---

## 3. TUI Rendering & UX

### 3.1 Critical: Approve/deny writes wrong column — effects never actually approved
- **File:** `tui/db.rs:612–648`
- Sets `claimed_by = 'tui-approved'` (the claim column) instead of inserting an `effect_outcomes` row. The effect remains pending from the daemon's perspective.

### 3.2 High: `recompute_widget_data()` never called — sparkline/gauge always empty
- **File:** `state.rs:514`, `app.rs` (missing call)
- `token_history`, `context_used`, `context_total` are never populated at runtime.

### 3.3 High: `error_count` and `budget_remaining` never populated
- **File:** `app.rs:696–773`, `db.rs:550,568`
- `recent_error_count()` and `budget_status()` are dead code. Dashboard errors always shows "0".

### 3.4 High: Audit log always empty (wrong column names in query)
- **File:** `db.rs:502–548`
- See §1.1. Also: `severity` is hardcoded to `"info"` for every event, making the severity filter useless.

### 3.5 High: Memory search `'/'` key documented but not implemented
- **File:** `input.rs`, `memory.rs:16`, `status_bar.rs:95`
- No `TuiAction::StartSearch` variant. `memory_search_query` is never mutated. The search bar is permanently inert.

### 3.6 Medium: Scroll `visible=20` hardcoded — broken on short terminals
- **File:** `app.rs:327, 379`
- Selection disappears below fold on terminals < 20 content rows.

### 3.7 Medium: Footer overlaps border in agents/approvals views
- **File:** `agents.rs:149–169`, `approvals.rs:204–219`
- Footer paragraph rendered on the bottom border row, destroying the frame.

### 3.8 Medium: ScrollToBottom/ScrollToTop hardcode 20 visible rows
- **File:** `app.rs:607, 614`

### 3.9 Medium: SQL injection risk in memory FTS query
- **File:** `db.rs:408–430`
- Uses `format!` string interpolation instead of parameterized queries.

### 3.10 Medium: Wide dashboard renders health panel twice
- **File:** `dashboard.rs:122–148`
- Health shown both as sidebar (`cols[2]`) and full-width panel (`rows[1]`).

### 3.11 Medium: `'a'`/`'d'` keys fire globally, not just on Approvals tab
- **File:** `input.rs:173–174`
- State mutation is guarded but the actions are dispatched from any tab.

### 3.12 Low: Timeline "No run selected" UX gap
- **File:** `app.rs:277–282`
- No prompt telling user to select a run from F3 first.

### 3.13 Low: `stat()` syscall on hot render path
- **File:** `system.rs:152–156`
- DB size checked via `std::fs::metadata` on every frame.

### 3.14 Low: RSS always "unknown" on macOS
- **File:** `system.rs:398–415`
- `process_rss_kb()` is `#[cfg(target_os = "linux")]` only.

### 3.15 Low: Run detail widgets silently hidden below 14 rows
- **File:** `run_detail.rs:96–107`

### 3.16 Low: `tab_bar` widget has stale 7-entry list (actual: 8 tabs)
- **File:** `tab_bar.rs:29–37`
- Dead code, but wrong labels and wrong F-key mappings.

### 3.17 Low: Context gauge label creates bg color discontinuity
- **File:** `context_gauge.rs:109–124`

### 3.18 Low: Zero-height area possible in widget strip sub-splits
- **File:** `dashboard.rs:396–530`

### 3.19 Low: InputMode::Insert/Command never actually activated
- **File:** `input.rs:111–117`

---

## 4. CLI Command Issues

### 4.1 Critical: `polkagent logs` command broken (confirmed live)
- Same BUG-01 schema mismatch. Running `polkagent logs --lines 20` fails with `no such column: event_type`.

### 4.2 Critical: `export.rs` and `inspect.rs` not registered in mod.rs or cli.rs
- Both files contain substantial code but are not compiled. Completely dead.

### 4.3 Critical: `memory` commands use `Handle::current()` — works only inside tokio runtime
- **File:** `memory.rs:43`
- Should build own runtime like `eval.rs:82–85` does.

### 4.4 High: `explain` command is a permanent stub
- **File:** `explain.rs:14–32`
- Always prints "Extrinsic decode not yet connected to chain adapter".

### 4.5 High: All 4 `chain` subcommands are stubs
- **File:** `chain.rs:27–108`
- All return "Chain adapter not yet connected".

### 4.6 High: `inspect` handlers are always-error stubs
- **File:** `inspect.rs:231–431`
- `inspect_run`, `inspect_effect`, `inspect_artifact`, `inspect_agent` always bail with "not found".

### 4.7 High: `skill` commands query non-existent `skills` table
- **File:** `skill.rs:48–319`
- `update`, `show`, `remove` will hard-fail. `list` degrades gracefully.

### 4.8 Medium: `output.rs` — `format_output` never called by any handler
- **File:** `output.rs:99`
- `OutputFormat` flag is parsed then immediately discarded with `let _ = format;` in main.rs:109.

### 4.9 Medium: `exit_codes.rs` — all 5 exit code constants unused
- **File:** `exit_codes.rs:16–28`

### 4.10 Medium: `run.rs load_config()` bypasses ConfigLoader
- **File:** `run.rs:332–350`
- No env-var overrides, no layered merge, silent parse errors. See §7.1.

---

## 5. REST/WebSocket API

### 5.1 High: `/events/stream` potentially shadowed by `/events/{id}`
- **File:** `routes/mod.rs:207–209`
- Static literal registered after capture segment. May route `stream` to `get_event("stream")`.

### 5.2 High: `/health/ready` and `/health/startup` always return 200 OK
- **File:** `routes/mod.rs:145–152`
- Hardcoded inline closures ignore `HealthState`. Real handlers are dead code.

### 5.3 Medium: Metrics double-count — `runs_completed()` called on create
- **File:** `routes/runs.rs:55–85`
- Every run creation increments both `started` and `completed` counters.

### 5.4 Medium: WebSocket pong timeout logic inverted
- **File:** `routes/ws.rs:479–502`
- Measures time since pong was received, not since ping was sent.

### 5.5 Medium: `start_agent` doesn't verify agent exists
- **File:** `routes/runs.rs:365–376`
- Always returns 200 OK for any UUID.

### 5.6 Medium: Two incompatible WebSocket endpoints, one undocumented
- **File:** `routes/mod.rs:264`, `openapi.yaml:745`

### 5.7 Medium: `std::sync::Mutex` in async `AppService` context
- **File:** `app.rs:430, 487`
- Safe today but fragile to refactoring.

### 5.8 Medium: Terminal run events hardcoded with sequence=3
- **File:** `app.rs:844, 858`

### 5.9 Low: `effects::list_effects` dead code — `runs::list_run_effects` used instead
- **File:** `routes/effects.rs:74–95`

### 5.10 Low: `get_usage` passes empty string for agent_id
- **File:** `routes/payments.rs:96–101`

### 5.11 Low: WS parse errors silently discarded
- **File:** `routes/ws.rs:513–518`

### 5.12 Low: OpenAPI spec missing `/health`, `/metrics`, `/openapi.json`, `/ws/v1alpha1`
- **File:** `openapi.yaml`

### 5.13 Low: `create_run` doesn't validate `body.input`
- **File:** `routes/runs.rs:44–85`

### 5.14 Low: Approval events emitted for potentially invalid run_id
- **File:** `routes/effects.rs:208–216`

---

## 6. Effect Pipeline

### 6.1 High: Inbox approve/deny ineffective — sets `claimed_by` not `state`
- **File:** `inbox.rs:177, 253`
- Worker `claim_intent` picks by `state = 'pending'`, so approved effects get re-claimed immediately.

### 6.2 High: Recovery is purely advisory — expired-lease intents never transitioned
- **File:** `recovery.rs:129–185`
- `recover_all` returns actions but never applies them. No background reaper.

### 6.3 High: `propose_intent` drops `turn_id` silently
- **File:** `store.rs:1521–1524`
- FK to `turns` is always NULL.

### 6.4 High: `claim_intent` ignores `EffectPriority`
- **File:** `store.rs:1578–1597`
- Always picks oldest by `created_at`. No `priority` column in schema.

### 6.5 Medium: BLAKE3 digest never computed — always `[0u8; 32]`
- **File:** `pipeline.rs:268–269`, `types.rs:440–442`
- EFF-INV-3 integrity guarantee is absent.

### 6.6 Medium: `check_duplicate` does O(N) full-run scan
- **File:** `idempotency.rs:169–205`
- Has an index on `idempotency_key` but never queries it directly.

### 6.7 Medium: InMemoryStore `release_claim` has no resolved-state guard
- **File:** `pipeline.rs:482` (InMemoryStore impl)
- Drop-after-resolve can reset resolved intent to pending in tests.

### 6.8 Medium: `record_attempt_start` is non-atomic read-then-write
- **File:** `store.rs:1862–1896`
- No transaction wrapping the MAX+INSERT. Concurrent recovery can cause UNIQUE violations.

### 6.9 Medium: Inbox `run_events` INSERT silently fails (result discarded)
- **File:** `inbox.rs:208, 285`
- `let _ = writer.execute(...)` swallows schema mismatch errors.

### 6.10 Medium: `record_outcome` conflict detection on wrong dimension
- **File:** `store.rs:1956–1963`

---

## 7. Config & Provider System

### 7.1 Critical: `run.rs load_config()` reimplements config loading wrong
- **File:** `commands/run.rs:332–350`
- No env-var overrides, no layered merge, silent parse errors. Should use `ConfigLoader::new().load()`.

### 7.2 High: Empty API key accepted in `synthesize_providers_from_env`
- **File:** `model_registry.rs:520–535`
- `is_ok()` check doesn't verify non-empty. Provider created with blank key.

### 7.3 High: `try_provider_by_name` uses unknown model slug
- **File:** `commands/run.rs:502`
- `claude-sonnet-4-20250514` not in BuiltInModelCatalog.

### 7.4 High: Gemini silently routed to OpenAiExecutor
- **File:** `commands/run.rs:415–418`
- Wrong base URL, wrong auth header, wrong function-call format. No warning logged.

### 7.5 High: TUI config source detection diverges from actual loader
- **File:** `tui/views/system.rs:418–441`
- Checks `/etc/polkagent/`, `polkagent.toml` (wrong path), `POLKAGENT_CONFIG` (env var loader ignores).

### 7.6 Medium: `merge(Config, Config)` defeats TOML-value merge invariant
- **File:** `loader.rs:242–251`
- Serialized Config has all defaults materialized; overlay defaults clobber base.

### 7.7 Medium: `tool_format` validator allowlist doesn't match ToolFormat enum
- **File:** `validate.rs:313–325`
- Allows `["json","xml","native"]` but enum serializes as `"anthropic_blocks"`, `"open_ai_json"`, etc.

### 7.8 Medium: `find_project_config` walks past repo root
- **File:** `loader.rs:413–421`
- No boundary detection (`.git`, filesystem root). Test is vacuous.

### 7.9 Low: `validate_database` double-reads env var
- **File:** `validate.rs:137–146`

### 7.10 Low: No validation that `execution.default_provider` references a known provider
- **File:** `validate.rs`

### 7.11 Low: Bad `api.bind_address` skips validation when `api.enabled = false`
- **File:** `validate.rs:329–353`

### 7.12 Info: `POLKAGENT_TUI_THEME` uses stringly-typed match
- **File:** `env.rs:218–242`

---

## 8. Memory System

### 8.1 High: Cross-agent search uses nil UUID — always returns empty
- **File:** `commands/memory.rs:59–70, 127–137`
- `AgentId::from_uuid(Uuid::nil())` never matches any stored memory.

### 8.2 High: `Handle::current()` — see §4.3

### 8.3 Medium: Double-nested row result unwrapping is fragile
- **File:** `sqlite.rs:343–354, 427–434, 662–669, 759–767`

### 8.4 Medium: `?100` sentinel placeholder in FTS query
- **File:** `sqlite.rs:643, 654–656`
- Collides if query accumulates >= 99 dynamic parameters.

### 8.5 Low: FTS5 index inconsistency if `fts_available` changes across opens
- **File:** `sqlite.rs:78–99`

### 8.6 Low: `stats` opens redundant raw connection; `store` argument unused
- **File:** `commands/memory.rs:220–296`

### 8.7 Low: Export count silently capped at `usize::MAX / 2`
- **File:** `service.rs:248–250`

---

## 9. Eval Framework

### 9.1 High: `failed` count underflows (usize panic) when expected_outcome=Error
- **File:** `report.rs:81–83`
- Case counted in both `passed` and `skipped` → `total - passed - skipped` underflows.

### 9.2 Medium: Zero-check cases always score 1.0 — silently inflates suite scores
- **File:** `scorer.rs:178–182`

### 9.3 Medium: Refusal detection misses Unicode apostrophes
- **File:** `scorer.rs:144–153`
- `"I'm unable"` with U+2019 right quote not detected.

### 9.4 Low: `temperature` type mismatch — f32 in runner vs f64 in judge
- **File:** `runner.rs:30`, `judge.rs:54`

### 9.5 Low: `eval run` mixes stdout/stderr for markdown vs summary
- **File:** `commands/eval.rs:128–129`

### 9.6 Low: Default `min_delta=0.0` flags floating-point noise as regressions
- **File:** `regression.rs:76–79`

---

## 10. Dead Code

### 10.1 Entire unregistered modules
| Module | File | Status |
|--------|------|--------|
| `export` | `commands/export.rs` | Not in `mod.rs`, not in `cli.rs` |
| `inspect` | `commands/inspect.rs` | Not in `mod.rs`, not in `cli.rs` |

### 10.2 Unused widgets
| Widget | File | Issue |
|--------|------|-------|
| `tab_bar::render()` | `widgets/tab_bar.rs` | `App::render_tab_bar` duplicates inline |
| `error_digest::render()` | `widgets/error_digest.rs` | Never called; no error view exists |
| `ErrorData` struct | `widgets/error_digest.rs:25` | Never instantiated |

### 10.3 Dead TUI methods
| Method | File:Line | Why Dead |
|--------|-----------|----------|
| `TuiDb::token_usage_history()` | `db.rs:546` | Tests only |
| `TuiDb::recent_error_count()` | `db.rs:567` | Tests only |
| `TuiDb::budget_status()` | `db.rs:585` | Tests only |
| `TuiState::recompute_widget_data()` | `state.rs:514` | Tests only |
| `ScrollState::reset()` | `state.rs:372` | Never called |
| `Tab::next()` / `Tab::prev()` | `app.rs:132,148` | Never called |
| `Tab::ALL` | `app.rs:89` | Never iterated |
| 14 theme helper methods | `theme.rs:213–317` | Zero call sites |

### 10.4 Dead CLI utilities
| Item | File:Line | Why Dead |
|------|-----------|----------|
| `format_output()` | `output.rs:99` | Never called by handlers |
| 5 exit code constants | `exit_codes.rs:16–28` | Never referenced |
| `ChainPoller::is_configured()` | `tui/db.rs:767` | Compiler warning |

---

## 11. Implementation Checklist

### Phase 0: Schema Fix Sprint (P0 — unblocks everything) ✅ COMPLETE

- [x] **Fix `run_events` column names** in `tui/db.rs` (lines 500, 570) — rename `event_type`→`kind`, `payload_json`→`data_json`, `created_at`→`timestamp`
- [x] **Fix `run_events` column names** in `commands/logs.rs` (lines 127, 135, 145, 153)
- [x] **Fix `run_events` column names** in `commands/inbox.rs` (lines 209, 286)
- [x] **Fix `run_events` column names** in `commands/export.rs` (line 692)
- [x] **Fix test schema** in `tui/db.rs` (lines 954–961) to match production schema
- [x] **Fix `effect_intents`** missing `state`/`retry_class` — add columns via migration or remove from queries in `store.rs`
- [x] **Fix `effect_attempts`** missing `worker_id`/`payload_json` in `store.rs`
- [x] **Fix `effect_outcomes`** missing `attempt_id`/`run_id`/`consumed` in `store.rs`
- [x] **Add missing `skills` table** migration or make `skill.rs` commands handle missing table gracefully
- [x] **Verify**: `cargo test` passes after all schema fixes

### Phase 1: Run Lifecycle Fixes (P0) — 7/8 DONE

- [x] **Fix event sequence numbering** in `manager.rs:386` — query `max_sequence` before emitting
- [x] **Add startup reaper** in `lifecycle.rs` — scan for runs in `running`/`queued`/`completing` and transition to `Failed`
- [x] **Fix orchestrator error path** in `app.rs:827–873` — call `run_manager.fail_run` on error, add panic handler
- [x] **Fix timeout state** in `orchestrator.rs:574–584` — use `TimedOut` state instead of `Failed`
- [x] **Persist token usage** — turn data written via `RunManager::record_turn` → `store.insert_turn`
- [ ] **Wire up `TimeoutEnforcer`** or remove it to reduce dead code
- [x] **Fix CLI timeout** in `run.rs:239–247` — use `TimedOut` not `Cancelled`
- [x] **Fix duplicate events** in `app.rs:832–854` — remove the second event emission

### Phase 2: TUI Critical Fixes (P0) — 6/7 DONE

- [x] **Wire `recompute_widget_data()`** — call it at the end of `refresh_data()` in `app.rs`
- [x] **Wire `recent_error_count()` and `budget_status()`** — call them in `refresh_data()`, populate `error_count` and `budget_remaining`
- [x] **Fix TUI approve/deny** in `db.rs:612–648` — insert `effect_outcomes` row instead of setting `claimed_by`
- [x] **Implement memory search** — add `TuiAction::Search`, wire `'/'` key, mutate `memory_search_query`
- [ ] **Fix audit severity** — derive from event `kind` instead of hardcoding `"info"`
- [x] **Fix scroll visible count** — compute from actual terminal height via `inner.height.saturating_sub(1)`
- [x] **Fix footer overlap** — use `inner.y + inner.height.saturating_sub(1)` with bounds check

### Phase 3: CLI Command Fixes (P1) — 3/6 DONE

- [x] **Wire `export.rs` and `inspect.rs`** into `commands/mod.rs` and `cli.rs`
- [x] **Fix `memory.rs` runtime** — uses `tokio_handle()` instead of `Handle::current()`
- [x] **Fix `run.rs load_config()`** — now uses `ConfigLoader::new().load()`
- [ ] **Fix `output.rs`** — wire `format_output()` into command handlers or remove dead code
- [ ] **Fix `exit_codes.rs`** — use constants in error paths or remove
- [ ] **Add `--json` flag** to `polkagent doctor`, `polkagent status`, `polkagent agent list` for machine-readable output

### Phase 4: API Fixes (P1) — 5/6 DONE

- [x] **Fix route ordering** — `/events/stream` now registered before `/events/{id}` in `routes/mod.rs`
- [x] **Wire health check handlers** — real `liveness()`, `readiness()`, `startup()` handlers wired
- [x] **Fix metrics** — `runs_started()` called on create; `runs_completed()` only on actual completion
- [x] **Fix WebSocket pong timeout** — now tracks `ping_sent_at` with proper elapsed check
- [x] **Fix `start_agent`** — returns `ApiError::AgentNotFound` if agent doesn't exist
- [ ] **Document `/ws/v1alpha1`** in openapi.yaml or consolidate with `/events/stream`

### Phase 5: Effect Pipeline Fixes (P1) — 6/7 DONE

- [x] **Fix inbox approve/deny** — now inserts `effect_outcomes` row with correct schema
- [x] **Fix inbox column names** — uses `kind`/`data_json`/`timestamp` in run_events INSERT
- [x] **Implement recovery reaper** — `recover_all` calls `apply_recovery_actions()` automatically
- [x] **Fix `propose_intent`** — `turn_id` column persisted in INSERT
- [x] **Fix `claim_intent`** — orders by `priority DESC, created_at ASC`
- [x] **Compute BLAKE3 digest** — `blake3::hash(&result_bytes)` computed on `record_outcome`
- [ ] **Use indexed idempotency lookup** — `WHERE idempotency_key = ?1` instead of full-run scan

### Phase 6: Config & Provider Fixes (P2) ✅ COMPLETE

- [x] **Fix empty API key** in `synthesize_providers_from_env` — now checks `is_ok_and(|v| !v.is_empty())`
- [x] **Fix Anthropic default model** — uses `pc.default_model.clone()` from config, no hardcoded slug
- [x] **Fix Gemini routing** — now uses dedicated `GeminiExecutor::new()`, not silently routed to OpenAI
- [x] **Fix TUI config detection** — calls `polkagent_config::loader::{global_config_path(), find_project_config()}`
- [x] **Fix `tool_format` validator** — allowlist matches ToolFormat enum: `anthropic_blocks`, `open_ai_json`, `gemini_native`, `re_act_text`

### Phase 7: Memory & Eval Fixes (P2) ✅ COMPLETE

- [x] **Fix cross-agent search** — accepts `None` for agent_id to search all agents
- [x] **Fix eval report underflow** — uses `saturating_sub` for `failed` count
- [x] **Fix refusal detection** — normalizes U+2019 and U+2018 Unicode apostrophes before matching
- [x] **Fix eval stdout/stderr mixing** — both markdown and summary sent to stdout via `println!()`

### Phase 8: Dead Code Cleanup (P3) — 2/5 DONE

- [x] **Delete or wire** `tab_bar.rs`, `error_digest.rs` widgets — both deleted
- [x] **Delete or wire** `export.rs`, `inspect.rs` commands — both wired into `mod.rs` and `cli.rs`
- [ ] **Remove stale `#[allow(dead_code)]`** annotations where code is now used
- [ ] **Remove or wire** unused theme helper methods
- [ ] **Clean up** `Tab::next()`/`Tab::prev()`/`Tab::ALL` if navigation stays F-key-based

### Phase 9: Claude Diagnostic Workflow (P2) — 1/6 DONE

- [ ] **Add `polkagent debug dump` command** — dumps DB state, config, run status as JSON
- [ ] **Add `--json` output** to all major CLI commands for machine-readable feedback
- [ ] **Add headless TUI render mode** — render TUI to string buffer for snapshot testing
- [x] **Add TUI snapshot tests** using ratatui `TestBackend` — 19 snapshot tests exist in Phase 2d
- [ ] **Add `polkagent doctor --fix`** — auto-fix common issues (reset zombie runs, clear stale claims)
- [ ] **Document diagnostic workflow** for Claude Code in CLAUDE.md

---

## Live Diagnostic Baseline (2026-08-03)

Commands run and their results:

| Command | Result |
|---------|--------|
| `polkagent doctor` | 11/20 checks passed (API keys not set, signer not configured, daemon not running) |
| `polkagent status` | 1 agent, 3 active runs, 0 pending effects, 0 memory entries |
| `polkagent agent list` | test-codex agent, active |
| `polkagent config show` | Valid config at `.polkagent/polkagent.toml` |
| `polkagent logs --lines 20` | **FAILS** — `no such column: event_type` |
| `cargo build` | Compiles with 4 warnings (1 unused import, 1 unused var, 1 dead code, 1 suggestion) |
| `cargo test` | All 9 test suites pass, 0 failures |
| DB: `SELECT * FROM runs WHERE state = 'running'` | 3 zombie runs (Aug 2) |
| DB: `SELECT * FROM runs WHERE state = 'queued'` | 1 stuck run (Aug 1) |
| DB: `SELECT COUNT(*) FROM turns` | 0 rows |
| DB: `SELECT COUNT(*) FROM run_events` | 0 rows |

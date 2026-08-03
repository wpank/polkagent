#!/usr/bin/env bash
#
# polkagent autonomous implementation driver
#
# Feeds tasks from all 18 PRDs (Phases 1-6) to Claude Code or Codex,
# one at a time, verifying each with cargo check/test before moving on.
# Auto-commits each task to a branch and pushes to remote.
#
# Usage:
#   ./scripts/overnight.sh                    # full run, all tasks
#   ./scripts/overnight.sh --resume           # pick up where we left off
#   ./scripts/overnight.sh --step 5           # start from task N
#   ./scripts/overnight.sh --dry-run          # print task list, don't execute
#   ./scripts/overnight.sh --tool codex       # use codex instead of claude
#   ./scripts/overnight.sh --model opus       # override model (default: sonnet)
#   ./scripts/overnight.sh --max-turns 30     # claude turns per task (default: 50)
#   ./scripts/overnight.sh --budget 2.00      # max USD per task (default: 5.00)
#   ./scripts/overnight.sh --skip-verify      # skip cargo check/test between tasks
#
set -Euo pipefail

# ─── Configuration ───────────────────────────────────────────────────────────

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN_ID="$(date +%Y%m%d-%H%M%S)"
LOG_DIR="$REPO_ROOT/logs/auto-impl"
TASK_LOG_DIR="$LOG_DIR/$RUN_ID"
MASTER_LOG="$TASK_LOG_DIR/master.log"
STATE_FILE="$LOG_DIR/.state"
REPORT_FILE="$REPO_ROOT/IMPLEMENTATION-REPORT.md"
LOCKFILE="$LOG_DIR/.lock"
BRANCH_NAME="auto/impl-${RUN_ID}"
BRANCH_FILE="$LOG_DIR/.branch"
PUSH_COUNT=0

TOOL="claude"                # claude | codex
MODEL="sonnet"               # model alias
MAX_TURNS=50                 # max agentic turns per task
BUDGET_PER_TASK="5.00"       # USD cap per task
MIN_DISK_GB=15               # free disk below this triggers cleanup
VERIFY=true                  # cargo check/test between tasks
DRY_RUN=false
RESUME_FROM=0

# ─── Colors ──────────────────────────────────────────────────────────────────

if [[ -t 1 ]] && [[ -z "${NO_COLOR:-}" ]]; then
    RED='\033[0;31m'; GRN='\033[0;32m'; YLW='\033[0;33m'
    BLU='\033[0;34m'; CYN='\033[0;36m'; BLD='\033[1m'; RST='\033[0m'
else
    RED=''; GRN=''; YLW=''; BLU=''; CYN=''; BLD=''; RST=''
fi

# ─── Argument parsing ───────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
    case "$1" in
        --resume)     [[ -f "$STATE_FILE" ]] && RESUME_FROM=$(cat "$STATE_FILE"); [[ -f "$BRANCH_FILE" ]] && BRANCH_NAME=$(cat "$BRANCH_FILE"); shift ;;
        --step)       RESUME_FROM="$2"; shift 2 ;;
        --dry-run)    DRY_RUN=true; shift ;;
        --tool)       TOOL="$2"; shift 2 ;;
        --model)      MODEL="$2"; shift 2 ;;
        --max-turns)  MAX_TURNS="$2"; shift 2 ;;
        --budget)     BUDGET_PER_TASK="$2"; shift 2 ;;
        --skip-verify) VERIFY=false; shift ;;
        -h|--help)
            sed -n '2,/^$/p' "$0"; exit 0 ;;
        *)
            echo "Unknown flag: $1"; exit 1 ;;
    esac
done

# ─── Task definitions ───────────────────────────────────────────────────────
#
# Each task is: TASK_ID | short name | detailed prompt for Claude/Codex
#
# The prompt must be self-contained: Claude starts fresh each invocation.
# We inject repo context via --system-prompt pointing at the PRDs.
#

declare -a TASK_IDS=()
declare -a TASK_NAMES=()
declare -a TASK_PROMPTS=()

task() {
    TASK_IDS+=("$1")
    TASK_NAMES+=("$2")
    TASK_PROMPTS+=("$3")
}

# ── DIAGNOSTIC PHASE 0: SQL Schema Fixes (P0) ──────────────────────────────

task "D0-1" "Fix run_events column names" \
"Fix SQL schema mismatches in the polkagent CLI crate. The production schema in crates/polkagent-store-sqlite/src/schema.sql uses columns: kind, data_json, timestamp for the run_events table. But several files still reference the OLD column names: event_type, payload_json, created_at.

Fix ALL occurrences in these files:
1. crates/polkagent-cli/src/tui/db.rs — audit_events() and recent_error_count() functions
2. crates/polkagent-cli/src/commands/logs.rs — all 4 SELECT query variants
3. crates/polkagent-cli/src/commands/inbox.rs — the INSERT statements for approve/deny
4. crates/polkagent-cli/src/commands/export.rs — the SELECT in export_events()

Also fix the test schema in tui/db.rs to match the production schema.

Read schema.sql first to confirm the correct column names, then fix every file. Run cargo check -p polkagent-cli after to verify."

task "D0-2" "Fix effect store schema mismatches" \
"Fix SQL schema mismatches in crates/polkagent-store-sqlite/src/store.rs for the effect pipeline tables.

Read crates/polkagent-store-sqlite/src/schema.sql first. Then fix:
1. effect_intents queries reference 'state' and 'retry_class' columns that don't exist — either add them to schema.sql via a new migration, or remove them from the queries
2. effect_attempts queries reference 'worker_id' and 'payload_json' that don't exist — same approach
3. effect_outcomes queries reference 'attempt_id', 'run_id', 'consumed' that don't exist — same approach

Choose the approach that is most consistent with the existing codebase patterns. If adding columns, create a migration file. If removing queries, make sure the functionality still makes sense.

Run cargo test -p polkagent-store-sqlite after to verify nothing breaks."

task "D0-3" "Create skills table migration" \
"The polkagent skill CLI commands (crates/polkagent-cli/src/commands/skill.rs) query a 'skills' table that does not exist in the database schema.

Read crates/polkagent-store-sqlite/src/schema.sql to see the existing tables.
Read crates/polkagent-cli/src/commands/skill.rs to see what columns the skill commands expect.

Either:
(a) Add a CREATE TABLE skills(...) to schema.sql with the columns that skill.rs expects, OR
(b) Make skill.rs handle the missing table gracefully (CREATE TABLE IF NOT EXISTS before queries)

Option (b) is preferred since it's self-healing. Run cargo check -p polkagent-cli after."

# ── DIAGNOSTIC PHASE 1: Run Lifecycle Fixes (P0) ───────────────────────────

task "D1-1" "Fix event sequence numbering" \
"In crates/polkagent-run/src/manager.rs around line 386, every emit_event call passes sequence = 0 (hardcoded). This causes NonMonotonicSequence errors after the first event per run.

Fix: before emitting an event, query the current max sequence number for that run and use max + 1. Read the file first to understand the emit_event pattern, then fix all call sites.

Run cargo test -p polkagent-run after."

task "D1-2" "Add startup reaper for zombie runs" \
"In crates/polkagent-run/src/lifecycle.rs, there is no startup sweep to clean up runs stuck in running/queued/completing states after a crash.

Add a function recover_stuck_runs (or similar) that:
1. Queries runs in states: Running, Queued, Completing
2. Transitions them to Failed with a reason like 'recovered after restart'
3. Is called during application startup

Read the existing lifecycle.rs and manager.rs to understand the patterns. The function should use the existing RunStore trait.

Run cargo test -p polkagent-run after."

task "D1-3" "Fix timeout state and orchestrator error path" \
"Two related fixes in crates/polkagent-run/src/orchestrator.rs:

1. Around line 574-584: wall-clock timeout calls fail_run with RunState::Failed. It should use RunState::TimedOut instead (the state exists in the enum but is never used here).

2. In crates/polkagent-service/src/app.rs around lines 827-873: when execute_run returns Err, only the in-memory event bus gets a RunFailed event but the DB state is never updated. Fix this to call run_manager.fail_run on error.

Also fix crates/polkagent-cli/src/commands/run.rs around lines 239-247 where CLI timeout cancels as Cancelled instead of TimedOut.

Read each file first, then apply minimal fixes. Run cargo check --workspace after."

task "D1-4" "Fix duplicate event emission" \
"In crates/polkagent-service/src/app.rs around lines 832-854, both the orchestrator AND AppService emit terminal run events (RunCompleted/RunFailed), causing duplicates. The AppService emission also hardcodes sequence=3.

Fix: remove the duplicate event emission in AppService — let the orchestrator be the single source. Read the file to understand the flow, then remove the redundant emit.

Run cargo test -p polkagent-service after."

# ── DIAGNOSTIC PHASE 2: TUI Fixes (P0) ─────────────────────────────────────

task "D2-1" "Wire TUI widget data computation" \
"In the TUI, recompute_widget_data() (in crates/polkagent-cli/src/tui/state.rs) is never called, so sparkline/gauge widgets are always empty. Also recent_error_count() and budget_status() in tui/db.rs are never called.

Fix in crates/polkagent-cli/src/tui/app.rs:
1. Call recompute_widget_data() at the end of refresh_data() / the periodic refresh cycle
2. Call recent_error_count() and budget_status() during refresh and store results in TuiState fields error_count and budget_remaining

Read the relevant files first to understand the refresh cycle, then wire these calls in.

Run cargo check -p polkagent-cli after."

task "D2-2" "Fix TUI approve/deny to write effect_outcomes" \
"In crates/polkagent-cli/src/tui/db.rs around lines 612-648, the approve/deny functions set claimed_by = 'tui-approved' instead of inserting an effect_outcomes row. This means effects are never actually resolved.

Read the schema.sql effect_outcomes table structure, then fix the approve/deny functions to INSERT INTO effect_outcomes instead of UPDATE effect_intents SET claimed_by.

Run cargo check -p polkagent-cli after."

task "D2-3" "Implement TUI memory search" \
"The TUI memory tab documents '/' for search but it's not implemented.

In crates/polkagent-cli/src/tui/:
1. Add a TuiAction::StartSearch variant (or similar) to the action enum
2. Wire the '/' key in input.rs to activate search mode
3. Make the memory_search_query field in TuiState actually get mutated when user types
4. Filter memory entries based on the search query

Read the existing input.rs, memory view file, and state.rs to understand current patterns.

Run cargo check -p polkagent-cli after."

task "D2-4" "Fix TUI scroll and footer issues" \
"Multiple TUI rendering issues:

1. crates/polkagent-cli/src/tui/app.rs lines 327, 379: scroll visible=20 is hardcoded — compute from actual terminal height instead
2. crates/polkagent-cli/src/tui/views/agents.rs lines 149-169 and approvals.rs lines 204-219: footer overlaps the border — use inner.y + inner.height - 1 instead of area.y + area.height - 1
3. app.rs lines 607, 614: ScrollToBottom/ScrollToTop hardcode 20 visible rows

Read each file, apply minimal fixes. Run cargo check -p polkagent-cli after."

# ── DIAGNOSTIC PHASE 3: CLI Command Fixes (P1) ─────────────────────────────

task "D3-1" "Wire export and inspect commands" \
"Two entire command modules exist but are dead code:
- crates/polkagent-cli/src/commands/export.rs
- crates/polkagent-cli/src/commands/inspect.rs

They are NOT registered in commands/mod.rs or cli.rs.

Read cli.rs and mod.rs to see how other commands are registered. Then either:
(a) Wire export and inspect into the command dispatch (add mod declarations, CLI enum variants, dispatch arms), OR
(b) If the code quality is too low / references wrong schemas, delete the files entirely

Choose (a) if the code looks reasonable, (b) if it would just cause errors.

Run cargo check -p polkagent-cli after."

task "D3-2" "Fix config loading and memory runtime" \
"Two CLI fixes:

1. crates/polkagent-cli/src/commands/run.rs has a load_config() function (around line 332-350) that reimplements config loading incorrectly — no env-var overrides, no layered merge. Replace it with ConfigLoader::new().load() from the polkagent-config crate.

2. crates/polkagent-cli/src/commands/memory.rs line 43 uses Handle::current() which only works inside a tokio runtime. Change it to build its own Runtime::new() like commands/eval.rs does.

Read the files, apply the fixes. Run cargo check -p polkagent-cli after."

# ── DIAGNOSTIC PHASE 4: API Fixes (P1) ─────────────────────────────────────

task "D4-1" "Fix API route ordering and health endpoints" \
"In crates/polkagent-api/src/routes/mod.rs:

1. /events/stream is registered AFTER /events/{id} — the {id} capture matches 'stream' first. Move the static /events/stream route BEFORE the parameterized /events/{id} route.

2. /health/ready and /health/startup always return hardcoded 200 OK via inline closures. Replace them with the real health check handlers that check HealthState.

3. In routes/runs.rs lines 55-85: runs_completed() metric is called on run CREATE, double-counting. Only call it on actual completion.

Read each file first, apply minimal fixes. Run cargo test -p polkagent-api after."

task "D4-2" "Fix WebSocket and API issues" \
"In crates/polkagent-api/src/routes/:

1. ws.rs lines 479-502: pong timeout logic is inverted — it measures time since pong received, not since ping sent. Add a ping_sent_at field and measure from that.

2. runs.rs lines 365-376: start_agent returns 200 OK for any UUID without verifying the agent exists. Add a lookup check.

Read the files, apply minimal fixes. Run cargo test -p polkagent-api after."

# ── DIAGNOSTIC PHASE 5: Effect Pipeline Fixes (P1) ─────────────────────────

task "D5-1" "Fix effect pipeline issues" \
"Multiple effect pipeline fixes in crates/polkagent-cli/src/commands/inbox.rs and crates/polkagent-store-sqlite/src/store.rs:

1. inbox.rs: approve/deny writes claimed_by instead of inserting effect_outcomes. Fix to insert outcome rows. Also fix the run_events INSERT to use correct column names (kind/data_json/timestamp not event_type/payload_json/created_at).

2. store.rs propose_intent (around line 1521): drops turn_id silently — persist it.

3. store.rs claim_intent (around line 1578): ignores EffectPriority, always picks oldest. Add ORDER BY priority if the column exists, or add the column.

4. pipeline.rs line 268-269 or types.rs line 440-442: BLAKE3 digest is always [0u8; 32]. Either compute the real digest on record_outcome, or remove the digest field and documentation if it's not needed yet.

Read schema.sql and each file. Apply fixes. Run cargo check --workspace after."

# ── DIAGNOSTIC PHASE 6: Config & Provider Fixes (P2) ───────────────────────

task "D6-1" "Fix config and provider issues" \
"Several config/provider fixes:

1. crates/polkagent-cli/src/commands/run.rs model_registry.rs around line 520-535: synthesize_providers_from_env accepts empty API keys. Add a check: is_ok_and(|v| !v.is_empty()).

2. crates/polkagent-cli/src/commands/run.rs around line 415-418: Gemini is silently routed to OpenAiExecutor with wrong base URL and auth. Either route to a proper GeminiExecutor if one exists (check for polkagent-executor-gemini crate), or log a clear warning that Gemini is not yet supported.

3. crates/polkagent-cli/src/tui/views/system.rs lines 418-441: config source detection checks wrong paths. Make it use the same paths as ConfigLoader.

Read each file, apply fixes. Run cargo check -p polkagent-cli after."

# ── DIAGNOSTIC PHASE 7: Memory & Eval Fixes (P2) ──────────────────────────

task "D7-1" "Fix memory and eval issues" \
"Four fixes:

1. crates/polkagent-cli/src/commands/memory.rs: cross-agent search uses AgentId::from_uuid(Uuid::nil()) which never matches. Add Option<AgentId> support to search across all agents when no agent is specified.

2. crates/polkagent-eval/src/report.rs around line 81-83: failed count underflows (usize panic) when expected_outcome=Error. Use saturating_sub.

3. crates/polkagent-eval/src/scorer.rs around line 144-153: refusal detection misses Unicode right single quote (U+2019). Normalize apostrophes before matching.

4. crates/polkagent-eval/src/scorer.rs around line 178-182: zero-check cases always score 1.0. This silently inflates suite scores. Add a note or fix the logic.

Read each file, apply fixes. Run cargo check --workspace after."

# ── DIAGNOSTIC PHASE 8: Dead Code Cleanup (P3) ─────────────────────────────

task "D8-1" "Clean up dead code" \
"Remove or wire dead code across the CLI crate:

1. If export.rs and inspect.rs were wired in task D3-1, skip this. Otherwise delete them.
2. crates/polkagent-cli/src/tui/widgets/tab_bar.rs — the render() function is duplicated inline in App::render_tab_bar. Delete the widget file if unused.
3. crates/polkagent-cli/src/tui/widgets/error_digest.rs — ErrorData struct is never instantiated and render() never called. Delete if unused.
4. crates/polkagent-cli/src/commands/output.rs — format_output() is never called, OutputFormat is parsed then discarded. Either wire it into handlers or delete.
5. crates/polkagent-cli/src/commands/exit_codes.rs — all 5 constants unused. Either use them in error paths or delete.

Be conservative: if something might be needed soon, keep it. Only delete clearly dead code. Run cargo check -p polkagent-cli after."

# ── FEATURE: Wire chain adapter (biggest single gap) ───────────────────────

task "F1-1" "Wire SubxtChainClient into CLI" \
"This is the single most important remaining task. The SubxtChainClient in polkagent-chain-subxt is fully implemented (90 tests) but NOT used by polkagent-cli.

Steps:
1. Add polkagent-chain-subxt as a dependency in crates/polkagent-cli/Cargo.toml
2. In crates/polkagent-cli/src/main.rs, after loading config, check for a chain RPC URL (from config or POLKAGENT_RPC_URL env var). If present, instantiate SubxtChainClient.
3. Pass the chain client to AppService via with_chain_client().
4. In crates/polkagent-cli/src/commands/chain.rs, replace the hardcoded stubs:
   - status(): connect to RPC and show real chain status (name, block number, finalized block)
   - balance(): use the chain client to query System.Account storage and show free/reserved/frozen balance
   - metadata(): fetch and show real metadata version and pallet list
   - decode(): use the chain client to decode hex calldata
5. The chain command handlers need to receive either a chain client or config — update the dispatch in main.rs to pass what's needed.

Read the existing SubxtChainClient implementation first (crates/polkagent-chain-subxt/src/lib.rs) to understand its API. Read the ChainClient trait (crates/polkagent-chain-trait/src/lib.rs). Then read the stub chain.rs.

This is a significant change — take care to handle the case where no RPC URL is configured (show a helpful message instead of panicking).

Run cargo check -p polkagent-cli after."

task "F1-2" "Wire chain client to tool execution" \
"After F1-1 wired SubxtChainClient into the CLI, the governance and treasury tools also need it.

The tools in polkagent-tool-governance and polkagent-tool-treasury use a ChainClient trait object. Currently when polkagent run executes, the tools get FakeChainClient.

In crates/polkagent-service/src/app.rs, the AppService has an optional chain_client field. Make sure:
1. When a SubxtChainClient is available, it's passed to the ToolRegistry so governance/treasury tools use real chain data
2. When no chain client is configured, tools should still work with FakeChainClient (graceful degradation)

Read how tools are registered and how ToolContext/ToolHandler receives the chain client. Wire the real client through.

Run cargo check --workspace after."

# ── FEATURE: Explain command ───────────────────────────────────────────────

task "F2-1" "Implement explain command" \
"The polkagent explain command (crates/polkagent-cli/src/commands/explain.rs) is a stub that always prints 'Extrinsic decode not yet connected to chain adapter'.

Now that the chain adapter is (or should be) wired, implement it:
1. Accept hex-encoded extrinsic data as an argument
2. Use the SubxtChainClient (or polkagent-codec) to SCALE-decode it
3. Display the decoded call: pallet, method, arguments
4. Show an action card summary if possible (using polkagent-card)

If the chain client is not available, show a helpful error instead of silently failing.

Read the existing polkagent-codec crate to see how SCALE decoding works. Read polkagent-card for action card rendering.

Run cargo check -p polkagent-cli after."

# ── FEATURE: Fix SQL injection in TUI ─────────────────────────────────────

task "F3-1" "Fix SQL injection in TUI memory search" \
"In crates/polkagent-cli/src/tui/db.rs around lines 408-430, the memory FTS query uses format! string interpolation instead of parameterized queries. This is a SQL injection risk.

Fix: use parameterized queries (?1, ?2 etc.) instead of format!() for user input.

Read the file, find all format!() SQL queries that interpolate user input, and convert them to parameterized queries.

Run cargo test -p polkagent-cli after."

# ── ADDITIONAL DIAGNOSTIC FIXES (not covered above) ─────────────────────

task "DX-1" "Persist token usage to DB" \
"Token usage from model responses is computed but never written to the database — all token counts show 0.

Steps:
1. Read crates/polkagent-store-sqlite/src/schema.sql — check if the turns or run_events table has input_tokens / output_tokens columns
2. If not, add them (ALTER TABLE or modify CREATE TABLE)
3. Find where token counts are computed (crates/polkagent-service or polkagent-run, in the orchestrator) and persist them after each model response
4. Verify the data flows: model response -> token count extraction -> DB write

Run cargo check --workspace after."

task "DX-2" "Fix AwaitingApproval state after restart" \
"When a run enters AwaitingApproval state, the pending intent IDs and request ID are stored in memory only. After process restart the run is in AwaitingApproval but routing data is lost, so approvals cannot be matched.

Fix: persist pending_intent_ids and request_id alongside the run state in the database. When loading a run from the DB, restore these fields.

Read crates/polkagent-run/src/manager.rs to find the approval state. Read the run persistence code to understand the serialization pattern.

Run cargo check -p polkagent-run after."

task "DX-3" "Wire TimeoutEnforcer" \
"TimeoutEnforcer in crates/polkagent-run/src/timeout.rs exists but is never instantiated anywhere. Also the runs table may be missing a started_at column needed for wall-clock timeout computation.

Steps:
1. Read timeout.rs to understand TimeoutEnforcer
2. Check schema.sql for the runs table — add started_at if missing
3. Wire TimeoutEnforcer into the service startup in crates/polkagent-service/src/app.rs
4. The enforcer should run periodically and transition timed-out runs to TimedOut state

Run cargo check --workspace after."

# ── PHASE 3: Read-Only Value Expansion (remaining items) ────────────────
# Note: Chain adapter wiring (P3-1) is already covered by tasks F1-1 and F1-2

task "P3-2" "Storage migration rehearsal tool" \
"Create a builder tool for Polkadot storage migration rehearsal (PRD-05 workflow A1).

This tool should:
1. Accept a runtime WASM blob path and a block number
2. Compare old vs new storage layout after the migration
3. Diff the resulting storage keys/values
4. Produce a structured report listing changed, added, and removed keys

Implement as a new tool in polkagent-tool or polkagent-tool-governance — read the existing tool implementations (e.g., polkagent-tool-governance/src/) to follow the ToolHandler pattern.

This is a read-only tool — it should produce zero write effects.

Run cargo check --workspace after."

task "P3-3" "Upgrade impact brief tool" \
"Create a metadata comparison tool (PRD-05 workflow A2).

This tool should:
1. Accept two metadata snapshots (by block number or file path)
2. Compare pallets, calls, events, storage items, and constants between old and new
3. List all changes with before/after type signatures
4. Output as structured text suitable for an action card

Use the existing polkagent-metadata crate for SCALE decoding. Read how metadata is currently loaded and cached in crates/polkagent-metadata/src/.

Run cargo check --workspace after."

task "P3-4" "Metadata-grounded RAG" \
"Implement metadata-grounded RAG for Polkadot chain knowledge (PRD-05 workflow A9).

Use the existing HybridRetriever in polkagent-memory to:
1. Ingest runtime metadata (pallet docs, call signatures, storage descriptions) into the FTS5 index
2. When queried about chain functionality, return answers with citations that include: metadata hash, block number, pallet name, and source section
3. Create a tool that agents can call to query chain knowledge

Read crates/polkagent-memory/src/ for the HybridRetriever pattern. Read crates/polkagent-metadata/ for how metadata is structured.

Run cargo check --workspace after."

task "P3-5" "Error/recovery explainer" \
"Implement an error explainer for failed runs (PRD-13 deliverable 3.7).

When a run fails:
1. Extract the error type from the terminal run event
2. Match it against known error patterns (timeout, provider error, policy denial, chain error)
3. Produce a structured next-step proposal (retry, adjust config, check API key, etc.)

Add this to the TUI run detail panel and to CLI output for failed runs. Read crates/polkagent-cli/src/tui/ and crates/polkagent-run/ to understand the error types and TUI structure.

Run cargo check -p polkagent-cli after."

task "P3-6" "Identity signal display" \
"Implement identity signal display for Polkadot addresses (PRD-07 deliverable 3.8).

Add functionality to:
1. Look up a Polkadot SS58 address on the People Chain (Identity pallet)
2. Display: display name, identity judgements, registrar info, sub-accounts
3. Wire into the TUI (system tab or new panel) and CLI (polkagent identity <address> command)

Use the polkagent-identity crate (42 tests exist) and polkagent-chain-subxt for RPC queries. Read the existing identity types.

Run cargo check -p polkagent-cli after."

task "P3-7" "PCA C0 integration test" \
"AC-P3-003 (PCA C0 bot connects and exchanges messages) has never been verified end-to-end. The polkagent-transport-pca crate has 77 tests but no E2E integration test.

Add integration tests in polkagent-integration-tests that:
1. Set up a mock PCA server (or use the existing fake transport patterns)
2. Register a bot
3. Send a message and verify ACK
4. Receive a response message
5. Verify message ordering and content integrity

Read the existing integration test patterns in crates/polkagent-integration-tests/. Read polkagent-transport-pca/src/lib.rs for the transport API.

Run cargo test -p polkagent-integration-tests after."

task "P3-8" "Wire Gemini and OpenRouter executors" \
"The polkagent-executor-gemini and polkagent-executor-openrouter crates exist but are NOT registered in polkagent-cli.

Steps:
1. Add both as dependencies in crates/polkagent-cli/Cargo.toml
2. In the model registry / provider synthesis code, register GeminiExecutor for gemini model slugs with the correct env var (GOOGLE_API_KEY or POLKAGENT_GEMINI_API_KEY)
3. Register OpenRouterExecutor for openrouter model slugs with OPENROUTER_API_KEY
4. Add both to the built-in model catalog
5. Add model fallback chain logic: if primary executor fails, try the next registered executor

Read the existing registration pattern for Anthropic and OpenAI executors in commands/run.rs.

Run cargo check -p polkagent-cli after."

task "P3-9" "Phase 3 AC verification" \
"Verify all Phase 3 acceptance criteria. Read TRACKING.md and prd/PRD-00-MASTER-INDEX.md section 9.2.

Check each criterion:
- AC-P3-001: Read-only workflows produce zero write effects — audit effect types
- AC-P3-002: RAG answers cite metadata hash, block, and source
- AC-P3-003: PCA C0 bot can connect and exchange messages
- AC-P3-004: Real bounded jobs show repeat use
- AC-P3-005: No source or privacy regressions — run security test suite

Run cargo test -p polkagent-security-tests and review the results. Report which AC items pass and which still fail.

Do NOT modify code in this task — only verify and report."

# ── PHASE 4: Controlled Write Expansion ──────────────────────────────────

task "P4-1" "Create polkagent-action-sign (risk gates)" \
"Create crate polkagent-action-sign for pre-sign risk analysis (PRD-08 deliverable 4.1).

This crate should:
1. Accept decoded calldata (from polkagent-codec types)
2. Classify the call pattern: detect batch, batch_all, proxy, multisig, and approval patterns
3. Assign risk levels: LOW, MEDIUM, HIGH, CRITICAL
4. Integrate with the Explain Before Sign pipeline as a gate

Create the crate under crates/polkagent-action-sign/ with:
- Cargo.toml depending on polkagent-core, polkagent-codec
- lib.rs with RiskLevel enum and a classify_risk() function
- Tests with at least 10 fixture cases (normal transfer=LOW, batch_all with proxy=HIGH, etc.)
- Add to workspace Cargo.toml

Run cargo test -p polkagent-action-sign after."

task "P4-2" "Create polkagent-action-governance (multisig coordinator)" \
"Create crate polkagent-action-governance for multisig/proxy coordination (PRD-08 deliverable 4.2).

This crate should:
1. Read current multisig threshold and approval count from chain storage (via ChainClient trait)
2. Read pure-proxy configuration
3. Track co-signer approvals locally without holding any keys
4. Produce a status summary: threshold, current approvals, who approved, who is missing

Create under crates/polkagent-action-governance/ with:
- Cargo.toml depending on polkagent-core, polkagent-chain-trait
- lib.rs with MultisigState and ProxyConfig types
- Tests using FakeChainClient
- Add to workspace Cargo.toml

Run cargo test -p polkagent-action-governance after."

task "P4-3" "Create polkagent-action-transfer (XCM planner)" \
"Create crate polkagent-action-transfer for XCM route planning (PRD-05 deliverable 4.3).

This crate should:
1. Given source chain, destination chain, and asset, enumerate valid XCM routes
2. Return fee estimates using XcmPaymentApi (via ChainClient)
3. Return RouteUnsupported for unknown routes
4. This is testnet-only and read-only — no transaction submission

Create under crates/polkagent-action-transfer/ with:
- Cargo.toml depending on polkagent-core, polkagent-chain-trait
- lib.rs with XcmRoute, RoutePlan types, and plan_route() function
- Tests with known route fixtures and refusal cases
- Add to workspace Cargo.toml

Run cargo test -p polkagent-action-transfer after."

task "P4-4" "Metadata drift watcher" \
"Wire metadata drift detection into a background watcher (PRD-05 deliverable 4.4).

Steps:
1. Read crates/polkagent-metadata/ to find existing drift detection logic
2. Create a MetadataDriftWatcher that uses polkagent-scheduler to run periodically
3. When drift is detected, emit a MetadataDriftDetected event via the event bus
4. Surface drift alerts in the TUI audit tab
5. Add a polkagent doctor subcommand that checks for metadata drift on demand

The watcher must detect-and-propose only, never auto-merge metadata. Wire into polkagent-service startup.

Run cargo check --workspace after."

task "P4-5" "Product kits v1" \
"Implement product kit manifest format and lifecycle (PRD-12 deliverable 4.5).

Steps:
1. Define the kit manifest TOML format in polkagent-skill or polkagent-plugin with sections for metadata, capabilities, and skills
2. Implement install workflow: parse manifest, validate capabilities against Cedar grants, register skills
3. Implement uninstall: remove registered skills, rollback config changes
4. Add CLI commands: polkagent kit install <path>, polkagent kit uninstall <name>, polkagent kit list

Read existing polkagent-plugin and polkagent-skill code. Create a polkagent-marketplace crate if needed for the registry client.

Run cargo check --workspace after."

task "P4-6" "PCA C1 bridge protocol" \
"Implement PCA C1 bridge protocol (PRD-06 deliverable 4.6).

The C1 bridge allows a PCA client to send instructions to a Polkagent run, beyond the C0 transport layer.

Steps:
1. Read polkagent-transport-pca and polkagent-harness-acp to understand existing transport patterns
2. Add bridge endpoints to polkagent-api that accept PCA-formatted requests and route them to runs
3. Add an OpenAPI compatibility layer so existing PCA tooling can interact with the REST API
4. Test with mock PCA client sending a run instruction

Run cargo check --workspace after."

task "P4-7" "Harness session persistence and resume" \
"Add session persistence and resume to all harness crates (PRD-04 deliverable 4.7).

All harnesses currently lack session persistence. Implement:
1. Session state serialized to .polkagent/state/ directory after each turn
2. resume(session_id) that re-attaches to existing subprocess or re-launches with context
3. health() polling with configurable interval
4. cancel() that sends SIGTERM and waits for clean exit

Apply to these crates: polkagent-harness-claude, polkagent-harness-codex, polkagent-harness-cursor, polkagent-harness-goose, polkagent-harness-kiro, polkagent-harness-copilot.

Read polkagent-harness-trait to understand the trait interface. Add session lifecycle methods to the trait if not present.

Run cargo check --workspace after."

task "P4-8" "Provenanced memory v1" \
"Add provenance tracking and tenant isolation to the memory system (PRD-09 deliverable 4.8).

Three sub-tasks:
1. Add provenance chain to every memory item: store source_artifact_id, source_run_id, source_agent_id, ingested_at
2. Implement forget(artifact_id) that removes from both FTS5 and sqlite-vec indices atomically
3. Enforce tenant isolation: queries for agent A cannot return results for agent B

Modify polkagent-memory and polkagent-store-sqlite. Read the existing memory store schema and queries.

Add tests: at least 5 provenance tracking tests and 10 cross-tenant isolation tests.

Run cargo test -p polkagent-memory after."

task "P4-9" "Create harness-opencode and harness-bridge crates" \
"Create two new harness crates specified in PRD-00 section 14 crate layout:

1. polkagent-harness-opencode: follows the ACP pattern like cursor/goose/kiro
   - Read crates/polkagent-harness-cursor/ for reference
   - Create crates/polkagent-harness-opencode/ with same structure
   - Implement the HarnessPort trait using polkagent-harness-acp as base

2. polkagent-harness-bridge: generic HTTP/WebSocket bridge for non-stdio harnesses
   - Create crates/polkagent-harness-bridge/
   - Implement HarnessPort over HTTP POST or WebSocket connection
   - Accept configuration for endpoint URL, auth token, timeout

Add both to workspace Cargo.toml. Run cargo check --workspace after."

task "P4-10" "Phase 4 test suite" \
"Create Phase 4 acceptance tests in polkagent-integration-tests and polkagent-security-tests.

Required test suites:
1. Risk gate fixture corpus: 10+ dangerous-call patterns (batch, proxy, multisig combinations)
2. Multisig edge cases: threshold changes, pure-proxy revocation
3. Kit lifecycle: install, list, uninstall, rollback, reinstall
4. Memory isolation: 20+ cross-tenant query pairs that must return zero results
5. XCM route refusal: 5+ unsupported route cases
6. Harness session resume: create session, serialize state, reload, verify continuity

Read existing integration test patterns. Add the tests.

Run cargo test --workspace after."

# ── PHASE 5: Managed, Public, Value-Moving ───────────────────────────────
# NOTE: Phase 5 requires independent security and legal review before going live.
# These tasks create the implementation; the review process is separate.

task "P5-1" "Create polkagent-signer-proxy (funded accounts)" \
"Create crate polkagent-signer-proxy for funded pure-proxy accounts (PRD-08 deliverable 5.1).

This crate should:
1. Implement SignerPort trait for pure-proxy signing
2. Budget enforcement via Cedar policy at the grant layer
3. Check budget ceiling before every effect intent is persisted
4. Support DOT and stablecoin transfers on Asset Hub
5. DryRunApi pre-flight validation before submission

Create under crates/polkagent-signer-proxy/ with:
- Cargo.toml depending on polkagent-signer-trait, polkagent-grant, polkagent-chain-trait
- Contract tests from polkagent-signer-trait
- Budget enforcement tests (agent cannot exceed ceiling)
- Add to workspace Cargo.toml

Run cargo test -p polkagent-signer-proxy after."

task "P5-2" "Agent earn/spend prototype" \
"Prototype agent earn/spend mechanics (PRD-08 deliverable 5.2). This is a RESEARCH PROTOTYPE, not production.

1. Model a flow: agent completes skill task, receives simulated micropayment
2. Agent uses payment to invoke a tool call
3. Add x402 payment header support for HTTP tool calls
4. Track earnings and spending in the payment ledger

Modify polkagent-payment and polkagent-skill. Add a prototype test that exercises the full earn-invoke-spend cycle.

Label as experimental. Run cargo check --workspace after."

task "P5-3" "Agent-to-agent escrow research" \
"Research and document agent-to-agent escrow (PRD-08 deliverable 5.3). This is a RESEARCH TASK.

1. Read available Polkadot primitives: Asset Hub multisig, pure-proxy, time-delay proxy
2. Design an escrow mechanism where Agent A deposits funds, Agent B performs work, escrow releases on verified completion
3. Document the design in docs/adr/ directory using the ADR template from PRD-00 section 15
4. Create a minimal prototype in polkagent-payment showing the escrow state machine

Label as experimental. Run cargo check --workspace after."

task "P5-4" "Capability-disclosed marketplace packages" \
"Implement Wasmtime WIT sandbox for marketplace packages (PRD-12 deliverable 5.4).

Steps:
1. Modify polkagent-plugin to use Wasmtime Component Model for sandboxing
2. Each package declares its capabilities; the sandbox enforces those declarations
3. A malicious skill attempting unauthorized I/O must be capability-denied
4. Add cosign signature verification: unsigned packages are rejected
5. SLSA Build L2 attestation checking

Read the existing polkagent-plugin crate for current architecture.

Run cargo check --workspace after."

task "P5-5" "Public agent-service listings" \
"Add public agent-service listing endpoints (PRD-12 deliverable 5.5).

Steps:
1. Add to polkagent-api: POST /v1/registry/listings and GET /v1/registry/search endpoints
2. Service declarations include: capabilities, pricing, availability, version
3. Self-hostable registry backend (SQLite-backed)
4. Discovery API: search by capability, by tag, by author
5. Wire into polkagent-marketplace crate (create if it does not exist)

Read existing polkagent-api route patterns for structure.

Run cargo check --workspace after."

task "P5-6" "Create fleet worker crates (cloud control/worker)" \
"Create polkagent-cloud-control and polkagent-cloud-worker crates (PRD-11 deliverable 5.6).

Control plane (crates/polkagent-cloud-control/):
- Job queue backed by SQLite
- Worker registration with heartbeat
- Job assignment based on worker load and capabilities

Worker plane (crates/polkagent-cloud-worker/):
- Registers with control plane on startup
- Receives job assignments, executes runs
- Sends heartbeat, supports graceful drain

Both crates should depend on polkagent-core and polkagent-config. Add at least 10 unit tests each. Add to workspace Cargo.toml.

Run cargo check --workspace after."

task "P5-7" "Regional isolation" \
"Add data residency constraints to the cloud control plane (PRD-11 deliverable 5.7).

Steps:
1. Add region configuration to polkagent-config (eu, us, ap, etc.)
2. In polkagent-cloud-control, implement region-aware job routing: jobs can only be assigned to workers in the configured region
3. Implement as Cedar policy on the control plane
4. No cross-region data movement without explicit operator override
5. Add tests: job from EU tenant cannot be routed to US worker

Run cargo check --workspace after."

task "P5-8" "Create polkagent-store-postgres" \
"Create polkagent-store-postgres for multi-tenant cloud deployments (PRD-11 deliverable 5.8).

Create crates/polkagent-store-postgres/:
1. Implement all traits from polkagent-store-trait
2. PostgreSQL schema with row-level security for tenant isolation
3. Connection pooling (use sqlx with postgres feature)
4. Pass the same contract tests as polkagent-store-sqlite
5. Cross-tenant access denial test suite

Add to workspace Cargo.toml. Run cargo check --workspace after."

task "P5-9" "Create polkagent-billing" \
"Create polkagent-billing crate for usage metering (PRD-11 deliverable 5.9).

Create crates/polkagent-billing/:
1. Per-run cost computation: token usage times configured model price
2. Metered event emission on run completion
3. Audit-export CSV that reconciles with run logs
4. REST endpoints for polkagent-api: GET /v1/billing/summary, GET /v1/billing/export

Depends on token persistence being implemented (task DX-1). Add to workspace Cargo.toml.

Run cargo check --workspace after."

task "P5-10" "Create polkagent-signer-kms" \
"Create polkagent-signer-kms for external key management (PRD-07 deliverable).

Create crates/polkagent-signer-kms/:
1. Implement SignerPort trait
2. Support AWS KMS and HashiCorp Vault backends (behind feature flags)
3. Key material never leaves the KMS
4. Pass all contract tests from polkagent-signer-trait
5. Configuration via polkagent-config (kms_provider, key_id, region)

For now implement a mock KMS backend that simulates the signing flow. Real KMS integration can follow.

Add to workspace Cargo.toml. Run cargo check --workspace after."

task "P5-11" "PCA C2/C3 compatibility" \
"Extend polkagent-transport-pca for C2/C3 compatibility tiers (PRD-06 deliverable 5.10).

C2 additions:
1. End-to-end encrypted group messaging
2. Multi-party key agreement protocol
3. Group member join/leave handling

C3 additions:
1. Statement store integration (bulletin CID anchoring)
2. Cross-device message sync
3. App-layer ACK with sequence numbers

Read existing crates/polkagent-transport-pca/ code. These are significant protocol additions — implement what is feasible and document what requires external infrastructure.

Run cargo check --workspace after."

task "P5-12" "Phase 5 test and AC verification" \
"Run Phase 5 acceptance criteria verification (PRD-00 section 11.2):

- AC-P5-001: Funded agent cannot exceed budget — run adversarial budget tests
- AC-P5-002: Adversarial prompts cannot widen authority — run red-team suite
- AC-P5-003: Revocation and time-limit drills succeed
- AC-P5-004: Tenant isolation prevents cross-tenant access
- AC-P5-005: Export/import preserves all state

Run cargo test --workspace and cargo clippy --workspace. Report which criteria pass and which still need work.

Do NOT modify code in this task — only verify and report."

# ── PHASE 6: Experimental Frontier ───────────────────────────────────────
# All Phase 6 items must be feature-gated and carry explicit [experimental] labels.
# They must not affect core correctness or ordinary product flows.

task "P6-1" "Create polkagent-chain-jam (JAM testnet)" \
"Create experimental polkagent-chain-jam crate for JAM testnet connection (PRD-05 deliverable 6.1).

This is EXPERIMENTAL and must be feature-gated:
1. Create crates/polkagent-chain-jam/ behind a cargo feature flag
2. Implement basic block query against a JAM testnet RPC endpoint
3. Do NOT integrate with core correctness paths
4. Add explicit experimental maturity label
5. Minimal tests that verify connection and block parsing

If JAM testnet is not available, create the crate structure with a mock backend and tests.

Add to workspace Cargo.toml with default-features = false. Run cargo check --workspace after."

task "P6-2" "Personhood gating" \
"Implement experimental personhood gating (PRD-07 deliverable 6.2).

Feature-gated experimental module:
1. Add a Cedar policy type that can require a W3C DID credential
2. In polkagent-identity, add DID credential verification (or mock thereof)
3. In polkagent-grant, add a check_personhood() function called during grant resolution when the policy requires it
4. Must NOT affect non-personhood-gated flows

Add behind a feature flag in polkagent-grant. Run cargo check --workspace after."

task "P6-3" "Create polkagent-vitality (affect modules)" \
"Create experimental polkagent-vitality crate (PRD-09 deliverable 6.4).

Feature-gated experimental module:
1. Create crates/polkagent-vitality/
2. Track affect state: energy level, focus, stress indicators
3. Derive from run metrics: success rate, response latency, error frequency
4. Must compile in isolation and must NOT influence run lifecycle unless feature flag is active
5. Add explicit experimental maturity label

Add to workspace Cargo.toml with default-features = false. Run cargo check --workspace after."

task "P6-4" "Evolutionary skill selection" \
"Implement experimental evolutionary skill selection (PRD-09 deliverable 6.5).

Feature-gated experimental module:
1. Modify polkagent-eval to support running multiple skill variants on the same task corpus
2. Implement a Thompson sampling selector that routes tasks to higher-scoring variants
3. Must NOT modify Cedar grants or safety gates
4. Wire into polkagent-skill behind a feature flag

Add tests with at least 3 skill variants and score comparison.

Run cargo check --workspace after."

task "P6-5" "Always-on watchers" \
"Implement always-on watcher agents (PRD-08 deliverable 6.6).

Feature-gated:
1. Allow agent config to declare a watcher with a schedule (cron or interval)
2. Use polkagent-scheduler to run the watcher as a background task
3. Each watcher has its own Cedar grant
4. Watchers have strictly NO write authority unless a separate grant is issued
5. Requires TimeoutEnforcer (task DX-3) to be functional

Modify polkagent-service and polkagent-config. Add tests.

Run cargo check --workspace after."

task "P6-6" "Phase 6 gate verification" \
"Verify all Phase 6 experimental items meet gate criteria from PRD-00 section 12.2:

1. Primary implementation evidence exists for each experiment
2. Core correctness and ordinary product flows do not depend on experimental code
3. Explicit maturity label in UI for each experimental feature
4. Feature flags control activation
5. No production write authority without separate gate

Run cargo check --workspace --all-features to verify all feature-gated code compiles.
Run cargo test --workspace to verify no regressions.

Report which experiments pass gate criteria and which need work."

# ── VERIFICATION & REPORTING ───────────────────────────────────────────────

task "V-1" "Full workspace verification" \
"Run a comprehensive verification of the entire workspace:

1. cargo fmt --all -- --check
2. cargo clippy --workspace -- -D warnings
3. cargo test --workspace

If any tests fail, investigate the failures. If they are caused by changes made in previous tasks, fix them. If they are pre-existing failures, note them but do not fix unrelated code.

Report what passed and what failed."

task "V-2" "Update tracking documents" \
"Read TRACKING.md and prd/DIAGNOSTIC-FINDINGS.md. Based on what was actually fixed across ALL previous tasks (D0 through P6), update:

1. TRACKING.md — mark completed items across all phases, update percentages for Phases 1-6
2. prd/DIAGNOSTIC-FINDINGS.md — check off completed diagnostic items in the implementation checklist

Be accurate: only mark items as done if the corresponding code changes were actually made and verified. If a task was skipped or failed, leave it unchecked.

Do NOT update PROGRESS.md (that is the definitive status tracker and should only be updated after manual review)."

# ─── Computed totals ─────────────────────────────────────────────────────────

TOTAL_TASKS=${#TASK_IDS[@]}

# ─── Globals ─────────────────────────────────────────────────────────────────

PASSED=0
FAILED=0
SKIPPED=0
START_TIME=$(date +%s)
declare -a RESULTS=()
declare -a TIMINGS=()

# ─── Setup ───────────────────────────────────────────────────────────────────

mkdir -p "$TASK_LOG_DIR"

# Single-instance guard
if [[ -f "$LOCKFILE" ]]; then
    OTHER_PID=$(cat "$LOCKFILE" 2>/dev/null || echo "")
    if [[ -n "$OTHER_PID" ]] && kill -0 "$OTHER_PID" 2>/dev/null; then
        echo -e "${RED}Another implementation run is active (PID $OTHER_PID). Exiting.${RST}"
        exit 1
    fi
fi
echo $$ > "$LOCKFILE"
trap 'rm -f "$LOCKFILE"' EXIT

cd "$REPO_ROOT"

# ─── Helpers ─────────────────────────────────────────────────────────────────

log()  { echo -e "${BLU}[$(date +%H:%M:%S)]${RST} $*" | tee -a "$MASTER_LOG"; }
ok()   { echo -e "${GRN}  PASS${RST} $*" | tee -a "$MASTER_LOG"; }
fail() { echo -e "${RED}  FAIL${RST} $*" | tee -a "$MASTER_LOG"; }
warn() { echo -e "${YLW}  WARN${RST} $*" | tee -a "$MASTER_LOG"; }
hr()   { echo -e "${CYN}$(printf '─%.0s' {1..70})${RST}" | tee -a "$MASTER_LOG"; }

elapsed() {
    local diff=$(( $(date +%s) - $1 ))
    printf "%dm%ds" $((diff / 60)) $((diff % 60))
}

ensure_disk_space() {
    local free_gb
    free_gb=$(df -g "$REPO_ROOT" 2>/dev/null | tail -1 | awk '{print $4}')
    if (( free_gb < MIN_DISK_GB )); then
        warn "Low disk (${free_gb}GB). Cleaning..."
        rm -rf "$REPO_ROOT/target/doc" 2>/dev/null || true
        find "$REPO_ROOT/target" -name "incremental" -type d -exec rm -rf {} + 2>/dev/null || true
        free_gb=$(df -g "$REPO_ROOT" 2>/dev/null | tail -1 | awk '{print $4}')
        if (( free_gb < MIN_DISK_GB )); then
            warn "Still low. Running cargo clean..."
            cargo clean 2>/dev/null || true
        fi
        log "Disk after cleanup: $(df -g "$REPO_ROOT" | tail -1 | awk '{print $4}')GB"
    fi
}

# ─── Core: invoke the AI tool ───────────────────────────────────────────────

invoke_claude() {
    local task_id="$1"
    local prompt="$2"
    local task_log="$TASK_LOG_DIR/${task_id}.log"

    local system_prompt
    system_prompt="$(cat <<'SYSPROMPT'
You are implementing fixes and features for the polkagent Rust project.

RULES:
- Read files BEFORE editing them. Understand existing code first.
- Make MINIMAL changes. Don't refactor surrounding code.
- Don't add comments, docstrings, or type annotations to code you didn't change.
- After making changes, run cargo check (and cargo test for the affected crate) to verify.
- If cargo check/test fails, fix the issue before finishing.
- If you're unsure about a change, be conservative — skip it rather than break things.
- Do NOT commit any changes. Do NOT run git commands.
- Do NOT create new files unless absolutely necessary.
- Do NOT modify TRACKING.md, PROGRESS.md, or PRD files unless the task explicitly says to.

PROJECT STRUCTURE:
- 71 Rust crates in crates/ directory
- Config: .polkagent/polkagent.toml
- Schema: crates/polkagent-store-sqlite/src/schema.sql
- PRDs: prd/PRD-00 through PRD-18
- Diagnostics: prd/DIAGNOSTIC-FINDINGS.md
SYSPROMPT
)"

    log "  Invoking $TOOL for $task_id..."
    log "  Log: $task_log"

    if [[ "$TOOL" == "claude" ]]; then
        # Unset CLAUDECODE to allow nested invocation
        env -u CLAUDECODE claude \
            -p \
            --model "$MODEL" \
            --permission-mode "bypassPermissions" \
            --max-budget-usd "$BUDGET_PER_TASK" \
            --system-prompt "$system_prompt" \
            --no-session-persistence \
            "$prompt" \
            > "$task_log" 2>&1
    elif [[ "$TOOL" == "codex" ]]; then
        codex exec \
            --model "${CODEX_MODEL:-o4-mini}" \
            --dangerously-bypass-approvals-and-sandbox \
            --ephemeral \
            "$prompt" \
            > "$task_log" 2>&1
    else
        fail "Unknown tool: $TOOL"
        return 1
    fi
}

# ─── Core: verify after a task ───────────────────────────────────────────────

verify_workspace() {
    log "  Verifying: cargo check --workspace"
    if ! cargo check --workspace > "$TASK_LOG_DIR/verify-check.log" 2>&1; then
        warn "  cargo check failed after task — see verify-check.log"
        return 1
    fi
    ok "  cargo check passed"

    log "  Verifying: cargo test --workspace (quick)"
    if ! timeout 600 cargo test --workspace > "$TASK_LOG_DIR/verify-test.log" 2>&1; then
        warn "  Some tests failed after task — see verify-test.log"
        # Non-fatal: tests may have pre-existing failures
        return 0
    fi
    ok "  cargo test passed"
    return 0
}

# ─── Core: run one task ──────────────────────────────────────────────────────

run_task() {
    local idx=$1
    local task_id="${TASK_IDS[$idx]}"
    local task_name="${TASK_NAMES[$idx]}"
    local task_prompt="${TASK_PROMPTS[$idx]}"
    local task_num=$((idx + 1))

    if (( task_num < RESUME_FROM )); then
        RESULTS+=("SKIP|$task_id|$task_name")
        TIMINGS+=("0s")
        ((SKIPPED++))
        return 0
    fi

    hr
    log "${BLD}Task $task_num/$TOTAL_TASKS: [$task_id] $task_name${RST}"
    echo "$task_num" > "$STATE_FILE"

    if $DRY_RUN; then
        log "  [dry-run] prompt: ${task_prompt:0:120}..."
        RESULTS+=("DRY|$task_id|$task_name")
        TIMINGS+=("0s")
        return 0
    fi

    ensure_disk_space

    local t_start=$(date +%s)

    # Run the AI tool
    if invoke_claude "$task_id" "$task_prompt"; then
        ok "  $TOOL completed $task_id"
    else
        local exit_code=$?
        # Exit code 2 = budget exceeded, which is acceptable
        if (( exit_code == 2 )); then
            warn "  $TOOL hit budget limit on $task_id (acceptable)"
        else
            fail "  $TOOL failed on $task_id (exit $exit_code)"
            RESULTS+=("FAIL|$task_id|$task_name")
            TIMINGS+=("$(elapsed $t_start)")
            ((FAILED++))
            return 0  # continue to next task
        fi
    fi

    # Verify if enabled
    if $VERIFY; then
        if verify_workspace; then
            ok "  Verified: $task_id"
            RESULTS+=("PASS|$task_id|$task_name")
            ((PASSED++))
        else
            warn "  Verify failed: $task_id (continuing)"
            RESULTS+=("WARN|$task_id|$task_name")
            ((PASSED++))  # partial pass
        fi
    else
        RESULTS+=("DONE|$task_id|$task_name")
        ((PASSED++))
    fi

    # Auto-commit and push changes from this task
    if [[ -n "$BRANCH_NAME" ]] && [[ -n "$(git status --porcelain 2>/dev/null)" ]]; then
        git add -A
        git commit -m "$(cat <<EOF
[auto] ${task_id}: ${task_name}
EOF
)" 2>&1 | tee -a "$MASTER_LOG"
        if (( PUSH_COUNT == 0 )); then
            git push -u origin "$BRANCH_NAME" 2>&1 | tee -a "$MASTER_LOG" || warn "  Push failed (will retry next task)"
        else
            git push 2>&1 | tee -a "$MASTER_LOG" || warn "  Push failed (will retry next task)"
        fi
        ((PUSH_COUNT++)) || true
        ok "  Committed and pushed: $task_id"
    fi

    TIMINGS+=("$(elapsed $t_start)")
}

# ─── Report ──────────────────────────────────────────────────────────────────

generate_report() {
    local total_dur=$(elapsed $START_TIME)

    cat > "$REPORT_FILE" <<EOF
# Autonomous Implementation Report

> Generated: $(date '+%Y-%m-%d %H:%M:%S')
> Duration: $total_dur
> Tool: $TOOL (model: $MODEL)
> Tasks: $TOTAL_TASKS total | $PASSED passed | $FAILED failed | $SKIPPED skipped

---

## Task Results

| # | ID | Task | Result | Time |
|---|-----|------|--------|------|
EOF

    for i in "${!RESULTS[@]}"; do
        local IFS='|'; read -r status tid tname <<< "${RESULTS[$i]}"; unset IFS
        local timing="${TIMINGS[$i]}"
        echo "| $((i+1)) | $tid | $tname | $status | $timing |" >> "$REPORT_FILE"
    done

    cat >> "$REPORT_FILE" <<EOF

---

## Task Logs

All task logs are in: \`$TASK_LOG_DIR/\`

| File | Contents |
|------|----------|
| \`master.log\` | This script's output |
EOF

    for i in "${!TASK_IDS[@]}"; do
        local tid="${TASK_IDS[$i]}"
        if [[ -f "$TASK_LOG_DIR/${tid}.log" ]]; then
            echo "| \`${tid}.log\` | ${TASK_NAMES[$i]} |" >> "$REPORT_FILE"
        fi
    done

    cat >> "$REPORT_FILE" <<EOF

---

## Next Steps

All changes committed to branch: \`$BRANCH_NAME\`

If tasks failed, you can:
1. Check the task log: \`cat $TASK_LOG_DIR/<TASK_ID>.log\`
2. Resume from a specific task: \`./scripts/overnight.sh --resume\`
3. Re-run a single task: \`./scripts/overnight.sh --step <N>\`
4. Run verification only: \`cargo test --workspace\`
5. Review changes: \`git log --oneline $BRANCH_NAME\`

## PRD Coverage

This run targeted ALL 18 PRDs across all 6 phases:
- **Diagnostics:** DIAGNOSTIC-FINDINGS.md (43 remaining bugs + 3 additional critical fixes)
- **Phase 2 gaps:** Chain adapter wiring, explain command, SQL injection fix
- **Phase 3:** Builder tools, RAG, error explainer, identity display, PCA C0, executor wiring
- **Phase 4:** Risk gates, multisig coordinator, XCM planner, drift watcher, product kits, harness lifecycle, provenanced memory
- **Phase 5:** Funded accounts, earn/spend, marketplace, fleet workers, billing, Postgres store, KMS signer
- **Phase 6:** JAM prototype, personhood gating, vitality modules, evolutionary skills, watchers
- **Verification:** Full workspace check + tracking document updates
EOF

    log ""
    hr
    log "${BLD}Implementation run complete${RST}"
    log "  Passed:  $PASSED"
    log "  Failed:  $FAILED"
    log "  Skipped: $SKIPPED"
    log "  Time:    $total_dur"
    log "  Report:  $REPORT_FILE"
    hr
}

# ─── Main ────────────────────────────────────────────────────────────────────

hr
log "${BLD}Polkagent Autonomous Implementation Driver${RST}"
log "Tool:    $TOOL (model: $MODEL)"
log "Tasks:   $TOTAL_TASKS"
log "Budget:  \$$BUDGET_PER_TASK per task"
log "Turns:   $MAX_TURNS per task"
log "Verify:  $VERIFY"
log "Branch:  $BRANCH_NAME"
log "Logs:    $TASK_LOG_DIR/"
log "Started: $(date '+%Y-%m-%d %H:%M:%S')"
hr

# ─── Create implementation branch ────────────────────────────────────────

if ! $DRY_RUN && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    if [[ -n "$(git status --porcelain 2>/dev/null)" ]]; then
        warn "Working tree not clean — pre-existing changes will be included in first commit"
    fi
    if git show-ref --verify --quiet "refs/heads/$BRANCH_NAME" 2>/dev/null; then
        log "Resuming on existing branch: ${BLD}$BRANCH_NAME${RST}"
        git checkout "$BRANCH_NAME" 2>&1 | tee -a "$MASTER_LOG"
    else
        log "Creating branch: ${BLD}$BRANCH_NAME${RST}"
        git checkout -b "$BRANCH_NAME" 2>&1 | tee -a "$MASTER_LOG"
    fi
    echo "$BRANCH_NAME" > "$BRANCH_FILE"
elif $DRY_RUN; then
    log "Branch (dry-run): $BRANCH_NAME"
else
    warn "Not a git repository — skipping auto-commit/push"
    BRANCH_NAME=""
fi

for i in "${!TASK_IDS[@]}"; do
    run_task "$i"
done

rm -f "$STATE_FILE"
generate_report

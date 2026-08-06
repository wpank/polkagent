# Polkagent implementation backlog and parallel agent plan

**Status:** canonical execution queue
**Updated:** 2026-08-06
**Input:** source audit, green workspace tests, archived diagnostics/UX/E2E
evidence, PRD-17, and PRD-19

## Outcome order

The roadmap is dependency-driven:

```text
Wave 0: contracts
  FND-01 production runtime + FND-02 interaction/event contract
                       |
Wave 1: executable core
  EXE-01 tool/effect loop + API-01 durable API + OBS-01 replay/recovery
                       |
Wave 2: user surfaces
  TUI-01 terminal/TUI + ACP-01 Zed + ORC-01 groups/feeds

Parallel throughout where interfaces are stable:
  CHAIN-01 live chain | SEC-01 auth/custody | PCA-01 networking |
  QA-01 CI lint (complete; retain as a gate)

Bounded productization slices already delivered; remaining work:
  OPS-01 successful backend/drain/Postgres/online-encrypted backup/upgrade
  EXT-01 runtime activation/sandbox/trust | PAY-01 settlement
```

## Operating rules for implementation agents

- Read [`README.md`](README.md), [`STATUS.md`](STATUS.md), this file, and the
  relevant PRD before changing code.
- Claim one work-packet ID. Put the ID in the branch/PR description and final
  handoff.
- Treat `Cargo.toml`, CLI dispatch, shared API state, and SQLite migration
  registration as hot files. One integration owner coordinates those changes.
- Prefer adding a narrowly owned crate/module behind a frozen interface over
  having several agents edit the same composition root.
- Do not mark a packet done because its types compile. Meet every exit check.
- Add evidence to the packet or `EVIDENCE-BACKLOG.md`; do not create another
  top-level TODO dump in `tmp/`.
- Preserve real approval, exact-byte signing, durable-before-I/O, default deny,
  tenant isolation, and explicit unknown outcomes.

## P0 — shared foundation and executable core

### QA-01 — Mandatory workspace Clippy gate — COMPLETE

**Goal:** make the existing CI command `cargo clippy --workspace -- -D
warnings` pass without weakening the workspace lint policy or hiding product
defects behind broad crate-level allowances.

**Current evidence:** the exact mandatory command and the stronger
all-target/all-feature variant both exit zero locally. The full remediation
covered production correctness issues, adapter/runtime cleanup, hidden test and
benchmark targets, and narrowly reasoned assertion allowances confined to
test/conformance scopes. [`QA-01-CLIPPY-INVENTORY.json`](QA-01-CLIPPY-INVENTORY.json)
records the zero-diagnostic closure while preserving the original 465-error,
14-package baseline at commit `97a9147`.

**Test/conformance lint policy:** production code must recover, propagate, or
prove an invariant rather than panic. Assertion-oriented test and reusable
conformance modules may use `expect`/`unwrap` only under a module- or
target-local allowance with a concrete reason; never add a workspace- or
crate-wide exception to make a batch green.

**Checklist:**

- [x] Align the declared and CI-tested MSRV at Rust 1.89.
- [x] Clear current workspace rustdoc warnings.
- [x] Enforce `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` in
  CI.
- [x] Capture a machine-readable crate/lint inventory from the exact CI
  command and partition it into independently owned crate batches.
- [x] Fix production-code correctness/style diagnostics rather than adding
  workspace-wide allowances.
- [x] Decide and document the narrow test/conformance policy for
  `expect_used`/`unwrap_used`; scope any allowances to test or conformance
  modules with reasons.
- [x] Re-run `cargo +1.89 check --workspace --locked`, strict rustdoc, and the
  full workspace test suite as final closure gates.
- [x] Re-run the exact stable CI command and retain a machine-readable closure
  record.
- [x] Run the stronger all-target/all-feature Clippy gate to cover tests,
  benches, and optional features omitted by the current mandatory command.

**Exit checks:** `cargo clippy --workspace -- -D warnings` exits 0 on the same
stable toolchain used by CI, with no broad reduction in lint levels.

**Ownership:** partition by crate; one integration agent owns lint-policy
changes and the final exact-CI verification. **Depends on:** none and can run in
parallel with every product packet.

### FND-01 — One production runtime composition root

**Goal:** CLI run, serve, TUI, chat, ACP, and transports construct the same
durable application runtime.

**Scope/ownership:** `polkagent-runtime`, production composition tests, and
narrow executable-adapter migrations. This packet owns runtime construction
but not TUI widgets or ACP protocol mapping.

**Checklist:**

- [x] Define `RuntimeOptions`, `PolkagentRuntime`, readiness report, and
  `RuntimeFactory::build`.
- [ ] Centralize config-path provenance, database path, provider/model/harness,
  chain, signer, tools/skills, policy/grants, and read-only selection. The
  factory now owns config/database/provider/model/harness/chain/tool selection;
  signer, skill mutation/trust, policy/grant injection, and service-level
  read-only enforcement remain explicit degraded-readiness gaps. Configured
  skill discovery now feeds one immutable runtime snapshot and read-only API.
- [ ] Open/migrate durable run, effect, event, artifact, conversation, memory,
  payment, audit, agent, group/feed, and registry stores as applicable. The
  current factory wires SQLite run/effect/event/conversation/payment and
  configured memory; the API now projects typed query/non-mutating exact reads,
  non-mutating aggregate statistics, and atomic deletion from that same store.
  Audit, group/feed, and registry persistence remain incomplete.
- [ ] Remove `NoopEffectStore` and in-memory production fallbacks from
  executable configurations; missing required dependencies fail at startup.
  One-shot run, TUI, ACP, and `serve` now consume the shared runtime; the
  runtime now refuses an executable tool registry without a real effect store.
  Approval/policy/resume and some optional API store adapters remain
  incomplete.
- [x] Rehydrate agents and recover/timeout stuck work.
- [x] Migrate one-shot `run` without behavior regression.
- [x] Make `serve` use the shared runtime and durable agents/runs/effects/events/
  artifacts/payments/conversations. Artifact metadata, verified content,
  classification, and lineage survive restart; the exact runtime tool registry
  backs deterministic read-only API discovery. Runtime-owned skill reads and
  the four-route bounded memory API adapter are now composed; nine optional skill
  mutation, audit, and registry routes remain registered with a published,
  tested 501 boundary until truthful contracts/stores are composed.

**2026-08-05 runtime checkpoint:** `polkagent-runtime` now provides strict or
explicitly simulated adapter policy, workdir-aware config provenance, durable
SQLite composition, one event bus/`AppService`, active-agent rehydration,
startup recovery, timeout startup, and structured readiness. Twenty focused
runtime tests plus executable restart/recovery tests pass. `polkagent run`, the
TUI, and ACP now retain the factory's runtime, durable pool, service, and event
bus. `serve` now uses the same strict factory and durable core stores. Complete
optional-store composition plus shutdown/tool/policy gaps keep FND-01 open.

**Exit checks:** a production-composition integration test starts runtime on a
temporary SQLite database, creates a run through service and API paths, observes
the same durable record/events, restarts, and reloads it. No production API
route unexpectedly returns 501 because startup forgot a store.

**Depends on:** none. This is the critical-path integration packet.

### FND-02 — Durable interaction, command, and event contracts

**Goal:** one surface-neutral multi-turn contract for terminal, TUI, API, ACP,
and group orchestration.

**Scope/ownership:** the existing `polkagent-interaction` crate, its runtime and
SQLite adapters, conversation/interaction types, typed command registry,
projections, and additive migrations. Avoid editing individual TUI/ACP
renderers.

**2026-08-05 implementation checkpoint:** `polkagent-interaction` freezes the
serializable contract and now also provides a durable store port, bounded
durable/live event hub, and shared typed command handlers. SQLite migrations
and an adapter persist safe session defaults, turn/run links, replay sequences,
and exactly one terminal event. The runtime now composes a durable headless
service for create/list/load/archive, target config, prompt, cancel, and
subscribe with atomic transcripts, prepared-run causality, idempotent retry,
lag backfill, paged turn-correlated transcript projection, and restart recovery.
TUI, terminal chat, HTTP, and ACP consume it. ACP maps its protocol session ID
exactly to the durable conversation UUID and uses the same prompt, cancel,
target/model configuration, event, and load/resume lifecycle. Model-executor
interactions receive bounded typed prior context; effect-backed tool lifecycle
projection reaches replay/chat/TUI/ACP with stable effect identity, while
approval and execution-scoped provider/harness/autonomy/max-turn/budget config
remain, so this checkpoint is not FND-02 completion.

**Checklist:**

- [x] Define conversation-target/config, prompt request, turn handle, and
  stable interaction/tool/approval/plan/usage event types from PRD-19.
- [x] Add `interaction_sessions`, `interaction_turns`,
  `interaction_turn_runs`, and `interaction_events` with idempotent
  session/turn/event writes and durable run links.
- [x] Prepare a caller-identified run with its conversation correlation without
  publishing, atomically create the turn/run join and user transcript, attach
  the subscriber, and only then activate the earliest run event.
- [x] Persist the user message before execution and atomically persist the
  assistant message plus exactly one terminal event; retrying the same caller
  turn ID cannot execute or write twice.
- [x] Project the real effect-backed tool lifecycle into durable interaction
  replay and chat/TUI/ACP with exact effect-derived tool-call identity, safe
  status/error summaries, lag recovery, and restart evidence. Raw arguments and
  output remain withheld until one explicit redaction/classification contract.
- [ ] Project durable approval observations and permission decisions with exact
  effect/run/conversation identity through the coordinator and checkpoint
  design in
  [`APPROVAL-PAUSE-RESUME-DESIGN.md`](APPROVAL-PAUSE-RESUME-DESIGN.md).
- [x] Implement new/list/load/archive/prompt/cancel/target-config/subscribe.
- [x] Project the durable user/assistant transcript through the service with
  exact turn/run IDs and bounded store pages; a 1,002-turn fixture crosses the
  page boundary without scanning or leaking unrelated low-level messages.
- [x] Assemble typed model-executor context from the newest 32 completed
  user/assistant pairs among the latest 1,000 prior turn records, then append
  the current user exactly once. Failed/cancelled/timed-out partial turns are
  omitted as whole pairs; string-only harness follow-up fails explicitly and
  durably rather than flattening roles.
- [x] Implement persisted and per-prompt execution-scoped model selection with
  prompt > session > agent precedence, canonical same-provider validation, a
  cloned prepared-run spec, retry conflict semantics, and strict harness
  refusal unless unchanged/fixed-model evidence exists.
- [ ] Implement approve/deny and execution-scoped provider/harness/autonomy/
  max-turn/budget configuration; current methods fail explicitly rather than
  accepting ignored settings.
- [x] Implement bounded live broadcast plus durable checkpoint recovery when a
  subscriber lags; TUI and terminal chat resubscribe, while HTTP exposes finite
  checkpoint pages and a checkpoint-aware replay-then-follow SSE adapter.
- [x] Page replay lazily after attaching the live receiver: subscription does
  zero replay reads and each `recv` retains at most one 1,000-event page while
  preserving filters, exact checkpoints, concurrent publication, and lag
  recovery.
- [x] Define one typed parser, catalog, availability model, and output contract
  for `/help`, `/status`, `/agents`, `/agent`, `/new`, `/resume`, `/runs`,
  `/inspect`, `/cancel`, `/approve`, `/deny`, and `/model`.
- [x] Implement the typed command handlers for `/help`, `/status`, `/agents`,
  `/agent`, `/new`, `/resume`, `/runs`, `/inspect`, `/cancel`, `/approve`,
  `/deny`, and `/model` against shared service/runtime ports.
- [ ] Wire the shared command executor through every applicable surface.
  Terminal chat executes help/status/agents/agent/cancel/new/resume/model; the
  TUI executes help/status/agents/agent/new/resume/model with structured
  results; ACP executes a truthful
  durable help/status/agents/agent/model/cancel subset; HTTP exposes typed
  resource operations rather than slash text. Broader cross-surface parity
  remains open.

**Exit checks:** current headless tests create, contextually prompt, stream,
retry, cancel, restart, load, and replay without CLI, Ratatui, Axum, or ACP
types. FND-02 closes only after real approve/deny/tool state passes the same
restart tests and harness/session behavior has a role-safe contract.

**Depends on:** the accepted ADR-002 boundary, the approval coordinator/
checkpoint packet, and a role-safe harness history contract.

### EXE-01 — Real tool/effect/policy/approval execution loop

**Goal:** replace synthetic model tool success with the durable safe-action
pipeline required by PRD-03/04/07.

**Scope/ownership:** `polkagent-run` orchestration and service bridges. This
packet owns the orchestrator hot files; FND-01 only injects its dependencies.

**2026-08-06 approval-foundation checkpoint:** APR-00 and the SQLite portion of
APR-01 from
[`APPROVAL-PAUSE-RESUME-DESIGN.md`](APPROVAL-PAUSE-RESUME-DESIGN.md) are
implemented by the V18 migration and serialized coordinator/checkpoint stores.
The foundation includes exact run state-and-revision CAS, authoritative effect
state, stable identical pause retries, scoped one-shot resolution, paired
effect/checkpoint leases, durable canonical run events, generic-path isolation
for approval-linked effects, and the durable `Claimed` to `Executing` attempt
boundary. It preserves legacy dead-letter state and can recover an expired
zero-attempt pre-I/O claim. Conformance, race, close/reopen, rollback,
wrong-scope, lineage, event-replay, and migration tests cover that boundary.

This is persistence infrastructure, not an end-to-end approval claim. Subject
and checkpoint integrity digests are caller-supplied and cross-checked, not yet
canonically constructed or recomputed. APR-03 still owns orchestrator wiring,
grant revalidation, reducer/checkpoint integrity, startup recovery, and
post-I/O retry reconciliation before any surface advertises approval support.

**Checklist:**

- [x] Advertise only exact registered definitions in the agent's allowlist and
  with no required grant; unknown and grant-bearing tools remain invisible.
- [x] Project stable effect identity and safe tool status after parsing JSON,
  verifying registry/allowlist/grant state, and persisting stable step/effect/
  attempt identities.
- [ ] Finish schema-wide canonical argument validation and redaction-safe
  arguments/output/locations/diffs.
- [ ] Resolve grants/policy/budgets before dispatch.
- [x] Persist the parent turn, normalized step, `EffectIntent`, claim, and
  attempt before handler I/O while retaining SQLite foreign-key enforcement.
- [x] Execute grantless calls through the exact runtime `ToolRegistry` and
  `EffectPipeline`, not fabricated JSON. This slice dispatches in-process after
  a durable claim; a separate effect worker/drain path remains.
- [ ] Pause durably for approval and resume the exact call/effect.
- [x] Persist a success/failure/timeout outcome and feed the exact serialized
  tool result into the next typed model inference request. Artifact projection
  remains open.
- [ ] Enforce max turns, cancellation, timeout, retries, and explicit unknown
  outcomes without duplicate effects.

**Exit checks:** a real runtime test covers read-only tool use, denied tool,
approval-required tool, cancellation, crash/restart between intent and I/O,
duplicate event/retry, and tool output returned to the model.

**Depends on:** FND-01 dependency injection interface. May run parallel to
FND-02 in files exclusively owned by this packet.

### API-01 — Durable control plane and contract parity

**Goal:** API-created work executes in the same runtime as CLI-created work and
survives restart.

**Scope/ownership:** `polkagent-api` adapters/state/routes plus OpenAPI. Do not
own runtime construction or orchestrator internals.

**Checklist:**

- [x] Implement durable `AgentStore` and `RunManagerTrait` adapters over the
  shared runtime/stores. The adapters now project SQLite rows strictly and
  route lifecycle mutations through the shared `AppService`; `serve` selects
  them through `app_state_from_runtime`.
- [x] Inject durable event, artifact, conversation, and payment dependencies;
  artifact reads preserve classification/lineage and verify content integrity.
- [x] Inject a truthful read-only projection of the exact runtime tool registry,
  including deterministic list/detail/grant behavior and an empty view when
  registration is disabled.
- [x] Inject immutable skill list/detail and durable memory query/exact lookup,
  non-mutating statistics, and atomic deletion through exact runtime-owned
  adapters.
- [ ] Inject skill mutation, audit, and service-registry adapters. Their exact
  uncomposed routes remain in the published 501 list.
- [x] Expose interaction create/list/load/archive/turn/prompt/cancel plus strict
  target/model config endpoints over the exact runtime `InteractionService`;
  caller-supplied turn IDs provide retry idempotency and conflict detection.
  Config mutation accepts only one tagged supported option, persists across
  service reconstruction, creates no work, and retains the original target
  endpoint as a deprecated compatibility delegate.
- [x] Add finite JSON cursor/replay semantics with durable sequence checkpoints,
  turn filtering, restart recovery, and no skip across unrelated events.
- [x] Add checkpoint-aware interaction SSE with `Last-Event-ID` precedence,
  typed envelopes/durable sequence IDs, bounded live delivery, exact lag
  recovery after the last emitted sequence, turn filtering, and keepalives.
  Existing sockets remain distinct run-event protocols.
- [x] Reconcile all ten implemented durable-interaction operations and their
  strict DTO/error/event/checkpoint schemas with `openapi.yaml`; tests parse all
  local references, compare `/openapi.json`, and prove unsupported fields and
  approve/deny paths remain absent.
- [x] Enforce ordinary HTTP router/OpenAPI parity from the actual router AST,
  including unique operation IDs, local refs, and exact served-document
  equality. Zero ordinary routes are missing or stale; the two bidirectional
  WebSocket transports remain an explicit documented allowlist.
- [x] Use OpenAPI 3.1 null unions throughout. A recursive regression rejects
  legacy `nullable`, representative null semantics are protected, and Redocly
  reports zero active warnings/errors with five exact public-operation ignores.
- [ ] Compose the 9 intentionally unavailable optional skill mutation, audit,
  and registry routes with real contracts/stores while preserving auth/read-only
  policy. Immutable skill reads and durable memory query/exact lookup/stats/
  deletion are composed and restart-tested.
- [ ] Unify root config selection, bind address/port, auth, CORS, read-only,
  and rate-limit provenance. The canonical deployment smoke now proves the
  current shared-key auth boundary with rate limiting explicitly disabled; it
  does not prove TLS, rotation, principal authorization, or production limits.

**Exit checks:** black-box API test creates an agent and interaction, streams
real events/tool approval, cancels, restarts the server, and reads identical
state. OpenAPI conformance and authenticated/read-only tests pass.

**Depends on:** FND-01; consumes FND-02 for multi-turn APIs.

## P1 — real external behavior and actionable surfaces

### OBS-01 — Durable event replay, projections, and operator evidence

**Goal:** no surface silently loses the state needed to understand active work.

**Checklist:**

- [ ] Persist remaining user-visible diagnostic/ephemeral deltas when they are
  required for correctness. Interaction checkpoints and durable run-event
  global checkpoints are complete; best-effort run frames remain explicitly
  non-replayable. Extend the SQLite run-event schema to retain the full optional
  correlation/causation metadata instead of reconstructing absent replay fields
  with typed defaults.
- [x] Replace silent broadcast-lag drops with durable checkpoint recovery for
  interaction SSE and `GET /api/v1alpha1/events/stream`. The run-event socket
  attaches live delivery before bounded replay, scans filters across pages,
  deduplicates by global sequence, and closes explicitly on recovery failure.
- [x] Recover `/ws/v1alpha1` durable lag within an accepted connection without
  changing its distinct command protocol: attach live before upgrade, establish
  an internal checkpoint from the first valid durable record, page from the
  store after later lag, deduplicate, route both run and agent subscriptions,
  cap subscriptions at 256, and fail with a sanitized error plus 1011 close
  when recovery cannot be proved. Deterministic TCP tests cover lag before and
  after the first checkpoint, malformed zero sequence, unsubscribe/filtering,
  multi-subscription routing, backend failure, missing store, and reconnect.
- [ ] Add a public reconnect cursor/checkpoint to `/ws/v1alpha1` only through a
  versioned protocol decision. The current command envelope carries neither a
  cursor input nor a global checkpoint output, so reconnect remains truthfully
  live-only. Also either implement a real `system` producer or remove that
  advertised channel, and decide whether effect/conversation channels and a
  cancel command belong here or in the interaction API.
- [x] Remove the fixed first-10k event-ID scan: the object-safe store contract
  has an uncapped validated 1,000-row cursor fallback, SQLite uses its primary-
  key index, and API lookup sanitizes backend failures.
- [ ] Wire telemetry, audit, retention, backup, and recovery into lifecycle.
- [ ] Add restart, retention, corrupted-projection, and broader operator
  diagnostics. The API run-event socket now has deterministic slow-consumer,
  duplicate, reconnect, filter/page-bound, authentication, unavailable-store,
  and sanitized backend-recovery coverage.

**Ownership:** event/artifact/telemetry/projection modules; coordinate API/TUI
adapter hooks through small interfaces.

**Depends on:** FND-01 and event contract from FND-02.

### TUI-01 — Interactive terminal chat and actionable TUI

**Goal:** users can prompt and orchestrate without leaving the terminal UI.

**Checklist:**

- [x] Deliver an interim actionable vertical slice: F9 Console, active-agent
  selection, prompt submission, non-blocking live output/lifecycle projection,
  cancellation, durable run selection, and focused reducer/render/bootstrap
  tests using the one-shot service composition.
- [x] Thread the root explicit `--config`/`POLKAGENT_CONFIG` selection into
  both one-shot and TUI run composition instead of silently rediscovering
  provider/harness/execution settings.
- [x] Add `polkagent chat` using `InteractionService` and the truthful
  help/status/agents/agent/cancel/new/resume/model subset of the shared command
  handlers. Process tests cover non-TTY stdout, multiline input, restart target/
  model resume, zero-work target changes, refusal, lag resubscribe, and SIGINT
  cancellation.
- [x] Convert the TUI loop to async/channel-driven input, runtime events, and
  background completion. The bounded pump applies backpressure to lossless
  key/paste input, coalesces resize and tick bursts, awaits the bounded
  interaction-controller channel, and cancels/drains/reaps producer tasks on
  normal exit or error with abort-on-unwind fallback.
- [x] Move periodic aggregate SQLite projections and blocking chain HTTP polls
  off the terminal thread. Each class is single-flight, database refreshes
  collapse to one newest-generation follow-up, results cross a four-slot
  bounded channel, stale run-selection generations cannot apply, partial
  redacted failures remain visible, and production tracks the actual blocking
  handles through bounded shutdown rather than only their async wrappers.
- [x] Pass one long-lived `PolkagentRuntime`, not only a SQLite pool, into the
  TUI and reuse its exact service/event bus/pool across sequential prompts.
- [x] Pass the full durable interaction-service handle into the TUI and route
  prompt/cancel through it with conversation/turn/run correlation.
- [x] Add extended-grapheme-safe editing/navigation/deletion, multiline
  composition, bounded history, viewport scrolling, 128-KiB whole-grapheme
  limits, safe bracketed-paste normalization, and reducer/render/PTY coverage.
- [x] Add registry-derived slash-command discovery/help/completion, including
  aliases, argument hints, keyboard selection, and truthful surface filtering.
- [x] Execute the truthful `/help`, `/status`, `/new`, and `/resume` subset in
  the Console through shared handlers; render structured command state, guard
  stale results, switch/load exact same-agent sessions, and keep commands out
  of the model transcript.
- [x] Execute `/model [id]` through the same service executor, persist the
  selection per durable conversation, project it into Console status/header,
  guard stale original-conversation results, and prove restart/isolation/
  refusal without turns or shared-AgentSpec mutation.
- [x] Execute `/agents` and `/agent <name-or-id>` through a read-only active-
  agent projection from the retained runtime's already-open SQLite pool and
  the durable target-config service path in chat and Console. Preserve the
  conversation/transcript/model, guard stale/active work, reject unknown/
  inactive/ambiguous selectors, route the next run to the restart-persisted
  target, and create no turn/run/event or shared-AgentSpec mutation. Per-agent
  readiness remains explicitly unprojected rather than guessed.
- [x] Add an explicit durable conversation selector. `s` asynchronously lists
  at most 50 same-agent sessions from the newest 1,000 summaries, loads the
  exact selected transcript through `InteractionService`, refuses while a turn
  or command is active, and guards stale list/load results. Restart coverage
  proves exact selection while excluding a foreign-agent session.
- [ ] Add richer structured tool/plan transcript rendering. The selected
  durable session reloads bounded transcript/composer history after restart and
  renders typed lifecycle/usage/error observations.
- [x] Add durable transcript follow-up and service-routed prompt/cancel.
- [x] Assemble bounded prior completed transcript into typed model-executor
  context with exact roles and whole-turn truncation. Harness-backed follow-up
  fails explicitly because its string ingress cannot preserve roles.
- [ ] Add service-routed approve/deny after the durable coordinator/checkpoint
  packet; legacy Approvals-tab mutations remain outside the Console interaction
  path. Agent list/selection is now service-routed for this slice.
- [ ] Remove UI direct DB mutations.
- [x] Preserve all existing monitoring tabs while adding the Console.
- [x] Prove terminal restoration on normal error and panic unwind through the
  lifecycle guard and a real Unix PTY, including escape ordering and termios
  restoration. Windows ConPTY remains an evidence gap.

**Current boundary:** terminal chat and the TUI Console are single-agent,
conversation-model-selectable, single-active-turn surfaces over the durable
`InteractionService`. The Console
reloads per-agent transcript/history after restart, `/new` and `/resume` switch
an explicit conversation ID, `s` provides a bounded same-agent durable session
selector, and terminal chat can resume one.
Prior completed turns enter model-executor context; string-only harness follow-
up is unsupported. Broader shared-command coverage, simultaneous orchestration,
rich plan/redaction-safe structured-tool rendering, and service-routed
approvals remain open.

The TUI shell is now event-driven rather than a Crossterm poll/read frame loop.
Headless tests cover bounded ordered input, latest-resize and tick coalescing,
background completion wakeups, idle blocking, input failure, and bounded
shutdown. Periodic SQLite projection and chain HTTP work now run as separately
single-flight tracked jobs behind a bounded result queue: missed database
refreshes coalesce to the newest selected-run generation and missed chain ticks
are satisfied by the in-flight poll. Deterministic blocked/failing-worker tests
prove cancel, prompt-entry, resize, and controller events remain responsive;
stale generations cannot overwrite the newest selected run. SQLite lock waits
and each RPC exchange are bounded. Normal shutdown joins the actual blocking
handles within a six-second ceiling; an abnormal job that exceeds that ceiling
is reported explicitly rather than being described as joined. Existing
controller tests continue to cover prompt, follow-up, cancellation, commands,
and session selection through awaited events; the Unix PTY lifecycle test
continues to prove terminal restoration. Monitoring-tab content and rendering
remain unchanged.

**Exit checks:** user can launch, select/create an agent, prompt, see tokens and
tools, approve/deny, cancel, prompt again, restart, and resume. Headless event
loop and snapshot tests cover narrow/resized terminals and simultaneous runs.

**Ownership:** CLI chat/TUI modules only. **Depends on:** FND-01, FND-02, and
the stable execution event path; can run fully parallel to ACP-01.

### ACP-01 — Polkagent ACP server and Zed integration

**Goal:** Zed and other ACP clients can use Polkagent as an external agent.

**Checklist:**

- [x] Add a dedicated `polkagent-surface-acp` server crate using the official
  ACP Rust SDK; retain `polkagent-harness-acp` as the separate downstream client.
- [x] Add protocol-safe early `polkagent acp` stdio dispatch so stdout contains
  JSON-RPC frames only.
- [x] Implement the bounded initialize/new/prompt/cancel lifecycle, agent-message
  updates, terminal/cancel stop reasons, and unknown/busy-session errors.
- [x] Map ACP session IDs exactly to durable conversation UUIDs and add
  new/load/resume plus restart recovery through `InteractionService`.
- [ ] Add session list/import when the pinned SDK and surface expose those
  operations; neither is currently advertised.
- [x] Advertise and handle `/help`, `/status`, `/agents`, and `/agent` through
  `available_commands_update`.
- [x] Move ACP discovery/parsing/help/aliases onto the shared command registry,
  including real current-prompt `/cancel`/`/stop`; registered commands outside
  the adapter's durable subset are refused explicitly.
- [x] Add native ACP configuration for active-agent and standard model
  selection, backed by the same durable interaction config as `/agent` and
  `/model`; prove restart persistence and that concurrent sessions retain their
  chosen agent/model at the real run boundary without shared-spec mutation.
- [x] Map typed text/lifecycle/usage/checkpoint events through the durable
  interaction stream, including exact terminal reconciliation and lag replay.
- [x] Map effect-backed tool start/update events to official ACP tool messages
  with one stable effect-derived ID, safe content, lag recovery, and restart
  replay. ACP v1 lacks cancelled/unknown statuses, so both map to failed with a
  safe detail.
- [ ] Map durable approval/plan events after the coordinator/checkpoint runtime
  produces them; raw arguments/output remain withheld pending redaction policy.
- [ ] Add durable command-executor routing plus truthful group/auto-target,
  provider, harness, and autonomy configuration after those settings can be
  isolated per execution. Active-agent target selection is already composed.
- [x] Validate absolute cwd and reject unsupported MCP servers/additional roots
  explicitly instead of ignoring them.
- [x] Persist one immutable lexically normalized origin cwd in SQLite v17 and
  verify it before every prompt/load/resume/replay. Cross-workspace, relative,
  traversal, and legacy-unknown origins fail before events/turns/runs; generic
  legacy list/load/archive remains available.
- [x] Forward bounded real runtime text updates before the terminal response,
  reconcile exact final text without duplication, and emit ACP usage only from
  real terminal token counts plus a known model context window.
- [ ] Add permission round-trips, default deny on timeout/disconnect, rich plan
  updates, and client filesystem/terminal capabilities. Native safe tool
  start/update projection is implemented.
- [x] Add an official-client cancellation/stop-reason test covering
  cancellation during an active provider request and the durable terminal run
  state/timestamp.
- [x] Add an official-client new/prompt/same-turn-retry/restart/load/follow-up/
  resume fixture proving exact conversation/turn/run IDs, persisted model
  config, replay-on-load, no replay-on-resume, and no duplicate retry run.
- [x] Prove successful-session stdout purity and fail-closed missing-explicit-
  config startup with empty stdout, a stderr diagnostic, and exit code 4.
- [x] Prove provider/backend error redaction, backend panic containment,
  process-level ACP panic-payload suppression, and unavailable-provider startup
  behavior without protocol-stdout contamination.
- [x] Implement and prove opt-in ACP-safe JSONL file diagnostics with known-
  pattern redaction, a 4 KiB detail cap, 1 MiB rotation, three retained
  generations, restrictive Unix permissions, `O_NOFOLLOW`, and non-regular-
  path refusal. Diagnostics remain per-process and intentionally exclude raw
  prompt/response bodies.
- [x] Add an official-SDK subprocess fixture and a Zed custom-agent setup guide.
- [ ] Complete the manual Zed smoke (tool, approval/deny, cancel, restart/load,
  and ACP-log inspection); add import only when the pinned SDK/surface exposes
  it.

**Exit checks:** in Zed, add the custom external agent, prompt, observe a real
tool call, approve/deny, cancel, restart/load the thread, and inspect clean ACP
logs. Add import proof only when the pinned SDK/surface exposes it. Protocol
fixtures pass without TUI/CLI-specific behavior.

**Ownership:** existing `polkagent-surface-acp` crate and focused CLI early
dispatch. **Depends on:** FND-01/FND-02; runs parallel to TUI-01 and API-01
after contracts freeze.

### CHAIN-01 — Real local-chain read/write/finality slice

**Goal:** close the gap between fake-backed action tests and a real Polkadot
network before claiming safe writes.

**Checklist:**

- [x] Repair PRD-17 fixture/workflow variable, endpoint-scheme, and binary
  assumptions.
- [x] Ensure tests actually consume live RPC environment variables. The live
  target now probes both chains and requires relay finality progression; retain
  the checklist below because this is read-only evidence, not action E2E.
- [ ] Add a real dev signer with explicit testnet-only safeguards.
- [ ] Pin metadata/genesis/spec version; decode and sign the exact bytes shown.
- [ ] Match submitted transaction/events during inclusion/finality watching.
- [ ] Implement or explicitly scope dry-run and XCM capability methods.
- [ ] Exercise one read, one failed/refused write, one approved write, restart,
  stale metadata, wrong network, and finality timeout.

**Exit checks:** `polkagent testnet go` or an equivalent documented command
spawns a network and produces machine-readable evidence that real bytes were
signed, submitted, included/finalized, and reconciled.

**Ownership:** PRD-17 fixtures/scripts/workflow, chain adapter, test-only signer,
and E2E crate. **Depends on:** no surface work; final action flow integrates
with EXE-01/FND-01.

### SEC-01 — Production identity, auth, policy, and secret custody

**Goal:** production composition is default-deny with identifiable principals
and protected credentials.

**Checklist:**

- [ ] Replace plaintext credential storage with OS keychain/encrypted or
  external secret backends and migration guidance.
- [ ] Extend the composed constant-time shared-key API auth into selected
  production auth/session/JWT, principal/tenant/RBAC context, and tenant
  filters. The container replacement/restore smoke proves public/protected
  routing and secret-free artifacts, not multi-principal authorization.
- [ ] Load configured policy/grants into every run/effect path; remove empty
  default policy on executable paths.
- [ ] Bind approvals/audit to a principal and exact payload/effect.
- [ ] Implement or explicitly exclude mock KMS/Vault/DID branches from
  production readiness.
- [ ] Add cross-tenant, secret-at-rest, token rotation, revocation, replay, and
  least-privilege tests.

**Ownership:** security/auth/secret/signer policy adapters. **Depends on:** a
small principal/runtime interface from FND-01; otherwise parallel.

### PCA-01 — Production PCA network transport

**Goal:** a real PCA peer/app message can durably enter Polkagent, execute, and
produce a reply.

**Checklist:**

- [x] Add bounded cross-process TCP peer/application I/O around the existing
  X25519/ChaCha20-Poly1305 session types. `TcpPcaTransport` uses framed,
  encrypted messages and validates the configured peer identity on both the
  handshake and decrypted message.
- [x] Persist the sender outbox before accepting `send`, persist receiver inbox
  plus dedup marker before the wire ACK, retain messages until application ACK,
  redeliver expired application leases, retry the oldest entry after reconnect,
  recover unacked inbox entries after restart, and enforce
  message/inbox/outbox limits. Evidence:
  `tcp_network.rs` covers socket delivery, delayed-peer reconnect, duplicate
  retry, lease expiry, receiver restart, limits/backpressure, and a separate
  child process.
- [x] Add typed cancellation, status, and error reply application frames on the
  same encrypted durable lane. Sender and receiver enforce control-field,
  total-message, JSON, progress, retry-metadata, shared-backpressure, and
  sender-identity validation before persistence/wire ACK. Evidence:
  `tcp_network.rs` covers offline cancellation restart/reconnect and duplicate
  retry, status receiver restart, structured status/error exchange across a
  child process, corrupted inbound rejection, and size/backpressure failures.
- [ ] Replace the bounded TCP protocol with (or adapt it behind) the pinned PCA
  reference application's Statement Store/Polkadot App wire protocol; add
  cryptographic SS58 authentication rather than trusting a configured identity
  string, multi-process state-file exclusion, dedup retention/compaction, and
  attachment/file frames.
- [ ] Connect delivery to shared interaction/runtime and map replies/status.
- [ ] Add compatibility fixtures against the PCA reference implementation.

**Exit checks:** cross-process interoperability test covers normal message,
duplicate, crash-before-ack, reconnect, cancellation, error reply, and file
handling.

**Ownership:** PCA transport/surface adapter. **Depends on:** FND-01/FND-02
integration interfaces; protocol work can begin in parallel.

### ORC-01 — Memory, groups, feeds, and eval through the real runtime

**Goal:** turn existing domain libraries into observable durable orchestration.

**Checklist:**

- [ ] Retrieve policy-filtered memory into context and admit outcomes with
  lineage/classification.
- [ ] Add durable group CRUD and child-run creation with grant/budget
  intersection, cancellation, quorum/synthesis, and evidence.
- [ ] Make feed/scheduler triggers create idempotent shared-runtime runs.
- [ ] Make eval target the same configured runtime/provider instead of always a
  fake executor.
- [ ] Add CLI/TUI/ACP inspectable group plans only after the headless path works.

**Exit checks:** headless tests prove restart-safe group and feed execution,
memory isolation, cancellation propagation, budget enforcement, and a real
runtime eval result.

**Depends on:** FND-01/FND-02/EXE-01. Domain persistence work can run earlier in
separate files.

## P2 — productization after the core path is real

### OPS-01 — Deployable self-hosted and managed runtime

- [x] Fix Docker command/port/config drift and add a CI container health smoke
  test. Delivered by `scripts/container-smoke.sh`: locked image build,
  unprivileged boot, all three HTTP probes, and SQLite file creation. This is
  single-instance boot evidence, not durable API or production-ops proof.
- [x] Prove the bounded single-instance HTTP lifecycle: root config selection
  reaches `serve`; a read-only bind-mounted config file is honoured; SIGTERM
  drains Axum and exits 0; and a replacement container reuses the named volume.
  The smoke now creates an agent plus a configured interaction/prompt through
  HTTP and proves the exact reason-bearing failed turn/run and empty transcript
  output survive replacement under the same IDs. CI uploads selected lifecycle
  and HTTP-projection diagnostics. The real local provider is intentionally
  unreachable, so successful production-backend output and daemon
  worker/run/effect recovery remain unproved.
- [x] Prove a bounded cold SQLite volume backup/restore. After graceful source
  shutdown, the smoke captures the complete volume (including any WAL/SHM
  sidecars) read-only under the non-root runtime identity, verifies archive
  SHA-256 and SQLite integrity/foreign keys, restores into a fresh Compose
  project/volume under the same identity, and requires exact HTTP projections.
  This is not an online, encrypted, scheduled, PostgreSQL, or export/import
  backup contract.
- [x] Prove the authenticated recovery boundary with rate limiting explicitly
  disabled. Initial, replacement, and restored phases each pass five public
  requests plus four protected requests under missing, invalid, and valid-key
  credentials (15 public, 24 exact 401, and 12 valid successes total), with
  built-in and independent plaintext credential scans.
- [ ] Prove Postgres conformance and tenant isolation.
- [ ] Define migrations, online/encrypted backup and retention, supported
  export/import, PostgreSQL recovery, upgrade/rollback, resource limits,
  durable run/worker/effect draining, crash recovery, and release artifacts.
- [ ] Only then connect control/worker services and add Helm/Kubernetes/HA.

**Depends on:** FND-01, API-01, OBS-01, SEC-01.

### EXT-01 — Installable and safely executable extensions

- [ ] Freeze canonical manifest/package/lock format.
- [x] Build a durable local plugin/kit install/list/update/history/rollback/
  uninstall store with immutable version content, atomic state replacement,
  cross-process locking, restart validation, idempotency, and BLAKE3 integrity.
- [x] Expose local install/list/get/update/rollback/uninstall through
  `polkagent package` with structured output, dry-run inspection, explicit
  store selection, and strict-versus-development trust policy.
- [ ] Expose the lifecycle through the API and connect installed packages to
  real runtime execution (`run`).
- [ ] Implement actual sandbox engine and capability boundary.
- [ ] Add cryptographic signature/provenance verification plus advisory,
  malware, and reproducibility gates. The local store currently records
  signature material as an **unverified claim** and never reports it verified.
- [ ] Add federation/public/commercial behavior only after local lifecycle and
  trust gates pass.

**Depends on:** FND-01, EXE-01, SEC-01.

### PAY-01 — Real payment and budget settlement

- [ ] Wire payment store/budgets into shared runtime and effect policy.
- [ ] Replace placeholder signatures/ledger behavior with a selected real
  settlement protocol and explicit reconciliation states.
- [ ] Prove idempotency, insufficient funds, fee drift, timeout/unknown,
  reversal/refund, audit, and restart behavior on a test network.

**Depends on:** EXE-01, CHAIN-01, SEC-01; do not lead with this packet.

## Next safe parallel-agent allocation

The contract/runtime foundation and first TUI, chat, HTTP, and ACP slices are
already present. Completed rows below freeze dependency state; new agents start
from an explicitly ready remaining row rather than replaying foundation work.

| Lane | Packet and next deliverable | Exclusive primary ownership | Integration gate |
|---|---|---|---|
| Integration | **Complete:** APR-00/01 contracts, V18 SQLite coordinator, run CAS, checkpoint leases, and conformance | Contract package, store traits, SQLite coordinator, run CAS, checkpoint schema | Preserve atomic/idempotent/reopen/generic-path-isolation evidence; do not create a second coordinator. |
| Policy | **Complete:** APR-02 strict opt-in policy approval effects, config, resolver injection, and readiness | Policy/config/runtime-composition modules | Preserve default deny, permit, escalation precedence, and strict config tests. |
| Execution | **In progress:** APR-03/EXE-01 orchestrator pause/recovery and one approval-required tool | `polkagent-run` orchestrator and narrow service bridges | APR-01/APR-02 are satisfied; AllowOnce executes once and reject/cancel/timeout execute zero times, with unknown post-attempt recovery fail-closed. |
| ACP harness | **Complete:** APR-04 fake permission backend and official-SDK protocol harness | Focused ACP fake backend and protocol fixtures only | Preserve once-only identity/redaction/cancel/error/disconnect coverage; production binding remains APR-07. |
| Projection/API | APR-05 adds durable approval projection/query/approve-deny and HTTP adapter | Interaction/runtime projection, coordinator binding, narrow HTTP routes | Starts after APR-01 plus APR-03's event contract; replay and wrong-scope tests pass. |
| Terminal | APR-06 adds chat/TUI approval queue/detail/actions and removes direct DB writes | CLI chat/TUI modules | Starts after APR-05; exact approval identities survive cancel/restart with no accidental prompts. |
| Editor | APR-07 binds production ACP permission requests to the coordinator | `polkagent-surface-acp` production backend | Starts after APR-03/APR-04/APR-05; official client allow/reject/cancel/reconnect tests pass. |
| Closure | APR-08 proves the cross-surface crash/security/observability matrix | Cross-surface fixtures and evidence docs | Starts after APR-03/APR-05/APR-06/APR-07; user-path E2E and workspace gates pass. |
| TUI orchestration | **In progress:** TUI-03 replaces the single-active-turn controller with bounded simultaneous durable sessions | CLI TUI controller/state/render/tests only | Two conversations progress independently; selection and exact cancellation do not cross streams; shutdown reaps all owned tasks. |
| Observability | **In progress:** OBS-01 persists complete canonical SQLite run-event correlation/causation metadata | Event contract/recorder and forward SQLite migration | Legacy cursor order survives migration; populated/null metadata round-trips and corrupted rows fail closed; tenant/principal isolation remains open. |
| Control plane | API-01: compose one currently unavailable store family at a time | API state/adapters/routes/OpenAPI | Auth/read-only/restart test passes and router-derived ordinary HTTP drift remains zero. |
| Network | PCA-01: adapt durable TCP delivery into shared interaction/runtime | PCA transport/surface modules | Duplicate/reconnect/cancel frames map idempotently to one durable run and reply. |
| Chain/security | CHAIN-01 signed local action and SEC-01 principal/policy/custody can proceed in separate crates | chain fixture/adapter versus auth/secret/policy adapters | Exact signed bytes/finality evidence and default-deny principal-bound approval evidence. |
| Productization | OPS-01, EXT-01, and PAY-01 remain independent until their named core dependency is ready | deployment, marketplace, and payment paths respectively | Apply the completion gate in `STATUS.md`; do not replace missing production dependencies with fake stores. |

One integration owner must serialize changes to workspace manifests, CLI
dispatch, shared API state, and SQLite migration registration. Other lanes can
run concurrently when they keep to the ownership column and hand additive
interfaces to that owner.

## Required agent handoff

Every packet handoff must include:

```text
Packet:
Outcome delivered:
Production entry point:
Persistent state/migrations:
Real adapters used:
Known fake/no-op/unsupported paths:
Tests run and results:
Failure/restart/security evidence:
Docs/config updated:
Remaining gaps and newly unblocked packet IDs:
Files intentionally left for integration owner:
```

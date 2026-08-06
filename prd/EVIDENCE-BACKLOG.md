# Polkagent evidence and decision backlog

**Updated:** 2026-08-06

This queue holds product, safety, interoperability, and operational evidence
that cannot be closed by adding types or unit tests. It consolidates the still
useful obligations from archived research, validation plans, UX audits, and E2E
reports.

| ID | Evidence required | Blocks | Completion artifact |
|---|---|---|---|
| EVD-01 | Interview/observe representative builder, operator, and Polkadot action users on the first useful workflows. | Scope ordering and persona confidence | Findings with participant profile, tasks, observed pain, decision, and PRD changes |
| EVD-02 | Test explain-before-sign/action-card comprehension, canonical-vs-model text separation, fee/network/finality understanding, and refusal. | CHAIN-01, PAY-01 production UX | Script, anonymized results, comprehension/error rates, revised card acceptance criteria |
| EVD-03 | Decide and validate signer custody/handoff for local, external wallet, KMS/Vault, proxy, and testnet developer modes. | SEC-01, CHAIN-01, PAY-01 | Threat model, selected backends, byte-level handoff transcript, failure/revocation tests |
| EVD-04 | Prove metadata pinning, stale metadata, wrong genesis/network, runtime upgrade, decode drift, and exact signed bytes on a live local network. | CHAIN-01 release gate | Machine-readable run bundle with metadata hashes, bytes, events, finality, and negative cases |
| EVD-05 | Exercise crash boundaries before/after intent persistence, external I/O, unknown outcome, approval, outbox delivery, and restart. | EXE-01, OBS-01 | Fault matrix and durable records showing no duplicate external effect |
| EVD-06 | Interoperate cross-process with the PCA reference product for messages, attachments, dedupe, restart, and reply semantics. | PCA-01 | Versioned compatibility fixture/transcript and documented deviations |
| EVD-07 | Validate Polkagent as a Zed custom ACP agent: init, prompt streaming, tools, permission, cancel, restart/load, commands, config options, cwd/MCP behavior. | ACP-01 | ACP transcript fixtures, Zed screenshots/log review, setup and troubleshooting guide |
| EVD-08 | Run container/deployment smoke with persistent data, health, shutdown, restart, backup/restore, upgrade/rollback, auth, and resource pressure. | OPS-01 | CI artifact, runbook, recovery timings, known limits |
| EVD-09 | Attempt extension escape/capability abuse, malicious package/update, dependency confusion, and rollback. | EXT-01 | Red-team corpus, sandbox/trust results, signed package provenance |
| EVD-10 | Prove tenant/principal isolation through API, stores, events, memory, artifacts, groups, payments, logs, and metrics. | SEC-01, OPS-01 | Cross-tenant matrix with deny/audit evidence |
| EVD-11 (closed 2026-08-05) | Compare TUI, terminal chat, API, and ACP views of the same restarted interaction, including a refusal critical path. | — | [`cross_surface_interaction_e2e.rs`](../crates/polkagent-cli/tests/cross_surface_interaction_e2e.rs) using one exact conversation and stable turn/run/event IDs |
| EVD-12 | Validate real provider/harness readiness, streaming, tool protocol, cancellation, retry, billing/usage, context limits, and redaction. | PRD-04a maturity claims | Per-adapter conformance report; unsupported features explicitly labeled |

## Captured evidence and remaining gaps

- **FND-02 / durable headless interaction slice (2026-08-05):** runtime tests
  prove the user transcript, caller turn identity, conversation-correlated run,
  and turn/run link exist before the first observable `RunCreated`; a completed
  prompt writes one ordered User/Assistant pair whose assistant text equals the
  nonempty terminal result. Same-ID retry returns one handle/execution while
  changed input conflicts. Further tests cover more than one 1,000-event replay
  page, synchronous idempotent cancellation after a forced run-bus lag, durable
  checkpoint replay, more than 1,000 sessions, unlinked prepared-row cleanup,
  the linked pre-activation crash window, idempotent restart recovery, and
  fail-closed completed-run recovery when no durable output can reconstruct the
  response. A separate 1,002-turn/2,005-message projection fixture crosses
  store pages, retains exact turn/run correlation and completed/failed/
  cancelled terminal states, and excludes unrelated low-level messages.
  A restart/follow-up executor fixture proves exact User/Assistant/User roles,
  current-prompt single insertion, and no model call on same-turn retry. Other
  fixtures prove newest-32 whole completed-pair truncation, exclusion of failed
  and cancelled partial output, and a role-safe `Unsupported` harness follow-up
  whose attempted linked turn/run is durably failed without invoking the
  harness. Concurrent-session model fixtures prove prompt > persisted session >
  agent-default precedence, cloned prepared-spec isolation, restart persistence,
  prompt-clear/no-leak behavior, effective-config retry conflict, refusal before
  turn/executor creation for unknown models, and discovery-only harness model
  rejection. Unsupported provider/harness/autonomy/budget config returns typed
  errors. Approval operations require an explicitly bound exact authority.
  This is strong headless evidence. The successful/restarted EVD-11 path now
  has one cross-surface fixture; FND-02 remains open for production-bound effect
  approvals, raw tool-data redaction, and a role-safe harness contract.

- **EXE-01 / grantless registered-tool slice (2026-08-05):** a SQLite-backed
  `AppService` fixture runs a real registered handler selected only from the
  exact agent allowlist/registry intersection. The handler observes that its
  parent turn, normalized step, effect intent, claim, and attempt already
  exist before I/O; the immutable outcome survives database reopen and the
  exact serialized tool result enters the next inference request. Separate
  cases prove unknown, unallowlisted, malformed-JSON, registry-mismatched, and
  grant-bearing calls never execute and receive typed tool errors. This closes
  the fabricated-success defect for the bounded grantless path, not EVD-05:
  approval pause/resume, crash boundaries, cancellation during handler I/O,
  unknown-outcome reconciliation, duplicate retry, worker drain, and an
  external side effect remain unproved.

- **FND-02 / effect-backed tool projection slice (2026-08-05):** live and
  restarted interaction replay re-read the exact persisted intent/attempt/
  outcome and emit deterministic tool-started/updated envelopes. `ToolCallId`
  is the exact effect ID; SQLite v16 preserves attempt/run outcome lineage and
  the forward migration hardens it against tampering. Terminal chat and the
  TUI render the safe stable ID/status. The official ACP v1 adapter emits native
  `tool_call`/`tool_call_update` messages with that same ID across lag and
  restart; unsupported cancelled/unknown ACP statuses map to failed with safe
  detail. Arguments and raw output are deliberately absent because no common
  classification/redaction contract exists. This proves projection, not
  approval, permission, or external-tool safety.

- **APR-05 / scoped durable approval projection and HTTP contract
  (2026-08-06):** a real SQLite runtime fixture seeds one APR-03 lineage and
  proves the pending query returns stable approval/effect/run IDs in the exact
  tenant/workspace/conversation/principal scope. Wrong conversation and
  principal fail closed, AllowOnce persists, an identical retry returns the
  same decision, an opposite retry conflicts, and reopen/reprojection yields
  exactly one requested and one resolved deterministic interaction event. The
  shared `AppService` coordinator is the sole mutation path; the process-local
  approval broadcast and fabricated effect-only success path are removed.
  Focused HTTP tests prove missing/invalid credentials fail, explicit
  principal-bound composition succeeds, wrong conversation is forbidden,
  read-only mode blocks decisions, and the denial limit matches OpenAPI at
  4,096 Unicode characters. Router/source/embedded/served OpenAPI parity passes.
  The pending HTTP response is a non-paginated first page capped at 100.

  This evidence is test-composable, not production-usable:
  `RuntimeFactory` supplies no stable authenticated approval principal,
  `app_state_from_runtime` leaves the approval service unset, readiness remains
  unavailable/disabled, and normal surfaces advertise no grant-bearing tools.
  APR-06 terminal/TUI adapter work, APR-07 ACP, and APR-08 cross-surface crash/
  security work were the next gates at that checkpoint. Legacy
  `/effects/{id}/approve|deny` are deprecated 501 stubs because an effect UUID
  alone is not authority.

- **APR-06 / terminal-chat approval adapter (2026-08-06):** terminal chat now
  derives approval-command discovery from the shared registry and its exact
  scoped pending set, projects at most 100 stable approval IDs in `/status`,
  and routes `/approve` and `/deny` only through `InteractionService`. A real
  SQLite coordinator fixture proves wrong-conversation refusal, identical
  retry after service restart, opposite-decision conflict, preservation of a
  pending approval when generic cancellation is refused, and ordinary
  cancellation when an approval-capable service returns no pending requests.
  Denial reasons are capped at 4,096 Unicode characters; request/resolution
  progress emits only stable IDs and fixed status text, never untrusted titles,
  descriptions, policy reasons, or denial text. A subprocess proves the normal
  authority-unbound `RuntimeFactory` hides both commands and rejects explicit
  mutation without creating a turn, run, or approval row. Production authority,
  grant-bearing execution, and APR-08 crash closure stay open.

- **APR-06 / TUI F6 approval adapter (2026-08-06):** F6 now scopes the shared
  `InteractionService` pending/approve/deny operations to the selected durable
  F9 Console conversation. List and decisions run on the existing bounded
  async controller alongside active turns; request plus conversation IDs reject
  stale completions after a session switch. The first 100 pending rows expose
  exact approval/effect/run/tool IDs and only bounded, redacted service title,
  description, policy reason, status, and expiry. Full conversation and
  approval IDs are retained through confirmation. The old raw
  `effect_intents` query and synthetic `effect_outcomes` inserts are deleted.
  Controller/reducer/render fixtures prove wrong scope, identical retry,
  opposite-decision conflict, controller restart, active-run concurrency,
  unavailable/unsupported authority, truncation visibility, selection by
  stable ID, oversized metadata redaction, and no fabricated risk/pallet data.
  Until coordinator-aware approval cancellation is composed, a turn known to
  await approval refuses generic cancel/shutdown cancellation and preserves the
  durable pending request. Normal `RuntimeFactory` composition therefore shows
  truthful unavailable guidance rather than claiming production usability.

- **EVD-07 / ACP client-protocol slices (2026-08-05):** the official ACP Rust
  client launches `polkagent acp` as a subprocess and proves initialization,
  durable session creation, shared-registry command discovery/help/aliases,
  persisted agent/model selection, truthful refusal of unsupported commands,
  real prompt streaming, and current-prompt cancellation with a durable
  cancelled run. ACP session IDs are the exact conversation UUIDs. An
  official-client new/prompt/same-turn-retry/restart/load/follow-up/resume
  fixture proves exact durable conversation/turn/run links, no duplicate retry
  run, transcript replay on load, no replay on resume, persisted model config,
  and continued use of the same interaction after subprocess replacement.
  That official-client path now advertises registry-identical `/runs` and
  `/inspect` entries, lists and inspects the exact durable run ID, then repeats
  both reads after `session/load` in a replacement subprocess. The shared read
  model caps runs/artifacts at 20, removes stored reason suffixes and all prompt,
  parameter, body, metadata, and provider-error fields, and returns the same
  not-found result for missing and foreign-conversation run IDs.
  Separate fixtures prove JSON-only protocol stdout, missing/unavailable
  startup failure before protocol output, provider/backend secret redaction,
  backend panic containment, ACP-scoped panic-payload suppression, and opt-in
  bounded JSONL diagnostics with restrictive/no-follow file behavior. The
  diagnostics fixture proves known-pattern secret redaction, rotation, and the
  absence of exercised prompt/response bodies while ACP stdout stays protocol-
  only. A separate official-client restart fixture seeds an abandoned run and
  proves `RuntimeFactory` recovery on the editor process's durable database. A
  two-session official-client fixture changes native agent/model selectors and
  `/model`, overlaps real prompts, and proves each provider body and durable run
  retains that session's selection; unsupported values/settings are rejected. A
  delayed-provider fixture proves a real runtime text update, then truthful
  7+3/200k usage, then the ACP terminal response with exact non-duplicated text
  and JSON-only stdout. Native effect-backed tool-call/update messages carry
  the stable effect ID through restart. A separate multi-process fixture proves
  one immutable SQLite-v17 lexical origin cwd: exact load succeeds after
  restart, cross-workspace/relative/traversal and legacy-unknown origins fail
  before adding an event, turn, or run, and generic legacy rows remain readable/
  archivable. This is runtime-event progress, not provider HTTP/SSE token
  streaming or a real editor run.
  EVD-07 remains open for a real Zed run, session list/import, rich plans,
  provider/autonomy options, MCP passthrough, editor-side ACP log inspection,
  and provider HTTP/SSE token-level streaming.

- **APR-07 / bounded local-stdio ACP approvals (2026-08-06):** commit
  `d659c35` composes one explicit all-or-none tenant/workspace/non-nil-principal
  authority through `RuntimeFactory`, `InteractionService`, and the existing
  APR-03 coordinator/checkpoint executor on one SQLite pool. Authority remains
  absent by default and invalid input fails before protocol stdout. The official
  ACP client proves native allow executes exactly one attempt and outcome,
  reject and cancel execute zero attempts, exact approval/run/effect identities
  are enforced, raw arguments/output/locations/diffs stay withheld, and
  disconnect/protocol failure never grants. A SIGKILL-before-decision fixture
  leaves one pending approval, then `session/load` with the same authority,
  database, and cwd re-presents exactly one request and completes exactly one
  effect. The pending recovery query is hard-capped at 100 and overflow fails
  closed. Focused interaction/surface/runtime tests and all 16 ACP stdio E2E
  cases pass. Manual Zed permission/cancel/restart/log inspection remains open;
  this is not shared or remote multi-principal authentication.

- **ZED-LOCAL / non-GUI setup readiness (2026-08-06):** a read-only local audit
  found Zed stable 1.14.2 (build `20260805.160132`) and Preview 0.167.1 installed;
  neither exposes `zed` on the shell `PATH`, while each application bundle's CLI
  reports its version. No GUI was launched and no user settings were inspected.
  Official Zed 1.14.2 source confirms a local custom agent is spawned in the
  first/default project root and sends that same root as ACP cwd for new, load,
  and resume, matching Polkagent's exact approval cwd contract for a local
  single-root project. `acp_zed_config_contract.rs` parses every guide JSON block
  and the checked-in complete example, enforces absolute executable/config/log
  paths, an empty environment, and the complete approval authority tuple, then
  passes its arguments through the real CLI parser. The existing ACP stdio E2E
  suite owns live command-catalog and protocol-stdout evidence. Manual GUI
  permission/cancel/restart smoke, remote projects, and settings-specific MCP
  forwarding remain open; the current setup requires no forwarded MCP servers.

- **EVD-11 / terminal interaction slices (2026-08-05):** the TUI lifecycle test
  runs the real integration binary inside a Unix PTY and proves ordered
  alternate-screen/mouse/cursor restoration escapes plus termios restoration
  after explicit exit, ordinary error, and caught-panic unwind. This is strong
  terminal-boundary evidence. The same PTY boundary now includes bracketed-
  paste enable/disable restoration; reducer/render tests prove ZWJ emoji,
  combining-mark, CJK, multiline slash-prefixed paste, control normalization,
  and 128-KiB whole-grapheme bounds without accidental submit. A focused real-
  runtime test proves the Console creates one durable interaction, completes
  two prompts, reloads exact transcript/correlation after restart, completes
  another prompt, cancels an
  immediate turn durably, bounds UTF-8 output, and ignores stale history loads.
  A restart fixture creates two same-agent sessions plus a foreign-agent
  session, opens the `s` selector, excludes the foreign session, loads the exact
  selected transcript through `InteractionService`, and proves the next prompt
  appends to it. Controller/reducer coverage enforces the 1,000-summary scan and
  50-row display bounds, serialized selector/command requests, background-turn
  navigation, empty/error guidance, and stale list/load result guards; selection
  creates no model turn. A controlled two-execution fixture starts distinct
  agent/conversation activities concurrently, interleaves their output, restores
  each durable transcript and unsent draft, rejects a same-conversation duplicate
  without clearing input, cancels exactly one identity, and lets the other finish.
  Separate tests prove the 8-run admission limit, bounded 256-update channel,
  deterministic oldest-terminal eviction from a 32-activity window, redacted
  public/debug projections, compact TestBackend rendering, and full task reaping.
  Existing real-runtime restart/follow-up/cancel fixtures remain green; durable
  group/child-run execution is not claimed.
  Durable `/model` fixtures drive two conversations through distinct real
  captured executor model IDs, survive service and process restart, retain the
  selected model in the TUI header/status, and reject unknown, cross-provider,
  and unsupported harness changes. They also prove commands create no turns
  and never mutate the shared AgentSpec.
  Chat and TUI `/agents`/`/agent` fixtures use a read-only active-agent
  projection from the retained runtime's already-open SQLite pool and the
  durable target-config service path. They reject unknown, inactive, ambiguous,
  active-work, and stale-result changes; preserve conversation/transcript/model;
  create zero turns/runs/events; leave AgentSpec JSON unchanged; survive process
  restart; and route the next real run to the persisted target without claiming
  unproved per-agent readiness.
  A terminal-chat process fixture creates one durable run, lists and inspects
  its exact ID from later `--resume` processes, and proves a fresh conversation
  cannot inspect it. Terminal chat and ACP render those projections through
  the same bounded formatter. The TUI now consumes that exact runtime-owned
  scoped read model and formatter through its asynchronous control channel.
  Focused controller/reducer/render fixtures prove selected-conversation list
  and inspection, byte-identical foreign/missing refusal, 20-run/artifact
  bounds, reason/summary/provider-error redaction, stale-selection rejection,
  restart survival, zero created turns/runs, and unchanged concurrent activity,
  viewport, shutdown, and exact-cancellation behavior.
  A single deterministic SQLite fixture now creates and configures one exact
  conversation through the in-process HTTP router, prompts it through a real
  `polkagent chat --resume` subprocess, restarts and loads it through an
  official ACP client subprocess, follows up through ACP, then reconstructs
  the TUI controller/session selector, loads the same conversation, prompts a
  third turn, and renders it through Ratatui `TestBackend`. Final HTTP and
  durable-store projections agree on the same conversation, ordered transcript,
  exact turn/run links, model config, lifecycle, terminal usage, and ACP
  checkpoint. `/model`, `/status`, and an invalid model refusal create no
  turns or runs. This closes EVD-11 through its restarted success and refusal
  paths. Cancellation uses the same durable boundary and remains proven by the
  existing adapter-specific HTTP/chat/ACP/TUI tests rather than interrupting
  this linear three-turn fixture; Windows ConPTY is also untested.

- **API-01 / durable HTTP core slice (2026-08-05):** a black-box router test
  builds the same runtime composition used by `serve`, creates an agent and run
  over HTTP, rebuilds the runtime on the same SQLite database, and reloads both
  projections. Effects, events, artifacts, payments, conversations, and the
  live runtime bus are injected. Artifact content, classification, and lineage
  survive restart with integrity verification, auth, and read-only coverage;
  the runtime's exact tool registry is exposed with deterministic schema/grant/
  classification reads plus auth/read-only/disabled-registry coverage. The
  runtime's immutable configured-skill snapshot is exposed with deterministic
  restart/disabled/unknown/mutation-boundary evidence. Typed memory query,
  non-mutating exact lookup/statistics, and atomic deletion retain the exact
  `AppService`-owned SQLite store, survive restart, preserve access counters/
  timestamps, and cover disabled, unknown, invalid, auth, and read-only
  behavior. A machine-readable list proves 9 optional routes remain explicit
  501s and the 3 principal-bound approval routes remain explicit 503s in the
  normal runtime. The
  API now injects the exact runtime `InteractionService` and exposes durable
  create/list/load/archive, turns, prompt, cancel, target/model config, and
  finite event-replay routes. Black-box fixtures prove user/assistant
  transcript, caller turn-ID retry/conflict, idempotent cancellation, filtered checkpoint paging,
  restart replay, authentication/read-only policy, and an uncomposed 501. A
  live HTTP fixture proves replay-before-connect, disconnect/`Last-Event-ID`
  reconnect without duplicate or skipped IDs, turn filtering, and later-turn
  delivery on the same connection after a terminal event. Deterministic forced
  lag proves resubscription starts after the last envelope actually emitted;
  backend logging exposes only a typed error code. A separate event-hub suite
  proves subscription performs zero replay reads, then demand-loads one bounded
  1,000-event page across more than two pages; concurrent publication is
  delivered exactly once, filtered hidden events advance the cursor, early
  drop does not load the remaining backlog, and backend retry is exact. This is
  durable control-plane evidence, not full EVD-08: worker/effect drain,
  successful backend output, and the remaining optional-route composition stay
  open; container replacement and cold restore are covered below. A Syn-based
  parity test derives the actual registered method/path set
  from the router AST, normalizes path parameters, and proves zero missing or
  stale ordinary HTTP operations, unique operation IDs, valid local refs, and
  byte-equivalent `/openapi.json`. Exactly two WebSocket transports remain on a
  documented allowlist because OpenAPI cannot specify their frame protocols:
  `GET /api/v1alpha1/events/stream` and `GET /ws/v1alpha1`.
  A separate config fixture proves persisted set/clear/inheritance, strict
  same-provider model and target validation, two-session isolation, restart,
  deprecated target-route compatibility, and refusal of unsupported tags and
  extra fields with no partial mutation or turn/run creation.
  All 51 legacy OpenAPI `nullable` uses were converted to 3.1 primitive unions
  or explicit `$ref`/null `anyOf` composition. A recursive contract test guards
  the dialect and representative semantics. Redocly 2 now has zero active
  warnings/errors; five exact public-operational rule pointers are deliberately
  ignored.

- **OBS-01 / event-ID lookup slice (2026-08-05):** store-trait, SQLite,
  conformance, and API tests find a target after 10,001 earlier events. The
  default object-safe contract uses bounded 1,000-row cursor pages with strict
  page-size and forward-progress validation and no history cap; malformed
  stores fail closed. SQLite proves an exact primary-key-index query plan. HTTP
  returns typed 404 for absence and a generic 500 for backend failure, while a
  private backend sentinel appears in neither response nor typed log fields.

- **OBS-01 / run-event WebSocket recovery slice (2026-08-06):** a live TCP
  fixture proves `GET /api/v1alpha1/events/stream` attaches its bounded bus
  receiver before replay, reads the injected `EventStore` in 256-record pages,
  reaches filtered matches beyond a page boundary, follows new durable commits,
  and reconnects strictly after the durable `global_sequence` carried by each
  durable frame. A deterministic capacity-two overflow proves forced lag
  replays after the last consumed checkpoint and deduplicates replay/live
  overlap. Separate handshake/close assertions cover invalid checkpoints,
  authentication, missing durable storage, and a lag-recovery backend failure:
  the last closes with status 1011 and a generic reason containing no private
  backend sentinel. Diagnostic and ephemeral frames remain explicitly
  best-effort. A second fixture uses a real migrated SQLite `EventStore` and
  the canonical `EventRecorder`, proving stored `data_json` projects back to
  the typed `EventKind` and SQLite rowid is the durable global reconnect cursor.
  Replay is truthful to the fields retained by `EventStore`.

- **OBS-01 / durable event-metadata fidelity slice (2026-08-06):** SQLite V19
  adds conversation, causation, scope, durability, trace/span, and all four
  optional component-correlation columns without rebuilding `run_events`.
  A V18 file migration fixture proves exact legacy rowid/global-cursor
  preservation, truthful null/empty defaults, diagnostic-prefix durability
  backfill, canonical diagnostic event-type normalization, and exclusion of
  diagnostic rows from durable cursor reads. Canonical recorder/store/API tests
  cover null and fully populated turn/step/effect-intent/effect-attempt/
  causation metadata. Corrupt JSON, timestamps, typed IDs, and mismatched
  event-type/payload pairs fail closed, including a real-SQLite WebSocket 1011
  assertion. The APR-03 integration gate then exposed V18 approval events with
  non-UUID IDs; V20 deterministically normalizes those IDs without moving their
  rowid/global cursors and catalogs `effects_resolved`, with migration replay
  plus all 16 coordinator tests passing. PostgreSQL schema/adapter mapping is
  present, but its live test
  returned early because `TEST_DATABASE_URL` was unset; production upgrade
  evidence remains open. Conversation/scope filters do not prove tenant or
  principal isolation.

- **OBS-01 / command WebSocket recovery slice (2026-08-06):** real TCP tests
  prove `/ws/v1alpha1` attaches before upgrade completion, preserves its
  existing single-channel command envelope, routes concurrent run and agent
  subscriptions, stops delivery after unsubscribe, and caps each connection at
  256 distinct subscriptions. A forced capacity-two lag after a delivered
  durable event replays from that internal global checkpoint without gaps or
  duplicates. Lag before any checkpoint and a malformed zero-valued point
  lookup both fail closed with a generic error envelope and 1011 close without
  fabricating replay; missing store and backend failure are equally explicit
  and sanitized. Durable envelopes now add an opaque versioned
  `v1:<global_sequence>` cursor. Reconnect requires a valid query token, all
  initial subscribe commands, and an additive `ready` barrier before the
  connection-global cursor can advance. Deterministic fixtures prove the
  barrier prevents loss across multiple subscriptions, full initial replay,
  reconnect across 256-row pages, run filtering, replay/live dedupe, forced-lag
  interaction, exact bounded reads, and rejection of malformed, stale, and
  future cursors. Unauthenticated probes perform no store reads and backend
  cursor-validation detail is sanitized; omitting a cursor remains live-only.
  The producer-less `system` channel is rejected instead of advertised.
  Diagnostic/ephemeral events remain best-effort.

- **EVD-08 / OPS-01 container lifecycle slices (2026-08-05):**
  `scripts/container-smoke.sh`, invoked by the `container-smoke` CI job,
  validates the Compose model, builds the canonical image with `Cargo.lock`,
  waits for Docker readiness, and verifies a configured non-root user and
  process UID. Auth is enabled with one deterministic test-only key hash and
  rate limiting disabled. Initial, replacement, and restored instances each
  prove five public OpenAPI/health/PCA-health paths without credentials, exact
  missing/invalid 401 bodies for four protected system/metrics/PCA/agent paths,
  and valid-key success on all four. It also proves
  that a read-only bind-mounted config reaches `serve`, SIGTERM drains the HTTP
  server with exit 0, and a newly created container retains the same named
  volume. Through real HTTP routes it creates an agent, targeted durable
  interaction and model config, submits a prompt to an intentionally
  unreachable real local provider, records the exact conversation/turn/run IDs
  and reason-bearing failed terminal state, then requires the same IDs and
  exact agent, interaction/config, turn, run, and transcript projections after
  replacement. The empty assistant output is asserted so this does not simulate
  a successful model. The same smoke then stops/drains the source, captures the
  whole SQLite volume read-only under the runtime's non-root UID/GID, preserves
  any WAL/SHM sidecars, verifies SHA-256 plus SQLite integrity/foreign keys, and
  restores into a fresh Compose project and named volume. The normal service
  must return the exact pre-backup API projections. Plaintext valid and invalid
  test credentials are scanned across response JSON, metrics/headers, logs,
  backup/summary, and uploaded artifacts; secret-bearing diagnostics are
  withheld. CI uploads a summary, verified backup/manifest, HTTP JSON, selected
  state, and container logs on success or failure. EVD-08 remains open for
  successful production-backend output, worker/run/effect draining, online/
  encrypted/export or PostgreSQL backup, retention/RPO automation,
  upgrade/rollback, TLS/key rotation/
  revocation, multi-principal authorization, tenant isolation, crash boundaries,
  and resource-pressure evidence.
  The authoritative clean detached-worktree Colima run at exact commit
  `e8adad340a6cbd1106a51f5961ee7f15cb6ee4ea` captured a 1,945,600-byte
  archive with SHA-256
  `0010aae43371990ccab19a419bb731d035db41926ef786fe953aa5f65edb8d1d` and
  SQLite integrity `ok`. Capture rounded to 0 seconds, restore-through-API took
  7 seconds, and cached full smoke took 23 seconds. Across initial, replacement,
  and restored phases it recorded 15 public successes, 24 exact unauthorized
  responses, and 12 valid-key protected successes; built-in and independent
  plaintext credential scans passed. Reproducible CI artifact capture remains
  external evidence.

## Evidence quality rules

- Record the commit/worktree state, configuration, real/fake adapters, command,
  environment, expected outcome, actual result, and durable artifact paths.
- A passing unit test is not a substitute for a client, network, process, or
  restart boundary named by the evidence item.
- Preserve failed evidence. Move raw captures to a dated `tmp/archive/`, and
  summarize the decision or open gap here.
- Close an item only when its artifact is reproducible by another agent or
  operator from documented commands.
- Link closed evidence from `STATUS.md` before upgrading a maturity level.

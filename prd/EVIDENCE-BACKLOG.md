# Polkagent evidence and decision backlog

**Updated:** 2026-08-05

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
| EVD-11 | Compare TUI, terminal chat, API, and ACP views of the same active/restarted interaction. | FND-02, TUI-01, ACP-01, API-01 | Cross-surface conformance test using stable interaction/run/event IDs |
| EVD-12 | Validate real provider/harness readiness, streaming, tool protocol, cancellation, retry, billing/usage, context limits, and redaction. | PRD-04a maturity claims | Per-adapter conformance report; unsupported features explicitly labeled |

## Partial evidence captured

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
  rejection. Unsupported provider/harness/autonomy/budget config and approval/
  deny return typed errors.
  This is strong headless evidence, but FND-02/EVD-11 remain open until real
  effect approvals and cross-surface restart tests pass.

- **EVD-07 / ACP client-protocol slices (2026-08-05):** the official ACP Rust
  client launches `polkagent acp` as a subprocess and proves initialization,
  session creation, shared-registry command discovery/help/aliases, agent
  selection, truthful refusal of unsupported durable commands, real prompt
  streaming, and current-prompt cancellation with a durable cancelled run.
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
  and JSON-only stdout. This is runtime-event progress, not evidence of
  provider HTTP/SSE token streaming.
  EVD-07 remains open for a real Zed run, durable session load/import, tools,
  permission round-trips, provider/target/autonomy options, MCP passthrough,
  interaction restart, and editor-side ACP log inspection.

- **EVD-11 / terminal interaction slices (2026-08-05):** the TUI lifecycle test
  runs the real integration binary inside a Unix PTY and proves ordered
  alternate-screen/mouse/cursor restoration escapes plus termios restoration
  after explicit exit, ordinary error, and caught-panic unwind. This is strong
  terminal-boundary evidence. The same PTY boundary now includes bracketed-
  paste enable/disable restoration; reducer/render tests prove ZWJ emoji,
  combining-mark, CJK, multiline slash-prefixed paste, control normalization,
  and 128-KiB whole-grapheme bounds without accidental submit. A focused real-
  runtime test proves the Console creates one durable interaction, completes
  two prompts, reloads exact
  transcript/correlation after restart, completes another prompt, cancels an
  immediate turn durably, bounds UTF-8 output, and ignores stale history loads.
  A restart fixture creates two same-agent sessions plus a foreign-agent
  session, opens the `s` selector, excludes the foreign session, loads the exact
  selected transcript through `InteractionService`, and proves the next prompt
  appends to it. Controller/reducer coverage enforces the 1,000-summary scan and
  50-row display bounds, active-work refusal, empty/error guidance, and stale
  list/load result guards; selection creates no model turn.
  Separate terminal-chat subprocess tests prove exact non-TTY stdout, restart
  transcript resume, explicit configuration refusal, shared-command help, and
  SIGINT cancellation with durable terminal state. These are single-surface
  fixtures: EVD-11 remains open because no test yet compares TUI, chat, API,
  and ACP projections of the same interaction; Windows ConPTY is also untested.

- **API-01 / durable HTTP core slice (2026-08-05):** a black-box router test
  builds the same runtime composition used by `serve`, creates an agent and run
  over HTTP, rebuilds the runtime on the same SQLite database, and reloads both
  projections. Effects, events, artifacts, payments, conversations, and the
  live runtime bus are injected. Artifact content, classification, and lineage
  survive restart with integrity verification, auth, and read-only coverage;
  the runtime's exact tool registry is exposed with deterministic schema/grant/
  classification reads plus auth/read-only/disabled-registry coverage. A
  machine-readable list proves 15 optional routes remain explicit 501s. The
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
  container-level interaction recovery, and optional-route composition remain
  open. A Syn-based parity test derives the actual registered method/path set
  from the router AST, normalizes path parameters, and proves zero missing or
  stale ordinary HTTP operations, unique operation IDs, valid local refs, and
  byte-equivalent `/openapi.json`. Exactly two WebSocket transports remain on a
  documented allowlist because OpenAPI cannot specify their frame protocols:
  `GET /api/v1alpha1/events/stream` and `GET /ws/v1alpha1`.
  A separate config fixture proves persisted set/clear/inheritance, strict
  same-provider model and target validation, two-session isolation, restart,
  deprecated target-route compatibility, and refusal of unsupported tags and
  extra fields with no partial mutation or turn/run creation.

- **OBS-01 / event-ID lookup slice (2026-08-05):** store-trait, SQLite,
  conformance, and API tests find a target after 10,001 earlier events. The
  default object-safe contract uses bounded 1,000-row cursor pages with strict
  page-size and forward-progress validation and no history cap; malformed
  stores fail closed. SQLite proves an exact primary-key-index query plan. HTTP
  returns typed 404 for absence and a generic 500 for backend failure, while a
  private backend sentinel appears in neither response nor typed log fields.

- **EVD-08 / OPS-01 container lifecycle slices (2026-08-05):**
  `scripts/container-smoke.sh`, invoked by the `container-smoke` CI job,
  validates the Compose model, builds the canonical image with `Cargo.lock`,
  waits for Docker readiness, probes live/ready/startup through the published
  port, and verifies a configured non-root user and process UID. It also proves
  that a read-only bind-mounted config reaches `serve`, SIGTERM drains the HTTP
  server with exit 0, a newly created container retains the same named volume,
  and a SQLite-backed CLI agent marker survives replacement. CI uploads a
  summary, selected state, and container logs on success or failure. EVD-08
  remains open for durable API run/agent recovery, worker/effect draining,
  Postgres, backup/restore, upgrade/rollback, auth, tenant isolation, crash
  boundaries, and resource-pressure evidence.

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

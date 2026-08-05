# Polkagent implementation status

**Evidence snapshot:** 2026-08-06
**Conclusion:** broad component maturity; a shared runtime and durable
interaction substrate now exist, but production-surface convergence is
incomplete and no PRD is verified complete end-to-end

## Verification baseline

At this evidence snapshot, the following local gates passed:

```text
cargo check --workspace
cargo +1.89 check --workspace --locked
cargo +1.89 check --workspace --all-targets --all-features --locked
cargo test --workspace --no-fail-fast
cargo test --workspace --all-features --no-fail-fast
cargo test --workspace --tests -- --ignored
cargo clippy --workspace -- -D warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p polkagent-cli --test tui_tests
cargo test -p polkagent-cli --test tui_pty_lifecycle
cargo test -p polkagent-cli --test acp_stdio_e2e
cargo test -p polkagent-interaction
cargo test -p polkagent-runtime --all-targets
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps
./scripts/container-smoke.sh
```

The latest recorded all-feature workspace run exited 0 with 8,063 ordinary
tests and 84 doc tests passing, zero failures, five ignored ordinary tests, and
26 ignored doc tests. The recorded baseline includes
extensive unit, property, contract, integration, security, TUI rendering, API,
and doc coverage. Eighteen PostgreSQL conformance tests plus six cross-tenant
tests returned early because `TEST_DATABASE_URL` was unset. Two live Polkadot
RPC/finality tests likewise returned early because their relay and parachain
endpoint variables were unset. They are not external-system evidence even
though Cargo reports them as passed. This is a strong component baseline. The
container smoke additionally proves that the
locked canonical image builds, starts unprivileged, honours a bind-mounted
read-only config, proves five credential-free public paths plus exact missing/
invalid/valid-key behavior for four protected paths, drains HTTP on SIGTERM with
a clean exit, and replaces the container on the same named volume while
retaining an HTTP-created agent, configured interaction, reason-bearing failed
turn/run, and transcript under their exact IDs and JSON projections. The real local
provider is intentionally unreachable, so no successful model output is
simulated or claimed. It then takes a cold whole-volume SQLite snapshot after
another clean drain, verifies its hash and database integrity, restores under
the non-root runtime identity into a fresh Compose project/volume, and requires
the same HTTP projections. A focused official-SDK subprocess suite also proves
ACP initialize/new/prompt, a real `AppService` run, and cancellation while a
provider request is active. The client receives the `Cancelled` stop reason,
and SQLite retains the reason-bearing terminal state plus completion timestamp.
The same suite checks that successful-session stdout lines are JSON and that a
missing explicit config exits 4 with empty stdout and its diagnostic on stderr.
Focused TUI tests cover reducer/input/render behavior, grapheme-safe editing,
bounded bracketed paste, multiline prompts, durable target/model selection,
follow-up/restart, stale-load race protection, and exact cancellation.
Terminal-chat subprocess tests cover clean non-TTY output, durable transcript/
target/model resume, unsupported configuration, and SIGINT cancellation. HTTP interaction tests cover retry/
conflict, filtered checkpoint replay, cancellation, restart, auth, and read-only
mode. A Unix PTY
subprocess test now proves actual Crossterm escape ordering and termios
restoration on normal exit, ordinary error, and caught-panic unwind; Windows
ConPTY remains unproved. These checks do not prove worker/effect draining,
Postgres or online/encrypted/export backup, tenant isolation, HA, a real chain
action, role-safe harness multi-turn input, or complete Zed/editor behavior.

The exact mandatory CI lint command, `cargo clippy --workspace -- -D
warnings`, now exits zero locally. The stronger
`cargo clippy --workspace --all-targets --all-features -- -D warnings` gate
also exits zero, covering tests, benches, and optional-feature targets that the
current CI command does not compile. The closure record and the preserved
465-diagnostic baseline are in
[`QA-01-CLIPPY-INVENTORY.json`](QA-01-CLIPPY-INVENTORY.json). This closes the
lint-debt packet; it does not close the production composition, live adapter,
client-interoperability, or operations evidence gaps below.

The audit intentionally treats tests such as “returns 501 when store is not
configured” as contract coverage and simultaneous evidence that production
startup still has a wiring gap.

The published schema now uses OpenAPI 3.1 null unions throughout. Recursive
contract tests reject legacy `nullable`, preserve representative primitive and
`$ref` null semantics, and keep router parity exact. Redocly 2 now reports zero
active warnings or errors; five deliberately public operational operations use
exact ignored rule pointers rather than broad suppression.

## Product-surface status

| Surface/capability | Component state | Product state | Decisive gap |
|---|---|---|---|
| One-shot CLI run | Uses the shared `RuntimeFactory`; subprocess and durable-restart coverage pass | Partially usable; the bounded grantless registered-tool path is composed below its CLI boundary | The path is `AppService`-tested, but no one-shot CLI subprocess proves schema advertisement through handler I/O and next inference; policy, approvals, crash recovery, cancellation during I/O, and external-tool evidence remain. |
| Monitoring TUI | Rich views plus a durable F9 Console, grapheme-safe multiline editor/history/paste, executable shared-command subset, guarded terminal lifecycle, and durable session/agent selection | Actionable for one turn at a time and restart-resumable | Prompt/follow-up/live typed output/cancel, bounded contextual model-executor history, help/status/agents/agent/new/resume/model, exact session switching, and safe tool status lines run through shared services; simultaneous orchestration, harness context, rich plans, and service-routed approvals remain. |
| REST/WebSocket API | `serve` uses the strict shared runtime plus durable core stores, exact runtime tool/skill/memory discovery, and the exact runtime `InteractionService` | Durable interaction/control-plane slice with ordinary HTTP/OpenAPI route parity | Versioned interaction lifecycle, persisted target/model configuration, finite replay, checkpointed SSE, immutable skill reads, and durable memory query/lookup survive restart; 11 optional mutation/stats/audit/registry routes, two separately documented WebSocket transports, and full shutdown remain. |
| Interactive terminal chat | `polkagent chat` uses the durable runtime interaction service and shared command handlers | Usable single-agent, target/model-selectable line-mode session | Interactive/non-TTY prompt, multiline input, contextual follow-up, transcript resume, persisted conversation-scoped `/agent` and `/model`, safe tool status, lag replay, and SIGINT cancellation work; provider/harness/autonomy changes, approvals, harness follow-up, rich content, and groups are explicitly unavailable. |
| ACP from Polkagent to other harnesses | ACP client exists and tests pass | Useful downstream adapter | This is client-side harness support only. |
| Polkagent inside Zed/ACP clients | Official-SDK ACP v1 stdio adapter over the durable interaction service, with stable conversation IDs, new/load/resume, shared-registry commands, persisted agent/model selectors, immutable cwd provenance, and native tool updates | Restart-resumable protocol slice; editor interoperability and permissions unverified | Session list/import are unsupported by the pinned SDK/surface; provider/autonomy selectors, permission round-trips, raw tool-data redaction policy, MCP passthrough, and manual Zed tool/approval/restart smoke remain. |
| Providers/harnesses | Many adapters exist | Partially composed | Each adapter needs shared-runtime conformance and real failure/readiness evidence. |
| Tools/skills | Registries and handlers plus a real bounded orchestrator path and effect-backed interaction projection | Grantless tools in the exact agent allowlist/registry intersection execute in the normal model loop and project stable tool IDs/status through chat, TUI, replay, and ACP | Unknown, unallowlisted, malformed-JSON, and grant-bearing calls fail without handler I/O; raw arguments/output remain withheld pending a redaction contract, while approval/resume, schema-wide validation, cancellation/recovery, and external-tool proof remain. |
| Effects/approvals/policy | Strong domain libraries plus a composed grantless tool-effect slice | Tool turn/step/intent/claim/attempt/outcome persist around real handler I/O | Grant resolver/policy/budgets, approval pause/resume, crash unknown-outcome recovery, cancellation during I/O, and effect drain remain. |
| Conversations/memory | `InteractionService` atomically persists transcript/run correlation/replay/context; the API projects the runtime-owned SQLite memory store | Headless service, TUI, terminal chat, HTTP API, and ACP consume the durable lifecycle; typed memory query and non-mutating exact lookup survive restart | One exact successful restarted interaction is cross-surface tested; string-only harness history fails explicitly, approvals are unavailable, memory stats/deletion remain uncomposed, and general memory is not assembled into prompt context. |
| Groups/feeds/evals | Significant libraries/tests | Mostly unsurfaced | No production caller creates durable child runs or evaluates the real composed runtime. |
| Polkadot reads | RPC/metadata/codec components exist | Partially usable | Pinned live metadata and network behavior need real-path validation. |
| Polkadot writes | Effect/signing/finality components exist | Not end-to-end proven | Real signer, exact bytes, transaction matching, finality, dry-run/XCM, and local-chain tests remain. |
| PCA compatibility | Crypto/queue/sync building blocks plus durable TCP text, cancellation, status, and error peer I/O exist | Cross-process protocol slice; not yet PCA-reference compatible or runtime-composed | The TCP adapter needs Statement Store/Polkadot App adaptation, signed identity, runtime cancellation/reply mapping, attachments, and reference fixtures. |
| Security | Grants, tests, redaction, signer abstractions exist | Not production hardened | Plaintext file secrets, shared-key API auth, mock KMS/DID paths, and unused policy runtime. |
| Payments | Intent/store/budget components exist | Not value-moving | Store/runtime integration, real signature/settlement, and failure reconciliation remain. |
| Marketplace/plugins | Durable local plugin/kit lifecycle, operator CLI, and listing components | Local install/list/get/update/rollback/uninstall is actionable; package execution is missing | Manifest/lock format remains split; no API/runtime activation, actual sandbox engine, or cryptographic trust pipeline. |
| Deployment/cloud | Canonical image/Compose boot, mounted config, auth boundary, health, graceful HTTP stop, same-volume HTTP recovery, and cold SQLite volume restore smoke pass | Bounded authenticated single-instance SQLite lifecycle with intentionally failed provider execution | Successful production-backend output, TLS/key rotation and principal authorization, worker/run/effect drain, online/encrypted/export or PostgreSQL backup, retention, tenant isolation, release, HA, and control/worker paths remain unproven. |

## PRD implementation posture

| PRD | Component maturity | Production wiring | User-visible E2E | Disposition |
|---|---|---|---|---|
| 01 Vision | N/A | N/A | N/A | Active normative direction |
| 02 Architecture | Strong but drifted | Partial | No | Active invariants; reconcile when touched |
| 03 Execution | Strong libraries plus bounded grantless real-tool loop | Partial central composition | SQLite-handler vertical slice only | Active P0 |
| 04/04a Providers/tools/harnesses | Strong adapters plus allowlist/registry tool schema composition | Partial | Scripted-model/SQLite-handler `AppService` vertical slice; external adapters remain | Active P0/P1 |
| 05 Polkadot | Strong read/action components | Partial | No real write proof | Active P1 + PRD-17 |
| 06 PCA | Strong primitives plus tested cross-process TCP/control delivery | Transport not composed into runtime or PCA reference network | No runtime or reference-client E2E | Active P1 |
| 07 Security | Strong primitives/tests | Partial/unsafe defaults | No production security proof | Active P1 |
| 08 Payments | Domain/store components | Missing from runtime | No | Active P2 after safe action path |
| 09 Memory/groups/evals | Typed memory API reads composed; broader components exist | Prompt context/stats/deletion and groups/feeds/evals remain | Restart-safe API memory reads only; no orchestration proof | Active P1 |
| 10 Observability | Interaction replay/lag recovery and core artifact/event injection are proved | Product-wide run transports, audit/telemetry, retention, and operator recovery remain | Interaction recovery/replay proof only | Active P1 |
| 11 Deployment/cloud | Authenticated container boot/config/HTTP drain/same-volume interaction recovery and fresh-volume cold SQLite restore verified | Single-instance SQLite only | Exact auth matrix and failed lifecycle restore; successful backend output, online/encrypted/PostgreSQL recovery, and worker/run/effect recovery remain unproved | Active P2 |
| 12 Marketplace/extensions | Durable local lifecycle and CLI | Operator management works; execution missing | No install-to-run proof | Active P2 |
| 13 UX | Durable terminal chat plus monitoring TUI with durable actionable Console | Partial | Contextual follow-up/restart/cancel, persisted `/agent` and `/model`, session selection, safe tool status, and a truthful command subset are tested; harness context, approvals, broader commands, and orchestration remain | Active P0/P1 + PRD-19 |
| 14 API/config | Shared-runtime durable core, interaction routes, and broad control-plane routes | Agent/run/artifact/tool/skill/memory/interaction reads and core mutations plus checkpointed interaction SSE are composed; 11 optional mutation/stats/audit/registry routes remain explicitly unavailable | HTTP retry/cancel/replay/live reconnect/restart/auth/read-only proof, durable skill/memory reads, zero-drift ordinary HTTP parity, and an OpenAPI 3.1-valid schema exist; two WebSocket frame protocols are separately documented | Active P0/P1 |
| 15 Testing | Broad passing check/test/rustdoc suite; mandatory and extended Clippy gates are locally green | Production paths remain under-tested | Hosted CI confirmation plus live/client/ops gates missing | Active cross-cutting |
| 17 Local testnet | Pinned native fixture, provisioning, CI gate, and live RPC/finality test target | Read-only baseline wired; signed action path missing | No real write proof; CI network artifact pending | Active P1 |
| 19 Interactive/ACP | ACP server, durable terminal chat/TUI, shared runtime, headless interaction service, HTTP adapter, and command handlers implemented | TUI, chat, API, and ACP consume `InteractionService`; ACP maps its session ID exactly to the durable conversation UUID and projects effect-backed tool updates | One SQLite conversation is proven across HTTP/chat/ACP restart/TUI/HTTP with exact transcript/config/IDs/usage and refusal behavior; immutable cwd/restart isolation also passes, while harness context, approvals, session list/import, and manual Zed remain | Active P0/P1 |

## Decisive implementation evidence

- `crates/polkagent-cli/src/commands/serve.rs` now builds one strict
  `PolkagentRuntime`; `app_state_from_runtime` injects durable agent, run,
  effect, event, payment, and conversation dependencies plus the runtime event
  bus. A black-box test creates work over HTTP and reloads it after rebuilding
  the runtime on the same database. The exact composed/unavailable boundary is
  documented in
  [`runtime-api-composition.md`](../docs/runtime-api-composition.md).
- `crates/polkagent-api/src/state.rs` models event, artifact, skill, tool,
  memory, payment, audit, conversation, and registry dependencies as optional;
  route handlers return `NotImplemented` when startup does not inject them.
- `crates/polkagent-run/src/orchestrator.rs` advertises only grantless tools in
  the exact `AgentSpec.tools`/runtime-registry intersection. For each accepted
  call it persists the parent turn and normalized step, proposes and claims a
  `ToolCall` effect intent, records the attempt before handler I/O and the
  immutable outcome after it, then sends the exact serialized result into the
  next inference request. A SQLite `AppService` test lets the handler observe
  its own pre-existing turn/step/intent and proves restart persistence. Unknown,
  unallowlisted, malformed-JSON, registry-mismatched, and grant-bearing calls
  never reach a handler. This is not approval/policy/resume/crash-recovery or
  external-tool evidence.
- `crates/polkagent-runtime/src/interaction.rs` projects that exact persisted
  effect lifecycle into deterministic tool-started/updated envelopes. The
  effect ID is the stable tool-call ID through replay, lag, restart, terminal
  chat, TUI, and native ACP messages. SQLite v16 protects attempt/run outcome
  lineage. Raw arguments/output stay absent until a shared classification and
  redaction contract exists; approval/permission remains unimplemented.
- `crates/polkagent-marketplace/src/local.rs` now persists immutable plugin/kit
  versions and restart-safe selection/history with integrity checks. It
  explicitly records signature bundles as unverified claims because no
  cryptographic verifier or runtime sandbox is connected yet.
- `crates/polkagent-cli/src/commands/package.rs` exposes that lifecycle through
  restart-safe local commands with structured output. Strict trust fails
  closed; development trust requires an explicit CLI selection.
- `crates/polkagent-cli/src/tui/interaction.rs` retains one
  `PolkagentRuntime` and routes prompts/cancellation through its exact durable
  `InteractionService`. The nonblocking controller projects typed interaction
  events, durable conversation/turn/run identities, bounded live/final output,
  and durable per-agent history. Focused lifecycle tests prove two prompts use
  one interaction, history reloads after runtime restart, a post-restart prompt
  continues that transcript, immediate cancellation becomes durably terminal,
  and a stale history result cannot overwrite newer work. Transcript reloads
  now use the interaction service's bounded, turn-correlated projection rather
  than scanning raw conversation rows. `/help`, `/status`, `/new`, and
  `/resume` execute through shared handlers, render guarded structured results,
  switch exact same-agent sessions, and never become model turns. Pressing `s`
  opens an asynchronous selector over the newest same-agent durable sessions;
  it bounds discovery to 1,000 summaries and rendering to 50, refuses while a
  turn or command is active, guards stale list/load results, and has restart
  evidence that a foreign-agent session is excluded and the selected exact
  transcript receives the next prompt.
  `/model [id]` uses the same service executor and persisted per-conversation
  config; the header/status/result projection follows the selection and rejects
  stale request, agent, or original-conversation results. Restart, two-session
  isolation, unknown/cross-provider/harness refusal, unchanged shared AgentSpec,
  and zero-command-turn coverage pass.
  `/agents` and `/agent <name-or-id>` now use a read-only active-agent
  projection from the retained runtime's already-open SQLite pool and the same
  target-config service path. Unknown, inactive, ambiguous, active-work,
  and stale-result cases fail closed; a successful switch preserves the exact
  conversation/transcript/model, survives restart, routes the next run to the
  persisted target, creates no turn/run/event, and does not mutate AgentSpec.
- `crates/polkagent-cli/src/commands/chat.rs` implements retained-runtime
  interactive and non-TTY durable chat. It creates or resumes an interaction,
  accepts multiline input, streams typed updates with checkpoint resubscribe,
  and keeps assistant text on stdout while lifecycle/usage diagnostics use
  stderr. Shared handlers execute the truthful help/status/agents/agent/cancel/
  new/resume/model subset; unsupported provider/harness/autonomy, approval,
  inspection, and group commands fail explicitly. Process tests prove clean
  output, restart transcript/target/model resume, next-run target routing,
  zero-work command behavior, refusal, and SIGINT cancellation with durable
  terminal state.
- `crates/polkagent-api/src/durable.rs` provides strict SQLite-backed
  `RuntimeAgentStore` and `RuntimeRunManager` components over one
  `PolkagentRuntime`/`AppService`. They keep durable agents synchronized with
  the live registry and route run lifecycle changes through service methods.
  The production `serve` constructor injects them now. A strict artifact
  adapter also preserves classification and lineage and verifies BLAKE3 bodies
  after restart. The exact `AppService` tool registry backs deterministic API
  metadata/schema/grant reads without a second mutable registry. Read-only
  adapters also expose the exact immutable skill snapshot and the exact
  runtime-owned SQLite memory store for typed query and non-mutating lookup;
  configured/restart/disabled/unknown/auth/read-only cases are black-box tested.
  A machine-readable 11-route boundary keeps the remaining skill mutations,
  memory stats/deletion, audit, and registry APIs explicitly at 501 rather than
  installing in-memory substitutes.
  Versioned `/interactions` routes inject the exact runtime service for all
  execution/mutations and a read-only SQLite projection for finite durable
  event replay. A separate SSE adapter replays and follows the exact service
  stream using durable event IDs, `Last-Event-ID`, filtered checkpoints,
  keepalives, and gap-free lag recovery after the last emitted sequence.
  The shared hub attaches live delivery first and demand-loads at most one
  1,000-event replay page, avoiding full-backlog materialization without a
  publish/replay race.
  Separate event-ID reads no longer stop at 10,000 rows: the generic store
  fallback pages to completion with progress validation, SQLite performs an
  indexed point lookup, and API backend failures are sanitized.
  Black-box tests prove prompt transcript, caller-turn retry and conflict,
  idempotent cancel, target/model config updates, finite/live reconnect, restart,
  authentication/read-only policy, and an explicit uncomposed 501 response.
  Config GET/PUT exposes only the two implemented options, persists model
  clear/inheritance and same-provider validation across service reconstruction,
  isolates two sessions, rejects unsupported tags/extra fields without partial
  mutation, and creates no turn or run. The original target-only mutation
  remains as a deprecated compatibility delegate over the same setter.
- `crates/polkagent-runtime` is the production composition root for durable
  SQLite stores, providers/executors/harnesses, chain/tool registration, one
  event bus and `AppService`, startup recovery, active-agent rehydration, and
  structured readiness. `polkagent run`, TUI, and ACP now consume it; executable
  tests cover TUI runtime reuse, ACP abandoned-run recovery, and HTTP-created
  state across runtime restart. Runtime readiness explicitly reports missing
  signer, skill mutation/trust, policy/grant, shutdown, audit, and Postgres
  support; configured skill discovery and immutable API reads are composed.
- `crates/polkagent-interaction` freezes surface-neutral interaction IDs,
  target/config/prompt/turn types, structured event envelopes, replay-aware
  stream and service traits, a typed 12-command registry/parser, and shared
  command handlers. The SQLite adapter now persists safe session defaults,
  turn/run links, and exactly-once terminal event envelopes; the event hub
  combines durable replay with bounded live delivery and explicit lag recovery.
  `polkagent-runtime::DurableInteractionService` now implements create/list/
  load/archive, target/model configuration, prompt, cancel, and subscribe. It
  commits
  the user transcript and turn/run correlation before the first run event,
  atomically finishes assistant transcript plus terminal state, supports
  caller turn-ID idempotency, and backfills durable run events across lag and
  restart. Recovery paginates a stable session snapshot, closes the
  pre-activation crash window, deletes only unlinked prepared rows, and fails a
  completed turn closed when no durable output can reconstruct its transcript.
  Its paged transcript projection returns exact turn/run-linked user and
  assistant content and excludes unrelated low-level conversation messages.
  Model selection uses prompt > persisted interaction > agent precedence and
  applies only to the cloned prepared-run spec after canonical same-provider
  validation; concurrent sessions cannot mutate the shared agent. Provider,
  harness, autonomy, max-turn, budget, and approve/deny remain explicitly
  unsupported/unavailable. For model executors, the service sends
  the newest 32 completed typed user/assistant pairs from the latest 1,000
  prior records and appends the current user exactly once; failed/cancelled/
  timed-out partial pairs are excluded. String-only harness history fails with
  a typed unsupported error and a durable failed linked turn/run rather than
  flattening roles. TUI, terminal chat, HTTP, and ACP bind to this service.
- [`APPROVAL-PAUSE-RESUME-DESIGN.md`](APPROVAL-PAUSE-RESUME-DESIGN.md) records
  the audited approval blocker: current effect approval methods are bus-only and
  not a durable permission boundary. It freezes the required coordinator/run
  CAS/checkpoint design, default-deny rules, crash/disconnect invariants,
  parallel packets, and executable acceptance matrix before UI approval work.
- `crates/polkagent-harness-acp` is an ACP client for downstream coding-agent
  harnesses, not a Polkagent ACP agent server.
- `crates/polkagent-surface-acp` is the separate server-side adapter. The
  `acp_stdio_e2e` tests use the official Rust client to launch `polkagent acp`,
  negotiate ACP v1, discover commands, execute `/help`, complete a durable
  interaction turn, project effect-backed native tool-call/update messages,
  and cancel one while a provider request is active. The
  adapter maps each ACP session ID exactly to its conversation UUID and routes
  new/load/resume, prompt, cancel, agent selection, and model selection through
  `InteractionService`. An official-client restart fixture proves persisted
  transcript/config restoration, exact turn/run identities, same-turn retry
  without duplication, transcript replay on load, no replay on resume, and a
  successful follow-up after process replacement. It also asserts
  `Cancelled`, the reason-bearing durable terminal run state/timestamp,
  JSON-only successful-session stdout, redacted provider failures and backend
  panics, a payload-suppressing ACP panic hook, and empty stdout for a missing
  explicit config startup failure. ACP uses the shared registry for truthful
  help/status/agents/agent/model/cancel discovery and refuses registered commands it
  cannot execute. Native configuration advertises exactly an active-agent
  selector and the standard model selector, both persisted per interaction.
  A two-session race fixture proves each concurrent real provider request
  retains its own chosen agent/model without mutating the shared AgentSpec;
  unsupported group/auto targets, provider, harness, and autonomy settings are
  not advertised. A bounded
  checkpoint-aware update channel forwards typed interaction text before the
  terminal ACP response, reconciles the terminal prefix to prevent duplicate
  output, and emits truthful usage only when token counts and a model context
  window are known. Opt-in ACP JSONL diagnostics are bounded, pattern-redacted,
  restrictive-permission, no-follow opened, and proven not to contaminate
  protocol stdout or record exercised prompt/response bodies. Schema v17 also
  persists one immutable lexical origin cwd; every prompt/load/resume verifies
  it before replay/work, wrong/traversal roots fail with zero new events/turns/
  runs, and legacy rows fail closed for editor attachment. Session list/import,
  provider HTTP/SSE token streaming, permission round-trips, raw tool-data
  redaction, and manual Zed evidence remain open.
- `crates/polkagent-cli/tests/cross_surface_interaction_e2e.rs` freezes one
  exact SQLite conversation across HTTP creation/configuration, a real
  terminal-chat subprocess, ACP load/follow-up after subprocess restart, a
  reconstructed TUI controller/session selector plus `TestBackend`, and final
  HTTP/store projection. It proves exactly three ordered completed turns and
  linked unique runs, exact text/IDs/model/10-input+5-output usage/checkpoint,
  and zero work from HTTP config, chat `/model`, ACP `/status`, and an invalid
  model refusal. This closes EVD-11's restarted success/refusal contract;
  cancellation remains covered by the separate adapter-specific durable tests.
- `crates/polkagent-transport-pca::network::TcpPcaTransport` now exercises
  encrypted OS-socket I/O across separate processes with a durable inbox,
  outbox, deduplication, reconnect retry, restart redelivery, and typed
  cancellation/status/error frames with strict validation. This is a bounded
  transport seam, not yet the pinned PCA Statement Store/Polkadot App protocol
  or a shared-runtime surface; receiving a cancellation frame does not yet
  cancel a runtime run.
- `crates/polkagent-payment/src/ledger.rs` labels its signature as an
  experimental placeholder.
- PRD-17 now has an honest live-node target that queries relay/Asset Hub RPC and
  requires relay finality to advance. The actual CI network run is still needed
  as evidence, and no bytes are yet signed, submitted, matched, or reconciled.
- `scripts/container-smoke.sh` builds the locked canonical image and verifies
  Compose boot, non-root execution, read-only bind-mounted config-file behavior,
  clean SIGTERM HTTP drain, and container replacement on the same named volume.
  Authentication is enabled with a deterministic test-only key hash and rate
  limiting disabled: on initial, replacement, and restored instances, five
  public OpenAPI/health/PCA-health paths succeed without credentials while
  system/metrics/PCA-inbound/agent routes reject missing and invalid credentials
  with exact 401 bodies and accept the valid key. It uses real HTTP routes to
  create an agent, configured interaction, and prompt; records the exact
  conversation, turn, and run IDs plus the reason-bearing failed state and empty assistant
  output from an intentionally unreachable local provider; then requires the
  exact agent, interaction/config, turn, run, and transcript projections after
  replacement. It then stops the source, captures the whole volume read-only
  under the non-root runtime identity, verifies SHA-256 plus SQLite integrity
  and foreign keys, restores into a fresh Compose project/volume, and requires
  the same API projections. The smoke scans response JSON, metrics/headers,
  container logs, backup/summary, and uploaded artifacts for both plaintext test
  credentials, withholding secret-bearing diagnostics. The CI job runs
  independently of the Rust 1.89 MSRV matrix and uploads lifecycle,
  backup/manifest, log, and HTTP JSON artifacts. Successful production-backend
  output, TLS/key rotation/principal authorization, worker/run/effect recovery,
  online/encrypted/export backup, PostgreSQL recovery, retention, and
  upgrade/rollback remain open.
  The clean detached-worktree run at exact commit
  `e8adad340a6cbd1106a51f5961ee7f15cb6ee4ea` passed in 23 seconds with 15
  public successes, 24 exact missing/invalid 401s, 12 valid-key protected
  successes, SIGTERM exit 0, identical replacement/restore projections, a
  1,945,600-byte SHA-256-verified archive, SQLite integrity `ok`, and independent
  plaintext credential scans.
- CI checks the workspace on stable Rust and the declared Rust 1.89 MSRV, and
  enforces `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` on
  stable. Both the exact workspace Clippy command and the stronger local
  all-target/all-feature variant now pass; the hosted CI run remains external
  evidence to collect.
- The slow/ignored CI pass is scoped to Rust test targets so it runs the PCA
  subprocess helpers without asking rustdoc to compile illustrative
  adapter-placeholder examples marked `ignore`.

## Completion gate for future status updates

Before changing any row to end-to-end verified, attach all of these:

1. The exact executable entry point and production composition builder.
2. The persistent records created and restart/recovery behavior.
3. The real external adapter used; name any remaining fake explicitly.
4. Happy-path and critical failure-path test names.
5. A user/client transcript or machine-readable artifact.
6. Security and observability evidence.
7. Documentation/configuration changes.

If one item is absent, keep the row partial and add a backlog item rather than
estimating a completion percentage.

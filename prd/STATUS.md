# Polkagent implementation status

**Evidence snapshot:** 2026-08-05
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

The final uncontended all-feature workspace run exited 0 with 7,871 ordinary
tests and 84 doc tests passing, zero failures, and 28 ignored tests. It includes
extensive unit, property, contract, integration, security, TUI rendering, API,
and doc coverage. Twenty-four live PostgreSQL tests returned early because
`TEST_DATABASE_URL` was unset, so they are not external-database evidence even
though Cargo reports them as passed. This is a strong component baseline. The
container smoke additionally proves that the
locked canonical image builds, starts unprivileged, honours a bind-mounted
read-only config, answers the three HTTP probes, drains HTTP on SIGTERM with a
clean exit, and replaces the container on the same named volume while retaining
a SQLite CLI marker. A focused official-SDK subprocess suite also proves ACP
initialize/new/prompt, a real `AppService` run, and cancellation while a
provider request is active. The client receives the `Cancelled` stop reason,
and SQLite retains the reason-bearing terminal state plus completion timestamp.
The same suite checks that successful-session stdout lines are JSON and that a
missing explicit config exits 4 with empty stdout and its diagnostic on stderr.
Focused TUI tests cover reducer/input/render behavior, grapheme-safe editing,
bounded bracketed paste, multiline prompts, durable follow-up/restart, stale-
load race protection, and exact cancellation. Terminal-chat subprocess tests
cover clean non-TTY output, durable transcript resume, unsupported
configuration, and SIGINT cancellation. HTTP interaction tests cover retry/
conflict, filtered checkpoint replay, cancellation, restart, auth, and read-only
mode. A Unix PTY
subprocess test now proves actual Crossterm escape ordering and termios
restoration on normal exit, ordinary error, and caught-panic unwind; Windows
ConPTY remains unproved. These checks do not prove worker/effect draining,
Postgres, backup/restore, tenant isolation, HA, a real chain action, role-safe
harness multi-turn input, cross-surface conformance, or complete Zed/editor
behavior.

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
`$ref` null semantics, and keep router parity exact. Redocly 2 reports zero
dialect errors and 22 remaining style warnings.

## Product-surface status

| Surface/capability | Component state | Product state | Decisive gap |
|---|---|---|---|
| One-shot CLI run | Uses the shared `RuntimeFactory`; subprocess and durable-restart coverage pass | Partially usable | All primary executable surfaces now share the factory, but tools/effects/policy are not a real model loop. |
| Monitoring TUI | Rich views plus a durable F9 Console, grapheme-safe multiline editor/history/paste, executable shared-command subset, guarded terminal lifecycle, and a durable session selector | Actionable for one turn at a time and restart-resumable | Prompt/follow-up/live typed output/cancel, bounded contextual model-executor history, help/status/new/resume/model, and exact same-agent session switching run through shared services; broader commands, simultaneous orchestration, harness context, and service-routed approvals remain. |
| REST/WebSocket API | `serve` uses the strict shared runtime plus durable core stores, exact runtime tool discovery, and the exact runtime `InteractionService` | Durable interaction/control-plane slice with ordinary HTTP/OpenAPI route parity | Versioned interaction lifecycle, strict persisted target/model configuration, finite replay, and checkpointed SSE survive restart and match tested OpenAPI schemas; 15 optional skill/memory/audit/registry routes, two separately documented WebSocket transports, full shutdown, and cross-surface E2E remain. |
| Interactive terminal chat | `polkagent chat` uses the durable runtime interaction service and shared command handlers | Usable single-agent, model-selectable line-mode session | Interactive/non-TTY prompt, multiline input, contextual model-executor follow-up, transcript resume, persisted conversation-scoped `/model`, lag replay, and SIGINT cancellation work; agent/provider/harness/autonomy changes, approvals, harness follow-up, rich content, and groups are explicitly unavailable. |
| ACP from Polkagent to other harnesses | ACP client exists and tests pass | Useful downstream adapter | This is client-side harness support only. |
| Polkagent inside Zed/ACP clients | Official-SDK ACP v1 stdio slice with shared-registry discovery, native agent/model selectors, and executable subprocess coverage | Executable protocol slice; editor interoperability and UX unverified | No durable list/load/import or shared conversations; provider/target/autonomy selectors, structured tools/permissions, MCP passthrough, and Zed tool/approval/restart smoke remain. |
| Providers/harnesses | Many adapters exist | Partially composed | Each adapter needs shared-runtime conformance and real failure/readiness evidence. |
| Tools/skills | Registries and handlers exist | Not actionable in normal run loop | Orchestrator sends no tool schemas and synthesizes tool success instead of executing. |
| Effects/approvals/policy | Strong domain libraries | Incomplete execution path | Effect pipeline and grant resolver are not used by the central orchestrator. |
| Conversations/memory | `InteractionService` atomically persists user/assistant transcript, turn/run correlation, terminal state, replay, and bounded typed model context | Headless service, TUI, terminal chat, and HTTP API consume the durable lifecycle | ACP still uses an ephemeral session path; string-only harness history fails explicitly, approvals are unavailable, and general memory is not assembled into normal context. |
| Groups/feeds/evals | Significant libraries/tests | Mostly unsurfaced | No production caller creates durable child runs or evaluates the real composed runtime. |
| Polkadot reads | RPC/metadata/codec components exist | Partially usable | Pinned live metadata and network behavior need real-path validation. |
| Polkadot writes | Effect/signing/finality components exist | Not end-to-end proven | Real signer, exact bytes, transaction matching, finality, dry-run/XCM, and local-chain tests remain. |
| PCA compatibility | Crypto/queue/sync building blocks plus durable TCP text, cancellation, status, and error peer I/O exist | Cross-process protocol slice; not yet PCA-reference compatible or runtime-composed | The TCP adapter needs Statement Store/Polkadot App adaptation, signed identity, runtime cancellation/reply mapping, attachments, and reference fixtures. |
| Security | Grants, tests, redaction, signer abstractions exist | Not production hardened | Plaintext file secrets, shared-key API auth, mock KMS/DID paths, and unused policy runtime. |
| Payments | Intent/store/budget components exist | Not value-moving | Store/runtime integration, real signature/settlement, and failure reconciliation remain. |
| Marketplace/plugins | Durable local plugin/kit lifecycle, operator CLI, and listing components | Local install/list/get/update/rollback/uninstall is actionable; package execution is missing | Manifest/lock format remains split; no API/runtime activation, actual sandbox engine, or cryptographic trust pipeline. |
| Deployment/cloud | Canonical image/Compose boot, mounted config, health, graceful HTTP stop, and same-volume replacement smoke pass | Bounded single-instance lifecycle only | Durable API/run recovery, worker/effect drain, Postgres/tenant isolation, backup/restore, auth, release, HA, and control/worker paths remain unproven. |

## PRD implementation posture

| PRD | Component maturity | Production wiring | User-visible E2E | Disposition |
|---|---|---|---|---|
| 01 Vision | N/A | N/A | N/A | Active normative direction |
| 02 Architecture | Strong but drifted | Partial | No | Active invariants; reconcile when touched |
| 03 Execution | Strong libraries | Blocked at central loop | No | Active P0 |
| 04/04a Providers/tools/harnesses | Strong adapters | Partial | Partial one-shot only | Active P0/P1 |
| 05 Polkadot | Strong read/action components | Partial | No real write proof | Active P1 + PRD-17 |
| 06 PCA | Strong primitives plus tested cross-process TCP/control delivery | Transport not composed into runtime or PCA reference network | No runtime or reference-client E2E | Active P1 |
| 07 Security | Strong primitives/tests | Partial/unsafe defaults | No production security proof | Active P1 |
| 08 Payments | Domain/store components | Missing from runtime | No | Active P2 after safe action path |
| 09 Memory/groups/evals | Strong components | Mostly missing | No orchestration proof | Active P1 |
| 10 Observability | Strong components | Partial | No recovery/replay proof | Active P1 |
| 11 Deployment/cloud | Container boot/config/HTTP drain/same-volume replacement verified; broader scaffolding exists | Single-instance SQLite only | CLI marker persistence, not durable API/run recovery | Active P2 |
| 12 Marketplace/extensions | Durable local lifecycle and CLI | Operator management works; execution missing | No install-to-run proof | Active P2 |
| 13 UX | Durable terminal chat plus monitoring TUI with durable actionable Console | Partial | Target-only contextual model-executor follow-up/restart/cancel and a command subset are tested; harness context, approvals, broader commands, and orchestration remain | Active P0/P1 + PRD-19 |
| 14 API/config | Shared-runtime durable core, interaction routes, and broad control-plane routes | Agent/run/artifact/tool/interaction reads and mutations plus checkpointed interaction SSE are composed; 15 optional skill/memory/audit/registry routes remain explicitly unavailable | HTTP interaction retry/cancel/replay/live reconnect/restart/auth/read-only proof, zero-drift ordinary HTTP parity, and an OpenAPI 3.1-valid schema exist; two WebSocket frame protocols are separately documented | Active P0/P1 |
| 15 Testing | Broad passing check/test/rustdoc suite; mandatory and extended Clippy gates are locally green | Production paths remain under-tested | Hosted CI confirmation plus live/client/ops gates missing | Active cross-cutting |
| 17 Local testnet | Pinned native fixture, provisioning, CI gate, and live RPC/finality test target | Read-only baseline wired; signed action path missing | No real write proof; CI network artifact pending | Active P1 |
| 19 Interactive/ACP | ACP server, durable terminal chat/TUI, shared runtime, headless interaction service, HTTP adapter, and command handlers implemented | TUI, chat, and API consume `InteractionService`; ACP reuses the runtime but still owns an ephemeral run/session mapping | Headless and surface tests prove transcript/correlation, typed contextual model-executor follow-up, retry/cancel/replay/restart; harness context, approvals/tools, full Zed, and cross-surface conformance remain | Active P0/P1 |

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
- `crates/polkagent-run/src/orchestrator.rs` sends an empty tool list and does
  not drive the configured effect pipeline/grant resolver through real tool
  calls and approvals.
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
- `crates/polkagent-cli/src/commands/chat.rs` implements retained-runtime
  interactive and non-TTY durable chat. It creates or resumes an interaction,
  accepts multiline input, streams typed updates with checkpoint resubscribe,
  and keeps assistant text on stdout while lifecycle/usage diagnostics use
  stderr. Shared handlers execute the truthful help/status/cancel/new/resume/
  model subset; unsupported configuration, approval, inspection, and group commands
  fail explicitly. Process tests prove clean output, restart transcript resume,
  persisted conversation-model selection, refusal, and SIGINT cancellation
  with durable terminal state.
- `crates/polkagent-api/src/durable.rs` provides strict SQLite-backed
  `RuntimeAgentStore` and `RuntimeRunManager` components over one
  `PolkagentRuntime`/`AppService`. They keep durable agents synchronized with
  the live registry and route run lifecycle changes through service methods.
  The production `serve` constructor injects them now. A strict artifact
  adapter also preserves classification and lineage and verifies BLAKE3 bodies
  after restart. The exact `AppService` tool registry backs deterministic API
  metadata/schema/grant reads without a second mutable registry. A
  machine-readable 15-route boundary keeps unsupported skill/memory/audit/
  registry APIs explicitly at 501 rather than installing in-memory substitutes.
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
  signer, configured-skill, policy/grant, shutdown, audit, and Postgres support.
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
  flattening roles. TUI, terminal chat, and HTTP bind to this service; ACP
  migration remains open.
- `crates/polkagent-harness-acp` is an ACP client for downstream coding-agent
  harnesses, not a Polkagent ACP agent server.
- `crates/polkagent-surface-acp` is the separate server-side adapter. The
  `acp_stdio_e2e` tests use the official Rust client to launch `polkagent acp`,
  negotiate ACP v1, discover commands, execute `/help`, complete a real
  `AppService` run, and cancel one while a provider request is active. They
  assert `Cancelled`, the reason-bearing durable terminal run state/timestamp,
  JSON-only successful-session stdout, redacted provider failures and backend
  panics, a payload-suppressing ACP panic hook, and empty stdout for a missing
  explicit config startup failure. ACP uses the shared registry for truthful
  help/status/agent/model/cancel discovery and refuses registered commands it
  cannot durably execute. Native session configuration advertises exactly an
  active-agent selector and the standard model selector. A two-session race
  fixture proves each concurrent real provider request retains its own chosen
  agent/model; unsupported provider/target/autonomy settings are not
  advertised. A bounded update channel forwards real runtime text before the
  terminal ACP response, reconciles the terminal prefix to prevent duplicate
  output, and emits truthful usage only when token counts and a model context
  window are known. Opt-in ACP JSONL diagnostics are bounded, pattern-redacted,
  restrictive-permission, no-follow opened, and proven not to contaminate
  protocol stdout or record exercised prompt/response bodies. ACP now retains
  one `PolkagentRuntime`; an official-client restart fixture proves startup
  recovery on the same durable database. Selections are process-local because
  session persistence, provider HTTP/SSE token streaming, permissions/tools,
  and manual Zed evidence remain open.
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
  Compose boot, non-root execution, `/health/{live,ready,startup}`, read-only
  bind-mounted config behavior, clean SIGTERM HTTP drain, container replacement
  on the same named volume, and restart-safe SQLite CLI marker lookup. The CI
  job runs independently of the Rust 1.89 MSRV matrix and uploads selected
  lifecycle state/log artifacts. Durable API interaction/run restart now has
  local black-box coverage, but the container smoke still proves only the CLI
  marker across replacement rather than an HTTP-created interaction lifecycle.
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

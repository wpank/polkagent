# Polkagent implementation status

**Evidence snapshot:** 2026-08-05
**Conclusion:** broad component maturity; incomplete production composition;
no PRD verified complete end-to-end

## Verification baseline

The current worktree passed:

```text
cargo check --workspace
cargo test --workspace --no-fail-fast
cargo test -p polkagent-cli --test acp_stdio_e2e
./scripts/container-smoke.sh
```

The workspace test run exited 0 and includes extensive unit, property,
contract, integration, security, TUI rendering, API, and doc tests. This is a
strong component baseline. The container smoke additionally proves that the
locked canonical image builds, starts unprivileged, honours a bind-mounted
read-only config, answers the three HTTP probes, drains HTTP on SIGTERM with a
clean exit, and replaces the container on the same named volume while retaining
a SQLite CLI marker. A focused official-SDK subprocess test also proves one ACP
initialize/new/prompt session and a real `AppService` run. These checks do not
prove durable API/run recovery, worker/effect draining, Postgres,
backup/restore, auth, HA, a real chain action, an interactive TUI prompt, or
complete Zed/editor behavior.

The audit intentionally treats tests such as “returns 501 when store is not
configured” as contract coverage and simultaneous evidence that production
startup still has a wiring gap.

## Product-surface status

| Surface/capability | Component state | Product state | Decisive gap |
|---|---|---|---|
| One-shot CLI run | Substantial and tested | Partially usable | Bootstrap is embedded in the command; tools/effects/policy are not a real model loop. |
| Monitoring TUI | Rich views and rendering tests | Not actionable as an agent console | No prompt/session/start/cancel path; direct DB mutations bypass services. |
| REST/WebSocket API | Broad route and middleware coverage | Not a durable production control plane | `serve` uses in-memory agents/runs and omits most optional stores/registries, producing many 501s. |
| Interactive terminal chat | No shared surface | Missing | Requires `InteractionService`, command registry, and runtime factory. |
| ACP from Polkagent to other harnesses | ACP client exists and tests pass | Useful downstream adapter | This is client-side harness support only. |
| Polkagent inside Zed/ACP clients | Official-SDK ACP v1 stdio slice with executable subprocess coverage | Protocol-usable MVP; manual Zed support unverified | No durable list/load/import, shared conversations/config registry, structured tools/permissions, MCP passthrough, or Zed tool/approval/restart smoke. |
| Providers/harnesses | Many adapters exist | Partially composed | Each adapter needs shared-runtime conformance and real failure/readiness evidence. |
| Tools/skills | Registries and handlers exist | Not actionable in normal run loop | Orchestrator sends no tool schemas and synthesizes tool success instead of executing. |
| Effects/approvals/policy | Strong domain libraries | Incomplete execution path | Effect pipeline and grant resolver are not used by the central orchestrator. |
| Conversations/memory | Durable stores and migrations exist | Not a shared multi-turn experience | Runs are not durably linked to interaction turns; memory is not assembled into normal context. |
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
| 13 UX | CLI/TUI exist | Partial | Interactive experience missing | Active P0/P1 + PRD-19 |
| 14 API/config | Broad components/routes | P0 composition gap | No durable control-plane proof | Active P0/P1 |
| 15 Testing | Broad green suite | Production paths under-tested | Live/client/ops gates missing | Active cross-cutting |
| 17 Local testnet | Pinned native fixture, provisioning, CI gate, and live RPC/finality test target | Read-only baseline wired; signed action path missing | No real write proof; CI network artifact pending | Active P1 |
| 19 Interactive/ACP | Initial ACP server implemented; TUI architecture specified | ACP independently composes `AppService`; shared interaction/runtime factory remains missing | Official client proves command discovery and prompt/run; no full Zed or interactive TUI E2E | Active P0/P1 |

## Decisive implementation evidence

- `crates/polkagent-cli/src/commands/serve.rs` constructs
  `InMemoryAgentStore` and `InMemoryRunManager`.
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
- `crates/polkagent-cli/src/tui/app.rs` owns a database pool and polls it; it
  does not own the application/interaction runtime or a live event receiver.
- `crates/polkagent-harness-acp` is an ACP client for downstream coding-agent
  harnesses, not a Polkagent ACP agent server.
- `crates/polkagent-surface-acp` is the separate server-side adapter. The
  `acp_stdio_e2e` test uses the official Rust client to launch `polkagent acp`,
  negotiate ACP v1, discover commands, execute `/help`, and complete a real
  `AppService` run. Session persistence, permissions/tools, and manual Zed
  evidence remain absent.
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
  lifecycle state/log artifacts. API agents and runs are still in-memory.

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

# Polkagent implementation backlog and parallel agent plan

**Status:** canonical execution queue
**Updated:** 2026-08-05
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
  CHAIN-01 live chain | SEC-01 auth/custody | PCA-01 networking

Later productization:
  OPS-01 deployment | EXT-01 marketplace | PAY-01 settlement
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

### FND-01 — One production runtime composition root

**Goal:** CLI run, serve, TUI, chat, ACP, and transports construct the same
durable application runtime.

**Scope/ownership:** a new `polkagent-runtime` crate or a clearly isolated
`polkagent-service::runtime` module; production composition tests; migration of
`commands/run.rs` and `commands/serve.rs`. This packet owns runtime construction
but not TUI widgets or ACP protocol mapping.

**Checklist:**

- [ ] Define `RuntimeOptions`, `PolkagentRuntime`, readiness report, and
  `RuntimeFactory::build`.
- [ ] Centralize config-path provenance, database path, provider/model/harness,
  chain, signer, tools/skills, policy/grants, and read-only selection.
- [ ] Open/migrate durable run, effect, event, artifact, conversation, memory,
  payment, audit, agent, group/feed, and registry stores as applicable.
- [ ] Remove `NoopEffectStore` and in-memory production fallbacks from
  executable configurations; missing required dependencies fail at startup.
- [ ] Rehydrate agents and recover/timeout stuck work.
- [ ] Migrate one-shot `run` without behavior regression.
- [ ] Make `serve` use durable agents/runs and inject every advertised route
  dependency, or deliberately remove/disable the route at startup.

**Exit checks:** a production-composition integration test starts runtime on a
temporary SQLite database, creates a run through service and API paths, observes
the same durable record/events, restarts, and reloads it. No production API
route unexpectedly returns 501 because startup forgot a store.

**Depends on:** none. This is the critical-path integration packet.

### FND-02 — Durable interaction, command, and event contracts

**Goal:** one surface-neutral multi-turn contract for terminal, TUI, API, ACP,
and group orchestration.

**Scope/ownership:** new `polkagent-interaction` crate, conversation/interaction
types, typed command registry, projections, and additive SQLite migrations.
Avoid editing individual TUI/ACP renderers.

**Checklist:**

- [ ] Implement conversation-target/config, prompt request, turn handle, and
  stable interaction/tool/approval/plan/usage event types from PRD-19.
- [ ] Add `interaction_turns` and `interaction_turn_runs` with idempotent IDs
  and run correlation at creation time.
- [ ] Persist user message before execution and assistant/tool/terminal state
  exactly once.
- [ ] Implement new/list/load/delete/prompt/cancel/config/approve/deny/subscribe.
- [ ] Implement bounded live broadcast plus durable checkpoint/projection
  recovery when a subscriber lags.
- [ ] Add one typed command registry for `/help`, `/status`, `/agents`,
  `/agent`, `/new`, `/resume`, `/runs`, `/inspect`, `/cancel`, `/approve`,
  `/deny`, and `/model`.

**Exit checks:** headless tests create, prompt, stream, cancel, restart, load,
resume, approve, and deny without depending on CLI, Ratatui, Axum, or ACP types.

**Depends on:** agree the minimal runtime handle with FND-01. Implementation can
proceed in parallel after that interface is frozen.

### EXE-01 — Real tool/effect/policy/approval execution loop

**Goal:** replace synthetic model tool success with the durable safe-action
pipeline required by PRD-03/04/07.

**Scope/ownership:** `polkagent-run` orchestration and service bridges. This
packet owns the orchestrator hot files; FND-01 only injects its dependencies.

**Checklist:**

- [ ] Advertise effective tool schemas to model inference.
- [ ] Normalize and validate tool calls with stable IDs.
- [ ] Resolve grants/policy/budgets before dispatch.
- [ ] Persist `EffectIntent` before external I/O.
- [ ] Execute through `ToolRegistry`/effect worker, not fabricated JSON.
- [ ] Pause durably for approval and resume the exact call/effect.
- [ ] Record tool output/artifacts/evidence and feed it into the next model turn.
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

- [ ] Implement durable `AgentStore` and `RunManagerTrait` adapters over the
  shared runtime/stores.
- [ ] Inject event, artifact, conversation, memory, payment, audit, skill,
  tool, registry, and service-registry dependencies.
- [ ] Make create/cancel/resume endpoints operate on real work.
- [ ] Add cursor/replay semantics and explicit stream-gap recovery.
- [ ] Reconcile OpenAPI with runtime routes including WebSocket, metrics,
  audit, conversation, and registry surfaces; decide SSE deliberately.
- [ ] Unify root config selection, bind address/port, auth, CORS, read-only,
  and rate-limit provenance.

**Exit checks:** black-box API test creates an agent and interaction, streams
real events/tool approval, cancels, restarts the server, and reads identical
state. OpenAPI conformance and authenticated/read-only tests pass.

**Depends on:** FND-01; consumes FND-02 for multi-turn APIs.

## P1 — real external behavior and actionable surfaces

### OBS-01 — Durable event replay, projections, and operator evidence

**Goal:** no surface silently loses the state needed to understand active work.

**Checklist:**

- [ ] Persist user-visible deltas/checkpoints with stable event and sequence IDs.
- [ ] Replace silent broadcast-lag drops with a gap event and projection reload.
- [ ] Remove fixed first-10k scans from event lookup.
- [ ] Wire telemetry, audit, retention, backup, and recovery into lifecycle.
- [ ] Add restart, slow-consumer, duplicate, retention, and corrupted-projection
  tests plus operator diagnostics.

**Ownership:** event/artifact/telemetry/projection modules; coordinate API/TUI
adapter hooks through small interfaces.

**Depends on:** FND-01 and event contract from FND-02.

### TUI-01 — Interactive terminal chat and actionable TUI

**Goal:** users can prompt and orchestrate without leaving the terminal UI.

**Checklist:**

- [ ] Add `polkagent chat` using `InteractionService` and shared commands.
- [ ] Convert the TUI loop to async/channel-driven input, runtime events, and
  background completion.
- [ ] Pass a runtime/interaction handle, not only a SQLite pool.
- [ ] Add conversation workspace, Unicode/multiline composer, history,
  completion, streaming transcript, tool/plan/approval/usage/error rendering.
- [ ] Add start/follow-up/cancel/approve/deny/create-select-agent actions.
- [ ] Remove UI direct DB mutations.
- [ ] Preserve all monitoring tabs and terminal restoration on panic/error.

**Exit checks:** user can launch, select/create an agent, prompt, see tokens and
tools, approve/deny, cancel, prompt again, restart, and resume. Headless event
loop and snapshot tests cover narrow/resized terminals and simultaneous runs.

**Ownership:** CLI chat/TUI modules only. **Depends on:** FND-01, FND-02, and
the stable execution event path; can run fully parallel to ACP-01.

### ACP-01 — Polkagent ACP server and Zed integration

**Goal:** Zed and other ACP clients can use Polkagent as an external agent.

**Checklist:**

- [ ] Add a dedicated `polkagent-surface-acp` server crate using the official
  ACP Rust SDK; retain `polkagent-harness-acp` as the separate downstream client.
- [ ] Add protocol-safe early `polkagent acp` stdio dispatch so stdout contains
  JSON-RPC frames only.
- [ ] Implement initialize, new/list/load session, prompt, cancel, permission,
  tool/plan updates, and terminal stop reasons.
- [ ] Map shared slash commands and dynamic target/model/autonomy options.
- [ ] Validate cwd, supplied MCP servers, secret redaction, disconnect/default
  deny, and concurrent/busy session behavior.
- [ ] Add ACP transcript/conformance fixtures and a Zed custom-agent guide.

**Exit checks:** in Zed, add the custom external agent, prompt, observe a real
tool call, approve/deny, cancel, restart/import thread, and inspect clean ACP
logs. Protocol fixtures pass without TUI/CLI-specific behavior.

**Ownership:** new ACP surface crate and focused CLI early dispatch. **Depends
on:** FND-01/FND-02; runs parallel to TUI-01 and API-01 after contracts freeze.

### CHAIN-01 — Real local-chain read/write/finality slice

**Goal:** close the gap between fake-backed action tests and a real Polkadot
network before claiming safe writes.

**Checklist:**

- [ ] Repair PRD-17 fixture/workflow variable, endpoint-scheme, and binary
  assumptions.
- [ ] Ensure tests actually consume live RPC environment variables.
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
- [ ] Implement production auth/session/JWT validation, constant-time secret
  comparison, principal/tenant/RBAC context, and tenant filters.
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

- [ ] Implement actual peer/application I/O around existing crypto/sync types.
- [ ] Enforce durable-before-ACK ingress, leases, dedupe, retry, ordering,
  reconnect, restart, attachment limits, and backpressure.
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

- [ ] Fix Docker command/port/config drift and add container health smoke test.
- [ ] Prove Postgres conformance and tenant isolation.
- [ ] Define migrations, backup/restore, upgrade/rollback, resource limits,
  graceful shutdown, and release artifacts.
- [ ] Only then connect control/worker services and add Helm/Kubernetes/HA.

**Depends on:** FND-01, API-01, OBS-01, SEC-01.

### EXT-01 — Installable and safely executable extensions

- [ ] Freeze canonical manifest/package/lock format.
- [x] Build a durable local plugin/kit install/list/update/history/rollback/
  uninstall store with immutable version content, atomic state replacement,
  cross-process locking, restart validation, idempotency, and BLAKE3 integrity.
- [ ] Expose the local lifecycle through CLI/API surfaces and connect installed
  packages to real runtime execution (`run`).
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

## Recommended parallel-agent waves

| Wave | Concurrent lanes | Integration gate |
|---|---|---|
| 0 | FND-01 runtime interface; FND-02 data/event interface; CHAIN-01 workflow repair; SEC-01 backend research/spikes | Freeze runtime handle, interaction events, principal context, and migration ownership. |
| 1 | FND-01 composition; FND-02 headless interaction; EXE-01 orchestrator; CHAIN-01 real tests | Production composition test plus real tool-call test. |
| 2 | API-01; TUI-01; ACP-01; OBS-01; PCA-01 | Same conversation/run can be initiated and inspected across clients; restart and lag tests pass. |
| 3 | ORC-01; OPS-01 container/Postgres; EXT-01 local package lifecycle; PAY-01 design/spike | Completion gate from `STATUS.md`; no fake path hidden by surface tests. |

At each wave boundary, one integration agent should own workspace manifests,
CLI command registration, SQLite migration registration, and the final
cross-surface test. Other agents should not concurrently edit those hot files.

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

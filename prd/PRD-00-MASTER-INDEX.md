# PRD-00 — Master index and roadmap authority

**Status:** active index
**Audited:** 2026-08-06
**Scope:** current `main` worktree after the 2026-08-06 implementation audit

## Executive state

Polkagent has a broad passing Rust check/test/rustdoc suite, and both the
mandatory workspace Clippy command and a stronger all-target/all-feature gate
are locally green. Bounded product slices now exist for the TUI, ACP, local
packages, PCA transport, and container lifecycle, but no PRD is complete
end-to-end under the repository's completion rule. A production runtime,
durable interaction/session/event service, and shared command handlers now
exist. TUI, terminal chat, HTTP, and ACP now consume the interaction service;
one exact restarted conversation is proven across all four surfaces, and a
bounded grantless registered-tool loop now persists intent before real handler
I/O and projects one stable effect-backed tool identity through terminal/TUI/
ACP. The TUI additionally has a bounded simultaneous agent/conversation
activity slice with exact switching and cancellation. The durable approval
coordinator/executor and explicit local-process authority for ACP, terminal
chat, and `polkagent tui` now exist. One APR-08 TUI/chat happy-path and restart
fixture proves exact identities, one handler attempt/outcome, and no pending or
orphan approval rows. The dominant gaps are authenticated shared/remote/API
authority, the broader APR-08 crash and reconciliation matrix, durable group/
child-run orchestration, provider/harness parity, rich plans, and manual-editor
evidence. Defaults and the production API remain authority-unbound; the local
tuple is process-owner assertion, not multi-principal authentication.

The next milestone is therefore not “add more crates.” It is closing the
remaining approval/security/crash matrix, validating the implemented ACP
permission path in a real editor, adding authenticated shared-service
authority, and extending the proven interaction runtime into durable group
orchestration.

Current execution truth lives in:

- [`STATUS.md`](STATUS.md)
- [`IMPLEMENTATION-BACKLOG.md`](IMPLEMENTATION-BACKLOG.md)
- [`EVIDENCE-BACKLOG.md`](EVIDENCE-BACKLOG.md)

## Maturity language

| Level | Meaning |
|---|---|
| Specified | Requirement/design exists, but no relevant implementation proof. |
| Component-tested | Library/adapter exists and focused tests pass. |
| Composed | Production startup wires the capability with durable dependencies. |
| End-to-end verified | A real surface and real adapter complete the happy path and critical failures. |
| Operationally proven | Restart, recovery, security, observability, packaging, and runbooks are validated. |

Do not collapse these levels into a single “done” checkmark or progress
percentage.

## Active normative suite

| Document | Role | Current posture |
|---|---|---|
| [PRD-01](PRD-01-VISION-PRINCIPLES-PERSONAS.md) | Vision, principles, personas, pillars | Normative product direction; implementation prose is historical. |
| [PRD-02](PRD-02-VOCABULARY-ARCHITECTURE.md) | Vocabulary, invariants, architecture | Preserve invariants; reconcile sketches with current types during touched work. |
| [PRD-03](PRD-03-EXECUTION-MODEL.md) | Runs, turns, effects, recovery | Active; bounded grantless execution plus durable approval pause/decision/recovery and one cross-surface restart seam exist. The full crash-boundary matrix, possible-I/O reconciliation, handler-I/O cancellation, and effect draining remain P0. |
| [PRD-04](PRD-04-PROVIDERS-MODELS-TOOLS.md) | Providers, models, harnesses, tools, skills | Active; grantless allowlisted tools and one strict-policy approval-gated governance tool execute in bounded model loops. Schema-wide safe projection and external-adapter conformance remain. |
| [PRD-04a](PRD-04a-PROVIDER-HARNESS-EXPANSION.md) | Provider/harness expansion | Active component scope; prove each adapter through the shared runtime. |
| [PRD-05](PRD-05-POLKADOT-INTEGRATIONS.md) | Polkadot read/write integrations | Active; live finality, signing, dry-run/XCM, and action E2E remain. |
| [PRD-06](PRD-06-PCA-COMPATIBILITY.md) | PCA compatibility and transport | Active; durable encrypted TCP/control delivery is cross-process tested, but reference PCA framing, signed identity, attachments, and runtime composition remain. |
| [PRD-07](PRD-07-IDENTITY-SECURITY.md) | Identity, grants, policy, secrets, signers | Active; strict policy enforcement and exact local-process approval authority are composed. Shared/remote principal authentication, API mapping, TLS/key rotation, secret custody, audit, and tenant enforcement remain. |
| [PRD-08](PRD-08-PAYMENTS-AUTONOMY.md) | Payments, budgets, autonomy | Active; domain code is not a value-moving composed product. |
| [PRD-09](PRD-09-MEMORY-GROUPS-EVALS.md) | Memory, groups, feeds, evals | Active; runtime-owned durable memory query/exact lookup/non-mutating stats/atomic deletion are API-composed, while prompt-context, groups, feeds, and eval orchestration remain. |
| [PRD-10](PRD-10-DATA-OBSERVABILITY.md) | Events, artifacts, telemetry, recovery | Active; interaction SSE, exact durable run-event metadata, global replay/reconnect, and versioned command-socket cursor/lag recovery have bounded real-TCP evidence, while trace/context injection, best-effort deltas, audit/telemetry, retention, and operator recovery remain. |
| [PRD-11](PRD-11-DEPLOYMENT-CLOUD.md) | Deployment and cloud | Active; authenticated single-instance container/config/shutdown/replacement/cold-restore persistence is proven, while successful backend output and production operations remain. |
| [PRD-12](PRD-12-MARKETPLACE-EXTENSIONS.md) | Extensions and marketplace | Active; durable local package lifecycle and operator CLI work, while activation, sandbox execution, cryptographic trust, and registry paths remain. |
| [PRD-13](PRD-13-UX-SURFACES.md) | CLI, TUI, web/mobile surfaces | Active; durable terminal chat and the TUI Console provide contextual follow-up, persisted selection, safe tool status, restart, bounded simultaneous activities, and selected-conversation approval actions. Explicit local authority and coordinator-backed pending cancellation work; shared/remote/API authority, pagination, handler-I/O cancellation, harness context, group orchestration, and studio/mobile surfaces remain. |
| [PRD-14](PRD-14-APIS-SCHEMAS-CONFIG.md) | APIs, schemas, configuration | Active; shared-runtime interaction/core/skill/memory operations, checkpointed SSE, both bounded WebSocket reconnect protocols, zero-drift ordinary HTTP parity, and an OpenAPI 3.1 contract exist, while 9 optional routes and broader command-socket channels remain. |
| [PRD-15](PRD-15-TESTING-ROADMAP.md) | Testing and release gates | Active; broad component coverage, stable/Rust-1.89 checks, strict rustdoc, and mandatory/extended Clippy gates pass locally; production/live/client conformance remains incomplete. |

## Active delivery PRDs

| Document | Delivery outcome | Dependency |
|---|---|---|
| [PRD-17](PRD-17-LOCAL-TESTNET-E2E.md) | Real local Polkadot network, signed actions, finality, and reproducible CI evidence | Runtime/action wiring, real signer, corrected live-test workflow |
| [PRD-19](PRD-19-INTERACTIVE-CONSOLE-ACP.md) | Terminal chat, actionable TUI, shared commands, ACP server, Zed, and orchestration | Shared runtime and interaction contracts |
| [Approval pause/resume design](APPROVAL-PAUSE-RESUME-DESIGN.md) | Atomic durable permission coordinator, checkpoint recovery, and cross-surface approval | APR-00 through APR-07 have bounded implementations; one APR-08 happy-path/restart fixture is complete, while the broader packet remains active |

PRD-19 supersedes the implementation role of archived PRD-18 while preserving
its useful prompt/TUI requirements. PRD-16 and the diagnostic ledger were
point-in-time audits; their unresolved work is now in the implementation
backlog.

## Critical path

```text
shared production runtime (implemented; run/TUI/chat/ACP/serve migrated)
        |
        +--> real tool/effect/policy-enforcement/approval execution
             (bounded local-process path implemented; crash/ops closure open)
        |
        +--> durable interaction/session/event service
             (prompt/transcript/cancel/replay and target/model config implemented)
                    |
                    +--> terminal chat + durable/actionable TUI
                         (responsive session/model/restart/cancel plus bounded
                          simultaneous activities and service-routed F6
                          approvals exist; explicit local authority works,
                          while shared/remote/API authority, handler-I/O
                          cancellation, and group execution remain open)
                    +--> complete rich ACP + Zed support
                         (durable stdio new/load/resume/cwd/tool slice and
                          bounded local-stdio native permissions exist;
                          list/import/rich plans/manual Zed remain)
                    +--> durable API control plane
                         (interaction/core HTTP slice exists)
                    +--> group/feed orchestration

parallel foundations: live-chain E2E, security custody/auth, PCA networking,
observability recovery
```

The dependency-safe agent queue and exact exit criteria are in
[`IMPLEMENTATION-BACKLOG.md`](IMPLEMENTATION-BACKLOG.md).

## Archived material

The 2026-08-05 archive contains:

- superseded master/status/diagnostic/TUI plans;
- the original owner baseline, research brief, and source-coverage inputs;
- current-day point-in-time status snapshots.

See [`archive/2026-08-05/README.md`](archive/2026-08-05/README.md). Nothing was
deleted; archived documents remain available for provenance.

The post-approval-wave archive audit on 2026-08-06 found no additional active
PRD eligible for archiving. Every active PRD retains at least one unmet
production-composition, user-path, failure/restart, security, observability, or
operator-documentation criterion recorded in `STATUS.md` and the canonical
backlog. The dated `prd/archive/` and local `tmp/archive/` remain provenance;
neither contains a current execution queue that should be promoted back into
active planning.

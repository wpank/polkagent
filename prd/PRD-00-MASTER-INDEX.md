# PRD-00 — Master index and roadmap authority

**Status:** active index
**Audited:** 2026-08-05
**Scope:** current `main` worktree after the 2026-08-05 implementation audit

## Executive state

Polkagent has a broad passing Rust check/test/rustdoc suite, and both the
mandatory workspace Clippy command and a stronger all-target/all-feature gate
are locally green. Bounded product slices now exist for the TUI, ACP, local
packages, PCA transport, and container lifecycle, but no PRD is complete
end-to-end under the repository's completion rule. The dominant gap is
composition: these slices do not converge on one shared durable
runtime/session/event contract, and the API/tool/effect/policy paths remain
incomplete.

The next milestone is therefore not “add more crates.” It is one durable
runtime and interaction contract, followed by real tool/effect execution and
actionable surfaces built on that contract.

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
| [PRD-03](PRD-03-EXECUTION-MODEL.md) | Runs, turns, effects, recovery | Active; real tool/effect/policy loop is a P0 gap. |
| [PRD-04](PRD-04-PROVIDERS-MODELS-TOOLS.md) | Providers, models, harnesses, tools, skills | Active; adapters exist but tools are not in the model execution loop. |
| [PRD-04a](PRD-04a-PROVIDER-HARNESS-EXPANSION.md) | Provider/harness expansion | Active component scope; prove each adapter through the shared runtime. |
| [PRD-05](PRD-05-POLKADOT-INTEGRATIONS.md) | Polkadot read/write integrations | Active; live finality, signing, dry-run/XCM, and action E2E remain. |
| [PRD-06](PRD-06-PCA-COMPATIBILITY.md) | PCA compatibility and transport | Active; durable encrypted TCP/control delivery is cross-process tested, but reference PCA framing, signed identity, attachments, and runtime composition remain. |
| [PRD-07](PRD-07-IDENTITY-SECURITY.md) | Identity, grants, policy, secrets, signers | Active; production auth, secret custody, policy wiring, and tenant enforcement remain. |
| [PRD-08](PRD-08-PAYMENTS-AUTONOMY.md) | Payments, budgets, autonomy | Active; domain code is not a value-moving composed product. |
| [PRD-09](PRD-09-MEMORY-GROUPS-EVALS.md) | Memory, groups, feeds, evals | Active; substantial libraries, little production orchestration wiring. |
| [PRD-10](PRD-10-DATA-OBSERVABILITY.md) | Events, artifacts, telemetry, recovery | Active; production injection and stream gap/replay behavior remain. |
| [PRD-11](PRD-11-DEPLOYMENT-CLOUD.md) | Deployment and cloud | Active; bounded single-instance container/config/shutdown/replacement persistence is proven, while durable API/run recovery and production operations remain. |
| [PRD-12](PRD-12-MARKETPLACE-EXTENSIONS.md) | Extensions and marketplace | Active; durable local package lifecycle and operator CLI work, while activation, sandbox execution, cryptographic trust, and registry paths remain. |
| [PRD-13](PRD-13-UX-SURFACES.md) | CLI, TUI, web/mobile surfaces | Active; the TUI has a bounded actionable Console, but durable chat/orchestration and studio/mobile surfaces are absent. |
| [PRD-14](PRD-14-APIs-SCHEMAS-CONFIG.md) | APIs, schemas, configuration | Active; API production composition and OpenAPI parity are P0/P1 gaps. |
| [PRD-15](PRD-15-TESTING-ROADMAP.md) | Testing and release gates | Active; broad component coverage, stable/Rust-1.89 checks, strict rustdoc, and mandatory/extended Clippy gates pass locally; production/live/client conformance remains incomplete. |

## Active delivery PRDs

| Document | Delivery outcome | Dependency |
|---|---|---|
| [PRD-17](PRD-17-LOCAL-TESTNET-E2E.md) | Real local Polkadot network, signed actions, finality, and reproducible CI evidence | Runtime/action wiring, real signer, corrected live-test workflow |
| [PRD-19](PRD-19-INTERACTIVE-CONSOLE-ACP.md) | Terminal chat, actionable TUI, shared commands, ACP server, Zed, and orchestration | Shared runtime and interaction contracts |

PRD-19 supersedes the implementation role of archived PRD-18 while preserving
its useful prompt/TUI requirements. PRD-16 and the diagnostic ledger were
point-in-time audits; their unresolved work is now in the implementation
backlog.

## Critical path

```text
shared production runtime
        |
        +--> real tool/effect/policy/approval execution
        |
        +--> durable interaction/session/event contract
                    |
                    +--> terminal chat + durable/actionable TUI
                         (bounded single-run TUI slice exists)
                    +--> complete ACP server + Zed support
                         (bounded stdio protocol slice exists)
                    +--> durable API control plane
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

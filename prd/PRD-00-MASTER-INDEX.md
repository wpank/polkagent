# PRD-00: Master Index, Implementation Plan and Verification Checklists

**Status:** definitive orchestration document for the Polkagent PRD suite

**Date:** 2026-07-30

**Purpose:** This document is the single entry point for the definitive Polkagent PRD
suite. It provides a complete index of all 15 PRDs, their dependency graph,
a structured six-phase implementation plan with per-phase acceptance criteria,
implementation checklists for agents, a cross-PRD requirement traceability matrix,
an ADR template, a workspace/crate layout plan, and a verification checklist
to ensure completeness across the entire suite.

## Table of contents

- [1. How to read this document](#1-how-to-read-this-document)
- [2. What is Polkagent](#2-what-is-polkagent)
  - [2.1 Competitive positioning](#21-competitive-positioning)
  - [2.2 Interoperability standards adopted](#22-interoperability-standards-adopted)
- [3. Complete PRD index](#3-complete-prd-index)
- [4. PRD dependency graph](#4-prd-dependency-graph)
- [5. Cross-PRD vocabulary alignment](#5-cross-prd-vocabulary-alignment)
- [6. Implementation roadmap](#6-implementation-roadmap)
  - [6.1 Dependency-ordered 12-step build sequence](#61-dependency-ordered-12-step-build-sequence)
  - [6.2 Staged recommendation timeline](#62-staged-recommendation-timeline)
- [7. Phase 1: Safety kernel](#7-phase-1-safety-kernel)
- [8. Phase 2: Build + Act + Reach proof](#8-phase-2-build--act--reach-proof)
- [9. Phase 3: Read-only value expansion](#9-phase-3-read-only-value-expansion)
- [10. Phase 4: Controlled write expansion](#10-phase-4-controlled-write-expansion)
- [11. Phase 5: Managed, public and value-moving](#11-phase-5-managed-public-and-value-moving)
- [12. Phase 6: Experimental frontier](#12-phase-6-experimental-frontier)
- [13. Cross-cutting architecture: risks, primitives, and confirmed stack](#13-cross-cutting-architecture-risks-primitives-and-confirmed-stack)
  - [13.1 Top 10 risks and mitigations](#131-top-10-risks-and-mitigations)
  - [13.2 Signature primitives](#132-signature-primitives)
  - [13.3 Confirmed technology stack](#133-confirmed-technology-stack)
  - [13.4 Regulatory flag](#134-regulatory-flag)
  - [13.5 OSS governance (Rust project)](#135-oss-governance-rust-project)
- [14. Rust workspace and crate layout](#14-rust-workspace-and-crate-layout)
- [15. Architecture decision record template](#15-architecture-decision-record-template)
- [16. Cross-PRD requirement traceability matrix](#16-cross-prd-requirement-traceability-matrix)
- [17. Implementation checklists for agents](#17-implementation-checklists-for-agents)
- [18. Per-PRD acceptance criteria summary](#18-per-prd-acceptance-criteria-summary)
- [19. Suite-wide verification checklist](#19-suite-wide-verification-checklist)
  - [19.7 Cross-cutting research (Domain 00) integration](#197-cross-cutting-research-domain-00-integration)

---

## 1. How to read this document

This is the orchestration layer. It does not introduce new requirements — it
indexes, sequences, and cross-references requirements defined in PRDs 01–15.

**For a first-time reader:** start with section 2 (what is Polkagent), then
section 3 (PRD index) to find the PRD relevant to your interest. Read that
PRD. Return here for sequencing and cross-cutting concerns.

**For an implementing agent:** start with section 6 (roadmap overview), find
your current phase, then use section 17 (implementation checklists) for the
step-by-step task list. Each checklist item references the normative PRD
section.

**For a reviewer:** use section 18 (per-PRD acceptance criteria) and
section 19 (suite-wide verification) to validate completeness.

### 1.1 Maturity labels

| Label | Meaning |
|---|---|
| **Established** | Owner-confirmed direction; PRDs must preserve |
| **Default** | Recommended out-of-box behavior; user may choose another supported mode |
| **Configurable** | Deliberate choice exposed through safe UX and versioned config |
| **Phased** | Committed long-term, delivered after dependencies and release gates |
| **Experimental** | Requires research spike and explicit maturity labeling |
| **Open** | Evidence or implementation decision still required |

### 1.2 Document conventions

- **MUST**, **MUST NOT**, **SHALL**, **SHOULD**, **MAY** follow RFC 2119
- Cross-references use the format `PRD-NN § section` (e.g., `PRD-03 § 5.2`)
- Requirement IDs use the format `RQ-PRNN-NNN` (e.g., `RQ-PR03-001`)
- Acceptance criteria use `AC-PRNN-NNN`

---

## 2. What is Polkagent

Polkagent is a proposed Rust-first, Polkadot-native product-engineering and
agent platform with three equal pillars:

1. **Build:** help people and teams create, test, deploy, and operate agents,
   applications, contracts/PVM programs, Polkadot SDK runtimes/chains, and
   emerging JAM services.
2. **Act and pay:** explain, prepare, simulate, approve, sign, submit, watch,
   and — when deliberately configured — fully automate Polkadot payments and
   on-chain actions.
3. **Reach users:** preserve and improve the mobile/private-chat, coding-agent,
   projects/files, framework bridge, and deployment experience of
   `polkadot-chat-agents` (PCA), with a cleaner, more modular Rust-first
   architecture.

**Current status:** design and planning. The repository contains research,
PRDs, architecture decisions, and delivery plans — not a production
implementation. Two existing codebases provide context:

- **PCA** (`polkadot-chat-agents`) — the Node.js reference product to preserve
  and improve
- **Roko** — a separate Rust agent-platform project providing useful patterns

See `PRD-01 § 2` for the complete vision narrative.

### 2.1 Competitive positioning

Polkagent competes against Web3 AI-agent platforms (EVM-centric "autonomous
wallet" patterns, x402-first designs) on safety, Rust, and local-first
architecture — not on feature count. The decisive differentiators are:

- **Chain-verified evidence (DryRunApi):** action cards are derived from
  calldata, not from model narrative. The model never serves as source of
  truth for what an extrinsic does.
- **Keys outside the model:** signing keys are never in the model's context
  window or reachable through tool calls. Key handles + `zeroize` enforce this
  at the type level.
- **Policy gate (Cedar) outside the agent surface:** grant resolution runs in
  formally verified policy code, not in model-generated text. Adversarial
  prompts cannot widen grants.

This positions Polkagent as safe-by-construction rather than safe-by-convention.

### 2.2 Interoperability standards adopted

Polkagent adopts existing protocols as adapters rather than inventing new ones:

| Standard | Role |
|---|---|
| MCP Streamable HTTP / stdio | Tool and resource exposure to external hosts |
| A2A v1.0.1 | Agent-to-agent delegation and task handoff |
| x402 (non-EVM path only, trigger-gated) | Micropayment signaling |
| AP2 | Mandate/authorization model for agent delegation |

These are adapters at the port layer, not core abstractions. The internal
effect pipeline, grant model, and outbox remain independent of all four.

---

## 3. Complete PRD index

| PRD | Title | Lines | Focus area | Key deliverables |
|---|---|---|---|---|
| **PRD-01** | Vision, Principles, Personas and Product Pillars | 2,030 | Product strategy | Vision, pillars, principles, personas, JTBDs, glossary |
| **PRD-02** | Vocabulary, Invariants and System Architecture | 2,151 | Architecture | Canonical vocabulary, system invariants, architecture layers, Rust workspace layout, trust boundaries |
| **PRD-03** | Agent/Run/Effect/Graph Execution Model | 2,764 | Core runtime | Agent/run/turn/step lifecycle, effect pipeline (Intent→Attempt→Outcome), durable outbox, crash recovery, graph composition |
| **PRD-04** | Providers, Models, Harnesses, Tools and Skills | 2,533 | AI integration | Provider/model/executor/harness/tool/skill taxonomy, capability negotiation, streaming, 3 end-to-end examples |
| **PRD-05** | Polkadot Chain, SDK, JAM/PVM and Product-Building Integrations | 1,591 | Ecosystem | Polkadot primer, integration matrix, Subxt/PAPI, builder workflows A1–A10, product-engineering workbench |
| **PRD-06** | PCA Compatibility, Chat/Mobile and Messaging | 2,754 | Migration | PCA feature audit, C0–C3 compatibility tiers, migration schemas, bridge compatibility, transport architecture |
| **PRD-07** | Identity, Accounts, Signers, Policy and Security | 1,939 | Security | Identity taxonomy, signer architecture (9 modes), policy/grant model, threat model, key isolation invariant |
| **PRD-08** | Payments, Autonomous Agents and Economic Controls | 2,721 | Payments | Payment domain model, Explain Before Sign, autonomy tiers 0–3, funded agent accounts, receipts, compliance |
| **PRD-09** | Memory, Knowledge, Learning, Multi-Agent Groups and Evals | 2,535 | Intelligence | Memory (episodic/semantic/procedural), groups with grant intersection, feeds/triggers, evals, experimental modules |
| **PRD-10** | Data, Artifacts, Events, Observability and Recovery | 2,462 | Data | Artifact taxonomy/lifecycle, event system, observability stack, evidence-bearing effects, crash recovery, database schemas |
| **PRD-11** | Self-Hosting, Managed Cloud and Multi-Tenancy | 2,748 | Deployment | Three-plane architecture, self-hosted and managed cloud, workers, offline-safe degradation, portability, billing/SLOs |
| **PRD-12** | Marketplace, Registry, Extension SDK and Product Kits | 2,284 | Ecosystem | Package taxonomy, manifest spec, registry federation, trust model, sandboxing, product kits, fees |
| **PRD-13** | UX, CLI, Studio, Inbox, Mobile and Operator Surfaces | 2,093 | UX | Persona journeys, CLI design, Agent Studio, Inbox, action cards, mobile/chat, onboarding, accessibility |
| **PRD-14** | APIs, Schemas, Configuration and Migration | 2,905 | APIs | REST/WebSocket/gRPC APIs, database schemas, configuration hierarchy, DTOs, SDKs, PCA migration, feature flags |
| **PRD-15** | Testing, Security Assurance, Roadmap and Acceptance | 1,385 | Assurance | Threat model, testing pyramid, security testing, CI/CD gates, 6-phase roadmap, cross-PRD acceptance |
| **Total** | | **34,895** | | |

### 3.1 Reading order by role

| Role | Recommended reading order |
|---|---|
| Product manager | PRD-01 → PRD-13 → PRD-08 → PRD-15 § roadmap |
| Rust engineer (core) | PRD-02 → PRD-03 → PRD-10 → PRD-14 |
| Rust engineer (AI/providers) | PRD-04 → PRD-03 → PRD-09 |
| Rust engineer (Polkadot) | PRD-05 → PRD-08 → PRD-07 |
| Rust engineer (PCA migration) | PRD-06 → PRD-04 → PRD-14 |
| DevOps/infra engineer | PRD-11 → PRD-14 → PRD-15 |
| Security engineer | PRD-07 → PRD-08 § autonomy → PRD-15 → PRD-03 § 5 |
| UX designer | PRD-13 → PRD-01 § personas → PRD-08 § action cards |
| Marketplace developer | PRD-12 → PRD-04 § skills → PRD-07 § policy |

---

## 4. PRD dependency graph

```text
PRD-01 (Vision)
  │
  ├──> PRD-02 (Architecture)
  │      │
  │      ├──> PRD-03 (Execution Model)  ◄── core runtime
  │      │      │
  │      │      ├──> PRD-04 (Providers/Tools)
  │      │      ├──> PRD-10 (Data/Observability)
  │      │      └──> PRD-09 (Memory/Groups)
  │      │
  │      ├──> PRD-07 (Identity/Security)
  │      │      │
  │      │      └──> PRD-08 (Payments/Autonomy)
  │      │
  │      ├──> PRD-14 (APIs/Schemas)
  │      │
  │      └──> PRD-11 (Cloud/Deployment)
  │
  ├──> PRD-05 (Polkadot Integrations)
  │      └──> PRD-08 (Payments) [chain-specific operations]
  │
  ├──> PRD-06 (PCA Compatibility)
  │      └──> PRD-04 (Providers) [brain/harness mapping]
  │
  ├──> PRD-12 (Marketplace)
  │      └──> PRD-04 (Providers) [skill/tool packages]
  │      └──> PRD-07 (Identity) [publisher identity]
  │
  ├──> PRD-13 (UX/Surfaces)
  │      └──> PRD-08 (Payments) [action cards]
  │      └──> PRD-06 (PCA) [chat surface]
  │
  └──> PRD-15 (Testing/Roadmap)  ◄── cross-cutting
         └──> ALL other PRDs [acceptance criteria]
```

### 4.1 Dependency rules

1. All PRDs depend on PRD-01 (vision and vocabulary foundations)
2. All technical PRDs depend on PRD-02 (architecture and invariants)
3. PRD-03 (execution model) is the foundational runtime — PRDs 04, 09, 10 build on it
4. PRD-07 (identity/security) is required before PRD-08 (payments/autonomy)
5. PRD-15 (testing/roadmap) references acceptance criteria from all other PRDs
6. No circular dependencies exist between PRDs

---

## 5. Cross-PRD vocabulary alignment

These canonical terms are defined in PRD-02 § 2 and used consistently across all PRDs:

| Term | Canonical definition | Primary PRD | Also used in |
|---|---|---|---|
| Agent | Configured, named entity that can perform runs | PRD-02 § 2.1 | All |
| Run | One bounded execution of an agent against a request | PRD-03 § 3 | 10, 11, 14 |
| Turn | One model inference round-trip within a run | PRD-03 § 4 | 04, 14 |
| Step | One atomic operation within a turn | PRD-03 § 4 | 04 |
| EffectIntent | Durable command recorded BEFORE I/O | PRD-03 § 5.2 | 08, 10 |
| EffectAttempt | One claim/lease/retry lifecycle | PRD-03 § 5.3 | 08, 10 |
| EffectOutcome | Immutable result (Success/Failure/Timeout/Cancelled/Unknown) | PRD-03 § 5.4 | 08, 10, 13 |
| Grant | Resolved set of permissions for a run or effect | PRD-07 § 8 | 03, 08, 09, 12 |
| Capability | Named permission type that can be granted | PRD-07 § 8 | 04, 12 |
| Policy | Rules that determine grant resolution | PRD-07 § 8 | 08, 11 |
| Provider | Connection/service exposing one or more models | PRD-04 § 2 | 11, 12 |
| Model | Particular model identity and capability set | PRD-04 § 3 | 09, 12 |
| Executor | Component performing one run/turn | PRD-04 § 4 | 03, 11 |
| Harness | Long-lived coding-agent process/service | PRD-04 § 5 | 06, 11 |
| Tool | Typed operation under capability grant | PRD-04 § 6 | 05, 12 |
| Skill | Versioned package of instructions/schemas/tests | PRD-04 § 7 | 12, 13 |
| Artifact | Durable, attributable content or evidence | PRD-10 § 5 | 03, 05, 14 |
| Event | Ordered observation of lifecycle/streaming activity | PRD-10 § 8 | 03, 14 |
| Signer | Component that holds or delegates signing authority | PRD-07 § 7 | 08, 05 |
| Transport | Channel for message delivery (encrypted chat, HTTP, etc.) | PRD-06 § 3.4 | 02, 11 |
| Surface | User-facing presentation (CLI, web, mobile, chat) | PRD-13 § 1 | 02, 06 |
| Profile | Chain/network binding with metadata | PRD-05 § 2 | 08, 14 |
| Tenant | Isolation boundary in managed deployment | PRD-11 § 2 | 07, 14 |
| Product Kit | Bundled package for one job | PRD-12 § 3 | 05, 13 |

---

## 6. Implementation roadmap

The roadmap follows dependency order, not calendar commitment. Each phase has
explicit gates that MUST pass before the next phase begins. Phases may overlap
where dependencies permit.

```text
Phase 1: Safety Kernel
  │ Gate: fault injection shows no duplicate effect
  v
Phase 2: Build + Act + Reach Proof
  │ Gate: canonical card, external signer, comprehension tests
  v
Phase 3: Read-Only Value Expansion
  │ Gate: real bounded jobs with repeat use
  v
Phase 4: Controlled Write Expansion
  │ Gate: family-specific schema/simulation/failure/approval evidence
  v
Phase 5: Managed, Public, Value-Moving
  │ Gate: security review, legal review, tenant isolation
  v
Phase 6: Experimental Frontier
  │ Gate: primary implementation evidence per experiment
  v
  [Ongoing iteration]
```

### 6.1 Dependency-ordered 12-step build sequence

This sequence refines the phase model into an ordered series of build steps
where each step's output is a hard prerequisite for the next. Steps within
a phase may parallelize; cross-step dependencies MUST be respected.

| Step | Deliverable | Phase |
|---|---|---|
| 1 | Kernel + types + ports skeleton (hexagonal, no-cross-talk fitness function enforced by CI) | 1 |
| 2 | SQLite WAL authority store (single-writer task, IMMEDIATE transactions, busy_timeout=5000ms) + event sourcing + transactional outbox + pure reducer | 1 |
| 3 | Cedar policy gate + capability grants OUTSIDE agent surface + keys-outside-model (handles + `zeroize`) | 1 |
| 4 | subxt static+dynamic decode + CheckMetadataHash + Vault/Ledger signer adapters | 2 |
| 5 | DryRunApi + XcmPaymentApi evidence engine → calldata-derived action cards → human-approval UX | 2 |
| 6 | Provider adapters (internal schema) + MCP Streamable HTTP/stdio tool port | 2–3 |
| 7 | Payments: DOT/stablecoin transfers on Asset Hub via pure-proxy + budgets, DryRunApi pre-flight | 4–5 |
| 8 | Memory (sqlite-vec+FTS5+RRF) + observability (tracing+OTel+redaction) | 4 |
| 9 | REACH: JS codec bridge + Statement Store live signal + Bulletin CID anchoring + app-layer reliability | 4 |
| 10 | Marketplace: Wasmtime WIT sandbox + cosign/SLSA Build L2 supply chain | 5 |
| 11 | Managed cloud (data/control-plane split), multi-tenant, billing | 5 |
| 12 | Trigger-gated: JAM/CorePlay; x402 non-EVM; MPC signing | 6 |

### 6.2 Staged recommendation timeline

| Window | Scope |
|---|---|
| **Now (0–3 mo)** | Ship treasury-operator ACT workflow read-only + simulate (DryRunApi) with kernel / outbox / Cedar / keys-outside-model / subxt+CheckMetadataHash / Vault-Ledger stack |
| **Next (3–6 mo)** | Enable bounded autonomy (pure-proxy + budgets + time-delay); payments with DryRunApi pre-flight; memory (sqlite-vec+FTS5+RRF); observability; OWASP injection eval gate |
| **Later (6–12 mo)** | REACH; marketplace (Wasmtime WIT + cosign/SLSA L2); managed cloud |
| **Trigger-gated** | JAM/CorePlay; x402 non-EVM; USDC fee-sufficiency; MPC signing |

---

## 7. Phase 1: Safety kernel

**Goal:** Prove that the core execution model is correct, crash-safe, and
never duplicates an irreversible effect.

### 7.1 Deliverables

| # | Deliverable | Primary PRD | Crates |
|---|---|---|---|
| 1.1 | Durable run lifecycle (state machine) | PRD-03 § 3 | `polkagent-core`, `polkagent-run` |
| 1.2 | Effect pipeline (Intent → Attempt → Outcome) | PRD-03 § 5 | `polkagent-effect` |
| 1.3 | Durable outbox with ordered delivery | PRD-03 § 6 | `polkagent-outbox` |
| 1.4 | Grant resolution engine | PRD-07 § 8 | `polkagent-grant` |
| 1.5 | Artifact lineage tracking | PRD-10 § 6 | `polkagent-artifact` |
| 1.6 | Event stream (in-process) | PRD-10 § 8 | `polkagent-event` |
| 1.7 | Fake transport adapter | PRD-06 § 7 | `polkagent-transport-fake` |
| 1.8 | Fake executor adapter | PRD-04 § 4 | `polkagent-executor-fake` |
| 1.9 | Fake signer adapter | PRD-07 § 7 | `polkagent-signer-fake` |
| 1.10 | Local SQLite store | PRD-10 § 7.2 | `polkagent-store-sqlite` |
| 1.11 | Configuration loading and validation | PRD-14 § 6 | `polkagent-config` |

### 7.2 Acceptance criteria

| ID | Criterion | Verification method |
|---|---|---|
| AC-P1-001 | Run state machine transitions match PRD-03 § 3.2 exactly | State machine model test with all transitions |
| AC-P1-002 | EffectIntent is persisted BEFORE any I/O | Crash-injection test: kill during I/O, verify intent exists on restart |
| AC-P1-003 | Restart after crash never duplicates a completed effect | Fault injection at each effect stage, verify exactly-once |
| AC-P1-004 | Unknown outcome is never collapsed to success or failure | Property test: unknown remains unknown through all projections |
| AC-P1-005 | Outbox delivers in order, handles duplicates | Inject duplicate messages, verify consumer idempotency |
| AC-P1-006 | Grant resolution rejects undeclared capabilities | Capability test suite per PRD-07 § 8 |
| AC-P1-007 | Artifact lineage tracks parent/child correctly | Create chain of artifacts, verify lineage graph |
| AC-P1-008 | Fake adapters pass port contract tests | Contract test suite for transport, executor, signer |
| AC-P1-009 | Configuration rejects invalid schemas | Fuzz test with malformed config files |

### 7.3 Implementation checklist for agents

```
Phase 1 Implementation Checklist
=================================

[ ] 1. Set up Rust workspace structure (see § 14)
    - [ ] Create workspace Cargo.toml
    - [ ] Create crate directories per § 14
    - [ ] Set up shared CI (clippy, fmt, test)
    Reference: PRD-02 § 6, PRD-15 § 7

[ ] 2. Implement polkagent-core types
    - [ ] RunId, AgentId, TurnId, StepId types
    - [ ] RunState enum with all states from PRD-03 § 3.2
    - [ ] EffectIntent, EffectAttempt, EffectOutcome types from PRD-03 § 5
    - [ ] Grant, Capability, Policy types from PRD-07 § 8
    - [ ] Artifact, ArtifactId, ArtifactType from PRD-10 § 5
    - [ ] Event, EventId types from PRD-10 § 8
    Reference: PRD-02 § 2, PRD-03 § all Rust sketches

[ ] 3. Implement polkagent-run
    - [ ] Run state machine with guard functions
    - [ ] Turn lifecycle within a run
    - [ ] Step recording
    - [ ] Run persistence to store trait
    Reference: PRD-03 § 3, 4

[ ] 4. Implement polkagent-effect
    - [ ] EffectIntent creation and persistence
    - [ ] EffectAttempt lifecycle (claim, lease, retry)
    - [ ] EffectOutcome recording (immutable)
    - [ ] Idempotency key generation and checking
    - [ ] Crash recovery: detect incomplete attempts on startup
    Reference: PRD-03 § 5, 6, 7, 8

[ ] 5. Implement polkagent-outbox
    - [ ] Ordered message queue
    - [ ] At-least-once delivery with consumer dedup
    - [ ] Delivery acknowledgement
    Reference: PRD-03 § 6

[ ] 6. Implement polkagent-grant (Cedar policy engine)
    - [ ] Cedar policy evaluation (formally verified; replaces ad-hoc rule engines)
    - [ ] Grant resolution from Cedar policies; grants held OUTSIDE agent-editable surface
    - [ ] Capability matching
    - [ ] Time-bounded grants
    - [ ] Least-privilege default (no grants = no effects)
    - [ ] keys-outside-model: signing-key handles only; zeroize on drop
    Reference: PRD-07 § 8; § 13.2 (signature primitives); § 13.3 (Cedar entry)

[ ] 7. Implement polkagent-artifact
    - [ ] Artifact creation with content hash
    - [ ] Lineage tracking (parent/child)
    - [ ] Lifecycle state transitions
    Reference: PRD-10 § 5, 6

[ ] 8. Implement polkagent-event
    - [ ] Event creation and ordering
    - [ ] In-process broadcast channel
    - [ ] Event persistence to store
    Reference: PRD-10 § 8

[ ] 9. Implement polkagent-store-sqlite
    - [ ] Schema creation (runs, effects, artifacts, events, grants)
    - [ ] CRUD operations for each entity
    - [ ] WAL mode enabled; single-writer enforced (one task owns the write connection)
    - [ ] BEGIN IMMEDIATE for all write transactions; busy_timeout=5000ms
    - [ ] Transactional outbox: effect intent written in same transaction as state update
    - [ ] WAL checkpoint tuning to avoid reader starvation
    Reference: PRD-10 § 7.2, PRD-14 § 7; § 13.1 risk 4 (write contention); § 13.3 (SQLite entry)

[ ] 10. Implement fake adapters
    - [ ] Fake transport: in-memory message send/receive
    - [ ] Fake executor: deterministic model responses
    - [ ] Fake signer: accepts all, returns deterministic signature
    Reference: PRD-04 § 4, PRD-06 § 7, PRD-07 § 7

[ ] 11. Implement polkagent-config
    - [ ] TOML config file parsing
    - [ ] Schema validation
    - [ ] Default values
    - [ ] Environment variable overrides
    Reference: PRD-14 § 6

[ ] 12. Write Phase 1 tests
    - [ ] State machine property tests
    - [ ] Crash recovery fault injection tests
    - [ ] Idempotency property tests
    - [ ] Grant denial tests
    - [ ] Port contract test suites
    Reference: PRD-15 § 3, 4
```

---

## 8. Phase 2: Build + Act + Reach proof

**Goal:** Prove the architecture with one useful action from each pillar,
projected through at least one surface.

### 8.1 Deliverables

| # | Deliverable | Primary PRD | Crates |
|---|---|---|---|
| 2.1 | Explain an extrinsic (A6) | PRD-05 § 8 | `polkagent-chain-adapter` |
| 2.2 | Explain Before Sign action (B1) | PRD-08 § 4 | `polkagent-action-sign` |
| 2.3 | Canonical action card renderer | PRD-13 § 7 | `polkagent-card` |
| 2.4 | One real model executor (Anthropic) | PRD-04 § 2.1.1 | `polkagent-executor-anthropic` |
| 2.5 | External signer adapter (browser extension) | PRD-07 § 7 | `polkagent-signer-external` |
| 2.6 | CLI surface (basic) | PRD-13 § 4 | `polkagent-cli` |
| 2.7 | Web surface (basic Agent Studio) | PRD-13 § 5 | `polkagent-web` |
| 2.8 | Subxt chain adapter | PRD-05 § 2.11 | `polkagent-chain-subxt` |
| 2.9 | Metadata pinning and decode | PRD-05 § 3 | `polkagent-metadata` |
| 2.10 | REST API (core endpoints) | PRD-14 § 4 | `polkagent-api` |

### 8.2 Acceptance criteria

| ID | Criterion | Verification method |
|---|---|---|
| AC-P2-001 | Extrinsic decoded correctly against pinned metadata | Test with known extrinsics on Polkadot, Asset Hub, Westend |
| AC-P2-002 | Action card displays canonical fields, model text is visually separate | UX review against PRD-13 § 7 wireframes |
| AC-P2-003 | Signer receives exact bytes, never sees model-modified data | Integration test: card → signer → verify byte equality |
| AC-P2-004 | Stale metadata produces explicit error, not wrong decode | Test with outdated metadata against current chain |
| AC-P2-005 | Wrong-network extrinsic is rejected before signing | Cross-network test corpus |
| AC-P2-006 | CLI and web show identical run state | Run same action through both, compare state projections |
| AC-P2-007 | Model executor streams tokens and emits correct events | Streaming test with event capture |
| AC-P2-008 | REST API returns correct run/effect/artifact data | API contract test suite |

### 8.3 Implementation checklist for agents

```
Phase 2 Implementation Checklist
=================================

[ ] 1. Implement Subxt chain adapter (static + dynamic)
    - [ ] Connect to Polkadot/Westend/Asset Hub
    - [ ] Static decode path (generated types) for known pallets
    - [ ] Dynamic decode path (runtime metadata) for unknown/upgraded pallets
    - [ ] CheckMetadataHash signed extension: verify metadata hash matches chain; refuse on mismatch
    - [ ] Submit signed extrinsics
    - [ ] Subscribe to finality
    - [ ] Vault and Ledger signer adapters (hardware signing; keys never leave device)
    Reference: PRD-05 § 2.11; § 13.1 risk 2 (metadata drift); § 13.3 (subxt entry)

[ ] 2. Implement metadata service
    - [ ] Metadata fetching and caching
    - [ ] Metadata pinning (hash → metadata snapshot)
    - [ ] Drift detection (compare cached vs live)
    - [ ] SCALE decode using pinned metadata
    - [ ] DryRunApi integration: simulate extrinsic and derive calldata-only action card
    - [ ] XcmPaymentApi integration: fee estimation before XCM dispatch
    Reference: PRD-05 § 3; § 13.2 (calldata-derived action cards primitive)

[ ] 3. Implement Anthropic executor
    - [ ] API client with streaming
    - [ ] Tool call support
    - [ ] Token usage tracking
    - [ ] Error handling and retries
    Reference: PRD-04 § 2.1.1

[ ] 4. Implement Explain Before Sign flow
    - [ ] Intent construction from user request
    - [ ] Metadata-pinned decode
    - [ ] Pre-flight checks (balance, fee, ED, nonce)
    - [ ] Canonical action card generation
    - [ ] Signer handoff (external wallet)
    - [ ] Finality observation
    Reference: PRD-08 § 4, PRD-05 § 8

[ ] 5. Implement action card renderer
    - [ ] Card data model (canonical fields)
    - [ ] Separation of model text vs canonical data
    - [ ] Risk indicators
    - [ ] Approval/deny controls
    Reference: PRD-13 § 7

[ ] 6. Implement external signer adapter
    - [ ] Browser extension signing protocol
    - [ ] Payload verification (bytes match card)
    - [ ] Signature return handling
    Reference: PRD-07 § 7

[ ] 7. Build CLI surface
    - [ ] Command structure: polkagent [agent|run|explain|sign|config]
    - [ ] Interactive approval flow
    - [ ] JSON and human output modes
    Reference: PRD-13 § 4

[ ] 8. Build basic web surface
    - [ ] Run timeline view
    - [ ] Action card display
    - [ ] Approval interface
    Reference: PRD-13 § 5

[ ] 9. Implement core REST API
    - [ ] Agent CRUD endpoints
    - [ ] Run create/list/get/cancel
    - [ ] Effect list/approve/deny
    - [ ] WebSocket event streaming
    Reference: PRD-14 § 4

[ ] 10. Write Phase 2 tests
    - [ ] Extrinsic decode test corpus (valid, stale, wrong-network, malformed)
    - [ ] Action card fidelity tests
    - [ ] Signer byte-equality tests
    - [ ] API contract tests
    - [ ] Comprehension study setup (predefined critical-error threshold)
    Reference: PRD-15 § 3, 4, 9
```

---

## 9. Phase 3: Read-only value expansion

**Goal:** Expand to useful read-only workflows across all three pillars
without introducing write authority.

### 9.1 Deliverables

| # | Deliverable | Candidates | Primary PRD |
|---|---|---|---|
| 3.1 | Storage-migration rehearsal | A1 | PRD-05 § 8 |
| 3.2 | Upgrade impact brief | A2 | PRD-05 § 8 |
| 3.3 | Metadata-grounded RAG | A9 | PRD-05 § 8 |
| 3.4 | OpenGov research copilot | B2 | PRD-08 § 5 |
| 3.5 | Treasury/portfolio research | B9 | PRD-08 § 5 |
| 3.6 | Live run timeline | F3 | PRD-13 § 5 |
| 3.7 | Error/recovery explainer | F7 | PRD-13 § 12 |
| 3.8 | Identity signal display | I2 | PRD-07 § 4 |
| 3.9 | PCA C0 compatibility work | C0 | PRD-06 § 3 |
| 3.10 | Additional model executors | — | PRD-04 § 2 |

### 9.2 Acceptance criteria

| ID | Criterion | Verification method |
|---|---|---|
| AC-P3-001 | Read-only workflows produce zero write effects | Effect audit: no write-type effects in any run |
| AC-P3-002 | RAG answers cite metadata hash, block, and source | Citation coverage evaluation corpus |
| AC-P3-003 | PCA C0 bot can connect and exchange messages | Integration test with PCA transport |
| AC-P3-004 | Real bounded jobs show repeat use | Usage metrics over test period |
| AC-P3-005 | No source or privacy regressions | Security regression test suite |

### 9.3 Implementation checklist for agents

```
Phase 3 Implementation Checklist
=================================

[ ] 1. Builder tools (read-only)
    - [ ] Storage migration rehearsal (fork + check + diff)
    - [ ] Upgrade impact brief (old/new metadata comparison)
    - [ ] Metadata-grounded RAG with citation
    Reference: PRD-05 § 8 (A1, A2, A9)

[ ] 2. Governance tools (read-only)
    - [ ] OpenGov referendum brief reader
    - [ ] Treasury/portfolio summary reader
    Reference: PRD-08 § 5 (B2, B9)

[ ] 3. UX improvements
    - [ ] Live run timeline with reconnect recovery
    - [ ] Error/recovery explainer with next-step proposals
    - [ ] Identity signal display (People Chain lookup)
    Reference: PRD-13 § 5, 12; PRD-07 § 4

[ ] 4. PCA C0 compatibility
    - [ ] Transport adapter for PCA encrypted chat
    - [ ] Bot registration flow
    - [ ] Message send/receive with ACK
    Reference: PRD-06 § 3, 6

[ ] 5. Additional model executors
    - [ ] OpenAI-compatible executor
    - [ ] Local model executor (ollama)
    - [ ] Model fallback chain
    Reference: PRD-04 § 2

[ ] 6. Write Phase 3 tests
    - [ ] Zero-write-effect audit for all read-only workflows
    - [ ] RAG citation evaluation corpus
    - [ ] PCA transport integration tests
    Reference: PRD-15 § 3
```

---

## 10. Phase 4: Controlled write expansion

**Goal:** Enable carefully gated write operations with family-specific
evidence.

### 10.1 Deliverables

| # | Deliverable | Candidates | Primary PRD |
|---|---|---|---|
| 4.1 | Pre-sign risk gates | B4 | PRD-08 § 6 |
| 4.2 | Multisig/proxy coordinator | B6 | PRD-08 § 6 |
| 4.3 | XCM planning (prototype) | A3/B5 | PRD-05 § 8 |
| 4.4 | Metadata-drift watcher | A7 | PRD-05 § 8 |
| 4.5 | Product kits v1 | H1 | PRD-12 § 10 |
| 4.6 | PCA C1 compatibility | C1 | PRD-06 § 3 |
| 4.7 | Harness lifecycle (Claude, Codex) | — | PRD-04 § 5 |
| 4.8 | Provenanced memory v1 | G1 | PRD-09 § 2 |

### 10.2 Acceptance criteria

| ID | Criterion | Verification method |
|---|---|---|
| AC-P4-001 | Risk gates flag batch/proxy/approval patterns | Seeded dangerous-call fixture corpus |
| AC-P4-002 | Multisig coordinator tracks approvals without holding keys | Multi-party test with pure-proxy edge cases |
| AC-P4-003 | XCM planner refuses unsupported routes | Route test corpus with failure cases |
| AC-P4-004 | Product kit install/uninstall/conformance works | Kit lifecycle test suite |
| AC-P4-005 | PCA C1 bridges function | Bridge integration test corpus |
| AC-P4-006 | Harness sessions resume after restart | Session persistence and resume tests |
| AC-P4-007 | Memory respects tenant/conversation boundaries | Cross-tenant access denial tests |

### 10.3 Implementation checklist for agents

```
Phase 4 Implementation Checklist
=================================

[ ] 1. Write-operation safety
    - [ ] Pre-sign risk gate engine (batch, proxy, approval detection)
    - [ ] Multisig/proxy state reader and coordinator
    - [ ] XCM route planner (prototype, testnet only)
    - [ ] Metadata-drift watcher (detect + propose, never merge)
    Reference: PRD-08 § 6, PRD-05 § 8

[ ] 2. Product kits
    - [ ] Kit manifest format and validation
    - [ ] Kit installation workflow
    - [ ] Kit conformance test framework
    - [ ] Kit uninstall and rollback
    Reference: PRD-12 § 10

[ ] 3. PCA C1 compatibility
    - [ ] Bridge protocol implementation
    - [ ] OpenAPI compatibility layer
    Reference: PRD-06 § 3

[ ] 4. Harness lifecycle
    - [ ] Claude CLI harness adapter
    - [ ] Codex CLI harness adapter
    - [ ] Session create/resume/health/cancel
    - [ ] Workspace/worktree binding
    Reference: PRD-04 § 5

[ ] 5. Memory v1
    - [ ] Episodic memory storage
    - [ ] Semantic memory with provenance
    - [ ] User-controlled forget/export
    - [ ] Tenant boundary enforcement
    Reference: PRD-09 § 2, 3

[ ] 6. Write Phase 4 tests
    - [ ] Risk gate fixture corpus
    - [ ] Multisig edge case tests
    - [ ] Kit lifecycle tests
    - [ ] Memory isolation tests
    Reference: PRD-15 § 3, 4
```

---

## 11. Phase 5: Managed, public and value-moving

**Goal:** Enable managed cloud deployment, real-value operations, and
marketplace publication.

### 11.1 Deliverables

| # | Deliverable | Candidates | Primary PRD |
|---|---|---|---|
| 5.1 | Funded policy-bounded accounts | C1 | PRD-08 § 7 |
| 5.2 | Agent earns/spends (prototype) | C2 | PRD-08 § 9 |
| 5.3 | Agent-to-agent escrow (research) | C3 | PRD-08 § 9 |
| 5.4 | Capability-disclosed packages | H2 | PRD-12 § 8 |
| 5.5 | Public agent-service listings | H4 | PRD-12 § 13 |
| 5.6 | Fleet workers | J2 | PRD-11 § 6 |
| 5.7 | Regional isolation | J4 | PRD-11 § 12 |
| 5.8 | Multi-tenant managed platform | — | PRD-11 § 5 |
| 5.9 | Billing and metering | — | PRD-11 § 10 |
| 5.10 | PCA C2/C3 compatibility | C2/C3 | PRD-06 § 3 |

### 11.2 Acceptance criteria

| ID | Criterion | Verification method |
|---|---|---|
| AC-P5-001 | Funded agent cannot exceed configured budget | Adversarial budget-exhaustion tests |
| AC-P5-002 | Adversarial prompts/tools cannot widen authority | Red-team test suite (PRD-15 § 5.1) |
| AC-P5-003 | Revocation and time-limit drills succeed | Timed revocation tests |
| AC-P5-004 | Tenant isolation prevents cross-tenant access | Tenant isolation test suite |
| AC-P5-005 | Export/import preserves all state | Portability drill: export → new deployment → verify |
| AC-P5-006 | Independent security review completed | External review report |
| AC-P5-007 | Legal review completed for value-moving operations | Legal sign-off document |

### 11.3 Gate: this phase requires independent security and legal review

No real-value operations may go live without:
- External penetration test report
- Signer/custody security review
- Legal/compliance review for the jurisdiction, covering:
  - Money-transmission licensing exposure
  - Sanctions screening (OFAC and equivalents) implementation or attestation
  - Tax-reporting obligations and audit-export capability
- Reconciliation, overdraft, pause, and unknown-finality drills
- Tenant isolation verification

See § 13.4 (regulatory flag) for the full counsel-referral requirement.

---

## 12. Phase 6: Experimental frontier

**Goal:** Explore long-term opportunities that do not affect core correctness.

### 12.1 Deliverables

| # | Deliverable | Candidates | Primary PRD |
|---|---|---|---|
| 6.1 | JAM service prototypes | D1–D4 | PRD-05 § 12 |
| 6.2 | Personhood gating (experimental) | I3 | PRD-07 § 4.4 |
| 6.3 | Visionary bets | K1–K6 | PRD-05 § 12 |
| 6.4 | Affect/vitality modules | — | PRD-09 § 10 |
| 6.5 | Evolutionary skill selection | G5 | PRD-09 § 8 |
| 6.6 | Always-on watchers (autonomous) | C4 | PRD-08 § 7 |

### 12.2 Gate criteria

- Primary implementation evidence exists for each experiment
- Core correctness and ordinary product flows do not depend on it
- Explicit maturity label in UI
- Feature flag controls activation
- No production write authority without separate gate

---

## 13. Cross-cutting architecture: risks, primitives, and confirmed stack

### 13.1 Top 10 risks and mitigations

| # | Risk | Mitigation |
|---|---|---|
| 1 | **Indirect prompt injection moves value** | Keys-outside-model + Cedar gate + calldata-derived action cards + human approval + hard budgets |
| 2 | **Metadata drift / runtime upgrade** | Dynamic decode + CheckMetadataHash; refuse-on-mismatch |
| 3 | **Look-alike asset fraud on Asset Hub** | Verify asset IDs (1337/1984) on-chain; allowlist; DryRunApi pre-flight |
| 4 | **SQLite write contention** | Single-writer task, BEGIN IMMEDIATE, busy_timeout=5000ms, WAL checkpoint tuning |
| 5 | **Double-submit on retry** | Idempotency keys + outbox dedupe + DryRunApi pre-flight + nonce discipline |
| 6 | **Regulatory exposure** | Counsel review required (see § 13.4); screening + audit-export hooks; jurisdictional gates |
| 7 | **Malicious marketplace extension** | Wasmtime capability sandbox + cosign/SLSA + curated review + RustSec advisory checks |
| 8 | **Over-reliance on immature primitives** | Trigger-gate JAM, Bulletin durability, USDC fee-sufficiency — each requires explicit graduation evidence |
| 9 | **Provider outage / cost blowout** | Circuit-breaker + failover routing + token/cost budgets + local-model fallback |
| 10 | **Statement Store treated as reliable queue** | App-layer ACK + sequence numbers + idempotent processing + Bulletin CID anchoring |

### 13.2 Signature primitives

These primitives MUST recur consistently across every PRD, phase, crate, and
action type. They are not optional patterns — they are the foundational
invariants of the system.

- **Calldata-derived, chain-verified action cards (DryRunApi):** the model narrative
  is never the source of truth for what an extrinsic does.
- **Keys outside the model; capability grants + policy gate (Cedar) outside the
  agent-editable surface:** no path exists from model output to key material or
  grant widening.
- **Event-sourcing + transactional outbox + pure reducer with effect-intents on
  single-writer SQLite WAL:** durable, replayable, crash-safe by construction.
- **Idempotency keys on every effect; at-least-once delivery + at-most-once
  external mutation:** retry-safe without double-spend.
- **Durable content-addressed artifacts (BLAKE3) + ephemeral event streams with
  explicit graduation boundary:** artifacts are facts; events are signals.
- **Narrow ports/adapters, no cross-talk, enforced by CI fitness functions:**
  structural architecture rule, not a convention.
- **Grant intersection + hard budgets for multi-agent groups:** the weakest grant
  in a group wins; no agent can elevate group authority.
- **Provenance + classification tags on all memory and evidence:** every artifact
  and memory item carries its origin chain.

### 13.3 Confirmed technology stack

The following technology choices are confirmed by research. Each is a named
decision (not a placeholder) and should be recorded in the project ADR log.

| Component | Technology | Notes |
|---|---|---|
| Policy engine | Cedar (formally verified in Lean) | Replaces ad-hoc rule engines |
| Authority store | SQLite WAL, BEGIN IMMEDIATE, busy_timeout=5000ms | Single-writer; no Postgres required for Phase 1 |
| Plugin sandbox | Wasmtime Component Model / WIT | Not Extism; chosen for capability safety |
| Supply chain | Sigstore cosign v3 + SLSA Build L2 | Marketplace packages only |
| Local RAG | sqlite-vec + FTS5 + RRF | Local-first; no vector DB dependency |
| Content addressing | BLAKE3 internal / SHA-256 interop | |
| MCP transport | Streamable HTTP (SSE deprecated) | stdio for local harness |
| Agent interop | A2A v1.0.1 + x402 + AP2 | Adapters only; not core abstractions |
| Chain decode | subxt static+dynamic + CheckMetadataHash | Refuse on hash mismatch |
| Evidence engine | DryRunApi + XcmPaymentApi | Never model narrative |
| Observability | OpenTelemetry + tracing crate | Redaction required on sensitive fields |
| Config format | TOML (not YAML) | |
| Testing | proptest + cargo-fuzz | Property tests and fuzz targets |
| Durable execution | DBOS pattern on SQLite | Temporal is rejected |

### 13.4 Regulatory flag

**FLAG FOR COUNSEL:** Agents that move value — including DOT/stablecoin
transfers, payment routing, and funded autonomous agent accounts — may implicate
money-transmission licensing, sanctions screening (OFAC and equivalents), and
tax-reporting obligations. No value-moving feature in Phase 5 may go live
without legal review covering: (a) jurisdictional money-transmission rules,
(b) sanctions screening integration or attestation, and (c) transaction
audit-export capability. See Phase 5 gate (§ 12) and traceability matrix
row "Regulatory compliance" (§ 16).

### 13.5 OSS governance (Rust project)

Adopt the following governance instruments early, before the first public
release, to avoid painful retrofits:

- **RFC process:** all breaking API changes and new primitives require an RFC
  document reviewed by maintainers before implementation begins.
- **Types-crate stability policy:** `polkagent-core` (public types) follows a
  documented semver stability contract; breaking changes require a major version
  bump and migration guide.
- **Security-disclosure channel:** a `SECURITY.md` with a private disclosure
  address and a defined response SLA must exist before any public crate
  publication.

---

## 14. Rust workspace and crate layout

```text
polkagent/
├── Cargo.toml                          # workspace root
├── crates/
│   ├── polkagent-core/                 # Shared types, IDs, errors
│   ├── polkagent-run/                  # Run/turn/step state machine
│   ├── polkagent-effect/               # Effect pipeline (Intent→Attempt→Outcome)
│   ├── polkagent-outbox/               # Durable ordered message delivery
│   ├── polkagent-grant/                # Policy/capability/grant resolution
│   ├── polkagent-artifact/             # Artifact lifecycle and lineage
│   ├── polkagent-event/                # Event stream and persistence
│   ├── polkagent-memory/               # Memory subsystem (episodic/semantic/procedural)
│   │
│   ├── polkagent-executor-trait/       # Executor port trait
│   ├── polkagent-executor-anthropic/   # Anthropic API executor
│   ├── polkagent-executor-openai/      # OpenAI-compatible executor
│   ├── polkagent-executor-local/       # Local model executor (ollama, llama.cpp)
│   ├── polkagent-executor-fake/        # Deterministic test executor
│   │
│   ├── polkagent-harness-trait/        # Harness port trait
│   ├── polkagent-harness-claude/       # Claude CLI harness
│   ├── polkagent-harness-codex/        # Codex CLI harness
│   ├── polkagent-harness-opencode/     # OpenCode harness
│   ├── polkagent-harness-bridge/       # Generic HTTP/WebSocket bridge
│   │
│   ├── polkagent-signer-trait/         # Signer port trait
│   ├── polkagent-signer-external/      # Browser extension signer
│   ├── polkagent-signer-proxy/         # Proxy/multisig signer
│   ├── polkagent-signer-kms/           # Managed KMS/HSM signer
│   ├── polkagent-signer-fake/          # Test signer
│   │
│   ├── polkagent-transport-trait/      # Transport port trait
│   ├── polkagent-transport-pca/        # PCA encrypted chat transport
│   ├── polkagent-transport-http/       # HTTP/WebSocket transport
│   ├── polkagent-transport-fake/       # Test transport
│   │
│   ├── polkagent-chain-trait/          # Chain adapter port trait
│   ├── polkagent-chain-subxt/          # Subxt chain adapter
│   ├── polkagent-metadata/             # Runtime metadata service
│   │
│   ├── polkagent-store-trait/          # Storage port trait
│   ├── polkagent-store-sqlite/         # SQLite storage adapter
│   ├── polkagent-store-postgres/       # PostgreSQL storage adapter
│   │
│   ├── polkagent-card/                 # Action card rendering
│   ├── polkagent-action-sign/          # Explain Before Sign action
│   ├── polkagent-action-transfer/      # Transfer action
│   ├── polkagent-action-governance/    # Governance actions
│   │
│   ├── polkagent-config/               # Configuration loading/validation
│   ├── polkagent-api/                  # REST/WebSocket API server
│   ├── polkagent-cli/                  # CLI binary
│   ├── polkagent-daemon/               # Background daemon
│   │
│   ├── polkagent-marketplace/          # Marketplace/registry client
│   ├── polkagent-skill/                # Skill loading and execution
│   ├── polkagent-tool/                 # Tool registry and dispatch
│   │
│   ├── polkagent-cloud-control/        # Control plane (managed mode)
│   ├── polkagent-cloud-worker/         # Worker registration/scheduling
│   ├── polkagent-billing/              # Usage metering and billing
│   │
│   └── polkagent-test-fixtures/        # Shared test fixtures and helpers
│
├── web/                                # Agent Studio (TypeScript/web)
├── docs/                               # Architecture decisions, guides
├── fixtures/                           # Test fixture data (extrinsics, metadata)
└── deploy/                             # Docker, Helm, systemd configs
```

### 13.1 Crate dependency rules

1. `polkagent-core` has zero internal dependencies — only external crates
2. Port trait crates (`*-trait`) depend only on `polkagent-core`
3. Adapter crates (`*-subxt`, `*-anthropic`, etc.) depend on their port trait + `polkagent-core`
4. No adapter may depend on another adapter
5. The kernel crates (`run`, `effect`, `outbox`, `grant`) depend on `polkagent-core` and port traits
6. Binary crates (`cli`, `daemon`, `api`) depend on kernel + selected adapters
7. No circular dependencies permitted

### 13.2 Phase-to-crate mapping

| Phase | Crates introduced |
|---|---|
| Phase 1 | core, run, effect, outbox, grant, artifact, event, store-trait, store-sqlite, executor-trait, executor-fake, signer-trait, signer-fake, transport-trait, transport-fake, config, test-fixtures |
| Phase 2 | executor-anthropic, signer-external, chain-trait, chain-subxt, metadata, card, action-sign, cli, api |
| Phase 3 | executor-openai, executor-local, transport-pca |
| Phase 4 | harness-trait, harness-claude, harness-codex, skill, tool, marketplace, memory, action-transfer |
| Phase 5 | cloud-control, cloud-worker, billing, store-postgres, signer-kms, signer-proxy |
| Phase 6 | Experimental crates as needed |

---

## 15. Architecture decision record template

Use this template for all architecture decisions. Store ADRs in `docs/adr/`.

```markdown
# ADR-NNN: [Short title]

**Status:** proposed | accepted | deprecated | superseded by ADR-NNN

**Date:** YYYY-MM-DD

**Deciders:** [who participated]

**PRD references:** [which PRD sections this implements]

## Context

[What is the issue? Why is a decision needed?]

## Decision

[What was decided and why?]

## Options considered

### Option A: [name]
- Pros: ...
- Cons: ...

### Option B: [name]
- Pros: ...
- Cons: ...

## Consequences

### Positive
- ...

### Negative
- ...

### Neutral
- ...

## Validation

[How will we verify this decision was correct?]

## Related decisions

- ADR-NNN: [related decision]
```

---

## 16. Cross-PRD requirement traceability matrix

This matrix traces key requirements from their source (owner decision or
research finding) through PRD definition to implementation phase, test
category, and acceptance evidence.

| Requirement | Source | PRD | Phase | Test category | Acceptance evidence |
|---|---|---|---|---|---|
| Models never receive raw signing keys | Owner decision | PRD-07 § 7.1 | 1 | Security invariant | Red-team test: model context never contains key material |
| Grants resolved outside model-controlled text | Owner decision | PRD-07 § 8 | 1 | Security invariant | Property test: grant resolution ignores model output |
| Effects durable before I/O | Architecture | PRD-03 § 5.2 | 1 | Crash recovery | Kill-during-I/O test: intent persists on restart |
| Unknown never collapsed | Architecture | PRD-03 § 5.4 | 1 | State integrity | Property test across all projections |
| Reputation never authorizes | Owner decision | PRD-07 § 11 | 1 | Security invariant | Reputation change cannot widen any grant |
| Least privilege default | Owner decision | PRD-07 § 8 | 1 | Security | New agent has zero effects without explicit grants |
| Local correctness without cloud | Owner decision | PRD-11 § 7 | 1 | Degradation | Disconnect drill: no grant widened, no silent execution |
| Explain Before Sign | Research brief | PRD-08 § 4 | 2 | E2E action | Comprehension study with critical-error threshold |
| PCA transport compatibility | Owner decision | PRD-06 § 6 | 3 | Integration | Bot connects, exchanges messages with PCA |
| Metadata-pinned decode | Architecture | PRD-05 § 3 | 2 | Correctness | Known extrinsic corpus with pinned metadata |
| Permissionless marketplace | Owner decision | PRD-12 § 2 | 4–5 | Integration | Publish without central gate, self-hosted registry works |
| Funded autonomous agents | Owner decision | PRD-08 § 7 | 5 | Security | Adversarial tests cannot widen authority |
| Portable state/config | Owner decision | PRD-11 § 8 | 3 | Portability | Export → import → verify on different deployment |
| Tenant isolation | Architecture | PRD-11 § 5 | 5 | Security | Cross-tenant access denial suite |
| Configurable custody | Owner decision | PRD-07 § 7 | 2–5 | Integration | Each signer mode passes contract tests |
| Equal Build/Act/Reach | Owner decision | PRD-01 § 3 | 2+ | Product | Each phase delivers to all three pillars |
| Regulatory compliance for value-moving | Research (Domain 00) | PRD-08 § autonomy | 5 | Legal gate | Counsel review complete; screening + audit-export hooks present; jurisdictional gates implemented |
| OSS Rust governance (RFC + stability + disclosure) | Research (Domain 00) | PRD-02 § workspace | Pre-release | Process | SECURITY.md, types-crate semver policy, and RFC template in repo before first crate publication |
| Durable execution (DBOS on SQLite; no Temporal) | Research (Domain 00) | PRD-03 § outbox | 1 | Architecture | Single-writer SQLite WAL + pure reducer + outbox passes fault injection; no Temporal dependency |
| Positioning: safety+Rust+local-first vs EVM wallets | Research (Domain 00) | PRD-01 § positioning | 2 | Product | Action card / DryRunApi demo; keys-outside-model red-team result |

---

## 17. Implementation checklists for agents

The per-phase checklists in sections 7.3, 8.3, 9.3, 10.3 above are the
primary implementation guides. This section provides cross-cutting checklists.

### 16.1 New crate checklist

```
For each new crate:
[ ] Create crate directory under crates/
[ ] Add to workspace Cargo.toml
[ ] Write lib.rs with module structure
[ ] Add README.md with purpose and dependencies
[ ] Add to CI test matrix
[ ] Verify dependency rules (§ 14.1)
[ ] Write at minimum one unit test
[ ] Write port contract tests if it's an adapter
[ ] Verify no circular dependencies (cargo tree)
```

### 16.2 New adapter checklist

```
For each new adapter (executor, signer, transport, chain, store):
[ ] Implement the corresponding port trait
[ ] Pass all contract tests from the trait crate
[ ] Handle all error cases from the trait's error type
[ ] Implement health check / connection validation
[ ] Write integration tests with real or mock backend
[ ] Document configuration options
[ ] Add to the config schema (PRD-14 § 6)
[ ] Write onboarding guide for adding similar adapters
```

### 16.3 New action checklist

```
For each new action type (sign, transfer, governance, etc.):
[ ] Define action intent type
[ ] Define pre-flight checks
[ ] Define canonical action card fields (PRD-13 § 7)
[ ] Define risk indicators
[ ] Define approval flow
[ ] Define signer handoff protocol
[ ] Define finality observation
[ ] Define failure/unknown handling
[ ] Write test corpus: valid, invalid, stale, wrong-network, malformed
[ ] Verify model text is visually separate from canonical data
[ ] Add to grant capability catalog
```

### 16.4 New surface checklist

```
For each new surface (CLI command, web view, mobile screen):
[ ] Verify it renders canonical state from the same source as other surfaces
[ ] Verify model narrative is visually separate from canonical data
[ ] Verify truthful status (unknown never shown as success/failure)
[ ] Verify accessibility (keyboard navigation, screen reader, contrast)
[ ] Verify reconnect/refresh recovers truthful state
[ ] Write consistency test: same run state renders identically across surfaces
```

---

## 18. Per-PRD acceptance criteria summary

### PRD-01: Vision, Principles, Personas and Product Pillars

| # | Criterion | Method |
|---|---|---|
| 1 | All three pillars are equally represented | Review: no pillar is treated as secondary |
| 2 | Every persona has at least one JTBD | Review: JTBD table complete |
| 3 | Glossary covers all terms used across PRDs 02–15 | Cross-reference check |
| 4 | Owner decisions table matches research brief exactly | Diff against 00-DEEP-RESEARCH-BRIEF.md § 2 |
| 5 | Non-goals are explicitly stated | Review: exclusions section present |

### PRD-02: Vocabulary, Invariants and System Architecture

| # | Criterion | Method |
|---|---|---|
| 1 | Every canonical term has exactly one definition | No duplicate/conflicting definitions across suite |
| 2 | System invariants are testable | Each invariant maps to a test in PRD-15 |
| 3 | Architecture layers are unambiguous | Dependency diagram has no circular paths |
| 4 | Rust crate layout is consistent with § 14 | Structural comparison |
| 5 | Trust boundaries are explicitly drawn | Diagram review |

### PRD-03: Agent/Run/Effect/Graph Execution Model

| # | Criterion | Method |
|---|---|---|
| 1 | State machines have complete transition tables | Model checking or exhaustive property tests |
| 2 | Rust trait sketches compile | Type-check against Rust compiler |
| 3 | Crash recovery is specified for every state | Review: every state has restart behavior |
| 4 | Three end-to-end examples are included | Review: examples present and self-consistent |
| 5 | Idempotency keys cover all effect types | Review: no effect type lacks idempotency |

### PRD-04: Providers, Models, Harnesses, Tools and Skills

| # | Criterion | Method |
|---|---|---|
| 1 | Taxonomy covers all provider types from research | Cross-reference with research brief § Package E |
| 2 | Capability descriptors distinguish provider semantics | Review: no false-equivalence across providers |
| 3 | Three end-to-end examples are included | Review: examples match research brief requirements |
| 4 | Contract tests are defined for each adapter type | Review: test specification present |
| 5 | Harness lifecycle includes health, cancel, resume | Review: all lifecycle operations specified |

### PRD-05: Polkadot Chain, SDK, JAM/PVM and Product-Building Integrations

| # | Criterion | Method |
|---|---|---|
| 1 | Integration matrix covers all listed ecosystems | Checklist against research brief Package A |
| 2 | Each integration has maturity label and evidence | Review: no unlabeled integration |
| 3 | JAM/PVM section is clearly marked experimental | Review: experimental labels present |
| 4 | Builder workflows A1–A10 are all addressed | Coverage check |
| 5 | Ecosystem primer is understandable by non-Polkadot developers | Readability review |

### PRD-06: PCA Compatibility, Chat/Mobile and Messaging

| # | Criterion | Method |
|---|---|---|
| 1 | Feature audit covers every PCA capability | Cross-reference with PCA source audit |
| 2 | C0–C3 tiers have clear, testable definitions | Review: each tier has acceptance tests |
| 3 | Migration schema covers state, config, identity | Schema review |
| 4 | User journey is understandable by PCA newcomers | Readability review |
| 5 | Bridge compatibility plan maintains existing integrations | Integration test specification |

### PRD-07: Identity, Accounts, Signers, Policy and Security

| # | Criterion | Method |
|---|---|---|
| 1 | All 9 signer modes are specified | Coverage check |
| 2 | Key isolation invariant is unambiguous | Review: single clear statement, testable |
| 3 | Grant resolution is fully specified | State machine review |
| 4 | Threat model covers OWASP + agent-specific threats | Threat coverage check |
| 5 | Reputation explicitly cannot authorize | Test specification present |

### PRD-08: Payments, Autonomous Agents and Economic Controls

| # | Criterion | Method |
|---|---|---|
| 1 | Autonomy tiers 0–3 are fully specified | Review: each tier has config, controls, gates |
| 2 | Explain Before Sign flow is end-to-end | Review: complete user flow diagram |
| 3 | Payment state machine is complete | Model checking |
| 4 | Compliance considerations are present (without legal advice) | Review |
| 5 | Phase gates for real-value operations are explicit | Review: named gates present |

### PRD-09: Memory, Knowledge, Learning, Multi-Agent Groups and Evals

| # | Criterion | Method |
|---|---|---|
| 1 | Memory types are distinct and non-overlapping | Definition review |
| 2 | Privacy controls include forget/export | Review: user controls specified |
| 3 | Grant intersection is formally defined | Property test specification |
| 4 | Evals cannot modify safety gates | Invariant test specification |
| 5 | Experimental modules are clearly separated from core | Architecture review |

### PRD-10: Data, Artifacts, Events, Observability and Recovery

| # | Criterion | Method |
|---|---|---|
| 1 | Artifact types cover all output categories | Coverage check |
| 2 | Event ordering guarantees are specified | Review: ordering semantics present |
| 3 | Recovery procedures cover crash, backup, disaster | Review: all recovery types present |
| 4 | Database schemas are sufficient for Phase 1 | Schema review against Phase 1 deliverables |
| 5 | Observability covers metrics, traces, logs | Coverage check |

### PRD-11: Self-Hosting, Managed Cloud and Multi-Tenancy

| # | Criterion | Method |
|---|---|---|
| 1 | Local and cloud are treated equally | Review: no cloud-required features in core |
| 2 | Three-plane separation is clean | Dependency review: no plane crosses boundary |
| 3 | Offline-safe degradation is specified | Review: behavior when control plane unavailable |
| 4 | Deployment modes cover Docker, systemd, K8s, desktop | Coverage check |
| 5 | Portability export/import is specified | Review: format and validation present |

### PRD-12: Marketplace, Registry, Extension SDK and Product Kits

| # | Criterion | Method |
|---|---|---|
| 1 | Publication is permissionless (no central gate) | Review: architecture allows self-hosted registries |
| 2 | Sandboxing model is specified | Review: isolation boundaries present |
| 3 | Product kit manifest is fully defined | Schema review |
| 4 | Revocation and vulnerability response are covered | Review: incident procedures present |
| 5 | Fee model is configurable | Review: fee configuration options present |

### PRD-13: UX, CLI, Studio, Inbox, Mobile and Operator Surfaces

| # | Criterion | Method |
|---|---|---|
| 1 | Action card anatomy separates canonical from model text | Wireframe review |
| 2 | CLI command structure is complete for Phase 2 | Coverage check |
| 3 | Accessibility requirements are specified | WCAG coverage check |
| 4 | Surface comparison recommends per-phase surfaces | Review: recommendation table present |
| 5 | Onboarding flow covers first-run and PCA migration | Review: both flows present |

### PRD-14: APIs, Schemas, Configuration and Migration

| # | Criterion | Method |
|---|---|---|
| 1 | API endpoints cover all core resources | Coverage check against PRD-03, 04, 10 types |
| 2 | Configuration schema is versioned | Review: version field and migration present |
| 3 | Database schemas support Phase 1 operations | Schema review |
| 4 | PCA migration path is specified | Review: import format and tool design present |
| 5 | API versioning policy is explicit | Review: policy document present |

### PRD-15: Testing, Security Assurance, Roadmap and Acceptance Criteria

| # | Criterion | Method |
|---|---|---|
| 1 | Threat model covers all trust boundaries from PRD-02 | Cross-reference |
| 2 | Testing pyramid covers all test types | Coverage check |
| 3 | Roadmap phases match this document (PRD-00 § 7–12) | Consistency check |
| 4 | CI/CD gates are specified per phase | Review: gates present per phase |
| 5 | Cross-PRD acceptance criteria cover all PRDs | Coverage check |

---

## 19. Suite-wide verification checklist

This checklist ensures the PRD suite as a whole is complete, consistent, and
free of gaps. An implementing agent should verify all items before beginning
implementation.

### 19.1 Completeness

```
[ ] All 15 PRDs exist and have content
[ ] PRD-00 (this document) indexes all PRDs correctly
[ ] Every research package (A–M) from the research brief is covered:
    [ ] A (Polkadot/product matrix) → PRD-05
    [ ] B (JAM/PVM) → PRD-05 § 12
    [ ] C (PCA compatibility) → PRD-06
    [ ] D (Roko synthesis) → PRD-02, PRD-03, PRD-09
    [ ] E (execution/providers) → PRD-03, PRD-04
    [ ] F (product engineering) → PRD-05
    [ ] G (payments/autonomy) → PRD-08
    [ ] H (identity/reputation) → PRD-07
    [ ] I (memory/multi-agent) → PRD-09
    [ ] J (cloud) → PRD-11
    [ ] K (marketplace) → PRD-12
    [ ] L (UX) → PRD-13
    [ ] M (assurance) → PRD-15
[ ] Every owner decision from research brief § 2 is preserved
[ ] Every candidate (A1–A10, B1–B9, C1–C4, D1–D4, E1–E5, F1–F7, G1–G5, H1–H4, I1–I4, J1–J4, K1–K6) is addressed
```

### 19.2 Consistency

```
[ ] Canonical vocabulary is used consistently (no term redefined differently)
[ ] System invariants are not contradicted by any PRD
[ ] Architecture layers and dependency rules are respected
[ ] Maturity labels are used consistently
[ ] Phase numbering matches between PRD-00 and PRD-15
[ ] Crate names are consistent between layout (§ 14) and phase deliverables
[ ] Cross-references between PRDs use correct section numbers
```

### 19.3 Self-containment

```
[ ] Every PRD has a reader orientation / purpose section
[ ] Every PRD defines its terms on first use
[ ] Every PRD is understandable without reading other PRDs first
[ ] Every PRD has its own acceptance criteria
[ ] No PRD requires opening the research corpus to understand requirements
```

### 19.4 Implementability

```
[ ] Rust trait sketches compile (or are clearly pseudo-code)
[ ] State machines have complete transition tables
[ ] Database schemas cover required operations
[ ] API endpoints match the data model
[ ] Configuration schema covers required settings
[ ] Test strategies are concrete and actionable
[ ] Phase gates have measurable criteria (not subjective judgments)
```

### 19.5 Safety and security

```
[ ] Key isolation invariant is stated and testable
[ ] Grant resolution is specified and outside model control
[ ] Effect durability is specified (intent before I/O)
[ ] Unknown outcomes are preserved (never collapsed)
[ ] Reputation is explicitly non-authorizing
[ ] Least privilege is the default
[ ] Offline-safe degradation is specified
[ ] Threat model covers agent-specific attacks
[ ] Red-team scenarios are defined
[ ] Security review gates exist for value-moving operations
```

### 19.6 No gaps or hallucinations

```
[ ] No PRD claims features are implemented (this is a design corpus)
[ ] No PRD promises unverified external capabilities
[ ] Every maturity label (experimental, phased) is honest
[ ] Every Polkadot integration has a dated maturity assessment
[ ] Every recommendation maps to a user job, not just technical elegance
[ ] No PRD invents requirements contradicting the owner's confirmed decisions
[ ] File sizes are proportional to topic complexity (no stub PRDs)
```

### 19.7 Cross-cutting research (Domain 00) integration

```
[ ] Competitive positioning is documented (§ 2.1) and referenced from PRD-01
[ ] Interop standards (MCP/A2A/x402/AP2) are scoped as adapters, not core abstractions
[ ] 12-step dependency-ordered build sequence (§ 6.1) is reflected in phase deliverables
[ ] Staged timeline (§ 6.2: 0-3 mo / 3-6 mo / 6-12 mo / trigger-gated) matches phase gates
[ ] All 10 risks from § 13.1 map to at least one mitigating PRD requirement
[ ] All 8 signature primitives from § 13.2 appear in at least one PRD invariant
[ ] Technology stack in § 13.3 is consistent with ADRs and PRD tool selections:
    [ ] Cedar policy engine (PRD-07)
    [ ] SQLite WAL single-writer (PRD-03, PRD-10)
    [ ] Wasmtime WIT sandbox (PRD-12)
    [ ] Sigstore cosign v3 + SLSA L2 (PRD-12)
    [ ] sqlite-vec + FTS5 + RRF (PRD-09)
    [ ] BLAKE3/SHA-256 content addressing (PRD-10)
    [ ] MCP Streamable HTTP / stdio (PRD-04)
    [ ] A2A v1.0.1 + AP2 (PRD-04, PRD-09)
    [ ] subxt static+dynamic + CheckMetadataHash (PRD-05)
    [ ] DryRunApi + XcmPaymentApi (PRD-05, PRD-08)
    [ ] OpenTelemetry + tracing crate (PRD-10)
    [ ] TOML config (PRD-14)
    [ ] proptest + cargo-fuzz (PRD-15)
    [ ] DBOS pattern on SQLite; Temporal rejected (PRD-03)
[ ] Regulatory flag (§ 13.4) is reflected in Phase 5 legal gate (§ 12)
[ ] OSS governance instruments (§ 13.5: RFC process, types-crate policy, SECURITY.md)
    exist or are planned before first crate publication
[ ] Funding strategy documented (OpenGov treasury + bounties + W3F targeted grants;
    Decentralized Futures closed) — not in PRDs, but in project docs
```

---

## Appendix A: PRD file inventory

| File | Size (bytes) | Lines | Words (approx) |
|---|---|---|---|
| PRD-01-VISION-PRINCIPLES-PERSONAS.md | 124,472 | 2,030 | ~18,000 |
| PRD-02-VOCABULARY-ARCHITECTURE.md | 91,552 | 2,151 | ~13,000 |
| PRD-03-EXECUTION-MODEL.md | 99,240 | 2,764 | ~14,000 |
| PRD-04-PROVIDERS-MODELS-TOOLS.md | 91,111 | 2,533 | ~13,000 |
| PRD-05-POLKADOT-INTEGRATIONS.md | 97,138 | 1,591 | ~14,000 |
| PRD-06-PCA-COMPATIBILITY.md | 135,135 | 2,754 | ~19,000 |
| PRD-07-IDENTITY-SECURITY.md | 83,102 | 1,939 | ~12,000 |
| PRD-08-PAYMENTS-AUTONOMY.md | 103,619 | 2,721 | ~15,000 |
| PRD-09-MEMORY-GROUPS-EVALS.md | 101,833 | 2,535 | ~15,000 |
| PRD-10-DATA-OBSERVABILITY.md | 109,758 | 2,462 | ~16,000 |
| PRD-11-DEPLOYMENT-CLOUD.md | 104,911 | 2,748 | ~15,000 |
| PRD-12-MARKETPLACE-EXTENSIONS.md | 92,517 | 2,284 | ~13,000 |
| PRD-13-UX-SURFACES.md | 105,250 | 2,093 | ~15,000 |
| PRD-14-APIs-SCHEMAS-CONFIG.md | 88,891 | 2,905 | ~13,000 |
| PRD-15-TESTING-ROADMAP.md | 100,154 | 1,385 | ~14,000 |
| **PRD-00-MASTER-INDEX.md** | **~60,000** | **~1,100** | **~9,000** |
| **Total** | **~1,588,683** | **~35,995** | **~228,000** |

---

## Appendix B: Research corpus provenance

The PRDs were synthesized from this research corpus:

| Document | Lines | Role |
|---|---|---|
| 00-DEEP-RESEARCH-BRIEF.md | 758 | Research brief with owner decisions and package requirements |
| 01-ESTABLISHED-BASELINE.md | 1,356 | Owner-confirmed planning baseline |
| 02-SOURCE-COVERAGE-CHECKLIST.md | 1,358 | Source coverage tracking |
| research-pca-cloud-compat.md | 918 | PCA audit and cloud architecture |
| research-polkadot-jam-products.md | 1,091 | Polkadot/JAM ecosystem research |
| research-roko-definitive.md | 774 | Roko pattern synthesis |
| reserach/research1.md | — | Opportunity catalog |
| reserach/research2.md | — | Consolidated 20-prompt research |
| reserach/research3.md | — | Domain 00 cross-cutting synthesis: positioning, risks, primitives, confirmed stack, regulatory flag, OSS governance, 12-step build sequence |

The research corpus is input evidence. The PRDs are the normative output.
Where PRDs conflict with older research, the PRDs are authoritative (except
where the owner's confirmed decisions in the research brief take precedence).

Research3.md findings have been integrated into PRD-00 at: § 2.1 (positioning),
§ 2.2 (interop standards), § 6.1 (12-step sequence), § 6.2 (staged timeline),
§ 13.1 (top 10 risks), § 13.2 (signature primitives), § 13.3 (technology stack),
§ 13.4 (regulatory flag), § 13.5 (OSS governance), § 16 (traceability matrix),
and § 19.7 (verification checklist).

---

*End of PRD-00. This document was generated 2026-07-30.*

---

## APPENDIX A: DETAILED IMPLEMENTATION BLUEPRINT

### A.1 Workspace / Crate Layout

The complete Cargo workspace for Polkagent follows a strict layered architecture
modeled on two proven references: Roko (18 crates, 177K LOC, self-hosting agent
platform) and Bardo (36+ crates, layered 0–7 architecture). Each crate maps to
one or more PRDs and obeys explicit dependency rules.

#### A.1.1 Full Cargo.toml Workspace Sketch

```toml
# polkagent/Cargo.toml
[workspace]
resolver = "2"
members = [
    # ── Layer 0: Zero-dependency primitives ──────────────────────────────
    "crates/polkagent-core",          # Shared IDs, types, errors, trait defs
    "crates/polkagent-primitives",    # BLAKE3, HDC vectors, content addresses

    # ── Layer 1: Port traits (depend only on polkagent-core) ─────────────
    "crates/polkagent-store-trait",
    "crates/polkagent-executor-trait",
    "crates/polkagent-signer-trait",
    "crates/polkagent-transport-trait",
    "crates/polkagent-chain-trait",

    # ── Layer 2: Kernel (depend on core + port traits) ───────────────────
    "crates/polkagent-run",           # Run/turn/step state machine
    "crates/polkagent-effect",        # Effect pipeline (Intent→Attempt→Outcome)
    "crates/polkagent-outbox",        # Durable ordered delivery
    "crates/polkagent-grant",         # Cedar policy + capability resolution
    "crates/polkagent-artifact",      # Artifact lifecycle and lineage
    "crates/polkagent-event",         # Event stream and persistence
    "crates/polkagent-config",        # TOML config loading/validation

    # ── Layer 3: Storage adapters ─────────────────────────────────────────
    "crates/polkagent-store-sqlite",  # SQLite WAL, single-writer, IMMEDIATE
    "crates/polkagent-store-postgres",# PostgreSQL (Phase 5, managed cloud)

    # ── Layer 4: Executor adapters ────────────────────────────────────────
    "crates/polkagent-executor-anthropic",
    "crates/polkagent-executor-openai",
    "crates/polkagent-executor-local",  # ollama / llama.cpp
    "crates/polkagent-executor-fake",   # Deterministic test executor

    # ── Layer 4: Harness adapters ─────────────────────────────────────────
    "crates/polkagent-harness-trait",
    "crates/polkagent-harness-claude",
    "crates/polkagent-harness-codex",
    "crates/polkagent-harness-opencode",
    "crates/polkagent-harness-bridge",

    # ── Layer 4: Signer adapters ──────────────────────────────────────────
    "crates/polkagent-signer-external",  # Browser extension
    "crates/polkagent-signer-proxy",     # Pure-proxy / multisig
    "crates/polkagent-signer-vault",     # HashiCorp Vault / hardware
    "crates/polkagent-signer-ledger",    # Ledger hardware wallet
    "crates/polkagent-signer-kms",       # Cloud KMS / HSM (Phase 5)
    "crates/polkagent-signer-fake",

    # ── Layer 4: Transport adapters ───────────────────────────────────────
    "crates/polkagent-transport-pca",    # PCA encrypted chat
    "crates/polkagent-transport-http",   # HTTP/WebSocket
    "crates/polkagent-transport-fake",

    # ── Layer 4: Chain adapters ───────────────────────────────────────────
    "crates/polkagent-chain-subxt",      # subxt static+dynamic decode
    "crates/polkagent-metadata",         # Runtime metadata service

    # ── Layer 5: Domain logic ─────────────────────────────────────────────
    "crates/polkagent-memory",           # Episodic/semantic/procedural memory
    "crates/polkagent-card",             # Action card rendering
    "crates/polkagent-action-sign",      # Explain Before Sign
    "crates/polkagent-action-transfer",  # Transfer actions
    "crates/polkagent-action-governance",# Governance actions
    "crates/polkagent-skill",            # Skill loading and execution
    "crates/polkagent-tool",             # Tool registry and dispatch
    "crates/polkagent-marketplace",      # Marketplace/registry client

    # ── Layer 6: API / server surface ────────────────────────────────────
    "crates/polkagent-api",              # REST/WebSocket API (axum)
    "crates/polkagent-mcp-server",       # MCP Streamable HTTP / stdio

    # ── Layer 6: Cloud infrastructure (Phase 5) ───────────────────────────
    "crates/polkagent-cloud-control",    # Control plane (managed mode)
    "crates/polkagent-cloud-worker",     # Worker registration/scheduling
    "crates/polkagent-billing",          # Usage metering and billing

    # ── Layer 7: Binaries ─────────────────────────────────────────────────
    "crates/polkagent-cli",              # Main CLI binary
    "crates/polkagent-daemon",           # Background daemon

    # ── Test infrastructure ───────────────────────────────────────────────
    "crates/polkagent-test-fixtures",    # Shared test helpers, fake data
    "tests",                             # End-to-end integration tests
]

default-members = [
    "crates/polkagent-cli",
    "crates/polkagent-mcp-server",
]

[workspace.package]
edition = "2024"
rust-version = "1.85"
license = "MIT OR Apache-2.0"
authors = ["Polkagent Contributors"]
repository = "https://github.com/paritytech/polkagent"

[workspace.dependencies]
# Async runtime
tokio = { version = "1.50", features = ["full"] }
futures = "0.3"
async-trait = "0.1"

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# Error handling
thiserror = "2.0"
anyhow = "1.0"

# Logging / tracing
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
opentelemetry = { version = "0.27", features = ["metrics"] }

# Database
rusqlite = { version = "0.35", features = ["bundled"] }
sqlite-vec = "0"  # local-first vector search

# Policy engine
cedar-policy = "4"

# Polkadot / chain
subxt = "0.38"

# Content addressing
blake3 = "1"

# Cryptography
k256 = { version = "0.13", features = ["ecdsa"] }
zeroize = { version = "1", features = ["derive"] }

# HTTP / Web
axum = { version = "0.8", features = ["ws"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls", "stream"] }
tower = { version = "0.5", features = ["limit", "load-shed"] }

# Plugin sandbox
wasmtime = "26"

# TUI
ratatui = "0.29"
crossterm = { version = "0.28", features = ["event-stream"] }

# Utilities
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
clap = { version = "4", features = ["derive"] }
dashmap = "6"
parking_lot = "0.12"

# Testing
proptest = "1.10"
tempfile = "3"
criterion = { version = "0.5", features = ["html_reports"] }

[workspace.lints.rust]
unsafe_code = "deny"
missing_docs = "warn"

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
nursery = { level = "warn", priority = -1 }
unwrap_used = "deny"
module_name_repetitions = "allow"
must_use_candidate = "allow"
missing_errors_doc = "allow"

[profile.release]
lto = "thin"
codegen-units = 1
strip = true
panic = "abort"

[profile.dev]
debug = true
opt-level = 1
```

#### A.1.2 Crate Descriptions

| Crate | Type | Description | Primary PRD(s) |
|---|---|---|---|
| `polkagent-core` | lib | Shared IDs (RunId, AgentId, TurnId, StepId, EffectId, ArtifactId), error types, configuration types, trait foundations | PRD-02, PRD-03 |
| `polkagent-primitives` | lib | BLAKE3 content addressing, HDC vectors, zero-dependency utilities | PRD-02, PRD-10 |
| `polkagent-store-trait` | lib | Store port trait: CRUD for runs, effects, artifacts, events, grants | PRD-10 |
| `polkagent-executor-trait` | lib | Executor port trait: streaming inference, token usage, tool calls | PRD-04 |
| `polkagent-signer-trait` | lib | Signer port trait: sign bytes, verify, key handles, zeroize | PRD-07 |
| `polkagent-transport-trait` | lib | Transport port trait: send/receive messages with ACK | PRD-06 |
| `polkagent-chain-trait` | lib | Chain port trait: decode extrinsics, submit, observe finality | PRD-05 |
| `polkagent-run` | lib | Run/turn/step state machine, lifecycle management, persistence | PRD-03 |
| `polkagent-effect` | lib | Effect pipeline: EffectIntent (before I/O), EffectAttempt (claim/lease), EffectOutcome (immutable) | PRD-03 |
| `polkagent-outbox` | lib | Durable ordered delivery, at-least-once, consumer dedup | PRD-03 |
| `polkagent-grant` | lib | Cedar policy evaluation, grant resolution outside agent surface, capability matching, time-bounded grants | PRD-07 |
| `polkagent-artifact` | lib | Artifact creation, content hashing, lineage tracking, lifecycle | PRD-10 |
| `polkagent-event` | lib | Event creation, ordering, in-process broadcast, persistence | PRD-10 |
| `polkagent-config` | lib | TOML parsing, schema validation, env-var overrides, defaults | PRD-14 |
| `polkagent-store-sqlite` | lib | SQLite WAL adapter, single-writer, BEGIN IMMEDIATE, busy_timeout=5000ms | PRD-10, PRD-14 |
| `polkagent-store-postgres` | lib | PostgreSQL adapter for managed cloud (Phase 5) | PRD-11, PRD-14 |
| `polkagent-executor-anthropic` | lib | Anthropic API: streaming, tool calls, token tracking, retries | PRD-04 |
| `polkagent-executor-openai` | lib | OpenAI-compatible: same streaming interface | PRD-04 |
| `polkagent-executor-local` | lib | Local model (ollama, llama.cpp): offline execution | PRD-04 |
| `polkagent-executor-fake` | lib | Deterministic test executor: scripted responses | PRD-04, PRD-15 |
| `polkagent-harness-trait` | lib | Harness port: create/resume/health/cancel long-lived coding agent sessions | PRD-04 |
| `polkagent-harness-claude` | lib | Claude CLI harness: spawn, attach, workspace binding | PRD-04 |
| `polkagent-harness-codex` | lib | Codex CLI harness | PRD-04 |
| `polkagent-harness-opencode` | lib | OpenCode harness | PRD-04 |
| `polkagent-harness-bridge` | lib | Generic HTTP/WebSocket bridge harness | PRD-04 |
| `polkagent-signer-external` | lib | Browser extension signing: payload handoff, byte verification | PRD-07 |
| `polkagent-signer-proxy` | lib | Pure-proxy / multisig coordinator | PRD-07, PRD-08 |
| `polkagent-signer-vault` | lib | HashiCorp Vault + Ledger hardware: keys never leave device | PRD-07 |
| `polkagent-signer-ledger` | lib | Ledger hardware wallet adapter | PRD-07 |
| `polkagent-signer-kms` | lib | Cloud KMS / HSM adapter (Phase 5) | PRD-07, PRD-11 |
| `polkagent-signer-fake` | lib | Test signer: accepts all, deterministic output | PRD-07, PRD-15 |
| `polkagent-transport-pca` | lib | PCA encrypted chat: connect, register, send/receive/ACK | PRD-06 |
| `polkagent-transport-http` | lib | HTTP/WebSocket transport for external delivery | PRD-06 |
| `polkagent-transport-fake` | lib | In-memory test transport | PRD-06, PRD-15 |
| `polkagent-chain-subxt` | lib | subxt static+dynamic decode, CheckMetadataHash, DryRunApi, XcmPaymentApi, finality subscription | PRD-05 |
| `polkagent-metadata` | lib | Metadata fetching, caching, pinning, drift detection, SCALE decode | PRD-05 |
| `polkagent-memory` | lib | Episodic/semantic/procedural memory (sqlite-vec + FTS5 + RRF), provenance, tenant isolation, forget/export | PRD-09 |
| `polkagent-card` | lib | Action card data model, renderer, risk indicators, canonical/model text separation | PRD-13 |
| `polkagent-action-sign` | lib | Explain Before Sign: intent → pre-flight → card → signer → finality | PRD-08 |
| `polkagent-action-transfer` | lib | DOT/stablecoin transfer on Asset Hub via pure-proxy + budgets | PRD-08 |
| `polkagent-action-governance` | lib | OpenGov: referendum read/vote/delegate | PRD-08 |
| `polkagent-skill` | lib | Skill loading, validation, execution in Wasmtime WIT sandbox | PRD-12 |
| `polkagent-tool` | lib | Tool registry, dispatch, capability checking, MCP tool bridge | PRD-04, PRD-12 |
| `polkagent-marketplace` | lib | Registry client, package manifest, cosign/SLSA verification | PRD-12 |
| `polkagent-api` | lib | REST/WebSocket API server (axum): agents, runs, effects, artifacts, events, grants | PRD-14 |
| `polkagent-mcp-server` | bin | MCP Streamable HTTP / stdio server: tool and resource exposure | PRD-04 |
| `polkagent-cloud-control` | lib | Control plane: tenant management, quota, config distribution | PRD-11 |
| `polkagent-cloud-worker` | lib | Worker registration, job scheduling, fleet management | PRD-11 |
| `polkagent-billing` | lib | Usage metering, cost attribution, audit export | PRD-11 |
| `polkagent-cli` | bin | Main CLI: all subcommands, ratatui TUI, `polkagent dashboard` | PRD-13 |
| `polkagent-daemon` | bin | Background daemon: launchd/systemd integration | PRD-11 |
| `polkagent-test-fixtures` | lib | Shared test data: extrinsic corpus, metadata snapshots, fake agents | PRD-15 |

#### A.1.3 Dependency Graph

```text
polkagent-core (no internal deps)
polkagent-primitives (no internal deps)
    │
    ├── polkagent-store-trait → core
    ├── polkagent-executor-trait → core
    ├── polkagent-signer-trait → core, primitives
    ├── polkagent-transport-trait → core
    ├── polkagent-chain-trait → core
    │
    ├── polkagent-run → core, store-trait
    ├── polkagent-effect → core, store-trait, outbox
    ├── polkagent-outbox → core, store-trait
    ├── polkagent-grant → core, primitives (Cedar dep)
    ├── polkagent-artifact → core, primitives, store-trait
    ├── polkagent-event → core, store-trait
    ├── polkagent-config → core
    │
    ├── polkagent-store-sqlite → store-trait, core
    ├── polkagent-store-postgres → store-trait, core
    │
    ├── polkagent-executor-{anthropic,openai,local,fake} → executor-trait, core
    ├── polkagent-harness-{trait,claude,codex,opencode,bridge} → executor-trait, core
    ├── polkagent-signer-{external,proxy,vault,ledger,kms,fake} → signer-trait, core
    ├── polkagent-transport-{pca,http,fake} → transport-trait, core
    ├── polkagent-chain-subxt → chain-trait, core, metadata
    ├── polkagent-metadata → chain-trait, core, primitives
    │
    ├── polkagent-memory → core, store-trait, artifact, event
    ├── polkagent-card → core, chain-trait, primitives
    ├── polkagent-action-sign → core, effect, card, chain-subxt, grant
    ├── polkagent-action-transfer → core, effect, card, chain-subxt, grant
    ├── polkagent-action-governance → core, effect, card, chain-subxt, grant
    ├── polkagent-skill → core, grant (Wasmtime dep)
    ├── polkagent-tool → core, grant, skill
    ├── polkagent-marketplace → core, skill, tool (cosign/SLSA dep)
    │
    ├── polkagent-api → run, effect, artifact, event, grant, config (axum dep)
    ├── polkagent-mcp-server → tool, executor-trait, config
    │
    ├── polkagent-cloud-control → api, billing, config
    ├── polkagent-cloud-worker → run, effect, config
    ├── polkagent-billing → core, store-trait
    │
    └── polkagent-cli → ALL above (CLI orchestration)
        polkagent-daemon → run, effect, outbox, api, config
```

**Rule: no adapter may depend on another adapter.** `polkagent-chain-subxt` may not
import `polkagent-executor-anthropic`. Both reach up from the same trait layer.

#### A.1.4 Feature Flags Per Crate

| Crate | Feature flags | Notes |
|---|---|---|
| `polkagent-core` | (none) | Intentionally featureless; always minimal |
| `polkagent-store-sqlite` | `bundled` (default), `encryption` | `bundled` uses rusqlite bundled SQLite |
| `polkagent-store-postgres` | `tls` (default) | Requires PostgreSQL at runtime |
| `polkagent-executor-local` | `cuda`, `metal` | GPU acceleration for local models |
| `polkagent-memory` | `vec` (default), `fts5` (default), `tantivy` | `vec` = sqlite-vec, `fts5` = SQLite FTS5 |
| `polkagent-chain-subxt` | `polkadot`, `asset-hub`, `westend`, `kusama` | Chain-specific generated type bundles |
| `polkagent-skill` | `wasmtime` (default), `native` (testing) | `native` skips sandbox for tests |
| `polkagent-cli` | `tui` (default), `no-tui` | `no-tui` for headless/server builds |
| `polkagent-daemon` | `systemd`, `launchd` | Platform-specific service integration |
| `polkagent-api` | `grpc`, `rest` (default), `ws` (default) | gRPC is Phase 5 |

#### A.1.5 PRD-to-Crate Mapping

| PRD | Primary crates | Secondary crates |
|---|---|---|
| PRD-02 (Architecture) | `polkagent-core`, `polkagent-primitives` | All port-trait crates |
| PRD-03 (Execution Model) | `polkagent-run`, `polkagent-effect`, `polkagent-outbox` | `store-sqlite`, `executor-trait` |
| PRD-04 (Providers/Tools) | `polkagent-executor-*`, `polkagent-harness-*`, `polkagent-tool`, `polkagent-mcp-server` | `polkagent-skill` |
| PRD-05 (Polkadot) | `polkagent-chain-subxt`, `polkagent-metadata` | `action-sign`, `action-transfer` |
| PRD-06 (PCA/Messaging) | `polkagent-transport-pca`, `polkagent-transport-http` | `polkagent-api` |
| PRD-07 (Identity/Security) | `polkagent-grant`, `polkagent-signer-*` | `polkagent-core` |
| PRD-08 (Payments) | `polkagent-action-sign`, `polkagent-action-transfer`, `polkagent-action-governance` | `polkagent-grant`, `chain-subxt` |
| PRD-09 (Memory/Groups) | `polkagent-memory` | `polkagent-grant`, `polkagent-artifact` |
| PRD-10 (Data/Observability) | `polkagent-artifact`, `polkagent-event`, `polkagent-store-sqlite` | `polkagent-primitives` |
| PRD-11 (Deployment) | `polkagent-cloud-control`, `polkagent-cloud-worker`, `polkagent-billing`, `polkagent-daemon` | `store-postgres` |
| PRD-12 (Marketplace) | `polkagent-marketplace`, `polkagent-skill`, `polkagent-tool` | `polkagent-grant` |
| PRD-13 (UX/Surfaces) | `polkagent-cli`, `polkagent-card` | `polkagent-api` |
| PRD-14 (APIs/Config) | `polkagent-api`, `polkagent-config` | `store-sqlite`, `store-postgres` |
| PRD-15 (Testing) | `polkagent-test-fixtures` | All crates |

---

### A.2 Phased Implementation Roadmap with Detailed Milestones

#### Phase 1: Safety Kernel (Estimated: 8–12 weeks, 2 engineers)

**Goal:** Correct, crash-safe execution core. No real I/O. Fault injection
shows no duplicate effects.

| Milestone | Deliverables | Acceptance criteria | Complexity | Risks |
|---|---|---|---|---|
| 1A: Workspace bootstrap | Cargo.toml, CI pipeline (fmt+clippy+test), `polkagent-core` types compile | `cargo build --workspace` green; `cargo clippy -D warnings` clean | S | None |
| 1B: State machine | `polkagent-run`: RunState enum, all transitions, guard functions, persistence to store trait | proptest exhaustive: every state has a defined transition; no unreachable arms | M | State space explosion if too fine-grained |
| 1C: Effect pipeline | `polkagent-effect`: EffectIntent persisted before I/O, EffectAttempt lifecycle, EffectOutcome immutability | Fault injection at 5 kill points; restart never duplicates completed effect | L | SQLite single-writer contention under test parallelism |
| 1D: Durable outbox | `polkagent-outbox`: ordered delivery, at-least-once, consumer dedup via idempotency key | Inject duplicate messages; consumer receives exactly once | M | None |
| 1E: Cedar grant engine | `polkagent-grant`: Cedar policy evaluation, capability matching, time-bounded grants, keys-outside-model | Red-team: no path from grant to key material; undeclared caps rejected | L | Cedar Lean proofs are slow to compile; cache compiled policies |
| 1F: Artifact + event | `polkagent-artifact`: BLAKE3 content hash, lineage graph; `polkagent-event`: ordered broadcast | Lineage test: chain of 10 artifacts, verify parent/child integrity | M | None |
| 1G: SQLite adapter | `polkagent-store-sqlite`: WAL, BEGIN IMMEDIATE, busy_timeout=5000ms, schema for all Phase 1 entities | Write-contention test with 100 concurrent readers, 1 writer; zero deadlocks | M | WAL checkpoint timing; reader starvation under heavy write load |
| 1H: Fake adapters | `executor-fake`, `signer-fake`, `transport-fake`: pass port contract test suites | All contract tests green; deterministic responses enable reproducible tests | S | None |
| 1I: Config loading | `polkagent-config`: TOML parse, schema validation, env-var overrides | Fuzz test: 10,000 malformed configs rejected with structured errors | S | None |

**Phase gate:** fault injection test suite (AC-P1-001 through AC-P1-009) all
pass. No external network calls anywhere in the test suite.

**Dependencies on prior phases:** none — this is the foundation.

**Mitigations:**
- SQLite write contention: single-writer Tokio task owns the write connection exclusively
- Cedar compile time: pre-compile policies at startup, cache compiled `PolicySet`
- State machine completeness: use a proc-macro or const-checked enum to enforce exhaustive transitions

#### Phase 2: Build + Act + Reach Proof (Estimated: 10–14 weeks, 3 engineers)

**Goal:** One useful action from each pillar: explain an extrinsic (Build), submit
a signed transaction (Act), and exchange a message (Reach).

| Milestone | Deliverables | Acceptance criteria | Complexity | Risks |
|---|---|---|---|---|
| 2A: Subxt adapter | `polkagent-chain-subxt`: static+dynamic decode, CheckMetadataHash enforcement, DryRunApi, XcmPaymentApi | Known extrinsic corpus (Polkadot, Asset Hub, Westend) decodes correctly; stale metadata produces explicit error | XL | Runtime upgrades break generated types; mitigate with dynamic fallback |
| 2B: Metadata service | `polkagent-metadata`: fetch, cache, pin (hash→snapshot), drift detection, SCALE decode | Outdated metadata vs live chain → error, not wrong decode | L | Metadata size (multi-MB); cache invalidation strategy |
| 2C: Anthropic executor | `polkagent-executor-anthropic`: streaming, tool calls, token tracking | Streaming test captures all tokens; tool calls round-trip correctly | M | API rate limits; implement exponential backoff |
| 2D: Explain Before Sign | `polkagent-action-sign`: intent → pre-flight (balance/fee/ED/nonce) → canonical card → signer handoff → finality | Signer receives byte-identical payload to action card; card separates model text from canonical data | XL | DryRunApi unavailability; add fallback warning mode |
| 2E: Action card | `polkagent-card`: card data model, risk indicators, approval/deny controls, renderer | UX review: canonical fields visible; model narrative visually distinct and cannot obscure fields | M | None |
| 2F: External signer | `polkagent-signer-external`: browser extension signing protocol, byte verification | Integration: card → signer → verify byte equality | M | Browser extension API differences across wallets |
| 2G: CLI surface | `polkagent-cli`: `polkagent agent|run|explain|sign|config` commands, interactive approval, JSON output | CLI and web show identical run state for same run ID | M | None |
| 2H: REST API | `polkagent-api`: CRUD for agents/runs/effects; WebSocket event streaming | API contract test suite green; WebSocket reconnects without state loss | L | WebSocket backpressure; implement bounded send buffers |
| 2I: PCA C0 bootstrap | `polkagent-transport-pca`: connect, register, send/receive/ACK | Integration: bot connects to PCA, exchanges test messages | M | PCA protocol documentation gaps; study PCA source directly |

**Phase gate:** AC-P2-001 through AC-P2-008 pass. Comprehension study: 5 users
show correct understanding of action cards; zero critical-error threshold.

**Dependencies:** Phase 1 complete (kernel, outbox, grant, SQLite, fake adapters).

#### Phase 3: Read-Only Value Expansion (Estimated: 6–8 weeks, 2 engineers)

**Goal:** Useful read-only workflows across all three pillars. Zero write effects.
Real bounded jobs show repeat use.

| Milestone | Deliverables | Acceptance criteria | Complexity | Risks |
|---|---|---|---|---|
| 3A: Builder read tools | Storage migration rehearsal (A1), upgrade impact brief (A2), metadata-grounded RAG with citations (A9) | AC-P3-002: RAG answers cite metadata hash, block, and source | L | RAG relevance quality; tune RRF blend ratios |
| 3B: Governance read tools | OpenGov referendum brief reader (B2), treasury/portfolio research (B9) | AC-P3-001: zero write effects in any run; effect audit passes | M | Polkadot OpenGov API rate limits |
| 3C: UX improvements | Live run timeline (F3), error/recovery explainer (F7), identity signal display (I2) | Reconnect: SSE client auto-reconnects within 3s, timeline state intact | M | None |
| 3D: PCA C0 complete | Full C0 compatibility: message schema, bot lifecycle, ACK semantics | AC-P3-003: PCA bot integration test passes | M | PCA schema changes |
| 3E: More executors | OpenAI-compatible executor, local ollama executor, model fallback chain | Fallback test: primary model fails → local model responds correctly | M | ollama API instability across versions |

**Phase gate:** real bounded jobs run with repeat use; AC-P3-001 through AC-P3-005.

#### Phase 4: Controlled Write Expansion (Estimated: 10–14 weeks, 3 engineers)

**Goal:** Carefully gated write operations. Each family has schema, simulation,
failure, and approval evidence.

| Milestone | Deliverables | Acceptance criteria | Complexity | Risks |
|---|---|---|---|---|
| 4A: Pre-sign risk gates | Risk gate engine: batch/proxy/approval pattern detection, seeded dangerous-call corpus | AC-P4-001: seeded fixtures flagged correctly; zero false-negatives on dangerous calls | L | False-positive rate on legitimate batch calls |
| 4B: Multisig/proxy | Multisig state reader, pure-proxy coordinator, coordinator holds no keys | AC-P4-002: multi-party test with pure-proxy edge cases | XL | Polkadot multisig threshold complexity |
| 4C: XCM planner | XCM route planner prototype (testnet only), unsupported routes rejected | AC-P4-003: route test corpus with failure cases | XL | XCM v4 route table incompleteness |
| 4D: Harness lifecycle | `harness-claude`, `harness-codex`: create, resume after restart, health, cancel, workspace binding | AC-P4-006: session persists and resumes after process kill | L | Claude CLI API stability |
| 4E: Memory v1 | `polkagent-memory`: episodic+semantic storage (sqlite-vec+FTS5+RRF), provenance, tenant isolation, forget/export | AC-P4-007: cross-tenant access denied in all test cases | XL | sqlite-vec index performance at scale |
| 4F: Product kits v1 | Kit manifest format, install workflow, conformance test framework, uninstall/rollback | AC-P4-004: kit lifecycle test suite green | M | Kit dependency resolution conflicts |
| 4G: PCA C1 compat | Bridge protocol, OpenAPI compatibility layer | AC-P4-005: bridge integration test corpus green | M | PCA OpenAPI schema drift |

**Phase gate:** AC-P4-001 through AC-P4-007. Family-specific evidence: each
write operation type has a documented simulation test with failure case.

#### Phase 5: Managed, Public, Value-Moving (Estimated: 16–24 weeks, 4 engineers + legal)

**Goal:** Managed cloud, real-value operations, marketplace publication. Requires
external security review and legal review before go-live.

| Milestone | Deliverables | Acceptance criteria | Complexity | Risks |
|---|---|---|---|---|
| 5A: Funded accounts | Pure-proxy funded accounts with policy-bounded budgets | AC-P5-001: adversarial budget exhaustion test; agent cannot exceed limit | XL | Regulatory exposure; block on legal review |
| 5B: Agent earns/spends | Prototype agent economic model: earn via skills, spend via transfers | AC-P5-002: adversarial prompt cannot widen authority | XL | Regulatory exposure; requires counsel sign-off |
| 5C: Marketplace v1 | Wasmtime WIT sandbox, cosign/SLSA Build L2 supply chain, permissionless publish, self-hosted registry | AC: kit install completes in < 30s; cosign verification mandatory | XL | Sigstore infrastructure availability |
| 5D: Managed cloud | Control/data/worker plane split; multi-tenant SQLite or Postgres; tenant isolation | AC-P5-004: cross-tenant access denied; tenant isolation test suite | XL | Operational complexity; start with single-region |
| 5E: Billing/metering | Usage metering, cost attribution per tenant/agent, audit export | AC: billing audit export matches run logs exactly | L | None |
| 5F: Fleet workers | Worker registration, job scheduling, fleet management | AC: fleet of 10 workers; any worker failure auto-rescheduled | L | None |
| 5G: Legal + security gate | External penetration test, signer/custody review, legal/compliance review | AC-P5-006: external review report accepted; AC-P5-007: legal sign-off | — | **Hard gate: no real value moves before this** |

**Phase gate:** AC-P5-001 through AC-P5-007. Independent security review.
Legal review covering money-transmission, OFAC screening, tax-reporting.

#### Phase 6: Experimental Frontier (Ongoing)

| Experiment | Gate criteria | Complexity |
|---|---|---|
| JAM service prototypes (D1–D4) | JAM testnet accessible; implementation evidence exists | XL |
| Personhood gating (I3) | W3C DID credentials pipeline working; no production write authority | L |
| Affect/vitality modules | Clearly isolated; no core correctness dependency | M |
| Evolutionary skill selection (G5) | Eval harness operational; experimental label visible in UI | XL |
| Always-on watchers (C4) | Separate feature flag; security gate for write authority | L |

---

### A.3 Build Sequence Detail

This expands the 12-step build sequence from § 6.1 with prerequisites, outputs,
verification, and rollback procedures.

#### Step 1: Kernel + types + ports skeleton

**Prerequisites:** Rust toolchain 1.85+ installed; `cargo-nextest` installed.

**Inputs:** PRD-02 § 6 (workspace layout), PRD-03 § all Rust sketches.

**Exact output artifacts:**
- `/polkagent/Cargo.toml` — workspace root
- `/polkagent/crates/polkagent-core/src/lib.rs` — all ID types, error enum
- `/polkagent/crates/polkagent-core/src/types/` — RunState, EffectIntent, EffectAttempt, EffectOutcome, Grant, Capability, Artifact, Event
- All port-trait crates: `lib.rs` with trait definition only
- CI: `.github/workflows/ci.yml` running fmt + clippy + nextest

**Verification:** `cargo build --workspace` green; `cargo clippy --workspace -- -D warnings` clean; `cargo nextest run --workspace` green (no tests yet, but compilation succeeds).

**Rollback:** delete the workspace root; no state has been written.

#### Step 2: SQLite WAL authority store + event sourcing + transactional outbox

**Prerequisites:** Step 1 complete.

**Inputs:** PRD-10 § 7.2 (schema), PRD-03 § 6 (outbox), PRD-14 § 7 (migration).

**Exact output artifacts:**
- `polkagent-store-sqlite/src/schema.sql` — DDL for runs, effects, artifacts, events, grants, outbox
- `polkagent-store-sqlite/src/lib.rs` — `SqliteStore` implementing `StorePort`
- `polkagent-outbox/src/lib.rs` — `DurableOutbox` with SQLite backing
- `polkagent-effect/src/lib.rs` — `EffectPipeline` with transactional outbox integration
- Integration tests: fault injection at 5 kill points

**Verification:**
```bash
cargo nextest run -p polkagent-store-sqlite
cargo nextest run -p polkagent-effect
# Manually: kill process during effect commit; verify on restart effect not duplicated
```

**Rollback:** delete `.polkagent/` data directory; schema is idempotent via `CREATE TABLE IF NOT EXISTS`.

#### Step 3: Cedar policy gate + keys-outside-model

**Prerequisites:** Step 1, Step 2 (grant storage).

**Inputs:** PRD-07 § 8 (grant model), PRD-13.2 (signature primitives), ADR for Cedar selection.

**Exact output artifacts:**
- `polkagent-grant/src/lib.rs` — `CedarGrantEngine`: policy compilation, principal/resource/action evaluation
- `polkagent-grant/src/key_handle.rs` — `KeyHandle` newtype + `zeroize::Zeroize` impl; raw key bytes never exposed
- `polkagent-grant/policies/defaults.cedar` — default deny-all policy
- Red-team test suite: 20 cases attempting key extraction via grant API

**Verification:**
```bash
cargo nextest run -p polkagent-grant
# Red-team: confirm KeyHandle never returns raw bytes; no path from grant API to key material
```

**Rollback:** revert to default-deny policy; grant store schema is additive.

#### Step 4: subxt static+dynamic decode + CheckMetadataHash + Vault/Ledger adapters

**Prerequisites:** Steps 1–3.

**Inputs:** PRD-05 § 2.11 (subxt), PRD-05 § 3 (metadata pinning).

**Exact output artifacts:**
- `polkagent-chain-subxt/src/lib.rs` — `SubxtChainAdapter` implementing `ChainPort`
- `polkagent-chain-subxt/src/codegen/` — generated types for Polkadot, Asset Hub, Westend
- `polkagent-metadata/src/lib.rs` — `MetadataService`: fetch, pin, cache, drift-detect
- `polkagent-signer-vault/src/lib.rs` — Vault transit engine adapter
- `polkagent-signer-ledger/src/lib.rs` — Ledger SCALE signing via ledger-rs
- Test fixture: `/polkagent/fixtures/extrinsics/` — known extrinsic corpus (50+ samples)

**Verification:**
```bash
# Against Westend testnet:
cargo nextest run -p polkagent-chain-subxt -- --ignored  # network tests
# Verify: stale metadata → explicit error; wrong-network extrinsic → rejected
```

**Rollback:** disable subxt adapter in config; fall back to no chain connectivity.

#### Step 5: DryRunApi + XcmPaymentApi evidence engine → action cards → approval UX

**Prerequisites:** Steps 1–4.

**Inputs:** PRD-08 § 4 (Explain Before Sign), PRD-13 § 7 (action card anatomy).

**Exact output artifacts:**
- `polkagent-action-sign/src/lib.rs` — full Explain Before Sign flow
- `polkagent-card/src/lib.rs` — `ActionCard` data model + renderer
- `polkagent-card/src/separator.rs` — enforces visual separation: canonical fields above fold, model text below
- `polkagent-cli/src/commands/explain.rs` — `polkagent explain` command
- Comprehension test scripts: `/polkagent/tests/comprehension/`

**Verification:** comprehension study with 5 users; zero critical errors on action meaning.

**Rollback:** disable action-sign feature; CLI falls back to raw extrinsic display.

#### Step 6: Provider adapters + MCP Streamable HTTP/stdio tool port

**Prerequisites:** Steps 1–3.

**Inputs:** PRD-04 § 2 (providers), PRD-04 § 6 (tools).

**Exact output artifacts:**
- `polkagent-executor-anthropic/src/lib.rs`
- `polkagent-executor-openai/src/lib.rs`
- `polkagent-executor-local/src/lib.rs` (ollama HTTP)
- `polkagent-tool/src/lib.rs` — tool registry, dispatch, capability check
- `polkagent-mcp-server/src/main.rs` — MCP Streamable HTTP server on :6688

**Verification:**
```bash
cargo run -p polkagent-mcp-server &
# Test MCP client connects, lists tools, calls tool, gets response
```

**Rollback:** disable MCP server in config; executors fall back to fake.

#### Step 7: Payments — DOT/stablecoin on Asset Hub via pure-proxy + budgets + DryRunApi pre-flight

**Prerequisites:** Steps 1–5.

**Inputs:** PRD-08 § 7 (funded accounts), PRD-08 § 6 (risk gates).

**Exact output artifacts:**
- `polkagent-action-transfer/src/lib.rs` — transfer pipeline: DryRunApi → card → approval → pure-proxy → finality
- `polkagent-grant/src/budget.rs` — `HardBudget` enforced inside Cedar policy, not in agent code
- Testnet integration: transfer 0.01 DOT on Westend Asset Hub

**Verification:** adversarial test: prompt agent to exceed budget; Cedar rejects before subxt call.

**Rollback:** disable `payments` feature flag; all transfer actions return `CapabilityDenied`.

#### Step 8: Memory (sqlite-vec+FTS5+RRF) + observability (tracing+OTel+redaction)

**Prerequisites:** Steps 1–3.

**Inputs:** PRD-09 § 2–4 (memory), PRD-10 § 9 (observability).

**Exact output artifacts:**
- `polkagent-memory/src/lib.rs` — `MemoryStore` with sqlite-vec index, FTS5, RRF fusion
- `polkagent-memory/src/privacy.rs` — tenant boundary enforcement, forget, export
- Tracing instrumentation in all kernel crates (spans for every run/turn/effect)
- OTel export config in `polkagent-config`

**Verification:**
```bash
# Cross-tenant test: agent A cannot read agent B's memories
cargo nextest run -p polkagent-memory -- tenant_isolation
```

**Rollback:** disable memory feature; agents operate without episodic recall.

#### Step 9: REACH — PCA C1/C2/C3, Statement Store, Bulletin CID anchoring

**Prerequisites:** Steps 1–3, 6 (transport).

**Inputs:** PRD-06 § 3 (PCA tiers), PRD-06 § 7 (transport reliability).

**Exact output artifacts:**
- `polkagent-transport-pca/src/` — full PCA protocol: C0 (connect), C1 (bridge), C2 (JS codec), C3 (Statement Store + Bulletin)
- App-layer ACK + sequence number + idempotent processing
- `/polkagent/tests/pca_integration/` — integration test against PCA test environment

**Verification:** bot registers, exchanges 1000 messages, zero duplicates, zero missed with simulated drops.

**Rollback:** degrade to C0; PCA messages still deliver but without bridge features.

#### Step 10: Marketplace — Wasmtime WIT sandbox + cosign/SLSA Build L2

**Prerequisites:** Steps 1–3, 6.

**Inputs:** PRD-12 § 7 (sandboxing), PRD-12 § 9 (supply chain).

**Exact output artifacts:**
- `polkagent-skill/src/sandbox.rs` — Wasmtime Component Model executor with WIT interface
- `polkagent-marketplace/src/verify.rs` — cosign signature verification + SLSA provenance check
- Self-hosted registry: Docker Compose spec in `/polkagent/deploy/registry/`

**Verification:** install a malicious skill that attempts file-system access; Wasmtime capability model blocks it.

**Rollback:** disable marketplace feature flag; built-in skills still function.

#### Step 11: Managed cloud — data/control-plane split, multi-tenant, billing

**Prerequisites:** Steps 1–10.

**Inputs:** PRD-11 § 5 (three-plane), PRD-11 § 10 (billing).

**Exact output artifacts:**
- `polkagent-cloud-control/src/` — control plane: tenant CRUD, quota enforcement, config distribution
- `polkagent-cloud-worker/src/` — worker: job polling, run execution, health heartbeat
- `polkagent-billing/src/` — metering: per-run cost, token attribution, audit export
- `polkagent-store-postgres/src/` — Postgres adapter for cloud deployments
- Helm chart: `/polkagent/deploy/helm/`

**Verification:** deploy 3-worker fleet; kill 1 worker; verify jobs reschedule within 30s.

**Rollback:** switch config from `cloud` to `local` mode; single-node continues operating.

#### Step 12: Trigger-gated — JAM/CorePlay, x402 non-EVM, MPC signing

**Prerequisites:** Steps 1–11. JAM testnet operational. x402 non-EVM spec finalized.

**Inputs:** PRD-05 § 12 (JAM), PRD-08 § JAM x402 section.

**Exact output artifacts:** experimental crates behind feature flags. Explicit maturity labels.

**Verification:** primary implementation evidence exists per experiment; core flows unaffected when disabled.

**Rollback:** disable feature flags; no core system dependency.

---

## APPENDIX B: AGENT IMPLEMENTATION CHECKLIST

This checklist is designed for an AI agent implementing Polkagent from scratch.
Each item is: `[ ] Task — acceptance criteria — estimated complexity`.

### B.0 Pre-Implementation Setup

- [ ] Install Rust toolchain 1.85+ via rustup — `rustup show` reports `1.85.x` or later — S
- [ ] Install cargo-nextest — `cargo nextest --version` succeeds — S
- [ ] Install cargo-fuzz — `cargo fuzz --version` succeeds — S
- [ ] Install cargo-dist for release builds — `cargo dist --version` succeeds — S
- [ ] Create GitHub repository with branch protection on `main` — PR required for all changes; direct push disabled — S
- [ ] Configure CI workflow — `.github/workflows/ci.yml` runs fmt + clippy + nextest on every PR — S
- [ ] Write `SECURITY.md` — private disclosure email, response SLA defined — S
- [ ] Write `docs/adr/ADR-001-cedar-policy-engine.md` — Cedar selected; alternatives documented — S
- [ ] Write `docs/adr/ADR-002-sqlite-wal-authority-store.md` — single-writer SQLite WAL; Temporal rejected — S
- [ ] Write `docs/adr/ADR-003-wasmtime-wit-sandbox.md` — Wasmtime Component Model; Extism rejected — S

### B.1 Phase 1: Safety Kernel

#### B.1.1 Workspace Bootstrap
- [ ] Create `Cargo.toml` workspace root with all Phase 1 crates listed in members — `cargo build --workspace` green — S
- [ ] Create `crates/polkagent-core/src/lib.rs` with `RunId`, `AgentId`, `TurnId`, `StepId`, `EffectId`, `ArtifactId` newtypes — all are `Copy`, `Eq`, `Hash`, `Serialize`, `Deserialize` — S
- [ ] Add `RunState` enum to `polkagent-core` with all states from PRD-03 § 3.2 — proptest: `RunState::all()` is exhaustive — M
- [ ] Add `EffectIntent`, `EffectAttempt`, `EffectOutcome` types — `EffectOutcome::Unknown` is a distinct variant, not collapsed — S
- [ ] Add `Grant`, `Capability`, `Policy` types — `Grant` holds no key material, only capability set + expiry — S
- [ ] Add `Artifact`, `ArtifactId`, `ArtifactType` types — `Artifact` contains BLAKE3 content hash field — S
- [ ] Add `Event`, `EventId`, `EventKind` types — `Event` has monotonic sequence number — S

#### B.1.2 Port Traits
- [ ] Create `polkagent-store-trait`: `StorePort` trait with async CRUD for all Phase 1 entity types — trait compiles; zero concrete implementations required — S
- [ ] Create `polkagent-executor-trait`: `ExecutorPort` trait with `stream_inference()`, `call_tool()`, `token_usage()` — trait compiles — S
- [ ] Create `polkagent-signer-trait`: `SignerPort` trait with `sign(payload: &[u8]) -> KeyHandle`; no raw key return — `sign()` return type cannot be `Vec<u8>` containing raw key — S
- [ ] Create `polkagent-transport-trait`: `TransportPort` trait with `send()`, `receive()`, `ack()` — trait compiles — S
- [ ] Create `polkagent-chain-trait`: `ChainPort` trait with `decode_extrinsic()`, `dry_run()`, `submit()`, `observe_finality()` — trait compiles — S

#### B.1.3 Run State Machine
- [ ] Implement `polkagent-run`: `RunStateMachine` with guard functions for each transition — proptest: 1000 random transition sequences; no invalid state reached — L
- [ ] Implement turn lifecycle within a run: `Turn` records model, token usage, tool calls — `Turn` persistence is atomic with parent run update — M
- [ ] Implement step recording: `Step` logged with timestamp and outcome — step count matches turn record — S
- [ ] Implement run persistence: `RunRepository` using `StorePort` — crash during persist → run state recoverable on restart — M

#### B.1.4 Effect Pipeline
- [ ] Implement `polkagent-effect`: `EffectPipeline` that writes `EffectIntent` in same transaction as state update — fault injection: kill at T+0ms; intent recoverable on restart — L
- [ ] Implement `EffectAttempt` lifecycle: `claim()`, `lease(duration)`, `retry()`, `complete()` — lease expiry → attempt automatically requeued — M
- [ ] Implement `EffectOutcome` immutability: `seal()` transitions to terminal state, no further mutations — proptest: `seal()` result is idempotent and immutable — S
- [ ] Implement idempotency key generation and checking — duplicate attempt with same idempotency key returns existing outcome — M
- [ ] Implement crash recovery: on startup, scan for incomplete attempts; requeue expired leases — kill during I/O at 5 checkpoints; no duplicate on restart — L

#### B.1.5 Durable Outbox
- [ ] Implement `polkagent-outbox`: ordered queue backed by SQLite — messages ordered by sequence number; no gaps — M
- [ ] Implement at-least-once delivery: poll + acknowledge — consumer dedup via idempotency key; inject 100 duplicates; consumer counts exactly-once — M
- [ ] Implement delivery acknowledgement: `ack()` marks message consumed — un-acked messages redelivered after TTL — S

#### B.1.6 Cedar Grant Engine
- [ ] Implement `polkagent-grant`: `CedarGrantEngine` with `cedar-policy` crate — Cedar evaluates principal/action/resource/context — L
- [ ] Implement capability matching: `Grant::has_capability(cap)` — new agent without explicit grant has zero capabilities; `has_capability` returns false — M
- [ ] Implement time-bounded grants: `Grant::is_expired()` — expired grant returns false for all capabilities — S
- [ ] Implement `KeyHandle` newtype: wraps key identifier only; no raw bytes accessible — red-team: no API surface returns `Vec<u8>` containing key material — M
- [ ] Write default deny-all Cedar policy: `policies/default_deny.cedar` — new principal with no policy: all `Permit` evaluations return `Deny` — S

#### B.1.7 Artifact and Event
- [ ] Implement `polkagent-artifact`: `Artifact::new(content: &[u8])` computes BLAKE3 hash — two identical content buffers produce identical `ArtifactId` — S
- [ ] Implement lineage tracking: `Artifact::with_parent(parent_id)` — chain of 10 artifacts; lineage graph traversal correct — M
- [ ] Implement `polkagent-event`: `EventBus` using tokio broadcast channel — 1000 events; all subscribers receive all events in order — M
- [ ] Implement event persistence: `EventRepository` writes to `StorePort` — events survives process restart — S

#### B.1.8 SQLite Adapter
- [ ] Implement `polkagent-store-sqlite`: `SqliteStore` with WAL mode, single-writer Tokio task — WAL mode confirmed via `PRAGMA journal_mode` — M
- [ ] Implement `BEGIN IMMEDIATE` for all write transactions — concurrent writer test: second writer blocks, then succeeds (no deadlock) — M
- [ ] Set `busy_timeout = 5000` — writer blocked for 4.9s; no timeout error; blocked 5.1s → timeout error — S
- [ ] Implement schema creation: idempotent `CREATE TABLE IF NOT EXISTS` for all entities — `SqliteStore::new()` called twice; no duplicate-table error — S
- [ ] Implement WAL checkpoint tuning: `PRAGMA wal_autocheckpoint = 1000` — reader starvation test: 100 readers, 1 writer; no reader starves — M

#### B.1.9 Fake Adapters
- [ ] Implement `polkagent-executor-fake`: scripted response queue, deterministic tool call responses — two test runs with identical scripts produce identical results — S
- [ ] Implement `polkagent-signer-fake`: accepts all payloads, returns deterministic signature — `sign()` is idempotent for same payload — S
- [ ] Implement `polkagent-transport-fake`: in-memory bounded channel — `send()` then `receive()` returns same message — S
- [ ] Write port contract tests for all fake adapters — fake adapters pass ALL port trait contract tests — M

#### B.1.10 Config Loading
- [ ] Implement `polkagent-config`: `Config::from_file(path)` parses TOML — malformed TOML returns structured error with line number — S
- [ ] Implement schema validation: required fields, type checks, enum values — fuzz test: 10,000 random configs; all invalid configs return error, no panics — M
- [ ] Implement env-var overrides: `POLKAGENT_LOG_LEVEL` overrides `config.log_level` — env var takes precedence over file value — S
- [ ] Implement default values: `Config::default()` returns valid config for local development — `Config::default()` + `cargo run` starts without error — S

#### B.1.11 Phase 1 Tests
- [ ] Write state machine property tests with proptest — 10,000 random transition sequences; zero panics, no unreachable states — M
- [ ] Write crash recovery fault injection tests — kill at 5 checkpoints; restart; verify no duplicate effects — L
- [ ] Write idempotency property tests — 1,000 duplicate effect attempts; exactly-one outcome per idempotency key — M
- [ ] Write grant denial tests — 50 capability combinations; undeclared caps always denied — M
- [ ] Write port contract test suites for all port traits — all fake adapters pass; trait-level invariants documented — M

### B.2 Phase 2: Build + Act + Reach Proof

- [ ] Implement subxt static decode for Polkadot relay chain — decode `balances.transfer` from Polkadot; all fields correct — L
- [ ] Implement subxt dynamic decode path — decode any pallet without pre-generated types using runtime metadata — L
- [ ] Implement CheckMetadataHash enforcement — stale metadata → `MetadataHashMismatch` error, not wrong decode — M
- [ ] Add fixture file `/polkagent/fixtures/extrinsics/polkadot-balances-transfer.hex` and 49 others — 50 known extrinsics decode correctly — M
- [ ] Implement `polkagent-metadata`: `MetadataService::pin(block_hash)` stores metadata snapshot — pinned metadata survives restart — M
- [ ] Implement DryRunApi integration — `dry_run(extrinsic)` returns fee estimate + simulation result without submitting — L
- [ ] Implement XcmPaymentApi integration — `xcm_fee(route)` returns fee estimate for XCM transfer — M
- [ ] Implement `polkagent-executor-anthropic` with streaming — token stream received; usage stats accurate to within 1% — M
- [ ] Implement tool call round-trip in Anthropic executor — tool call → tool result → final response; no message lost — M
- [ ] Implement full Explain Before Sign pipeline — card bytes byte-identical to signer payload; model text in separate rendered section — XL
- [ ] Implement `polkagent-card` risk indicator logic — `batch` extrinsics show `HIGH_RISK`; simple transfer shows `LOW_RISK` — M
- [ ] Implement `polkagent-signer-external` browser extension protocol — payload received by extension = payload computed from action card — M
- [ ] Implement `polkagent-cli` with `polkagent explain`, `polkagent sign`, `polkagent agent`, `polkagent run`, `polkagent config` — all commands show help without panic — M
- [ ] Implement interactive approval flow in CLI — `polkagent explain <tx>` shows action card; user types `approve`; signed tx submitted — M
- [ ] Implement `polkagent-api` REST endpoints for agents, runs, effects — `GET /v1/agents`, `POST /v1/runs`, `GET /v1/effects` return correct data — L
- [ ] Implement WebSocket event streaming in API — connect WS; run agent; receive all events in order — L
- [ ] Implement PCA C0 transport: connect, register, send/receive/ACK — test bot registers with PCA; exchanges 10 messages — M

### B.3 Phase 3: Read-Only Value Expansion

- [ ] Implement storage migration rehearsal (A1) — fork + apply + diff produces no regression output for known-good migration — L
- [ ] Implement upgrade impact brief (A2) — compare old/new metadata; list changed pallets/calls — M
- [ ] Implement metadata-grounded RAG (A9) — every answer cites metadata hash, block number, and pallet source — L
- [ ] Implement OpenGov referendum reader (B2) — fetch referendum details; model answer cites on-chain data — M
- [ ] Implement live run timeline with SSE reconnect (F3) — kill server during stream; client reconnects; timeline intact — M
- [ ] Implement error/recovery explainer (F7) — failed run → model produces next-step proposals citing error type — M
- [ ] Implement People Chain identity lookup (I2) — lookup known identity; display judgement status — M
- [ ] Complete PCA C0 compatibility — full ACK semantics; bot lifecycle (connect/disconnect/reconnect) — M
- [ ] Implement OpenAI-compatible executor — same interface as Anthropic; `gpt-4o` calls succeed — M
- [ ] Implement local ollama executor — `ollama pull llama3.2`; executor calls it; response returned — M
- [ ] Implement model fallback chain — primary fails; fallback activates within 500ms — M

### B.4 Phase 4: Controlled Write Expansion

- [ ] Implement pre-sign risk gate engine — seeded dangerous-call corpus: 50 fixtures; all flagged before signing — L
- [ ] Implement multisig/proxy state reader — read multisig threshold and current approvals from chain — M
- [ ] Implement pure-proxy coordinator — agent proposes extrinsic via pure-proxy; coordinator tracks approvals without holding keys — XL
- [ ] Implement XCM route planner (testnet) — known routes produce fee estimates; unknown routes return `RouteUnsupported` — XL
- [ ] Implement Claude CLI harness — spawn Claude CLI subprocess; attach stdin/stdout; session resumes after kill — L
- [ ] Implement Codex harness — same pattern as Claude harness — M
- [ ] Implement `polkagent-memory` with sqlite-vec+FTS5+RRF — hybrid search returns relevant results; RRF scores blend correctly — XL
- [ ] Implement tenant isolation in memory — agent A cannot query agent B's memories; isolation tested with 100 cross-tenant queries — M
- [ ] Implement forget/export controls — `forget(artifact_id)` removes from vector and FTS5 index; `export()` produces portable archive — M
- [ ] Implement product kit manifest format — TOML kit manifest with `[kit.metadata]`, `[kit.capabilities]`, `[kit.skills]` — M
- [ ] Implement kit install/uninstall workflow — install skill kit; skill available in tool registry; uninstall removes it — M
- [ ] Implement PCA C1 bridge protocol — bridge protocol message exchange succeeds in integration test — M

### B.5 Phase 5: Managed, Public, Value-Moving

- [ ] Implement funded pure-proxy accounts with Cedar budget enforcement — adversarial: agent cannot exceed 10 DOT budget; Cedar policy rejects — XL
- [ ] Implement agent earn/spend prototype — agent completes skill task; receives micropayment; spends on tool call — XL
- [ ] Implement Wasmtime WIT sandbox for skills — malicious skill attempting `fs::read("/etc/passwd")` → capability denied — XL
- [ ] Implement cosign/SLSA Build L2 verification — unsigned package install fails; correctly signed package installs — L
- [ ] Implement permissionless registry — publish skill to self-hosted registry without central approval — M
- [ ] Implement control/data/worker plane split — control plane URL in config; workers register; control plane assigns jobs — XL
- [ ] Implement multi-tenant SQLite isolation (local) or Postgres (cloud) — cross-tenant query returns empty; not cross-tenant data — XL
- [ ] Implement billing metering — per-run cost computed; audit export CSV matches run logs exactly — L
- [ ] Commission external penetration test — external report received; critical findings addressed before go-live — M (process)
- [ ] Complete legal review for value-moving features — sign-off document covering money-transmission, OFAC, tax reporting — M (process)

### B.6 Phase 6: Experimental Frontier

- [ ] Set up JAM testnet connection — JAM testnet RPC accessible; basic block queries succeed — XL
- [ ] Prototype CorePlay service (D1) — service deploys; basic execution evidence — XL
- [ ] Implement personhood gating with feature flag — W3C DID credential checked; personhood-gated tool blocked without credential — L
- [ ] Implement affect/vitality module (isolated) — module compiles; does not affect run lifecycle unless feature flag enabled — M
- [ ] Implement evolutionary skill selection (G5) — eval harness runs; skill variants selected by performance — XL
- [ ] Implement always-on watcher (C4) — watcher runs independently; no write authority without separate Cedar grant — L

### B.7 TUI Implementation Checklist

- [ ] Set up ratatui app skeleton in `polkagent-cli/src/tui/` — `polkagent dashboard` opens and renders without panic — M
- [ ] Implement tab bar with F1–F7 shortcuts — all tabs switch correctly; active tab highlighted — S
- [ ] Implement dashboard/home screen — current agent count, active runs, effect queue depth — M
- [ ] Implement agents view — list agents with status badges; select agent for detail — M
- [ ] Implement runs view — timeline of recent runs; select for step-by-step detail — M
- [ ] Implement effects view with approval modal — pending effects listed; approval/deny keyboard controls — L
- [ ] Implement action card modal — card fields rendered; model text in separate section below separator — M
- [ ] Implement config view — read-only display of active configuration — S
- [ ] Implement logs view with JSONL tailing — live log stream; search/filter — M
- [ ] Implement memory/knowledge view — query interface; result list with provenance metadata — M
- [ ] Implement keyboard navigation throughout — all screens navigable without mouse — M
- [ ] Implement help modal (F1 or `?`) — all keyboard shortcuts listed — S
- [ ] Implement quit modal with confirmation — `q` → confirm; `Esc` cancels — S

---

## APPENDIX C: REFERENCE FILE MAP

Map every PRD to specific reference files in Roko and Bardo that provide proven
implementation patterns.

| PRD | Roko Reference Files | Bardo Reference Files | Key Patterns to Adopt |
|---|---|---|---|
| **PRD-02 Architecture** | `/Users/will/dev/nunchi/roko/roko/Cargo.toml` — workspace layout; `crates/roko-core/src/lib.rs` — kernel types | `/Users/will/dev/uniswap/bardo/Cargo.toml` — layered architecture (Layer 0–7); `crates/golem-core/` — primitive types | Strict layering; no cross-talk between adapters; port trait pattern |
| **PRD-03 Execution Model** | `crates/roko-cli/src/runner/event_loop.rs` — plan-execute-gate loop; `crates/roko-orchestrator/src/` — DAG executor; `crates/roko-fs/src/` — JSONL signal persistence | `apps/mori/src/orchestrator/` — orchestration; `crates/golem-core/` — execution primitives | Event-sourcing loop; single-writer SQLite; crash-safe state machine |
| **PRD-04 Providers/Tools** | `crates/roko-agent/src/dispatcher/mod.rs` — 5+ LLM backends; `crates/roko-mcp-stdio/` and `crates/roko-mcp-code/` — MCP servers; `crates/roko-std/src/` — 19 builtin tools | `crates/golem-inference/` — inference protocol; `apps/bardo-terminal/src/` (agent communication) | Multi-backend dispatch; MCP Streamable HTTP server; tool capability checking |
| **PRD-05 Polkadot** | `crates/roko-chain/src/` — chain witness primitives (Phase 2+); `crates/roko-compose/src/system_prompt_builder.rs` — context enrichment for chain data | `crates/golem-chain/` — chain integration; `crates/golem-chain-intelligence/` — chain analysis; `crates/golem-uniswap/` — DEX patterns (analogous to Asset Hub patterns) | Chain adapter pattern; metadata pinning; DryRunApi integration |
| **PRD-06 PCA/Messaging** | `crates/roko-acp/src/` — ACP server for editor integrations; `crates/roko-mcp-slack/` — messaging integration | `crates/golem-engagement/` — engagement/messaging; `apps/bardo-terminal/src/screens/hearth/` — chat surface | Transport port trait; ACK semantics; reconnect logic |
| **PRD-07 Identity/Security** | `crates/roko-agent/src/safety/` — safety layer integrated into tool dispatcher; `crates/roko-core/src/` — contract types | `crates/golem-safety/` — safety layer; `crates/golem-identity/` — identity primitives; `crates/mpp/` — payment primitives | Keys-outside-model pattern; Cedar grant evaluation; contract test suites |
| **PRD-08 Payments** | `crates/roko-chain/src/` — chain wallet trait abstractions; `crates/roko-orchestrator/src/` — safety integration | `crates/golem-economy/` — economic model; `crates/golem-mortality/` — lifecycle + resource bounds; `crates/mpp/` — Machine Payment Protocol | DryRunApi pre-flight; hard budget enforcement in grant layer; pure-proxy pattern |
| **PRD-09 Memory/Groups** | `crates/roko-neuro/src/` — durable knowledge store, distillation, tier progression; `crates/roko-learn/src/` — episodes, playbooks, bandits; `crates/roko-dreams/src/` — offline consolidation | `crates/golem-grimoire/` — knowledge store (tantivy BM25); `crates/golem-dreams/` — dream consolidation; `crates/golem-context/` — context management | sqlite-vec + FTS5 + RRF pattern; episodic log; tenant isolation; forget/export |
| **PRD-10 Data/Observability** | `crates/roko-fs/src/` — FileSubstrate JSONL; `crates/roko-primitives/src/` — HDC vectors, content hashing; `.roko/episodes.jsonl` — episode log | `crates/bardo-primitives/src/` — primitives; `crates/golem-ta/` — data analysis; OpenTelemetry setup in `apps/bardo-gateway/` | BLAKE3 content addressing; JSONL append-only log; OTel span hierarchy |
| **PRD-11 Deployment** | `crates/roko-serve/src/routes/` — ~85 HTTP routes; `crates/roko-agent-server/` — per-agent sidecar; `crates/roko-runtime/src/` — ProcessSupervisor | `apps/bardo-gateway/` — gateway service; `apps/bardo-compute/` — compute workers; `apps/bardo-styx/` — styx service | Three-plane architecture; worker registration; ProcessSupervisor pattern |
| **PRD-12 Marketplace** | `crates/roko-plugin/src/` — plugin SDK (EventSource, FeedbackCollector); `crates/roko-mcp-github/` and `crates/roko-mcp-scripts/` — MCP integrations | `crates/golem-tools/` — tool system; `crates/golem-surfaces/` — surface abstractions | Wasmtime WIT sandbox; cosign verification; manifest format |
| **PRD-13 UX/Surfaces** | `crates/roko-cli/src/tui/` — full ratatui TUI (80 files); `crates/roko-cli/src/tui/pages/` — page system; `crates/roko-cli/src/tui/widgets/` — ROSEDUST widget system; `crates/roko-cli/src/tui/modals/` — modal dialogs | `apps/bardo-terminal/src/` — full screen TUI; `apps/bardo-terminal/src/design/` — design system (palette, tokens, transitions); `apps/mori/src/tui/views/` — 13 view modules; `apps/mori/src/tui/widgets/` — widget library | ratatui app/event loop; tab navigation; approval modal pattern; design tokens |
| **PRD-14 APIs/Config** | `crates/roko-serve/src/routes/` — REST route organization; `crates/roko-cli/src/` — TOML config loading | `apps/bardo-gateway/` — API gateway; `crates/golem-api/` — API surface | Config hierarchy; route organization; schema migration |
| **PRD-15 Testing** | `tests/` — E2E integration tests; `crates/roko-gate/src/` — 7-rung gate pipeline; `.roko/learn/` — adaptive gate thresholds | `tests/harness/` — test harness; `crates/golem-eval/` — MVP gate | proptest patterns; fault injection; port contract tests |

### C.1 Specific High-Value Reference Files

The following individual files are particularly worth studying:

| File | Why study it |
|---|---|
| `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/runner/event_loop.rs` | The canonical plan-execute-gate-persist loop; directly maps to PRD-03 execution model |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-agent/src/dispatcher/mod.rs` | Multi-backend LLM dispatch with 5+ backends; maps to PRD-04 provider model |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-agent/src/safety/` | Safety layer integration with tool dispatcher; maps to PRD-07 key isolation |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-compose/src/system_prompt_builder.rs` | 9-layer prompt assembly; maps to PRD-04 context composition |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/app.rs` | ratatui app event loop; maps to PRD-13 TUI architecture |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/tabs.rs` | Tab navigation system; maps to PRD-13 tab structure |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/rosedust.rs` | ROSEDUST design system; analogous to Polkagent design tokens |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/modals/approval.rs` | Approval modal pattern; maps to PRD-08 human-approval UX |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-gate/src/` | 7-rung gate pipeline; maps to PRD-15 CI gates |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-learn/src/` | Episodic learning, playbooks, bandits; maps to PRD-09 memory |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-neuro/src/` | Durable knowledge store; maps to PRD-09 semantic memory |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-fs/src/` | JSONL substrate; maps to PRD-10 event persistence |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-orchestrator/src/` | DAG executor, merge queue, parallel execution; maps to PRD-03 graph execution |
| `/Users/will/dev/nunchi/roko/roko/crates/roko-runtime/src/` | ProcessSupervisor; maps to PRD-11 worker lifecycle |
| `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/design/palette.rs` | Color/token design system; maps to PRD-13 visual design |
| `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/design/tokens.rs` | Design token definitions; maps to PRD-13 design system |
| `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/design/transitions.rs` | TUI transition animations; maps to PRD-13 UX polish |
| `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/soma/` | Vitality/health screens; maps to PRD-13 agent status display |
| `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/hearth/` — `portfolio.rs`, `activity.rs` | Portfolio and activity screens; maps to PRD-08 treasury/payment UX |
| `/Users/will/dev/uniswap/bardo/apps/mori/src/tui/views/dashboard.rs` | Dashboard layout; maps to PRD-13 home screen |
| `/Users/will/dev/uniswap/bardo/apps/mori/src/tui/views/agents.rs` | Agent list view; maps to PRD-13 agent management screen |
| `/Users/will/dev/uniswap/bardo/apps/mori/src/tui/modals/approval.rs` | Approval modal; maps to PRD-08 human-approval workflow |
| `/Users/will/dev/uniswap/bardo/crates/golem-safety/` | Safety primitives; maps to PRD-07 security invariants |
| `/Users/will/dev/uniswap/bardo/crates/golem-economy/` | Economic model; maps to PRD-08 funded agents |
| `/Users/will/dev/uniswap/bardo/crates/golem-chain/` | Chain adapter pattern; maps to PRD-05 subxt integration |
| `/Users/will/dev/uniswap/bardo/crates/mpp/` | Machine Payment Protocol; maps to PRD-08 payment primitives |
| `/Users/will/dev/uniswap/bardo/tests/harness/` | Integration test harness; maps to PRD-15 test infrastructure |

---

## APPENDIX D: CONFIGURATION AND RUNNING GUIDE

### D.1 Developer Setup

**Step 1: Install Rust toolchain**

```bash
# Install rustup if not present
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install the required stable toolchain (1.85+)
rustup toolchain install stable
rustup default stable

# Verify
rustc --version   # must be 1.85.x or later
cargo --version
```

**Step 2: Install required development tools**

```bash
# Faster test runner (strongly recommended)
cargo install cargo-nextest --locked

# Fuzzing support (Phase 1+ tests)
cargo install cargo-fuzz

# Dependency graph inspection
cargo install cargo-tree

# Audit crates for known vulnerabilities
cargo install cargo-audit

# Binary release tooling (CI only, optional for local dev)
cargo install cargo-dist --locked

# Optional: flamegraph profiling
cargo install flamegraph
```

**Step 3: Install system dependencies**

```bash
# macOS (Apple Silicon / Intel)
brew install openssl pkg-config

# Linux (Debian/Ubuntu)
sudo apt-get install -y libssl-dev pkg-config build-essential

# Ledger support (optional, Phase 2+)
# macOS: brew install libudev-dev
# Linux: sudo apt-get install libudev-dev
```

**Step 4: Clone and build**

```bash
git clone https://github.com/paritytech/polkagent
cd polkagent

# First build (expect 5–10 minutes on first run; subsequent builds are incremental)
cargo build --workspace

# Run the test suite
cargo nextest run --workspace

# Lint check
cargo clippy --workspace --no-deps -- -D warnings

# Format check
cargo fmt --all -- --check
```

**Step 5: Initialize local configuration**

```bash
# Initialize .polkagent/ directory and default polkagent.toml
cargo run -p polkagent-cli -- init

# Verify configuration
cargo run -p polkagent-cli -- config show

# Run diagnostics
cargo run -p polkagent-cli -- doctor
```

**Step 6: Set required environment variables**

```bash
# Copy the example env file
cp .env.example .env

# Edit .env and populate:
# POLKAGENT_ANTHROPIC_API_KEY=sk-ant-...
# POLKAGENT_RPC_URL=wss://rpc.polkadot.io  (or Westend for dev)
# POLKAGENT_LOG_LEVEL=info

# Load env vars for development
source .env
```

**Step 7: First run**

```bash
# Start the API server
cargo run -p polkagent-cli -- serve &

# Open the TUI dashboard
cargo run -p polkagent-cli -- dashboard

# In another terminal: run a simple agent task
cargo run -p polkagent-cli -- run "Explain the current Polkadot relay chain block production rate"

# Or explain an extrinsic
cargo run -p polkagent-cli -- explain --tx 0x...hex...
```

---

### D.2 Configuration Hierarchy

Configuration is resolved in this priority order (highest wins):

```
1. CLI flags          (--rpc-url wss://...)
2. Environment vars   (POLKAGENT_RPC_URL=...)
3. Local config file  (.polkagent/polkagent.toml)
4. Global config file (~/.config/polkagent/polkagent.toml)
5. Built-in defaults  (compiled into polkagent-config)
```

**Data directory structure** (modeled on Roko's `.roko/` directory):

```
.polkagent/
├── polkagent.toml          # Local configuration
├── state/
│   ├── runs.db             # SQLite WAL database (single-writer)
│   └── executor.json       # Executor snapshot for resume
├── artifacts/              # Content-addressed artifact storage
│   └── <blake3-hash>.bin
├── episodes.jsonl          # Episodic log (append-only)
├── signals.jsonl           # Signal log (append-only)
├── prd/                    # PRD storage (if using self-hosting)
├── learn/
│   ├── episodes.jsonl      # Learning episodes
│   ├── cascade-router.json # Model routing state
│   └── gate-thresholds.json# Adaptive gate thresholds
├── memory/
│   ├── vec.db              # sqlite-vec vector index
│   ├── fts5.db             # Full-text search index
│   └── episodic.jsonl      # Episodic memory log
└── keys/                   # Key handles only (no raw keys)
    └── handles.json
```

**Configuration file schema** (TOML):

```toml
# polkagent.toml

[polkagent]
version = "1"
data_dir = ".polkagent"  # overridable with POLKAGENT_DATA_DIR

[log]
level = "info"           # trace | debug | info | warn | error
format = "pretty"        # pretty | json
otlp_endpoint = ""       # empty = no OTel export

[chain]
rpc_url = "wss://rpc.polkadot.io"
metadata_cache_dir = ".polkagent/metadata"
check_metadata_hash = true       # enforce CheckMetadataHash; default true

[executor]
default = "anthropic"
fallback = ["openai", "local"]

[executor.anthropic]
model = "claude-opus-4-6"
max_tokens = 8192

[executor.openai]
base_url = "https://api.openai.com/v1"
model = "gpt-4o"

[executor.local]
base_url = "http://localhost:11434"
model = "llama3.2"

[store]
# "sqlite" for local dev; "postgres" for managed cloud
backend = "sqlite"
sqlite.path = ".polkagent/state/runs.db"
sqlite.wal_autocheckpoint = 1000
sqlite.busy_timeout_ms = 5000

[store.postgres]  # Phase 5, managed cloud only
url = "postgresql://..."

[policy]
# Cedar policy files loaded at startup
policy_dir = ".polkagent/policies"
default = "deny-all"

[memory]
enabled = true
backend = "sqlite-vec"
vec_db_path = ".polkagent/memory/vec.db"
fts5_db_path = ".polkagent/memory/fts5.db"

[api]
enabled = true
bind = "127.0.0.1:6688"
cors_origins = ["http://localhost:3000"]

[mcp]
enabled = true
bind = "127.0.0.1:6689"
transport = "streamable-http"  # streamable-http | stdio
```

**Environment variable overrides** (full list):

| Variable | Overrides | Example |
|---|---|---|
| `POLKAGENT_LOG_LEVEL` | `log.level` | `debug` |
| `POLKAGENT_DATA_DIR` | `polkagent.data_dir` | `/var/lib/polkagent` |
| `POLKAGENT_RPC_URL` | `chain.rpc_url` | `wss://westend-rpc.polkadot.io` |
| `POLKAGENT_ANTHROPIC_API_KEY` | secret, not in config | `sk-ant-...` |
| `POLKAGENT_OPENAI_API_KEY` | secret, not in config | `sk-...` |
| `POLKAGENT_STORE_BACKEND` | `store.backend` | `postgres` |
| `POLKAGENT_POSTGRES_URL` | `store.postgres.url` | `postgresql://...` |
| `POLKAGENT_API_BIND` | `api.bind` | `0.0.0.0:6688` |
| `POLKAGENT_DISABLE_MEMORY` | `memory.enabled` | `true` |

**Secrets management:** API keys are NEVER written to config files. They are
read from environment variables only. In production, use a secrets manager:

```bash
# Development: .env file (never commit)
POLKAGENT_ANTHROPIC_API_KEY=sk-ant-...

# Production: environment from secrets manager
# AWS Secrets Manager, HashiCorp Vault, or equivalent
```

---

### D.3 Running Modes

#### Local Development Mode

```bash
# Uses SQLite, local executors, fake signer
cargo run -p polkagent-cli -- serve
# Config auto-detects: store.backend=sqlite, executor.default=anthropic
# No cloud connectivity required
```

#### Testnet Mode (Westend)

```bash
POLKAGENT_RPC_URL=wss://westend-rpc.polkadot.io \
cargo run -p polkagent-cli -- serve
# All chain operations use Westend; no real value at risk
```

#### Local Testing (Fake Everything)

```bash
# Fully deterministic: no network calls, no LLM API costs
POLKAGENT_EXECUTOR_DEFAULT=fake \
POLKAGENT_SIGNER_DEFAULT=fake \
POLKAGENT_CHAIN_DEFAULT=fake \
cargo nextest run --workspace
```

#### Production / Managed Cloud Mode (Phase 5)

```bash
# Switch to PostgreSQL and cloud control plane
POLKAGENT_STORE_BACKEND=postgres \
POLKAGENT_POSTGRES_URL=postgresql://... \
POLKAGENT_CONTROL_PLANE_URL=https://control.polkagent.io \
cargo run -p polkagent-daemon -- start
```

#### SQLite vs PostgreSQL Decision Guide

| Scenario | Use SQLite | Use PostgreSQL |
|---|---|---|
| Single developer, local dev | Yes | No |
| Single-node self-hosted production | Yes | Optional |
| Multi-worker fleet (Phase 5) | No | Yes |
| Managed cloud (Phase 5) | No | Yes |
| CI/CD test suite | Yes (`:memory:`) | No |
| Offline/air-gapped | Yes | No |

**Important:** switching from SQLite to PostgreSQL requires a data migration.
Use `polkagent config migrate --from sqlite --to postgres` (Phase 5 tool) and
verify with the portability drill from AC-P5-005.

---

## APPENDIX E: CROSS-PRD DEPENDENCY MATRIX

This matrix shows which PRDs depend on which, with the specific type of dependency.
Dependency types: **Code** (crate-level import), **Config** (shared configuration
schema), **Schema** (shared database schema or API DTO), **API** (REST/WS endpoint
consumer), **Concept** (vocabulary or invariant defined in source PRD).

| PRD | Depends On | Dependency Type | Specific Dependency |
|---|---|---|---|
| PRD-01 | — | — | Foundation document; all PRDs inherit its vision |
| PRD-02 | PRD-01 | Concept | Vocabulary, principles, personas |
| PRD-03 | PRD-02 | Concept, Code | Architecture layers, port trait pattern; kernel crates |
| PRD-03 | PRD-07 | Code, Concept | `Grant` type used in effect pipeline authorization |
| PRD-03 | PRD-10 | Schema | Artifact, Event schemas defined in PRD-10 used by execution model |
| PRD-04 | PRD-02 | Concept | Architecture layer for adapters |
| PRD-04 | PRD-03 | Code, Concept | `Run`, `Turn`, `Step` lifecycle; executor called from run |
| PRD-04 | PRD-07 | Code | `Grant`/`Capability` checked before tool dispatch |
| PRD-05 | PRD-02 | Concept | Architecture; chain adapter layer |
| PRD-05 | PRD-03 | Code | `EffectIntent` for chain submission |
| PRD-05 | PRD-07 | Code | `SignerPort` for transaction signing |
| PRD-05 | PRD-08 | Code, Concept | DryRunApi and XcmPaymentApi feed action cards in PRD-08 |
| PRD-06 | PRD-02 | Concept, Code | Transport port trait |
| PRD-06 | PRD-03 | Code | Runs created when message received |
| PRD-06 | PRD-04 | Code | Executor (brain) invoked from transport handler |
| PRD-06 | PRD-14 | Schema | Message DTO and transport config schemas |
| PRD-07 | PRD-02 | Concept, Code | Architecture; port trait for signers |
| PRD-07 | PRD-03 | Code | `Grant` resolution run inside effect pipeline |
| PRD-07 | PRD-10 | Schema | Identity artifact types |
| PRD-08 | PRD-02 | Concept | Architecture |
| PRD-08 | PRD-03 | Code, Schema | `EffectIntent`/`EffectOutcome` for payment effects |
| PRD-08 | PRD-05 | Code | DryRunApi, XcmPaymentApi, subxt for chain operations |
| PRD-08 | PRD-07 | Code, Concept | `Grant`/`Capability` for payment authorization; `SignerPort` for signing |
| PRD-08 | PRD-10 | Schema | Payment receipt artifacts |
| PRD-09 | PRD-02 | Concept | Architecture |
| PRD-09 | PRD-03 | Code | Episodes written from run lifecycle events |
| PRD-09 | PRD-07 | Code | Grant intersection for multi-agent groups; tenant isolation |
| PRD-09 | PRD-10 | Schema, Code | Artifact provenance; memory items stored as artifacts |
| PRD-10 | PRD-02 | Concept, Schema | Architecture; canonical artifact types |
| PRD-10 | PRD-03 | Schema | Run, turn, step IDs referenced in events |
| PRD-10 | PRD-14 | Schema | Database schema defined in PRD-14 consumed by PRD-10 |
| PRD-11 | PRD-02 | Concept | Architecture |
| PRD-11 | PRD-03 | Code, Config | Worker runs execution engine; config for plane separation |
| PRD-11 | PRD-07 | Code | Tenant isolation uses grant model |
| PRD-11 | PRD-10 | Code, Schema | Data plane persists artifacts, events |
| PRD-11 | PRD-14 | Schema, Config | Deployment config schema; API versioning |
| PRD-12 | PRD-02 | Concept | Architecture |
| PRD-12 | PRD-04 | Code, Concept | Skills are installed into tool registry; harness may run skill code |
| PRD-12 | PRD-07 | Code, Concept | Publisher identity; capability disclosure in manifest |
| PRD-13 | PRD-01 | Concept | Personas, JTBDs drive surface design |
| PRD-13 | PRD-03 | API | Run timeline, effect status from REST API |
| PRD-13 | PRD-06 | Code, Concept | Chat surface is the PCA transport consumer |
| PRD-13 | PRD-08 | Code, Concept | Action card anatomy; approval flow |
| PRD-13 | PRD-14 | API | All TUI data comes from REST/WebSocket API |
| PRD-14 | PRD-02 | Concept, Schema | Architecture; vocabulary defines all type names |
| PRD-14 | PRD-03 | Schema | Run, turn, effect, artifact schemas |
| PRD-14 | PRD-04 | Schema | Provider, model, executor schemas |
| PRD-14 | PRD-07 | Schema | Identity, grant, capability schemas |
| PRD-14 | PRD-10 | Schema | Event, artifact schemas |
| PRD-14 | PRD-11 | Schema, Config | Tenant, worker schemas; deployment config |
| PRD-15 | ALL | Code, Concept | Acceptance criteria reference all other PRDs |

### E.1 Circular Dependency Check

The dependency matrix above has **no circular dependencies**. Verified by
topological sort:

```
Layer 0 (no deps): PRD-01, PRD-02
Layer 1: PRD-03, PRD-07, PRD-10
Layer 2: PRD-04, PRD-05, PRD-06, PRD-08, PRD-09, PRD-11
Layer 3: PRD-12, PRD-13, PRD-14
Layer 4: PRD-15 (references all)
```

---

## APPENDIX F: TUI/UX INTEGRATION MAP

This appendix maps each PRD's features to the specific TUI screens, views, and
widgets needed to surface them. Reference architecture: Roko's 29-screen ratatui
TUI (F1–F7 tabs, pages/, views/, widgets/, modals/) and Bardo's screen system
(bardo-terminal: screens/clade/, screens/soma/, screens/hearth/, screens/mind/;
mori: tui/views/, tui/widgets/, tui/modals/).

### F.1 TUI Screen Architecture

```
polkagent-cli/src/tui/
├── app.rs                  # Main ratatui App, event loop, state
├── tabs.rs                 # Tab bar (F1–F8)
├── layout.rs               # Layout calculations
├── theme.rs                # Color palette, ROSEDUST-equivalent design tokens
├── input.rs                # Keyboard/mouse event handling
├── state.rs                # Global TUI state (active tab, selection, modals)
├── event.rs                # TUI event types
├── ws_client.rs            # WebSocket client for live data from API
├── pages/
│   ├── mod.rs
│   ├── dashboard.rs        # F1: Home dashboard (agent count, run queue, effect depth)
│   └── operations.rs       # F6: System operations (metrics, logs)
├── views/
│   ├── mod.rs
│   ├── agents_view.rs      # F2: Agent list with status badges
│   ├── runs_view.rs        # F3: Run timeline, step detail
│   ├── effects_view.rs     # F4: Effect queue, pending approvals
│   ├── memory_view.rs      # F5: Memory/knowledge query interface
│   ├── config_view.rs      # F7: Active configuration (read-only)
│   ├── logs_view.rs        # F8: JSONL log tail with filter
│   └── marketplace_view.rs # F9: Marketplace browser (Phase 4+)
├── widgets/
│   ├── mod.rs
│   ├── status_badge.rs     # Agent/run status indicator
│   ├── action_card.rs      # Action card renderer (PRD-08 canonical/model separation)
│   ├── run_timeline.rs     # Run event timeline
│   ├── effect_list.rs      # Pending effects list
│   ├── token_gauge.rs      # Token usage gauge
│   ├── memory_result.rs    # Memory search result with provenance
│   ├── chain_info.rs       # Chain connection status, block
│   ├── tab_bar.rs          # Tab navigation bar
│   ├── status_bar.rs       # Bottom status bar
│   └── header_bar.rs       # Top header with agent name + chain
└── modals/
    ├── mod.rs
    ├── approval.rs         # Action approval/deny (PRD-08 human approval)
    ├── action_card.rs      # Full action card detail view
    ├── batch_review.rs     # Batch effect review
    ├── confirm.rs          # Generic confirmation dialog
    ├── help.rs             # Keyboard shortcut reference
    ├── quit.rs             # Quit confirmation
    ├── agent_detail.rs     # Agent detail panel
    ├── run_detail.rs       # Run step-by-step detail
    └── memory_detail.rs    # Memory item detail with provenance
```

### F.2 PRD-to-Screen Mapping

| PRD | TUI Screens Required | Widgets Required | Modals Required | Roko Reference | Bardo Reference |
|---|---|---|---|---|---|
| **PRD-01 Vision** | — | — | — | — | — |
| **PRD-02 Architecture** | All screens (architectural principles surface throughout) | `status_badge.rs` (truthful status display) | — | `crates/roko-cli/src/tui/state.rs` | `apps/mori/src/tui/views/dashboard.rs` |
| **PRD-03 Execution Model** | `views/runs_view.rs` (run timeline), `pages/dashboard.rs` (live queue depth) | `run_timeline.rs`, `token_gauge.rs`, `status_badge.rs` | `modals/run_detail.rs` (step-by-step) | `crates/roko-cli/src/tui/views/plans_view.rs`, `views/dashboard_view.rs` | `apps/mori/src/tui/views/plans.rs`, `tui/views/tasks.rs` |
| **PRD-04 Providers/Tools** | `pages/dashboard.rs` (active executor), `views/agents_view.rs` (model per agent) | `status_badge.rs` (executor health) | `modals/agent_detail.rs` (executor config) | `crates/roko-cli/src/tui/views/agents_view.rs` | `apps/mori/src/tui/views/agents.rs` |
| **PRD-05 Polkadot** | `widgets/chain_info.rs` (block/connection), `views/runs_view.rs` (chain effects) | `chain_info.rs`, `status_badge.rs` | `modals/action_card.rs` (extrinsic detail) | `crates/roko-cli/src/tui/views/context_view.rs` | `apps/bardo-terminal/src/screens/hearth/portfolio.rs` |
| **PRD-06 PCA/Messaging** | `views/logs_view.rs` (message log), future: `views/inbox_view.rs` | `status_badge.rs` (transport health) | — | `crates/roko-cli/src/tui/views/logs_view.rs` | `apps/bardo-terminal/src/screens/hearth/home.rs` |
| **PRD-07 Identity/Security** | `views/agents_view.rs` (identity display), future: `views/identity_view.rs` | `status_badge.rs` (grant status) | `modals/agent_detail.rs` (capabilities) | `crates/roko-cli/src/tui/views/config_view.rs` | `apps/bardo-terminal/src/screens/soma/` |
| **PRD-08 Payments/Autonomy** | `views/effects_view.rs` (pending payments), `modals/approval.rs` (CRITICAL: human approval) | `action_card.rs` (canonical/model separation), `effect_list.rs` | `modals/approval.rs`, `modals/action_card.rs`, `modals/batch_review.rs` | `crates/roko-cli/src/tui/modals/approval.rs` | `apps/mori/src/tui/modals/approval.rs`, `apps/bardo-terminal/src/screens/hearth/activity.rs` |
| **PRD-09 Memory/Groups** | `views/memory_view.rs` (query interface, result list) | `memory_result.rs` (result with provenance) | `modals/memory_detail.rs` (full item detail) | `crates/roko-cli/src/tui/views/context_view.rs` | `apps/bardo-terminal/src/screens/mind/` |
| **PRD-10 Data/Observability** | `views/logs_view.rs` (event stream), `pages/operations.rs` (metrics) | `run_timeline.rs`, `token_gauge.rs` | — | `crates/roko-cli/src/tui/views/logs_view.rs`, `tui/views/learning_view.rs` | `apps/mori/src/tui/views/monitors.rs`, `tui/views/processes.rs` |
| **PRD-11 Deployment** | `pages/operations.rs` (worker health), future: `views/workers_view.rs` | `status_badge.rs` (worker status) | — | `crates/roko-cli/src/tui/views/dashboard_view.rs` | `apps/mori/src/tui/views/pipeline.rs` |
| **PRD-12 Marketplace** | `views/marketplace_view.rs` (Phase 4+) | — | — | `crates/roko-cli/src/tui/views/marketplace_view.rs` | `apps/bardo-terminal/src/` |
| **PRD-13 UX/Surfaces** | ALL screens | ALL widgets | ALL modals | `crates/roko-cli/src/tui/` (80 files) | `apps/mori/src/tui/`, `apps/bardo-terminal/src/` |
| **PRD-14 APIs/Config** | `views/config_view.rs` (config display) | — | — | `crates/roko-cli/src/tui/views/config_view.rs` | `apps/mori/src/tui/views/config.rs` |
| **PRD-15 Testing** | `views/logs_view.rs` (gate results), future: `views/eval_view.rs` | `status_badge.rs` (gate pass/fail) | — | `crates/roko-cli/src/tui/verdicts.rs` | `apps/mori/src/tui/views/review.rs` |

### F.3 Critical UX Invariants in TUI

The following invariants from PRD-02 and PRD-08 MUST be enforced in every TUI
widget and screen that displays state:

1. **Truthful status:** `EffectOutcome::Unknown` MUST be displayed as `UNKNOWN`
   (amber/yellow). It MUST NOT be displayed as `SUCCESS` or `FAILURE`. Reference:
   `crates/roko-cli/src/tui/widgets/status_badge.rs` — map unknown to amber.

2. **Canonical/model separation in action cards:** The `action_card.rs` widget
   MUST render canonical fields (pallet, call, args, fee, account) ABOVE a visual
   separator. Model-generated narrative MUST appear BELOW the separator in a
   visually distinct style (dimmed, italic, or bordered). This separation cannot
   be collapsed by the user. Reference: `apps/mori/src/tui/modals/approval.rs`.

3. **Approval controls keyboard-first:** The `modals/approval.rs` modal MUST
   have keyboard-primary approval (`Enter` = approve, `Esc` = cancel). Mouse
   click approval is secondary. The approval button MUST require a deliberate
   action (no auto-dismiss, no default accept on timeout).

4. **No silent execution:** if the control plane is unreachable, the TUI MUST
   display a clear offline warning. No effects may be submitted while offline
   unless the user explicitly confirms "local mode" with offline-safe degradation.

### F.4 TUI Design System

Polkagent's TUI design system draws from both reference projects:

- **Roko ROSEDUST:** `crates/roko-cli/src/tui/widgets/rosedust.rs` — widget
  rendering primitives; status color semantics (green/red/amber/blue/gray)
- **Bardo design system:** `apps/bardo-terminal/src/design/palette.rs`,
  `design/tokens.rs`, `design/transitions.rs` — color tokens, typography tokens,
  animation curves

Proposed Polkagent design token hierarchy:

```rust
// polkagent-cli/src/tui/theme.rs

pub struct Theme {
    // Status colors
    pub success: Color,      // Green  — confirmed, completed
    pub failure: Color,      // Red    — failed, rejected
    pub unknown: Color,      // Amber  — MUST never be green or red
    pub pending: Color,      // Blue   — in-flight, processing
    pub disabled: Color,     // Gray   — inactive, no capability

    // Action card separation
    pub canonical_bg: Color, // Card canonical fields background
    pub separator: Color,    // Visual separator between canonical and model
    pub model_bg: Color,     // Card model narrative background (dimmed)

    // Layout
    pub header_bg: Color,
    pub tab_active: Color,
    pub tab_inactive: Color,
    pub status_bar_bg: Color,
    pub modal_border: Color,
}

pub const POLKAGENT_DARK: Theme = Theme {
    success: Color::Rgb(80, 200, 120),
    failure: Color::Rgb(220, 80, 80),
    unknown: Color::Rgb(220, 160, 30),  // amber — not green, not red
    pending: Color::Rgb(80, 140, 220),
    disabled: Color::Rgb(120, 120, 120),
    canonical_bg: Color::Rgb(20, 30, 45),
    separator: Color::Rgb(100, 100, 130),
    model_bg: Color::Rgb(15, 20, 30),
    header_bg: Color::Rgb(25, 25, 40),
    tab_active: Color::Rgb(180, 140, 255),
    tab_inactive: Color::Rgb(100, 100, 140),
    status_bar_bg: Color::Rgb(20, 20, 35),
    modal_border: Color::Rgb(140, 100, 220),
};
```

### F.5 Tab Layout (F1–F8)

| Key | Tab | Primary PRDs | Main content |
|---|---|---|---|
| F1 | Dashboard / Home | PRD-03, PRD-13 | Agent count, active run count, effect queue depth, chain connection, recent activity |
| F2 | Agents | PRD-04, PRD-13 | Agent list with status badges, model, last run time; select for detail modal |
| F3 | Runs | PRD-03, PRD-13 | Run timeline, step log; select run for step-by-step detail modal |
| F4 | Effects / Approvals | PRD-08, PRD-13 | Pending effects; action card preview; keyboard approval (Enter/Esc) |
| F5 | Memory / Knowledge | PRD-09, PRD-13 | Search input; result list with provenance; select for detail |
| F6 | Operations | PRD-10, PRD-11, PRD-13 | System metrics (CPU/mem/disk), OTel spans, worker health, log tail |
| F7 | Configuration | PRD-14, PRD-13 | Active config (read-only); active policy files; chain profile |
| F8 | Logs | PRD-10, PRD-13 | JSONL event stream tail; text filter; structured field display |

---

*Appendices A–F appended 2026-07-30.*

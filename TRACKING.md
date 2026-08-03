# Polkagent Implementation Tracking

> Generated: 2026-08-03
> Source of truth: PRD-00 through PRD-18, PROGRESS.md, DIAGNOSTIC-FINDINGS.md

---

## Executive Summary

| Area | Done | Total | % |
|------|------|-------|---|
| **Phase 1: Safety Kernel** | 9/9 AC, 19/19 crates | 28/28 | **100%** |
| **Phase 2: Build + Act + Reach** | 3/8 AC, 46/46 crates | 49/54 | **91%** |
| **Phase 3: Read-Only Value** | 0/5 AC, 6/6 deliverables | 6/11 | **55%** |
| **Phase 4: Controlled Write** | 0/7 AC, 0/8 deliverables | 0/15 | **0%** |
| **Phase 5: Managed + Public** | 0/7 AC, 0/10 deliverables | 0/17 | **0%** |
| **Phase 6: Experimental** | 0/0 AC, 0/6 deliverables | 0/6 | **0%** |
| **Infrastructure Crates** | 11/11 crates | 11/11 | **100%** |
| **Build Infrastructure** | 10/10 items | 10/10 | **100%** |
| **Diagnostic Fixes** | 64/107 findings | 64/107 | **60%** |
| **PRD Acceptance Criteria (suite)** | ~48/75 per-PRD criteria | ~48/75 | **~64%** |
| | | | |
| **Overall weighted progress** | | | **~45%** |

### How to read the overall number

- Phases 1-3 represent the **self-hosted read-only product** (~60% of total scope) and are ~85% done by crate count
- Phases 4-6 represent **write operations, cloud, marketplace, experimental** (~40% of scope) and are 0% done
- The diagnostic audit found 107 bugs; 64 are fixed, 43 remain
- The "45%" reflects that the crate/code work is heavily front-loaded but critical wiring (chain adapter, real tool execution) is still stubbed

---

## Phase 1: Safety Kernel — COMPLETE (100%)

> **Goal:** Crash-safe execution core with no real I/O. Fault injection shows no duplicate effects.

### Crates (19/19 done)

| Crate | Tests | Status |
|-------|-------|--------|
| `polkagent-core` | 170 + 59 proptest | Done |
| `polkagent-store-trait` | 7 | Done |
| `polkagent-executor-trait` | 8 + 4 contract | Done |
| `polkagent-signer-trait` | 8 + 4 contract | Done |
| `polkagent-chain-trait` | 0 (interface) | Done |
| `polkagent-transport-trait` | 9 + 2 contract | Done |
| `polkagent-effect` | 64 + 11 proptest | Done |
| `polkagent-event` | 39 | Done |
| `polkagent-run` | 135 | Done |
| `polkagent-grant` | 115 + 18 proptest | Done |
| `polkagent-artifact` | 42 | Done |
| `polkagent-config` | 138 | Done |
| `polkagent-card` | 35 | Done |
| `polkagent-outbox` | 33 | Done |
| `polkagent-store-sqlite` | 156 | Done |
| `polkagent-executor-fake` | 19 | Done |
| `polkagent-signer-fake` | 17 | Done |
| `polkagent-transport-fake` | 17 | Done |
| `polkagent-test-fixtures` | 26 | Done |

### Acceptance Criteria (9/9 passed)

| ID | Criterion | Status |
|----|-----------|--------|
| AC-P1-001 | Run state machine transitions match PRD-03 §3.2 | PASS |
| AC-P1-002 | EffectIntent persisted BEFORE any I/O | PASS |
| AC-P1-003 | Restart after crash never duplicates a completed effect | PASS |
| AC-P1-004 | Unknown outcome never collapsed to success or failure | PASS |
| AC-P1-005 | Outbox delivers in order, handles duplicates | PASS |
| AC-P1-006 | Grant resolution rejects undeclared capabilities | PASS |
| AC-P1-007 | Artifact lineage tracks parent/child correctly | PASS |
| AC-P1-008 | Fake adapters pass port contract tests | PASS |
| AC-P1-009 | Configuration rejects invalid schemas | PASS |

### PRD-00 §7.3 Implementation Checklist (12/12 done)

All 12 checklist items (workspace setup, core types, run, effect, outbox, grant, artifact, event, store-sqlite, fake adapters, config, Phase 1 tests) are complete.

---

## Phase 2: Build + Act + Reach — SUBSTANTIALLY COMPLETE (91%)

> **Goal:** One useful action from each pillar, projected through at least one surface.

### Crates — all layers complete (46/46)

**2a Integration Layer (6/6)**

| Crate | Tests | Status |
|-------|-------|--------|
| `polkagent-store-sqlite` | 203 | Done |
| `polkagent-store-sqlite-group` | 41 | Done |
| `polkagent-store-sqlite-feed` | 29 | Done |
| `polkagent-api` | 159 | Done |
| `polkagent-cli` | 105 | Done |
| `polkagent-service` | 71 | Done |

**2b Real Adapters (9/9)**

| Crate | Tests | Status |
|-------|-------|--------|
| `polkagent-executor-anthropic` | 60 | Done |
| `polkagent-executor-openai` | 57 | Done |
| `polkagent-executor-local` | 42 | Done |
| `polkagent-metadata` | 80 | Done |
| `polkagent-memory` | 135 | Done |
| `polkagent-signer-external` | 42 | Done |
| `polkagent-chain-fake` | 42 | Done |
| `polkagent-chain-subxt` | 90 | Done |
| `polkagent-transport-pca` | 77 | Done |

**2c Domain Crates (14/14)**

| Crate | Tests | Status |
|-------|-------|--------|
| `polkagent-tool` | 43 | Done |
| `polkagent-tool-governance` | 67 | Done |
| `polkagent-tool-treasury` | 65 | Done |
| `polkagent-skill` | 55 | Done |
| `polkagent-telemetry` | 42 | Done |
| `polkagent-harness-trait` | 22 | Done |
| `polkagent-harness-claude` | 56 | Done |
| `polkagent-secret` | 66 | Done |
| `polkagent-identity` | 42 | Done |
| `polkagent-payment` | 46 | Done |
| `polkagent-conversation` | 44 | Done |
| `polkagent-codec` | 80 | Done |
| `polkagent-group` | 85 | Done |
| `polkagent-feed` | 72 | Done |

**2d Testing Infrastructure (13/13)**

| Component | Tests | Status |
|-----------|-------|--------|
| `polkagent-eval` | 71 | Done |
| `polkagent-fault` | 54 | Done |
| Property tests (core) | 59 | Done |
| Property tests (effect) | 11 | Done |
| Property tests (grant) | 18 | Done |
| Property tests (5 crates) | 35 | Done |
| Integration tests | 380 | Done |
| Port contract tests | 10 | Done |
| Store contract tests | 46 | Done |
| Security tests | 157 | Done |
| TUI snapshot tests | 19 | Done |
| Fuzz test harnesses | 10 | Done |
| Red-team security tests | 67 | Done |

**2e End-to-End Flows (8/8)**

| Flow | Status |
|------|--------|
| Explain Before Sign | Done |
| CLI `polkagent run` | Done |
| CLI `polkagent serve` | Done |
| CLI `polkagent network` | Done |
| CLI `polkagent eval` | Done |
| API event streaming | Done |
| API Prometheus metrics | Done |
| Extrinsic decode fixture | Done |

### Acceptance Criteria (3/8 — this is the gap)

| ID | Criterion | Status | Blocker |
|----|-----------|--------|---------|
| AC-P2-001 | Extrinsic decoded correctly against pinned metadata | **PENDING** | Validation wired in polkagent-metadata but not exercised end-to-end against live chain |
| AC-P2-002 | Action card displays canonical fields, model text visually separate | PASS | polkagent-card complete with render.rs |
| AC-P2-003 | Signer receives exact bytes, never model-modified data | **PENDING** | Integration test not wired end-to-end |
| AC-P2-004 | Stale metadata produces explicit error, not wrong decode | **PENDING** | polkagent-metadata has drift detection but no live-chain test |
| AC-P2-005 | Wrong-network extrinsic rejected before signing | **PENDING** | Cross-network test corpus not executed |
| AC-P2-006 | CLI and web show identical run state | **PENDING** | No web surface exists (web/ directory planned but not built) |
| AC-P2-007 | Model executor streams tokens and emits correct events | PASS | |
| AC-P2-008 | REST API returns correct run/effect/artifact data | PASS | |

### PRD-00 §8.3 Implementation Checklist (7/10)

| # | Item | Status | Notes |
|---|------|--------|-------|
| 1 | Subxt chain adapter | Done | polkagent-chain-subxt (90 tests), but NOT wired into CLI |
| 2 | Metadata service | Done | polkagent-metadata (80 tests), drift detection works |
| 3 | Anthropic executor | Done | polkagent-executor-anthropic (60 tests) |
| 4 | Explain Before Sign flow | Done | Pipeline in polkagent-service |
| 5 | Action card renderer | Done | polkagent-card |
| 6 | External signer adapter | Done | polkagent-signer-external (42 tests) |
| 7 | CLI surface | Done | polkagent-cli (105 tests) |
| 8 | Web surface (Agent Studio) | **NOT STARTED** | No web/ directory or TypeScript code exists |
| 9 | Core REST API | Done | polkagent-api (159 tests) |
| 10 | Phase 2 tests | **PARTIAL** | Tests exist but end-to-end validation AC items not verified |

---

## Phase 3: Read-Only Value — PARTIALLY COMPLETE (55%)

> **Goal:** Useful read-only workflows without write authority.

### Deliverables (6/10)

| # | Deliverable | Status | Notes |
|---|-------------|--------|-------|
| 3.1 | Storage-migration rehearsal (A1) | **NOT STARTED** | |
| 3.2 | Upgrade impact brief (A2) | **NOT STARTED** | |
| 3.3 | Metadata-grounded RAG (A9) | **NOT STARTED** | |
| 3.4 | OpenGov research copilot (B2) | Done | polkagent-tool-governance (67 tests) — but tools hit FakeChainClient |
| 3.5 | Treasury/portfolio research (B9) | Done | polkagent-tool-treasury (65 tests) — but tools hit FakeChainClient |
| 3.6 | Live run timeline (F3) | Done | TUI runs tab exists |
| 3.7 | Error/recovery explainer (F7) | **NOT STARTED** | |
| 3.8 | Identity signal display (I2) | **NOT STARTED** | |
| 3.9 | PCA C0 compatibility (C0) | Done | polkagent-transport-pca (77 tests) |
| 3.10 | Additional model executors | Done | OpenAI (57 tests), Local/Ollama (42 tests) |

### Acceptance Criteria (0/5)

| ID | Criterion | Status | Blocker |
|----|-----------|--------|---------|
| AC-P3-001 | Read-only workflows produce zero write effects | **PENDING** | Effect audit not run — tools use FakeChainClient |
| AC-P3-002 | RAG answers cite metadata hash, block, and source | **PENDING** | RAG not implemented |
| AC-P3-003 | PCA C0 bot can connect and exchange messages | **PENDING** | Transport crate exists but integration test not run |
| AC-P3-004 | Real bounded jobs show repeat use | **PENDING** | No real-chain usage yet |
| AC-P3-005 | No source or privacy regressions | **PENDING** | Security regression suite not run against Phase 3 |

### Critical Blocker: Chain Adapter Not Wired

The governance and treasury tools (3.4, 3.5) are **implemented and tested** (132 combined tests) but they execute against `FakeChainClient`, not `SubxtChainClient`. The real chain client exists (90 tests) but is:
- Not depended on by `polkagent-cli` (missing in Cargo.toml)
- Not instantiated in `main.rs` dispatch
- The `polkagent chain` CLI subcommands are hardcoded stubs

**This is the single biggest gap preventing real-world usage.**

---

## Phase 4: Controlled Write — NOT STARTED (0%)

> **Goal:** Carefully gated write operations with family-specific evidence.

### Deliverables (0/8)

| # | Deliverable | Status |
|---|-------------|--------|
| 4.1 | Pre-sign risk gates (B4) | Not started |
| 4.2 | Multisig/proxy coordinator (B6) | Not started |
| 4.3 | XCM planning prototype (A3/B5) | Not started |
| 4.4 | Metadata-drift watcher (A7) | Not started |
| 4.5 | Product kits v1 (H1) | Not started |
| 4.6 | PCA C1 compatibility (C1) | Not started |
| 4.7 | Harness lifecycle (Claude, Codex) | Partial — harness-claude (56 tests) and harness-codex exist, but session persistence/resume not done |
| 4.8 | Provenanced memory v1 (G1) | Partial — memory system exists (135 tests) but provenance chain not implemented |

### Acceptance Criteria (0/7)

| ID | Criterion | Status |
|----|-----------|--------|
| AC-P4-001 | Risk gates flag batch/proxy/approval patterns | Not started |
| AC-P4-002 | Multisig coordinator tracks approvals without holding keys | Not started |
| AC-P4-003 | XCM planner refuses unsupported routes | Not started |
| AC-P4-004 | Product kit install/uninstall/conformance works | Not started |
| AC-P4-005 | PCA C1 bridges function | Not started |
| AC-P4-006 | Harness sessions resume after restart | Not started |
| AC-P4-007 | Memory respects tenant/conversation boundaries | Not started |

---

## Phase 5: Managed + Public — NOT STARTED (0%)

> **Goal:** Cloud deployment, real-value operations, marketplace.
> **GATE: Independent security and legal review required.**

### Deliverables (0/10)

| # | Deliverable | Status |
|---|-------------|--------|
| 5.1 | Funded policy-bounded accounts | Not started |
| 5.2 | Agent earns/spends prototype | Not started |
| 5.3 | Agent-to-agent escrow (research) | Not started |
| 5.4 | Capability-disclosed packages | Not started |
| 5.5 | Public agent-service listings | Not started |
| 5.6 | Fleet workers | Not started |
| 5.7 | Regional isolation | Not started |
| 5.8 | Multi-tenant managed platform | Not started |
| 5.9 | Billing and metering | Not started |
| 5.10 | PCA C2/C3 compatibility | Not started |

### Acceptance Criteria (0/7)

All pending: AC-P5-001 through AC-P5-007

### Planned Crates (0/6 exist)

`polkagent-cloud-control`, `polkagent-cloud-worker`, `polkagent-billing`, `polkagent-store-postgres`, `polkagent-signer-kms`, `polkagent-signer-proxy`

---

## Phase 6: Experimental Frontier — NOT STARTED (0%)

> **Goal:** Explore long-term opportunities that do not affect core correctness.

### Deliverables (0/6)

| # | Deliverable | Status |
|---|-------------|--------|
| 6.1 | JAM service prototypes (D1-D4) | Not started |
| 6.2 | Personhood gating (I3) | Not started |
| 6.3 | Visionary bets (K1-K6) | Not started |
| 6.4 | Affect/vitality modules | Not started |
| 6.5 | Evolutionary skill selection (G5) | Not started |
| 6.6 | Always-on watchers (C4) | Not started |

---

## Infrastructure Crates — COMPLETE (100%)

| Crate | Tests | Status |
|-------|-------|--------|
| `polkagent-health` | 54 | Done |
| `polkagent-scheduler` | 84 | Done |
| `polkagent-cache` | 45 | Done |
| `polkagent-retry` | 54 | Done |
| `polkagent-batch` | 53 | Done |
| `polkagent-plugin` | 94 | Done |
| `polkagent-rate-limit` | 50 | Done |
| `polkagent-audit` | 68 | Done |
| `polkagent-context` | 71 | Done |
| `polkagent-migration` | 63 | Done |
| `polkagent-surface-webhook` | 52 | Done |

---

## Diagnostic Findings — 60% Fixed

Source: `prd/DIAGNOSTIC-FINDINGS.md` (2026-08-03 audit)

### By Category

| Category | Found | Fixed | Remaining | % Fixed |
|----------|-------|-------|-----------|---------|
| SQL Schema Mismatches | 17 | 12 | 5 | 71% |
| Run Lifecycle | 12 | 7 | 5 | 58% |
| TUI Rendering | 19 | 12 | 7 | 63% |
| CLI Commands | 10 | 8 | 2 | 80% |
| REST/WS API | 14 | 7 | 7 | 50% |
| Effect Pipeline | 10 | 6 | 4 | 60% |
| Config & Providers | 12 | 5 | 7 | 42% |
| Memory System | 7 | 3 | 4 | 43% |
| Eval Framework | 6 | 4 | 2 | 67% |
| **Total** | **107** | **64** | **43** | **60%** |

### Remaining P0/Critical Issues

| ID | Issue | File | Impact |
|----|-------|------|--------|
| 1.1 | `run_events` column names still wrong in 5 files | `event_store.rs`, `store.rs` | Events silently fail |
| 2.5 | Token usage never persisted to DB | `orchestrator.rs` | All token counts show 0 |
| 2.6 | AwaitingApproval state loses payload after reload | `manager.rs` | Approval routing breaks after restart |
| 2.7 | TimeoutEnforcer is dead code, never instantiated | `timeout.rs` | No wall-clock enforcement |

### Remaining P1 Issues (selected)

| ID | Issue | Impact |
|----|-------|--------|
| 3.6 | Scroll hardcoded to 20 rows — broken on short terminals | UX broken on small screens |
| 3.9 | SQL injection risk in memory FTS query | Security vulnerability |
| 4.4 | `explain` command is permanent stub | Feature non-functional |
| 4.5 | All 4 `chain` subcommands are stubs | Feature non-functional |
| 4.6 | `inspect` handlers are always-error stubs | Feature non-functional |
| 4.7 | `skill` commands query non-existent `skills` table | Commands crash |
| 5.3 | Metrics double-count runs | Metrics inaccurate |
| 5.4 | WebSocket pong timeout inverted | Connection management broken |
| 7.2 | Empty API key accepted for providers | Silent auth failure |
| 7.4 | Gemini silently routed to OpenAI executor | Wrong model used |

### Dead Code (16 items)

- 2 entire unregistered modules (`export.rs`, `inspect.rs`)
- 3 unused widgets
- 8 dead TUI methods
- 3 dead CLI utilities

---

## Per-PRD Acceptance Criteria Status

Each PRD defines 5 acceptance criteria in PRD-00 §18. Assessment based on implementation evidence:

| PRD | Title | Criteria Met | Total | % | Notes |
|-----|-------|-------------|-------|---|-------|
| PRD-01 | Vision, Principles, Personas | 5 | 5 | 100% | Design doc, review-verified |
| PRD-02 | Vocabulary, Architecture | 5 | 5 | 100% | Crate layout matches, no circular deps |
| PRD-03 | Execution Model | 4 | 5 | 80% | Crash recovery specified but has bugs (§2.2, 2.3) |
| PRD-04 | Providers, Models, Tools | 4 | 5 | 80% | Harness lifecycle incomplete (no resume) |
| PRD-05 | Polkadot Integrations | 2 | 5 | 40% | Chain adapter exists but not wired; JAM experimental |
| PRD-06 | PCA Compatibility | 2 | 5 | 40% | Transport exists; C0 integration test not run |
| PRD-07 | Identity, Security | 4 | 5 | 80% | Key isolation enforced; grant resolution works; threat model incomplete |
| PRD-08 | Payments, Autonomy | 2 | 5 | 40% | EBS pipeline built; payment domain exists; no real-value tests |
| PRD-09 | Memory, Groups, Evals | 4 | 5 | 80% | Memory+groups built; provenance not tracked |
| PRD-10 | Data, Observability | 4 | 5 | 80% | Artifacts, events, telemetry done; BLAKE3 digest always zeros |
| PRD-11 | Deployment, Cloud | 1 | 5 | 20% | Docker files exist; no cloud control/worker/billing |
| PRD-12 | Marketplace, Extensions | 1 | 5 | 20% | Plugin crate exists; no marketplace/registry |
| PRD-13 | UX, Surfaces | 3 | 5 | 60% | CLI+TUI done; no web surface; accessibility untested |
| PRD-14 | APIs, Schemas, Config | 4 | 5 | 80% | REST+WS API done; config done; migration tool done |
| PRD-15 | Testing, Roadmap | 4 | 5 | 80% | Testing pyramid complete; CI gates present; security review pending |
| | | **~49** | **75** | **~65%** | |

---

## Planned vs Actual Crate Layout

From PRD-00 §14 vs what exists:

| Planned Crate | Exists? | Phase |
|---------------|---------|-------|
| `polkagent-core` | Yes | 1 |
| `polkagent-run` | Yes | 1 |
| `polkagent-effect` | Yes | 1 |
| `polkagent-outbox` | Yes | 1 |
| `polkagent-grant` | Yes | 1 |
| `polkagent-artifact` | Yes | 1 |
| `polkagent-event` | Yes | 1 |
| `polkagent-memory` | Yes | 1 |
| `polkagent-executor-trait` | Yes | 1 |
| `polkagent-executor-anthropic` | Yes | 2 |
| `polkagent-executor-openai` | Yes | 3 |
| `polkagent-executor-local` | Yes | 3 |
| `polkagent-executor-fake` | Yes | 1 |
| `polkagent-harness-trait` | Yes | 4 |
| `polkagent-harness-claude` | Yes | 4 |
| `polkagent-harness-codex` | Yes | 4 |
| `polkagent-harness-opencode` | **No** | 4 |
| `polkagent-harness-bridge` | **No** | 4 |
| `polkagent-signer-trait` | Yes | 1 |
| `polkagent-signer-external` | Yes | 2 |
| `polkagent-signer-proxy` | **No** | 5 |
| `polkagent-signer-kms` | **No** | 5 |
| `polkagent-signer-fake` | Yes | 1 |
| `polkagent-transport-trait` | Yes | 1 |
| `polkagent-transport-pca` | Yes | 3 |
| `polkagent-transport-http` | **No** | 3 |
| `polkagent-transport-fake` | Yes | 1 |
| `polkagent-chain-trait` | Yes | 1 |
| `polkagent-chain-subxt` | Yes | 2 |
| `polkagent-chain-fake` | Yes | 1 |
| `polkagent-metadata` | Yes | 2 |
| `polkagent-store-trait` | Yes | 1 |
| `polkagent-store-sqlite` | Yes | 1 |
| `polkagent-store-postgres` | **No** | 5 |
| `polkagent-card` | Yes | 2 |
| `polkagent-action-sign` | **No** | 2 |
| `polkagent-action-transfer` | **No** | 4 |
| `polkagent-action-governance` | **No** | 4 |
| `polkagent-config` | Yes | 1 |
| `polkagent-api` | Yes | 2 |
| `polkagent-cli` | Yes | 2 |
| `polkagent-daemon` | **No** | 2 |
| `polkagent-marketplace` | **No** | 5 |
| `polkagent-skill` | Yes | 4 |
| `polkagent-tool` | Yes | 4 |
| `polkagent-cloud-control` | **No** | 5 |
| `polkagent-cloud-worker` | **No** | 5 |
| `polkagent-billing` | **No** | 5 |
| `polkagent-test-fixtures` | Yes | 1 |

**Planned: 49 crates | Exist: 37 | Missing: 12** (most are Phase 4-5)

---

## What Would Make It Actually Work (Priority Order)

### 1. Wire SubxtChainClient into CLI (unblocks real-world usage)

- Add `polkagent-chain-subxt` dependency to `polkagent-cli/Cargo.toml`
- Instantiate `SubxtChainClient` in `main.rs` based on `POLKAGENT_RPC_URL` or config
- Replace stubs in `commands/chain.rs` with real RPC calls
- Pass chain client to `AppService` for tool execution
- **Impact:** Unblocks `polkagent chain balance`, all governance/treasury tools hit real chains

### 2. Fix remaining P0 diagnostic issues (4 items)

- Token persistence, approval state reload, timeout enforcer, remaining SQL schema fixes
- **Impact:** Runs actually track costs, survive restarts, enforce timeouts

### 3. Create `skills` table migration

- The `polkagent skill` commands all query a table that doesn't exist
- **Impact:** Skill management becomes functional

### 4. Web surface (Agent Studio)

- PRD-00 §8.1 deliverable 2.7, currently not started
- AC-P2-006 depends on it (CLI and web show identical run state)
- **Impact:** Completes Phase 2 acceptance criteria

---

## Cross-Reference Index

| Document | Location | What it covers |
|----------|----------|---------------|
| PRD-00 Master Index | `prd/PRD-00-MASTER-INDEX.md` | Roadmap, phases, acceptance criteria, checklists |
| PRD-01 Vision | `prd/PRD-01-VISION-PRINCIPLES-PERSONAS.md` | Product strategy, pillars, personas |
| PRD-02 Architecture | `prd/PRD-02-VOCABULARY-ARCHITECTURE.md` | Canonical vocabulary, invariants, crate layout |
| PRD-03 Execution | `prd/PRD-03-EXECUTION-MODEL.md` | Agent/run/effect lifecycle, crash recovery |
| PRD-04 Providers | `prd/PRD-04-PROVIDERS-MODELS-TOOLS.md` | Provider/executor/harness/tool/skill taxonomy |
| PRD-04a Expansion | `prd/PRD-04a-PROVIDER-HARNESS-EXPANSION.md` | Phase 2-4 provider expansion |
| PRD-05 Polkadot | `prd/PRD-05-POLKADOT-INTEGRATIONS.md` | Chain integration, builder workflows |
| PRD-06 PCA | `prd/PRD-06-PCA-COMPATIBILITY.md` | PCA migration, chat/mobile |
| PRD-07 Security | `prd/PRD-07-IDENTITY-SECURITY.md` | Identity, signers, policy, threat model |
| PRD-08 Payments | `prd/PRD-08-PAYMENTS-AUTONOMY.md` | Payments, Explain Before Sign, autonomy tiers |
| PRD-09 Memory | `prd/PRD-09-MEMORY-GROUPS-EVALS.md` | Memory types, groups, feeds, evals |
| PRD-10 Data | `prd/PRD-10-DATA-OBSERVABILITY.md` | Artifacts, events, observability, recovery |
| PRD-11 Deployment | `prd/PRD-11-DEPLOYMENT-CLOUD.md` | Self-hosted, cloud, multi-tenancy |
| PRD-12 Marketplace | `prd/PRD-12-MARKETPLACE-EXTENSIONS.md` | Registry, extension SDK, product kits |
| PRD-13 UX | `prd/PRD-13-UX-SURFACES.md` | CLI, Studio, Inbox, mobile, action cards |
| PRD-14 APIs | `prd/PRD-14-APIs-SCHEMAS-CONFIG.md` | REST/WS/gRPC, schemas, config, migration |
| PRD-15 Testing | `prd/PRD-15-TESTING-ROADMAP.md` | Testing pyramid, CI/CD gates, roadmap |
| PRD-16 TUI Diagnostics | `prd/PRD-16-TUI-DIAGNOSTICS.md` | 8 TUI bugs, diagnostic workflow |
| PRD-17 Local Testnet | `prd/PRD-17-LOCAL-TESTNET-E2E.md` | E2E test scenarios, chain integration |
| PRD-18 Interactive TUI | `prd/PRD-18-INTERACTIVE-TUI.md` | Prompt bar, run control, agent management |
| PROGRESS.md | `PROGRESS.md` | Crate-by-crate status with test counts |
| Diagnostic Findings | `prd/DIAGNOSTIC-FINDINGS.md` | 107 bugs, fix status, implementation checklist |
| Evidence Backlog | `tmp/40-report-18-evidence-backlog.md` | 9 open research questions |
| Architecture | `ARCHITECTURE.md` | Hexagonal architecture, dependency rules |

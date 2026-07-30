# Polkagent Implementation Progress

> Auto-updated as implementation proceeds. Each phase has acceptance criteria that must pass before advancing.

## Phase 1: Safety Kernel — COMPLETE

**Goal:** Crash-safe execution core with no real I/O. Fault injection shows no duplicate effects.

| Crate | Status | Tests | Description |
|-------|--------|-------|-------------|
| `polkagent-core` | Done | 170 | IDs, state machines, effect/event/artifact types + 59 proptest |
| `polkagent-store-trait` | Done | 7 | RunStore, EffectStore, ArtifactStore, EventStore traits |
| `polkagent-executor-trait` | Done | 8+4 | ModelExecutor trait + port contract suite |
| `polkagent-signer-trait` | Done | 8+4 | Signer trait + port contract suite |
| `polkagent-chain-trait` | Done | 0 | ChainClient trait (interface only) |
| `polkagent-transport-trait` | Done | 9+2 | Transport trait + port contract suite |
| `polkagent-effect` | Done | 64 | Crash-safe effect pipeline + 11 proptest |
| `polkagent-event` | Done | 39 | Event bus, recorder, projections, monotonic sequences |
| `polkagent-run` | Done | 76 | RunManager, state machine, timeout, turn manager |
| `polkagent-grant` | Done | 64 | Policy evaluation, budget tracking + 18 proptest |
| `polkagent-artifact` | Done | 42 | BLAKE3 content-addressed storage, lineage DAG |
| `polkagent-config` | Done | 43 | TOML loader, env overrides, validation |
| `polkagent-card` | Done | 35 | Action cards, canonical/narrative sections |
| `polkagent-outbox` | Done | 33 | Durable ordered delivery, deduplication |
| `polkagent-store-sqlite` | Done | 137 | WAL-mode SQLite + all 4 store trait impls + 46 contract tests |
| `polkagent-executor-fake` | Done | 19 | Cycling responses, streaming + contract tests |
| `polkagent-signer-fake` | Done | 17 | Deterministic signatures + contract tests |
| `polkagent-transport-fake` | Done | 17 | Channel-based, fault injection + contract tests |
| `polkagent-test-fixtures` | Done | 26 | Builders and factory functions |

### Phase 1 Acceptance Criteria

- [x] AC-P1-001: Run state machine transitions match PRD-03 §3.2
- [x] AC-P1-002: EffectIntent persisted BEFORE any I/O
- [x] AC-P1-003: Restart after crash never duplicates a completed effect
- [x] AC-P1-004: Unknown outcome never collapsed to success or failure
- [x] AC-P1-005: Outbox delivers in order, handles duplicates
- [x] AC-P1-006: Grant resolution rejects undeclared capabilities
- [x] AC-P1-007: Artifact lineage tracks parent/child correctly
- [x] AC-P1-008: Fake adapters pass port contract tests
- [x] AC-P1-009: Configuration rejects invalid schemas

---

## Phase 2: Build + Act + Reach Proof — IN PROGRESS

**Goal:** One useful action from each pillar, projected through at least one surface.

### Phase 2a: Integration Layer

| Crate | Status | Tests | Description |
|-------|--------|-------|-------------|
| `polkagent-store-sqlite` | Done | 137 | All 4 store traits + 46 contract tests |
| `polkagent-api` | Done | 32 | Trait-object stores, WebSocket streaming, effect endpoints |
| `polkagent-cli` | Done | 0 | Wired to SqlitePool, migrations on startup, all commands use store traits |

### Phase 2b: Real Adapters

| Crate | Status | Tests | Description |
|-------|--------|-------|-------------|
| `polkagent-executor-anthropic` | Done | 60 | Anthropic Messages API with streaming, tool calls, retries |
| `polkagent-executor-openai` | Done | 57 | OpenAI-compatible API (OpenAI, Azure, vLLM, Ollama) |
| `polkagent-executor-local` | Done | 42 | Local models via Ollama API, no auth, graceful degradation |
| `polkagent-metadata` | Done | 68 | Metadata cache, pinning, drift detection, service facade |
| `polkagent-memory` | Done | 27 | Episodic/semantic/procedural memory with FTS5 + provenance |
| `polkagent-signer-external` | Done | 42 | External signer with INV-01 enforcement, approval callbacks |
| `polkagent-chain-subxt` | Not started | — | Subxt static+dynamic decode, CheckMetadataHash |

### Phase 2c: End-to-End Flows

| Flow | Status | Description |
|------|--------|-------------|
| Explain Before Sign | Not started | Decode extrinsic → action card → signer handoff → finality |
| CLI `polkagent run` | Partial | Commands wired, needs real executor integration |
| API event streaming | Done | WebSocket with run_id/kind filtering, ping/pong keepalive |

### Phase 2d: Testing Infrastructure

| Component | Status | Tests | Description |
|-----------|--------|-------|-------------|
| Property tests (core) | Done | 59 | Proptest for IDs, states, BlobRef, artifacts, tokens |
| Property tests (effect) | Done | 11 | Proptest for idempotency keys, effect kinds, priorities |
| Property tests (grant) | Done | 18 | Proptest for policies, budgets, gate composition |
| Integration tests | Done | 30 | Cross-crate e2e lifecycle, effects, grants, artifacts, config |
| Port contract tests | Done | 10 | Executor, signer, transport conformance suites |
| Store contract tests | Done | 46 | RunStore, EffectStore, EventStore, ArtifactStore on SQLite |
| Fuzz test harnesses | Not started | — | cargo-fuzz targets per PRD-15 |

### Phase 2 Acceptance Criteria

- [ ] AC-P2-001: Extrinsic decoded correctly against pinned metadata
- [ ] AC-P2-002: Action card displays canonical fields, model text visually separate
- [ ] AC-P2-003: Signer receives exact bytes, never sees model-modified data
- [ ] AC-P2-004: Stale metadata produces explicit error, not wrong decode
- [ ] AC-P2-005: Wrong-network extrinsic rejected before signing
- [ ] AC-P2-006: CLI and web show identical run state
- [x] AC-P2-007: Model executor streams tokens and emits correct events
- [x] AC-P2-008: REST API returns correct run/effect/artifact data

---

## Phase 3: Read-Only Value — PARTIALLY STARTED

**Goal:** Useful read-only workflows without write authority.

| Deliverable | Status | Description |
|-------------|--------|-------------|
| Memory system | Done | polkagent-memory: SQLite FTS5 + provenance tracking |
| OpenAI executor | Done | polkagent-executor-openai: OpenAI/Azure/vLLM compatible |
| Local executor | Done | polkagent-executor-local: Ollama API adapter |
| PCA transport | Not started | PCA encrypted chat transport |
| OpenGov research | Not started | Governance research copilot tools |
| Treasury research | Not started | Portfolio/treasury analysis tools |

---

## Phase 4: Controlled Write — NOT STARTED

**Goal:** Carefully gated write operations with family-specific evidence.

---

## Phase 5: Managed + Public — NOT STARTED

**Goal:** Cloud deployment, real-value operations, marketplace.

**Mandatory legal gate before Phase 5 launch.**

---

## Workspace Summary

| Metric | Count |
|--------|-------|
| Crates | 29 |
| Source files | ~200 |
| Tests | 1,206 |
| Lines of Rust | ~55,000 |

## Build Commands

```bash
# Check everything compiles
cargo check --workspace

# Run all tests
cargo test --workspace

# Run specific crate tests
cargo test -p polkagent-core

# Run only property tests
cargo test -p polkagent-core --test proptests

# Run store contract tests
cargo test -p polkagent-store-sqlite --test store_contracts

# Build release binary
cargo build --release -p polkagent-cli
```

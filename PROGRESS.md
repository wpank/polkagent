# Polkagent Implementation Progress

> Auto-updated as implementation proceeds. Each phase has acceptance criteria that must pass before advancing.

## Phase 1: Safety Kernel — COMPLETE

**Goal:** Crash-safe execution core with no real I/O. Fault injection shows no duplicate effects.

| Crate | Status | Tests | Description |
|-------|--------|-------|-------------|
| `polkagent-core` | Done | 170 | IDs, state machines, effect/event/artifact types + 59 proptest |
| `polkagent-store-trait` | Done | 7 | RunStore, EffectStore, ArtifactStore, EventStore traits |
| `polkagent-executor-trait` | Done | 8 | ModelExecutor trait with streaming |
| `polkagent-signer-trait` | Done | 8 | Signer trait with INV-01 enforcement |
| `polkagent-chain-trait` | Done | 0 | ChainClient trait (interface only) |
| `polkagent-transport-trait` | Done | 9 | Transport trait with ack-exactly-once |
| `polkagent-effect` | Done | 64 | Crash-safe effect pipeline + 11 proptest |
| `polkagent-event` | Done | 39 | Event bus, recorder, projections, monotonic sequences |
| `polkagent-run` | Done | 76 | RunManager, state machine, timeout, turn manager |
| `polkagent-grant` | Done | 64 | Policy evaluation, budget tracking + 18 proptest |
| `polkagent-artifact` | Done | 42 | BLAKE3 content-addressed storage, lineage DAG |
| `polkagent-config` | Done | 43 | TOML loader, env overrides, validation |
| `polkagent-card` | Done | 35 | Action cards, canonical/narrative sections |
| `polkagent-outbox` | Done | 33 | Durable ordered delivery, deduplication |
| `polkagent-store-sqlite` | Done | 91 | WAL-mode SQLite + RunStore/EffectStore/EventStore/ArtifactStore impls |
| `polkagent-executor-fake` | Done | 15 | Cycling responses, streaming |
| `polkagent-signer-fake` | Done | 13 | Deterministic signatures, expiry |
| `polkagent-transport-fake` | Done | 15 | Channel-based, fault injection |
| `polkagent-test-fixtures` | Done | 26 | Builders and factory functions |
| `polkagent-api` | Done | 32 | REST server + WebSocket streaming + effect endpoints |
| `polkagent-cli` | Done | 0 | ROSEDUST TUI (4 views), clap CLI, TEA architecture |
| `polkagent-integration-tests` | Done | 30 | Cross-crate e2e tests (lifecycle, effects, grants, artifacts, config) |

**Total: 982 tests, 0 failures, 0 warnings**

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
| `polkagent-store-sqlite` | Done | 91 | All 4 store traits implemented on SqlitePool (RunStore, EffectStore, EventStore, ArtifactStore) |
| `polkagent-api` | Done | 32 | Trait-object stores, WebSocket event streaming, effect endpoints |
| `polkagent-cli` | Needs work | — | Wire commands to real stores; interactive approval flow |

### Phase 2b: Real Adapters

| Crate | Status | Tests | Description |
|-------|--------|-------|-------------|
| `polkagent-executor-anthropic` | Done | 60 | Anthropic Messages API with streaming, tool calls, retries, token tracking |
| `polkagent-metadata` | Done | 68 | Metadata cache, pinning, drift detection, service facade |
| `polkagent-chain-subxt` | Not started | — | Subxt static+dynamic decode, CheckMetadataHash, submit/finality |
| `polkagent-signer-external` | Not started | — | Browser extension signing protocol |

### Phase 2c: End-to-End Flows

| Flow | Status | Description |
|------|--------|-------------|
| Explain Before Sign | Not started | Decode extrinsic → action card → signer handoff → finality |
| CLI `polkagent run` | Needs work | Create run with real executor, observe via TUI |
| API event streaming | Done | WebSocket streaming with run_id/kind filtering, ping/pong keepalive |

### Phase 2d: Testing Infrastructure

| Component | Status | Tests | Description |
|-----------|--------|-------|-------------|
| Property tests (core) | Done | 59 | Proptest for IDs, states, BlobRef, artifacts, tokens |
| Property tests (effect) | Done | 11 | Proptest for idempotency keys, effect kinds, priorities |
| Property tests (grant) | Done | 18 | Proptest for policies, budgets, gate composition |
| Integration tests | Done | 30 | Cross-crate e2e lifecycle, effects, grants, artifacts, config |
| Fuzz test harnesses | Not started | — | cargo-fuzz targets per PRD-15 |
| Contract test suites | Not started | — | Port conformance tests |

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

## Phase 3: Read-Only Value — NOT STARTED

**Goal:** Useful read-only workflows without write authority.

| Deliverable | Status | Description |
|-------------|--------|-------------|
| Memory system | Not started | Episodic, semantic, procedural storage with provenance |
| OpenAI executor | Not started | OpenAI-compatible executor adapter |
| Local executor | Not started | Ollama/llama.cpp integration |
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
| Crates | 25 |
| Source files | ~170 |
| Tests | 982 |
| Lines of Rust | ~45,000 |

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

# Build release binary
cargo build --release -p polkagent-cli
```

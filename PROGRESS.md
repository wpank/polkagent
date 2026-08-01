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
| `polkagent-run` | Done | 135 | RunManager, state machine, timeout, turn manager, DAG engine, orchestrator |
| `polkagent-grant` | Done | 115 | Policy evaluation, budget tracking, Cedar loader, ABAC conditions, policy templates + 18 proptest |
| `polkagent-artifact` | Done | 42 | BLAKE3 content-addressed storage, lineage DAG |
| `polkagent-config` | Done | 138 | TOML loader, 9 config sections, env overrides, full validation |
| `polkagent-card` | Done | 35 | Action cards, canonical/narrative sections |
| `polkagent-outbox` | Done | 33 | Durable ordered delivery, deduplication |
| `polkagent-store-sqlite` | Done | 156 | WAL-mode SQLite + run/effect/artifact/event/payment/conversation store impls |
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

## Phase 2: Build + Act + Reach Proof — SUBSTANTIALLY COMPLETE

**Goal:** One useful action from each pillar, projected through at least one surface.

### Phase 2a: Integration Layer

| Crate | Status | Tests | Description |
|-------|--------|-------|-------------|
| `polkagent-store-sqlite` | Done | 203 | All 6 store traits (+ payment, conversation) + conformance tests |
| `polkagent-store-sqlite-group` | Done | 41 | Dedicated GroupStore SQLite impl (isolated from store-sqlite for serde recursion) |
| `polkagent-store-sqlite-feed` | Done | 29 | FeedStore SQLite impl (manual serde for recursive TriggerCondition) |
| `polkagent-api` | Done | 159 | REST + WebSocket, health, metrics, audit, conversation, rate-limit middleware |
| `polkagent-cli` | Done | 105 | ROSEDUST TUI, 15 commands, serve/network/eval, approve/deny flow |
| `polkagent-service` | Done | 71 | AppService facade, signer/chain wiring, webhook/scheduler/plugin integration |

### Phase 2b: Real Adapters

| Crate | Status | Tests | Description |
|-------|--------|-------|-------------|
| `polkagent-executor-anthropic` | Done | 60 | Anthropic Messages API with streaming, tool calls, retries |
| `polkagent-executor-openai` | Done | 57 | OpenAI-compatible API (OpenAI, Azure, vLLM, Ollama) |
| `polkagent-executor-local` | Done | 42 | Local models via Ollama API, no auth, graceful degradation |
| `polkagent-metadata` | Done | 80 | Metadata cache, pinning, drift detection, CachedMetadataService with LRU+TTL |
| `polkagent-memory` | Done | 135 | FTS5 memory + admission, retention, tenant isolation, VectorIndex embedding |
| `polkagent-signer-external` | Done | 42 | External signer with INV-01 enforcement, approval callbacks |
| `polkagent-chain-fake` | Done | 42 | Fake ChainClient with pre-seeded data, fault injection, call tracking |
| `polkagent-chain-subxt` | Done | 90 | JSON-RPC chain client, SCALE decode via polkagent-codec, multi-profile |
| `polkagent-transport-pca` | Done | 77 | X25519 + ChaCha20-Poly1305 encrypted transport |

### Phase 2c: Domain Crates

| Crate | Status | Tests | Description |
|-------|--------|-------|-------------|
| `polkagent-tool` | Done | 43 | ToolRegistry, ToolHandler, BatchToolExecutor, grant-checked execution |
| `polkagent-tool-governance` | Done | 67 | OpenGov governance research tools (referendum, track, voter, delegation, treasury) |
| `polkagent-tool-treasury` | Done | 65 | Portfolio/balance/staking/transfer/vesting analysis tools |
| `polkagent-skill` | Done | 55 | TOML manifests, SkillLoader, dependency resolution, semver |
| `polkagent-telemetry` | Done | 42 | OpenTelemetry, Redacted<T>, MetricRecorder, PrometheusRegistry, JsonlWriter |
| `polkagent-harness-trait` | Done | 22 | Harness port trait with session lifecycle |
| `polkagent-harness-claude` | Done | 56 | Claude Code harness with real subprocess I/O |
| `polkagent-secret` | Done | 66 | SecretValue (zeroize), env/file/chain stores, audit log, secret scanning/redaction |
| `polkagent-identity` | Done | 42 | AccountId32, SS58 encode/decode, Ed25519 signature verification |
| `polkagent-payment` | Done | 46 | Amount arithmetic, BudgetChecker, CostEstimator, PaymentStore |
| `polkagent-conversation` | Done | 44 | Conversation/Message types, InMemoryStore, ContextWindow |
| `polkagent-codec` | Done | 80 | Pure-Rust SCALE encode/decode, metadata parsing, call helpers |
| `polkagent-group` | Done | 85 | Multi-agent groups: quorum, grant intersection, budget, evidence |
| `polkagent-feed` | Done | 72 | Feeds, triggers, recipes: cursor-backed processing, condition evaluation |

### Phase 2d: Testing Infrastructure

| Component | Status | Tests | Description |
|-----------|--------|-------|-------------|
| `polkagent-eval` | Done | 71 | Eval framework: suites, scorer, regression detection, builtin safety corpus |
| `polkagent-fault` | Done | 54 | Fault injection: crash/timeout/corrupt wrappers for all port traits |
| Property tests (core) | Done | 59 | Proptest for IDs, states, BlobRef, artifacts, tokens |
| Property tests (effect) | Done | 11 | Proptest for idempotency keys, effect kinds, priorities |
| Property tests (grant) | Done | 18 | Proptest for policies, budgets, gate composition |
| Property tests (5 crates) | Done | 35 | Proptest for config, card, outbox, event, artifact |
| Integration tests | Done | 380 | Cross-crate e2e: lifecycle, effects, grants, tools, skills, conversations, faults, infrastructure |
| Port contract tests | Done | 10 | Executor, signer, transport conformance suites |
| Store contract tests | Done | 46 | RunStore, EffectStore, EventStore, ArtifactStore on SQLite |
| Security tests | Done | 157 | Secrets, injection, cards + RT-02-12 red-team + MD-02/03/05 metadata |
| TUI snapshot tests | Done | 19 | Widget rendering and view layout tests |
| Fuzz test harnesses | Done | 10 | cargo-fuzz targets: config, card, policy, effect, JSON, ID, event, transport, skill, API |
| Red-team security tests | Done | 67 | RT-02 through RT-12, MD-02/03/05 metadata safety |
| Port conformance suites | Done | 63 | Shared test functions for all 6 port traits |
| Property tests (PB-01-10) | Done | 37 | Proptest for grant intersection, effect FSM, serialization, FIFO, budget |
| Fault injection tests | Done | 57 | FaultInjector framework + FI-01/02/03 integration tests |

### Phase 2e: End-to-End Flows

| Flow | Status | Description |
|------|--------|-------------|
| Explain Before Sign | Done | ExplainBeforeSign pipeline in polkagent-service (decode → card → approve → sign → finality) |
| CLI `polkagent run` | Done | Real executor detection (Anthropic/OpenAI/Local), wired in run command |
| CLI `polkagent serve` | Done | API server with --host/--port/--cors-origin/--read-only flags |
| CLI `polkagent network` | Done | Chain status and metadata subcommands via FakeChainClient |
| CLI `polkagent eval` | Done | Enhanced eval with --agent/--details flags |
| API event streaming | Done | WebSocket with run_id/kind filtering, ping/pong keepalive |
| API Prometheus metrics | Done | `/metrics` endpoint with PrometheusRegistry |
| Extrinsic decode fixture | Done | Real Polkadot balance transfer SCALE fixture + decode tests |


### Phase 2 Acceptance Criteria

- [ ] AC-P2-001: Extrinsic decoded correctly against pinned metadata (validation wired in polkagent-metadata)
- [x] AC-P2-002: Action card displays canonical fields, model text visually separate (polkagent-card complete with render.rs)
- [ ] AC-P2-003: Signer receives exact bytes, never sees model-modified data
- [ ] AC-P2-004: Stale metadata produces explicit error, not wrong decode
- [ ] AC-P2-005: Wrong-network extrinsic rejected before signing
- [ ] AC-P2-006: CLI and web show identical run state
- [x] AC-P2-007: Model executor streams tokens and emits correct events
- [x] AC-P2-008: REST API returns correct run/effect/artifact data

---

## Phase 3: Read-Only Value — SUBSTANTIALLY COMPLETE

**Goal:** Useful read-only workflows without write authority.

| Deliverable | Status | Description |
|-------------|--------|-------------|
| Memory system | Done | polkagent-memory: SQLite FTS5 + admission control, retention policies, tenant isolation, classification, export/import |
| OpenAI executor | Done | polkagent-executor-openai: OpenAI/Azure/vLLM compatible |
| Local executor | Done | polkagent-executor-local: Ollama API adapter |
| PCA transport | Done | polkagent-transport-pca: X25519 + ChaCha20-Poly1305 encrypted peer-to-peer transport |
| OpenGov research | Done | polkagent-tool-governance: referendum, track, voter, delegation, treasury tools |
| Treasury research | Done | polkagent-tool-treasury: portfolio, balance, staking, transfer, vesting analysis |

---

## Phase 4: Controlled Write — NOT STARTED

**Goal:** Carefully gated write operations with family-specific evidence.

---

## Phase 5: Managed + Public — NOT STARTED

**Goal:** Cloud deployment, real-value operations, marketplace.

**Mandatory legal gate before Phase 5 launch.**

---

## Infrastructure Crates

| Crate | Status | Tests | Description |
|-------|--------|-------|-------------|
| `polkagent-health` | Done | 54 | Health aggregation, liveness/readiness/startup probes, dependency checks |
| `polkagent-scheduler` | Done | 84 | Cron expressions, task scheduling, in-memory store, runner |
| `polkagent-cache` | Done | 45 | LRU cache with TTL, cache-aside pattern, BLAKE3 key hashing, stats |
| `polkagent-retry` | Done | 54 | Retry policies, exponential/linear backoff, circuit breaker, bulkhead, timeout |
| `polkagent-batch` | Done | 53 | Batch processing: sequential/parallel execution, error policies, collector |
| `polkagent-plugin` | Done | 94 | TOML manifest loading, capability-based sandboxing, dependency resolution |
| `polkagent-rate-limit` | Done | 50 | Token bucket, sliding window, leaky bucket, composite, keyed, tower middleware |
| `polkagent-audit` | Done | 68 | Append-only audit log with BLAKE3 integrity chain, query builder, formatters |
| `polkagent-context` | Done | 71 | Context assembly, token budget, template engine, truncation strategies |
| `polkagent-migration` | Done | 63 | Standalone migration CLI, BLAKE3 checksums, lock file, generator |
| `polkagent-surface-webhook` | Done | 52 | HMAC-signed webhook delivery, registry, delivery store |

## Build Infrastructure

| Component | Status | Description |
|-----------|--------|-------------|
| CI/CD | Done | `.github/workflows/ci.yml` (check, test, MSRV), `nightly.yml` (audit, deny, fuzz) |
| Docker | Done | `Dockerfile`, `Dockerfile.api`, `docker-compose.yml`, `docker-compose.dev.yml` |
| Makefile | Done | build, test, check, lint, fmt, docker targets |
| Policy fixtures | Done | `fixtures/policies/` — default-deny, read-only, developer, operator, time-limited |
| Config fixtures | Done | `fixtures/configs/` — minimal, full, invalid |
| Security docs | Done | `SECURITY.md`, `CONTRIBUTING.md`, `ARCHITECTURE.md` |
| cargo-deny | Done | `deny.toml` — license/advisory config |
| Eval corpus | Done | polkagent-eval with builtin safety corpus and regression detection |
| OpenAPI spec | Done | Wired through polkagent-api, auto-documented routes |
| Benchmarks | Done | Store benchmarks, effect pipeline benchmarks |

---

## Workspace Summary

| Metric | Count |
|--------|-------|
| Crates | 62 |
| Workspace members | 62 |
| Tests | 4,893 |
| Lines of Rust | ~207,000 |

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

# Run security tests
cargo test -p polkagent-security-tests

# Run fuzz targets
cargo +nightly fuzz run fuzz_config -- -max_total_time=60

# Build release binary
cargo build --release -p polkagent-cli
```

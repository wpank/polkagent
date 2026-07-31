# Architecture

This document describes the high-level architecture of Polkagent: a Rust-first,
Polkadot-native platform for building, using, and publishing AI agents.

## System Overview

Polkagent is organized as a Cargo workspace of ~29 crates following a
**hexagonal (ports and adapters) architecture**. Domain logic lives in pure
crates with no I/O dependencies. External systems are accessed through narrow
trait-based ports, with concrete adapters provided separately.

```
                          +-----------+
                          |    User   |
                          +-----+-----+
                                |
                    +-----------+-----------+
                    |  Surfaces (CLI / API) |
                    +-----------+-----------+
                                |
              +-----------------+-----------------+
              |          Application Core         |
              |  +-----------+  +-------------+   |
              |  |    Run    |  |    Grant     |   |
              |  |  Manager  |  |   Resolver   |   |
              |  +-----+-----+  +------+------+   |
              |        |               |           |
              |  +-----+-----+  +-----+-----+     |
              |  |  Effect   |  |   Event    |     |
              |  |  Pipeline |  |    Bus     |     |
              |  +-----+-----+  +-----+-----+     |
              |        |               |           |
              |  +-----+-----+  +-----+-----+     |
              |  |  Artifact |  |   Outbox   |     |
              |  |  Store    |  |            |     |
              |  +-----------+  +-----------+     |
              +-----------------+-----------------+
                                |
              +-----------------+-----------------+
              |              Ports                |
              |  (executor, signer, store,        |
              |   chain, transport traits)        |
              +-----------------+-----------------+
                                |
              +-----------------+-----------------+
              |            Adapters               |
              |  SQLite  Anthropic  OpenAI  Fake  |
              |  Ollama  External-Signer  ...     |
              +-----------------+-----------------+
                                |
              +-----------------+-----------------+
              |         External Systems          |
              |  LLM APIs  Polkadot  Filesystem   |
              +-----------------------------------+
```

## Hexagonal Architecture

The architecture enforces strict dependency rules:

- **Domain crates** (`polkagent-core`, `polkagent-run`, `polkagent-effect`,
  `polkagent-grant`, `polkagent-event`, `polkagent-artifact`,
  `polkagent-outbox`, `polkagent-card`, `polkagent-config`) contain business
  logic and domain types. They depend only on other domain crates and
  general-purpose libraries (serde, uuid, chrono, etc.). They never depend on
  adapters or I/O libraries.

- **Port crates** (`polkagent-store-trait`, `polkagent-executor-trait`,
  `polkagent-signer-trait`, `polkagent-chain-trait`,
  `polkagent-transport-trait`) define the async trait interfaces that adapters
  must implement. Each port crate also ships a contract test suite that any
  adapter must pass.

- **Adapter crates** (`polkagent-store-sqlite`, `polkagent-executor-anthropic`,
  `polkagent-executor-openai`, `polkagent-executor-local`,
  `polkagent-executor-fake`, `polkagent-signer-fake`,
  `polkagent-signer-external`, `polkagent-transport-fake`) implement ports
  against real or fake external systems.

- **Surface crates** (`polkagent-api`, `polkagent-cli`) wire domain logic to
  user-facing interfaces (HTTP/WebSocket API, terminal UI).

- **Test crates** (`polkagent-test-fixtures`, `polkagent-integration-tests`)
  provide shared builders, factory functions, and cross-crate integration tests.

## Crate Dependency Diagram

```
polkagent-cli ──────────────────┐
polkagent-api ──────────────────┤
                                ▼
                    polkagent-run
                   /     |       \
                  ▼      ▼        ▼
     polkagent-effect  polkagent-event  polkagent-grant
          |              |                  |
          ▼              ▼                  ▼
     polkagent-core  polkagent-core   polkagent-core
          |
          ▼
     polkagent-artifact
          |
          ▼
     polkagent-outbox

Port traits (no domain deps):
     polkagent-store-trait
     polkagent-executor-trait
     polkagent-signer-trait
     polkagent-chain-trait
     polkagent-transport-trait

Adapters (implement ports):
     polkagent-store-sqlite ──────► polkagent-store-trait
     polkagent-executor-anthropic ► polkagent-executor-trait
     polkagent-executor-openai ──► polkagent-executor-trait
     polkagent-executor-local ───► polkagent-executor-trait
     polkagent-executor-fake ────► polkagent-executor-trait
     polkagent-signer-fake ──────► polkagent-signer-trait
     polkagent-signer-external ──► polkagent-signer-trait
     polkagent-transport-fake ───► polkagent-transport-trait

Supplementary:
     polkagent-config      (TOML loader, env overrides)
     polkagent-card        (action card rendering)
     polkagent-metadata    (Polkadot metadata cache)
     polkagent-memory      (episodic / semantic memory)
     polkagent-test-fixtures
     polkagent-integration-tests
```

## Key Invariants

These properties hold in every configuration, deployment mode, and autonomy
level. All contributions must preserve them.

### INV-01: Signer Isolation

The signer never sees model-modified data. Only user-approved, integrity-checked
bytes reach the signing boundary. The model may explain, annotate, and
summarize, but the canonical payload that enters the signer is constructed from
verified chain data (decoded extrinsic, metadata-checked fields). This prevents
a compromised or hallucinating model from altering transaction semantics.

### INV-02: Effect Intent Before I/O

An `EffectIntent` is durably persisted BEFORE any corresponding I/O is
attempted. If the process crashes between persisting the intent and completing
the I/O, recovery can detect the incomplete intent and decide whether to retry
or mark it as unknown -- but it will never silently skip or duplicate the
operation.

### INV-03: No Silent Duplicate Effects

A crash never silently repeats an irreversible external action. The effect
pipeline uses idempotency keys, claim/lease semantics, and outcome recording to
ensure that each effect is attempted at most once per attempt record. If a crash
occurs mid-flight, the outcome is recorded as `Unknown` and requires explicit
resolution.

### INV-04: Unknown Stays Unknown

An `EffectOutcome::Unknown` is never automatically collapsed to `Success` or
`Failure`. It remains visibly unknown in all projections, APIs, and UIs until a
human or automated reconciliation process explicitly resolves it. This prevents
optimistic assumptions about the state of irreversible operations.

## Data Flow

A typical request flows through the system as follows:

```
User Request
    │
    ▼
Surface (CLI / API)
    │  parse input, create RunRequest
    ▼
RunManager
    │  create Run (state: Created → Running)
    │  create Turn
    ▼
Grant Resolver
    │  resolve permissions for this run
    │  check capabilities, budget limits
    ▼
Executor Port (model inference)
    │  send prompt to LLM
    │  receive streaming response
    │  extract tool calls / effect requests
    ▼
Effect Pipeline
    │  for each requested effect:
    │    1. Create EffectIntent (persist BEFORE I/O)
    │    2. Check grant gates (approve / deny / escalate)
    │    3. Claim and lease the effect
    │    4. Execute via appropriate port adapter
    │    5. Record EffectOutcome (Success / Failure / Unknown)
    ▼
Event Bus
    │  emit lifecycle events (RunStarted, TurnCompleted,
    │  EffectResolved, TokensStreamed, etc.)
    ▼
Artifact Store
    │  persist content-addressed artifacts with
    │  BLAKE3 digests and lineage (parent → child)
    ▼
Outbox
    │  durable ordered delivery of pending effects
    │  deduplication by idempotency key
    ▼
Projections
    │  derive read models for surfaces
    ▼
Surface (CLI / API)
    │  render results to user
    ▼
User Response
```

## Testing Strategy

The project uses a layered testing approach:

- **Unit tests** in each crate for domain logic.
- **Property-based tests** (proptest) for invariant verification in core,
  effect, and grant crates.
- **Port contract tests** shipped with each `-trait` crate; every adapter must
  pass them.
- **Integration tests** in `polkagent-integration-tests` for cross-crate
  lifecycle flows.
- **Fuzz targets** (planned) for parsing and deserialization boundaries.

See `CONTRIBUTING.md` for instructions on running each test category.

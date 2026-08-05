# ADR-002: Shared Runtime and Interaction Semantics

**Status:** accepted

**Date:** 2026-08-05

**Owners:** runtime, interaction, CLI/TUI, API, and ACP maintainers

## Context

Polkagent grew several executable surfaces around capable domain libraries:
one-shot CLI runs, a monitoring TUI, HTTP/WebSocket serving, and an ACP agent
server for editors. Each surface assembled a different subset of providers,
stores, tools, recovery, and configuration. A prompt therefore had different
durability and safety properties depending on where it originated.

The user-facing session model was also implicit. Runs, conversations, terminal
output, approvals, and editor updates exposed overlapping but non-identical
identities. This made multi-turn resume, cross-surface observation, reliable
cancellation, and truthful slash commands impossible to implement once and
reuse.

## Decision

Polkagent has one production composition root and one surface-neutral
interaction boundary.

### Production composition

`polkagent-runtime` owns concrete production construction. `RuntimeFactory`
loads and validates configuration, opens and migrates the durable store,
selects adapters, constructs one event bus and `AppService`, rehydrates agents,
performs startup recovery, and returns structured readiness. It never writes
protocol output.

Executable surfaces receive a `PolkagentRuntime` handle. They must not quietly
reconstruct providers, fake chain clients, stores, or `AppService` instances.
Simulation is an explicit adapter policy and is reported as degraded
readiness; it is not a production fallback.

### Interaction boundary

`InteractionService` is the only application boundary for human/editor
interaction mutations. Terminal chat, the Ratatui Console, HTTP/WebSocket, ACP,
and future transports are adapters over the same operations and projections.

The durable identity hierarchy is:

```text
ConversationId (interaction/session)
  -> InteractionTurnId (one human prompt and its orchestration)
       -> one or more RunId values with stable roles
            -> executor turns, steps, effects, artifacts, and run events
```

The user message is durable before execution. Run correlation exists before a
run can emit user-visible work. Assistant/tool state and exactly one terminal
interaction event are durable. No database transaction remains open across
model or tool execution.

### Persistence and delivery

`InteractionStore` is below `InteractionService`; surfaces do not call it
directly. The SQLite adapter stores safe session defaults, turn/run links, and
structured event envelopes. Credentials and raw provider payloads are not
interaction metadata.

Event sequence numbers are monotonic within a conversation. Live delivery is
bounded and happens only after durable append. A lagging receiver gets an
explicit durable recovery checkpoint; it must reload/replay rather than
silently accepting a gap.

### Commands

One `CommandRegistry` defines parsing, aliases, help, completion metadata,
availability, and mutability. `ServiceCommandExecutor` implements the typed MVP
handlers against `InteractionService` and a small runtime read-model port.
Adapters may expose only the commands they can execute truthfully. They must
not advertise a durable `/resume`, `/runs`, or approval operation backed by
ephemeral editor-local state.

### Protocol and terminal safety

ACP stdout is reserved for JSON-RPC. Readiness, logs, panics, and diagnostics
use structured return values, stderr, or a redacted file sink. TUI setup owns a
drop-backed restoration guard from the first terminal mutation and attempts
every cleanup operation even after a partial failure.

## Consequences

- CLI, TUI, API, and ACP migrations can be incremental, but temporary
  composition duplication is explicitly tracked and cannot be called parity.
- `serve` needs durable adapters for its API-specific agent and run ports before
  it can consume the runtime without retaining in-memory authority.
- The shared event/command contracts improve every surface, but they do not
  make the orchestrator execute tools; that remains the execution-loop packet.
- Group orchestration maps multiple role-bearing runs to one interaction turn
  without changing surface protocols.
- Read-only mode must ultimately be enforced by application services, not only
  HTTP middleware or UI affordances.
- Schema changes are forward-only. Existing migrations are never rewritten.

## Current implementation boundary

Accepted does not mean the migration is complete. As of this decision:

- `polkagent-runtime` exists with SQLite composition, readiness, recovery, and
  rehydration; executable surfaces are being migrated onto it;
- interaction contracts, SQLite session/turn/event persistence, bounded
  durable/live replay, and typed command handlers exist;
- the full durable `InteractionService` prompt lifecycle and transcript
  projection are not yet implemented;
- TUI and ACP still need to consume that service for durable multi-turn parity;
- `serve` still needs durable API port adapters;
- real tool/effect/policy execution remains a separate critical-path decision.

## Rejected alternatives

- **A runtime per surface:** repeats the configuration and durability drift that
  caused this decision.
- **The database as a command bus:** bypasses authorization, orchestration, and
  transactional invariants.
- **ACP types as the domain model:** couples terminal/API behavior to one editor
  protocol and prevents other surfaces from sharing semantics.
- **Unbounded live channels:** trades explicit recovery for memory growth and
  still cannot guarantee delivery after process failure.
- **Silent fake adapters:** makes readiness and safety claims depend on which
  executable path happened to construct the service.

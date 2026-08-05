# Runtime-composed HTTP server

Status: implemented for `polkagent serve` on 2026-08-05.

`polkagent serve` builds one `RuntimeFactory` / `PolkagentRuntime` and keeps it
alive for the full HTTP server lifetime. The runtime is the composition root
for configuration, SQLite migration, startup recovery, agent rehydration,
execution adapters, `AppService`, and the event bus. The API does not construct
in-memory agent or run stores.

## Composed dependencies

| API dependency | Concrete source | Persistence/restart behavior |
|---|---|---|
| `AgentStore` | `RuntimeAgentStore` over the runtime pool and `AppService` | Full `AgentSpec` survives restart; writes update the live service registry |
| `RunManagerTrait` | `RuntimeRunManager` over the runtime pool and `AppService` | Run input and lifecycle survive restart; mutations use the service façade |
| `EffectStore` | Runtime `SqlitePool` | Durable |
| `EventStore` | Runtime `SqlitePool` | Durable |
| `ArtifactStore` | `SqliteApiArtifactStore` over the runtime pool | Durable metadata, verified BLAKE3 content, classification, and lineage |
| `ToolRegistryStore` | Read-only projection of `AppService::tool_registry()` | Exact process-wide registry; deterministic list, or an empty view when chain-backed registration is disabled |
| `PaymentStore` | Runtime `SqlitePool` | Durable |
| `ConversationStore` | Runtime `SqlitePool` | Durable |
| `InteractionService` | The exact `Arc` returned by `PolkagentRuntime::interactions()` | Durable prompts, turns, transcripts, correlation, cancellation, and recovery use the process-wide runtime |
| `InteractionStore` | `SqliteInteractionStore` over the runtime pool | Durable ordered event replay survives HTTP-server restart |
| `EventBus` | Runtime event bus | Process-local live stream paired with the durable event recorder |

The API config starts as a clone of the runtime's resolved config. The
`--cors-origin` override is applied only to that HTTP-surface clone.
`--read-only` is passed into `RuntimeOptions` as well as reflected in the API
middleware configuration. Auth, rate limits, and all other API configuration
therefore retain their resolved values. `--host` and `--port` continue to own
the bind address.

Daemon startup uses the runtime's strict adapter policy: a configured provider
or discovered harness must be usable. It does not silently install a fake
executor. On SIGINT or SIGTERM, Axum drains active HTTP connections while the
runtime remains alive. `AppService` still lacks an explicit shutdown handle for
its timeout-enforcer task; runtime readiness reports that upstream limitation.

### Persisted run-state grammar

The `runs.state` projection uses `RunState`'s display encoding: fixed tags for
unit states, `awaiting_approval:<request-id>`,
`waiting_effect:<comma-separated-effect-ids>`, `failed:<reason>`, and
`cancelled:<reason>`. `timed_out` is a unit state and has no reason-bearing
form. Startup recovery now records `failed:recovered after restart`, retaining
the cause while SQLite preserves the terminal timestamp.

For databases created by older releases, both the kernel and API projection
also accept bare `failed` and `cancelled`. They surface an explicit legacy
"reason unavailable" value rather than failing the whole run projection.
Unknown tags, malformed effect IDs, and reason-bearing `timed_out` values fail
closed as internal errors; HTTP responses do not echo the stored value.

## Durable interaction HTTP surface

The agent-execution API is exposed under `/api/v1alpha1/interactions`. It uses
the exact `InteractionService` instance exposed by the runtime rather than
reconstructing a service or mutating conversation rows directly. Migration of
other surfaces onto that shared service is tracked separately.

| Method | Route | Contract |
|---|---|---|
| `POST` | `/interactions` | Create a durable interaction for an agent target |
| `GET` | `/interactions` | List durable interaction projections |
| `GET` | `/interactions/{id}` | Load one interaction |
| `DELETE` | `/interactions/{id}` | Archive an interaction after all work is terminal |
| `GET` | `/interactions/{id}/turns` | List durable turn projections |
| `POST` | `/interactions/{id}/prompt` | Atomically persist and execute a text prompt; accepts a caller-generated `turn_id` for idempotency |
| `POST` | `/interactions/{id}/turns/{turn_id}/cancel` | Cancel the path-scoped turn idempotently |
| `PUT` | `/interactions/{id}/target` | Change only the supported agent target |
| `GET` | `/interactions/{id}/events` | Return a finite ordered page after a durable sequence checkpoint |
| `GET` | `/interactions/{id}/events/stream` | Replay and follow the exact bounded service stream over checkpointed SSE |

Prompt responses are `202 Accepted` once the turn and its initial event are
durable. Retrying the same `turn_id` with the same prompt returns the same turn
handle; reusing it for different input is `409 Conflict`. Model/provider
overrides and approval/denial are deliberately absent because the runtime does
not yet support those operations end to end.

Event replay accepts `after_sequence`, optional `turn_id`, and `limit` query
parameters. Events are strictly ordered by the conversation sequence. The
response checkpoint's `next_after_sequence` is safe to send on the next
request, including when a turn filter skips unrelated events; `has_more`
indicates that another matching event is already durable.

The SSE route accepts `after_sequence`, optional `turn_id`, and the standard
`Last-Event-ID` header. A valid header takes precedence over the query
checkpoint. Data events are named `interaction_event`; their SSE IDs are the
durable conversation sequences and their JSON data are typed interaction event
envelopes. On bounded-channel lag, the API resubscribes after the last sequence
it emitted, so durable replay closes the gap without duplicates. Terminal turn
events leave the stream open for later turns, and dropping the HTTP connection
drops the owned service receiver. Backend or closed-stream failures end the
connection without serializing backend messages; server logs record only the
typed error code. The older `/events/stream` and `/ws/v1alpha1` transports
remain separate run-event protocols.

`InteractionEventHub::subscribe` attaches the bounded live receiver before any
replay read, then the returned stream loads at most one bounded durable page at
a time as its consumer calls `recv`. This keeps old-checkpoint reconnects
storage-bounded without losing publications that race with replay; dropping the
stream prevents any later replay pages from being loaded.

The `/conversations` API is a low-level transcript compatibility surface.
In particular, `POST /conversations/{id}/messages` appends a record only: it
does not invoke the interaction service, start an agent, or produce interaction
events. New execution clients use `/interactions/{id}/prompt`.

## Explicit 501 boundary

The following routes remain registered but intentionally have no substitute
store. They return `501 Not Implemented`. The machine-readable source of truth
is `polkagent_api::RUNTIME_UNAVAILABLE_ROUTES`.

| Dependency | Method | Route | Missing boundary |
|---|---|---|---|
| Skills | `GET` | `/api/v1alpha1/skills` | Runtime skill runner has no API `SkillRegistry` adapter |
| Skills | `POST` | `/api/v1alpha1/skills/install` | Same missing adapter |
| Skills | `GET` | `/api/v1alpha1/skills/{skill_id}` | Same missing adapter |
| Skills | `POST` | `/api/v1alpha1/skills/{skill_id}/uninstall` | Same missing adapter |
| Skills | `PUT` | `/api/v1alpha1/skills/{skill_id}/config` | Same missing adapter |
| Memory | `POST` | `/api/v1alpha1/memory/query` | Runtime memory store has no API `MemoryStore` adapter |
| Memory | `GET` | `/api/v1alpha1/memory/stats` | Same missing adapter |
| Memory | `POST` | `/api/v1alpha1/memory/forget` | Same missing adapter |
| Memory | `GET` | `/api/v1alpha1/memory/entries/{entry_id}` | Same missing adapter |
| Audit | `GET` | `/api/v1alpha1/audit` | `RuntimeFactory` does not compose an `AuditStore` |
| Audit | `GET` | `/api/v1alpha1/audit/verify` | Same missing store |
| Audit | `GET` | `/api/v1alpha1/audit/{id}` | Same missing store |
| Service registry | `POST` | `/api/v1alpha1/registry/listings` | `RuntimeFactory` does not compose a `ServiceRegistryStore` |
| Service registry | `GET` | `/api/v1alpha1/registry/listings/{id}` | Same missing store |
| Service registry | `GET` | `/api/v1alpha1/registry/search` | Same missing store |

## Next implementation slices

1. Add a read-only query adapter for the runtime skill runner; decide
   separately whether install/config/uninstall belong in a production daemon.
2. Define one memory port shared by the API and `polkagent-memory`, then add
   query, statistics, entry lookup, and deletion contract tests.
3. Compose durable audit and service-registry stores in `RuntimeFactory` before
   enabling those routes.
4. Expose explicit runtime shutdown and await background-task termination
   after HTTP connection draining.

Artifact projection is now closed by a forward V15 migration and a strict
adapter in `polkagent-store-sqlite`. Existing rows receive the only truthful
legacy defaults (`blake3`, `public`). New rows preserve algorithm and
classification. The adapter accepts only BLAKE3, validates the supplied digest
before writing, verifies digest and byte length on every content read, rejects
`secret_forbidden` writes/projections, and fails distinctly for absence,
corrupt projections/content, conflicts, connection failures, and other backend
errors. Lineage uses deterministic breadth-first traversal and survives
restart. The current API artifact list contract is intentionally unpaginated;
it returns the complete run-scoped list in creation order.

Artifact metadata, content, provenance, and run-scoped listing are safe `GET`
operations and remain available when `--read-only` is enabled. They still pass
through the server's normal API-key middleware; enabling auth requires a valid
Bearer or `X-Api-Key` credential before any artifact classification is
projected.

Tool metadata is projected directly from the registry already owned by
`AppService`; the API never creates a second mutable production registry.
Configured tools retain their input JSON Schemas, grant patterns, and
snake-case output classifications. Listing is sorted by tool ID so responses
are deterministic despite the registry's internal hash-map order. When
chain-backed tool registration is disabled, the same API routes truthfully
return an empty list or `404` for a named lookup. These `GET` routes remain
available in read-only mode and retain the normal API authentication boundary.

The black-box test `durable_runtime_api` constructs the server through the same
runtime composition helper used by `serve`, creates an agent and a run over
HTTP, rebuilds the runtime on the same database, and verifies both projections
after restart. It also verifies the composed optional stores and representative
`501` responses.

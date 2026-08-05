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
| `PaymentStore` | Runtime `SqlitePool` | Durable |
| `ConversationStore` | Runtime `SqlitePool` | Durable |
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
| Tools | `GET` | `/api/v1alpha1/tools` | Runtime tool registry has no API `ToolRegistryStore` adapter |
| Tools | `GET` | `/api/v1alpha1/tools/{tool_id}` | Same missing adapter |
| Tools | `GET` | `/api/v1alpha1/tools/{tool_id}/grants` | Same missing adapter |
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

1. Add read-only query adapters for the runtime tool registry and skill
   runner; decide separately whether install/config/uninstall belong in a
   production daemon.
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

The black-box test `durable_runtime_api` constructs the server through the same
runtime composition helper used by `serve`, creates an agent and a run over
HTTP, rebuilds the runtime on the same database, and verifies both projections
after restart. It also verifies the composed optional stores and representative
`501` responses.

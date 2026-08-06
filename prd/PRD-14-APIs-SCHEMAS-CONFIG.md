# PRD-14 -- APIs, Schemas, Configuration and Migration

> **Implementation note (audited 2026-08-05):** This PRD remains normative,
> but its embedded implementation statements and checklists are not current
> status evidence. Use [STATUS.md](STATUS.md) and
> [IMPLEMENTATION-BACKLOG.md](IMPLEMENTATION-BACKLOG.md) for verified state and
> the dependency-ordered execution queue.

**Status:** definitive PRD
**Audience:** engineers, architects, integrators, and operators with no prior
Polkagent context
**Date:** 2026-07-30
**Depends on:** PRD-02 (vocabulary/architecture), PRD-03 (execution model),
PRD-04 (providers/harnesses/tools), PRD-06 (PCA compatibility), PRD-10
(data/artifacts/events), PRD-11 (cloud/tenancy)

---

## 1. Purpose and orientation

This document defines every public and internal API surface, wire-level data
schema, configuration contract, and migration path for the Polkagent platform.
It is the reference that SDK authors, adapter implementors, operator tooling,
and future PRD writers use to understand how external consumers and internal
subsystems communicate.

### 1.1 What this document covers

- **Public APIs** that external clients, SDKs, and UIs consume (REST, WebSocket,
  optional gRPC).
- **Internal service APIs** that connect the execution engine, effect processor,
  signer service, event bus, and run manager.
- **Wire-level DTOs** -- the versioned JSON structures that cross every boundary.
- **Database schemas** for the authority store (SQLite v1, PostgreSQL managed).
- **Configuration schema** -- the TOML/YAML hierarchy from defaults through
  per-run overrides.
- **SDK design** for Rust and TypeScript consumers.
- **PCA migration** -- state import, config translation, identity migration, and
  rollback.
- **API versioning, deprecation, and feature-flag policies.**
- **Integration testing contracts** -- contract test framework and provider/consumer
  tests.
- **OpenAPI skeleton** for the public REST surface.

### 1.2 What this document does not cover

- Internal Rust trait signatures for kernel ports (PRD-02, PRD-03).
- UX wireframes and interaction flows (PRD-13).
- Security threat model details (PRD-07, PRD-15).
- Deployment topology and infrastructure (PRD-11).

### 1.3 Reader orientation

Polkagent is a Rust-first, Polkadot-native agent platform with three pillars:
**Build** (Polkadot product engineering), **Act** (payments and on-chain actions),
and **Reach** (PCA-compatible mobile/chat). Its architecture separates a small
durable kernel from replaceable adapters for providers, harnesses, tools,
transports, signers, chain clients, and stores.

All APIs described here serve that architecture. The public API is a projection
of kernel state; the internal APIs are typed contracts between subsystems. No
API bypasses the kernel's policy evaluation, grant resolution, or effect
lifecycle.

### 1.4 Key terms

| Term | Meaning |
|---|---|
| **DTO** | Data transfer object: a versioned wire-level data shape. |
| **Resource** | A first-class API entity (agent, run, effect, artifact, event). |
| **Projection** | A read-optimized view derived from kernel state. |
| **Wire format** | The serialized representation crossing a network boundary. |
| **Schema version** | An explicit version tag in every configuration and DTO. |
| **Profile** | A pinned chain/network identity with genesis, spec version, metadata hash. |

---

## 2. API design principles

### 2.1 Versioned from day one

**Requirement API-PRINC-001.** Every public and internal API carries an explicit
version identifier. The initial version is `v1alpha1`. Version appears in:

- URL path prefix for REST: `/api/v1alpha1/...`
- WebSocket connection path: `/ws/v1alpha1`
- gRPC package namespace: `polkagent.v1alpha1`
- Configuration file `apiVersion` field
- DTO envelope `version` field

**Requirement API-PRINC-002.** Multiple API versions may be served
simultaneously. The server advertises supported versions at `GET /api/versions`.

### 2.2 Schema-first design

**Requirement API-PRINC-003.** All DTOs are defined in a canonical schema
repository (`polkagent-schemas`) before implementation. Rust types, TypeScript
types, OpenAPI specs, and JSON Schema files are generated from or validated
against this source. The repository layout is:

```
polkagent-schemas/
  ├── json-schema/         # Canonical JSON Schema (draft 2020-12) definitions
  ├── openapi/             # OpenAPI 3.1 specification derived from JSON Schema
  └── generators/
      ├── rust/            # Rust type generation (serde + validation)
      └── typescript/      # TypeScript type generation (zod)
```

**Requirement API-PRINC-003a.** JSON Schema is the single source of truth.
OpenAPI 3.1 components reference the JSON Schema files directly (`$ref` into
`json-schema/`). Rust and TypeScript types are auto-generated from JSON Schema;
hand-written types for shared DTOs are prohibited.

**Requirement API-PRINC-004.** Every DTO includes:
- A `version` field (e.g., `"v1alpha1"`).
- A `kind` field where polymorphism exists (e.g., `"RunEvent"`, `"EffectOutcome"`).
- Stable, snake_case field names in JSON wire format.
- Required fields are never nullable; optional fields use explicit `null` or
  absence.

**Requirement API-PRINC-004a.** Polymorphic types use JSON Schema discriminated
unions: a required `kind` field (string literal) is the discriminator, and each
variant is a separate `$defs` entry. This pattern is consistent with how serde
`#[serde(tag = "kind")]` works in Rust and with TypeScript discriminated union
narrowing. Example:

```json
{
  "$defs": {
    "EffectIntent": {
      "oneOf": [
        { "$ref": "#/$defs/ChainTransferIntent" },
        { "$ref": "#/$defs/ToolInvocationIntent" }
      ],
      "discriminator": { "propertyName": "kind" }
    },
    "ChainTransferIntent": {
      "properties": { "kind": { "const": "ChainTransfer" }, "..." }
    }
  }
}
```

### 2.3 Backwards compatibility policy

**Requirement API-PRINC-005.** Within a version:
- New optional fields may be added.
- New enum variants may be added (clients must handle unknown variants).
- Existing fields must not change type, meaning, or requiredness.
- Existing fields must not be removed or renamed.

Breaking changes require a new API version. The old version continues to be
served for at least two minor platform releases after the new version reaches
`stable`.

### 2.4 Rate limiting and pagination

**Requirement API-PRINC-006.** All list endpoints use cursor-based pagination
with a consistent envelope. Cursor pagination is preferred over offset
pagination for large, append-only datasets (runs, events, effects) because
offsets become inconsistent under concurrent writes.

```json
{
  "data": [...],
  "cursor": {
    "next": "opaque-cursor-token",
    "has_more": true
  },
  "meta": {
    "page_size": 50,
    "total_estimate": 1234
  }
}
```

**Requirement API-PRINC-007.** Rate limits are expressed via standard headers:
- `X-RateLimit-Limit`: requests per window
- `X-RateLimit-Remaining`: remaining in current window
- `X-RateLimit-Reset`: UTC epoch seconds when window resets
- `Retry-After`: seconds to wait (on 429 responses)

Default limits are configurable per deployment. Managed cloud applies
tenant-scoped limits.

### 2.5 Error format

**Requirement API-PRINC-008.** All API errors use a consistent envelope:

```json
{
  "error": {
    "code": "EFFECT_DENIED",
    "message": "Human-readable explanation",
    "details": {
      "effect_id": "eff_abc123",
      "policy_revision": "pol_rev_7"
    },
    "request_id": "req_xyz789",
    "timestamp": "2026-07-30T12:00:00Z"
  }
}
```

HTTP status codes follow RFC 9110. Domain-specific error codes use
SCREAMING_SNAKE_CASE and are documented in the schema repository.

### 2.6 Authentication

**Requirement API-PRINC-009.** The public API supports multiple authentication
methods:

| Method | Use case | Mechanism |
|---|---|---|
| API key | CLI, SDK, automation | `Authorization: Bearer pak_...` header |
| OAuth 2.0 + PKCE | Web UI, third-party apps | Standard OAuth flow with PKCE |
| Agent identity token | Agent-to-platform | JWT signed by agent's bound keypair |
| Session cookie | Browser sessions | Secure, HttpOnly, SameSite=Strict |

**Requirement API-PRINC-010.** API keys are scoped: each key declares allowed
operations, target resources, IP allowlist, and expiry. Keys are revocable
without invalidating other keys.

**Requirement API-PRINC-011.** Every authenticated request carries a resolved
principal (user, agent, service account, API key) through the request context.
Authorization decisions reference the principal and applicable policies.

**Requirement API-PRINC-012.** API keys are scoped per capability (e.g.,
`runs:write`, `effects:write`, `payments:write`). A key cannot be used to
perform operations outside its declared scope even if the associated principal
would otherwise have access. Scope is checked at the API gateway layer before
reaching domain logic.

### 2.7 Streaming runs (SSE) and webhooks

**Requirement API-PRINC-013.** Long-running agent runs support two real-time
delivery modes for event streams:

| Mode | Mechanism | Use case |
|---|---|---|
| **Server-Sent Events (SSE)** | `GET /runs/{run_id}/stream` returning `text/event-stream` | Browser and CLI consumers that hold an open HTTP connection |
| **WebSocket subscription** | `ws://{host}/ws/{version}`, channel `runs:{run_id}` | Bidirectional, multi-resource subscriptions |

SSE is the preferred transport for "watch a single run" use cases (analogous to
Anthropic's streaming messages API and Stripe's event-stream endpoint). The
event envelope is identical to the WebSocket envelope; only the transport
framing differs.

```
GET /api/v1alpha1/runs/{run_id}/stream
Accept: text/event-stream

data: {"type":"event","id":"evt_01HQ...","channel":"runs:run_01HQ...","payload":{"type":"turn.token_delta","delta":"The staking"}}

data: {"type":"event","id":"evt_01HQ...","channel":"runs:run_01HQ...","payload":{"type":"run.completed"}}
```

**Requirement API-PRINC-014.** Webhooks provide asynchronous push delivery of
lifecycle events to registered HTTP endpoints. Webhook design follows the
Stripe model:

- **Signed payloads:** every delivery includes an `X-Polkagent-Signature`
  header (HMAC-SHA256 of the raw body using the webhook's signing secret).
  Recipients must verify the signature before processing.
- **Automatic retries:** failed deliveries are retried with exponential backoff
  (1 s, 5 s, 30 s, 2 min, 10 min, 1 h) up to a configurable maximum (default
  24 h). Each retry is a fresh HTTP POST with the same payload and a
  monotonically increasing `X-Polkagent-Delivery-Attempt` header.
- **Event filtering:** webhook registrations declare the event types they
  subscribe to. Unsubscribed events are not delivered.
- **Delivery log:** the platform records each delivery attempt (status code,
  latency, attempt number) and exposes them via `GET /webhooks/{id}/deliveries`.

Webhook endpoint management:

| Method | Path | Description |
|---|---|---|
| `POST` | `/webhooks` | Register a new webhook endpoint |
| `GET` | `/webhooks` | List registered webhooks |
| `GET` | `/webhooks/{id}` | Get webhook details |
| `PUT` | `/webhooks/{id}` | Update webhook configuration |
| `DELETE` | `/webhooks/{id}` | Remove webhook |
| `GET` | `/webhooks/{id}/deliveries` | List delivery attempts |
| `POST` | `/webhooks/{id}/test` | Send a test delivery |

**Requirement API-PRINC-015.** Idempotency keys are required (not merely
recommended) on all mutating endpoints that produce financial or blockchain
effects:

- `POST /runs` (when the run may trigger chain actions or payments)
- `POST /payments/intents`
- `POST /effects/{id}/approve`
- `POST /effects/{id}/deny`

For these endpoints, a missing `X-Idempotency-Key` header returns HTTP 422.
The idempotency window is 24 hours. A reused key with different content returns
HTTP 409 (`RESOURCE_CONFLICT`).

---

## 3. Public API surface

### 3.1 REST API

The REST API is the primary public interface. All endpoints live under
`/api/{version}/`. Request and response bodies are JSON (`application/json`).
Binary content uses `multipart/form-data` for upload and direct streaming for
download.

#### 3.1.1 Common request headers

| Header | Required | Purpose |
|---|---|---|
| `Authorization` | Yes (except public endpoints) | Authentication token |
| `X-Request-Id` | Recommended | Client-provided idempotency/correlation key |
| `X-Idempotency-Key` | Required for financial/blockchain POSTs | Ensures at-most-once semantics (see API-PRINC-015) |
| `Accept` | Optional | `application/json` (default) or `text/event-stream` for SSE endpoints |

#### 3.1.2 Common response headers

| Header | Purpose |
|---|---|
| `X-Request-Id` | Echoed or server-generated correlation ID |
| `X-RateLimit-*` | Rate limit status |
| `ETag` | Resource version for conditional requests |
| `Link` | Pagination links (RFC 8288) |

### 3.2 WebSocket API

**Requirement API-WS-001.** The WebSocket endpoint at `/ws/{version}` provides:

- Real-time event streaming for runs, effects, and artifacts.
- Bidirectional messaging for interactive agent sessions.
- Subscription management per resource type and ID.

**Requirement API-WS-002.** WebSocket message envelope:

```json
{
  "msg_type": "event" | "subscribe" | "unsubscribe" | "ping" | "pong" | "error",
  "id": "msg_unique_id",
  "channel": "runs:run_abc123",
  "payload": { ... },
  "timestamp": "2026-07-30T12:00:00Z"
}
```

**Requirement API-WS-003.** Subscription channels:

| Channel pattern | Events |
|---|---|
| `runs:{run_id}` | Run lifecycle, turn progress, streaming tokens |
| `effects:{effect_id}` | Effect state transitions, outcomes |
| `agents:{agent_id}` | Agent status changes, configuration updates |
| `conversations:{conv_id}` | New turns, messages, delivery status |
| `system` | Platform health, maintenance, version announcements |

**Requirement API-WS-004.** Clients authenticate WebSocket connections using
either a query parameter token (`?token=pak_...`) or the first message after
connection (`{"msg_type": "auth", "token": "pak_..."}`).

**Requirement API-WS-005.** The server sends periodic ping frames. If no pong
is received within the configured timeout (default 30s), the connection is
closed. Clients should reconnect with exponential backoff and resume
subscriptions.

**Implementation status (2026-08-06).** `/ws/v1alpha1` implements the
`msg_type` envelope, query/first-message token validation, one-channel
subscribe/unsubscribe commands, WebSocket ping/pong, and live run/agent event
routing with a 256-subscription cap. The `system` channel parses but has no
producer; effects, artifacts, conversations, and interactive prompt/cancel
messages are not implemented on this socket. After its first durable
observation it recovers in-session bus lag from bounded durable store pages,
but the frame contract has no public cursor/checkpoint, so reconnect is
live-only. Appendix A.2 is the authoritative implemented frame contract; the
broader channel/session requirements above remain target scope.

### 3.3 gRPC API (deferred)

**Maturity: deferred.** gRPC is explicitly deferred past Phase 1. REST + SSE +
WebSocket cover all initial use cases without the operational overhead of
maintaining a separate gRPC surface and proto toolchain. gRPC will be
re-evaluated when there is a concrete latency or throughput requirement that
REST cannot meet (e.g., high-frequency managed cloud worker-to-control-plane
communication). Content-negotiation-based versioning is similarly deferred in
favour of URL-path major versioning.

**If adopted:** A gRPC surface would use the same DTOs serialized as Protocol
Buffers. Proto definitions would be generated from the canonical JSON Schema
repository (not hand-written), ensuring Rust and TypeScript SDKs remain the
single source of truth.

---

## 4. Core API resources

Each resource section specifies endpoint patterns, request/response shapes,
query parameters, and lifecycle rules. All IDs use the format
`{type_prefix}_{ulid}` (e.g., `agt_01HQXYZ...`, `run_01HQABC...`).

### 4.1 Agents

An Agent is a versioned definition combining behavior, execution route, tools,
skills, policy, and deployment requirements.

#### Endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/agents` | Create a new agent from an AgentSpec |
| `GET` | `/agents` | List agents (paginated, filterable) |
| `GET` | `/agents/{agent_id}` | Get agent details |
| `PUT` | `/agents/{agent_id}` | Update agent spec (creates new revision) |
| `DELETE` | `/agents/{agent_id}` | Archive agent (soft delete) |
| `POST` | `/agents/{agent_id}/start` | Start agent (begin accepting work) |
| `POST` | `/agents/{agent_id}/stop` | Stop agent (drain and cease) |
| `GET` | `/agents/{agent_id}/status` | Get runtime status and health |
| `GET` | `/agents/{agent_id}/revisions` | List spec revisions |
| `GET` | `/agents/{agent_id}/revisions/{rev}` | Get specific revision |

#### Create agent request

```json
{
  "spec": {
    "apiVersion": "polkagent.dev/v1alpha1",
    "kind": "Agent",
    "metadata": {
      "name": "my-builder-agent",
      "labels": { "team": "runtime-eng" },
      "annotations": {}
    },
    "spec": {
      "description": "Polkadot SDK coding assistant",
      "execution": {
        "route": "recommended-coding-harness",
        "budget": {
          "model_usd_per_run": 5.00,
          "max_turns_per_run": 50
        }
      },
      "skills": [
        { "package": "registry.polkagent.dev/polkadot-sdk-context", "version": "1.2.0" }
      ],
      "tools": {
        "allow": ["filesystem", "shell", "git"],
        "deny": ["network_unrestricted"]
      },
      "chain_profiles": [
        { "ref": "polkadot-production" }
      ],
      "surfaces": [
        { "type": "web_inbox" },
        { "type": "cli" }
      ],
      "autonomy": {
        "mode": "per_action_approval"
      }
    }
  }
}
```

#### Agent response

```json
{
  "id": "agt_01HQ...",
  "spec": { "..." },
  "revision": 1,
  "status": "stopped",
  "created_at": "2026-07-30T12:00:00Z",
  "updated_at": "2026-07-30T12:00:00Z",
  "created_by": "usr_01HQ..."
}
```

#### Query parameters for list

| Parameter | Type | Description |
|---|---|---|
| `status` | string | Filter by status: `running`, `stopped`, `archived` |
| `label` | string | Filter by label key=value |
| `search` | string | Full-text search on name and description |
| `sort` | string | `created_at`, `updated_at`, `name` |
| `order` | string | `asc` or `desc` |
| `cursor` | string | Pagination cursor |
| `page_size` | integer | Items per page (max 100, default 50) |

### 4.2 Runs

A Run is one durable execution instance.

#### Endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/runs` | Create and start a new run |
| `GET` | `/runs` | List runs (paginated, filterable) |
| `GET` | `/runs/{run_id}` | Get run details |
| `POST` | `/runs/{run_id}/cancel` | Cancel a running execution |
| `POST` | `/runs/{run_id}/resume` | Resume a paused/waiting run |
| `GET` | `/runs/{run_id}/turns` | List turns within a run |
| `GET` | `/runs/{run_id}/events` | List events for a run |
| `GET` | `/runs/{run_id}/effects` | List effects for a run |
| `GET` | `/runs/{run_id}/artifacts` | List artifacts for a run |
| `GET` | `/runs/{run_id}/usage` | Get cost and usage summary |

#### Create run request

```json
{
  "agent_id": "agt_01HQ...",
  "input": {
    "type": "user_message",
    "content": "Explain the staking pallet's reward distribution logic",
    "attachments": []
  },
  "context": {
    "conversation_id": "conv_01HQ...",
    "workspace_id": "ws_01HQ...",
    "chain_profile_override": null
  },
  "options": {
    "model_override": null,
    "max_turns": 20,
    "timeout_seconds": 600,
    "idempotency_key": "idem_user123_msg456"
  }
}
```

#### Run response

```json
{
  "id": "run_01HQ...",
  "agent_id": "agt_01HQ...",
  "status": "running",
  "input": { "..." },
  "turns_completed": 0,
  "created_at": "2026-07-30T12:00:00Z",
  "started_at": "2026-07-30T12:00:00.100Z",
  "completed_at": null,
  "terminal_reason": null,
  "usage": {
    "input_tokens": 0,
    "output_tokens": 0,
    "model_cost_usd": 0.0
  },
  "grant_hash": "sha256:abc123..."
}
```

#### Run status values

| Status | Meaning |
|---|---|
| `pending` | Created, not yet executing |
| `running` | Actively executing turns |
| `waiting_approval` | Blocked on human or policy approval |
| `waiting_input` | Blocked on additional user input |
| `paused` | Manually paused |
| `cancelling` | Cancel requested, draining |
| `completed` | Finished successfully |
| `failed` | Terminated with error |
| `cancelled` | Successfully cancelled |
| `timed_out` | Exceeded deadline |

### 4.3 Effects

An Effect represents actual external work: model calls, tool invocations,
signing, submission, and chain actions.

#### Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/effects` | List effects (paginated, filterable) |
| `GET` | `/effects/{effect_id}` | Get effect details |
| `POST` | `/effects/{effect_id}/approve` | Approve a pending effect |
| `POST` | `/effects/{effect_id}/deny` | Deny a pending effect |
| `GET` | `/effects/{effect_id}/attempts` | List attempts for an effect |
| `GET` | `/effects/{effect_id}/outcome` | Get the resolved outcome |

#### Effect response

```json
{
  "id": "eff_01HQ...",
  "run_id": "run_01HQ...",
  "type": "chain_action",
  "status": "pending_approval",
  "intent": {
    "version": "v1alpha1",
    "kind": "ChainTransfer",
    "chain_profile": "polkadot-production",
    "from_account": "5GrwvaEF...",
    "call_data": "0x0500...",
    "decoded": {
      "pallet": "Balances",
      "call": "transferKeepAlive",
      "args": {
        "dest": { "Id": "5FHneW46..." },
        "value": "10000000000"
      }
    },
    "evidence": {
      "metadata_hash": "0xabc...",
      "spec_version": 1003000,
      "genesis_hash": "0x91b171...",
      "block_hash": "0xdef...",
      "dry_run": { "success": true, "events": [...] },
      "fee_estimate": { "partial_fee": "125000000", "asset": "DOT" }
    }
  },
  "policy_decision": {
    "decision": "require_approval",
    "reasons": ["chain_write action requires explicit approval"],
    "policy_revision": "pol_rev_7",
    "grant_hash": "sha256:def456..."
  },
  "created_at": "2026-07-30T12:01:00Z",
  "resolved_at": null,
  "attempts": []
}
```

#### Approve effect request

```json
{
  "approval": {
    "type": "human",
    "principal_id": "usr_01HQ...",
    "comment": "Reviewed and approved",
    "conditions": null
  }
}
```

### 4.4 Artifacts

Artifacts are durable, attributable content or evidence.

#### Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/artifacts` | List artifacts (paginated, filterable) |
| `GET` | `/artifacts/{artifact_id}` | Get artifact metadata |
| `GET` | `/artifacts/{artifact_id}/content` | Download artifact content |
| `POST` | `/artifacts` | Upload a new artifact |
| `GET` | `/artifacts/{artifact_id}/provenance` | Get provenance chain |

#### Artifact metadata response

```json
{
  "id": "art_01HQ...",
  "run_id": "run_01HQ...",
  "type": "code_diff",
  "classification": "workspace_output",
  "content_hash": "sha256:789...",
  "content_type": "text/x-diff",
  "size_bytes": 4096,
  "filename": "staking-fix.patch",
  "provenance": {
    "created_by": "agt_01HQ...",
    "tool": "filesystem.write",
    "turn_id": "turn_01HQ...",
    "parent_artifact_id": null
  },
  "created_at": "2026-07-30T12:02:00Z",
  "retention": {
    "policy": "workspace_default",
    "expires_at": null
  }
}
```

#### Upload artifact request

`POST /artifacts` with `multipart/form-data`:

| Field | Type | Required | Description |
|---|---|---|---|
| `file` | binary | Yes | File content |
| `run_id` | string | Yes | Associated run |
| `type` | string | Yes | Artifact type |
| `classification` | string | No | Data classification |
| `filename` | string | No | Original filename |
| `metadata` | JSON string | No | Additional structured metadata |

### 4.5 Events

Events are ordered observations of lifecycle or streaming activity.

#### Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/events` | List events (paginated, filterable) |
| `GET` | `/events/{event_id}` | Get event details |
| `POST` | `/events/subscribe` | Create a WebSocket subscription |

#### Event response

```json
{
  "id": "evt_01HQ...",
  "run_id": "run_01HQ...",
  "sequence": 42,
  "type": "turn.token_delta",
  "timestamp": "2026-07-30T12:01:05.123Z",
  "payload": {
    "turn_id": "turn_01HQ...",
    "delta": "The staking pallet distributes",
    "token_index": 15
  },
  "correlation_id": "run_01HQ...",
  "causation_id": "evt_01HQ_prev..."
}
```

#### Event types

| Category | Types |
|---|---|
| Run lifecycle | `run.created`, `run.started`, `run.completed`, `run.failed`, `run.cancelled`, `run.timed_out` |
| Turn lifecycle | `turn.started`, `turn.completed`, `turn.failed` |
| Streaming | `turn.token_delta`, `turn.thinking_delta`, `turn.tool_use_start`, `turn.tool_use_end` |
| Effect lifecycle | `effect.created`, `effect.pending_approval`, `effect.approved`, `effect.denied`, `effect.executing`, `effect.succeeded`, `effect.failed`, `effect.unknown` |
| Artifact lifecycle | `artifact.created`, `artifact.updated`, `artifact.deleted` |
| Agent lifecycle | `agent.started`, `agent.stopped`, `agent.config_changed` |
| System | `system.health`, `system.maintenance`, `system.version` |

#### Event query parameters

| Parameter | Type | Description |
|---|---|---|
| `run_id` | string | Filter by run |
| `agent_id` | string | Filter by agent |
| `type` | string | Filter by event type (comma-separated) |
| `after_sequence` | integer | Events after this sequence number |
| `since` | ISO 8601 | Events after this timestamp |
| `until` | ISO 8601 | Events before this timestamp |
| `cursor` | string | Pagination cursor |
| `page_size` | integer | Items per page (max 500, default 100) |

### 4.6 Models and providers

#### Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/providers` | List configured providers |
| `GET` | `/providers/{provider_id}` | Get provider details |
| `POST` | `/providers` | Register a new provider |
| `PUT` | `/providers/{provider_id}` | Update provider configuration |
| `DELETE` | `/providers/{provider_id}` | Remove provider |
| `GET` | `/providers/{provider_id}/models` | List models from provider |
| `GET` | `/models` | List all available models |
| `GET` | `/models/{model_id}` | Get model details and capabilities |
| `POST` | `/models/{model_id}/test` | Test model connectivity |

#### Provider response

```json
{
  "id": "prov_01HQ...",
  "type": "anthropic",
  "name": "Anthropic Production",
  "status": "healthy",
  "capabilities": {
    "streaming": true,
    "tool_use": true,
    "vision": true,
    "extended_thinking": true
  },
  "models": [
    {
      "id": "model_01HQ...",
      "model_id": "claude-opus-4-6",
      "display_name": "Claude Opus 4.6",
      "capabilities": {
        "max_context_tokens": 200000,
        "max_output_tokens": 16384,
        "supports_tool_use": true,
        "supports_vision": true,
        "supports_streaming": true,
        "supports_extended_thinking": true
      },
      "pricing": {
        "input_per_mtok_usd": 15.00,
        "output_per_mtok_usd": 75.00,
        "cache_read_per_mtok_usd": 1.50
      }
    }
  ],
  "health": {
    "last_check": "2026-07-30T12:00:00Z",
    "latency_ms_p50": 250,
    "error_rate_1h": 0.001
  }
}
```

### 4.7 Skills and tools

#### Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/skills` | List installed skills |
| `GET` | `/skills/{skill_id}` | Get skill details |
| `POST` | `/skills/install` | Install a skill from registry |
| `POST` | `/skills/{skill_id}/uninstall` | Uninstall a skill |
| `PUT` | `/skills/{skill_id}/config` | Update skill configuration |
| `GET` | `/tools` | List available tools |
| `GET` | `/tools/{tool_id}` | Get tool schema and capabilities |
| `GET` | `/tools/{tool_id}/grants` | Get current grants for tool |

#### Install skill request

```json
{
  "source": {
    "registry": "registry.polkagent.dev",
    "package": "opengov-monitor",
    "version": "1.4.2",
    "integrity": "sha256:abc..."
  },
  "agent_id": "agt_01HQ...",
  "config_overrides": {}
}
```

### 4.8 Memory

#### Endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/memory/query` | Query memory entries |
| `POST` | `/memory/forget` | Delete memory entries |
| `POST` | `/memory/export` | Export memory as portable archive |
| `GET` | `/memory/stats` | Get memory usage statistics |
| `GET` | `/memory/entries/{entry_id}` | Get a specific memory entry |

#### Memory query request

```json
{
  "agent_id": "agt_01HQ...",
  "query": {
    "text": "staking reward calculation",
    "types": ["episodic", "semantic", "procedural"],
    "time_range": {
      "after": "2026-06-01T00:00:00Z",
      "before": "2026-07-30T00:00:00Z"
    },
    "max_results": 20,
    "min_relevance": 0.7
  },
  "include_provenance": true
}
```

#### Memory query response

```json
{
  "entries": [
    {
      "id": "mem_01HQ...",
      "type": "episodic",
      "content": "User asked about era reward distribution...",
      "relevance_score": 0.92,
      "provenance": {
        "run_id": "run_01HQ...",
        "created_at": "2026-07-15T10:00:00Z",
        "source": "conversation"
      },
      "tags": ["staking", "rewards"],
      "retention": {
        "policy": "agent_default",
        "expires_at": null
      }
    }
  ],
  "cursor": { "next": null, "has_more": false },
  "meta": { "query_time_ms": 45 }
}
```

### 4.9 Payments

#### Endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/payments/intents` | Create a payment intent |
| `GET` | `/payments/intents` | List payment intents |
| `GET` | `/payments/intents/{intent_id}` | Get payment intent details |
| `POST` | `/payments/intents/{intent_id}/cancel` | Cancel a pending intent |
| `GET` | `/payments/receipts` | List payment receipts |
| `GET` | `/payments/receipts/{receipt_id}` | Get receipt details |
| `GET` | `/payments/balance` | Get agent account balance summary |
| `GET` | `/payments/usage` | Get usage and cost summary |

#### Payment intent response

```json
{
  "id": "pay_01HQ...",
  "run_id": "run_01HQ...",
  "type": "chain_transfer",
  "status": "pending_approval",
  "intent": {
    "chain_profile": "polkadot-production",
    "from": "5GrwvaEF...",
    "to": "5FHneW46...",
    "asset": {
      "type": "native",
      "symbol": "DOT",
      "decimals": 10
    },
    "amount": "10000000000",
    "amount_human": "1.0 DOT"
  },
  "evidence": {
    "fee_estimate": "125000000",
    "fee_asset": "DOT",
    "keep_alive_check": true,
    "simulation": { "success": true }
  },
  "policy_decision": {
    "decision": "require_approval",
    "mandate_ref": null
  },
  "created_at": "2026-07-30T12:05:00Z"
}
```

### 4.10 Conversations

#### Endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/conversations` | Create a new conversation |
| `GET` | `/conversations` | List conversations |
| `GET` | `/conversations/{conv_id}` | Get conversation details |
| `POST` | `/conversations/{conv_id}/messages` | Send a message |
| `GET` | `/conversations/{conv_id}/messages` | List messages |
| `DELETE` | `/conversations/{conv_id}` | Archive conversation |

### 4.11 Workspaces

#### Endpoints

| Method | Path | Description |
|---|---|---|
| `POST` | `/workspaces` | Create a workspace |
| `GET` | `/workspaces` | List workspaces |
| `GET` | `/workspaces/{ws_id}` | Get workspace details |
| `GET` | `/workspaces/{ws_id}/files` | List workspace files |
| `GET` | `/workspaces/{ws_id}/files/{path}` | Get file content |
| `DELETE` | `/workspaces/{ws_id}` | Delete workspace |

---

## 5. Internal service APIs

Internal APIs connect kernel subsystems. They are Rust trait-based in-process
or gRPC/message-bus-based in distributed deployments. They are not exposed to
external clients.

### 5.1 Execution engine to effect processor

```
ExecutionEngine ---[EffectRequest]---> EffectProcessor
                <---[EffectResult]---
```

**Requirement API-INT-001.** The execution engine submits effect requests
through a typed channel. The effect processor:

1. Validates the request against the active grant.
2. Evaluates policy to produce a `PolicyDecision`.
3. If approved (or covered by a mandate), creates an `EffectIntent`, claims an
   `EffectAttempt`, executes, and records an `EffectOutcome`.
4. Returns the result to the execution engine.

#### EffectRequest DTO

```json
{
  "run_id": "run_01HQ...",
  "turn_id": "turn_01HQ...",
  "effect_type": "tool_invocation",
  "idempotency_key": "idem_run01_turn03_tool_fs_write_1",
  "payload": {
    "tool_id": "filesystem.write",
    "arguments": { "path": "/workspace/src/lib.rs", "content": "..." }
  },
  "grant_hash": "sha256:abc123...",
  "deadline": "2026-07-30T12:05:00Z"
}
```

### 5.2 Effect processor to signer service

```
EffectProcessor ---[SignRequest]---> SignerService
                <---[SignResult]---
```

**Requirement API-INT-002.** The signer service receives only the canonical
payload and binding data. It never receives model context, conversation history,
or prompt content. The signer response includes the signature and
signer-attested metadata.

#### CanonicalSignRequest DTO

```json
{
  "effect_id": "eff_01HQ...",
  "signer_ref": "signer_vault_01",
  "payload": {
    "chain_profile": {
      "genesis_hash": "0x91b171...",
      "spec_version": 1003000,
      "metadata_hash": "0xabc..."
    },
    "encoded_call": "0x0500...",
    "era": { "mortal": { "period": 64, "phase": 5 } },
    "nonce": 42,
    "tip": "0",
    "additional_signed": { "..." }
  },
  "binding": {
    "intent_id": "eff_01HQ...",
    "approval_id": "apr_01HQ...",
    "policy_revision": "pol_rev_7",
    "grant_hash": "sha256:def456...",
    "payload_hash": "sha256:789..."
  },
  "deadline": "2026-07-30T12:06:00Z"
}
```

### 5.3 Run manager to harness lifecycle

```
RunManager ---[HarnessCommand]---> HarnessProcess
            <---[HarnessEvent]---
```

**Requirement API-INT-003.** Harness lifecycle commands:

| Command | Description |
|---|---|
| `Start` | Initialize harness with configuration and workspace |
| `Execute` | Submit a turn for execution |
| `Resume` | Resume from a checkpoint |
| `Cancel` | Request graceful cancellation |
| `Interrupt` | Request immediate interruption |
| `HealthCheck` | Query harness health and capabilities |
| `Shutdown` | Graceful shutdown with state persistence |

**Requirement API-INT-004.** Harness events use the same normalized event
envelope as kernel events. The harness adapter translates harness-specific
formats into the canonical envelope.

### 5.4 Event bus interfaces

**Requirement API-INT-005.** The internal event bus supports:

| Operation | Description |
|---|---|
| `publish(topic, event)` | Publish an event to a topic |
| `subscribe(topic, filter, handler)` | Subscribe to filtered events |
| `replay(topic, from_sequence)` | Replay events from a sequence |
| `acknowledge(subscription_id, sequence)` | Acknowledge event processing |

**Implementation: SQLite outbox.** In local and self-hosted deployments the
internal event bus is implemented as a durable ordered outbox table in SQLite
(see section 7 for the `events` table schema). The outbox provides:

- **Durability:** events are written in the same transaction as the state change
  that causes them, so an event can never be lost even if the process crashes
  immediately after.
- **Ordering:** the `(run_id, sequence)` unique index guarantees per-run
  ordering without a separate message broker.
- **Replay:** subscribers can replay from any sequence number by querying the
  table.

A background relay process tails the outbox and fans out to in-process
subscribers and the public SSE/WebSocket layer. In managed cloud deployments
the outbox relay can target an external message bus (e.g., NATS, Kafka), but
the outbox write pattern is preserved. This is the transactional outbox
pattern and avoids dual-write inconsistencies.

Topics follow a hierarchical naming convention:
`polkagent.{domain}.{resource_type}.{event_category}`

Examples:
- `polkagent.runs.lifecycle`
- `polkagent.effects.chain_action`
- `polkagent.artifacts.created`
- `polkagent.system.health`

### 5.5 Policy evaluation interface

**Requirement API-INT-006.** The policy evaluator accepts a typed request and
returns a deterministic decision:

```json
{
  "request": {
    "principal": { "type": "agent", "id": "agt_01HQ..." },
    "action": "tool.invoke",
    "resource": { "type": "tool", "id": "filesystem.write" },
    "context": {
      "run_id": "run_01HQ...",
      "workspace_id": "ws_01HQ...",
      "chain_profile": null
    }
  },
  "decision": {
    "result": "allow",
    "reasons": ["agent spec allows filesystem tools in workspace scope"],
    "policy_revision": "pol_rev_7",
    "grant": {
      "hash": "sha256:abc...",
      "capabilities": ["filesystem.read", "filesystem.write"],
      "constraints": {
        "paths": ["/workspace/**"],
        "max_file_size_bytes": 10485760
      },
      "expires_at": null
    }
  }
}
```

---

## 6. Configuration schema

### 6.1 File format and location

**Requirement CFG-001.** Configuration files use TOML exclusively. YAML is
not supported. Rationale: TOML is the idiomatic Rust configuration format
(`config` crate, `toml` crate), has a strict type system that maps cleanly to
Rust structs, and avoids the YAML footguns (implicit type coercion, significant
indentation, Norway problem). JSON is not a configuration format (it is a wire
format). Operators migrating from YAML-based tooling should use the provided
`polkagent config convert --from yaml` utility (Phase 1).

**Requirement CFG-002.** Configuration file locations:

| Level | Path | Purpose |
|---|---|---|
| Built-in defaults | Compiled into binary | Safe, minimal defaults |
| System | `/etc/polkagent/config.toml` | System-wide settings |
| User global | `~/.config/polkagent/config.toml` | User preferences |
| Project | `{project_root}/.polkagent/config.toml` | Project-specific settings |
| Agent | Inline in AgentSpec or `{agent_dir}/config.toml` | Agent-specific overrides |
| Run | API request parameters | Per-run overrides |
| Environment | `POLKAGENT_*` variables | Environment overrides |

### 6.2 Configuration hierarchy

**Requirement CFG-003.** Configuration resolution order (later overrides earlier).
The layering follows the standard pattern: compiled defaults then file layers
then environment then runtime flags:

```
built-in defaults           (compiled into binary; always safe to run)
  -> system config          (operator-wide settings)
    -> user global config   (per-user preferences)
      -> project config     (repository-scoped overrides)
        -> agent spec       (agent-level overrides)
          -> run parameters (per-run API overrides)
            -> environment variables  (POLKAGENT_* -- highest precedence)
```

CLI flags (e.g., `--log-level debug`) override all layers at runtime but are
not persisted. This mirrors the `defaults→file→env→flags` pattern used by
widely-adopted Rust tools (cargo, rustfmt, clippy).

**Requirement CFG-004.** Environment variable mapping follows a consistent
pattern: `POLKAGENT_{SECTION}_{KEY}` in SCREAMING_SNAKE_CASE. Nested keys use
double underscores: `POLKAGENT_EXECUTION__BUDGET__MODEL_USD_PER_RUN=5.00`.

### 6.3 Configuration schema (TOML)

```toml
# Polkagent configuration
# Schema version: v1alpha1

[meta]
api_version = "polkagent.dev/v1alpha1"
schema_version = 1

# ─── Server ─────────────────────────────────────────────────────────

[server]
host = "127.0.0.1"
port = 4840
tls.enabled = false
tls.cert_file = ""
tls.key_file = ""
cors.allowed_origins = ["http://localhost:*"]
cors.allowed_methods = ["GET", "POST", "PUT", "DELETE", "OPTIONS"]
request_timeout_seconds = 300
max_request_body_bytes = 10_485_760

[server.rate_limit]
enabled = true
requests_per_minute = 600
burst_size = 100

# ─── Authentication ─────────────────────────────────────────────────

[auth]
method = "api_key"                # api_key | oauth | none (local dev only)

[auth.api_key]
# Keys are stored in the secret resolver, not in config files
hash_algorithm = "argon2id"

[auth.oauth]
issuer_url = ""
client_id = ""
# client_secret resolved from secret resolver
audience = ""
scopes = ["agents:read", "agents:write", "runs:read", "runs:write"]

# ─── Database ────────────────────────────────────────────────────────

[database]
backend = "sqlite"               # sqlite | postgres

[database.sqlite]
path = "~/.local/share/polkagent/polkagent.db"
journal_mode = "wal"
synchronous = "normal"           # full for payment/action authority data
busy_timeout_ms = 5000
max_connections = 1              # write; reads use a separate pool
read_pool_size = 4

[database.postgres]
url = ""                         # resolved from secret resolver
max_connections = 20
min_connections = 2
acquire_timeout_seconds = 30
idle_timeout_seconds = 600
ssl_mode = "require"

# ─── Execution ───────────────────────────────────────────────────────

[execution]
max_concurrent_runs = 10
default_timeout_seconds = 600
default_max_turns = 50

[execution.budget]
model_usd_per_run = 5.00
model_usd_per_day = 50.00
warn_threshold_percent = 80

# ─── Default provider ───────────────────────────────────────────────

[[providers]]
id = "anthropic-default"
type = "anthropic"
# api_key resolved from secret resolver at runtime
base_url = "https://api.anthropic.com"
default_model = "claude-sonnet-4-6"
timeout_seconds = 120
max_retries = 3
retry_backoff_base_ms = 1000

[[providers]]
id = "openai-compat"
type = "openai_compatible"
base_url = "https://api.openai.com/v1"
default_model = "gpt-4o"
timeout_seconds = 120

# ─── Secrets ─────────────────────────────────────────────────────────

[secrets]
backend = "file"                 # file | env | keychain | vault | kms

[secrets.file]
path = "~/.config/polkagent/secrets.enc"
encryption = "age"               # age | gpg | none (dev only)

# ─── Storage ─────────────────────────────────────────────────────────

[storage]
artifact_backend = "local"       # local | s3 | gcs

[storage.local]
path = "~/.local/share/polkagent/artifacts"
max_artifact_size_bytes = 104_857_600     # 100 MB
max_total_size_bytes = 10_737_418_240     # 10 GB

[storage.s3]
bucket = ""
prefix = "polkagent/"
region = ""
# credentials resolved from secret resolver

# ─── Chain profiles ─────────────────────────────────────────────────

[[chain_profiles]]
id = "polkadot-production"
name = "Polkadot"
genesis_hash = "0x91b171bb158e2d3848fa23a9f1c25182fb8e20313b2c1eb49219da7a70ce90c3"
rpc_endpoints = [
  "wss://rpc.polkadot.io",
  "wss://polkadot-rpc.dwellir.com",
]
metadata_cache_ttl_seconds = 3600
auto_update_metadata = true

[[chain_profiles]]
id = "polkadot-paseo"
name = "Paseo Testnet"
genesis_hash = "0x77afd6190f1554ad45fd0d31aee62aacc33c6db0ea801129acb813f8e05f1016"
rpc_endpoints = [
  "wss://paseo-rpc.dwellir.com",
]

# ─── Signers ─────────────────────────────────────────────────────────

[[signers]]
id = "user-wallet"
type = "external_wallet"
description = "User's browser extension wallet"

[[signers]]
id = "vault-airgap"
type = "polkadot_vault"
description = "Air-gapped Polkadot Vault"

# ─── Transport ───────────────────────────────────────────────────────

[transport.web]
enabled = true

[transport.cli]
enabled = true

[transport.polkadot_chat]
enabled = false
# profile and identity configured separately

# ─── Memory ──────────────────────────────────────────────────────────

[memory]
backend = "sqlite"               # sqlite | postgres | external
max_entries_per_agent = 10000
default_retention_days = 90
embedding_model = "text-embedding-3-small"
embedding_dimensions = 1536

# ─── Observability ───────────────────────────────────────────────────

[telemetry]
enabled = true
export_format = "otlp"           # otlp | json | none
endpoint = ""
log_level = "info"
structured_logging = true

[telemetry.metrics]
enabled = true
export_interval_seconds = 60

[telemetry.tracing]
enabled = true
sample_rate = 1.0                # 0.0-1.0

# ─── Workspace ───────────────────────────────────────────────────────

[workspace]
default_root = "~/.local/share/polkagent/workspaces"
isolation = "process"            # process | container | none
max_file_size_bytes = 10_485_760
max_total_size_bytes = 1_073_741_824
```

### 6.4 Configuration validation

**Requirement CFG-005.** Configuration is validated at startup and on reload:

| Validation | Behavior |
|---|---|
| Schema version check | Reject unknown schema versions |
| Required field check | Fail with clear error naming the missing field |
| Type check | Fail with expected vs actual type |
| Range/constraint check | Fail with acceptable range |
| Secret reference check | Warn if referenced secret is unavailable |
| Chain profile check | Warn if RPC endpoint is unreachable |
| Provider check | Warn if provider API key is missing or invalid |

**Requirement CFG-006.** `polkagent config validate` performs full validation
and prints a structured report. `polkagent config show` displays the resolved
configuration with secrets redacted.

### 6.5 Versioned schema with migration

**Requirement CFG-007.** Configuration schema versions are monotonically
increasing integers. Migration rules:

| From | To | Migration |
|---|---|---|
| 1 | 2 | Automated with `polkagent config migrate` |
| N | N+1 | Each version bump includes a migration function |

Migration preserves all user-specified values. New required fields receive
documented defaults. Removed fields are archived in a `_migrated` section for
manual review.

### 6.6 Example configurations

#### Minimal local development

```toml
[meta]
api_version = "polkagent.dev/v1alpha1"
schema_version = 1

[[providers]]
id = "anthropic"
type = "anthropic"
default_model = "claude-sonnet-4-6"
```

All other values use built-in defaults (SQLite, local storage, no auth, no TLS).

#### Self-hosted production

```toml
[meta]
api_version = "polkagent.dev/v1alpha1"
schema_version = 1

[server]
host = "0.0.0.0"
port = 4840
tls.enabled = true
tls.cert_file = "/etc/polkagent/tls/cert.pem"
tls.key_file = "/etc/polkagent/tls/key.pem"

[auth]
method = "oauth"

[auth.oauth]
issuer_url = "https://auth.example.com"
client_id = "polkagent-prod"
audience = "https://api.polkagent.example.com"

[database]
backend = "sqlite"

[database.sqlite]
path = "/var/lib/polkagent/polkagent.db"
synchronous = "full"

[execution]
max_concurrent_runs = 20

[telemetry]
enabled = true
export_format = "otlp"
endpoint = "http://otel-collector:4317"
```

#### Managed cloud worker

```toml
[meta]
api_version = "polkagent.dev/v1alpha1"
schema_version = 1

[server]
host = "0.0.0.0"
port = 4840

[auth]
method = "api_key"

[database]
backend = "postgres"

[database.postgres]
# url resolved from POLKAGENT_DATABASE__POSTGRES__URL env var
max_connections = 50
ssl_mode = "require"

[storage]
artifact_backend = "s3"

[storage.s3]
bucket = "polkagent-prod-artifacts"
prefix = "tenant_abc/"
region = "us-east-1"

[secrets]
backend = "kms"

[execution]
max_concurrent_runs = 100

[telemetry]
enabled = true
export_format = "otlp"
endpoint = "http://otel-collector:4317"

[telemetry.tracing]
sample_rate = 0.1
```

---

## 7. Database schemas

### 7.1 Storage backend strategy

**Requirement DB-001.** The authority store uses SQLite for local/self-hosted
deployments and PostgreSQL for managed multi-tenant deployments. Both backends
implement the same repository/event-sourcing contracts.

**Requirement DB-002.** Domain code never embeds SQL directly. All data access
goes through repository traits. The SQL layer is an implementation detail behind
those traits.

### 7.2 Core tables

#### agents

```sql
CREATE TABLE agents (
    id              TEXT PRIMARY KEY,       -- agt_{ulid}
    name            TEXT NOT NULL,
    description     TEXT,
    spec_json       TEXT NOT NULL,          -- full AgentSpec as JSON
    revision        INTEGER NOT NULL DEFAULT 1,
    status          TEXT NOT NULL DEFAULT 'stopped',
        -- CHECK (status IN ('running', 'stopped', 'archived'))
    created_by      TEXT NOT NULL,          -- principal ID
    created_at      TEXT NOT NULL,          -- ISO 8601
    updated_at      TEXT NOT NULL,
    archived_at     TEXT,

    UNIQUE(name, created_by)
);

CREATE INDEX idx_agents_status ON agents(status);
CREATE INDEX idx_agents_created_by ON agents(created_by);
CREATE INDEX idx_agents_name ON agents(name);
```

#### agent_revisions

```sql
CREATE TABLE agent_revisions (
    id              TEXT PRIMARY KEY,       -- rev_{ulid}
    agent_id        TEXT NOT NULL REFERENCES agents(id),
    revision        INTEGER NOT NULL,
    spec_json       TEXT NOT NULL,
    created_by      TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    change_summary  TEXT,

    UNIQUE(agent_id, revision)
);

CREATE INDEX idx_agent_revisions_agent ON agent_revisions(agent_id);
```

#### conversations

```sql
CREATE TABLE conversations (
    id              TEXT PRIMARY KEY,       -- conv_{ulid}
    agent_id        TEXT REFERENCES agents(id),
    title           TEXT,
    status          TEXT NOT NULL DEFAULT 'active',
        -- CHECK (status IN ('active', 'archived'))
    created_by      TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,

    -- transport-specific metadata
    transport_type  TEXT,                   -- web, cli, polkadot_chat, api
    transport_meta  TEXT                    -- JSON: session IDs, device info
);

CREATE INDEX idx_conversations_agent ON conversations(agent_id);
CREATE INDEX idx_conversations_created_by ON conversations(created_by);
CREATE INDEX idx_conversations_updated ON conversations(updated_at DESC);
```

#### runs

```sql
CREATE TABLE runs (
    id              TEXT PRIMARY KEY,       -- run_{ulid}
    agent_id        TEXT NOT NULL REFERENCES agents(id),
    conversation_id TEXT REFERENCES conversations(id),
    workspace_id    TEXT,

    status          TEXT NOT NULL DEFAULT 'pending',
        -- CHECK (status IN ('pending','running','waiting_approval',
        --   'waiting_input','paused','cancelling','completed',
        --   'failed','cancelled','timed_out'))

    input_json      TEXT NOT NULL,          -- serialized RunInput
    config_json     TEXT,                   -- run-level config overrides
    grant_hash      TEXT NOT NULL,          -- SHA-256 of ResolvedGrant
    grant_json      TEXT NOT NULL,          -- serialized ResolvedGrant

    turns_completed INTEGER NOT NULL DEFAULT 0,
    terminal_reason TEXT,                   -- completion/failure detail

    -- usage accounting
    input_tokens    INTEGER NOT NULL DEFAULT 0,
    output_tokens   INTEGER NOT NULL DEFAULT 0,
    model_cost_usd  REAL NOT NULL DEFAULT 0.0,

    -- idempotency
    idempotency_key TEXT UNIQUE,

    -- lifecycle timestamps
    created_at      TEXT NOT NULL,
    started_at      TEXT,
    completed_at    TEXT,

    -- policy binding
    policy_revision TEXT,
    agent_revision  INTEGER
);

CREATE INDEX idx_runs_agent ON runs(agent_id);
CREATE INDEX idx_runs_conversation ON runs(conversation_id);
CREATE INDEX idx_runs_status ON runs(status);
CREATE INDEX idx_runs_created ON runs(created_at DESC);
CREATE INDEX idx_runs_idempotency ON runs(idempotency_key) WHERE idempotency_key IS NOT NULL;
```

#### turns

```sql
CREATE TABLE turns (
    id              TEXT PRIMARY KEY,       -- turn_{ulid}
    run_id          TEXT NOT NULL REFERENCES runs(id),
    sequence        INTEGER NOT NULL,

    role            TEXT NOT NULL,          -- user, assistant, system, tool
    input_json      TEXT,                   -- turn input
    output_json     TEXT,                   -- turn output

    status          TEXT NOT NULL DEFAULT 'pending',
        -- CHECK (status IN ('pending','running','completed','failed'))

    input_tokens    INTEGER NOT NULL DEFAULT 0,
    output_tokens   INTEGER NOT NULL DEFAULT 0,
    model_id        TEXT,
    provider_id     TEXT,

    started_at      TEXT,
    completed_at    TEXT,

    UNIQUE(run_id, sequence)
);

CREATE INDEX idx_turns_run ON turns(run_id);
```

#### effects

```sql
CREATE TABLE effects (
    id              TEXT PRIMARY KEY,       -- eff_{ulid}
    run_id          TEXT NOT NULL REFERENCES runs(id),
    turn_id         TEXT REFERENCES turns(id),

    type            TEXT NOT NULL,
        -- model_call, tool_invocation, chain_action, payment,
        -- delivery, signing, approval_request

    status          TEXT NOT NULL DEFAULT 'pending',
        -- CHECK (status IN ('pending','pending_approval','approved',
        --   'denied','executing','succeeded','failed','cancelled',
        --   'timed_out','unknown'))

    intent_json     TEXT NOT NULL,          -- serialized EffectIntent
    grant_hash      TEXT NOT NULL,

    idempotency_key TEXT NOT NULL UNIQUE,

    created_at      TEXT NOT NULL,
    resolved_at     TEXT,

    -- policy
    policy_decision TEXT NOT NULL,          -- allow, deny, require_approval
    policy_revision TEXT,
    policy_reasons  TEXT                    -- JSON array of reason strings
);

CREATE INDEX idx_effects_run ON effects(run_id);
CREATE INDEX idx_effects_status ON effects(status);
CREATE INDEX idx_effects_type ON effects(type);
CREATE INDEX idx_effects_idempotency ON effects(idempotency_key);
CREATE INDEX idx_effects_pending_approval ON effects(status)
    WHERE status = 'pending_approval';
```

#### effect_attempts

```sql
CREATE TABLE effect_attempts (
    id              TEXT PRIMARY KEY,       -- att_{ulid}
    effect_id       TEXT NOT NULL REFERENCES effects(id),
    attempt_number  INTEGER NOT NULL,

    status          TEXT NOT NULL DEFAULT 'claimed',
        -- CHECK (status IN ('claimed','executing','completed','failed',
        --   'timed_out','cancelled'))

    worker_id       TEXT,                   -- which worker/process claimed it
    lease_expires   TEXT,                   -- lease expiration
    retry_reason    TEXT,

    started_at      TEXT NOT NULL,
    completed_at    TEXT,

    UNIQUE(effect_id, attempt_number)
);

CREATE INDEX idx_attempts_effect ON effect_attempts(effect_id);
CREATE INDEX idx_attempts_status ON effect_attempts(status);
CREATE INDEX idx_attempts_lease ON effect_attempts(lease_expires)
    WHERE status = 'claimed';
```

#### effect_outcomes

```sql
CREATE TABLE effect_outcomes (
    id              TEXT PRIMARY KEY,       -- out_{ulid}
    effect_id       TEXT NOT NULL REFERENCES effects(id),
    attempt_id      TEXT NOT NULL REFERENCES effect_attempts(id),

    result          TEXT NOT NULL,
        -- CHECK (result IN ('success','failure','timeout',
        --   'cancellation','unknown'))

    result_json     TEXT,                   -- typed result data
    error_json      TEXT,                   -- typed error data

    -- chain-action-specific
    tx_hash         TEXT,
    block_hash      TEXT,
    block_number    INTEGER,
    finality_status TEXT,
        -- CHECK (finality_status IN ('submitted','included',
        --   'finalized','failed','unknown') OR finality_status IS NULL)

    -- usage
    duration_ms     INTEGER,
    tokens_used     INTEGER,
    cost_usd        REAL,

    created_at      TEXT NOT NULL,

    UNIQUE(attempt_id)
);

CREATE INDEX idx_outcomes_effect ON effect_outcomes(effect_id);
CREATE INDEX idx_outcomes_result ON effect_outcomes(result);
CREATE INDEX idx_outcomes_tx ON effect_outcomes(tx_hash)
    WHERE tx_hash IS NOT NULL;
```

#### artifacts

```sql
CREATE TABLE artifacts (
    id              TEXT PRIMARY KEY,       -- art_{ulid}
    run_id          TEXT REFERENCES runs(id),
    turn_id         TEXT REFERENCES turns(id),

    type            TEXT NOT NULL,
        -- code_diff, file, plan, decoded_call, simulation,
        -- receipt, test_result, evidence_package, log

    classification  TEXT NOT NULL DEFAULT 'workspace_output',
        -- workspace_output, evidence, user_upload, system

    content_hash    TEXT NOT NULL,          -- sha256:{hex}
    content_type    TEXT NOT NULL,          -- MIME type
    size_bytes      INTEGER NOT NULL,
    filename        TEXT,

    -- storage location
    storage_backend TEXT NOT NULL,          -- local, s3, inline
    storage_ref     TEXT NOT NULL,          -- path or object key

    -- provenance
    created_by_type TEXT NOT NULL,          -- agent, user, system, tool
    created_by_id   TEXT NOT NULL,
    parent_id       TEXT REFERENCES artifacts(id),
    tool_id         TEXT,

    -- retention
    retention_policy TEXT NOT NULL DEFAULT 'workspace_default',
    expires_at      TEXT,

    created_at      TEXT NOT NULL,
    metadata_json   TEXT                    -- arbitrary metadata
);

CREATE INDEX idx_artifacts_run ON artifacts(run_id);
CREATE INDEX idx_artifacts_type ON artifacts(type);
CREATE INDEX idx_artifacts_hash ON artifacts(content_hash);
CREATE INDEX idx_artifacts_created ON artifacts(created_at DESC);
```

#### events

```sql
CREATE TABLE events (
    id              TEXT PRIMARY KEY,       -- evt_{ulid}
    run_id          TEXT REFERENCES runs(id),
    agent_id        TEXT REFERENCES agents(id),

    sequence        INTEGER NOT NULL,       -- per-run sequence
    type            TEXT NOT NULL,          -- event type string
    timestamp       TEXT NOT NULL,          -- ISO 8601 with microseconds

    payload_json    TEXT NOT NULL,

    correlation_id  TEXT,                   -- typically run_id
    causation_id    TEXT,                   -- preceding event ID

    durability      TEXT NOT NULL DEFAULT 'persistent',
        -- CHECK (durability IN ('persistent','ephemeral'))

    -- denormalized for query performance
    turn_id         TEXT,
    effect_id       TEXT
);

CREATE INDEX idx_events_run_seq ON events(run_id, sequence);
CREATE INDEX idx_events_type ON events(type);
CREATE INDEX idx_events_timestamp ON events(timestamp DESC);
CREATE INDEX idx_events_correlation ON events(correlation_id);
```

#### grants

```sql
CREATE TABLE grants (
    hash            TEXT PRIMARY KEY,       -- sha256:{hex}
    grant_json      TEXT NOT NULL,          -- serialized ResolvedGrant
    policy_revision TEXT NOT NULL,
    created_at      TEXT NOT NULL
);
```

#### approvals

```sql
CREATE TABLE approvals (
    id              TEXT PRIMARY KEY,       -- apr_{ulid}
    effect_id       TEXT NOT NULL REFERENCES effects(id),

    type            TEXT NOT NULL,
        -- CHECK (type IN ('human','quorum','service','mandate'))

    principal_id    TEXT,
    comment         TEXT,
    conditions_json TEXT,

    -- mandate reference (for autonomous operation)
    mandate_ref     TEXT,
    mandate_hash    TEXT,

    decision        TEXT NOT NULL,
        -- CHECK (decision IN ('approved','denied'))

    created_at      TEXT NOT NULL,

    UNIQUE(effect_id)
);

CREATE INDEX idx_approvals_effect ON approvals(effect_id);
CREATE INDEX idx_approvals_principal ON approvals(principal_id);
```

#### memory_entries

```sql
CREATE TABLE memory_entries (
    id              TEXT PRIMARY KEY,       -- mem_{ulid}
    agent_id        TEXT NOT NULL REFERENCES agents(id),

    type            TEXT NOT NULL,
        -- CHECK (type IN ('episodic','semantic','procedural'))

    content         TEXT NOT NULL,
    embedding       BLOB,                   -- vector embedding
    tags_json       TEXT,                   -- JSON array of tags

    -- provenance
    source_type     TEXT NOT NULL,          -- conversation, tool, import, user
    source_ref      TEXT,                   -- run_id, artifact_id, etc.

    -- retention
    retention_policy TEXT NOT NULL DEFAULT 'agent_default',
    expires_at      TEXT,
    last_accessed   TEXT,

    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX idx_memory_agent ON memory_entries(agent_id);
CREATE INDEX idx_memory_type ON memory_entries(type);
CREATE INDEX idx_memory_created ON memory_entries(created_at DESC);
CREATE INDEX idx_memory_expires ON memory_entries(expires_at)
    WHERE expires_at IS NOT NULL;
```

#### api_keys

```sql
CREATE TABLE api_keys (
    id              TEXT PRIMARY KEY,       -- key_{ulid}
    principal_id    TEXT NOT NULL,
    name            TEXT NOT NULL,
    key_hash        TEXT NOT NULL,          -- argon2id hash of key
    prefix          TEXT NOT NULL,          -- first 8 chars for identification

    scopes_json     TEXT NOT NULL,          -- allowed operations
    ip_allowlist    TEXT,                   -- JSON array of CIDRs
    expires_at      TEXT,
    revoked_at      TEXT,

    last_used_at    TEXT,
    use_count       INTEGER NOT NULL DEFAULT 0,

    created_at      TEXT NOT NULL,

    UNIQUE(key_hash)
);

CREATE INDEX idx_api_keys_principal ON api_keys(principal_id);
CREATE INDEX idx_api_keys_prefix ON api_keys(prefix);
```

### 7.3 Migration strategy: SQLite to PostgreSQL

**Requirement DB-003.** Schema migrations are managed using a versioned
migrations table:

```sql
CREATE TABLE schema_migrations (
    version         INTEGER PRIMARY KEY,
    description     TEXT NOT NULL,
    applied_at      TEXT NOT NULL,
    checksum        TEXT NOT NULL           -- SHA-256 of migration SQL
);
```

**Requirement DB-004.** SQLite and PostgreSQL schemas are semantically
identical. Differences are limited to:

| Aspect | SQLite | PostgreSQL |
|---|---|---|
| Text type | `TEXT` | `TEXT` |
| Integer type | `INTEGER` | `BIGINT` |
| Real type | `REAL` | `DOUBLE PRECISION` |
| Blob type | `BLOB` | `BYTEA` |
| Boolean | `INTEGER (0/1)` | `BOOLEAN` |
| JSON | `TEXT` (validated in app) | `JSONB` |
| Timestamps | `TEXT` (ISO 8601) | `TIMESTAMPTZ` |
| Auto-increment | Not used (ULIDs) | Not used (ULIDs) |
| Full-text search | FTS5 | tsvector/GIN |
| Vector search | App-level | pgvector |

**Requirement DB-005.** `polkagent db migrate` applies pending migrations.
`polkagent db export` exports the full database as a portable JSON-lines
archive. `polkagent db import` imports from that archive into either backend.

---

## 8. Wire-level DTOs

All DTOs are defined as versioned schemas. This section catalogs the major types.

### 8.1 Common types

```json
// Timestamp
"2026-07-30T12:00:00.000000Z"          // ISO 8601 with microsecond precision

// ID
"run_01HQXYZ..."                        // {type_prefix}_{ulid}

// Money
{
  "amount": "10000000000",              // string to avoid floating point
  "decimals": 10,
  "symbol": "DOT",
  "asset_id": null                      // null for native asset
}

// Pagination cursor
{
  "next": "eyJ0IjoiMjAyNi0wNy0zMCJ9", // opaque base64
  "has_more": true
}

// Error
{
  "code": "EFFECT_DENIED",
  "message": "Policy denied filesystem write outside workspace",
  "details": { ... },
  "request_id": "req_xyz789",
  "timestamp": "2026-07-30T12:00:00Z"
}
```

### 8.2 Event payload schemas

#### run.started

```json
{
  "run_id": "run_01HQ...",
  "agent_id": "agt_01HQ...",
  "input_summary": "User asked about staking rewards",
  "grant_hash": "sha256:abc...",
  "model_id": "model_01HQ...",
  "started_at": "2026-07-30T12:00:00Z"
}
```

#### turn.token_delta

```json
{
  "run_id": "run_01HQ...",
  "turn_id": "turn_01HQ...",
  "delta": "The staking pallet",
  "token_index": 15,
  "role": "assistant"
}
```

#### effect.pending_approval

```json
{
  "effect_id": "eff_01HQ...",
  "run_id": "run_01HQ...",
  "type": "chain_action",
  "summary": "Transfer 1.0 DOT to 5FHneW46...",
  "risk_level": "medium",
  "intent_preview": {
    "action": "transferKeepAlive",
    "asset": "DOT",
    "amount_human": "1.0",
    "destination_short": "5FHn...W46"
  },
  "approval_deadline": "2026-07-30T12:10:00Z"
}
```

#### effect.succeeded

```json
{
  "effect_id": "eff_01HQ...",
  "run_id": "run_01HQ...",
  "type": "chain_action",
  "outcome": {
    "result": "success",
    "tx_hash": "0xabc...",
    "block_hash": "0xdef...",
    "block_number": 12345678,
    "finality_status": "finalized"
  },
  "duration_ms": 12500,
  "cost_usd": 0.001
}
```

### 8.3 WebSocket message types

| Message type | Direction | Purpose |
|---|---|---|
| `auth` | Client -> Server | Authenticate connection |
| `auth_ok` | Server -> Client | Authentication successful |
| `subscribe` | Client -> Server | Subscribe to channel |
| `subscribed` | Server -> Client | Subscription confirmed |
| `unsubscribe` | Client -> Server | Unsubscribe from channel |
| `unsubscribed` | Server -> Client | Unsubscription confirmed |
| `event` | Server -> Client | Event delivery |
| `message` | Client -> Server | Send user message to run |
| `ping` | Both | Keep-alive |
| `pong` | Both | Keep-alive response |
| `error` | Server -> Client | Error notification |
| `close` | Both | Connection closing |

### 8.4 Error response codes

| Code | HTTP Status | Meaning |
|---|---|---|
| `AUTHENTICATION_REQUIRED` | 401 | No valid credentials provided |
| `INSUFFICIENT_PERMISSIONS` | 403 | Principal lacks required scope |
| `RESOURCE_NOT_FOUND` | 404 | Referenced resource does not exist |
| `RESOURCE_CONFLICT` | 409 | Idempotency key reused with different content |
| `VALIDATION_ERROR` | 422 | Request body fails schema validation |
| `RATE_LIMIT_EXCEEDED` | 429 | Too many requests |
| `EFFECT_DENIED` | 403 | Policy denied the requested effect |
| `GRANT_INSUFFICIENT` | 403 | Active grant does not cover operation |
| `BUDGET_EXCEEDED` | 402 | Cost budget exhausted |
| `PROVIDER_ERROR` | 502 | Upstream model provider error |
| `PROVIDER_TIMEOUT` | 504 | Upstream model provider timeout |
| `SIGNER_ERROR` | 502 | Signer service error |
| `CHAIN_ERROR` | 502 | Chain RPC error |
| `INTERNAL_ERROR` | 500 | Unexpected internal error |

---

## 9. SDK design

### 9.1 Rust SDK

**Requirement SDK-RS-001.** The Rust SDK is the primary SDK. It is a set of
crates in the Polkagent workspace:

| Crate | Purpose |
|---|---|
| `polkagent-client` | HTTP/WebSocket client for the public API |
| `polkagent-types` | All DTO types, shared between client and server |
| `polkagent-config` | Configuration parsing and validation |
| `polkagent-schema` | Schema validation utilities |

#### Usage example

```rust
use polkagent_client::PolkagentClient;
use polkagent_types::{CreateRunRequest, RunInput};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = PolkagentClient::builder()
        .base_url("http://localhost:4840")
        .api_key("pak_...")
        .build()?;

    // Create a run
    let run = client.runs().create(CreateRunRequest {
        agent_id: "agt_01HQ...".into(),
        input: RunInput::UserMessage {
            content: "Explain the staking pallet".into(),
            attachments: vec![],
        },
        ..Default::default()
    }).await?;

    // Stream events
    let mut stream = client.runs().stream_events(&run.id).await?;
    while let Some(event) = stream.next().await {
        match event?.payload {
            EventPayload::TokenDelta { delta, .. } => print!("{delta}"),
            EventPayload::RunCompleted { .. } => break,
            _ => {}
        }
    }

    Ok(())
}
```

### 9.2 TypeScript SDK

**Requirement SDK-TS-001.** The TypeScript SDK is generated from the canonical
schema repository and provides typed access to the public API:

| Package | Purpose |
|---|---|
| `@polkagent/client` | HTTP/WebSocket client |
| `@polkagent/types` | Generated TypeScript types from JSON Schema |
| `@polkagent/config` | Configuration parsing for Node.js environments |

#### Usage example

```typescript
import { PolkagentClient } from '@polkagent/client';

const client = new PolkagentClient({
  baseUrl: 'http://localhost:4840',
  apiKey: 'pak_...',
});

// Create a run
const run = await client.runs.create({
  agentId: 'agt_01HQ...',
  input: {
    type: 'user_message',
    content: 'Explain the staking pallet',
  },
});

// Stream events
for await (const event of client.runs.streamEvents(run.id)) {
  if (event.type === 'turn.token_delta') {
    process.stdout.write(event.payload.delta);
  }
  if (event.type === 'run.completed') break;
}
```

### 9.3 SDK code generation

**Requirement SDK-GEN-001.** The code generation pipeline (JSON Schema is the
single source; all other formats are derived):

```
polkagent-schemas/
  ├── json-schema/         # Canonical JSON Schema (draft 2020-12) definitions
  ├── openapi/             # OpenAPI 3.1 -- $ref into json-schema/
  └── generators/
      ├── rust/            # Rust type generation (serde + validation)
      └── typescript/      # TypeScript type generation (zod)
```

**Requirement SDK-GEN-002.** Generation is deterministic: the same schema input
always produces identical output. Generated code is committed to the repository
and verified in CI. The CI check rejects any commit where generated code does
not match what the generator would produce from the checked-in schema.

**Requirement SDK-GEN-003.** Generated types include:
- Serialization/deserialization (serde for Rust, zod for TypeScript).
- Validation against the JSON Schema.
- Builder patterns for complex request types.
- Documented fields with descriptions from the schema.

**Maturity: validate-next.** The initial Phase 0 implementation hand-writes the
Rust types (as `polkagent-types` crate) and bootstraps the JSON Schema from
those types. SDK codegen (generating Rust from JSON Schema rather than the
reverse) is introduced in Phase 1 as the schema stabilises, at which point the
hand-written types are replaced by generated ones and the JSON Schema becomes
the authoritative source.

---

## 10. PCA migration

### 10.1 Migration scope

PCA (polkadot-chat-agents) is the existing Node.js reference product.
Polkagent must support importing PCA state and configuration so that existing
users can transition without losing data.

**Requirement MIG-001.** The migration tool (`polkagent migrate pca`) handles:

| Data | Source | Target |
|---|---|---|
| Bot identity | PCA identity/keyring | Polkagent identity + account binding |
| Configuration | PCA `config.yml` / env vars | Polkagent TOML configuration |
| Conversations | PCA message store | Polkagent conversations table |
| Projects/workspaces | PCA project directories | Polkagent workspace records |
| Session state | PCA session files | Polkagent run/conversation state |
| Bridge config | PCA bridge definitions | Polkagent harness configuration |
| Files/media | PCA file store | Polkagent artifact store |

### 10.2 State import format

**Requirement MIG-002.** PCA state is exported as a versioned JSON-lines
archive:

```
polkagent-pca-export-v1.jsonl
```

Each line is a typed record:

```json
{"type": "identity", "version": 1, "data": { "account": "5Grwva...", "name": "my-bot", ... }}
{"type": "config", "version": 1, "data": { "model": "claude-sonnet-4-6", "project_root": "/home/...", ... }}
{"type": "conversation", "version": 1, "data": { "id": "...", "messages": [...], ... }}
{"type": "project", "version": 1, "data": { "path": "...", "files": [...], ... }}
{"type": "session", "version": 1, "data": { "id": "...", "brain": "claude", "resume_token": "...", ... }}
{"type": "bridge", "version": 1, "data": { "name": "hermes", "endpoint": "http://...", ... }}
```

### 10.3 Configuration translation rules

**Requirement MIG-003.** PCA configuration maps to Polkagent configuration:

| PCA field | Polkagent equivalent | Notes |
|---|---|---|
| `model` | `providers[0].default_model` | Model name translation table |
| `apiKey` | Secret resolver reference | Never stored in config file |
| `project.root` | `workspace.default_root` | Path validation |
| `project.allowedPaths` | Agent spec `tools.constraints.paths` | Glob pattern translation |
| `brains.claude.maxTokens` | `execution.budget.max_output_tokens` | Direct mapping |
| `brains.claude.temperature` | Agent spec execution parameters | Direct mapping |
| `transport.polkadot.profile` | `chain_profiles[n]` + `transport.polkadot_chat` | Profile lookup and binding |
| `transport.polkadot.identity` | Identity import (separate) | Key material import |
| `bridges.*` | `[[harnesses]]` configuration | Per-bridge translation |
| `deployment.ssh.*` | Deployment configuration (PRD-11) | Separate migration |

**Requirement MIG-004.** Model name translation table (maintained and versioned):

| PCA model name | Polkagent model ID |
|---|---|
| `claude-sonnet-4-20250514` | `claude-sonnet-4-6` (latest) |
| `claude-opus-4-20250514` | `claude-opus-4-6` (latest) |
| `gpt-4o` | `gpt-4o` |
| `codex-mini` | `codex-mini-latest` |
| Custom | Preserved as-is with validation |

### 10.4 Identity migration

**Requirement MIG-005.** PCA bot identity migration:

1. Export PCA identity (account, name, network bindings).
2. Import into Polkagent identity store.
3. Create account bindings for each network profile.
4. Verify identity by signing a challenge with the PCA keyring.
5. Optionally generate a new Polkagent-native identity and link it to the
   PCA identity.

**Requirement MIG-006.** Identity migration must not expose raw key material
to the migration tool's stdout, logs, or temporary files. Key import uses
the secret resolver directly.

### 10.5 Data migration tool design

**Requirement MIG-007.** The migration tool workflow:

```
polkagent migrate pca \
    --source /path/to/pca \
    --target /path/to/polkagent \
    --dry-run
```

Phases:

| Phase | Description |
|---|---|
| **Discover** | Scan PCA directory for config, identity, sessions, projects, bridges |
| **Analyze** | Produce a migration plan with warnings and conflicts |
| **Dry run** | Simulate migration without writing to target |
| **Execute** | Perform migration with atomic commits per record type |
| **Verify** | Run compatibility checks against migrated data |
| **Report** | Produce a migration report with successes, warnings, and failures |

**Requirement MIG-008.** Migration is resumable. Each record is tagged with a
migration ID and status. A failed migration can be resumed from the last
successful record.

### 10.6 Rollback support

**Requirement MIG-009.** Before migration, the tool creates a backup:

```
polkagent-pre-migration-backup-{timestamp}.tar.gz
```

**Requirement MIG-010.** Rollback procedure:

1. `polkagent migrate rollback --backup <backup-file>` restores the
   pre-migration state.
2. PCA remains functional during and after migration (read-only mode during
   active migration).
3. Shadow mode: run both PCA and Polkagent in parallel, comparing outputs,
   before cutover.

---

## 11. Feature flags and maturity gates

### 11.1 Feature flag mechanism

**Requirement FF-001.** Features are gated by a typed flag system:

```toml
[features]
chain_actions = "stable"          # stable, beta, alpha, disabled
payments = "beta"
pvm_contracts = "alpha"
jam_services = "disabled"
multi_agent_groups = "alpha"
marketplace = "beta"
```

**Requirement FF-002.** Maturity levels:

| Level | Meaning | API stability | Config |
|---|---|---|---|
| `stable` | Production-ready, covered by compatibility policy | Full | Default on |
| `beta` | Functionally complete, API may change | Best-effort | Opt-in |
| `alpha` | Experimental, incomplete, API will change | None | Explicit opt-in |
| `disabled` | Not available in this build/deployment | N/A | Cannot enable |

### 11.2 Maturity label enforcement

**Requirement FF-003.** API endpoints, configuration sections, and CLI commands
associated with non-stable features return a structured warning:

```json
{
  "warning": {
    "code": "FEATURE_BETA",
    "feature": "payments",
    "message": "Payments API is in beta. Schema may change without notice."
  }
}
```

**Requirement FF-004.** Attempting to use a `disabled` feature returns HTTP 501:

```json
{
  "error": {
    "code": "FEATURE_DISABLED",
    "feature": "jam_services",
    "message": "JAM services are not available in this deployment"
  }
}
```

### 11.3 Experimental feature opt-in

**Requirement FF-005.** Alpha and beta features require explicit opt-in:

- Configuration: `features.{name} = "alpha"` in config file.
- API: `X-Polkagent-Features: payments=beta` header.
- CLI: `--feature payments=beta` flag.

---

## 12. API versioning policy

### 12.1 Semantic versioning rules

**Requirement VER-001.** API versions follow a Kubernetes-style maturity
lifecycle expressed in the URL path. This is the chosen versioning strategy;
content-negotiation versioning (via `Accept:` header) is explicitly deferred.

```
v1alpha1 -> v1alpha2 -> ... -> v1beta1 -> v1beta2 -> ... -> v1 -> v1.1 -> ...
```

| Stage | Stability | Breaking changes | Support window |
|---|---|---|---|
| `alpha` | None | May happen at any time | No support guarantee |
| `beta` | Best-effort | Announced 2 weeks ahead | 3 months after successor |
| `stable` | Full | Only via new major version | 12 months after successor |

The maturity stage in the URL path is the canonical source of truth. It maps
directly to the feature flag maturity levels in section 11: an endpoint that
exists only behind an `alpha` feature flag is itself `alpha`. When the feature
flag promotes to `stable`, the endpoint can graduate to a `v1beta1` or `v1`
path in the next version bump.

### 12.2 Deprecation process

**Requirement VER-002.** Deprecated features and endpoints:

1. Announce deprecation in release notes and API documentation.
2. Return `Deprecation: true` and `Sunset: <date>` headers.
3. Emit deprecation warnings in SDK clients.
4. Remove after the support window expires.

### 12.3 Breaking change policy

**Requirement VER-003.** Breaking changes include:

- Removing or renaming a field.
- Changing a field's type or semantics.
- Removing an endpoint.
- Changing authentication requirements.
- Changing error codes for existing conditions.

Non-breaking changes include:

- Adding new optional fields.
- Adding new endpoints.
- Adding new enum variants (clients must handle unknown variants).
- Adding new error codes for new conditions.
- Relaxing validation constraints.

---

## 13. Integration testing contracts

### 13.1 Contract test framework

**Requirement TEST-001.** API contracts are verified using a dual test approach:

| Test type | Verifies | Run by |
|---|---|---|
| **Provider contract tests** | Server produces conformant responses | Server CI |
| **Consumer contract tests** | Client handles all response shapes | SDK CI |
| **Compatibility tests** | Version N+1 serves version N clients | Server CI |

### 13.2 Provider contract tests

**Requirement TEST-002.** Provider contract tests verify:

- Every documented endpoint returns the documented response shape.
- Error responses match the error envelope schema.
- Pagination, sorting, and filtering behave as documented.
- WebSocket subscription and event delivery work correctly.
- Rate limiting headers are present and accurate.
- Authentication and authorization are enforced.

Contract test fixtures are stored in `polkagent-schemas/fixtures/`:

```
fixtures/
  ├── agents/
  │   ├── create-minimal.request.json
  │   ├── create-minimal.response.json
  │   ├── create-full.request.json
  │   ├── create-full.response.json
  │   ├── list-default.response.json
  │   └── ...
  ├── runs/
  │   └── ...
  ├── effects/
  │   └── ...
  └── errors/
      ├── validation-error.response.json
      ├── not-found.response.json
      └── ...
```

### 13.3 Consumer contract tests

**Requirement TEST-003.** Consumer contract tests verify:

- SDK clients can deserialize all documented response shapes.
- SDK clients handle unknown fields gracefully (ignore, not error).
- SDK clients handle unknown enum variants gracefully.
- SDK error handling covers all documented error codes.
- WebSocket reconnection and subscription recovery work.

### 13.4 Version compatibility tests

**Requirement TEST-004.** On every API version bump:

- The previous version's provider contract tests must still pass against the
  new server.
- The previous version's consumer contract tests must still pass against
  the new SDK.
- A documented migration guide covers any behavioral changes.

### 13.5 Staged implementation recommendations

Based on research into Stripe, Anthropic, and Kubernetes API design patterns,
the following phased approach is recommended for the API/schema/config domain:

#### Do now (Phase 0)

| Item | Rationale |
|---|---|
| Establish `polkagent-schemas` repo with JSON Schema as source of truth | Foundation that everything else builds on; impossible to retrofit cleanly later |
| Idempotency keys required on all financial/blockchain POSTs | Prevents double-spend bugs that are extremely difficult to remediate post-launch |
| TOML-only config (no YAML) | Simplifies the Rust config stack; eliminates YAML footgun classes |
| Cursor pagination on all list endpoints | Offset pagination is inconsistent under concurrent writes; cheap to do right from day one |

#### Validate next (Phase 1)

| Item | Rationale |
|---|---|
| SDK codegen from JSON Schema (replace hand-written types) | Requires the schema to be stable enough to round-trip; validate schema quality first |
| Contract test suite (provider + consumer) | Requires representative request/response fixtures; build once the API surface is settled |
| SSE streaming endpoint (`/runs/{id}/stream`) | Validate that the event envelope works for browser/CLI consumers before committing to the shape |
| Webhook delivery with signed payloads | Requires operational infrastructure (retry queue, delivery log); validate signing UX with early integrators |

#### Defer (post-Phase 1)

| Item | Rationale |
|---|---|
| gRPC surface | No concrete latency/throughput requirement yet; REST+SSE covers all known use cases |
| Content-negotiation versioning | Adds server complexity with no clear benefit over URL-path versioning |
| External message bus (NATS, Kafka) for internal event bus | SQLite outbox covers single-node and self-hosted; revisit when multi-node coordination is needed |

#### Avoid

| Item | Why |
|---|---|
| YAML config format | Implicit type coercion, significant whitespace, poor tooling in Rust ecosystem |
| Unversioned breaking changes | Any breaking change without a version bump violates the compatibility contract and breaks SDK users silently |
| Hand-written TypeScript types | Drift from JSON Schema is inevitable; only generated types are guaranteed to match |

---

## 14. OpenAPI specification (skeleton)

**Requirement API-OPENAPI-001.** The full OpenAPI 3.1 specification lives in
`polkagent-schemas/openapi/polkagent-api.yaml`. The following is a structural
skeleton:

```yaml
openapi: "3.1.0"
info:
  title: Polkagent API
  version: v1alpha1
  description: |
    Polkagent platform API for managing agents, runs, effects, artifacts,
    and configuration.
  license:
    name: Apache-2.0
    url: https://www.apache.org/licenses/LICENSE-2.0

servers:
  - url: http://localhost:4840/api/v1alpha1
    description: Local development
  - url: https://api.polkagent.dev/api/v1alpha1
    description: Managed cloud

security:
  - apiKey: []
  - oauth2: []

paths:
  /agents:
    get:
      operationId: listAgents
      tags: [Agents]
      summary: List agents
      parameters:
        - $ref: "#/components/parameters/CursorParam"
        - $ref: "#/components/parameters/PageSizeParam"
        - name: status
          in: query
          schema:
            type: string
            enum: [running, stopped, archived]
      responses:
        "200":
          description: Paginated list of agents
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/AgentListResponse"
    post:
      operationId: createAgent
      tags: [Agents]
      summary: Create a new agent
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: "#/components/schemas/CreateAgentRequest"
      responses:
        "201":
          description: Agent created
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/AgentResponse"
        "422":
          $ref: "#/components/responses/ValidationError"

  /agents/{agent_id}:
    get:
      operationId: getAgent
      tags: [Agents]
      parameters:
        - $ref: "#/components/parameters/AgentIdParam"
      responses:
        "200":
          description: Agent details
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/AgentResponse"
        "404":
          $ref: "#/components/responses/NotFound"

  /runs:
    post:
      operationId: createRun
      tags: [Runs]
      summary: Create and start a new run
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: "#/components/schemas/CreateRunRequest"
      responses:
        "201":
          description: Run created and started
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/RunResponse"

  /runs/{run_id}:
    get:
      operationId: getRun
      tags: [Runs]
      parameters:
        - $ref: "#/components/parameters/RunIdParam"
      responses:
        "200":
          description: Run details
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/RunResponse"

  /runs/{run_id}/cancel:
    post:
      operationId: cancelRun
      tags: [Runs]
      parameters:
        - $ref: "#/components/parameters/RunIdParam"
      responses:
        "200":
          description: Cancellation accepted

  /effects:
    get:
      operationId: listEffects
      tags: [Effects]
      responses:
        "200":
          description: Paginated list of effects

  /effects/{effect_id}/approve:
    post:
      operationId: approveEffect
      tags: [Effects]
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: "#/components/schemas/ApproveEffectRequest"
      responses:
        "200":
          description: Effect approved

  /effects/{effect_id}/deny:
    post:
      operationId: denyEffect
      tags: [Effects]
      responses:
        "200":
          description: Effect denied

  /artifacts:
    get:
      operationId: listArtifacts
      tags: [Artifacts]
      responses:
        "200":
          description: Paginated list of artifacts
    post:
      operationId: uploadArtifact
      tags: [Artifacts]
      requestBody:
        required: true
        content:
          multipart/form-data:
            schema:
              $ref: "#/components/schemas/UploadArtifactRequest"

  /events:
    get:
      operationId: listEvents
      tags: [Events]
      responses:
        "200":
          description: Paginated list of events

  /providers:
    get:
      operationId: listProviders
      tags: [Models]
      responses:
        "200":
          description: List of configured providers

  /models:
    get:
      operationId: listModels
      tags: [Models]
      responses:
        "200":
          description: List of available models

  /skills:
    get:
      operationId: listSkills
      tags: [Skills]
      responses:
        "200":
          description: List of installed skills

  /skills/install:
    post:
      operationId: installSkill
      tags: [Skills]
      responses:
        "200":
          description: Skill installed

  /memory/query:
    post:
      operationId: queryMemory
      tags: [Memory]
      responses:
        "200":
          description: Memory query results

  /payments/intents:
    post:
      operationId: createPaymentIntent
      tags: [Payments]
      responses:
        "201":
          description: Payment intent created

  /conversations:
    get:
      operationId: listConversations
      tags: [Conversations]
      responses:
        "200":
          description: List of conversations

  /workspaces:
    get:
      operationId: listWorkspaces
      tags: [Workspaces]
      responses:
        "200":
          description: List of workspaces

  # System
  /health:
    get:
      operationId: healthCheck
      tags: [System]
      security: []
      responses:
        "200":
          description: Service health

  /versions:
    get:
      operationId: listVersions
      tags: [System]
      security: []
      responses:
        "200":
          description: Supported API versions

components:
  securitySchemes:
    apiKey:
      type: http
      scheme: bearer
      bearerFormat: PAK
    oauth2:
      type: oauth2
      flows:
        authorizationCode:
          authorizationUrl: /oauth/authorize
          tokenUrl: /oauth/token
          scopes:
            agents:read: Read agent data
            agents:write: Create and modify agents
            runs:read: Read run data
            runs:write: Create and manage runs
            effects:write: Approve or deny effects
            artifacts:read: Read artifacts
            artifacts:write: Upload artifacts
            memory:read: Query memory
            memory:write: Modify memory
            payments:read: Read payment data
            payments:write: Create payment intents
            admin: Full administrative access

  parameters:
    CursorParam:
      name: cursor
      in: query
      schema:
        type: string
    PageSizeParam:
      name: page_size
      in: query
      schema:
        type: integer
        minimum: 1
        maximum: 100
        default: 50
    AgentIdParam:
      name: agent_id
      in: path
      required: true
      schema:
        type: string
        pattern: "^agt_[0-9A-Za-z]{26}$"
    RunIdParam:
      name: run_id
      in: path
      required: true
      schema:
        type: string
        pattern: "^run_[0-9A-Za-z]{26}$"

  responses:
    ValidationError:
      description: Request validation failed
      content:
        application/json:
          schema:
            $ref: "#/components/schemas/ErrorResponse"
    NotFound:
      description: Resource not found
      content:
        application/json:
          schema:
            $ref: "#/components/schemas/ErrorResponse"

  schemas:
    ErrorResponse:
      type: object
      required: [error]
      properties:
        error:
          type: object
          required: [code, message, request_id, timestamp]
          properties:
            code:
              type: string
            message:
              type: string
            details:
              type: object
            request_id:
              type: string
            timestamp:
              type: string
              format: date-time

    # Reference placeholders for full schema definitions
    AgentListResponse:
      type: object
    AgentResponse:
      type: object
    CreateAgentRequest:
      type: object
    RunResponse:
      type: object
    CreateRunRequest:
      type: object
    ApproveEffectRequest:
      type: object
    UploadArtifactRequest:
      type: object
```

---

## 15. Acceptance criteria and verification checklist

### 15.1 API surface acceptance

| ID | Criterion | Verification |
|---|---|---|
| AC-API-001 | All documented REST endpoints return documented response shapes | Provider contract test suite passes |
| AC-API-002 | WebSocket subscriptions deliver events within 500ms of emission | Load test with timing assertions |
| AC-API-003 | Cursor-based pagination works correctly for all list endpoints | Property-based tests across data sizes |
| AC-API-004 | Rate limiting enforces configured limits and returns correct headers | Load test with multiple clients |
| AC-API-005 | API key authentication rejects expired, revoked, and invalid keys | Security test suite |
| AC-API-006 | OAuth flow completes successfully with PKCE | Integration test with test IdP |
| AC-API-007 | Idempotency keys prevent duplicate effect creation | Concurrent request test |
| AC-API-008 | Error responses conform to documented error envelope | Contract test suite |
| AC-API-009 | Unknown JSON fields are ignored (not rejected) | Forward-compatibility test |
| AC-API-010 | API version negotiation works correctly | Multi-version client test |

### 15.2 Schema acceptance

| ID | Criterion | Verification |
|---|---|---|
| AC-SCH-001 | All DTOs validate against their JSON Schema | Automated schema validation in CI |
| AC-SCH-002 | Rust types serialize/deserialize all documented DTOs | Round-trip serde tests |
| AC-SCH-003 | TypeScript types match Rust types for all shared DTOs | Cross-SDK comparison test |
| AC-SCH-004 | Database schema supports all documented queries | Query plan analysis and benchmark |
| AC-SCH-005 | Schema migration v1->v2 preserves all data | Migration test with production-like data |
| AC-SCH-006 | SQLite and PostgreSQL schemas produce identical API responses | Cross-backend integration test |

### 15.3 Configuration acceptance

| ID | Criterion | Verification |
|---|---|---|
| AC-CFG-001 | Minimal config starts the server with safe defaults | Startup test with bare config |
| AC-CFG-002 | Configuration hierarchy resolves correctly | Unit tests per level |
| AC-CFG-003 | Environment variables override file configuration | Integration test |
| AC-CFG-004 | Invalid configuration produces clear error messages | Validation test with bad inputs |
| AC-CFG-005 | `polkagent config validate` catches all documented violations | Comprehensive validation corpus |
| AC-CFG-006 | `polkagent config show` redacts all secret values | Output inspection test |
| AC-CFG-007 | Configuration reload does not drop active connections | Hot-reload integration test |

### 15.4 Migration acceptance

| ID | Criterion | Verification |
|---|---|---|
| AC-MIG-001 | PCA config translates to valid Polkagent config | Translation test with real PCA configs |
| AC-MIG-002 | PCA conversations import without data loss | Diff test: PCA export vs Polkagent import |
| AC-MIG-003 | PCA identity imports and signs correctly | Signature verification test |
| AC-MIG-004 | Dry-run mode makes no changes to target | Filesystem/DB snapshot comparison |
| AC-MIG-005 | Migration is resumable after interruption | Kill-and-resume test |
| AC-MIG-006 | Rollback restores exact pre-migration state | Backup-and-restore round-trip test |
| AC-MIG-007 | Shadow mode compares PCA and Polkagent outputs | Parallel-run comparison test |

### 15.5 Internal API acceptance

| ID | Criterion | Verification |
|---|---|---|
| AC-INT-001 | Effect processor rejects requests exceeding active grant | Policy enforcement test |
| AC-INT-002 | Signer service receives only canonical payload, never model context | Data-flow audit test |
| AC-INT-003 | Event bus delivers events in sequence order per run | Ordering test with concurrent runs |
| AC-INT-004 | Harness lifecycle commands produce expected state transitions | State machine test |
| AC-INT-005 | Policy evaluator produces deterministic decisions | Property-based test with identical inputs |

### 15.6 SDK acceptance

| ID | Criterion | Verification |
|---|---|---|
| AC-SDK-001 | Rust SDK round-trips all public API operations | Integration test against running server |
| AC-SDK-002 | TypeScript SDK round-trips all public API operations | Integration test against running server |
| AC-SDK-003 | SDK code generation is deterministic | Hash comparison of generated output |
| AC-SDK-004 | SDKs handle server errors gracefully with typed error types | Error-injection test |
| AC-SDK-005 | SDKs reconnect WebSocket connections automatically | Network-fault test |

### 15.7 Verification schedule

| Phase | Scope |
|---|---|
| Phase 0 | AC-API-001 through AC-API-008, AC-SCH-001 through AC-SCH-004, AC-CFG-001 through AC-CFG-004, AC-INT-001 through AC-INT-005 |
| Phase 1 | AC-API-009, AC-API-010, AC-SCH-005, AC-SCH-006, AC-CFG-005 through AC-CFG-007, AC-SDK-001 through AC-SDK-005 |
| Phase 2 | AC-MIG-001 through AC-MIG-007 |
| Phase 3+ | Ongoing regression as features mature |

---

## 16. Cross-document interfaces

This PRD interfaces with other PRDs as follows:

| PRD | Interface |
|---|---|
| PRD-02 (Vocabulary/Architecture) | Canonical vocabulary for all DTO and table names |
| PRD-03 (Execution Model) | Run, turn, effect state machines that the API exposes |
| PRD-04 (Providers/Harnesses) | Provider and harness lifecycle APIs |
| PRD-06 (PCA Compatibility) | Migration schemas, C0-C3 compatibility contracts |
| PRD-07 (Identity/Security) | Authentication, authorization, grant resolution |
| PRD-08 (Payments) | Payment intent and receipt APIs |
| PRD-09 (Memory) | Memory query and management APIs |
| PRD-10 (Data/Artifacts) | Artifact and event schemas, storage contracts |
| PRD-11 (Cloud/Tenancy) | Multi-tenant API isolation, control-plane APIs |
| PRD-12 (Marketplace) | Registry and package management APIs |
| PRD-13 (UX) | API consumption patterns for each surface |
| PRD-15 (Testing/Security) | Contract test framework, security test requirements |

---

## 17. Glossary

| Term | Definition |
|---|---|
| **AgentSpec** | The portable, versioned declarative record defining an agent's behavior, routes, capabilities, memory, surfaces, policy, and deployment requirements. |
| **Contract test** | A test that verifies a service produces or consumes messages conforming to a documented schema. |
| **Cursor** | An opaque pagination token that identifies a position in a result set. |
| **EffectAttempt** | One claim/lease/retry lifecycle for a single attempt to execute an effect. |
| **EffectIntent** | The durable command recorded before external I/O begins. |
| **EffectOutcome** | The immutable observed result of exactly one attempt. |
| **Grant** | The resolved set of permissions and limits available to a run or effect. |
| **Idempotency key** | A client-provided string ensuring at-most-once semantics for mutating operations. |
| **PCA** | polkadot-chat-agents, the existing Node.js reference chat-agent product. |
| **PolicyDecision** | The deterministic allow/deny/require-approval result of policy evaluation. |
| **Profile** | A pinned chain/network identity with genesis hash, spec version, and metadata hash. |
| **Projection** | A read-optimized view derived from kernel state. |
| **ResolvedGrant** | An immutable, hashed snapshot of effective permissions bound to an effect. |
| **ULID** | Universally Unique Lexicographically Sortable Identifier, used for all resource IDs. |
| **Wire format** | The serialized representation (JSON, Protocol Buffers) crossing a network boundary. |

---

*End of PRD-14.*

---

## APPENDIX A: API IMPLEMENTATION BLUEPRINT

### A.1 Complete REST API Endpoints

This appendix provides a handler-by-handler implementation reference for every
public REST endpoint. Each entry follows the form: method, path, description,
request body, response body, relevant status codes, authentication scope, and
rate-limit classification.

All endpoints live under `/api/v1alpha1/`. Authentication uses
`Authorization: Bearer pak_...` (API key) or `Authorization: Bearer <JWT>`
(OAuth). Scopes reference the capability strings defined in section 2.6.

---

#### Resource: Agents

```
POST /api/v1alpha1/agents
  Description: Create a new agent from an AgentSpec.
  Auth scope: agents:write
  Rate limit: standard (600 req/min)
  Request body:
    {
      "spec": { <AgentSpec> }
    }
  Response body (201):
    {
      "id": "agt_{ulid}",
      "spec": { <AgentSpec> },
      "revision": 1,
      "status": "stopped",
      "created_at": "<ISO 8601>",
      "updated_at": "<ISO 8601>",
      "created_by": "usr_{ulid}"
    }
  Status codes:
    201 Created          Agent created successfully
    422 Unprocessable    AgentSpec fails schema validation
    409 Conflict         Agent with same name already exists for principal

GET /api/v1alpha1/agents
  Description: List agents. Cursor-paginated. Filterable by status, label, search.
  Auth scope: agents:read
  Rate limit: standard
  Query params: status, label, search, sort, order, cursor, page_size
  Response body (200):
    {
      "data": [ <AgentResponse>, ... ],
      "cursor": { "next": "<opaque>", "has_more": true },
      "meta": { "page_size": 50, "total_estimate": 123 }
    }

GET /api/v1alpha1/agents/{agent_id}
  Description: Get a single agent by ID including current status.
  Auth scope: agents:read
  Rate limit: standard
  Response body (200): <AgentResponse>
  Status codes:
    200 OK
    404 Not Found        AGENT_NOT_FOUND

PUT /api/v1alpha1/agents/{agent_id}
  Description: Update agent spec. Creates a new revision; does not restart agent.
  Auth scope: agents:write
  Rate limit: standard
  Request body: { "spec": { <AgentSpec> }, "change_summary": "optional note" }
  Response body (200): <AgentResponse> with incremented revision

DELETE /api/v1alpha1/agents/{agent_id}
  Description: Soft-delete (archive) an agent. Running agents are stopped first.
  Auth scope: agents:write
  Rate limit: standard
  Response body (200): { "archived_at": "<ISO 8601>" }

POST /api/v1alpha1/agents/{agent_id}/start
  Description: Transition agent from stopped to running state.
  Auth scope: agents:write
  Rate limit: standard
  Response body (200): { "status": "running", "started_at": "<ISO 8601>" }
  Status codes:
    200 OK
    409 Conflict         Agent already running

POST /api/v1alpha1/agents/{agent_id}/stop
  Description: Drain and stop a running agent. Waits for active runs to complete.
  Auth scope: agents:write
  Rate limit: standard
  Request body: { "force": false }   // force=true kills immediately
  Response body (200): { "status": "stopped", "stopped_at": "<ISO 8601>" }

GET /api/v1alpha1/agents/{agent_id}/status
  Description: Get real-time agent runtime status, active run count, and health.
  Auth scope: agents:read
  Rate limit: high-frequency (6000 req/min)
  Response body (200):
    {
      "status": "running",
      "active_runs": 3,
      "health": { "model_provider": "healthy", "signer": "healthy" },
      "uptime_seconds": 3600
    }

GET /api/v1alpha1/agents/{agent_id}/revisions
  Description: List all spec revisions for an agent.
  Auth scope: agents:read
  Rate limit: standard
  Response body (200): paginated list of { revision, created_at, created_by, change_summary }

GET /api/v1alpha1/agents/{agent_id}/revisions/{rev}
  Description: Get the full AgentSpec at a specific revision number.
  Auth scope: agents:read
  Rate limit: standard
  Response body (200): { "revision": 3, "spec": { <AgentSpec> }, "created_at": "..." }
```

---

#### Resource: Runs

```
POST /api/v1alpha1/runs
  Description: Create and immediately start a new run.
  Auth scope: runs:write
  Rate limit: standard
  Idempotency key: Required when run may trigger chain actions or payments.
  Request body:
    {
      "agent_id": "agt_{ulid}",
      "input": {
        "type": "user_message",
        "content": "...",
        "attachments": []
      },
      "context": {
        "conversation_id": "conv_{ulid}",    // optional: continue conversation
        "workspace_id": "ws_{ulid}",         // optional: use existing workspace
        "chain_profile_override": null       // optional: override agent's chain profile
      },
      "options": {
        "model_override": null,
        "max_turns": 20,
        "timeout_seconds": 600
      }
    }
  Response body (201): <RunResponse>
  Status codes:
    201 Created
    422 Unprocessable   Missing required fields, invalid agent_id
    402 Payment Req.    Budget exhausted (BUDGET_EXCEEDED)
    409 Conflict        Idempotency key reused with different payload

GET /api/v1alpha1/runs
  Description: List runs. Filterable by agent_id, status, conversation_id.
  Auth scope: runs:read
  Rate limit: standard
  Query params: agent_id, conversation_id, status, since, until, cursor, page_size
  Response body (200): paginated list of <RunResponse>

GET /api/v1alpha1/runs/{run_id}
  Description: Get run details including current status and usage.
  Auth scope: runs:read
  Rate limit: high-frequency
  Response body (200): <RunResponse>

POST /api/v1alpha1/runs/{run_id}/cancel
  Description: Request cancellation of an active run. Transitions to 'cancelling'.
  Auth scope: runs:write
  Rate limit: standard
  Response body (200): { "status": "cancelling" }

POST /api/v1alpha1/runs/{run_id}/resume
  Description: Resume a run in waiting_input or paused state.
  Auth scope: runs:write
  Rate limit: standard
  Request body: { "input": { "type": "user_message", "content": "..." } }
  Response body (200): <RunResponse>

GET /api/v1alpha1/runs/{run_id}/stream
  Description: Stream run events as Server-Sent Events. See Appendix A.3.
  Auth scope: runs:read
  Accept: text/event-stream
  Rate limit: streaming (max 100 concurrent per principal)

GET /api/v1alpha1/runs/{run_id}/turns
  Description: List all turns completed within a run.
  Auth scope: runs:read
  Rate limit: standard
  Response body (200): paginated list of { id, sequence, role, status, tokens, ... }

GET /api/v1alpha1/runs/{run_id}/events
  Description: List events for a run. Ordered by sequence.
  Auth scope: runs:read
  Rate limit: standard
  Query params: type, after_sequence, since, cursor, page_size

GET /api/v1alpha1/runs/{run_id}/effects
  Description: List effects for a run.
  Auth scope: runs:read
  Rate limit: standard

GET /api/v1alpha1/runs/{run_id}/artifacts
  Description: List artifacts produced in a run.
  Auth scope: artifacts:read
  Rate limit: standard

GET /api/v1alpha1/runs/{run_id}/usage
  Description: Get token usage and cost summary for a run.
  Auth scope: runs:read
  Rate limit: standard
  Response body (200):
    {
      "input_tokens": 12500,
      "output_tokens": 3200,
      "cache_read_tokens": 8000,
      "model_cost_usd": 0.3145,
      "turns_completed": 7
    }
```

---

#### Resource: Turns

```
GET /api/v1alpha1/turns/{turn_id}
  Description: Get details for a specific turn including input and output.
  Auth scope: runs:read
  Rate limit: standard
  Response body (200):
    {
      "id": "turn_{ulid}",
      "run_id": "run_{ulid}",
      "sequence": 3,
      "role": "assistant",
      "status": "completed",
      "input_json": { ... },
      "output_json": { ... },
      "input_tokens": 1800,
      "output_tokens": 450,
      "model_id": "model_{ulid}",
      "started_at": "...",
      "completed_at": "..."
    }
```

---

#### Resource: Effects

```
GET /api/v1alpha1/effects
  Description: List effects. Filterable by run_id, type, status.
  Auth scope: runs:read
  Rate limit: standard
  Query params: run_id, agent_id, type, status, since, cursor, page_size

GET /api/v1alpha1/effects/{effect_id}
  Description: Get full effect details including intent, policy decision, and attempts.
  Auth scope: runs:read
  Rate limit: standard

POST /api/v1alpha1/effects/{effect_id}/approve
  Description: Approve a pending_approval effect. Triggers execution.
  Auth scope: effects:write
  Rate limit: standard
  Idempotency key: Required.
  Request body:
    {
      "approval": {
        "type": "human",
        "principal_id": "usr_{ulid}",
        "comment": "Reviewed and approved",
        "conditions": null
      }
    }
  Response body (200): <EffectResponse> with status "approved"
  Status codes:
    200 OK
    409 Conflict        Effect already resolved
    404 Not Found       EFFECT_NOT_FOUND

POST /api/v1alpha1/effects/{effect_id}/deny
  Description: Deny a pending_approval effect.
  Auth scope: effects:write
  Rate limit: standard
  Idempotency key: Required.
  Request body: { "reason": "Risk level too high" }
  Response body (200): <EffectResponse> with status "denied"

GET /api/v1alpha1/effects/{effect_id}/attempts
  Description: List execution attempts for an effect.
  Auth scope: runs:read
  Rate limit: standard
  Response body (200): list of { id, attempt_number, status, started_at, completed_at, error }

GET /api/v1alpha1/effects/{effect_id}/outcome
  Description: Get the resolved outcome for an effect.
  Auth scope: runs:read
  Rate limit: standard
  Response body (200): <EffectOutcome>
  Status codes:
    200 OK
    404 Not Found       Effect not yet resolved
```

---

#### Resource: Artifacts

```
GET /api/v1alpha1/artifacts
  Description: List artifacts. Filterable by run_id, type, classification.
  Auth scope: artifacts:read
  Rate limit: standard
  Query params: run_id, agent_id, type, classification, since, cursor, page_size

GET /api/v1alpha1/artifacts/{artifact_id}
  Description: Get artifact metadata (not content).
  Auth scope: artifacts:read
  Rate limit: standard
  Response body (200): <ArtifactMetadata>

GET /api/v1alpha1/artifacts/{artifact_id}/content
  Description: Download artifact binary content. Streams directly.
  Auth scope: artifacts:read
  Rate limit: download (10 GB/day per principal)
  Response: Binary stream with Content-Type header matching artifact.content_type

POST /api/v1alpha1/artifacts
  Description: Upload a new artifact. multipart/form-data.
  Auth scope: artifacts:write
  Rate limit: standard
  Request: multipart/form-data with fields: file, run_id, type, classification, filename, metadata
  Response body (201): <ArtifactMetadata>
  Status codes:
    201 Created
    413 Too Large       Exceeds max_artifact_size_bytes

GET /api/v1alpha1/artifacts/{artifact_id}/provenance
  Description: Get the full provenance chain for an artifact.
  Auth scope: artifacts:read
  Rate limit: standard
  Response body (200):
    {
      "artifact_id": "art_{ulid}",
      "chain": [
        { "artifact_id": "art_{ulid}", "created_by_type": "tool", "tool_id": "filesystem.write", ... },
        { "artifact_id": "art_{ulid}", "created_by_type": "agent", "parent_id": null, ... }
      ]
    }
```

---

#### Resource: Events

```
GET /api/v1alpha1/events
  Description: Query persisted events. Ordered by sequence within a run.
  Auth scope: runs:read
  Rate limit: standard
  Query params: run_id, agent_id, type (comma-sep), after_sequence, since, until, cursor, page_size
  Response body (200): paginated list of <EventResponse>

GET /api/v1alpha1/events/{event_id}
  Description: Get a single event by ID.
  Auth scope: runs:read
  Rate limit: standard
  Response body (200): <EventResponse>

POST /api/v1alpha1/events/subscribe
  Description: Create a named WebSocket subscription (returns a subscription token).
  Auth scope: runs:read
  Rate limit: standard
  Request body:
    {
      "channels": ["runs:run_{ulid}", "effects:eff_{ulid}"],
      "back_pressure": "at_most_once"
    }
  Response body (201): { "subscription_id": "sub_{ulid}", "token": "..." }
```

---

#### Resource: Memory

```
POST /api/v1alpha1/memory/query
  Description: Query memory entries by relevance, type, and time range.
  Auth scope: memory:read
  Rate limit: standard
  Request body: <MemoryQueryRequest>
  Response body (200): <MemoryQueryResponse>

POST /api/v1alpha1/memory/forget
  Description: Delete memory entries matching a filter.
  Auth scope: memory:write
  Rate limit: standard
  Request body:
    {
      "agent_id": "agt_{ulid}",
      "filter": {
        "ids": ["mem_{ulid}", ...],      // explicit IDs, OR
        "tags": ["staking"],             // by tag, OR
        "before": "<ISO 8601>"           // by age
      }
    }
  Response body (200): { "deleted_count": 12 }

POST /api/v1alpha1/memory/export
  Description: Export all memory entries for an agent as a portable JSON-lines archive.
  Auth scope: memory:read
  Rate limit: download
  Response: JSON-lines stream (application/x-ndjson)

GET /api/v1alpha1/memory/stats
  Description: Get memory usage statistics for an agent.
  Auth scope: memory:read
  Rate limit: standard
  Query params: agent_id (required)
  Response body (200):
    {
      "agent_id": "agt_{ulid}",
      "entry_count": 842,
      "entry_count_by_type": { "episodic": 600, "semantic": 200, "procedural": 42 },
      "oldest_entry": "2026-01-01T00:00:00Z",
      "newest_entry": "2026-07-30T12:00:00Z",
      "embedding_dimensions": 1536
    }

GET /api/v1alpha1/memory/entries/{entry_id}
  Description: Get a single memory entry by ID.
  Auth scope: memory:read
  Rate limit: standard
  Response body (200): <MemoryEntryResponse>
```

---

#### Resource: Chains

```
GET /api/v1alpha1/chains
  Description: List configured chain profiles and their connectivity status.
  Auth scope: agents:read
  Rate limit: standard
  Response body (200):
    [
      {
        "id": "polkadot-production",
        "name": "Polkadot",
        "genesis_hash": "0x91b171...",
        "status": "connected",
        "latest_block": 21000000,
        "spec_version": 1003000,
        "metadata_hash": "0xabc..."
      }
    ]

GET /api/v1alpha1/chains/{chain_id}
  Description: Get detailed chain profile including current spec version and RPC latency.
  Auth scope: agents:read
  Rate limit: standard

POST /api/v1alpha1/chains/{chain_id}/dry-run
  Description: Submit a call for dry-run simulation against a chain profile.
  Auth scope: agents:write
  Rate limit: standard
  Request body:
    {
      "account": "5GrwvaEF...",
      "call_data": "0x0500...",
      "block_hash": null    // null = best block
    }
  Response body (200):
    {
      "success": true,
      "events": [ ... ],
      "gas_consumed": null,
      "fee_estimate": { "partial_fee": "125000000", "asset": "DOT" }
    }
```

---

#### Resource: Marketplace (beta)

```
GET /api/v1alpha1/marketplace/packages
  Description: Search and list packages available in the skill registry.
  Auth scope: agents:read
  Rate limit: standard
  Query params: q (search), category, publisher, cursor, page_size
  Response body (200): paginated list of <PackageSummary>

GET /api/v1alpha1/marketplace/packages/{package}
  Description: Get package metadata, versions, and changelog.
  Auth scope: agents:read
  Rate limit: standard

GET /api/v1alpha1/marketplace/packages/{package}/versions/{version}
  Description: Get specific version manifest including integrity hash.
  Auth scope: agents:read
  Rate limit: standard
  Response body (200):
    {
      "package": "opengov-monitor",
      "version": "1.4.2",
      "integrity": "sha256:abc...",
      "manifest": { ... },
      "publisher": "parity-technologies",
      "published_at": "2026-06-01T00:00:00Z"
    }

POST /api/v1alpha1/marketplace/packages/{package}/install
  Description: Install a package for a specific agent.
  Auth scope: agents:write
  Rate limit: standard
  Request body: { "agent_id": "agt_{ulid}", "version": "1.4.2", "config_overrides": {} }
  Response body (200): { "installed": true, "agent_id": "agt_{ulid}", "version": "1.4.2" }
```

---

#### System Endpoints

```
GET /api/v1alpha1/health
  Description: Liveness and readiness check. No authentication required.
  Auth scope: none
  Rate limit: unlimited
  Response body (200):
    {
      "status": "ok",
      "version": "0.1.0",
      "db": "ok",
      "providers": { "anthropic-default": "ok" },
      "uptime_seconds": 86400
    }

GET /api/v1alpha1/versions
  Description: List supported API versions and their maturity stages.
  Auth scope: none
  Rate limit: unlimited
  Response body (200):
    {
      "versions": [
        { "version": "v1alpha1", "stage": "alpha", "status": "current" }
      ],
      "current": "v1alpha1"
    }

GET /api/v1alpha1/me
  Description: Resolve the authenticated principal (user or API key metadata).
  Auth scope: any
  Rate limit: standard
  Response body (200):
    {
      "principal_id": "usr_{ulid}",
      "type": "user",
      "scopes": ["agents:read", "agents:write", "runs:read", "runs:write"],
      "key_prefix": "pak_abc1",
      "expires_at": null
    }
```

---

### A.2 WebSocket API

The WebSocket endpoint is at `/ws/v1alpha1`. It uses the Axum WebSocket
upgrade pattern and a command envelope distinct from the global run-event
WebSocket documented below.

#### Connection lifecycle

```
1. Client sends HTTP GET /ws/v1alpha1 with Upgrade: websocket header.
   Optional: ?token=pak_... for query-param auth.

2. Server requires a configured EventStore (otherwise HTTP 501), attaches its
   live EventBus receiver, and upgrades the connection.

3. Client authenticates (if not using query param):
   SEND  {"msg_type":"auth","token":"pak_..."}
   RECV  {"msg_type":"ack","id":null,"channel":null,"payload":null,
          "timestamp":"..."}
   or
   RECV  {"msg_type":"error","payload":{"reason":"invalid or missing token"},
          "timestamp":"..."}

4. Client subscribes one channel per command (maximum 256 distinct channels):
   SEND  {"msg_type":"subscribe","id":"req-1",
          "channel":"runs:{run_uuid}"}
   RECV  {"msg_type":"ack","id":"req-1",
          "channel":"runs:{run_uuid}","payload":null,"timestamp":"..."}

5. Server streams events:
   RECV  {"msg_type":"event","id":null,"channel":"runs:{run_uuid}",
          "payload":{...RunEvent...},"timestamp":"..."}

6. Keep-alive (server WebSocket Ping control frame every 30s):
   RECV  Ping
   SEND  Pong
   Timeout: 30s without pong closes the connection.

7. Client unsubscribes:
   SEND  {"msg_type":"unsubscribe","id":"req-2",
          "channel":"runs:{run_uuid}"}
   RECV  {"msg_type":"ack","id":"req-2",
          "channel":"runs:{run_uuid}","payload":null,"timestamp":"..."}

8. Connection close:
   Either side sends a WebSocket Close control frame. There is no JSON close
   command.
```

The actual client command discriminator is `msg_type`, not `type`. Commands are
`auth`, `subscribe`, `unsubscribe`, and `ping`; there is no cursor,
back-pressure declaration, multi-channel array, cancellation, or JSON close
command. The implemented channels are `runs:{run_id}`, `agents:{agent_id}`, and
syntactically `system`. Run and agent routing is active. `system` currently has
no producer in the `RunEvent` source; effect and conversation channels are not
implemented. Event envelopes always report the concrete run channel, including
when an agent subscription selected the event. Per-message/frame size caps
remain an open hardening item rather than an implemented guarantee.

#### Agent output subscription

For interactive runs where the client wants streaming token output, subscribe
to the `runs:{run_id}` channel after creating the run:

```json
// Subscribe after POST /runs returns run_id
{"msg_type":"subscribe","id":"run-output","channel":"runs:0198bd19-40c0-7000-8000-000000000001"}

// Receive streaming events
{"msg_type":"event","id":null,
 "channel":"runs:0198bd19-40c0-7000-8000-000000000001",
 "payload":{"id":"...","run_id":"...","sequence":1,
            "kind":{"streaming_token":{"text":"The staking pallet"}},
            "durability":"ephemeral","timestamp":"..."},
 "timestamp":"..."}
```

#### Event subscription endpoint

Multiple single-channel commands allow a connection to track multiple runs and
agents simultaneously:

```json
{"msg_type":"subscribe","id":"a","channel":"agents:0198bd19-40c0-7000-8000-000000000010"}
{"msg_type":"subscribe","id":"r","channel":"runs:0198bd19-40c0-7000-8000-000000000001"}
```

#### Real-time notifications

Subscribed run/agent events are forwarded live. The accepted `system` channel
does not yet deliver anything because the source is a run-event bus with no
implemented system-event producer.

#### Lag recovery and reconnect limitation

The receiver attaches before upgrade completion. Its first valid durable point
lookup establishes an internal global checkpoint after successful delivery (or
after a non-matching row is skipped). Later durable notifications and receiver
lag page from `EventStore` after that checkpoint, validate forward progress,
and deduplicate replay/live overlap. Lag before the first checkpoint, malformed
projection, or backend failure sends a generic `error` envelope and then closes
with status 1011. No backend detail is serialized.

The command envelope intentionally remains unchanged and exposes neither a
cursor input nor a global checkpoint output. Reconnecting therefore starts a
new live-only session and does not recover the disconnected interval.
Diagnostic/ephemeral events may also be lost during lag. Consumers requiring a
durable reconnect cursor use `/api/v1alpha1/events/stream` or interaction SSE.

---

### A.3 Server-Sent Events

The SSE endpoint at `/api/v1alpha1/runs/{run_id}/stream` implements the
pattern from Roko's `routes/sse.rs`: monotonic IDs, gap detection, and
materialized snapshots for lag recovery.

#### Event format

Each SSE frame carries:

```
id: 42
event: data
data: {"type":"turn.token_delta","run_id":"run_01HQ...","turn_id":"turn_01HQ...",
       "delta":"The staking pallet","token_index":0}

id: 43
event: data
data: {"type":"effect.pending_approval","effect_id":"eff_01HQ...","summary":"..."}

id: 44
event: data
data: {"type":"run.completed","run_id":"run_01HQ...","terminal_reason":"success"}
```

Gap events (when client cursor falls outside the ring buffer):

```
id: 512
event: gap
data: {"missed_events":68,"last_materialized_seq":511,"snapshot":{...}}
```

Keep-alive comments (every 8 seconds to prevent proxy timeouts):

```
: keepalive
```

#### Subscription management

To watch a run:

```http
GET /api/v1alpha1/runs/{run_id}/stream HTTP/1.1
Authorization: Bearer pak_...
Accept: text/event-stream
Last-Event-ID: 41    (optional: resume from seq 42)
```

Response headers (from Roko pattern):

```http
Content-Type: text/event-stream
Cache-Control: no-cache, no-store, no-transform, must-revalidate
Connection: keep-alive
X-Accel-Buffering: no          (disables Nginx/Railway buffering)
```

#### Reconnection with Last-Event-ID

The client must store the `id:` from the last received frame and include it
as `Last-Event-ID` on reconnect. The server resumes from `last_seen + 1`.
If the requested sequence is not in the buffer, the server responds with a
`gap` event containing a full materialized snapshot.

#### Global run-event WebSocket endpoint

The implemented global endpoint uses a WebSocket upgrade and streams events
across the globally configured runtime for an authorized connection:

```http
GET /api/v1alpha1/events/stream?after_sequence=41&kinds=run_started,run_completed HTTP/1.1
Authorization: Bearer pak_...
Connection: Upgrade
Upgrade: websocket
```

It accepts an optional non-negative global `after_sequence`, a UUID `run_id`,
and comma-separated `kinds`. Durable JSON frames add `global_sequence`; clients
persist it for reconnect. The receiver attaches before bounded durable replay,
and broadcast lag replays after the last consumed checkpoint without duplicate
durable frames. Diagnostic/ephemeral frames are live-only. Missing storage
rejects the upgrade with `501`; backend recovery failure closes with a generic
1011 reason. This is distinct from interaction SSE and `/ws/v1alpha1`.
Authentication/authorization gates the connection, but stored event rows are
not currently scoped by tenant or principal. Tenant/principal row filtering is
therefore still an open SEC-01/EVD-10 boundary and callers must not infer
per-principal isolation from this endpoint.

---

### A.4 Error Format

The canonical error envelope (section 2.5 of this PRD) is:

```json
{
  "error": {
    "code": "AGENT_NOT_FOUND",
    "message": "No agent with ID agt_01HQXYZ was found for this principal.",
    "details": {
      "agent_id": "agt_01HQXYZ"
    },
    "request_id": "req_01HRABC",
    "timestamp": "2026-07-30T12:00:00.000Z"
  }
}
```

#### Complete error code catalog

| Code | HTTP Status | Category | Description |
|---|---|---|---|
| `AUTHENTICATION_REQUIRED` | 401 | Auth | No valid credentials provided |
| `TOKEN_EXPIRED` | 401 | Auth | Bearer token has expired |
| `TOKEN_REVOKED` | 401 | Auth | API key has been revoked |
| `INSUFFICIENT_PERMISSIONS` | 403 | Auth | Principal lacks required capability scope |
| `SCOPE_EXCEEDED` | 403 | Auth | API key used outside its declared scope |
| `RESOURCE_NOT_FOUND` | 404 | Resource | Generic: referenced resource does not exist |
| `AGENT_NOT_FOUND` | 404 | Resource | Specific agent ID not found |
| `RUN_NOT_FOUND` | 404 | Resource | Specific run ID not found |
| `EFFECT_NOT_FOUND` | 404 | Resource | Specific effect ID not found |
| `ARTIFACT_NOT_FOUND` | 404 | Resource | Specific artifact ID not found |
| `PROVIDER_NOT_FOUND` | 404 | Resource | Provider ID not configured |
| `RESOURCE_CONFLICT` | 409 | Idempotency | Idempotency key reused with different payload |
| `AGENT_ALREADY_RUNNING` | 409 | State | Agent start requested but already running |
| `EFFECT_ALREADY_RESOLVED` | 409 | State | Approve/deny on already-resolved effect |
| `RUN_NOT_CANCELLABLE` | 409 | State | Cancel on completed/failed run |
| `VALIDATION_ERROR` | 422 | Validation | Request body fails JSON Schema validation |
| `MISSING_IDEMPOTENCY_KEY` | 422 | Validation | Required X-Idempotency-Key header absent |
| `INVALID_AGENT_SPEC` | 422 | Validation | AgentSpec is structurally invalid |
| `INVALID_CHAIN_PROFILE` | 422 | Validation | Requested chain profile not configured |
| `BUDGET_EXCEEDED` | 402 | Budget | Model cost budget exhausted |
| `DAILY_BUDGET_EXCEEDED` | 402 | Budget | Daily budget ceiling reached |
| `RATE_LIMIT_EXCEEDED` | 429 | Rate | Standard rate limit window exhausted |
| `EFFECT_DENIED` | 403 | Policy | Policy evaluation denied the effect |
| `GRANT_INSUFFICIENT` | 403 | Policy | Active grant does not cover requested operation |
| `MANDATE_EXPIRED` | 403 | Policy | Autonomous mandate has expired |
| `FEATURE_DISABLED` | 501 | Feature | Requested feature is disabled in this deployment |
| `FEATURE_BETA` | 200+warn | Feature | Feature is beta (warning in response body) |
| `PROVIDER_ERROR` | 502 | Upstream | Model provider returned an error |
| `PROVIDER_TIMEOUT` | 504 | Upstream | Model provider timed out |
| `PROVIDER_RATE_LIMITED` | 502 | Upstream | Upstream provider rate-limited the request |
| `SIGNER_ERROR` | 502 | Upstream | Signer service error |
| `SIGNER_TIMEOUT` | 504 | Upstream | Signer service timed out |
| `CHAIN_ERROR` | 502 | Upstream | Chain RPC node returned an error |
| `CHAIN_TIMEOUT` | 504 | Upstream | Chain RPC node timed out |
| `CHAIN_DRY_RUN_FAILED` | 422 | Chain | Dry-run simulation rejected the call |
| `ARTIFACT_TOO_LARGE` | 413 | Storage | Upload exceeds max_artifact_size_bytes |
| `STORAGE_FULL` | 507 | Storage | Total storage quota exceeded |
| `INTERNAL_ERROR` | 500 | Internal | Unexpected server-side error |
| `NOT_IMPLEMENTED` | 501 | Internal | Endpoint exists but is not yet implemented |

---

## APPENDIX B: DATABASE SCHEMAS

### B.1 SQLite v1 Schema (Local)

The complete schema for the SQLite authority store. Apply in sequence;
`schema_migrations` must be created first.

```sql
-- ─────────────────────────────────────────────────────────────────────────────
-- Schema version tracking
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE schema_migrations (
    version         INTEGER PRIMARY KEY,
    description     TEXT NOT NULL,
    applied_at      TEXT NOT NULL,          -- ISO 8601
    checksum        TEXT NOT NULL           -- sha256:{hex} of migration SQL
);

-- ─────────────────────────────────────────────────────────────────────────────
-- Principals and identity
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE principals (
    id              TEXT PRIMARY KEY,       -- usr_{ulid} | svc_{ulid}
    type            TEXT NOT NULL,          -- user | service_account
        -- CHECK (type IN ('user','service_account'))
    display_name    TEXT,
    email           TEXT UNIQUE,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    deactivated_at  TEXT
);

CREATE INDEX idx_principals_type ON principals(type);
CREATE INDEX idx_principals_email ON principals(email) WHERE email IS NOT NULL;

CREATE TABLE api_keys (
    id              TEXT PRIMARY KEY,       -- key_{ulid}
    principal_id    TEXT NOT NULL REFERENCES principals(id),
    name            TEXT NOT NULL,
    key_hash        TEXT NOT NULL UNIQUE,   -- argon2id hash of raw key
    prefix          TEXT NOT NULL,          -- first 8 chars: pak_XXXXXXXX
    scopes_json     TEXT NOT NULL,          -- JSON array of capability strings
    ip_allowlist    TEXT,                   -- JSON array of CIDRs (null = any)
    expires_at      TEXT,                   -- null = never
    revoked_at      TEXT,                   -- null = active
    last_used_at    TEXT,
    use_count       INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL
);

CREATE INDEX idx_api_keys_principal ON api_keys(principal_id);
CREATE INDEX idx_api_keys_prefix    ON api_keys(prefix);
-- Fast lookup by hash during auth:
CREATE INDEX idx_api_keys_hash      ON api_keys(key_hash) WHERE revoked_at IS NULL;

-- ─────────────────────────────────────────────────────────────────────────────
-- Agents and revisions
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE agents (
    id              TEXT PRIMARY KEY,       -- agt_{ulid}
    name            TEXT NOT NULL,
    description     TEXT,
    spec_json       TEXT NOT NULL,          -- current AgentSpec as JSON
    revision        INTEGER NOT NULL DEFAULT 1,
    status          TEXT NOT NULL DEFAULT 'stopped',
        -- CHECK (status IN ('running','stopped','archived'))
    created_by      TEXT NOT NULL REFERENCES principals(id),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    archived_at     TEXT,

    UNIQUE(name, created_by)
);

CREATE INDEX idx_agents_status      ON agents(status);
CREATE INDEX idx_agents_created_by  ON agents(created_by);
CREATE INDEX idx_agents_name        ON agents(name);
CREATE INDEX idx_agents_active      ON agents(status) WHERE status != 'archived';

CREATE TABLE agent_revisions (
    id              TEXT PRIMARY KEY,       -- rev_{ulid}
    agent_id        TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    revision        INTEGER NOT NULL,
    spec_json       TEXT NOT NULL,
    created_by      TEXT NOT NULL REFERENCES principals(id),
    created_at      TEXT NOT NULL,
    change_summary  TEXT,

    UNIQUE(agent_id, revision)
);

CREATE INDEX idx_agent_revisions_agent ON agent_revisions(agent_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- Workspaces
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE workspaces (
    id              TEXT PRIMARY KEY,       -- ws_{ulid}
    agent_id        TEXT REFERENCES agents(id),
    owner_id        TEXT REFERENCES principals(id),
    status          TEXT NOT NULL DEFAULT 'active',
        -- CHECK (status IN ('active','archived'))
    storage_backend TEXT NOT NULL DEFAULT 'local',
    storage_ref     TEXT NOT NULL,          -- path or object prefix
    total_size_bytes INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX idx_workspaces_agent   ON workspaces(agent_id);
CREATE INDEX idx_workspaces_owner   ON workspaces(owner_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- Conversations and runs
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE conversations (
    id              TEXT PRIMARY KEY,       -- conv_{ulid}
    agent_id        TEXT REFERENCES agents(id),
    title           TEXT,
    status          TEXT NOT NULL DEFAULT 'active',
        -- CHECK (status IN ('active','archived'))
    created_by      TEXT NOT NULL REFERENCES principals(id),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    transport_type  TEXT,                   -- web | cli | polkadot_chat | api
    transport_meta  TEXT                    -- JSON metadata (session IDs, etc.)
);

CREATE INDEX idx_conversations_agent      ON conversations(agent_id);
CREATE INDEX idx_conversations_created_by ON conversations(created_by);
CREATE INDEX idx_conversations_updated    ON conversations(updated_at DESC);

CREATE TABLE runs (
    id              TEXT PRIMARY KEY,       -- run_{ulid}
    agent_id        TEXT NOT NULL REFERENCES agents(id),
    conversation_id TEXT REFERENCES conversations(id),
    workspace_id    TEXT REFERENCES workspaces(id),

    status          TEXT NOT NULL DEFAULT 'pending',
        -- CHECK (status IN (
        --   'pending','running','waiting_approval','waiting_input',
        --   'paused','cancelling','completed','failed','cancelled','timed_out'))

    input_json      TEXT NOT NULL,          -- serialized RunInput
    config_json     TEXT,                   -- per-run configuration overrides
    grant_hash      TEXT NOT NULL,          -- sha256:{hex} of ResolvedGrant
    grant_json      TEXT NOT NULL,          -- full serialized ResolvedGrant

    turns_completed INTEGER NOT NULL DEFAULT 0,
    terminal_reason TEXT,                   -- detail on completion/failure

    -- Usage accounting
    input_tokens    INTEGER NOT NULL DEFAULT 0,
    output_tokens   INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    model_cost_usd  REAL NOT NULL DEFAULT 0.0,

    -- Idempotency
    idempotency_key TEXT UNIQUE,

    -- Lifecycle timestamps
    created_at      TEXT NOT NULL,
    started_at      TEXT,
    completed_at    TEXT,

    -- Policy binding
    policy_revision TEXT,
    agent_revision  INTEGER,
    created_by      TEXT REFERENCES principals(id)
);

CREATE INDEX idx_runs_agent         ON runs(agent_id);
CREATE INDEX idx_runs_conversation  ON runs(conversation_id);
CREATE INDEX idx_runs_status        ON runs(status);
CREATE INDEX idx_runs_created       ON runs(created_at DESC);
CREATE INDEX idx_runs_idempotency   ON runs(idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE INDEX idx_runs_active        ON runs(status)
    WHERE status IN ('pending','running','waiting_approval','waiting_input','paused');

CREATE TABLE turns (
    id              TEXT PRIMARY KEY,       -- turn_{ulid}
    run_id          TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    sequence        INTEGER NOT NULL,

    role            TEXT NOT NULL,          -- user | assistant | system | tool
    input_json      TEXT,
    output_json     TEXT,

    status          TEXT NOT NULL DEFAULT 'pending',
        -- CHECK (status IN ('pending','running','completed','failed'))

    input_tokens    INTEGER NOT NULL DEFAULT 0,
    output_tokens   INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    model_id        TEXT,
    provider_id     TEXT,

    started_at      TEXT,
    completed_at    TEXT,

    UNIQUE(run_id, sequence)
);

CREATE INDEX idx_turns_run ON turns(run_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- Effects, attempts, and outcomes
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE effects (
    id              TEXT PRIMARY KEY,       -- eff_{ulid}
    run_id          TEXT NOT NULL REFERENCES runs(id),
    turn_id         TEXT REFERENCES turns(id),

    type            TEXT NOT NULL,
        -- model_call | tool_invocation | chain_action | payment |
        -- delivery | signing | approval_request

    status          TEXT NOT NULL DEFAULT 'pending',
        -- CHECK (status IN (
        --   'pending','pending_approval','approved','denied',
        --   'executing','succeeded','failed','cancelled','timed_out','unknown'))

    intent_json     TEXT NOT NULL,          -- serialized EffectIntent
    grant_hash      TEXT NOT NULL,

    idempotency_key TEXT NOT NULL UNIQUE,

    created_at      TEXT NOT NULL,
    resolved_at     TEXT,

    -- Policy binding
    policy_decision TEXT NOT NULL,          -- allow | deny | require_approval
    policy_revision TEXT,
    policy_reasons  TEXT                    -- JSON array of reason strings
);

CREATE INDEX idx_effects_run              ON effects(run_id);
CREATE INDEX idx_effects_status           ON effects(status);
CREATE INDEX idx_effects_type             ON effects(type);
CREATE INDEX idx_effects_idempotency      ON effects(idempotency_key);
CREATE INDEX idx_effects_pending_approval ON effects(created_at DESC)
    WHERE status = 'pending_approval';

CREATE TABLE effect_attempts (
    id              TEXT PRIMARY KEY,       -- att_{ulid}
    effect_id       TEXT NOT NULL REFERENCES effects(id),
    attempt_number  INTEGER NOT NULL,

    status          TEXT NOT NULL DEFAULT 'claimed',
        -- CHECK (status IN (
        --   'claimed','executing','completed','failed','timed_out','cancelled'))

    worker_id       TEXT,
    lease_expires   TEXT,
    retry_reason    TEXT,

    started_at      TEXT NOT NULL,
    completed_at    TEXT,

    UNIQUE(effect_id, attempt_number)
);

CREATE INDEX idx_attempts_effect ON effect_attempts(effect_id);
CREATE INDEX idx_attempts_status ON effect_attempts(status);
CREATE INDEX idx_attempts_lease  ON effect_attempts(lease_expires)
    WHERE status = 'claimed';

CREATE TABLE effect_outcomes (
    id              TEXT PRIMARY KEY,       -- out_{ulid}
    effect_id       TEXT NOT NULL REFERENCES effects(id),
    attempt_id      TEXT NOT NULL REFERENCES effect_attempts(id),

    result          TEXT NOT NULL,
        -- CHECK (result IN ('success','failure','timeout','cancellation','unknown'))

    result_json     TEXT,
    error_json      TEXT,

    -- Chain-specific fields
    tx_hash         TEXT,
    block_hash      TEXT,
    block_number    INTEGER,
    finality_status TEXT,
        -- submitted | included | finalized | failed | unknown

    -- Resource accounting
    duration_ms     INTEGER,
    tokens_used     INTEGER,
    cost_usd        REAL,

    created_at      TEXT NOT NULL,

    UNIQUE(attempt_id)
);

CREATE INDEX idx_outcomes_effect ON effect_outcomes(effect_id);
CREATE INDEX idx_outcomes_result ON effect_outcomes(result);
CREATE INDEX idx_outcomes_tx     ON effect_outcomes(tx_hash)
    WHERE tx_hash IS NOT NULL;

CREATE TABLE approvals (
    id              TEXT PRIMARY KEY,       -- apr_{ulid}
    effect_id       TEXT NOT NULL REFERENCES effects(id),

    type            TEXT NOT NULL,
        -- CHECK (type IN ('human','quorum','service','mandate'))

    principal_id    TEXT REFERENCES principals(id),
    comment         TEXT,
    conditions_json TEXT,

    mandate_ref     TEXT,
    mandate_hash    TEXT,

    decision        TEXT NOT NULL,
        -- CHECK (decision IN ('approved','denied'))

    created_at      TEXT NOT NULL,

    UNIQUE(effect_id)
);

CREATE INDEX idx_approvals_effect    ON approvals(effect_id);
CREATE INDEX idx_approvals_principal ON approvals(principal_id);

-- ─────────────────────────────────────────────────────────────────────────────
-- Artifacts
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE artifacts (
    id              TEXT PRIMARY KEY,       -- art_{ulid}
    run_id          TEXT REFERENCES runs(id),
    turn_id         TEXT REFERENCES turns(id),

    type            TEXT NOT NULL,
        -- code_diff | file | plan | decoded_call | simulation |
        -- receipt | test_result | evidence_package | log

    classification  TEXT NOT NULL DEFAULT 'workspace_output',
        -- workspace_output | evidence | user_upload | system

    content_hash    TEXT NOT NULL,          -- sha256:{hex}
    content_type    TEXT NOT NULL,          -- MIME type
    size_bytes      INTEGER NOT NULL,
    filename        TEXT,

    storage_backend TEXT NOT NULL,          -- local | s3 | inline
    storage_ref     TEXT NOT NULL,

    created_by_type TEXT NOT NULL,          -- agent | user | system | tool
    created_by_id   TEXT NOT NULL,
    parent_id       TEXT REFERENCES artifacts(id),
    tool_id         TEXT,

    retention_policy TEXT NOT NULL DEFAULT 'workspace_default',
    expires_at      TEXT,

    created_at      TEXT NOT NULL,
    metadata_json   TEXT
);

CREATE INDEX idx_artifacts_run     ON artifacts(run_id);
CREATE INDEX idx_artifacts_type    ON artifacts(type);
CREATE INDEX idx_artifacts_hash    ON artifacts(content_hash);
CREATE INDEX idx_artifacts_created ON artifacts(created_at DESC);
CREATE INDEX idx_artifacts_expires ON artifacts(expires_at)
    WHERE expires_at IS NOT NULL;

-- ─────────────────────────────────────────────────────────────────────────────
-- Events (transactional outbox)
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE events (
    id              TEXT PRIMARY KEY,       -- evt_{ulid}
    run_id          TEXT REFERENCES runs(id),
    agent_id        TEXT REFERENCES agents(id),

    sequence        INTEGER NOT NULL,       -- per-run monotonic sequence
    type            TEXT NOT NULL,
    timestamp       TEXT NOT NULL,          -- ISO 8601 with microseconds

    payload_json    TEXT NOT NULL,

    correlation_id  TEXT,
    causation_id    TEXT,

    durability      TEXT NOT NULL DEFAULT 'persistent',
        -- CHECK (durability IN ('persistent','ephemeral'))

    -- Denormalized for query performance
    turn_id         TEXT,
    effect_id       TEXT
);

CREATE UNIQUE INDEX idx_events_run_seq    ON events(run_id, sequence)
    WHERE run_id IS NOT NULL;
CREATE INDEX idx_events_type              ON events(type);
CREATE INDEX idx_events_timestamp         ON events(timestamp DESC);
CREATE INDEX idx_events_correlation       ON events(correlation_id);
CREATE INDEX idx_events_ephemeral         ON events(timestamp)
    WHERE durability = 'ephemeral';

-- ─────────────────────────────────────────────────────────────────────────────
-- Grants, payments, and memory
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE grants (
    hash            TEXT PRIMARY KEY,       -- sha256:{hex}
    grant_json      TEXT NOT NULL,
    policy_revision TEXT NOT NULL,
    agent_id        TEXT REFERENCES agents(id),
    created_at      TEXT NOT NULL
);

CREATE TABLE payment_intents (
    id              TEXT PRIMARY KEY,       -- pay_{ulid}
    run_id          TEXT REFERENCES runs(id),
    agent_id        TEXT REFERENCES agents(id),

    type            TEXT NOT NULL DEFAULT 'chain_transfer',
    status          TEXT NOT NULL DEFAULT 'pending_approval',
        -- CHECK (status IN (
        --   'pending_approval','approved','rejected','executing',
        --   'succeeded','failed','cancelled'))

    intent_json     TEXT NOT NULL,
    evidence_json   TEXT,
    policy_decision TEXT,
    idempotency_key TEXT UNIQUE,

    created_at      TEXT NOT NULL,
    resolved_at     TEXT
);

CREATE INDEX idx_payment_intents_run    ON payment_intents(run_id);
CREATE INDEX idx_payment_intents_agent  ON payment_intents(agent_id);
CREATE INDEX idx_payment_intents_status ON payment_intents(status);

CREATE TABLE memory_entries (
    id              TEXT PRIMARY KEY,       -- mem_{ulid}
    agent_id        TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,

    type            TEXT NOT NULL,
        -- CHECK (type IN ('episodic','semantic','procedural'))

    content         TEXT NOT NULL,
    embedding       BLOB,                   -- float32 vector (dimension varies by model)
    tags_json       TEXT,

    source_type     TEXT NOT NULL,          -- conversation | tool | import | user
    source_ref      TEXT,

    retention_policy TEXT NOT NULL DEFAULT 'agent_default',
    expires_at      TEXT,
    last_accessed   TEXT,

    relevance_score REAL,                   -- updated on each retrieval
    access_count    INTEGER NOT NULL DEFAULT 0,

    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX idx_memory_agent   ON memory_entries(agent_id);
CREATE INDEX idx_memory_type    ON memory_entries(agent_id, type);
CREATE INDEX idx_memory_created ON memory_entries(created_at DESC);
CREATE INDEX idx_memory_expires ON memory_entries(expires_at)
    WHERE expires_at IS NOT NULL;

-- FTS5 virtual table for content search
CREATE VIRTUAL TABLE memory_fts USING fts5(
    content,
    tags_json,
    content='memory_entries',
    content_rowid='rowid'
);

CREATE TRIGGER memory_fts_insert AFTER INSERT ON memory_entries BEGIN
    INSERT INTO memory_fts(rowid, content, tags_json)
    VALUES (new.rowid, new.content, new.tags_json);
END;

CREATE TRIGGER memory_fts_delete AFTER DELETE ON memory_entries BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content, tags_json)
    VALUES ('delete', old.rowid, old.content, old.tags_json);
END;

CREATE TRIGGER memory_fts_update AFTER UPDATE ON memory_entries BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content, tags_json)
    VALUES ('delete', old.rowid, old.content, old.tags_json);
    INSERT INTO memory_fts(rowid, content, tags_json)
    VALUES (new.rowid, new.content, new.tags_json);
END;

-- ─────────────────────────────────────────────────────────────────────────────
-- Webhooks
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE webhooks (
    id              TEXT PRIMARY KEY,       -- wh_{ulid}
    principal_id    TEXT NOT NULL REFERENCES principals(id),
    url             TEXT NOT NULL,
    secret_hash     TEXT NOT NULL,          -- for HMAC-SHA256 signing
    event_types     TEXT NOT NULL,          -- JSON array of subscribed event types
    enabled         INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX idx_webhooks_principal ON webhooks(principal_id);
CREATE INDEX idx_webhooks_enabled   ON webhooks(enabled) WHERE enabled = 1;

CREATE TABLE webhook_deliveries (
    id              TEXT PRIMARY KEY,       -- wdl_{ulid}
    webhook_id      TEXT NOT NULL REFERENCES webhooks(id),
    event_id        TEXT REFERENCES events(id),

    attempt_number  INTEGER NOT NULL DEFAULT 1,
    status_code     INTEGER,
    response_body   TEXT,
    latency_ms      INTEGER,

    delivered_at    TEXT,
    next_retry_at   TEXT
);

CREATE INDEX idx_wdl_webhook     ON webhook_deliveries(webhook_id);
CREATE INDEX idx_wdl_next_retry  ON webhook_deliveries(next_retry_at)
    WHERE status_code IS NULL OR status_code >= 400;
```

---

### B.2 PostgreSQL Schema (Cloud)

The PostgreSQL schema is semantically identical to the SQLite schema with
type-system and performance adaptations.

```sql
-- Enable required extensions
CREATE EXTENSION IF NOT EXISTS "pgcrypto";   -- gen_random_uuid() fallback
CREATE EXTENSION IF NOT EXISTS "pg_trgm";    -- trigram full-text on content
CREATE EXTENSION IF NOT EXISTS "vector";     -- pgvector for embedding search

-- ─────────────────────────────────────────────────────────────────────────────
-- Type differences from SQLite
-- ─────────────────────────────────────────────────────────────────────────────
-- TEXT          -> TEXT (unchanged)
-- INTEGER       -> BIGINT
-- REAL          -> DOUBLE PRECISION
-- BLOB          -> BYTEA
-- INTEGER (0/1) -> BOOLEAN
-- TEXT (JSON)   -> JSONB
-- TEXT (ISO 8601) -> TIMESTAMPTZ

-- ─────────────────────────────────────────────────────────────────────────────
-- Schema migrations
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE schema_migrations (
    version         BIGINT PRIMARY KEY,
    description     TEXT NOT NULL,
    applied_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    checksum        TEXT NOT NULL
);

-- ─────────────────────────────────────────────────────────────────────────────
-- Multi-tenancy: tenant isolation
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE tenants (
    id              TEXT PRIMARY KEY,       -- ten_{ulid}
    name            TEXT NOT NULL UNIQUE,
    plan            TEXT NOT NULL DEFAULT 'free',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- tenant_id column is added to all tables for RLS
-- Example on agents:

CREATE TABLE agents (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL REFERENCES tenants(id),
    name            TEXT NOT NULL,
    description     TEXT,
    spec_json       JSONB NOT NULL,
    revision        BIGINT NOT NULL DEFAULT 1,
    status          TEXT NOT NULL DEFAULT 'stopped'
        CHECK (status IN ('running','stopped','archived')),
    created_by      TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived_at     TIMESTAMPTZ,

    UNIQUE(tenant_id, name, created_by)
);

CREATE INDEX idx_agents_tenant_status ON agents(tenant_id, status);
CREATE INDEX idx_agents_spec_gin      ON agents USING gin(spec_json);

-- Row-Level Security for tenant isolation
ALTER TABLE agents ENABLE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON agents
    USING (tenant_id = current_setting('app.tenant_id', true));

-- ─────────────────────────────────────────────────────────────────────────────
-- Events: partitioned by created month for scalable retention
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE events (
    id              TEXT NOT NULL,
    tenant_id       TEXT NOT NULL,
    run_id          TEXT,
    agent_id        TEXT,
    sequence        BIGINT NOT NULL,
    type            TEXT NOT NULL,
    timestamp       TIMESTAMPTZ NOT NULL,
    payload_json    JSONB NOT NULL,
    correlation_id  TEXT,
    causation_id    TEXT,
    durability      TEXT NOT NULL DEFAULT 'persistent'
        CHECK (durability IN ('persistent','ephemeral')),
    turn_id         TEXT,
    effect_id       TEXT,

    PRIMARY KEY (tenant_id, id, timestamp)
) PARTITION BY RANGE (timestamp);

-- Monthly partitions created programmatically:
CREATE TABLE events_2026_07 PARTITION OF events
    FOR VALUES FROM ('2026-07-01') TO ('2026-08-01');
CREATE TABLE events_2026_08 PARTITION OF events
    FOR VALUES FROM ('2026-08-01') TO ('2026-09-01');
-- ... additional partitions generated by migration scripts

CREATE INDEX idx_events_run_seq_pg ON events(tenant_id, run_id, sequence)
    WHERE run_id IS NOT NULL;
CREATE INDEX idx_events_type_pg    ON events(tenant_id, type, timestamp DESC);
CREATE INDEX idx_events_payload    ON events USING gin(payload_json);

-- ─────────────────────────────────────────────────────────────────────────────
-- Memory: pgvector for embedding similarity search
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE memory_entries (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    agent_id        TEXT NOT NULL,
    type            TEXT NOT NULL
        CHECK (type IN ('episodic','semantic','procedural')),
    content         TEXT NOT NULL,
    embedding       vector(1536),           -- text-embedding-3-small dimensions
    tags_json       JSONB,
    source_type     TEXT NOT NULL,
    source_ref      TEXT,
    retention_policy TEXT NOT NULL DEFAULT 'agent_default',
    expires_at      TIMESTAMPTZ,
    last_accessed   TIMESTAMPTZ,
    relevance_score DOUBLE PRECISION,
    access_count    BIGINT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE memory_entries ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON memory_entries
    USING (tenant_id = current_setting('app.tenant_id', true));

-- Approximate nearest-neighbor index (HNSW for performance at scale)
CREATE INDEX idx_memory_embedding_hnsw ON memory_entries
    USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

CREATE INDEX idx_memory_content_trgm ON memory_entries
    USING gin (content gin_trgm_ops);

-- ─────────────────────────────────────────────────────────────────────────────
-- Artifacts: JSONB for metadata queries
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE artifacts (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    run_id          TEXT,
    turn_id         TEXT,
    type            TEXT NOT NULL,
    classification  TEXT NOT NULL DEFAULT 'workspace_output',
    content_hash    TEXT NOT NULL,
    content_type    TEXT NOT NULL,
    size_bytes      BIGINT NOT NULL,
    filename        TEXT,
    storage_backend TEXT NOT NULL,
    storage_ref     TEXT NOT NULL,
    created_by_type TEXT NOT NULL,
    created_by_id   TEXT NOT NULL,
    parent_id       TEXT REFERENCES artifacts(id),
    tool_id         TEXT,
    retention_policy TEXT NOT NULL DEFAULT 'workspace_default',
    expires_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    metadata_json   JSONB
);

ALTER TABLE artifacts ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON artifacts
    USING (tenant_id = current_setting('app.tenant_id', true));

CREATE INDEX idx_artifacts_run_pg      ON artifacts(tenant_id, run_id);
CREATE INDEX idx_artifacts_metadata_pg ON artifacts USING gin(metadata_json);
```

---

### B.3 Migration Strategy

#### Schema versioning

All migrations are stored in `polkagent-db/migrations/` with the naming
convention `{version:04}_{description}.sql`:

```
migrations/
  0001_initial_schema.sql
  0002_add_tenant_id_columns.sql
  0003_add_memory_fts.sql
  0004_add_webhook_tables.sql
  0005_partition_events_by_month.sql
```

Each file begins with a comment block:

```sql
-- Migration: 0004
-- Description: Add webhook registration and delivery tracking tables
-- Depends on: 0003
-- Reversible: yes (see 0004_rollback.sql)
-- Checksum: sha256:abc...
```

#### Forward-only migrations

**Requirement DB-MIG-001.** Migrations are forward-only in production. The
`schema_migrations` table checksum prevents re-running or modifying applied
migrations. The only allowed operation after application is a forward migration
to a higher version.

`polkagent db migrate` applies all pending migrations in order:

```
polkagent db migrate
  Checking schema version... current: 3, latest: 5
  Applying migration 0004: add webhook tables... OK (42ms)
  Applying migration 0005: partition events by month... OK (1.2s)
  Schema is now at version 5.
```

#### Data migration scripts (SQLite to PostgreSQL)

For managed cloud promotion from self-hosted SQLite to PostgreSQL:

```
polkagent db export \
  --format jsonl \
  --output polkagent-export-$(date +%Y%m%d).jsonl

polkagent db import \
  --format jsonl \
  --input polkagent-export-20260730.jsonl \
  --target postgres \
  --url postgresql://...
```

The export format is JSON-lines, one record per line with a `table` field:

```jsonl
{"table":"principals","data":{"id":"usr_01HQ...","type":"user",...}}
{"table":"agents","data":{"id":"agt_01HQ...","name":"my-agent",...}}
{"table":"runs","data":{"id":"run_01HQ...","agent_id":"agt_01HQ...",...}}
```

Import validates referential integrity and applies records in dependency order.
Large tables (`events`, `memory_entries`) are streamed in batches of 1000 rows.

#### Rollback procedures

**Schema rollback** (pre-production only):

```
polkagent db rollback --to-version 3
```

Each migration with a rollback script in `migrations/{n}_rollback.sql` is
reversible. Rollback is blocked in production (requires `--force-production`
flag and writes a safety audit log entry).

**Data rollback**: use the pre-migration backup:

```
polkagent db restore --from polkagent-pre-migration-backup-20260730.tar.gz
```

---

## APPENDIX C: CONFIGURATION SCHEMA

### C.1 Complete TOML Configuration

The following is the exhaustive TOML configuration reference. Every field is
annotated with its type, default value, and the environment variable that
overrides it. Required fields are marked with `# REQUIRED`.

```toml
# Polkagent configuration
# Schema version: v1alpha1
# Environment variable prefix: POLKAGENT_
# Nested keys use double-underscore: POLKAGENT_SERVER__PORT=4840

[meta]
# apiVersion identifies the config schema family.
api_version = "polkagent.dev/v1alpha1"          # POLKAGENT_META__API_VERSION
# schema_version is the integer migration version.
schema_version = 1                              # POLKAGENT_META__SCHEMA_VERSION

# ─── Server ─────────────────────────────────────────────────────────────────

[server]
# TCP bind address.
host = "127.0.0.1"                              # POLKAGENT_SERVER__HOST
# TCP port.
port = 4840                                     # POLKAGENT_SERVER__PORT
# Maximum concurrent HTTP connections (0 = unlimited).
max_connections = 0                             # POLKAGENT_SERVER__MAX_CONNECTIONS
# HTTP request timeout (seconds). Streaming endpoints are exempt.
request_timeout_seconds = 300                   # POLKAGENT_SERVER__REQUEST_TIMEOUT_SECONDS
# Maximum request body size (bytes). Applies before parsing.
max_request_body_bytes = 10_485_760             # POLKAGENT_SERVER__MAX_REQUEST_BODY_BYTES
# Enable DANGER: public CORS (disables origin restriction). Dev only.
unsafe_public_cors = false                      # POLKAGENT_SERVER__UNSAFE_PUBLIC_CORS

[server.tls]
# Enable TLS. Requires cert_file and key_file.
enabled = false                                 # POLKAGENT_SERVER__TLS__ENABLED
# Path to PEM certificate file.
cert_file = ""                                  # POLKAGENT_SERVER__TLS__CERT_FILE
# Path to PEM private key file.
key_file = ""                                   # POLKAGENT_SERVER__TLS__KEY_FILE
# Minimum TLS version: "1.2" or "1.3".
min_version = "1.2"                             # POLKAGENT_SERVER__TLS__MIN_VERSION

[server.cors]
# Origins allowed to make cross-origin requests.
allowed_origins = ["http://localhost:*"]        # POLKAGENT_SERVER__CORS__ALLOWED_ORIGINS
# HTTP methods to allow.
allowed_methods = ["GET","POST","PUT","DELETE","OPTIONS"]
# Headers the client may include.
allowed_headers = ["Authorization","Content-Type","X-Request-Id","X-Idempotency-Key"]
# Whether to allow credentials in cross-origin requests.
allow_credentials = false                       # POLKAGENT_SERVER__CORS__ALLOW_CREDENTIALS

[server.rate_limit]
# Enable the global rate limiter.
enabled = true                                  # POLKAGENT_SERVER__RATE_LIMIT__ENABLED
# Requests allowed per minute (global, all principals combined).
requests_per_minute = 600                       # POLKAGENT_SERVER__RATE_LIMIT__REQUESTS_PER_MINUTE
# Burst allowance above the per-minute average.
burst_size = 100                                # POLKAGENT_SERVER__RATE_LIMIT__BURST_SIZE

# ─── Authentication ──────────────────────────────────────────────────────────

[auth]
# Authentication method: "api_key" | "oauth" | "none" (local dev only).
method = "api_key"                              # POLKAGENT_AUTH__METHOD

[auth.api_key]
# Hash algorithm for stored API keys.
hash_algorithm = "argon2id"                     # POLKAGENT_AUTH__API_KEY__HASH_ALGORITHM
# Time cost factor for argon2id.
time_cost = 2                                   # POLKAGENT_AUTH__API_KEY__TIME_COST
# Memory cost factor (KB) for argon2id.
memory_cost_kb = 65536                          # POLKAGENT_AUTH__API_KEY__MEMORY_COST_KB

[auth.oauth]
# OIDC issuer URL.
issuer_url = ""                                 # POLKAGENT_AUTH__OAUTH__ISSUER_URL
# OAuth client ID.
client_id = ""                                  # POLKAGENT_AUTH__OAUTH__CLIENT_ID
# OAuth audience claim.
audience = ""                                   # POLKAGENT_AUTH__OAUTH__AUDIENCE
# Allowed scopes (controls what tokens can request).
scopes = ["agents:read","agents:write","runs:read","runs:write",
          "effects:write","artifacts:read","artifacts:write",
          "memory:read","memory:write","payments:read","payments:write"]

# ─── Database ────────────────────────────────────────────────────────────────

[database]
# Storage backend: "sqlite" | "postgres".
backend = "sqlite"                              # POLKAGENT_DATABASE__BACKEND

[database.sqlite]
# Path to the SQLite database file. ~ is expanded.
path = "~/.local/share/polkagent/polkagent.db" # POLKAGENT_DATABASE__SQLITE__PATH
# WAL mode for concurrent reads.
journal_mode = "wal"                            # POLKAGENT_DATABASE__SQLITE__JOURNAL_MODE
# Sync mode: "full" for payment/chain data, "normal" for general use.
synchronous = "normal"                          # POLKAGENT_DATABASE__SQLITE__SYNCHRONOUS
# Timeout before returning BUSY (ms).
busy_timeout_ms = 5000                          # POLKAGENT_DATABASE__SQLITE__BUSY_TIMEOUT_MS
# Write connection pool size (SQLite supports only 1 writer at a time).
max_connections = 1                             # POLKAGENT_DATABASE__SQLITE__MAX_CONNECTIONS
# Read connection pool size.
read_pool_size = 4                              # POLKAGENT_DATABASE__SQLITE__READ_POOL_SIZE
# Enable SQLite WAL checkpoint on startup.
checkpoint_on_startup = true                    # POLKAGENT_DATABASE__SQLITE__CHECKPOINT_ON_STARTUP

[database.postgres]
# PostgreSQL connection URL. Resolved from secret resolver at runtime.
url = ""                                        # POLKAGENT_DATABASE__POSTGRES__URL
# Maximum pool size.
max_connections = 20                            # POLKAGENT_DATABASE__POSTGRES__MAX_CONNECTIONS
# Minimum idle connections.
min_connections = 2                             # POLKAGENT_DATABASE__POSTGRES__MIN_CONNECTIONS
# Timeout waiting for a connection from the pool (seconds).
acquire_timeout_seconds = 30                    # POLKAGENT_DATABASE__POSTGRES__ACQUIRE_TIMEOUT_SECONDS
# Idle connection timeout (seconds). 0 = no timeout.
idle_timeout_seconds = 600                      # POLKAGENT_DATABASE__POSTGRES__IDLE_TIMEOUT_SECONDS
# SSL mode: "disable" | "prefer" | "require" | "verify-ca" | "verify-full".
ssl_mode = "require"                            # POLKAGENT_DATABASE__POSTGRES__SSL_MODE

# ─── Execution ───────────────────────────────────────────────────────────────

[execution]
# Maximum concurrent runs across all agents.
max_concurrent_runs = 10                        # POLKAGENT_EXECUTION__MAX_CONCURRENT_RUNS
# Default run timeout (seconds). Agents may override per-run.
default_timeout_seconds = 600                   # POLKAGENT_EXECUTION__DEFAULT_TIMEOUT_SECONDS
# Default maximum turns per run. Agents may override.
default_max_turns = 50                          # POLKAGENT_EXECUTION__DEFAULT_MAX_TURNS
# Maximum parallel tool invocations per turn.
max_parallel_tools = 4                          # POLKAGENT_EXECUTION__MAX_PARALLEL_TOOLS
# Retry delay base for transient tool errors (ms).
tool_retry_base_ms = 1000                       # POLKAGENT_EXECUTION__TOOL_RETRY_BASE_MS
# Maximum tool retries per invocation.
tool_max_retries = 3                            # POLKAGENT_EXECUTION__TOOL_MAX_RETRIES

[execution.budget]
# Per-run model cost ceiling (USD). 0.0 = unlimited.
model_usd_per_run = 5.00                        # POLKAGENT_EXECUTION__BUDGET__MODEL_USD_PER_RUN
# Per-day model cost ceiling across all runs (USD). 0.0 = unlimited.
model_usd_per_day = 50.00                       # POLKAGENT_EXECUTION__BUDGET__MODEL_USD_PER_DAY
# Percentage of budget consumed before emitting a warning event.
warn_threshold_percent = 80                     # POLKAGENT_EXECUTION__BUDGET__WARN_THRESHOLD_PERCENT

# ─── Providers ───────────────────────────────────────────────────────────────
# Multiple [[providers]] entries are allowed.
# The first provider is the default unless a run specifies an override.

[[providers]]
# Unique identifier referenced by agents and runs.
id = "anthropic-default"                        # REQUIRED
# Provider type: "anthropic" | "openai_compatible" | "google" | "local".
type = "anthropic"                              # REQUIRED
# API key resolved from the secret resolver at runtime (not stored here).
# Set via: POLKAGENT_PROVIDERS__0__API_KEY or in secrets config.
api_key_secret_ref = "anthropic_api_key"
# Override the provider's base URL (for proxy or self-hosted deployments).
base_url = "https://api.anthropic.com"          # POLKAGENT_PROVIDERS__0__BASE_URL
# Default model ID to use when an agent doesn't specify one.
default_model = "claude-sonnet-4-6"             # POLKAGENT_PROVIDERS__0__DEFAULT_MODEL
# Per-request timeout (seconds). Includes streaming wait.
timeout_seconds = 120                           # POLKAGENT_PROVIDERS__0__TIMEOUT_SECONDS
# Maximum retries on transient errors (5xx, timeout).
max_retries = 3                                 # POLKAGENT_PROVIDERS__0__MAX_RETRIES
# Base delay for exponential retry backoff (ms).
retry_backoff_base_ms = 1000                    # POLKAGENT_PROVIDERS__0__RETRY_BACKOFF_BASE_MS
# Maximum backoff delay (ms).
retry_backoff_max_ms = 30000                    # POLKAGENT_PROVIDERS__0__RETRY_BACKOFF_MAX_MS

[[providers]]
id = "openai-compat"
type = "openai_compatible"
base_url = "https://api.openai.com/v1"
default_model = "gpt-4o"
timeout_seconds = 120
max_retries = 3

# ─── Secrets ─────────────────────────────────────────────────────────────────

[secrets]
# Secret storage backend: "file" | "env" | "keychain" | "vault" | "kms".
backend = "file"                                # POLKAGENT_SECRETS__BACKEND

[secrets.file]
# Path to the encrypted secrets file.
path = "~/.config/polkagent/secrets.enc"        # POLKAGENT_SECRETS__FILE__PATH
# Encryption: "age" | "gpg" | "none" (dev only).
encryption = "age"                              # POLKAGENT_SECRETS__FILE__ENCRYPTION
# Age identity file path (for decryption).
age_identity_file = "~/.config/polkagent/identity.age"

[secrets.env]
# Prefix for environment variable secret references.
# e.g., a secret "anthropic_api_key" resolves to env var "SECRET_ANTHROPIC_API_KEY".
env_prefix = "SECRET_"                          # POLKAGENT_SECRETS__ENV__ENV_PREFIX

[secrets.vault]
# HashiCorp Vault address.
address = ""                                    # POLKAGENT_SECRETS__VAULT__ADDRESS
# Vault token (prefer approle or k8s auth in production).
token_secret_ref = "vault_token"
# KV engine mount path.
mount = "secret"                                # POLKAGENT_SECRETS__VAULT__MOUNT

[secrets.kms]
# AWS KMS key ID for envelope encryption.
key_id = ""                                     # POLKAGENT_SECRETS__KMS__KEY_ID
# AWS region.
region = "us-east-1"                            # POLKAGENT_SECRETS__KMS__REGION

# ─── Storage ─────────────────────────────────────────────────────────────────

[storage]
# Artifact storage backend: "local" | "s3" | "gcs".
artifact_backend = "local"                      # POLKAGENT_STORAGE__ARTIFACT_BACKEND

[storage.local]
# Root directory for artifact files.
path = "~/.local/share/polkagent/artifacts"     # POLKAGENT_STORAGE__LOCAL__PATH
# Maximum size for a single artifact (bytes).
max_artifact_size_bytes = 104_857_600           # POLKAGENT_STORAGE__LOCAL__MAX_ARTIFACT_SIZE_BYTES
# Maximum total storage (bytes). 0 = unlimited.
max_total_size_bytes = 10_737_418_240           # POLKAGENT_STORAGE__LOCAL__MAX_TOTAL_SIZE_BYTES

[storage.s3]
# S3-compatible bucket name.
bucket = ""                                     # POLKAGENT_STORAGE__S3__BUCKET
# Object key prefix for all artifacts.
prefix = "polkagent/"                           # POLKAGENT_STORAGE__S3__PREFIX
# AWS region.
region = "us-east-1"                            # POLKAGENT_STORAGE__S3__REGION
# Override endpoint URL (for MinIO or other S3-compatible stores).
endpoint_url = ""                               # POLKAGENT_STORAGE__S3__ENDPOINT_URL
# Force path-style URLs (required for MinIO).
force_path_style = false                        # POLKAGENT_STORAGE__S3__FORCE_PATH_STYLE

[storage.gcs]
# GCS bucket name.
bucket = ""                                     # POLKAGENT_STORAGE__GCS__BUCKET
prefix = "polkagent/"                           # POLKAGENT_STORAGE__GCS__PREFIX

# ─── Chain profiles ──────────────────────────────────────────────────────────
# Multiple [[chain_profiles]] entries are allowed.

[[chain_profiles]]
# Unique identifier referenced by agents and effects.
id = "polkadot-production"                      # REQUIRED
# Human-readable display name.
name = "Polkadot"
# Genesis hash of the chain. Used to validate transaction binding.
genesis_hash = "0x91b171bb158e2d3848fa23a9f1c25182fb8e20313b2c1eb49219da7a70ce90c3"  # REQUIRED
# WebSocket RPC endpoints. Tried in order; first healthy endpoint is used.
rpc_endpoints = [                               # REQUIRED
  "wss://rpc.polkadot.io",
  "wss://polkadot-rpc.dwellir.com",
  "wss://polkadot.api.onfinality.io/public-ws",
]
# Metadata cache TTL (seconds). 0 = no cache.
metadata_cache_ttl_seconds = 3600              # POLKAGENT_CHAIN__POLKADOT_PRODUCTION__METADATA_CACHE_TTL_SECONDS
# Automatically refresh metadata when spec_version changes.
auto_update_metadata = true
# RPC connection timeout (seconds).
rpc_timeout_seconds = 30
# Maximum RPC retries per request.
rpc_max_retries = 3

[[chain_profiles]]
id = "polkadot-paseo"
name = "Paseo Testnet"
genesis_hash = "0x77afd6190f1554ad45fd0d31aee62aacc33c6db0ea801129acb813f8e05f1016"
rpc_endpoints = [
  "wss://paseo-rpc.dwellir.com",
  "wss://rpc.ibp.network/paseo",
]

# ─── Signers ─────────────────────────────────────────────────────────────────
# Multiple [[signers]] entries are allowed.

[[signers]]
# Unique identifier referenced by agent specs.
id = "user-wallet"                              # REQUIRED
# Signer type: "external_wallet" | "polkadot_vault" | "hardware" | "custodial".
type = "external_wallet"                        # REQUIRED
description = "User's browser extension wallet"
# Timeout waiting for user signature (seconds).
approval_timeout_seconds = 300

[[signers]]
id = "vault-airgap"
type = "polkadot_vault"
description = "Air-gapped Polkadot Vault (QR-based)"
approval_timeout_seconds = 600

# ─── Transport surfaces ───────────────────────────────────────────────────────

[transport.web]
# Enable the web UI transport (serves the React dashboard).
enabled = true                                  # POLKAGENT_TRANSPORT__WEB__ENABLED
# Static assets directory (empty = use embedded assets).
static_dir = ""

[transport.cli]
# Enable the CLI transport (used by `polkagent run`).
enabled = true                                  # POLKAGENT_TRANSPORT__CLI__ENABLED

[transport.polkadot_chat]
# Enable the Polkadot Chat (PCA-compatible) transport.
enabled = false                                 # POLKAGENT_TRANSPORT__POLKADOT_CHAT__ENABLED
# Matrix homeserver URL for Polkadot Chat.
homeserver_url = ""
# Bot identity (references a signer for account operations).
identity_signer_ref = ""

# ─── Memory ──────────────────────────────────────────────────────────────────

[memory]
# Storage backend for memory entries.
backend = "sqlite"                              # POLKAGENT_MEMORY__BACKEND
# Maximum entries per agent. Oldest entries are evicted when exceeded.
max_entries_per_agent = 10000                   # POLKAGENT_MEMORY__MAX_ENTRIES_PER_AGENT
# Default retention (days). 0 = forever.
default_retention_days = 90                     # POLKAGENT_MEMORY__DEFAULT_RETENTION_DAYS
# Embedding model ID for semantic search.
embedding_model = "text-embedding-3-small"      # POLKAGENT_MEMORY__EMBEDDING_MODEL
# Embedding vector dimensions (must match model).
embedding_dimensions = 1536                     # POLKAGENT_MEMORY__EMBEDDING_DIMENSIONS
# Minimum similarity score for memory retrieval (0.0-1.0).
min_relevance_score = 0.7                       # POLKAGENT_MEMORY__MIN_RELEVANCE_SCORE

# ─── Workspace ───────────────────────────────────────────────────────────────

[workspace]
# Default root directory for agent workspaces.
default_root = "~/.local/share/polkagent/workspaces"
# Isolation mode: "process" | "container" | "none".
isolation = "process"                           # POLKAGENT_WORKSPACE__ISOLATION
# Maximum size of a single file in the workspace (bytes).
max_file_size_bytes = 10_485_760
# Maximum total workspace size per run (bytes).
max_total_size_bytes = 1_073_741_824
# Initialize git repository in each workspace.
git_init = true                                 # POLKAGENT_WORKSPACE__GIT_INIT
# Git user email for automated commits.
git_user_email = "agent@polkagent.local"
git_user_name = "Polkagent"

# ─── Feature flags ───────────────────────────────────────────────────────────

[features]
# Stable: fully covered by API compatibility policy.
chain_actions = "stable"
# Beta: functionally complete, schema may change.
payments = "beta"
marketplace = "beta"
# Alpha: experimental, API will change.
pvm_contracts = "alpha"
multi_agent_groups = "alpha"
# Disabled: not available.
jam_services = "disabled"

# ─── Observability ───────────────────────────────────────────────────────────

[telemetry]
# Enable telemetry export.
enabled = true                                  # POLKAGENT_TELEMETRY__ENABLED
# Log level: "error" | "warn" | "info" | "debug" | "trace".
log_level = "info"                              # POLKAGENT_TELEMETRY__LOG_LEVEL
# Structured JSON logging (false = human-readable).
structured_logging = true                       # POLKAGENT_TELEMETRY__STRUCTURED_LOGGING
# OpenTelemetry export format: "otlp" | "json" | "none".
export_format = "none"                          # POLKAGENT_TELEMETRY__EXPORT_FORMAT
# OTLP collector endpoint (gRPC or HTTP).
endpoint = ""                                   # POLKAGENT_TELEMETRY__ENDPOINT
# Service name in telemetry data.
service_name = "polkagent"                      # POLKAGENT_TELEMETRY__SERVICE_NAME

[telemetry.metrics]
enabled = true                                  # POLKAGENT_TELEMETRY__METRICS__ENABLED
export_interval_seconds = 60

[telemetry.tracing]
enabled = true                                  # POLKAGENT_TELEMETRY__TRACING__ENABLED
# Fraction of requests to trace (1.0 = all, 0.1 = 10%).
sample_rate = 1.0                               # POLKAGENT_TELEMETRY__TRACING__SAMPLE_RATE
# Include full request/response bodies in spans (dev only).
include_bodies = false

# ─── Webhooks ────────────────────────────────────────────────────────────────

[webhooks]
# Enable outbound webhook delivery.
enabled = true                                  # POLKAGENT_WEBHOOKS__ENABLED
# Maximum delivery attempts before marking a webhook delivery failed.
max_attempts = 8                                # POLKAGENT_WEBHOOKS__MAX_ATTEMPTS
# Initial retry delay (seconds).
retry_initial_delay_seconds = 1
# Maximum retry delay (seconds).
retry_max_delay_seconds = 3600
# Webhook delivery timeout (seconds).
delivery_timeout_seconds = 10
# Retain delivery log entries for this many days.
delivery_log_retention_days = 30
```

---

### C.2 Configuration Hierarchy

Configuration resolves in the following order. Later layers override earlier
layers. Merging is field-by-field; a missing field in a later layer does not
clear the value from an earlier layer.

```
Layer 1: Built-in defaults
  Source: compiled into the polkagent binary
  Scope:  global
  Note:   always safe to start with no config file

Layer 2: System config
  Source: /etc/polkagent/config.toml
  Scope:  machine-wide
  Note:   typically managed by the operator or configuration management

Layer 3: User global config
  Source: $XDG_CONFIG_HOME/polkagent/config.toml
          (~/.config/polkagent/config.toml on Linux/macOS)
  Scope:  per-user
  Note:   personal preferences: default models, editor, auth

Layer 4: Project config
  Source: {cwd}/.polkagent/config.toml
          (searched upward to filesystem root, like .git)
  Scope:  repository/project
  Note:   committed to version control; chain profiles, tool grants,
          provider routing for the project

Layer 5: Agent spec inline overrides
  Source: AgentSpec.spec.execution, AgentSpec.spec.tools, etc.
  Scope:  per-agent
  Note:   declared in the agent's versioned spec

Layer 6: Run API parameters
  Source: POST /runs body: options.model_override, options.max_turns, etc.
  Scope:  per-run
  Note:   validated against the agent's effective grant

Layer 7: Environment variables
  Source: POLKAGENT_* variables in the server process environment
  Scope:  deployment/container
  Note:   highest precedence; used for secrets and infra-level overrides

Layer 8: CLI flags (runtime only, not persisted)
  Source: --log-level, --port, --config, etc.
  Scope:  single invocation
  Note:   override for the lifetime of the process; never written to disk
```

#### Precedence rules and conflict resolution

| Rule | Behavior |
|---|---|
| Type mismatch | Error at startup with field name and expected type |
| Missing required field | Error at startup naming the field and its layer |
| Secret in config file | Warning; recommend moving to secret resolver |
| Unknown field | Ignored with a debug-level log (forward compatibility) |
| Empty string for URL | Treated as "not configured"; no connection attempted |
| Zero for numeric limit | Interpreted as "unlimited" unless noted otherwise |
| `null` in env var | Treated as "unset"; does not override lower layers |

#### Environment variable examples

```bash
# Override port
POLKAGENT_SERVER__PORT=8080

# Set PostgreSQL URL (sensitive: use secret resolver in production)
POLKAGENT_DATABASE__POSTGRES__URL=postgresql://user:pass@host/db

# Override log level
POLKAGENT_TELEMETRY__LOG_LEVEL=debug

# Set provider API key (prefer secrets.env backend instead)
POLKAGENT_PROVIDERS__0__API_KEY=sk-ant-api03-...

# Enable a beta feature
POLKAGENT_FEATURES__PAYMENTS=beta

# Override execution budget
POLKAGENT_EXECUTION__BUDGET__MODEL_USD_PER_RUN=2.50
```

#### `polkagent config` command surface

```bash
# Validate resolved configuration against the schema
polkagent config validate

# Display the fully resolved configuration (secrets redacted)
polkagent config show

# Display the raw config file without resolution
polkagent config show --raw

# Display only a specific section
polkagent config show --section database

# Convert a YAML config from PCA to Polkagent TOML
polkagent config convert --from yaml --input pca-config.yml --output config.toml

# Migrate config to a new schema version
polkagent config migrate --from 1 --to 2

# Set a value in the project config
polkagent config set server.port 8080

# Unset a value (falls back to lower layer)
polkagent config unset server.port
```

---

## APPENDIX D: SDK DESIGN

### D.1 Rust SDK

The Rust SDK is organized as a workspace crate (`polkagent-client`) with an
ergonomic builder pattern and typed streaming responses.

```rust
// polkagent-client/src/lib.rs

use std::time::Duration;

/// Root client. Create one per process; it owns connection pools.
pub struct PolkagentClient {
    http:    reqwest::Client,
    base_url: url::Url,
    api_key:  Option<String>,
}

impl PolkagentClient {
    /// Begin building a client.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Access the agents resource group.
    pub fn agents(&self) -> AgentsClient<'_> { AgentsClient { client: self } }

    /// Access the runs resource group.
    pub fn runs(&self) -> RunsClient<'_> { RunsClient { client: self } }

    /// Access the effects resource group.
    pub fn effects(&self) -> EffectsClient<'_> { EffectsClient { client: self } }

    /// Access the artifacts resource group.
    pub fn artifacts(&self) -> ArtifactsClient<'_> { ArtifactsClient { client: self } }

    /// Access the events resource group.
    pub fn events(&self) -> EventsClient<'_> { EventsClient { client: self } }

    /// Access the memory resource group.
    pub fn memory(&self) -> MemoryClient<'_> { MemoryClient { client: self } }

    /// Access the providers resource group.
    pub fn providers(&self) -> ProvidersClient<'_> { ProvidersClient { client: self } }

    /// Open a WebSocket connection for real-time event subscriptions.
    pub async fn connect_ws(&self) -> Result<WsConnection, PolkagentError> { ... }
}

pub struct ClientBuilder {
    base_url:         Option<String>,
    api_key:          Option<String>,
    timeout:          Duration,
    connect_timeout:  Duration,
    max_retries:      u32,
    retry_backoff:    Duration,
}

impl ClientBuilder {
    pub fn base_url(mut self, url: impl Into<String>) -> Self { ... }
    pub fn api_key(mut self, key: impl Into<String>) -> Self { ... }
    pub fn timeout(mut self, t: Duration) -> Self { ... }
    pub fn max_retries(mut self, n: u32) -> Self { ... }
    pub fn build(self) -> Result<PolkagentClient, PolkagentError> { ... }
}

// ─── Agents ─────────────────────────────────────────────────────────────────

pub struct AgentsClient<'a> { client: &'a PolkagentClient }

impl<'a> AgentsClient<'a> {
    /// Create a new agent from an AgentSpec.
    pub async fn create(
        &self,
        req: CreateAgentRequest,
    ) -> Result<AgentResponse, PolkagentError> { ... }

    /// List agents with optional filters.
    pub fn list(&self) -> AgentListBuilder<'_> { ... }

    /// Get a single agent by ID.
    pub async fn get(
        &self,
        agent_id: &str,
    ) -> Result<AgentResponse, PolkagentError> { ... }

    /// Update an agent's spec (creates new revision).
    pub async fn update(
        &self,
        agent_id: &str,
        req: UpdateAgentRequest,
    ) -> Result<AgentResponse, PolkagentError> { ... }

    /// Archive (soft-delete) an agent.
    pub async fn delete(
        &self,
        agent_id: &str,
    ) -> Result<(), PolkagentError> { ... }

    /// Start an agent.
    pub async fn start(
        &self,
        agent_id: &str,
    ) -> Result<AgentStatusResponse, PolkagentError> { ... }

    /// Stop an agent.
    pub async fn stop(
        &self,
        agent_id: &str,
        force: bool,
    ) -> Result<AgentStatusResponse, PolkagentError> { ... }
}

// ─── Runs ────────────────────────────────────────────────────────────────────

pub struct RunsClient<'a> { client: &'a PolkagentClient }

impl<'a> RunsClient<'a> {
    /// Create and start a run.
    pub async fn create(
        &self,
        req: CreateRunRequest,
    ) -> Result<RunResponse, PolkagentError> { ... }

    /// Get run details.
    pub async fn get(
        &self,
        run_id: &str,
    ) -> Result<RunResponse, PolkagentError> { ... }

    /// Cancel an active run.
    pub async fn cancel(
        &self,
        run_id: &str,
    ) -> Result<RunResponse, PolkagentError> { ... }

    /// Stream events for a run using Server-Sent Events.
    /// Returns an async iterator of RunEvent.
    pub async fn stream_events(
        &self,
        run_id: &str,
    ) -> Result<impl futures::Stream<Item = Result<RunEvent, PolkagentError>>, PolkagentError>
    { ... }

    /// Poll until a run reaches a terminal state. Respects the timeout.
    pub async fn wait_for_completion(
        &self,
        run_id: &str,
        timeout: Duration,
    ) -> Result<RunResponse, PolkagentError> { ... }

    /// List turns for a run.
    pub fn list_turns(&self, run_id: &str) -> TurnListBuilder<'_> { ... }

    /// Get usage statistics for a run.
    pub async fn usage(
        &self,
        run_id: &str,
    ) -> Result<RunUsage, PolkagentError> { ... }
}

// ─── Effects ─────────────────────────────────────────────────────────────────

pub struct EffectsClient<'a> { client: &'a PolkagentClient }

impl<'a> EffectsClient<'a> {
    pub async fn get(
        &self,
        effect_id: &str,
    ) -> Result<EffectResponse, PolkagentError> { ... }

    pub async fn approve(
        &self,
        effect_id: &str,
        req: ApproveEffectRequest,
    ) -> Result<EffectResponse, PolkagentError> { ... }

    pub async fn deny(
        &self,
        effect_id: &str,
        reason: &str,
    ) -> Result<EffectResponse, PolkagentError> { ... }

    /// List effects pending approval for the principal.
    pub fn list_pending_approval(&self) -> EffectListBuilder<'_> { ... }
}

// ─── Error types ─────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum PolkagentError {
    #[error("API error {code}: {message}")]
    Api {
        code:       String,
        message:    String,
        details:    Option<serde_json::Value>,
        request_id: Option<String>,
        status:     u16,
    },
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Deserialization error: {0}")]
    Deserialize(#[from] serde_json::Error),
    #[error("Timeout after {0:?}")]
    Timeout(Duration),
    #[error("Stream ended unexpectedly")]
    StreamEnded,
}

impl PolkagentError {
    /// Returns true if this error is retryable (e.g., rate limit, provider timeout).
    pub fn is_retryable(&self) -> bool { ... }

    /// Returns the API error code if this is an API error.
    pub fn api_code(&self) -> Option<&str> { ... }
}

// ─── WebSocket connection ─────────────────────────────────────────────────────

pub struct WsConnection {
    // Internal tokio-tungstenite connection
}

impl WsConnection {
    /// Subscribe to one or more channels.
    pub async fn subscribe(
        &mut self,
        channels: Vec<String>,
        cursor: Option<u64>,
    ) -> Result<(), PolkagentError> { ... }

    /// Unsubscribe from channels.
    pub async fn unsubscribe(
        &mut self,
        channels: Vec<String>,
    ) -> Result<(), PolkagentError> { ... }

    /// Receive the next event. Returns None when connection is closed.
    pub async fn next_event(
        &mut self,
    ) -> Option<Result<WsEvent, PolkagentError>> { ... }

    /// Close the connection gracefully.
    pub async fn close(self) -> Result<(), PolkagentError> { ... }
}

// ─── Usage example ───────────────────────────────────────────────────────────

// #[tokio::main]
// async fn main() -> anyhow::Result<()> {
//     let client = PolkagentClient::builder()
//         .base_url("http://localhost:4840")
//         .api_key("pak_...")
//         .timeout(Duration::from_secs(60))
//         .build()?;
//
//     // Create a run
//     let run = client.runs().create(CreateRunRequest {
//         agent_id: "agt_01HQ...".into(),
//         input: RunInput::UserMessage {
//             content: "Explain the staking pallet's reward logic".into(),
//             attachments: vec![],
//         },
//         ..Default::default()
//     }).await?;
//
//     println!("Run started: {}", run.id);
//
//     // Stream events via SSE
//     let mut stream = client.runs().stream_events(&run.id).await?;
//     while let Some(event) = stream.next().await {
//         match event? {
//             RunEvent::TokenDelta { delta, .. } => print!("{delta}"),
//             RunEvent::EffectPendingApproval { effect_id, summary, .. } => {
//                 eprintln!("\nApproval required: {summary}");
//                 client.effects().approve(&effect_id, ApproveEffectRequest {
//                     approval_type: ApprovalType::Human,
//                     comment: Some("OK".into()),
//                     ..Default::default()
//                 }).await?;
//             }
//             RunEvent::RunCompleted { .. } => break,
//             RunEvent::RunFailed { terminal_reason, .. } => {
//                 anyhow::bail!("Run failed: {terminal_reason:?}")
//             }
//             _ => {}
//         }
//     }
//
//     Ok(())
// }
```

---

### D.2 TypeScript SDK

The TypeScript SDK provides a full-typed client with native `AsyncIterable`
streaming and Zod validation.

```typescript
// @polkagent/client/src/index.ts

import { z } from 'zod';

// ─── Client ──────────────────────────────────────────────────────────────────

export interface PolkagentClientOptions {
  /** Base URL of the Polkagent server, e.g. "http://localhost:4840" */
  baseUrl: string;
  /** API key for authentication */
  apiKey?: string;
  /** OAuth bearer token */
  bearerToken?: string;
  /** Request timeout in milliseconds (default: 60000) */
  timeoutMs?: number;
  /** Maximum retries on transient errors (default: 3) */
  maxRetries?: number;
}

export class PolkagentClient {
  readonly agents: AgentsResource;
  readonly runs: RunsResource;
  readonly effects: EffectsResource;
  readonly artifacts: ArtifactsResource;
  readonly events: EventsResource;
  readonly memory: MemoryResource;
  readonly providers: ProvidersResource;
  readonly ws: WebSocketManager;

  constructor(options: PolkagentClientOptions);
}

// ─── Agents ──────────────────────────────────────────────────────────────────

export class AgentsResource {
  /** Create a new agent */
  async create(req: CreateAgentRequest): Promise<AgentResponse>;

  /** List agents with optional filters */
  list(params?: AgentListParams): AsyncIterable<AgentResponse>;

  /** Get a single agent by ID */
  async get(agentId: string): Promise<AgentResponse>;

  /** Update agent spec (creates new revision) */
  async update(agentId: string, req: UpdateAgentRequest): Promise<AgentResponse>;

  /** Archive (soft-delete) an agent */
  async delete(agentId: string): Promise<void>;

  /** Start an agent */
  async start(agentId: string): Promise<AgentStatusResponse>;

  /** Stop an agent */
  async stop(agentId: string, options?: { force?: boolean }): Promise<AgentStatusResponse>;

  /** Get runtime status */
  async status(agentId: string): Promise<AgentStatusResponse>;
}

// ─── Runs ─────────────────────────────────────────────────────────────────────

export class RunsResource {
  /** Create and start a new run */
  async create(req: CreateRunRequest): Promise<RunResponse>;

  /** Get run details */
  async get(runId: string): Promise<RunResponse>;

  /** Cancel an active run */
  async cancel(runId: string): Promise<RunResponse>;

  /** Resume a paused or waiting run */
  async resume(runId: string, input: RunInput): Promise<RunResponse>;

  /**
   * Stream run events as an AsyncIterable of typed RunEvent objects.
   * Uses Server-Sent Events internally; reconnects automatically.
   * Pass the last received sequence number as `cursor` to resume.
   */
  streamEvents(runId: string, options?: { cursor?: number }): AsyncIterable<RunEvent>;

  /**
   * Wait for a run to reach a terminal state.
   * Polls via SSE; resolves when run is completed, failed, or cancelled.
   */
  async waitForCompletion(
    runId: string,
    options?: { timeoutMs?: number; onEvent?: (event: RunEvent) => void }
  ): Promise<RunResponse>;

  /** List turns for a run */
  listTurns(runId: string, params?: TurnListParams): AsyncIterable<TurnResponse>;

  /** Get usage statistics for a run */
  async usage(runId: string): Promise<RunUsage>;
}

// ─── Effects ─────────────────────────────────────────────────────────────────

export class EffectsResource {
  async get(effectId: string): Promise<EffectResponse>;

  async approve(effectId: string, req: ApproveEffectRequest): Promise<EffectResponse>;

  async deny(effectId: string, reason: string): Promise<EffectResponse>;

  listPendingApproval(params?: EffectListParams): AsyncIterable<EffectResponse>;
}

// ─── Streaming event types ────────────────────────────────────────────────────

export type RunEvent =
  | { type: 'run.started';           runId: string; agentId: string }
  | { type: 'run.completed';         runId: string; terminalReason: string | null }
  | { type: 'run.failed';            runId: string; terminalReason: string }
  | { type: 'run.cancelled';         runId: string }
  | { type: 'turn.started';          turnId: string; sequence: number }
  | { type: 'turn.completed';        turnId: string; inputTokens: number; outputTokens: number }
  | { type: 'turn.token_delta';      turnId: string; delta: string; tokenIndex: number; role: string }
  | { type: 'turn.thinking_delta';   turnId: string; delta: string }
  | { type: 'turn.tool_use_start';   turnId: string; toolId: string; toolName: string }
  | { type: 'turn.tool_use_end';     turnId: string; toolId: string; success: boolean }
  | { type: 'effect.created';        effectId: string; effectType: string }
  | { type: 'effect.pending_approval'; effectId: string; summary: string; riskLevel: string }
  | { type: 'effect.approved';       effectId: string }
  | { type: 'effect.denied';         effectId: string; reason: string }
  | { type: 'effect.succeeded';      effectId: string; txHash?: string }
  | { type: 'effect.failed';         effectId: string; error: string }
  | { type: 'artifact.created';      artifactId: string; artifactType: string; filename?: string }
  | { type: 'budget.warn';           runId: string; percentUsed: number; costUsd: number };

// ─── Error types ──────────────────────────────────────────────────────────────

export class PolkagentError extends Error {
  readonly code: string;
  readonly details: Record<string, unknown> | null;
  readonly requestId: string | null;
  readonly statusCode: number;

  /** True if the error is safe to retry */
  get isRetryable(): boolean;

  /** True if this is an authentication error */
  get isAuthError(): boolean;

  /** True if this is a rate limit error */
  get isRateLimit(): boolean;
}

// ─── WebSocket manager ────────────────────────────────────────────────────────

export class WebSocketManager {
  /**
   * Open a WebSocket connection.
   * Reconnects automatically with exponential backoff.
   */
  async connect(): Promise<WsConnection>;
}

export class WsConnection {
  /** Subscribe to channels with optional cursor for replay */
  async subscribe(channels: string[], cursor?: number): Promise<void>;

  /** Unsubscribe from channels */
  async unsubscribe(channels: string[]): Promise<void>;

  /** Receive events as an AsyncIterable */
  events(): AsyncIterable<WsEvent>;

  /** Close the connection gracefully */
  async close(): Promise<void>;
}

// ─── Usage example ────────────────────────────────────────────────────────────

// import { PolkagentClient } from '@polkagent/client';
//
// const client = new PolkagentClient({
//   baseUrl: 'http://localhost:4840',
//   apiKey: 'pak_...',
// });
//
// // Create and stream a run
// const run = await client.runs.create({
//   agentId: 'agt_01HQ...',
//   input: { type: 'user_message', content: 'Explain the staking pallet' },
// });
//
// for await (const event of client.runs.streamEvents(run.id)) {
//   if (event.type === 'turn.token_delta') {
//     process.stdout.write(event.delta);
//   }
//   if (event.type === 'effect.pending_approval') {
//     console.log(`\nApproval needed: ${event.summary}`);
//     await client.effects.approve(event.effectId, {
//       approvalType: 'human',
//       comment: 'Reviewed and approved',
//     });
//   }
//   if (event.type === 'run.completed' || event.type === 'run.failed') {
//     break;
//   }
// }
```

---

## APPENDIX E: IMPLEMENTATION CHECKLIST

Tasks are grouped by domain, ordered by dependency, and include acceptance
criteria. Phase annotations reference the implementation timeline.

### REST API

- [ ] **E-REST-01** `POST /agents` create endpoint
  - Phase: 0
  - Acceptance: Returns 201 with valid AgentResponse; 422 on invalid spec

- [ ] **E-REST-02** `GET /agents` list with cursor pagination
  - Phase: 0
  - Acceptance: Cursor is opaque; consistent ordering under concurrent writes

- [ ] **E-REST-03** `GET /agents/{id}`, `PUT`, `DELETE` CRUD
  - Phase: 0
  - Acceptance: 404 on unknown ID; PUT increments revision

- [ ] **E-REST-04** `POST /agents/{id}/start` and `POST /agents/{id}/stop`
  - Phase: 0
  - Acceptance: 409 if already in target state

- [ ] **E-REST-05** `POST /runs` with idempotency key enforcement
  - Phase: 0
  - Acceptance: Same key + same body returns 201 (idempotent); same key + different body returns 409

- [ ] **E-REST-06** `GET /runs/{id}`, `POST /runs/{id}/cancel`, `POST /runs/{id}/resume`
  - Phase: 0
  - Acceptance: cancel on terminal run returns 409

- [ ] **E-REST-07** `GET /runs/{id}/stream` SSE endpoint
  - Phase: 1 (validate-next)
  - Acceptance: Streams events in order; reconnects with Last-Event-ID; gap events on lag

- [ ] **E-REST-08** `GET /runs/{id}/turns`, `/events`, `/effects`, `/artifacts`, `/usage`
  - Phase: 0
  - Acceptance: All return paginated responses; filters work correctly

- [ ] **E-REST-09** Effects approve/deny endpoints with idempotency
  - Phase: 0
  - Acceptance: Idempotency key required; 409 on already-resolved effect

- [ ] **E-REST-10** Artifacts upload (`multipart/form-data`) and download (streaming)
  - Phase: 0
  - Acceptance: Content-Hash verified on upload; streaming download works

- [ ] **E-REST-11** Memory query, forget, export, stats endpoints
  - Phase: 1
  - Acceptance: Relevance score within 0.0-1.0; export is resumable JSON-lines

- [ ] **E-REST-12** Provider and model listing with health status
  - Phase: 0
  - Acceptance: Health reflects live connectivity checks

- [ ] **E-REST-13** Chain profile listing and dry-run endpoint
  - Phase: 0 (listing), 1 (dry-run)
  - Acceptance: Dry-run returns success/failure with event log

- [ ] **E-REST-14** Payments endpoints (beta)
  - Phase: 1
  - Acceptance: Protected by `POLKAGENT_FEATURES__PAYMENTS=beta` flag

- [ ] **E-REST-15** Marketplace endpoints (beta)
  - Phase: 2
  - Acceptance: Package integrity verified against registry

- [ ] **E-REST-16** `GET /health` and `GET /versions` (unauthenticated)
  - Phase: 0
  - Acceptance: Returns 200 without auth header; versions list is accurate

- [ ] **E-REST-17** Rate limiting middleware with X-RateLimit-* headers
  - Phase: 0
  - Acceptance: 429 after exhausting window; headers present on every response

- [ ] **E-REST-18** Error envelope on all error responses
  - Phase: 0
  - Acceptance: Every non-2xx response has `{"error":{"code":"...","message":"...","request_id":"..."}}`

### WebSocket

- [ ] **E-WS-01** WebSocket upgrade at `/ws/v1alpha1`
  - Phase: 0
  - Acceptance: Handles 100 concurrent connections; max_message_size enforced
  - Current: Upgrade and real TCP command flows work; explicit message/frame
    size caps and the 100-connection acceptance fixture remain open.

- [ ] **E-WS-02** Token-based auth on WebSocket (query param and first-message)
  - Phase: 0
  - Acceptance: Connection closed on invalid token
  - Current: Both token entry paths are implemented. Invalid first-message auth
    returns an error but does not close, so the stated acceptance is not met.

- [ ] **E-WS-03** Channel subscription with cursor-based replay
  - Phase: 0
  - Acceptance: Replay delivers events in sequence; gap event on buffer miss
  - Current: Durable lag after the first in-session checkpoint replays in
    order from `EventStore`; no public cursor or gap snapshot exists, and
    reconnect is live-only.

- [ ] **E-WS-04** Back-pressure mode declaration
  - Phase: 1
  - Acceptance: `at_most_once` drops when lagged; coalesce not yet implemented (logs warning)
  - Current: No declaration exists. Durable lag is recovered or fails closed;
    diagnostic/ephemeral frames remain best-effort.

- [ ] **E-WS-05** Server ping/pong keep-alive (25s interval, 30s timeout)
  - Phase: 0
  - Acceptance: Connection closed after timeout; client reconnects
  - Current: Server Ping and timeout are both 30 seconds; automatic reconnect
    is a client/SDK gap and command-socket reconnect has no cursor.

- [ ] **E-WS-06** Multi-channel subscriptions (agents, runs, effects, system)
  - Phase: 0
  - Acceptance: Events routed to correct channel; unsubscribe stops delivery
  - Current: Real TCP tests prove concurrent run/agent routing and unsubscribe,
    with a 256-channel cap. Effects are unsupported and `system` has no event
    producer, so the full acceptance remains open.

### SSE

- [ ] **E-SSE-01** `GET /runs/{id}/stream` endpoint
  - Phase: 1
  - Acceptance: Streams events in order with monotonic `id:`; keepalive comment every 8s

- [ ] **E-SSE-02** `Last-Event-ID` reconnection support
  - Phase: 1
  - Acceptance: Resume from `last_seen + 1`; gap event when sequence missing

- [ ] **E-SSE-03** `X-Accel-Buffering: no` and anti-buffering headers
  - Phase: 1
  - Acceptance: Events arrive in real time through Railway/Nginx proxies

- [ ] **E-SSE-04** Gap event with materialized snapshot on lag
  - Phase: 1
  - Acceptance: Gap event includes full state snapshot; client can reconstruct

### Database Schemas

- [ ] **E-DB-01** Initial SQLite schema (all tables from Appendix B.1)
  - Phase: 0
  - Acceptance: Schema applies cleanly; all foreign keys resolve

- [ ] **E-DB-02** Schema migration infrastructure (`schema_migrations` table)
  - Phase: 0
  - Acceptance: `polkagent db migrate` is idempotent; checksum prevents re-run

- [ ] **E-DB-03** Memory FTS5 virtual table with triggers
  - Phase: 1
  - Acceptance: Full-text search returns results; index stays consistent on update/delete

- [ ] **E-DB-04** PostgreSQL schema with RLS and tenant isolation
  - Phase: 2
  - Acceptance: Cross-tenant queries blocked at DB layer

- [ ] **E-DB-05** Events table monthly partitioning (PostgreSQL)
  - Phase: 2
  - Acceptance: Partition pruning used in EXPLAIN output for date-range queries

- [ ] **E-DB-06** pgvector HNSW index for memory similarity search
  - Phase: 2
  - Acceptance: ANN search returns top-20 in < 50ms for 100k entries

- [ ] **E-DB-07** `polkagent db export` and `polkagent db import` (JSON-lines)
  - Phase: 1
  - Acceptance: Round-trip of 100k rows produces identical data

- [ ] **E-DB-08** `polkagent db rollback` with safety gate
  - Phase: 1
  - Acceptance: Rollback blocked in production without `--force-production`

### Configuration

- [ ] **E-CFG-01** TOML configuration loading with 7-layer hierarchy
  - Phase: 0
  - Acceptance: Each layer correctly overrides lower layers; test with all 7 active

- [ ] **E-CFG-02** `POLKAGENT_*` environment variable mapping with `__` for nesting
  - Phase: 0
  - Acceptance: All TOML fields settable via env var

- [ ] **E-CFG-03** `polkagent config validate` with structured error output
  - Phase: 0
  - Acceptance: Reports missing fields, type errors, and range violations

- [ ] **E-CFG-04** `polkagent config show` with secret redaction
  - Phase: 0
  - Acceptance: No secret values in output; redacted fields shown as `[REDACTED]`

- [ ] **E-CFG-05** `polkagent config migrate` between schema versions
  - Phase: 1
  - Acceptance: v1 -> v2 preserves all user values; new fields get documented defaults

- [ ] **E-CFG-06** Hot-reload of configuration without dropping active runs
  - Phase: 1
  - Acceptance: `POST /config/reload` succeeds; active WebSocket connections unaffected

- [ ] **E-CFG-07** Feature flag enforcement (stable/beta/alpha/disabled)
  - Phase: 0
  - Acceptance: Disabled features return 501; beta features return warning object

### SDKs

- [ ] **E-SDK-01** `polkagent-client` Rust crate with all resource groups
  - Phase: 1
  - Acceptance: Compiles without warnings; all documented methods present

- [ ] **E-SDK-02** SSE streaming with typed RunEvent enum in Rust SDK
  - Phase: 1
  - Acceptance: Unknown event types are ignored (not errored)

- [ ] **E-SDK-03** WebSocket support with automatic reconnection in Rust SDK
  - Phase: 1
  - Acceptance: Reconnects after simulated network interruption

- [ ] **E-SDK-04** `@polkagent/client` TypeScript package
  - Phase: 1
  - Acceptance: Published to npm; tree-shakeable; ESM and CJS bundles

- [ ] **E-SDK-05** AsyncIterable streaming in TypeScript SDK
  - Phase: 1
  - Acceptance: `for await (const event of client.runs.streamEvents(id))` works in Node.js and browser

- [ ] **E-SDK-06** Zod schema validation for all request types in TS SDK
  - Phase: 1
  - Acceptance: Invalid requests throw `ZodError` before HTTP call

- [ ] **E-SDK-07** JSON Schema code generation pipeline replacing hand-written types
  - Phase: 2 (validate-next)
  - Acceptance: CI fails if generated types differ from checked-in types

---

## APPENDIX F: REFERENCE FILE MAP

This table maps Polkagent components to the reference implementation files in
Roko (`/Users/will/dev/nunchi/roko/roko/`) that demonstrate proven patterns.

| Polkagent Component | Roko Reference File | Key Pattern |
|---|---|---|
| HTTP server router | `crates/roko-serve/src/routes/mod.rs` | Axum router composition, middleware ordering, rate limiter, CORS |
| Agent management API | `crates/roko-serve/src/routes/agents.rs` | CRUD with supervisor integration, heartbeat tracking, proxy to subprocess |
| Run management API | `crates/roko-serve/src/routes/runs.rs` | RuntimeProjection-backed dashboard; event-sourced run summaries |
| Configuration API | `crates/roko-serve/src/routes/config.rs` | `CONFIG_MUTATION_GATE` mutex, atomic TOML write, hot-reload, secret masking |
| WebSocket streaming | `crates/roko-serve/src/routes/ws.rs` | Ring-buffer replay, cursor resume, back-pressure modes, lag detection |
| SSE streaming | `crates/roko-serve/src/routes/sse.rs` | `Last-Event-ID` reconnection, gap events, materialized snapshots, anti-buffering headers |
| Webhook ingress | `crates/roko-serve/src/routes/webhooks.rs` | HMAC-SHA256 signature verification, Engram persistence, provider-specific event translation |
| Provider health | `crates/roko-serve/src/routes/providers.rs` | Live health snapshot, model resolution, cascade router explanation |
| Workspace management | `crates/roko-serve/src/routes/workspaces.rs` | Ephemeral workspace create/delete, git initialization |
| Config TUI editor | `crates/roko-cli/src/tui/views/config_view.rs` | Scrollable editable field list, section headers, source tagging (file/env/default), save button, hint bar |

### Roko patterns to adopt directly

**Router assembly** (`mod.rs`): Use `Router::merge()` for each domain module
with a central `build_router()` function. Apply auth middleware as a layer
above the merged router, not per-route. This matches Roko's approach where
`api_auth.enabled` wraps the entire API sub-router.

**Config mutation gate** (`config.rs`): Use a `Mutex<()>` ownership gate plus
an `AtomicU64` generation counter to serialize config mutations. The gate holds
through the entire atomic write sequence (merge -> validate -> serialize ->
write TOML -> propagate). Never hold the gate across an `.await`.

**WebSocket size caps** (`ws.rs`): Apply `max_message_size(1 MiB)` and
`max_frame_size(256 KiB)` on every upgrade. Export a shared `apply_ws_size_limits()`
helper used by all WebSocket upgrade handlers (including proxy WebSocket routes).

**SSE gap handling** (`sse.rs`): When the requested `Last-Event-ID` sequence
falls outside the ring buffer, or when the replay slice is too large (> 256
events), replace the replay with a single `gap` event containing a materialized
state snapshot. Never silently truncate replays. The `replay_requires_snapshot()`
function encodes this logic cleanly.

**SSE anti-buffering headers** (`sse.rs`): Always set `X-Accel-Buffering: no`,
`Cache-Control: no-cache, no-store, no-transform, must-revalidate`, and
`Connection: keep-alive`. Use a keep-alive interval of 8 seconds (not 15s) to
survive Railway's 30-second and Nginx's 60-second proxy idle timeouts.

**HMAC webhook verification** (`webhooks.rs`): Verify signatures before any
payload parsing. Reject missing signature headers with 401 (not 400). Use
constant-time comparison to prevent timing attacks. Enforce a webhook body
limit (1 MiB) that is smaller than the global limit (4 MiB).

---

## APPENDIX G: TUI SURFACE FOR API/CONFIG

The Polkagent TUI (terminal user interface) provides three panels for
API/config interaction: a configuration editor, an API explorer, and a schema
browser. These are accessed from the main TUI dashboard via function keys.

---

### G.1 Configuration Editor (F6)

Based directly on Roko's `config_view.rs` pattern: a scrollable list of
editable fields grouped by section, with inline editing, source tagging, and
a save button.

```
┌──────────────────────────────────────────────────────────────────┐
│ Config                                                    [F6]   │
│──────────────────────────────────────────────────────────────────│
│ ── Server ────────────────────────────────────────────────────── │
│   host                          < 127.0.0.1 >           [file]  │
│   port                          < 4840 >                [file]  │
│   request_timeout_seconds        < 300 >               [default] │
│   unsafe_public_cors            [ ]                    [default] │
│                                                                  │
│ ── Auth ──────────────────────────────────────────────────────── │
│   method                        < api_key >             [file]  │
│                                                                  │
│ ── Database ──────────────────────────────────────────────────── │
│   backend                       < sqlite >              [file]  │
│   sqlite.path                                                    │
│     ~/.local/share/polkagent/polkagent.db              [file]  │
│   sqlite.journal_mode           < wal >                [default] │
│   sqlite.synchronous            < normal >             [default] │
│   sqlite.busy_timeout_ms        < 5000 >               [default] │
│                                                                  │
│ ── Execution ─────────────────────────────────────────────────── │
│   max_concurrent_runs           < 10 >                  [env]   │
│     Maximum parallel runs across all agents.                     │
│   default_timeout_seconds       < 600 >                [default] │
│   budget.model_usd_per_run      < 5.00 >               [file]  │
│                                                                  │
│ ── Providers ─────────────────────────────────────────────────── │
│   providers[0].id               < anthropic-default >   [file]  │
│   providers[0].type             < anthropic >           [file]  │
│   providers[0].default_model    < claude-sonnet-4-6 >   [file]  │
│   providers[0].timeout_seconds  < 120 >                [default] │
│                                                                  │
│ ── Runtime: Efficiency ────────────────────────────────────────── │
│   total_cost_usd                $0.2341                  [live] │
│   event_count                   142                      [live] │
│   avg_wall_time_ms              3421                     [live] │
│                                                                  │
│                      [ Apply & Save * ]                         │
│──────────────────────────────────────────────────────────────────│
│ j/k:nav  h/l:cycle  Enter:edit  Ctrl-S:save                     │
└──────────────────────────────────────────────────────────────────┘
```

**Source tags**: `[file]` = value from config file; `[env]` = value from
environment variable (shown in yellow/warning color); `[default]` = compiled
default (shown in muted color). This matches Roko's `ConfigSource` enum
(`File`, `Env`, `Default`).

**Editing mode**: Pressing `Enter` on a field activates inline editing.
The cursor indicator `_` appended to the value shows the edit buffer.
`Enter` confirms; `Esc` cancels. Boolean fields (`[ ]`/`[x]`) toggle with
`h`/`l` or `Enter`.

**Pending changes**: Modified fields are shown in bold accent color.
The save button shows `[ Apply & Save * ]` (with asterisk) when there are
pending changes. `Ctrl-S` writes to the project config file and triggers
hot-reload.

**Sub-tabs** (accessible via Tab or number keys):

```
[1] Config fields     [2] Provider health     [3] Model comparison
```

---

### G.2 API Explorer (F7)

Browse and test endpoints from the TUI. Inspired by httpie/curl but with
inline authentication.

```
┌──────────────────────────────────────────────────────────────────┐
│ API Explorer                                             [F7]   │
│──────────────────────────────────────────────────────────────────│
│ Base URL: http://localhost:4840/api/v1alpha1       Auth: pak_abc │
│                                                                  │
│ Resources                 │ Endpoints                            │
│ ─────────────────────────  ─────────────────────────────────────│
│   Agents           [A]   │ > GET    /agents                     │
│   Runs             [R]   │   POST   /agents                     │
│   Effects          [E]   │   GET    /agents/{id}                │
│   Artifacts        [T]   │   PUT    /agents/{id}                │
│   Events           [V]   │   DELETE /agents/{id}                │
│   Memory           [M]   │   POST   /agents/{id}/start          │
│   Providers        [P]   │   POST   /agents/{id}/stop           │
│   Chains           [C]   │   GET    /agents/{id}/status         │
│   Marketplace      [K]   │   GET    /agents/{id}/revisions      │
│   System           [S]   │                                      │
│                           │                                      │
│──────────────────────────────────────────────────────────────────│
│ Request                                                          │
│ GET /agents?status=running&page_size=10                          │
│                                                                  │
│ Response (200 OK)  42ms                                          │
│ {                                                                │
│   "data": [                                                      │
│     { "id": "agt_01HQ...", "name": "my-builder-agent",          │
│       "status": "running", "revision": 3, ... }                  │
│   ],                                                             │
│   "cursor": { "next": null, "has_more": false },                 │
│   "meta": { "page_size": 10, "total_estimate": 1 }              │
│ }                                                                │
│──────────────────────────────────────────────────────────────────│
│ [Enter]:execute  [e]:edit request  [c]:copy response  [h]:history│
└──────────────────────────────────────────────────────────────────┘
```

**Key bindings**:

| Key | Action |
|---|---|
| `j/k` | Navigate resources / endpoints |
| `Enter` | Execute the highlighted endpoint |
| `e` | Open request editor (body, headers, params) |
| `c` | Copy response to clipboard |
| `h` | Open request history |
| `s` | Toggle streaming (for SSE endpoints) |
| `/` | Search endpoints by name |
| `q` | Exit API explorer |

**Request editor** (activated by `e`):

```
┌──────────────────────────────────────────────────────────────────┐
│ Edit Request                                                     │
│──────────────────────────────────────────────────────────────────│
│ Method:  POST                                                    │
│ Path:    /agents                                                 │
│                                                                  │
│ Headers:                                                         │
│   Authorization: Bearer pak_...         [auto]                  │
│   X-Idempotency-Key: [generate]         [optional]              │
│                                                                  │
│ Body (JSON):                                                     │
│ {                                                                │
│   "spec": {                                                      │
│     "apiVersion": "polkagent.dev/v1alpha1",                     │
│     "kind": "Agent",                                            │
│     "metadata": { "name": "my-agent" },                         │
│     "spec": {                                                    │
│       "description": "My agent",                                │
│       "execution": { "route": "recommended-coding-harness" }    │
│     }                                                            │
│   }                                                              │
│ }                                                                │
│──────────────────────────────────────────────────────────────────│
│ [Ctrl-S]:send  [Esc]:cancel  [Tab]:next field                   │
└──────────────────────────────────────────────────────────────────┘
```

---

### G.3 Schema Browser (F8)

Browse the database tables and their live contents from the TUI.

```
┌──────────────────────────────────────────────────────────────────┐
│ Schema Browser                                           [F8]   │
│──────────────────────────────────────────────────────────────────│
│ Tables                    │ Columns: runs                        │
│ ─────────────────────────  ─────────────────────────────────────│
│   agents            (3)   │ Column              Type     Index   │
│ > runs              (12)  │ ──────────────────────────────────── │
│   turns             (47)  │ id                  TEXT     PK      │
│   effects           (8)   │ agent_id            TEXT     FK, IDX │
│   effect_attempts   (8)   │ conversation_id     TEXT     FK      │
│   effect_outcomes   (5)   │ workspace_id        TEXT     FK      │
│   artifacts         (23)  │ status              TEXT     IDX     │
│   events            (312) │ input_json          TEXT            │
│   grants            (3)   │ config_json         TEXT            │
│   approvals         (5)   │ grant_hash          TEXT            │
│   memory_entries    (91)  │ turns_completed     INTEGER         │
│   api_keys          (2)   │ input_tokens        INTEGER         │
│                           │ output_tokens       INTEGER         │
│                           │ model_cost_usd      REAL            │
│                           │ idempotency_key     TEXT     UNIQ   │
│                           │ created_at          TEXT     IDX    │
│                           │ started_at          TEXT            │
│                           │ completed_at        TEXT            │
│──────────────────────────────────────────────────────────────────│
│ Preview: runs (12 rows)                                          │
│ id              agent_id      status     turns  cost_usd         │
│ ─────────────────────────────────────────────────────────────── │
│ run_01HQ...     agt_01HQ...   completed  7      0.3145          │
│ run_01HR...     agt_01HQ...   running    3      0.1020          │
│ run_01HS...     agt_01HR...   failed     2      0.0891          │
│──────────────────────────────────────────────────────────────────│
│ [j/k]:nav tables  [l]:view rows  [i]:table info  [q]:search SQL │
└──────────────────────────────────────────────────────────────────┘
```

**Key bindings**:

| Key | Action |
|---|---|
| `j/k` | Navigate tables |
| `l` or `Enter` | View row preview for selected table |
| `i` | Show table info (indexes, constraints, row count) |
| `q` | Open SQL query input |
| `r` | Refresh row counts |
| `e` | Export table as JSON-lines |
| `/` | Filter tables by name |

**SQL query panel** (activated by `q`):

```
┌──────────────────────────────────────────────────────────────────┐
│ SQL Query                                                        │
│──────────────────────────────────────────────────────────────────│
│ > SELECT id, agent_id, status, model_cost_usd                   │
│     FROM runs                                                    │
│     WHERE status = 'completed'                                   │
│     ORDER BY created_at DESC                                     │
│     LIMIT 20;                                                    │
│                                                                  │
│ Results (20 rows, 12ms):                                         │
│ id              agent_id      status     model_cost_usd          │
│ ─────────────────────────────────────────────────────────────── │
│ run_01HQ...     agt_01HQ...   completed  0.3145                 │
│ run_01HP...     agt_01HQ...   completed  0.2891                 │
│ run_01HO...     agt_01HR...   completed  0.1023                 │
│──────────────────────────────────────────────────────────────────│
│ [Ctrl-Enter]:execute  [Esc]:cancel  [c]:copy results            │
└──────────────────────────────────────────────────────────────────┘
```

**Safety**: The SQL panel is read-only (SELECT only). DML and DDL are
rejected with an error message. The connection used by the schema browser
is the read connection pool; it never blocks writes.

---

*End of PRD-14 Appendices.*

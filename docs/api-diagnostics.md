# Polkagent API Diagnostic Endpoints Catalog

**Generated:** 2026-08-03
**Project:** /Users/will/dev/par/polkagent
**API Version:** v1alpha1 (defined in `/crates/polkagent-api/src/dto.rs`)

---

## Overview

The Polkagent API provides diagnostic endpoints for monitoring system health, collecting metrics, gathering system information, auditing operations, and streaming real-time events. The three orchestrator health probes and PCA bridge health discovery are public and bypass rate limiting. Metrics and versioned diagnostic APIs remain authenticated and rate-limited because they expose operational details.

---

## 1. Health Check Endpoints

**Location:** `/crates/polkagent-api/src/routes/health.rs`

Health endpoints are mounted at the **root** (not under `/api/v1alpha1`) for load-balancer compatibility. They implement the Kubernetes probe pattern.

### 1.1 `GET /health/live`
**Purpose:** Liveness probe — always returns 200 if the process is alive.

**Response:**
```json
{
  "status": "ok"
}
```

**Status Code:**
- `200 OK` — always (stateless check)

**Use Case:** Kubernetes liveness probe; detect dead processes.

---

### 1.2 `GET /health/ready`
**Purpose:** Readiness probe — 200 only when all critical dependencies are healthy.

**Response:**
```json
{
  "status": "ok|unavailable",
  "ready": true|false,
  "checks": [
    {
      "name": "database",
      "status": "ok|degraded|error",
      "latency_ms": 5,
      "message": null
    },
    {
      "name": "event_bus",
      "status": "ok",
      "latency_ms": 0
    },
    {
      "name": "memory_store",
      "status": "degraded",
      "message": "memory store not configured"
    }
  ]
}
```

**Status Code:**
- `200 OK` — if `ready` is true and no "error" components exist
- `503 Service Unavailable` — if any component has status "error" or `ready` is false

**Component Status Levels:**
- `"ok"` — component is fully functional
- `"degraded"` — component is available but not at full capacity (e.g., optional component unconfigured)
- `"error"` — component is not functional; readiness probe fails

**Fields:**
- `name` — component identifier (e.g., `"database"`, `"event_bus"`, `"memory_store"`)
- `status` — component health status
- `latency_ms` — probe round-trip latency in milliseconds (omitted if not measured)
- `message` — diagnostic message; omitted when null

**Use Case:** Kubernetes readiness probe; route traffic only to healthy instances.

---

### 1.3 `GET /health/startup`
**Purpose:** Startup probe — 200 after initial setup completes.

**Response:**
```json
{
  "status": "ok|starting",
  "detail": "initialisation complete|initialisation in progress"
}
```

**Status Code:**
- `200 OK` — once `HealthState.ready` has been set to `true`
- `503 Service Unavailable` — while server is still initializing

**Use Case:** Kubernetes startup probe; prevent traffic routing until initialization completes.

---

### 1.4 `GET /health` (Full Health Report)
**Purpose:** Comprehensive health report with component diagnostics (for production deployments).

**Response:**
```json
{
  "status": "ok|degraded|error",
  "version": "0.2.1",
  "uptime_seconds": 3600,
  "checks": [
    {
      "name": "database",
      "status": "ok",
      "latency_ms": 8,
      "message": null
    },
    {
      "name": "event_bus",
      "status": "ok",
      "latency_ms": 0
    }
  ]
}
```

**Status Code:**
- `200 OK` — when status is "ok" or "degraded"
- `503 Service Unavailable` — when status is "error"

**Aggregate Status Logic:**
- All components `"ok"` → status is `"ok"`
- Any component `"degraded"` (but no `"error"`) → status is `"degraded"`
- Any component `"error"` → status is `"error"`

**Fields:**
- `status` — aggregate health status across all components
- `version` — Polkagent platform version (from `CARGO_PKG_VERSION`)
- `uptime_seconds` — elapsed time since process startup
- `checks` — array of component health details (sorted by name)

**Use Case:** Detailed health monitoring; manual inspection; alerting thresholds.

---

## 2. Prometheus Metrics Endpoint

**Location:** `/crates/polkagent-api/src/routes/metrics.rs`

### 2.1 `GET /metrics`
**Purpose:** Prometheus text exposition format metrics for scraping.

**Response Format:** Prometheus text exposition format (OpenMetrics 0.0.4)
```
# HELP polkagent_runs_total Total number of runs executed
# TYPE polkagent_runs_total counter
polkagent_runs_total{agent="alpha",status="completed"} 42

# HELP polkagent_active_runs Number of currently running agents
# TYPE polkagent_active_runs gauge
polkagent_active_runs 3

# HELP polkagent_run_duration_seconds Run execution time in seconds
# TYPE polkagent_run_duration_seconds histogram
polkagent_run_duration_seconds_bucket{le="0.1"} 5
polkagent_run_duration_seconds_bucket{le="0.5"} 12
polkagent_run_duration_seconds_bucket{le="1.0"} 38
polkagent_run_duration_seconds_bucket{le="+Inf"} 42
polkagent_run_duration_seconds_count 42
polkagent_run_duration_seconds_sum 87.5

# HELP polkagent_model_tokens_total Total tokens consumed by model calls
# TYPE polkagent_model_tokens_total counter
polkagent_model_tokens_total{model="claude-opus-4-6",provider="anthropic"} 125000

# HELP polkagent_effects_total Total effects executed
# TYPE polkagent_effects_total counter
polkagent_effects_total{status="approved"} 150
polkagent_effects_total{status="denied"} 8
```

**Status Code:**
- `200 OK` — authenticated scrape (returns an empty body if no metrics are registered)
- `401 Unauthorized` — authentication is enabled and credentials are missing or invalid
- `429 Too Many Requests` — the collector exceeded its configured token bucket

**Content-Type:**
- `text/plain; version=0.0.4; charset=utf-8`

**Metric Types Exposed:**
- **Counters** — monotonically increasing values
  - `polkagent_runs_total` — cumulative runs completed
  - `polkagent_effects_total` — cumulative effects executed
  - `polkagent_model_tokens_total` — cumulative tokens consumed

- **Gauges** — snapshot current values
  - `polkagent_active_runs` — currently executing run count

- **Histograms** — distribution observations
  - `polkagent_run_duration_seconds` — run execution time buckets
  - Additional percentile buckets and sums available

**Use Case:** Integration with Prometheus, Grafana Agent, or OpenMetrics collectors; real-time dashboards.

**Source:** `/crates/polkagent-telemetry/src/prometheus.rs` (PrometheusRegistry)

---

## 3. System Information Endpoint

**Location:** `/crates/polkagent-api/src/routes/system.rs`

### 3.1 `GET /api/v1alpha1/system/info`
**Purpose:** Return version, uptime, and non-secret configuration summary.

**Response:**
```json
{
  "version": "v1alpha1",
  "platform_version": "0.2.1",
  "uptime_secs": 3600,
  "config_summary": {
    "bind_address": "0.0.0.0:9090",
    "database_backend": "sqlite",
    "max_concurrent_runs": 10
  }
}
```

**Status Code:**
- `200 OK` — always

**Response Fields:**
- `version` — API version string (currently `"v1alpha1"`)
- `platform_version` — Polkagent platform version (from `Cargo.toml`)
- `uptime_secs` — seconds elapsed since server process started
- `config_summary` — non-sensitive configuration details only:
  - `bind_address` — API server bind address (from `config.api.bind_address`)
  - `database_backend` — database type (`"sqlite"` or `"postgres"`)
  - `max_concurrent_runs` — maximum parallel run executions

**Security Notes:**
- No secrets, API keys, or database connection URLs are exposed
- Only non-sensitive configuration fields are included

**Use Case:** Verify platform version; check uptime; inspect non-secret configuration.

---

## 4. Audit Log Endpoints

**Location:** `/crates/polkagent-api/src/routes/audit.rs`

All audit endpoints return `501 Not Implemented` if no audit store is configured.

### 4.1 `GET /api/v1alpha1/audit`
**Purpose:** Query audit entries with optional filtering.

**Query Parameters:**
- `actor` (optional) — filter by actor ID (exact match)
- `action` (optional) — filter by action type (snake_case, e.g., `"run_started"`, `"tool_invoked"`)
- `since` (optional) — RFC-3339 timestamp; include entries at or after this time
- `until` (optional) — RFC-3339 timestamp; include entries at or before this time
- `limit` (optional) — max entries to return (default: 100, max recommended: 1000)

**Example Request:**
```
GET /api/v1alpha1/audit?actor=agent-1&action=run_started&since=2026-08-01T00:00:00Z&limit=50
```

**Response:**
```json
{
  "version": "v1alpha1",
  "count": 2,
  "data": [
    {
      "id": "550e8400-e29b-41d4-a716-446655440000",
      "timestamp": "2026-08-03T10:30:00Z",
      "actor": {
        "type": "agent",
        "id": "agent-1"
      },
      "action": "run_started",
      "resource": {
        "type": "run",
        "id": "run-123"
      },
      "outcome": "success",
      "details": {
        "trigger": "api"
      }
    }
  ]
}
```

**Status Code:**
- `200 OK` — query executed successfully
- `422 Unprocessable Entity` — invalid filter values (malformed timestamp, unknown action)
- `501 Not Implemented` — audit store not configured

**Filter Examples:**
- `/audit?actor=user-42` — all actions by user-42
- `/audit?action=tool_invoked` — all tool invocations
- `/audit?since=2026-08-03T00:00:00Z&until=2026-08-03T23:59:59Z` — events on Aug 3
- `/audit?limit=10` — most recent 10 entries

---

### 4.2 `GET /api/v1alpha1/audit/{id}`
**Purpose:** Get a single audit entry by UUID.

**Path Parameters:**
- `id` — UUID of the audit entry

**Response:**
```json
{
  "version": "v1alpha1",
  "data": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "timestamp": "2026-08-03T10:30:00Z",
    "actor": {
      "type": "agent",
      "id": "agent-1"
    },
    "action": "run_started",
    "resource": {
      "type": "run",
      "id": "run-123"
    },
    "outcome": "success",
    "details": {}
  }
}
```

**Status Code:**
- `200 OK` — entry found
- `404 Not Found` — entry does not exist
- `422 Unprocessable Entity` — invalid UUID format
- `501 Not Implemented` — audit store not configured

---

### 4.3 `GET /api/v1alpha1/audit/verify`
**Purpose:** Verify the integrity of the audit hash chain.

**Response:**
```json
{
  "version": "v1alpha1",
  "valid": true,
  "entries_checked": 12345,
  "error": null
}
```

**Status Code:**
- `200 OK` — integrity check completed (regardless of validity)
- `501 Not Implemented` — audit store not configured

**Response Fields:**
- `valid` — true if the entire hash chain is valid
- `entries_checked` — number of entries verified
- `error` — null if valid; error message if hash chain is broken

**Security Notes:**
- Verifies the cryptographic hash chain for audit log tamper-detection
- Detects insertion, deletion, or modification of entries
- Can be expensive on large audit logs (check `entries_checked`)

---

## 5. Event Stream WebSocket Endpoint

**Location:** `/crates/polkagent-api/src/routes/events.rs`

### 5.1 `GET /api/v1alpha1/events/stream` (WebSocket)
**Purpose:** Replay durable run events from a global checkpoint, then follow
live run events via WebSocket.

**Protocol:**
- Upgrade: WebSocket
- Frames: JSON text frames containing `RunEvent` objects; durable frames add
  `global_sequence`
- Keepalive: Server sends Ping frames every 30 seconds
- Filtering: Optional query parameters for selective streaming
- Recovery: Receiver attaches before bounded durable replay; lag resumes after
  the last consumed global checkpoint

**Query Parameters:**
- `after_sequence` (optional, default `0`) — replay durable events whose global
  sequence is strictly greater than this non-negative integer
- `run_id` (optional) — filter events to a specific run
- `kinds` (optional) — comma-separated event kind names (e.g., `run_created,turn_started,run_completed`)

**Example Connection:**
```
GET /api/v1alpha1/events/stream?after_sequence=42&run_id=0198bd19-40c0-7000-8000-000000000001&kinds=run_created,turn_started,run_completed HTTP/1.1
Upgrade: websocket
Connection: Upgrade
```

**Event Frame Format:**
```json
{
  "id": "0198bd19-40c0-7000-8000-000000000002",
  "run_id": "0198bd19-40c0-7000-8000-000000000001",
  "sequence": 1,
  "global_sequence": 43,
  "kind": "run_created",
  "durability": "durable",
  "timestamp": "2026-08-03T10:30:00Z",
  "correlation": {
    "run_id": "0198bd19-40c0-7000-8000-000000000001",
    "turn_id": null,
    "step_id": null,
    "effect_intent_id": null,
    "effect_attempt_id": null
  },
  "causation_id": null
}
```

Durable replay preserves all four optional correlation components and
`causation_id` exactly. Invalid persisted JSON, timestamps, typed IDs,
durability, or event-type/payload pairs close the socket with the sanitized
invalid-recovery reason; the server does not synthesize defaults. Scope and
conversation metadata are filter values, not tenant/principal authorization.

**Supported Event Kinds:**
- `run_created` — run initialized
- `run_started` — execution began
- `run_queued` — queued for execution
- `run_completed` — execution finished
- `turn_started` — reasoning turn started
- `turn_completed` — reasoning turn ended
- `tool_invoked` — external tool called
- `effect_proposed` — system effect proposed
- `effect_approved` — effect approved by human/system
- `effect_denied` — effect rejected
- Additional kinds available (see `/crates/polkagent-core/src/event.rs`)

**Filter Examples:**
- `/events/stream` — replay all retained durable events, then follow live events
- `/events/stream?after_sequence=42` — resume strictly after global sequence 42
- `/events/stream?run_id=run-123` — only events from run-123
- `/events/stream?kinds=run_started,run_completed` — only run lifecycle events
- `/events/stream?run_id=run-456&kinds=turn_started` — only turn starts for run-456

**Status Code (Upgrade):**
- `101 Switching Protocols` — WebSocket upgrade accepted
- `422 Unprocessable Entity` — invalid `run_id` or `after_sequence`
- `501 Not Implemented` — no durable event store is configured

**WebSocket Control Frames:**
- Ping: Sent by server every 30 seconds for keepalive
- Pong: Client should echo ping (automatic in most WebSocket libraries)
- Close: Client or server can close the connection gracefully

**Error Handling:**
- Client disconnect: Server cleanly exits the event loop
- EventBus closed: Server exits the stream
- Lagged receiver: Server reloads durable pages strictly after its last consumed
  global checkpoint and deduplicates replay/live overlap
- Durable store/backend failure: Server sends close status `1011` with the
  generic reason `durable event recovery unavailable`; backend text is never
  serialized
- Invalid durable projection: Server sends close status `1011` with the generic
  reason `durable event recovery invalid`

Durable replay reads at most 256 records at a time. Filters advance the global
checkpoint through non-matching rows, so a matching event on a later page is
not skipped. Diagnostic and ephemeral events remain live-only and may be lost
during disconnect or lag; clients must use durable events for correctness.

**Use Case:** Real-time dashboards; event logging systems; real-time status updates; debugging run execution.

### 5.2 `GET /ws/v1alpha1` (command WebSocket)

**Purpose:** Manage live run/agent channel subscriptions over a bidirectional
command envelope. This is a separate wire protocol from section 5.1.

**Implemented protocol:**

- Authentication: `?token=<token>` or first text message
  `{"msg_type":"auth","token":"..."}`
- Commands: `auth`, `subscribe`, `unsubscribe`, and `ping`; each subscription
  command carries one `channel`
- Channels: `runs:{run_id}`, `agents:{agent_id}`, and syntactically `system`;
  the current run-event source has no system-event producer
- Replies: `WsMessage` JSON with `msg_type` (`ack`, `pong`, or `error`), optional
  request `id`/`channel`, `payload`, and `timestamp`
- Events: `msg_type: "event"`, a canonical `RunEvent` in `payload`, and the
  concrete `runs:{run_id}` channel even when selected by an agent subscription
- Bounds: at most 256 distinct channels per connection; server Ping every 30
  seconds and a 30-second Pong timeout

The live receiver is attached before upgrade completion. Once the connection
has observed a valid durable event, an internal global checkpoint advances only
after successful delivery (or after a non-matching durable row is skipped).
Later live durable notifications and `Lagged` errors recover from the injected
`EventStore` in 256-record pages, validate forward progress, and deduplicate
overlap. Agent routing performs a current run lookup and does not retain an
unbounded per-connection run cache.

The existing command envelope exposes no cursor, so reconnect does not replay
events emitted while disconnected. Its event payload also does not expose the
internal global checkpoint. Diagnostic and ephemeral delivery remains
best-effort. `system` subscriptions currently receive no run events; effect,
conversation, cancellation, cursor, and back-pressure-declaration commands are
not implemented.

No durable store rejects the HTTP upgrade with `501`. A backend error, invalid
projection (including a zero/non-progressing durable sequence), or receiver lag
before the first durable checkpoint sends a generic `error` envelope and then a
`1011` close. Use section 5.1 or interaction SSE when reconnect recovery is a
correctness requirement.

---

## 6. Event Log REST Endpoint

**Location:** `/crates/polkagent-api/src/routes/events_rest.rs`

### 6.1 `GET /api/v1alpha1/events`
**Purpose:** Query historical events (REST alternative to WebSocket stream).

**Query Parameters:**
- Standard pagination: `limit`, `after`
- Filtering: `run_id`, `kind` (optional)

**Response:**
```json
{
  "version": "v1alpha1",
  "data": [
    {
      "id": "event-550e8400-e29b-41d4-a716-446655440000",
      "run_id": "run-123",
      "sequence": 1,
      "kind": "run_created",
      "timestamp": "2026-08-03T10:30:00Z",
      "correlation": {
        "run_id": "run-123",
        "agent_id": "agent-1"
      },
      "payload": {}
    }
  ]
}
```

**Status Code:** `200 OK`

---

### 6.2 `GET /api/v1alpha1/events/{id}`
**Purpose:** Fetch a single event by ID.

**Status Code:**
- `200 OK` — event found
- `404 Not Found` — event not found

---

## 7. Route Map Summary

```
ROOT (/)
  GET  /health                    → Full health report (200/503)
  GET  /health/live               → Liveness probe (always 200)
  GET  /health/ready              → Readiness probe (200/503)
  GET  /health/startup            → Startup probe (200/503)
  GET  /metrics                   → Prometheus metrics (200)

API VERSIONED (/api/v1alpha1)
  GET  /system/info               → System info (200)
  GET  /audit                     → List audit entries (200/422/501)
  GET  /audit/{id}                → Get audit entry (200/404/422/501)
  GET  /audit/verify              → Verify audit integrity (200/501)
  GET  /events                    → List events (200)
  GET  /events/{id}               → Get event (200/404)
  GET  /events/stream (WS)        → Stream events (101 upgrade)
```

---

## 8. Architecture Notes

### Health State Management
- Defined in `/crates/polkagent-health/src/lib.rs`
- Tracks per-component status in `Arc<RwLock<HashMap>>`
- Thread-safe concurrent read/write
- Components can update their status dynamically during execution

### Prometheus Registry
- Defined in `/crates/polkagent-telemetry/src/prometheus.rs`
- Maintains metric families in text exposition format
- Supports counters, gauges, and histograms
- Scrapeable by standard Prometheus/Grafana Agent

### Audit Store Interface
- Defined in `/crates/polkagent-audit/src/lib.rs`
- Optional: configured via `AppState::audit_store`
- Returns 501 if not configured
- Supports hash chain integrity verification

### Event Bus
- In-process broadcast channel
- Subscribers receive all events in real-time
- Capacity is configurable (default from `EventBus::with_default_capacity()`)
- WebSocket clients subscribe via `.subscribe()` method

---

## 9. Configuration

Health endpoints are always available (no version prefix) for load-balancer compatibility:
- Liveness, readiness, and startup probes at root
- Prometheus metrics at root
- Full health report available at `/health`

API diagnostics (system/info, audit, events) are under `/api/v1alpha1/` with standard authentication.

---

## 10. Security Considerations

### Public vs. Authenticated
- **Health probes** (live, ready, startup): No authentication required (load-balancer use)
- **Metrics endpoint**: No authentication (standard Prometheus)
- **System info**: Authenticated; no secrets exposed
- **Audit endpoints**: Authenticated; exposes operational metadata
- **Event stream**: Authenticated; exposes run execution events

### Data Exposure
- No API keys, connection strings, or secrets in any response
- System info redacts sensitive configuration
- Audit entries expose actor/action/resource; details are logged separately

---

## References

- **Health Routes:** `/crates/polkagent-api/src/routes/health.rs:330-337`
- **Health State:** `/crates/polkagent-health/src/lib.rs`
- **Metrics Handler:** `/crates/polkagent-api/src/routes/metrics.rs:35-45`
- **System Info Handler:** `/crates/polkagent-api/src/routes/system.rs:24-41`
- **Audit Handlers:** `/crates/polkagent-api/src/routes/audit.rs:48-182`
- **Event WebSocket:** `/crates/polkagent-api/src/routes/events.rs:113-214`
- **Event REST:** `/crates/polkagent-api/src/routes/events_rest.rs`
- **DTOs:** `/crates/polkagent-api/src/dto.rs:237-295`
- **OpenAPI Spec:** `/openapi.yaml` (lines 1-200+)

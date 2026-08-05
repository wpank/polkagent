# API Reference

Polkagent exposes a REST and WebSocket API under the `/api/v1alpha1` prefix.

## Starting the server

```bash
polkagent serve --port 9090 --host 127.0.0.1
```

**Body limits:** 1 MiB for standard requests, 10 MiB for artifact content.

```mermaid
graph LR
    subgraph API["/api/v1alpha1"]
        AG["/agents"]
        RN["/runs"]
        EF["/effects"]
        AR["/artifacts"]
        EV["/events"]
        PR["/providers"]
        MD["/models"]
        SK["/skills"]
        TL["/tools"]
        PM["/payments"]
        MM["/memory"]
        AU["/audit"]
        CV["/conversations"]
        IN["/interactions"]
        SY["/system"]
    end

    subgraph Health["Health & Metrics"]
        HL["/health/live"]
        HR["/health/ready"]
        HS["/health/startup"]
        MT["/metrics"]
        OA["/openapi.json"]
    end

    CLIENT["Client"] --> API
    CLIENT --> Health
```

---

## Endpoints

### Agents

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1alpha1/agents` | Create an agent |
| `GET` | `/api/v1alpha1/agents` | List agents |
| `GET` | `/api/v1alpha1/agents/:id` | Get agent by ID |
| `DELETE` | `/api/v1alpha1/agents/:id` | Delete an agent |
| `POST` | `/api/v1alpha1/agents/:id/start` | Start an agent |
| `POST` | `/api/v1alpha1/agents/:id/stop` | Stop an agent |
| `POST` | `/api/v1alpha1/agents/:id/pause` | Pause an agent |
| `POST` | `/api/v1alpha1/agents/:id/resume` | Resume an agent |

### Runs

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1alpha1/agents/:agent_id/runs` | Create a run for an agent |
| `GET` | `/api/v1alpha1/runs` | List runs |
| `GET` | `/api/v1alpha1/runs/:id` | Get run by ID |
| `POST` | `/api/v1alpha1/runs/:id/cancel` | Cancel a run |
| `GET` | `/api/v1alpha1/runs/:id/turns` | Get turns for a run |
| `GET` | `/api/v1alpha1/runs/:id/events` | Get events for a run |
| `GET` | `/api/v1alpha1/runs/:id/artifacts` | Get artifacts for a run |
| `GET` | `/api/v1alpha1/runs/:id/effects` | Get effects for a run |
| `POST` | `/api/v1alpha1/runs/:id/resume` | Resume a run |

```mermaid
sequenceDiagram
    participant C as Client
    participant API as Polkagent API
    participant RM as Run Manager
    participant LLM as LLM Provider

    C->>API: POST /agents/:id/runs {prompt}
    API->>RM: Create Run
    RM-->>API: Run (state: created)
    API-->>C: 201 Created {run_id, state: "created"}

    Note over RM: Async execution begins
    RM->>RM: Created → Queued → Running
    RM->>LLM: Model inference
    LLM-->>RM: Response
    RM->>RM: Running → Completed

    C->>API: GET /runs/:id
    API-->>C: 200 {state: "completed", turns, usage}

    C->>API: GET /runs/:id/artifacts
    API-->>C: 200 [{artifact_id, content_type}]
```

### Effects

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1alpha1/effects/:id` | Get effect by ID |
| `POST` | `/api/v1alpha1/effects/:id/approve` | Approve a pending effect |
| `POST` | `/api/v1alpha1/effects/:id/deny` | Deny a pending effect |

```mermaid
sequenceDiagram
    participant C as Client
    participant API as Polkagent API
    participant EP as Effect Pipeline
    participant EXT as External System

    Note over EP: Effect requires approval
    EP->>EP: Run → AwaitingApproval

    C->>API: GET /runs/:id
    API-->>C: {state: "awaiting_approval"}

    C->>API: GET /runs/:id/effects
    API-->>C: [{id, kind, state: "pending"}]

    C->>API: POST /effects/:id/approve
    API->>EP: Approve effect
    EP->>EP: Claim → Execute
    EP->>EXT: Perform I/O
    EXT-->>EP: Result
    EP->>EP: Record outcome
    EP-->>API: Effect resolved
    API-->>C: 200 {state: "resolved"}
```

### Artifacts

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1alpha1/artifacts/:id` | Get artifact metadata |
| `GET` | `/api/v1alpha1/artifacts/:id/content` | Download artifact content (10 MiB limit) |
| `GET` | `/api/v1alpha1/artifacts/:id/provenance` | Get artifact provenance chain |

### Events

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1alpha1/events` | List events |
| `GET` | `/api/v1alpha1/events/:id` | Get event by ID |
| `GET` | `/api/v1alpha1/events/stream` | WebSocket event stream |

### Providers

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1alpha1/providers` | List providers |
| `GET` | `/api/v1alpha1/providers/:id` | Get provider details |
| `GET` | `/api/v1alpha1/providers/:provider_id/models` | List models for a provider |

### Models

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1alpha1/models` | List all models |
| `GET` | `/api/v1alpha1/models/:model_id` | Get model details |

### Skills

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1alpha1/skills` | List skills |
| `GET` | `/api/v1alpha1/skills/:skill_id` | Get skill details |
| `POST` | `/api/v1alpha1/skills/install` | Install a skill |
| `POST` | `/api/v1alpha1/skills/:skill_id/uninstall` | Uninstall a skill |
| `PUT` | `/api/v1alpha1/skills/:skill_id/config` | Update skill config |

### Tools

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1alpha1/tools` | List available tools |
| `GET` | `/api/v1alpha1/tools/:tool_id` | Get tool details |
| `GET` | `/api/v1alpha1/tools/:tool_id/grants` | Get tool's required grants |

### Payments

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1alpha1/payments/balance` | Get payment balance |
| `GET` | `/api/v1alpha1/payments/usage` | Get usage summary |
| `GET` | `/api/v1alpha1/payments/receipts` | List receipts |
| `GET` | `/api/v1alpha1/payments/receipts/:receipt_id` | Get receipt |

### Memory

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1alpha1/memory/query` | Search memories |
| `GET` | `/api/v1alpha1/memory/stats` | Memory statistics |
| `POST` | `/api/v1alpha1/memory/forget` | Forget a memory |
| `GET` | `/api/v1alpha1/memory/entries/:entry_id` | Get memory entry |

### Audit

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1alpha1/audit` | List audit entries |
| `GET` | `/api/v1alpha1/audit/verify` | Verify audit integrity |
| `GET` | `/api/v1alpha1/audit/:id` | Get audit entry |

### Conversations

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1alpha1/conversations` | Create conversation |
| `GET` | `/api/v1alpha1/conversations` | List conversations |
| `GET` | `/api/v1alpha1/conversations/:id` | Get conversation |
| `POST` | `/api/v1alpha1/conversations/:id/messages` | Append a transcript record without executing an agent |
| `DELETE` | `/api/v1alpha1/conversations/:id` | Delete conversation |

The conversation routes are a low-level transcript compatibility API. Use the
interaction routes below when a prompt must initiate agent work.

### Durable interactions

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/api/v1alpha1/interactions` | Create an agent interaction |
| `GET` | `/api/v1alpha1/interactions` | List interactions |
| `GET` | `/api/v1alpha1/interactions/:id` | Get an interaction |
| `DELETE` | `/api/v1alpha1/interactions/:id` | Archive a terminal interaction |
| `GET` | `/api/v1alpha1/interactions/:id/turns` | List durable turns |
| `POST` | `/api/v1alpha1/interactions/:id/prompt` | Start an idempotent prompt turn |
| `POST` | `/api/v1alpha1/interactions/:id/turns/:turn_id/cancel` | Cancel a turn |
| `PUT` | `/api/v1alpha1/interactions/:id/target` | Change the agent target |
| `GET` | `/api/v1alpha1/interactions/:id/events` | Replay events after a durable sequence |
| `GET` | `/api/v1alpha1/interactions/:id/events/stream` | Replay and follow typed events over SSE |

Callers may supply `turn_id` when prompting. A retry with identical input
returns the original handle, while reuse with different input returns `409`.
For replay, send `after_sequence` and persist the returned
`checkpoint.next_after_sequence`; optional `turn_id` and `limit` parameters
filter and page the ordered durable history.

The SSE route accepts the same `after_sequence` and optional `turn_id` model.
On browser or client reconnect, a valid `Last-Event-ID` header takes precedence
over `after_sequence`. Each `interaction_event` has an `id` equal to its
durable interaction-wide sequence and a JSON `data` field containing the typed
event envelope. Comment-only keepalives carry no application data. On bounded
receiver lag, the server replays after the last event it emitted, avoiding
duplicates and gaps; terminal turn events do not close the session stream.

The underlying interaction service attaches its bounded live receiver first,
then loads durable replay lazily in bounded pages as the client consumes the
stream. Reconnecting from an old checkpoint therefore does not materialize the
full backlog, and disconnecting early stops further replay reads.

### System

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1alpha1/system/info` | System information |

### Health and metrics

These endpoints are served without the `/api/v1alpha1` prefix.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health/live` | Liveness probe |
| `GET` | `/health/ready` | Readiness probe |
| `GET` | `/health/startup` | Startup probe |
| `GET` | `/metrics` | Prometheus metrics |
| `GET` | `/openapi.json` | OpenAPI specification |

---

## WebSocket event streaming

Connect to `/api/v1alpha1/events/stream` to receive real-time events. The stream delivers run lifecycle events including `RunStarted`, `TurnCompleted`, `EffectResolved`, `TokensStreamed`, and others.

This WebSocket remains a run-event protocol and is not reused for durable
interaction delivery. Interaction clients use the separate checkpointed SSE
route documented above, or finite JSON replay when streaming is unsuitable.

```mermaid
stateDiagram-v2
    [*] --> created : POST /agents/:id/runs
    created --> queued : automatic
    queued --> running : automatic
    running --> awaiting_approval : effect needs approval
    awaiting_approval --> running : POST /effects/:id/approve
    running --> waiting_effect : effects in flight
    waiting_effect --> running : effects resolved
    running --> completing : final turn
    completing --> completed : artifacts finalized
    running --> failed : unrecoverable error
    running --> cancelled : POST /runs/:id/cancel
    running --> timed_out : deadline exceeded
    completed --> [*]
    failed --> [*]
    cancelled --> [*]
    timed_out --> [*]
```

---

## Authentication

When authentication is enabled, configure it in `polkagent.toml`:

```toml
[auth]
enabled = true
api_keys = ["hashed-key-digest"]
session_timeout_secs = 3600
```

Manage credentials with the `auth` subcommand:

```bash
polkagent auth login --provider anthropic
polkagent auth status
polkagent auth whoami
```

---

## Server configuration

```toml
[server]
bind_address = "127.0.0.1:9090"
cors_origins = ["*"]

[server.rate_limit]
enabled = true
requests_per_second = 100
burst = 200

[server.tls]
cert_path = "/path/to/cert.pem"
key_path = "/path/to/key.pem"
```

### Read-only mode

Start the server in read-only mode to reject all mutating requests (`POST`, `PUT`, `DELETE`):

```bash
polkagent serve --read-only
```

---

> For a hands-on API cookbook with curl examples, see [Examples: API Cookbook](examples.md#api-cookbook).

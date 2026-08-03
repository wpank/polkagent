# Harnesses

Harnesses are abstractions for wrapping and orchestrating external coding agents. Polkagent can delegate execution to various coding agent tools through a unified harness interface defined in `polkagent-harness-trait`.

```mermaid
graph LR
    subgraph Polkagent
        RM["Run Manager"]
        HT["Harness Trait"]
    end

    subgraph Harnesses
        HC["polkagent-harness-claude"]
        HX["polkagent-harness-codex"]
        HA["polkagent-harness-acp"]
    end

    subgraph External Agents
        CC["Claude Code"]
        CX["Codex CLI"]
        CU["Cursor"]
        CP["Copilot"]
        GO["Goose"]
        KI["Kiro"]
    end

    RM --> HT
    HT --> HC
    HT --> HX
    HT --> HA
    HC --> CC
    HX --> CX
    HA --> CU
    HA --> CP
    HA --> GO
    HA --> KI
```

## Transport Types

Each harness communicates with its underlying agent via one of the following transport flavors, defined in the `TransportFlavor` enum:

- `HttpApi` — HTTP-based API communication.
- `OneShotCli` — One-shot CLI execution. Supports the following output formats:
  - `StreamJson` — One event per line.
  - `JsonEnvelope` — Single JSON wrapper.
  - `NdJson` — NDJSON.
  - `PlainText` — Unstructured plain text.
- `JsonRpcStdio` — JSON-RPC 2.0 over stdin/stdout. Used by ACP-compatible agents.
- `WebSocket` — WebSocket-based communication.
- `McpServer` — Model Context Protocol server.

## Harness Capabilities

Each harness declares a set of capabilities that describe how it can be used:

| Capability | Options |
|------------|---------|
| `SessionResumeMode` | `None`, `ById`, `ByReplay` |
| `McpMode` | `None`, `Configurable`, `Passthrough` |
| `ToolInjection` | `None`, `McpConfig`, `CliFlags`, `ConfigFile` |
| `CancelMode` | `None`, `Signal`, `Api` |

## Supported Harnesses

| Harness | Crate | ID | Transport | Description |
|---------|-------|----|-----------|-------------|
| Claude Code | `polkagent-harness-claude` | `claude-code` | One-shot CLI (JSON lines over stdin/stdout) | Anthropic's Claude Code CLI. Uses SIGTERM for graceful shutdown. |
| Codex | `polkagent-harness-codex` | `codex` | CLI | OpenAI Codex CLI. |
| Cursor | `polkagent-harness-cursor` | `cursor` | JSON-RPC stdio (via ACP) | Cursor agent. |
| Copilot | `polkagent-harness-copilot` | `copilot` | CLI | GitHub Copilot agent. |
| Goose | `polkagent-harness-goose` | `goose` | JSON-RPC stdio (via ACP) | Block Inc's Goose agent. |
| Kiro | `polkagent-harness-kiro` | `kiro` | JSON-RPC stdio (via ACP) | Kiro agent. |

## ACP (Agent Client Protocol)

The `polkagent-harness-acp` crate provides a shared JSON-RPC 2.0 over stdio client used by the Cursor, Goose, and Kiro harnesses. The protocol flow is:

1. Send `initialize` request, receive capabilities response.
2. Send `initialized` notification.
3. Create sessions.
4. Send prompts.
5. Receive streaming notifications.

```mermaid
sequenceDiagram
    participant PA as Polkagent
    participant ACP as ACP Client
    participant AG as External Agent

    PA->>ACP: Start harness session
    ACP->>AG: initialize (JSON-RPC 2.0)
    AG-->>ACP: capabilities response
    ACP->>AG: initialized notification
    ACP->>AG: sessions/create
    AG-->>ACP: session_id
    ACP->>AG: prompts/send (with prompt)
    loop Streaming
        AG-->>ACP: notifications/progress
        AG-->>ACP: notifications/tool_call
        AG-->>ACP: notifications/completion
    end
    ACP-->>PA: Harness result
```

## Configuration

### Global Harness Config

```toml
[harness]
harness_type = "claude"        # Default harness
timeout_secs = 300             # Per-operation timeout
max_concurrent = 1             # Max concurrent harness operations
```

### Per-Harness Configuration

```toml
[harness.harnesses.claude-code]
binary_path = "claude"
transport = "stdio"
approval_mode = "auto"         # auto | manual | policy

[harness.harnesses.codex]
binary_path = "codex"
transport = "stdio"

[harness.harnesses.cursor]
transport = "stdio"
approval_mode = "policy"
```

### Overriding Harness Per Run

Pass `--harness` to override the configured default for a single run:

```bash
polkagent run -a my-agent -p "Fix the bug" --harness codex
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `POLKAGENT_HARNESS_TYPE` | Default harness type. |
| `POLKAGENT_HARNESS_TIMEOUT_SECS` | Per-operation timeout in seconds. |
| `POLKAGENT_HARNESS_MAX_CONCURRENT` | Maximum number of concurrent harness operations. |

## Harness Selection

```mermaid
flowchart TD
    A[Run Request] --> B{--harness flag?}
    B -->|Yes| C[Use specified harness]
    B -->|No| D{Agent has harness config?}
    D -->|Yes| E[Use agent's harness]
    D -->|No| F{Global harness.harness_type?}
    F -->|Yes| G[Use global default]
    F -->|No| H[Use built-in executor\nNo harness]

    C --> I{Harness binary found?}
    E --> I
    G --> I
    I -->|Yes| J[Initialize harness session]
    I -->|No| K[Error: harness not available]

    J --> L{Transport type?}
    L -->|OneShotCli| M[Spawn CLI process]
    L -->|JsonRpcStdio| N[Open stdin/stdout JSON-RPC]
    L -->|HttpApi| O[Connect HTTP endpoint]
    L -->|WebSocket| P[Connect WebSocket]
```

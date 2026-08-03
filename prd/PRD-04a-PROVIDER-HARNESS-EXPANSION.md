# PRD-04a — Provider, Executor, and Harness Expansion

**Status:** definitive PRD (draft)
**Date:** 2026-08-01
**Parent:** PRD-04 (Providers, Models, Harnesses, Tools and Skills)
**Audience:** engineers implementing the execution layer
**Scope:** expands PRD-04 with concrete provider catalog, harness implementations,
protocol specifications, tool name normalization, and configuration schemas
**Research basis:** Roko codebase analysis (32-crate workspace), Codex app-server
protocol (v2), ACP standard, and survey of 7 coding-agent CLIs

---

## 1. Purpose

PRD-04 defines the abstract taxonomy and trait boundaries for providers, executors,
harnesses, tools, and skills. This addendum expands it with:

1. A **concrete provider catalog** covering every provider Polkagent MUST support
2. A **built-in model registry** with compile-time capability metadata
3. **Harness implementations** for six coding-agent CLIs
4. **Protocol specifications** for ACP, Codex JSON-RPC, and Claude stream-json
5. **Tool name normalization** with per-backend alias maps
6. **Transport tier classification** for dispatch-time capability validation
7. **Configuration schema** for `polkagent.toml` provider/harness sections

### 1.1 Relationship to PRD-04

This document is an addendum, not a replacement. PRD-04's trait definitions,
lifecycle diagrams, and established principles remain authoritative. Where this
addendum specifies implementation details, PRD-04's abstract contracts govern.

### 1.2 Maturity labels

| Label | Meaning |
|---|---|
| **Phase 2** | Required for Phase 2 completion |
| **Phase 3** | Required for read-only value |
| **Phase 4** | Required for controlled write |
| **Deferred** | Committed but not yet scheduled |

---

## 2. Provider catalog

### 2.1 Provider kinds

Polkagent MUST support the following provider protocol families. Each maps to a
distinct executor adapter that translates between the vendor wire format and the
kernel's internal `InferenceRequest`/`InferenceResponse` schema.

| ProviderKind | Wire protocol | Auth | Phase |
|---|---|---|---|
| `anthropic_api` | Anthropic Messages API (HTTP+SSE) | `x-api-key` header | Phase 2 |
| `openai_compat` | OpenAI Chat Completions API (HTTP+SSE) | `Authorization: Bearer` | Phase 2 |
| `local` | OpenAI-compatible (HTTP, no auth) | None | Phase 2 |
| `gemini_api` | Google Gemini API (HTTP+SSE) | `x-goog-api-key` or OAuth | Phase 3 |
| `openrouter` | OpenRouter (OpenAI-compatible + routing headers) | `Authorization: Bearer` | Phase 3 |
| `bedrock` | AWS Bedrock (SigV4, HTTP) | AWS IAM credentials | Phase 4 |
| `azure_openai` | Azure OpenAI (HTTP+SSE, custom base URL) | API key or Azure AD | Phase 4 |
| `perplexity_api` | Perplexity Sonar (OpenAI-compatible + search extensions) | `Authorization: Bearer` | Deferred |
| `cerebras_api` | Cerebras Inference (OpenAI-compatible, ultra-fast) | `Authorization: Bearer` | Deferred |

**Design note (Established):** The `openai_compat` executor covers OpenAI, Azure
OpenAI, OpenRouter, vLLM, LM Studio, Together AI, Fireworks, Groq, Mistral, and
any other endpoint speaking the Chat Completions wire format. Dedicated provider
kinds (e.g., `openrouter`, `azure_openai`) exist only when the provider requires
additional wire-format modifications (routing headers, auth schemes) beyond what
`openai_compat` handles.

### 2.2 Standard provider synthesis

When no explicit `[[providers]]` block exists in configuration, Polkagent MUST
auto-synthesize providers from environment variables:

| Environment variable | Synthesized provider |
|---|---|
| `ANTHROPIC_API_KEY` | `anthropic_api` with default base URL |
| `OPENAI_API_KEY` | `openai_compat` with OpenAI base URL |
| `GEMINI_API_KEY` | `gemini_api` with default base URL |
| `OPENROUTER_API_KEY` | `openrouter` with OpenRouter base URL |
| `OLLAMA_URL` or `OLLAMA_MODEL` | `local` with Ollama base URL |
| `CODEX_API_KEY` | `openai_compat` with OpenAI base URL (for Codex models) |

The `POLKAGENT_`-prefixed variants (e.g., `POLKAGENT_ANTHROPIC_API_KEY`) take
precedence over unprefixed variants.

### 2.3 ProviderConfig schema

```rust
/// Configuration for a single AI provider.
pub struct ProviderConfig {
    /// Stable identifier (e.g., "anthropic", "openai", "local-ollama").
    pub id: ProviderId,
    /// Protocol family determining which executor adapter is used.
    pub kind: ProviderKind,
    /// API endpoint. Provider-specific defaults apply when absent.
    pub base_url: Option<String>,
    /// Name of environment variable holding the API key.
    /// Key is resolved at execution time, never embedded in config.
    pub api_key_env: Option<String>,
    /// Default model slug when none specified at request time.
    pub default_model: String,
    /// Per-request timeout.
    pub timeout: Duration,                    // Default: 120s
    /// Time-to-first-token timeout (distinct from full request timeout).
    pub ttft_timeout: Option<Duration>,       // Default: 15s
    /// TCP connection timeout.
    pub connect_timeout: Option<Duration>,    // Default: 5s
    /// Maximum automatic retries on transient failures.
    pub max_retries: u32,                     // Default: 3
    /// Maximum concurrent requests to this provider.
    pub max_concurrent: Option<u32>,          // Default: 10
    /// Extra HTTP headers injected on every request.
    pub extra_headers: HashMap<String, String>,
    /// Provider-specific extensions (e.g., OpenRouter routing config).
    pub extra: serde_json::Value,
}
```

**Invariant (Established):** API keys are **never stored in config files**. Config
files name the environment variable to read. Keys are resolved at execution time
by the executor adapter.

---

## 3. Model registry

### 3.1 Built-in models

Polkagent MUST ship a compile-time model registry with capability metadata for
routing and dispatch-time validation. The registry is advisory — users MAY
configure models not in the registry via `[[models]]` config blocks.

| Model slug | Provider | Context | Max output | Tools | Thinking | Vision | Phase |
|---|---|---|---|---|---|---|---|
| `claude-opus-4-6` | anthropic_api | 200k | 32k | Yes | Yes | Yes | Phase 2 |
| `claude-sonnet-4-6` | anthropic_api | 200k | 16.4k | Yes | Yes | Yes | Phase 2 |
| `claude-haiku-4-5` | anthropic_api | 200k | 8.2k | Yes | No | Yes | Phase 2 |
| `gpt-5.5` | openai_compat | 200k | 32.8k | Yes | Yes | Yes | Phase 3 |
| `gpt-5.4-mini` | openai_compat | 200k | 16.4k | Yes | No | Yes | Phase 3 |
| `o3` | openai_compat | 200k | 100k | Yes | Yes | Yes | Phase 3 |
| `o4-mini` | openai_compat | 200k | 100k | Yes | Yes | Yes | Phase 3 |
| `gpt-4o` | openai_compat | 128k | 16.4k | Yes | No | Yes | Phase 3 |
| `codex-mini` | openai_compat | 200k | 16.4k | Yes | Yes | No | Phase 3 |
| `gemini-2.5-pro` | gemini_api | 1M | 65.5k | Yes | Yes | Yes | Phase 3 |
| `gemini-2.5-flash` | gemini_api | 1M | 65.5k | Yes | Yes | Yes | Phase 3 |
| `sonar-pro` | perplexity_api | 200k | 8k | No | No | No | Deferred |
| `sonar` | perplexity_api | 128k | 8k | No | No | No | Deferred |

### 3.2 ModelDescriptor

```rust
/// Compile-time or user-configured model metadata.
pub struct ModelDescriptor {
    pub slug: String,                        // Wire ID sent to provider
    pub provider: ProviderId,                // Which provider serves this model
    pub context_window: u64,                 // Input tokens
    pub max_output: Option<u64>,             // Max generation tokens
    pub supports_tools: bool,
    pub supports_thinking: bool,             // Extended thinking / reasoning
    pub supports_vision: bool,
    pub supports_streaming: bool,            // Default: true
    pub supports_caching: bool,              // Provider-side context caching
    pub supports_structured_output: bool,
    pub supports_web_search: bool,
    pub tool_format: ToolFormat,             // Wire format for tool calls
    pub use_max_completion_tokens: bool,     // OpenAI models use this field
    pub cost_input_per_m: Option<f64>,       // USD per 1M input tokens
    pub cost_output_per_m: Option<f64>,      // USD per 1M output tokens
    pub cost_cache_read_per_m: Option<f64>,
    pub cost_cache_write_per_m: Option<f64>,
}
```

### 3.3 ToolFormat enum

Different model families require different wire representations for tool calls.
Polkagent MUST select the correct format based on model metadata.

| ToolFormat | Used by | Description |
|---|---|---|
| `AnthropicBlocks` | Claude models | `tool_use` content blocks in Messages API |
| `OpenAiJson` | GPT, o-series, Codex | `tools` array with JSON Schema in Chat Completions |
| `GeminiNative` | Gemini models | Native Gemini tool calling |
| `ReActText` | Local/unknown models | ReAct-style text parsing (fallback) |

### 3.4 ModelCatalog trait

```rust
#[async_trait]
pub trait ModelCatalog: Send + Sync {
    /// List models matching optional filter.
    async fn list(&self, filter: &ModelFilter) -> Vec<ModelDescriptor>;
    /// Get a specific model by slug.
    async fn get(&self, slug: &str) -> Option<ModelDescriptor>;
    /// Find models with required capabilities.
    async fn find_capable(&self, required: &ModelCapabilities) -> Vec<ModelDescriptor>;
    /// Refresh from remote (for dynamic catalogs).
    async fn refresh(&self) -> Result<(), CatalogError>;
}
```

---

## 4. Executor expansion

### 4.1 Current state

Polkagent has four executor implementations (PRD-04 § 4):

| Crate | ProviderKind | Status |
|---|---|---|
| `polkagent-executor-anthropic` | `anthropic_api` | Done (60 tests) |
| `polkagent-executor-openai` | `openai_compat` | Done (57 tests) |
| `polkagent-executor-local` | `local` | Done (42 tests) |
| `polkagent-executor-fake` | (testing) | Done (19 tests) |

### 4.2 New executors required

| Crate | ProviderKind | Phase | Notes |
|---|---|---|---|
| `polkagent-executor-gemini` | `gemini_api` | Phase 3 | Native Gemini tool format, thinking levels, 1M context |
| `polkagent-executor-openrouter` | `openrouter` | Phase 3 | Extends `openai_compat` with routing headers, provider order |

**Design note:** `bedrock` and `azure_openai` are deferred. When implemented, they
extend `openai_compat` or `anthropic_api` with modified auth and base URLs.

### 4.3 OpenAI-specific: `max_completion_tokens`

OpenAI reasoning models (o3, o4-mini, gpt-5.x, codex-mini) use
`max_completion_tokens` instead of `max_tokens` in the request body. The
`openai_compat` executor MUST check `ModelDescriptor::use_max_completion_tokens`
and emit the correct field. The current implementation uses `max_tokens`
unconditionally and MUST be updated.

### 4.4 Provider concurrency control

Each provider MUST enforce `max_concurrent` via a semaphore acquired before each
model call and held for the request duration. This prevents provider overload and
enables backpressure. The current implementations do not enforce concurrency
limits and MUST be updated.

### 4.5 Provider error classification

All executors MUST classify HTTP errors into actionable categories:

| Category | HTTP codes | Action |
|---|---|---|
| `RateLimit` | 429 | Wait `retry-after`, then retry |
| `AuthFailure` | 401, 403 | Skip (credentials issue) |
| `Timeout` | 408, gateway timeouts | Retry with fallback |
| `ServerError` | 500-599 | Retry with backoff |
| `ContentPolicy` | 400 (content filter) | Skip |
| `ContextOverflow` | 400 (context exceeded) | Truncate and retry |
| `ModelNotFound` | 404 | Skip |

---

## 5. Harness system expansion

### 5.1 Design philosophy (Established)

A **harness** wraps an external coding agent as a subprocess or service. The
harness is NOT an executor — it has its own session lifecycle, internal tool
ecosystem, workspace binding, and health management. Polkagent records what
the harness reports but does not re-implement the harness's internal tool
behavior (PRD-04 § 5.2.1).

### 5.2 Harness trait (current)

The existing `polkagent-harness-trait` defines:

```rust
#[async_trait]
pub trait Harness: Send + Sync + 'static {
    fn id(&self) -> &HarnessId;
    fn capabilities(&self) -> HarnessCapabilities;
    fn status(&self) -> HarnessStatus;
    async fn start_session(&self, config: SessionConfig) -> Result<SessionId, HarnessError>;
    async fn send_message(&self, session_id: SessionId, message: &str) -> Result<(), HarnessError>;
    async fn receive_events(&self, session_id: SessionId)
        -> Result<Pin<Box<dyn Stream<Item = HarnessEvent> + Send>>, HarnessError>;
    async fn end_session(&self, session_id: SessionId) -> Result<(), HarnessError>;
    async fn health(&self) -> Result<bool, HarnessError>;
}
```

This trait is sufficient for one-shot subprocess harnesses but lacks support for:
- Transport tier selection (stdio vs HTTP vs ACP)
- Capability negotiation and dispatch-time validation
- Daemon lifecycle management (start/stop/probe)
- Protocol-specific event parsing (stream-json, JSON-RPC, ACP)
- Approval flow integration (Codex/Cursor/Copilot approval protocols)

### 5.3 Proposed extensions

#### 5.3.1 TransportFlavor enum

Classifies how Polkagent communicates with a harness:

```rust
pub enum TransportFlavor {
    /// Tier 1: HTTP-based (OpenAI-compatible, Responses API)
    HttpApi,
    /// Tier 2: One-shot CLI invocation with structured output
    OneShotCli { output_format: CliOutputFormat },
    /// Tier 3: Persistent JSON-RPC over stdio (ACP, Codex app-server)
    JsonRpcStdio,
    /// Tier 4: Persistent WebSocket
    WebSocket,
    /// Tier 5: MCP server mode
    McpServer,
}

pub enum CliOutputFormat {
    /// Claude CLI `--output-format stream-json`
    StreamJson,
    /// JSON envelope on stdout
    JsonEnvelope,
    /// Newline-delimited JSON
    NdJson,
    /// Plain text (parse as single message)
    PlainText,
}
```

#### 5.3.2 HarnessCapabilities expansion

```rust
pub struct HarnessCapabilities {
    // --- Existing fields ---
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_sessions: bool,
    pub max_context_tokens: u32,
    pub models: Vec<String>,

    // --- New fields ---
    /// Transport used for communication.
    pub transport: TransportFlavor,
    /// Can the caller override the model per request?
    pub model_override: bool,
    /// Can sessions be resumed by ID?
    pub session_resume: SessionResumeMode,
    /// How MCP servers are made available to the harness.
    pub mcp_passthrough: McpMode,
    /// How polkagent-side tools are injected into the harness.
    pub tool_injection: ToolInjection,
    /// Mid-turn cancellation support.
    pub cancel: CancelMode,
    /// Safe for multiple concurrent sessions?
    pub multiplex_safe: bool,
}

pub enum SessionResumeMode {
    /// CLI flag (e.g., `--resume`, `--continue`)
    CliFlag(&'static str),
    /// ACP `session/load` request
    AcpSessionLoad,
    /// Codex `thread/resume` request
    CodexThreadResume,
    /// Not supported
    None,
}

pub enum McpMode {
    /// MCP servers passed per-call in tools array
    PerCall,
    /// MCP from config file via CLI flag
    ConfigFile(&'static str),
    /// Server-side only, cannot inject
    ServerOnly,
    /// No MCP support
    None,
}

pub enum ToolInjection {
    /// Tools in per-request body (OpenAI `tools` array)
    PerCallTools,
    /// Tools defined in config file ahead of time
    ConfigFile,
    /// Tools come from MCP only
    McpOnly,
    /// Harness has opaque internal tools, cannot inject
    Opaque,
}

pub enum CancelMode {
    /// Send SIGTERM to subprocess
    KillChild,
    /// ACP `session/cancel` request
    AcpCancel,
    /// Codex `turn/interrupt` request
    CodexTurnInterrupt,
    /// Not cancellable
    None,
}
```

#### 5.3.3 Dispatch-time validation

```rust
pub struct HarnessTaskRequirements {
    pub needs_tools: bool,
    pub needs_streaming: bool,
    pub needs_mcp: bool,
    pub needs_session_resume: bool,
    pub needs_cancel: bool,
}

/// Validate that an adapter can serve a task's requirements.
/// Returns Err with a human-readable mismatch if not.
pub fn validate_for_task(
    capabilities: &HarnessCapabilities,
    requirements: &HarnessTaskRequirements,
) -> Result<(), CapabilityMismatch>;
```

#### 5.3.4 EventParser trait

Normalizes harness-specific event streams into `HarnessEvent`:

```rust
pub trait EventParser: Send {
    /// Parse one line of stdout into zero or more normalized events.
    fn parse_stdout_line(&mut self, line: &str) -> Vec<HarnessEvent>;
    /// Parse one line of stderr. Default: emit as Error event.
    fn parse_stderr_line(&mut self, line: &str) -> Vec<HarnessEvent> {
        vec![HarnessEvent::Error { session_id: SessionId::default(), message: line.to_string() }]
    }
    /// Called after subprocess exits to flush buffered state.
    fn finalize(&mut self) -> Vec<HarnessEvent> { vec![] }
}
```

#### 5.3.5 HarnessService trait (daemon lifecycle)

For harnesses that run as persistent daemons (Codex app-server, Goose serve):

```rust
#[async_trait]
pub trait HarnessService: Send + Sync {
    fn service_name(&self) -> &str;
    async fn start(&self) -> Result<(), HarnessError>;
    async fn stop(&self) -> Result<(), HarnessError>;
    async fn status(&self) -> ServiceStatus;
    async fn healthcheck(&self) -> Result<(), HarnessError>;
    fn pid(&self) -> Option<u32>;
}

pub enum ServiceStatus {
    Running,
    Stopped,
    Starting,
    Unknown,
}
```

### 5.4 Coding agent harness catalog

Based on research of the Roko codebase and a survey of 7 coding-agent CLIs,
Polkagent MUST support the following harnesses:

#### 5.4.1 Claude Code (`polkagent-harness-claude`) — DONE

| Property | Value |
|---|---|
| Binary | `claude` |
| Transport | Tier 2: `OneShotCli { StreamJson }` |
| Spawning | `claude --print --output-format stream-json --model <model> [-p "prompt"]` |
| Protocol | Newline-delimited stream-json events on stdout |
| Event types | `system`, `assistant`, `content_block_start`, `content_block_delta`, `tool`, `result`, `error` |
| Session resume | `--resume <session_id>` or `--continue` |
| MCP | `--mcp-config <path>` flag |
| Tool injection | `--tools <csv>` flag |
| Cancel | SIGTERM → SIGKILL escalation |
| Auth | Inherits `ANTHROPIC_API_KEY` from environment |
| Phase | Phase 2 (done) |

**Parser: `ClaudeStreamJsonParser`**

Maps Claude stream-json events to `HarnessEvent`:

| Stream-JSON type | HarnessEvent |
|---|---|
| `system` | `SessionStarted` (extract session_id, model) |
| `assistant` | `MessageReceived` (text blocks), `ToolCallRequested` (tool_use blocks) |
| `content_block_delta` | `MessageReceived` (text deltas) |
| `tool` (subtype=result) | `ToolResultProvided` |
| `result` | Usage tracking + stop reason |
| `error` | `Error` |

**Stderr handling:** Claude emits events on both stdout and stderr. Parser MUST
try stream-json parsing on stderr lines before classifying as errors. Known
benign stderr patterns (startup messages, warnings) MUST be suppressed.

#### 5.4.2 Codex CLI (`polkagent-harness-codex`) — PHASE 3

| Property | Value |
|---|---|
| Binary | `codex` |
| Transport | Tier 3: `JsonRpcStdio` |
| Spawning | `codex app-server --listen stdio://` |
| Protocol | Bidirectional JSON-RPC 2.0 over JSONL (NO `"jsonrpc":"2.0"` field on wire) |
| Handshake | `initialize` request → response → `initialized` notification |
| Session | `thread/start` → `turn/start` → streaming notifications → `turn/completed` |
| Event types | `turn/started`, `item/started`, `item/agentMessage/delta`, `item/completed`, `turn/completed` |
| Approval | Server sends `commandExecution/requestApproval`, `fileChange/requestApproval`; client responds with decision |
| Session resume | `thread/resume { threadId }` |
| MCP | Configuration via `config/batchWrite` |
| Tool injection | Not directly injectable; Codex manages its own tools |
| Cancel | `turn/interrupt` request |
| Auth | `OPENAI_API_KEY` or `CODEX_API_KEY` |
| Phase | Phase 3 |

**Implementation notes:**

1. **AcpStdioClient pattern:** Spawn `codex app-server` as persistent subprocess.
   Maintain pending request map (`HashMap<u64, oneshot::Sender>`) for
   request/response correlation. Route responses by `id`, notifications by `method`.

2. **Initialize handshake:** MUST complete before any other method. Send:
   ```json
   { "id": 1, "method": "initialize", "params": {
     "clientInfo": { "name": "polkagent", "version": "0.1.0" },
     "capabilities": {}
   }}
   ```
   Wait for response, then send `{ "method": "initialized" }`.

3. **Turn lifecycle:** Map Polkagent `send_message()` to `turn/start`:
   ```json
   { "id": 2, "method": "turn/start", "params": {
     "threadId": "<thread_id>",
     "userInput": [{ "type": "text", "text": "<message>" }]
   }}
   ```
   Collect `item/agentMessage/delta` notifications into `MessageReceived` events.
   `turn/completed` signals turn end with usage data.

4. **Approval flow:** When server sends `commandExecution/requestApproval` (as a
   server request with `id`), the harness MUST either auto-approve or route to the
   Polkagent grant system for human approval, then respond:
   ```json
   { "id": <server_request_id>, "result": { "decision": "accept" } }
   ```

5. **Backpressure:** If server returns error code `-32001`, implement exponential
   backoff before retrying.

6. **Stderr suppression:** Codex emits 25+ known benign stderr patterns. The
   harness MUST classify and suppress these (apply_patch verification, state DB
   migration, shell snapshot, thread shutdown timeout, etc.).

#### 5.4.3 Cursor CLI (`polkagent-harness-cursor`) — PHASE 3

| Property | Value |
|---|---|
| Binary | `cursor` |
| Transport | Tier 3: `JsonRpcStdio` (ACP mode) |
| Spawning | `cursor agent acp` |
| Protocol | ACP JSON-RPC 2.0 over stdio |
| Handshake | `initialize` → response → `initialized` |
| Session | `session/new` → `session/prompt` → `session/update` notifications |
| Event types | `AgentMessageChunk`, `ToolCall`, `ToolCallUpdate`, `TurnEnd` |
| Approval | `session/request_permission` → client responds |
| Session resume | `session/load { session_key }` |
| MCP | Via `session/new { mcp_servers }` |
| Cancel | `session/cancel` |
| Auth | `CURSOR_API_KEY` or `cursor agent login` |
| Phase | Phase 3 |

**Design note:** Cursor also supports a simpler headless mode
(`cursor agent -p "prompt" --output-format stream-json`) which outputs Claude-
compatible stream-json. The ACP mode is preferred for full integration.

#### 5.4.4 GitHub Copilot CLI (`polkagent-harness-copilot`) — PHASE 4

| Property | Value |
|---|---|
| Binary | `github-copilot` (or `copilot`) |
| Transport | Tier 2: `OneShotCli { JsonEnvelope }` |
| Spawning | `github-copilot -p "prompt" --output-format=json` |
| Protocol | JSONL event stream on stdout |
| Event types | `assistant.*`, `tool.*`, `session.*`, `permission.*` |
| Approval | `--allow-tool=PATTERN`, `--deny-tool=PATTERN`, `permission.requested` events |
| Session resume | Via `events.jsonl` replay |
| Tool injection | Not directly injectable |
| Cancel | SIGTERM |
| Auth | GitHub OAuth or PAT |
| Phase | Phase 4 |

**Alternate path:** The Copilot SDK (technical preview) provides a richer JSON-RPC
interface. When the SDK reaches GA, Polkagent SHOULD upgrade to Tier 3.

#### 5.4.5 Goose (`polkagent-harness-goose`) — PHASE 4

| Property | Value |
|---|---|
| Binary | `goose` |
| Transport | Tier 3: `JsonRpcStdio` (ACP mode) |
| Spawning | `goose acp` |
| Protocol | ACP JSON-RPC 2.0 over stdio |
| Alt transport | HTTP/WebSocket via `goose serve` on configurable port |
| Approval | 4-tier: auto, approve, smart_approve, chat |
| Session resume | `--resume [name/id]` |
| Cancel | ACP `session/cancel` |
| Auth | `OPENAI_API_KEY` (default), configurable |
| Phase | Phase 4 |

**Design note:** Goose is Rust-based and open-source (Apache 2.0), making it the
highest-affinity harness for deep integration. The `goose serve` HTTP endpoint
enables remote/distributed agent orchestration.

#### 5.4.6 Kiro CLI (`polkagent-harness-kiro`) — DEFERRED

| Property | Value |
|---|---|
| Binary | `kiro-cli` |
| Transport | Tier 3: `JsonRpcStdio` (ACP mode) |
| Spawning | `kiro-cli acp` |
| Protocol | ACP JSON-RPC 2.0 over stdio |
| Session | `session/new` → `session/prompt` → notifications |
| Auth | `KIRO_API_KEY` |
| Phase | Deferred |

### 5.5 ACP (Agent Client Protocol) shared client

Three of the six harnesses (Cursor, Goose, Kiro) use the same ACP standard.
Rather than implementing the protocol three times, Polkagent MUST provide a
shared ACP client:

```rust
/// Shared JSON-RPC 2.0 client over stdio for ACP-compatible harnesses.
pub struct AcpStdioClient {
    config: AcpConfig,
    child: Option<Child>,
    stdin: Option<BufWriter<ChildStdin>>,
    next_id: AtomicU64,
    response_rx: Option<mpsc::UnboundedReceiver<(u64, Value)>>,
    notification_tx: mpsc::UnboundedSender<AcpNotification>,
    session_id: Option<String>,
}

pub struct AcpConfig {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub protocol_version: String,
    pub timeout: Duration,
}

impl AcpStdioClient {
    /// Spawn subprocess and complete initialize handshake.
    pub async fn connect(&mut self) -> Result<AcpInitResponse, AcpError>;
    /// Create a new session.
    pub async fn new_session(&mut self, opts: NewSessionOpts) -> Result<String, AcpError>;
    /// Send a prompt and get streaming notifications.
    pub async fn send_prompt(&mut self, session: &str, text: &str) -> Result<u64, AcpError>;
    /// Cancel the active turn.
    pub async fn cancel(&mut self, session: &str) -> Result<(), AcpError>;
    /// Load (resume) a previous session.
    pub async fn load_session(&mut self, session: &str) -> Result<(), AcpError>;
    /// Graceful shutdown.
    pub async fn shutdown(&mut self) -> Result<(), AcpError>;
    /// Take the notification receiver for streaming.
    pub fn take_notification_rx(&mut self) -> Option<mpsc::UnboundedReceiver<AcpNotification>>;
}
```

**Convenience constructors:**

```rust
impl AcpConfig {
    pub fn cursor(binary: &str, cwd: &Path) -> Self;
    pub fn goose(binary: &str, cwd: &Path) -> Self;
    pub fn kiro(binary: &str, cwd: &Path) -> Self;
}
```

### 5.6 Harness not supported (with rationale)

| Agent | Reason |
|---|---|
| **Aider** | No structured output, no event stream, no tool call visibility. Plain-text-only subprocess. |
| **Windsurf/Devin Desktop** | No CLI or headless mode. IDE-only. |
| **Continue.dev** | Alpha status, no streaming events, minimal tool visibility. |

---

## 6. Process lifecycle management

### 6.1 ChildProcessRunner

Shared subprocess lifecycle manager for one-shot and persistent harnesses:

```rust
pub struct ChildProcessRunner {
    program: PathBuf,
    current_dir: PathBuf,
    timeout: Duration,
    env_inject: Vec<(String, String)>,
    env_remove: Vec<String>,
    name: String,
}

impl ChildProcessRunner {
    pub fn new(program: impl AsRef<OsStr>, current_dir: impl AsRef<Path>) -> Self;
    pub fn with_timeout(self, timeout: Duration) -> Self;
    pub fn with_env(self, key: &str, value: &str) -> Self;
    pub fn without_env(self, key: &str) -> Self;

    /// Run a one-shot subprocess, streaming output through an EventParser.
    pub async fn run_one_shot(
        &self,
        args: &[&str],
        stdin_data: Option<&[u8]>,
        parser: &mut dyn EventParser,
    ) -> Result<Vec<HarnessEvent>, HarnessError>;

    /// Spawn a persistent subprocess (caller owns child and drives I/O).
    pub fn spawn_persistent(&self, args: &[&str]) -> Result<SpawnedChild, HarnessError>;
}
```

### 6.2 Graceful termination

Process tree termination MUST follow this escalation sequence:

1. Close stdin (EOF signal) — wait 1200ms
2. SIGTERM to process group — wait 800ms
3. SIGKILL if still alive

```rust
pub async fn kill_tree(child: &mut Child, grace_period: Duration) -> Result<(), io::Error>;
```

### 6.3 Environment scrubbing

When spawning harness subprocesses, Polkagent MUST:

- Inject required env vars (API keys, config paths)
- Remove vars that cause nested-session detection (e.g., `CLAUDECODE`)
- Never expose `POLKAGENT_*` internal state to child processes

---

## 7. Tool name normalization

### 7.1 Canonical tool names

Polkagent uses `snake_case` canonical names as the single source of truth for
built-in tools. All backend-specific names are translated to/from canonical
names at the harness boundary.

| Canonical | Claude CLI | Codex | Copilot CLI | Category |
|---|---|---|---|---|
| `read_file` | `Read` | `codex_read_file` | `read` | Read |
| `write_file` | `Write` | `codex_write_file` | `write` | Write |
| `edit_file` | `Edit` | `codex_edit_file` | `edit` | Write |
| `glob` | `Glob` | — | `grep` | Read |
| `grep` | `Grep` | — | `grep` | Read |
| `bash` | `Bash` | `codex_bash` | `shell` | Exec |
| `web_fetch` | `WebFetch` | — | `url` | Network |
| `web_search` | `WebSearch` | — | — | Network |
| `task` | `Agent` | — | — | Meta |
| `notebook_edit` | `NotebookEdit` | — | — | Notebook |
| `apply_patch` | — | `codex_apply_patch` | — | Write |

### 7.2 Alias resolution functions

```rust
/// Normalize a Claude CLI tool name to canonical form.
pub fn canonical_of_claude(claude_name: &str) -> Option<&'static str>;

/// Map canonical name to Claude CLI name.
pub fn claude_of_canonical(canonical: &str) -> Option<&'static str>;

/// Normalize a Codex tool name to canonical form.
pub fn canonical_of_codex(codex_name: &str) -> Option<&'static str>;
```

### 7.3 Design principle (Established)

Coding-agent CLIs have opaque internal tool ecosystems. Polkagent records tool
call events from harnesses for observability but does NOT attempt to re-implement
or override harness-internal tool behavior. The alias map is for event
normalization and auditing, not for tool injection.

---

## 8. Configuration schema

### 8.1 Provider configuration (`polkagent.toml`)

```toml
# --- Provider definitions ---
# When absent, auto-synthesized from environment variables.

[[providers]]
id = "anthropic"
kind = "anthropic_api"
api_key_env = "ANTHROPIC_API_KEY"
default_model = "claude-sonnet-4-6"
timeout_secs = 120
max_retries = 3
max_concurrent = 10

[[providers]]
id = "openai"
kind = "openai_compat"
api_key_env = "OPENAI_API_KEY"
default_model = "gpt-5.4-mini"
timeout_secs = 120

[[providers]]
id = "openrouter"
kind = "openai_compat"
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
default_model = "claude-sonnet-4-6"
[providers.extra]
sort = "price"

[[providers]]
id = "local-ollama"
kind = "local"
base_url = "http://localhost:11434/v1"
default_model = "llama3.2"
max_retries = 0

[[providers]]
id = "gemini"
kind = "gemini_api"
api_key_env = "GEMINI_API_KEY"
default_model = "gemini-2.5-flash"
```

### 8.2 Model overrides (`polkagent.toml`)

```toml
# --- Custom model definitions ---
# Override or supplement the built-in registry.

[[models]]
slug = "deepseek-coder-v3"
provider = "openrouter"
context_window = 128000
max_output = 16384
supports_tools = true
supports_thinking = false
tool_format = "openai_json"
cost_input_per_m = 0.14
cost_output_per_m = 0.28
```

### 8.3 Harness configuration (`polkagent.toml`)

```toml
# --- Harness definitions ---

[harness]
# Default harness when `polkagent run` is invoked.
default = "claude-code"
timeout_secs = 300
max_concurrent = 1

[harness.claude-code]
binary_path = "claude"                    # Default: "claude"
# working_dir = "/path/to/workspace"     # Optional override

[harness.codex]
binary_path = "codex"
# Codex-specific: approval mode
approval_mode = "auto"                    # "auto" | "prompt" | "deny"

[harness.cursor]
binary_path = "cursor"
transport = "acp"                         # "acp" | "headless"

[harness.goose]
binary_path = "goose"
transport = "acp"                         # "acp" | "http"
# http_port = 6677                        # For HTTP transport
```

### 8.4 Environment variable overrides

| Variable | Override target |
|---|---|
| `POLKAGENT_HARNESS_TYPE` | `harness.default` |
| `POLKAGENT_HARNESS_TIMEOUT_SECS` | `harness.timeout_secs` |
| `POLKAGENT_HARNESS_MAX_CONCURRENT` | `harness.max_concurrent` |
| `POLKAGENT_HARNESS_BINARY_<NAME>` | `harness.<name>.binary_path` |

---

## 9. Implementation crate plan

### 9.1 New crates

| Crate | Phase | Description |
|---|---|---|
| `polkagent-harness-codex` | Phase 3 | Codex app-server JSON-RPC harness |
| `polkagent-harness-acp` | Phase 3 | Shared ACP client (used by cursor, goose, kiro) |
| `polkagent-harness-cursor` | Phase 3 | Cursor ACP harness (thin wrapper over acp client) |
| `polkagent-executor-gemini` | Phase 3 | Gemini API executor |
| `polkagent-harness-copilot` | Phase 4 | Copilot CLI one-shot harness |
| `polkagent-harness-goose` | Phase 4 | Goose ACP harness (thin wrapper over acp client) |
| `polkagent-harness-kiro` | Deferred | Kiro ACP harness |

### 9.2 Modified crates

| Crate | Changes |
|---|---|
| `polkagent-harness-trait` | Add `TransportFlavor`, expanded `HarnessCapabilities`, `EventParser`, `HarnessService`, `ChildProcessRunner`, `validate_for_task()` |
| `polkagent-executor-openai` | Add `max_completion_tokens` support, provider concurrency semaphore |
| `polkagent-executor-anthropic` | Add provider concurrency semaphore |
| `polkagent-executor-local` | Add provider concurrency semaphore |
| `polkagent-config` | Add `[[providers]]`, `[[models]]`, `[harness]` config sections |
| `polkagent-cli` | Update `run` command for harness selection, `doctor` for harness probing, `auth` for multi-provider keys |
| `polkagent-service` | Add `ProviderRegistry`, `HarnessRegistry`, dispatch-time provider/harness selection |

### 9.3 Dependency graph

```
polkagent-harness-trait
  ├── polkagent-harness-claude  (done)
  ├── polkagent-harness-codex   (new, phase 3)
  ├── polkagent-harness-acp     (new, shared client)
  │   ├── polkagent-harness-cursor (new, thin wrapper)
  │   ├── polkagent-harness-goose  (new, thin wrapper)
  │   └── polkagent-harness-kiro   (deferred)
  └── polkagent-harness-copilot (new, phase 4)

polkagent-executor-trait
  ├── polkagent-executor-anthropic (done)
  ├── polkagent-executor-openai    (done, needs update)
  ├── polkagent-executor-local     (done, needs update)
  ├── polkagent-executor-gemini    (new, phase 3)
  └── polkagent-executor-fake      (done)
```

---

## 10. Doctor command expansion

The `polkagent doctor` command MUST probe all configured harnesses and providers:

### 10.1 Provider probes

| Check | Method | Pass criteria |
|---|---|---|
| API key present | Read env var named in `api_key_env` | Non-empty |
| Endpoint reachable | Provider-specific health endpoint | HTTP 200 |
| Model available | `GET /models` or equivalent | Listed |

### 10.2 Harness probes

| Check | Method | Pass criteria |
|---|---|---|
| Binary on PATH | `which <binary>` | Found |
| Version compatible | `<binary> --version` | Parses cleanly |
| Auth valid | Provider-specific (e.g., `OPENAI_API_KEY` for Codex) | Non-empty |
| Daemon running | Health endpoint (if daemon-mode harness) | HTTP 200 |

### 10.3 Output format

```
Polkagent Doctor
--------------------------------------------------
  Providers
    ◉ [OK  ] anthropic: claude-sonnet-4-6 (api.anthropic.com)
    ◉ [OK  ] openai: gpt-5.4-mini (api.openai.com)
    ■ [FAIL] gemini: GEMINI_API_KEY not set
    ◉ [OK  ] local-ollama: llama3.2 (localhost:11434)

  Harnesses
    ◉ [OK  ] claude-code: claude v1.0.24 (/usr/local/bin/claude)
    ◉ [OK  ] codex: codex v0.120.1 (/usr/local/bin/codex)
    ■ [FAIL] cursor: binary not found
    ■ [SKIP] goose: not configured

  4/6 provider checks passed.
  2/4 harness checks passed.
```

---

## 11. CLI executor selection update

The current `polkagent run` command uses a hardcoded priority order to detect
executors from environment variables. This MUST be replaced with a registry-based
approach:

### 11.1 Resolution order

1. **CLI flag:** `polkagent run --provider anthropic --model claude-opus-4-6`
2. **Config default:** `[execution] default_provider` in `polkagent.toml`
3. **Environment detection:** First available from synthesized providers
4. **Fallback:** `FakeExecutor` with warning message

### 11.2 Harness selection

1. **CLI flag:** `polkagent run --harness codex`
2. **Config default:** `[harness] default` in `polkagent.toml`
3. **First available:** Probe harnesses in order: claude-code, codex, cursor, goose
4. **Fallback:** No harness (executor-only mode)

---

## 12. Acceptance criteria

### Phase 3

- [ ] AC-04a-001: Codex harness completes initialize handshake and processes one turn
- [ ] AC-04a-002: Codex harness approval flow routes to grant system
- [ ] AC-04a-003: Cursor harness creates ACP session and receives streaming events
- [ ] AC-04a-004: ACP shared client works with at least two harnesses (Cursor + one other)
- [ ] AC-04a-005: Gemini executor passes ModelExecutor contract tests
- [ ] AC-04a-006: `polkagent doctor` probes all configured providers and harnesses
- [ ] AC-04a-007: `polkagent run --provider` selects executor from registry
- [ ] AC-04a-008: `polkagent run --harness` selects harness from registry
- [ ] AC-04a-009: OpenAI executor sends `max_completion_tokens` for reasoning models
- [ ] AC-04a-010: Provider concurrency enforced via semaphore

### Phase 4

- [ ] AC-04a-011: Copilot CLI harness processes one-shot prompts with JSON output
- [ ] AC-04a-012: Goose ACP harness creates session and processes turn
- [ ] AC-04a-013: HarnessTaskRequirements validation rejects incompatible adapter/task pairs

---

## Appendix A: ACP protocol summary

The Agent Client Protocol (ACP) is an emerging standard for coding-agent
subprocess communication, adopted by Cursor, Goose, and Kiro. Key properties:

- **Wire format:** JSON-RPC 2.0 over JSONL/stdio
- **Handshake:** `initialize` → response → `initialized` notification
- **Session lifecycle:** `session/new` → `session/prompt` → streaming `session/update` notifications → `session/cancel`
- **Permission flow:** `session/request_permission` → client response
- **Session resume:** `session/load { session_key }`
- **Spec version:** 0.12.2 (as of 2026-08)
- **SDKs:** Rust, TypeScript, Python, Java, Kotlin

Polkagent's `polkagent-harness-acp` crate implements the ACP client side,
providing a shared foundation for all ACP-compatible harnesses.

---

## Appendix B: Codex app-server protocol summary

The Codex app-server exposes a bidirectional JSON-RPC 2.0 interface. Key
differences from ACP:

| Feature | ACP | Codex |
|---|---|---|
| Primitives | Session/Prompt | Thread/Turn/Item |
| Persistence | Session key | Thread ID (persisted to disk) |
| `"jsonrpc":"2.0"` field | Required | NOT sent or expected |
| Approval | `session/request_permission` | `commandExecution/requestApproval`, `fileChange/requestApproval` |
| Cancel | `session/cancel` | `turn/interrupt` |
| MCP | Via `session/new { mcp_servers }` | Via `config/batchWrite` |
| Session resume | `session/load` | `thread/resume` |
| Filesystem ops | Not in protocol | `fs/readFile`, `fs/writeFile`, `fs/watch` |
| Realtime audio | Not in protocol | `thread/realtime/*` |
| History | Not in protocol | `thread/list`, `thread/items/list`, `thread/compact` |

Polkagent's `polkagent-harness-codex` crate implements the Codex-specific
protocol, which is NOT ACP-compatible despite both using JSON-RPC.

---

## Appendix C: Research sources

| Source | Contribution |
|---|---|
| Roko codebase (`/Users/will/dev/nunchi/roko/roko/`) | Harness architecture, ACP client, Claude parser, Codex integration, provider system, tool aliases, configuration schemas |
| Codex app-server (`github.com/openai/codex/codex-rs/app-server`) | Complete JSON-RPC protocol specification |
| ACP specification (`agentclientprotocol.com`) | Standard protocol for coding-agent communication |
| Cursor CLI docs (`cursor.com/docs/cli`) | ACP and headless mode specifications |
| Goose docs (`goose-docs.ai`) | ACP, HTTP, and permission model |
| Copilot CLI docs (`docs.github.com/copilot`) | Event schema, programmatic reference |
| Kiro CLI docs (`kiro.dev/docs/cli`) | ACP mode specification |
| Aider docs (`aider.chat/docs`) | Subprocess limitations |

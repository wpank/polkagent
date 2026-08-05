# PRD-04 — Providers, Models, Harnesses, Tools and Skills

> **Implementation note (audited 2026-08-05):** This PRD remains normative,
> but its embedded implementation statements and checklists are not current
> status evidence. Use [STATUS.md](STATUS.md) and
> [IMPLEMENTATION-BACKLOG.md](IMPLEMENTATION-BACKLOG.md) for verified state and
> the dependency-ordered execution queue.

**Status:** definitive PRD (draft)
**Date:** 2026-07-30
**Audience:** engineers, architects, and product designers building or integrating
with Polkagent; no prior familiarity with Roko, PCA, or Polkadot is assumed
**Scope:** the complete taxonomy, contracts, lifecycle, and conformance model for
how Polkagent connects to AI models, manages long-lived coding sessions, exposes
typed tools, and packages reusable skills
**Authority:** `01-ESTABLISHED-BASELINE.md` remains authoritative where this PRD
conflicts with it
**Implementation status:** design direction; nothing described here is deployed
or production-validated

---

## 1. Purpose and orientation

Polkagent is a proposed Rust-first, Polkadot-native agent platform with three
equal pillars: **Build** (create/test/deploy Polkadot products), **Act**
(safely prepare and execute on-chain actions), and **Reach** (preserve the
chat/mobile/coding-agent experience of `polkadot-chat-agents`). These pillars
share a small durable kernel of runs, effects, artifacts, grants, and events.

This PRD specifies the **execution layer** that sits between the kernel and the
outside world: where model inference happens, how long-lived coding sessions
are managed, what tools an agent can invoke, and how reusable behavior is
packaged. It answers five questions:

1. **Where does intelligence come from?** Providers and models.
2. **How is a single inference performed?** Executors.
3. **How are long-lived agent processes managed?** Harnesses.
4. **What operations can an agent perform?** Tools.
5. **How is reusable behavior packaged?** Skills.

These five concepts are not synonyms. A model is not a provider. A harness is
not an executor. A tool is not a skill. Collapsing them loses the authority,
lifecycle, failure, and retry boundaries that make Polkagent safe for
Polkadot work.

### 1.1 How to read this document

| Label | Meaning |
|---|---|
| **Established** | Owner-confirmed direction that must be preserved. |
| **Default** | Recommended out-of-box behavior; user may select another mode. |
| **Configurable** | A deliberate choice exposed through safe UX and versioned config. |
| **Phased** | Committed long-term; delivered after dependencies and release gates. |
| **Experimental** | Requires research, a spike, and explicit maturity labeling. |
| **Open** | Evidence or implementation decision still required. |

### 1.2 Relationship to other PRDs

| PRD | Interface with this document |
|---|---|
| PRD-02 (Vocabulary/Architecture) | Defines the kernel types (Run, Artifact, Effect, Grant) that this PRD's ports consume and produce. |
| PRD-03 (Execution Model) | Defines the run lifecycle, effect state machine, and durable outbox that executors and harnesses feed. |
| PRD-05 (Polkadot Integration) | Defines chain-specific tools (decode, simulate, submit) that this PRD's tool taxonomy hosts. |
| PRD-06 (PCA Compatibility) | Defines the direct-runner and framework-bridge patterns that this PRD's harness model must support. |
| PRD-07 (Identity/Security) | Defines the grant model and policy evaluation that governs tool invocation. |
| PRD-12 (Marketplace) | Defines discovery, publication, and installation for skills, tools, and provider adapters. |

### 1.3 Domain terms

| Term | Definition |
|---|---|
| **Provider** | A connection or service that exposes one or more AI models for inference. Examples: an Anthropic API key, a local Ollama instance, an OpenRouter gateway. |
| **Model** | A particular model identity and capability set, distinct from the provider that serves it. Example: `claude-opus-4-6` served by Anthropic or by a gateway. |
| **Executor** | The adapter that performs a single inference run/turn and emits normalized events. It is the boundary between vendor-specific protocols and the Polkagent kernel. |
| **Harness** | A long-lived external process or service (e.g. Claude Code CLI, Codex, OpenCode, a framework agent) with its own session lifecycle, tools, and health. |
| **Tool** | A typed operation an agent may invoke only through an effective grant. Tools have declared inputs, outputs, effects, and capability requirements. |
| **Skill** | A versioned package of instructions, schemas, examples, tests, context, and declared tool/capability requirements. A skill tells an agent *how* to do something; a tool *does* it. |
| **ModelRoute** | A resolved decision about which provider, model, and configuration to use for a particular request. |
| **CapabilityDescriptor** | A structured declaration of what a model, provider, or harness can do (streaming, tool calls, vision, structured output, etc.). |

---

## 2. Provider taxonomy

A **provider** is a configured connection to a service that can perform model
inference. It owns credentials, endpoints, rate limits, health state, and
protocol-specific behavior. It does not own model identity, capability
negotiation, or run lifecycle.

### 2.1 Provider categories

#### 2.1.1 Direct API providers

A direct API provider connects to a vendor's first-party inference endpoint
using the vendor's native protocol.

| Provider | Protocol | Key characteristics |
|---|---|---|
| Anthropic | Messages API (HTTP/SSE) | Native tool use, streaming, extended thinking, cache control, citations, PDF/image input |
| OpenAI | Chat Completions API (HTTP/SSE) | Function calling, streaming, structured output, vision, reasoning |
| Google (Vertex/Gemini) | Gemini API (HTTP/SSE) | Multimodal, grounding, function calling, long context |

Each direct provider adapter implements:
- Authentication (API key, OAuth, service account).
- Request serialization in the vendor's native format.
- Response deserialization and normalization to kernel events.
- Streaming connection management (SSE reconnect, backpressure).
- Vendor-specific error taxonomy mapping to Polkagent error kinds.
- Rate limit tracking and backoff.
- Usage extraction (input tokens, output tokens, cache hits, model-reported costs).

**Established:** The kernel communicates through a single internal
message/tool-call schema. Every direct-provider adapter translates between
this internal schema and the vendor's native wire format (Anthropic Messages
API, OpenAI Chat Completions, Gemini, Mistral). The kernel never imports
provider-specific types; all translation is confined to the adapter crate.

**Established:** Secrets (API keys, tokens) are resolved at execution time
through a `SecretResolver` port and never embedded in portable specs, stored
in artifacts, or exposed to model context.

#### 2.1.2 OpenAI-compatible endpoints

Many providers expose an OpenAI-compatible Chat Completions API. A single
adapter with vendor-specific configuration covers them:

| Provider | Notes |
|---|---|
| Together AI | OpenAI-compatible; extended model selection |
| Fireworks AI | OpenAI-compatible; function calling support varies by model |
| Groq | OpenAI-compatible; high throughput, limited model set |
| Mistral | Native API similar to OpenAI; also OpenAI-compatible mode |
| Perplexity | OpenAI-compatible; search-augmented |
| DeepSeek | OpenAI-compatible; reasoning models |
| Any custom deployment | vLLM, TGI, or other serving frameworks with OpenAI-compatible APIs |

The OpenAI-compatible adapter must:
- Accept a configurable base URL and authentication header.
- Probe or accept declared capability differences (some endpoints lack tool
  calling, streaming, or structured output).
- Not assume that "OpenAI-compatible" means semantically identical; token
  counting, tool call format, `finish_reason` values, usage field presence,
  and error shapes diverge across deployments and should be treated as
  best-effort matches.
- Record the actual endpoint and model version used in the effect attempt.

#### 2.1.3 Local model providers

Local providers run model inference on the user's own hardware. They have no
external network dependency for inference but may have limited capability.

| Provider | Interface | Notes |
|---|---|---|
| Ollama | HTTP REST (OpenAI-compatible subset) | Pull/run models locally; limited tool support varies by model |
| llama.cpp server | HTTP REST (OpenAI-compatible) | Direct GGUF model serving |
| vLLM / TGI | HTTP REST (OpenAI-compatible) | GPU-accelerated serving; production-grade throughput |
| LM Studio | HTTP REST (OpenAI-compatible) | Desktop GUI with local API |
| MLX (Apple Silicon) | Python/HTTP bridge | Optimized for Apple-silicon hardware; preferred local backend on macOS |
| Custom GGUF/ONNX | Polkagent-hosted inference | Future: embed inference directly |

**Default:** Ollama is the recommended local provider for development. All
other local runtimes (llama.cpp, vLLM, TGI, MLX) are served through the
OpenAI-compatible adapter with a configured base URL.

**Key difference from remote providers:** local providers have no API key, no
per-token cost (only compute cost), no external rate limits, and potentially
lower capability. The provider adapter must handle:
- Model availability (is the model downloaded/loaded?).
- Hardware capability detection (GPU, memory).
- Graceful degradation when a capability is missing (e.g., no tool calling).

#### 2.1.4 Routing gateways

A routing gateway is a proxy that routes inference requests to multiple
upstream model providers. It may add load balancing, fallback, caching,
cost optimization, or access to models the user lacks direct accounts for.

| Gateway | Notes |
|---|---|
| OpenRouter | Multi-provider routing; unified billing; OpenAI-compatible API |
| Amazon Bedrock | AWS-hosted model access (Anthropic, Meta, etc.) |
| Azure OpenAI | Microsoft-hosted OpenAI models |
| Cloudflare AI Gateway | Caching, rate limiting, observability proxy |
| LiteLLM (self-hosted) | Open-source multi-provider proxy |

A gateway adapter is structurally similar to an OpenAI-compatible adapter
but must additionally handle:
- Model name mapping (gateway model IDs may differ from upstream IDs).
- Provider-specific headers or authentication passed through the gateway.
- Gateway-reported versus upstream-reported usage and errors.
- The fact that the gateway is a separate trust boundary: it sees request
  content and credentials.

**Configurable:** LiteLLM (self-hosted) and OpenRouter are supported as
optional upstream routing layers. They are not required in default deployments
but can be configured to provide cross-provider load balancing, cost
optimization, and access to models not available through direct accounts. When
an upstream gateway is active, Polkagent's own router defers to it for model
selection but continues to record routing decisions and usage locally.

#### 2.1.5 Custom RPC/WASM providers

**Phased.** Future provider types for specialized or on-chain inference:

- **Custom RPC providers** connect to proprietary inference services through
  gRPC, WebSocket, or custom HTTP protocols.
- **WASM providers** run inference inside a WebAssembly sandbox, enabling
  deterministic and verifiable computation.
- **On-chain inference references** point to future PVM/JAM-hosted model
  services, if they become viable.

These are experimental. They share the same `ModelProvider` trait but require
additional capability probing and potentially different trust models.

### 2.2 Provider data model

```rust
/// A configured connection to a model inference service.
struct ProviderConfig {
    /// Unique identifier for this provider configuration.
    id: ProviderId,
    /// Human-readable name for display.
    name: String,
    /// Provider category and protocol.
    kind: ProviderKind,
    /// Base endpoint URL (or discovery mechanism for local providers).
    endpoint: EndpointConfig,
    /// Reference to credentials in the secret store.
    /// Resolved at execution time, never stored in portable specs.
    credential_ref: Option<SecretRef>,
    /// Rate limit configuration.
    rate_limits: RateLimitConfig,
    /// Health and circuit breaker state.
    health: HealthConfig,
    /// Provider-specific configuration (headers, API version, etc.).
    extra: serde_json::Value,
    /// When this configuration was last verified.
    verified_at: Option<Timestamp>,
}

enum ProviderKind {
    /// Vendor-native API (Anthropic, OpenAI, Google).
    Direct { vendor: Vendor },
    /// OpenAI-compatible endpoint.
    OpenAiCompatible,
    /// Local model server.
    Local { runtime: LocalRuntime },
    /// Routing gateway / proxy.
    Gateway { gateway: GatewayKind },
    /// Custom protocol.
    Custom { protocol: String },
}

enum Vendor {
    Anthropic,
    OpenAi,
    Google,
    Mistral,
    // Extensible without modifying the enum: use Custom(String) for
    // providers that follow a known vendor's protocol with modifications.
    Custom(String),
}

enum LocalRuntime {
    Ollama,
    LlamaCpp,
    LmStudio,
    Mlx,
    Custom(String),
}

enum GatewayKind {
    OpenRouter,
    Bedrock,
    AzureOpenAi,
    Cloudflare,
    LiteLlm,
    Custom(String),
}
```

### 2.3 Provider lifecycle

```text
Configured
  --> Probed (capabilities verified against live endpoint)
  --> Healthy (ready to serve requests)
  --> Degraded (partial failures, elevated latency)
  --> Unhealthy (circuit breaker open)
  --> Disabled (operator-disabled or credential revoked)
  --> Removed
```

- **Probe** verifies the endpoint is reachable, credentials are valid, and
  at least one model is available. It records discovered models and their
  capabilities.
- **Health** is tracked per-provider with exponential backoff on failures.
  See section 11 for circuit breaker details.
- **Credential rotation** triggers a re-probe. The provider continues
  serving with the old credential until the new one is verified.

### 2.4 Provider contract trait

```rust
/// The core provider contract. Implementations are leaf crates;
/// the kernel depends only on this trait.
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Return the provider's current health and status.
    async fn health(&self) -> ProviderHealth;

    /// Discover available models and their capabilities.
    async fn discover_models(&self) -> Result<Vec<ModelDescriptor>, ProviderError>;

    /// Create an executor for a specific model.
    /// The executor handles one inference request.
    async fn executor(
        &self,
        model: &ModelId,
        config: &ExecutionConfig,
    ) -> Result<Box<dyn Executor>, ProviderError>;

    /// Return provider-level rate limit status.
    fn rate_limit_status(&self) -> RateLimitStatus;
}
```

---

## 3. Model taxonomy

A **model** is a particular AI model identity and capability set. It is
distinct from the provider that serves it: the same model (e.g.
`claude-opus-4-6`) may be available through Anthropic's direct API, through
Amazon Bedrock, or through OpenRouter. The model's capabilities are intrinsic;
the provider determines how those capabilities are accessed.

### 3.1 Model identity

```rust
/// A model's stable identity, independent of provider.
struct ModelId {
    /// The canonical model identifier.
    /// Examples: "claude-opus-4-6", "gpt-4o", "gemini-2.5-pro"
    id: String,
    /// Optional version or snapshot identifier.
    version: Option<String>,
    /// Model family for grouping and comparison.
    family: ModelFamily,
}

enum ModelFamily {
    Claude,
    Gpt,
    Gemini,
    Llama,
    Mistral,
    DeepSeek,
    Qwen,
    Custom(String),
}
```

### 3.2 Capability descriptors

Not all models support all features. A capability descriptor declares what a
model can do, enabling the platform to match requirements to available models
and to refuse requests that exceed a model's abilities.

```rust
/// Declares what a model can do.
/// Each field is tri-state: Supported, NotSupported, or Unknown.
struct ModelCapabilities {
    /// Can the model stream partial responses?
    streaming: CapabilityState,
    /// Can the model make structured tool/function calls?
    tool_calls: CapabilityState,
    /// How many tools can be declared in a single request?
    max_tools: Option<u32>,
    /// Can the model produce structured JSON output?
    structured_output: CapabilityState,
    /// Does the model support vision (image input)?
    vision: CapabilityState,
    /// Does the model support PDF input?
    pdf_input: CapabilityState,
    /// Does the model support extended thinking/reasoning?
    extended_thinking: CapabilityState,
    /// Does the model support prompt caching?
    caching: CapabilityState,
    /// Does the model support citations?
    citations: CapabilityState,
    /// Does the model support computer use (screen interaction)?
    computer_use: CapabilityState,
    /// Does the model support code execution?
    code_execution: CapabilityState,
    /// Does the model support web search?
    web_search: CapabilityState,
    /// Does the model support MCP (Model Context Protocol)?
    mcp: CapabilityState,
    /// Vendor-specific capabilities not covered above.
    extra: HashMap<String, CapabilityState>,
}

enum CapabilityState {
    /// The model supports this capability.
    Supported,
    /// The model does not support this capability.
    NotSupported,
    /// Support is unknown; must be probed or tested.
    Unknown,
}
```

### 3.3 Context limits and pricing

```rust
/// Model resource constraints and cost information.
struct ModelLimits {
    /// Maximum input context length in tokens.
    max_input_tokens: Option<u64>,
    /// Maximum output length in tokens.
    max_output_tokens: Option<u64>,
    /// Maximum total context (input + output) in tokens.
    max_total_tokens: Option<u64>,
    /// Maximum number of images per request.
    max_images: Option<u32>,
    /// Token pricing, if known. Per million tokens.
    pricing: Option<ModelPricing>,
}

struct ModelPricing {
    /// Cost per million input tokens, in USD.
    input_per_million: Option<f64>,
    /// Cost per million output tokens, in USD.
    output_per_million: Option<f64>,
    /// Cost per million cached input tokens, if caching is supported.
    cached_input_per_million: Option<f64>,
    /// Currency (default USD).
    currency: String,
    /// When this pricing was last verified.
    as_of: Timestamp,
}
```

### 3.4 Model descriptor (combined)

```rust
/// Complete model description as discovered or configured.
struct ModelDescriptor {
    /// Model identity.
    id: ModelId,
    /// Which provider(s) can serve this model.
    providers: Vec<ProviderId>,
    /// What the model can do.
    capabilities: ModelCapabilities,
    /// Resource limits.
    limits: ModelLimits,
    /// Human-readable description.
    description: Option<String>,
    /// Whether this model is recommended, deprecated, preview, etc.
    status: ModelStatus,
}

enum ModelStatus {
    /// Generally available and recommended.
    Available,
    /// Available but a newer version exists.
    Deprecated { successor: Option<ModelId> },
    /// Preview or early access; may change without notice.
    Preview,
    /// Not currently available through any configured provider.
    Unavailable,
}
```

### 3.5 Model catalog

The **model catalog** is the registry of all known models and their
availability through configured providers.

```rust
/// The catalog of available models.
#[async_trait]
pub trait ModelCatalog: Send + Sync {
    /// List all known models, optionally filtered.
    async fn list(&self, filter: &ModelFilter) -> Vec<ModelDescriptor>;

    /// Get a specific model by ID.
    async fn get(&self, id: &ModelId) -> Option<ModelDescriptor>;

    /// Find models that satisfy a capability requirement.
    async fn find_capable(
        &self,
        required: &ModelCapabilities,
    ) -> Vec<ModelDescriptor>;

    /// Refresh the catalog by probing all configured providers.
    async fn refresh(&self) -> Result<(), CatalogError>;
}
```

The catalog is populated from:
1. **Static configuration** in agent/product specs (pinned model references).
2. **Provider discovery** by probing configured providers.
3. **Registry metadata** from marketplace or organization registries.

---

## 4. Executor: single-run inference

An **executor** performs one inference request (one "turn") against a model
and emits normalized events. It is the adapter boundary between vendor-specific
protocols and the Polkagent kernel. An executor is short-lived: it exists for
one request, then terminates.

### 4.1 Executor contract

```rust
/// Performs one inference turn and emits normalized events.
#[async_trait]
pub trait Executor: Send + Sync {
    /// Execute an inference request.
    /// Streams events through the sink as they arrive.
    /// Returns a terminal result when the inference completes.
    async fn execute(
        &self,
        request: ExecuteRequest,
        sink: &dyn ExecutorEventSink,
    ) -> Result<ExecutionResult, ExecuteError>;

    /// Request cancellation of an in-progress execution.
    async fn cancel(&self) -> Result<(), ExecuteError>;
}
```

### 4.2 Input contract

```rust
/// Everything needed to perform one inference turn.
struct ExecuteRequest {
    /// Unique identifier for this execution attempt.
    attempt_id: AttemptId,
    /// The model to use.
    model: ModelId,
    /// System instructions.
    system: Option<String>,
    /// Conversation messages.
    messages: Vec<Message>,
    /// Available tools for this turn.
    tools: Vec<ToolSchema>,
    /// Execution configuration.
    config: ExecutionConfig,
    /// Token budget for this turn.
    budget: TokenBudget,
    /// Deadline for this execution.
    deadline: Option<Timestamp>,
}

struct ExecutionConfig {
    /// Temperature (0.0 to 1.0+).
    temperature: Option<f64>,
    /// Top-p sampling.
    top_p: Option<f64>,
    /// Maximum output tokens for this request.
    max_tokens: Option<u64>,
    /// Stop sequences.
    stop_sequences: Vec<String>,
    /// Whether to request streaming.
    stream: bool,
    /// Whether to request extended thinking.
    extended_thinking: Option<ExtendedThinkingConfig>,
    /// Response format constraint (e.g., JSON schema).
    response_format: Option<ResponseFormat>,
    /// Vendor-specific parameters.
    extra: serde_json::Value,
}
```

### 4.3 Output contract

```rust
/// The terminal result of an execution.
struct ExecutionResult {
    /// What caused the execution to end.
    stop_reason: StopReason,
    /// Collected output content blocks.
    content: Vec<ContentBlock>,
    /// Tool calls requested by the model.
    tool_calls: Vec<ToolCallRequest>,
    /// Usage statistics for this execution.
    usage: Usage,
    /// Model-reported metadata (model version, request ID, etc.).
    metadata: ExecutionMetadata,
}

enum StopReason {
    /// Model reached a natural end.
    EndTurn,
    /// Model wants to call one or more tools.
    ToolUse,
    /// Output was truncated at the token limit.
    MaxTokens,
    /// A stop sequence was hit.
    StopSequence,
    /// Execution was cancelled.
    Cancelled,
    /// An error occurred.
    Error(ExecuteError),
}

enum ContentBlock {
    Text { text: String },
    Thinking { thinking: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    Image { media_type: String, data: Vec<u8> },
}

struct Usage {
    /// Input tokens consumed.
    input_tokens: u64,
    /// Output tokens produced.
    output_tokens: u64,
    /// Cached input tokens (if applicable).
    cache_read_tokens: Option<u64>,
    /// Tokens written to cache (if applicable).
    cache_write_tokens: Option<u64>,
    /// Provider-reported cost for this request, if available.
    cost: Option<Cost>,
}
```

### 4.4 Streaming event emission

During execution, the executor emits events through the sink:

```rust
/// Events emitted during execution, normalized across providers.
enum ExecutorEvent {
    /// Execution has started; model and provider are confirmed.
    Started {
        model: ModelId,
        provider: ProviderId,
    },
    /// A chunk of text content has been produced.
    TextDelta {
        delta: String,
    },
    /// A chunk of thinking content has been produced.
    ThinkingDelta {
        delta: String,
    },
    /// A tool call is being constructed (streaming).
    ToolCallDelta {
        tool_call_id: String,
        name: Option<String>,
        input_delta: String,
    },
    /// A complete tool call has been assembled.
    ToolCallComplete {
        tool_call: ToolCallRequest,
    },
    /// Usage statistics (may arrive mid-stream or at end).
    UsageUpdate {
        usage: Usage,
    },
    /// Execution has completed.
    Completed {
        result: ExecutionResult,
    },
    /// An error occurred.
    Error {
        error: ExecuteError,
    },
}

/// The sink that receives normalized events from an executor.
#[async_trait]
pub trait ExecutorEventSink: Send + Sync {
    async fn emit(&self, event: ExecutorEvent) -> Result<(), SinkError>;
}
```

### 4.5 Error taxonomy

```rust
enum ExecuteError {
    /// The provider is unreachable or returned a transport error.
    Transport { message: String, retryable: bool },
    /// Authentication failed (invalid or expired credentials).
    Authentication { message: String },
    /// Rate limit exceeded.
    RateLimit { retry_after: Option<Duration> },
    /// The request was too large (context, input, images, etc.).
    RequestTooLarge { message: String },
    /// The model is not available through this provider.
    ModelNotAvailable { model: ModelId },
    /// A capability required by the request is not supported.
    CapabilityMissing { capability: String },
    /// The model refused the request (content policy, safety).
    ContentPolicy { message: String },
    /// The execution timed out.
    Timeout { elapsed: Duration },
    /// The execution was cancelled.
    Cancelled,
    /// An internal or unknown error.
    Internal { message: String },
    /// The provider returned an error with a vendor-specific code.
    ProviderSpecific { code: String, message: String },
}
```

Each error variant carries a `retryable` signal. The retry/fallback system
(section 11) uses this to decide next steps.

---

## 5. Harness: long-lived agent processes

A **harness** is a long-lived external process or service that runs its own
agent loop, tool management, and session lifecycle. Unlike an executor, which
handles one turn, a harness persists across turns and may manage its own
workspace, tools, memory, and internal state.

### 5.1 Why harnesses are distinct from executors

| Concern | Executor | Harness |
|---|---|---|
| Lifetime | One inference request | Hours, days, or indefinitely |
| State | Stateless between calls | Maintains sessions, workspace, context |
| Tool management | Kernel mediates tools | May manage its own tools internally |
| Process model | In-process adapter | External process, container, or service |
| Cancellation | Cancel one request | Interrupt, pause, resume, or terminate |
| Health | Derived from provider health | Independent health/liveness probes |
| Examples | Direct API call to Claude | Claude Code CLI, Codex CLI, OpenCode, ACP/A2A service |

This distinction is critical. PCA's existing "brain" system runs Claude Code
and Codex as child processes with their own tool ecosystems. A "tool call to
Claude" is not the same operation as "managing a Claude Code session in a
workspace." The harness abstraction preserves this difference.

### 5.2 Harness categories

#### 5.2.1 Direct runners (coding CLIs)

A direct runner is a coding-agent CLI that Polkagent spawns and manages as a
child process. The harness adapter translates between the CLI's I/O protocol
and Polkagent's normalized events.

| Runner | Protocol | Notes |
|---|---|---|
| Claude Code (CLI) | Stdin/stdout JSON, process lifecycle | Tool use, file editing, shell commands, MCP |
| Codex CLI | Stdin/stdout, process lifecycle | OpenAI models, sandboxed execution |
| OpenCode | CLI interface | Open-source, multi-provider |
| Custom CLI | Configurable I/O protocol | Any process that speaks a supported protocol |

The direct-runner adapter must handle:
- **Process lifecycle:** spawn, monitor, restart, terminate.
- **Workspace binding:** the CLI runs in a specific directory with specific
  environment variables and permissions.
- **I/O normalization:** parse the CLI's output format (streaming text, tool
  calls, file edits, shell results) into `HarnessEvent`s.
- **Session management:** some CLIs support resume tokens or session IDs.
- **Resource limits:** CPU, memory, disk, and time limits on the child process.
- **Session isolation:** each session is a separate process instance with its
  own workspace and environment. Two concurrent sessions from different runs
  must not share process state, file handles, or internal tool context.

**Design note:** coding-agent CLIs (Claude Code, Codex, Aider, and similar)
are integrated as **process-model adapters**. The harness exposes only a
narrow port to each session: send message, receive events, cancel, terminate.
The internal tool ecosystem of the CLI (its own file editing, shell execution,
sub-agent calls) is opaque to Polkagent and is not re-implemented. Polkagent
records what the harness reports (file changes, shell results, artifacts) but
does not attempt to replicate or override the harness's internal tool behavior.
Building out a full matrix of harness-specific integrations is deferred until
the core adapter contract and session isolation model are validated.

#### 5.2.2 Framework bridges (HTTP/WebSocket/RPC)

A framework bridge connects to an agent service that exposes an HTTP,
WebSocket, or RPC API. The service manages its own model calls and tools;
Polkagent interacts through a structured protocol.

| Bridge type | Protocol | Notes |
|---|---|---|
| ACP (Agent Client Protocol) | HTTP+SSE | Standard agent interaction protocol |
| A2A (Agent-to-Agent) | HTTP/JSON-RPC | Google's agent interoperability spec |
| Custom HTTP | REST/WebSocket | Any service with a compatible API |
| Roko-compatible | gRPC/HTTP | Future Roko-runtime bridge |

Framework bridges differ from direct runners in that the service may be
remote, shared, and independently operated. The bridge adapter must handle:
- **Connection management:** connect, reconnect, authenticate.
- **Protocol translation:** map the framework's event model to `HarnessEvent`.
- **Session affinity:** route requests to the correct session/worker.
- **Trust boundary:** a remote service sees request content; classify
  accordingly.

#### 5.2.3 Custom process/WASM harnesses

**Phased.** Future harness types:
- **Custom process harnesses** run arbitrary executables with a defined I/O
  contract.
- **WASM harnesses** run agent logic inside a WebAssembly sandbox for
  deterministic, portable execution.

### 5.3 Harness lifecycle

```text
                          ┌──────────────────┐
                          │    Configured     │
                          └────────┬─────────┘
                                   │ start()
                                   v
                          ┌──────────────────┐
                     ┌────│    Starting       │
                     │    └────────┬─────────┘
                     │             │ ready
                     │             v
                     │    ┌──────────────────┐
    health_check()───┼───>│      Ready        │<──────────────┐
                     │    └──┬───────────┬───┘                │
                     │       │ execute() │ health fails       │
                     │       v           v                    │
                     │    ┌─────────┐ ┌──────────┐            │
                     │    │Executing│ │ Degraded  │───────────┘
                     │    └──┬──────┘ └──────────┘   recovers
                     │       │ complete/cancel
                     │       v
                     │    ┌──────────────────┐
                     │    │ Session Active    │ (may hold state)
                     │    └──┬──────────┬───┘
                     │       │ resume() │ terminate()
                     │       v          v
                     │    ┌────────┐ ┌──────────────┐
                     │    │  Ready │ │  Terminating  │
                     │    └────────┘ └──────┬───────┘
                     │                      │ terminated
                     │                      v
                     │             ┌──────────────────┐
                     └────────────>│    Terminated     │
                      start fails  └──────────────────┘
```

**Key lifecycle operations:**

| Operation | Meaning |
|---|---|
| `create` | Register a harness configuration |
| `start` | Spawn the process or connect to the service |
| `health` | Probe liveness and readiness |
| `execute` | Send a request and stream events |
| `resume` | Reconnect to an existing session |
| `cancel` | Request cancellation of the current execution |
| `interrupt` | Send an interrupt signal (e.g., Ctrl+C to a CLI) |
| `terminate` | Gracefully shut down the harness |

### 5.4 Session management

A harness session represents a durable interaction context. Sessions may be:

- **Ephemeral:** created for one request, discarded after completion.
- **Persistent:** maintained across multiple requests, holding conversation
  history, workspace state, and tool results.
- **Resumable:** can be reconnected after a disconnect or restart, using a
  session token or ID.

```rust
/// A harness session identifier and metadata.
struct HarnessSession {
    /// Unique session identifier.
    id: SessionId,
    /// The harness this session belongs to.
    harness_id: HarnessId,
    /// Workspace binding for this session.
    workspace: Option<WorkspaceBinding>,
    /// Session state.
    state: SessionState,
    /// When the session was created.
    created_at: Timestamp,
    /// When the session was last active.
    last_active_at: Timestamp,
    /// Resume token, if the harness supports resume.
    resume_token: Option<String>,
}

enum SessionState {
    Active,
    Idle,
    Suspended,
    Terminated,
    Error { message: String },
}
```

### 5.5 Workspace and worktree binding

A harness session is typically bound to a workspace: a directory tree where
the agent can read and write files, run commands, and manage project state.

```rust
/// Binds a harness session to a specific workspace.
struct WorkspaceBinding {
    /// Root directory for the workspace.
    root: PathBuf,
    /// Optional git worktree for isolated changes.
    worktree: Option<WorktreeRef>,
    /// Environment variables to set for the harness process.
    env: HashMap<String, String>,
    /// Filesystem paths the harness may access (beyond the root).
    allowed_paths: Vec<PathBuf>,
    /// Filesystem paths the harness must not access.
    denied_paths: Vec<PathBuf>,
}
```

**Established:** Workspace binding is explicit. A harness cannot access files
outside its declared workspace without a grant. The kernel records which
workspace root, worktree, and environment were used for each execution.

### 5.6 Sandbox boundaries

Harness execution must be sandboxed to prevent unintended effects:

| Boundary | Enforcement |
|---|---|
| Filesystem | Restricted to workspace root + allowed paths |
| Network | Configurable allow/deny lists per harness |
| Process | Resource limits (CPU, memory, disk, time) |
| Environment | Controlled environment variables; no credential leakage |
| Tools | Harness-internal tools are distinct from kernel-mediated tools |

**Direct runners** use OS-level sandboxing (filesystem permissions, cgroups,
seccomp profiles, or macOS sandbox profiles) where available.

**Framework bridges** rely on the remote service's own sandboxing; Polkagent
records the trust boundary and does not assume the service is safe.

### 5.7 Harness contract trait

```rust
/// The contract for managing a long-lived agent process or service.
#[async_trait]
pub trait HarnessService: Send + Sync {
    /// Return harness identity, version, and capabilities.
    fn descriptor(&self) -> &HarnessDescriptor;

    /// Start or connect to the harness.
    async fn start(&self, config: &HarnessStartConfig) -> Result<HarnessHandle, HarnessError>;

    /// Check health/liveness.
    async fn health(&self) -> HarnessHealth;

    /// Create or resume a session.
    async fn session(
        &self,
        request: SessionRequest,
    ) -> Result<HarnessSession, HarnessError>;

    /// Execute a request within a session, streaming events.
    async fn execute(
        &self,
        session: &SessionId,
        request: HarnessRequest,
        sink: &dyn HarnessEventSink,
    ) -> Result<HarnessResult, HarnessError>;

    /// Cancel an in-progress execution.
    async fn cancel(&self, session: &SessionId) -> Result<(), HarnessError>;

    /// Terminate a session, releasing resources.
    async fn terminate_session(&self, session: &SessionId) -> Result<(), HarnessError>;

    /// Shut down the harness entirely.
    async fn shutdown(&self) -> Result<(), HarnessError>;
}

/// Harness identity and capabilities.
struct HarnessDescriptor {
    /// Unique harness identifier.
    id: HarnessId,
    /// Human-readable name.
    name: String,
    /// Harness category.
    kind: HarnessKind,
    /// Version.
    version: String,
    /// Capabilities this harness supports.
    capabilities: HarnessCapabilities,
}

enum HarnessKind {
    /// A CLI process managed as a child process.
    DirectRunner { command: String },
    /// An HTTP/WebSocket/RPC service.
    FrameworkBridge { protocol: BridgeProtocol },
    /// A custom process.
    CustomProcess { command: String },
}

struct HarnessCapabilities {
    /// Can the harness resume sessions after disconnect?
    session_resume: bool,
    /// Does the harness manage its own tools?
    internal_tools: bool,
    /// Can the harness use kernel-mediated tools?
    kernel_tools: bool,
    /// Does the harness support workspace/worktree binding?
    workspace_binding: bool,
    /// Does the harness support streaming events?
    streaming: bool,
    /// Does the harness support cancellation?
    cancellation: bool,
    /// Maximum concurrent sessions.
    max_sessions: Option<u32>,
}
```

### 5.8 Normalized harness events

```rust
/// Events emitted by a harness, normalized across harness types.
enum HarnessEvent {
    /// The harness has started processing.
    Started { session: SessionId },
    /// Streaming text output from the harness.
    TextDelta { delta: String },
    /// The harness is requesting a tool call (kernel-mediated).
    ToolRequest { call: ToolCallRequest },
    /// The harness completed a tool call internally.
    ToolResult { call_id: String, result: ToolResult },
    /// A file was created or modified.
    FileChange { path: String, change_type: FileChangeType },
    /// A shell command was executed.
    ShellExecution { command: String, exit_code: Option<i32> },
    /// An artifact was produced.
    ArtifactProduced { artifact: ArtifactRef },
    /// Usage update.
    UsageUpdate { usage: Usage },
    /// Progress or status update.
    Progress { message: String, percent: Option<f32> },
    /// Warning.
    Warning { message: String },
    /// Error.
    Error { error: HarnessError },
    /// Execution completed.
    Completed { result: HarnessResult },
}
```

---

## 6. Tool taxonomy

A **tool** is a typed operation that an agent may invoke during execution. Every
tool invocation passes through the kernel's grant system: the caller must hold
an effective grant that permits the tool, its inputs, and its effects.

### 6.1 Tool schema

```rust
/// A tool's complete definition.
struct ToolSchema {
    /// Unique tool identifier (namespaced).
    /// Examples: "polkagent.file.read", "polkagent.chain.decode"
    id: ToolId,
    /// Human-readable name for display and model context.
    name: String,
    /// Description of what the tool does.
    description: String,
    /// JSON Schema for the tool's input parameters.
    input_schema: serde_json::Value,
    /// JSON Schema for the tool's output.
    output_schema: Option<serde_json::Value>,
    /// What category of tool this is.
    category: ToolCategory,
    /// What effects this tool may produce.
    effects: Vec<ToolEffect>,
    /// What capabilities are required to use this tool.
    required_capabilities: Vec<CapabilityRequirement>,
    /// Whether this tool's output may contain sensitive data.
    output_classification: Classification,
    /// Whether this tool is idempotent (safe to retry).
    idempotent: bool,
    /// Estimated cost or resource consumption.
    cost_hint: Option<CostHint>,
    /// Version of this tool definition.
    version: String,
}
```

### 6.2 Tool categories

Tools are organized into categories that reflect their authority boundary and
effect type. This categorization drives policy evaluation, grant resolution,
and marketplace display.

```rust
enum ToolCategory {
    /// File system operations: read, write, search, glob.
    File,
    /// Shell/process execution.
    Shell,
    /// Network operations: HTTP requests, DNS, etc.
    Network,
    /// Chain operations: decode, simulate, query state.
    Chain,
    /// Signer operations: sign payloads (high privilege).
    Signer,
    /// Memory operations: read/write agent memory.
    Memory,
    /// Workspace operations: git, project management.
    Workspace,
    /// Communication: send messages, notifications.
    Communication,
    /// Computation: math, data processing, code execution.
    Compute,
    /// Registry/marketplace operations.
    Registry,
    /// MCP-provided tools (from MCP servers).
    Mcp,
    /// Custom/extension-provided tools.
    Extension,
}
```

#### 6.2.1 File tools

| Tool | Description | Effects | Idempotent |
|---|---|---|---|
| `file.read` | Read file contents | None (read-only) | Yes |
| `file.write` | Write or create a file | Filesystem write | No |
| `file.edit` | Apply a targeted edit to a file | Filesystem write | No |
| `file.search` | Search file contents (grep) | None (read-only) | Yes |
| `file.glob` | Find files by pattern | None (read-only) | Yes |
| `file.list` | List directory contents | None (read-only) | Yes |

#### 6.2.2 Shell tools

| Tool | Description | Effects | Idempotent |
|---|---|---|---|
| `shell.execute` | Run a shell command | Process execution, filesystem, network | No |
| `shell.background` | Run a command in the background | Process execution | No |

#### 6.2.3 Network tools

| Tool | Description | Effects | Idempotent |
|---|---|---|---|
| `network.http` | Make an HTTP request | Network I/O | Depends on method |
| `network.fetch` | Fetch and extract web content | Network I/O | Yes |

#### 6.2.4 Chain tools

| Tool | Description | Effects | Idempotent |
|---|---|---|---|
| `chain.query` | Query on-chain state | Network I/O (read-only) | Yes |
| `chain.decode` | Decode an extrinsic or call | None (pure computation) | Yes |
| `chain.simulate` | Dry-run a transaction | Network I/O (read-only) | Yes |
| `chain.submit` | Submit a signed transaction | Chain write (irreversible) | No |
| `chain.watch` | Watch for finality/events | Network I/O (read-only) | Yes |
| `chain.metadata` | Fetch and inspect runtime metadata | Network I/O (read-only) | Yes |

**Important:** `chain.submit` is an irreversible effect. It is never invoked
directly by a model. The kernel's effect system handles submission through
the `EffectIntent` -> `EffectAttempt` -> `EffectOutcome` lifecycle (see
PRD-03). Chain tools are the *implementation* behind those effects.

#### 6.2.5 Signer tools

| Tool | Description | Effects | Idempotent |
|---|---|---|---|
| `signer.sign` | Request a signature on a canonical payload | Signer interaction | No |
| `signer.capabilities` | Query signer capabilities and accounts | None (read-only) | Yes |

**Established:** Signer tools are high-privilege. They are never exposed
directly to model context. Signing is always mediated by the kernel's
approval and effect system.

#### 6.2.6 Memory tools

| Tool | Description | Effects | Idempotent |
|---|---|---|---|
| `memory.read` | Read from agent memory | None (read-only) | Yes |
| `memory.write` | Write to agent memory | Memory write | No |
| `memory.search` | Search memory by query | None (read-only) | Yes |

#### 6.2.7 MCP tool integration

The Model Context Protocol (MCP) provides a standardized way for external
servers to expose tools to AI models. Polkagent integrates MCP tools through
a bridge that maps them to the kernel's tool system.

```rust
/// An MCP server connection that provides tools.
struct McpServerConfig {
    /// Server identifier.
    id: McpServerId,
    /// How to connect to the server.
    transport: McpTransport,
    /// Allowed tools from this server (empty = all discovered).
    allowed_tools: Vec<String>,
    /// Denied tools from this server.
    denied_tools: Vec<String>,
    /// Grant scope for tools from this server.
    grant_scope: GrantScope,
}

enum McpTransport {
    /// Stdio-based MCP server (subprocess). Use for local tools.
    Stdio { command: String, args: Vec<String>, env: HashMap<String, String> },
    /// HTTP+SSE-based MCP server. Deprecated since MCP spec 2025-03-26.
    /// Supported for compatibility with older servers only; prefer StreamableHttp.
    Sse { url: String },
    /// Streamable HTTP-based MCP server. Current standard for remote tools.
    /// As of the 2026-07-28 RC this transport no longer requires a session
    /// handshake, making it suitable for stateless, routable deployments.
    StreamableHttp { url: String },
}
```

**Established transport choices:**
- **Streamable HTTP** is the standard for remote MCP servers. It is the only
  transport actively developed in the MCP specification as of 2026.
- **Stdio** is the standard for local MCP servers (subprocesses running on
  the same host as Polkagent). It is appropriate for developer tooling and
  local integrations.
- **SSE** was deprecated in the MCP spec on 2025-03-26. New MCP server
  integrations must not use SSE. The adapter retains SSE support only for
  backward compatibility with servers that have not yet migrated.

MCP tools are discovered at server connection time and registered in the
tool registry with the `Mcp` category. Each MCP tool is wrapped:
1. Its JSON Schema input/output is preserved.
2. A `GrantScope` limits what effects MCP tools may produce.
3. Tool invocations are logged as `EffectAttempt`s.
4. MCP tool output is classified according to the server's trust level.

### 6.3 Tool invocation flow

```text
Model requests tool call
  --> Executor/Harness emits ToolCallRequest
  --> Kernel validates tool exists and is available
  --> PolicyEvaluator checks grant for this tool + these inputs
  --> If denied: return denial to model with reason
  --> If approval required: pause, request approval, resume on decision
  --> ToolHost.invoke(call, grant) executes the tool
  --> Tool result is classified and returned to the model
  --> Effect records (intent, attempt, outcome) are persisted
```

### 6.4 Tool host contract

```rust
/// Mediates tool invocations with grant enforcement.
#[async_trait]
pub trait ToolHost: Send + Sync {
    /// Register a tool implementation.
    fn register(&mut self, tool: ToolRegistration) -> Result<(), ToolError>;

    /// List available tools for a given grant scope.
    fn available_tools(&self, scope: &GrantScope) -> Vec<ToolSchema>;

    /// Invoke a tool with grant enforcement.
    async fn invoke(
        &self,
        call: ToolCall,
        grant: &ResolvedGrant,
        context: &RequestContext,
    ) -> Result<ToolResult, ToolError>;
}

/// A tool call request from a model or harness.
struct ToolCall {
    /// The tool call ID (from the model's response).
    id: String,
    /// The tool to invoke.
    tool_id: ToolId,
    /// Input parameters.
    input: serde_json::Value,
    /// Who is requesting this tool call.
    caller: CallerId,
}

/// The result of a tool invocation.
struct ToolResult {
    /// The tool call ID this result corresponds to.
    call_id: String,
    /// Whether the tool call succeeded.
    success: bool,
    /// Output content.
    content: Vec<ToolContent>,
    /// Output classification (may be elevated from input classification).
    classification: Classification,
    /// Execution metrics.
    metrics: ToolMetrics,
}

enum ToolContent {
    Text(String),
    Json(serde_json::Value),
    Image { media_type: String, data: Vec<u8> },
    Error { message: String },
}
```

### 6.5 Capability requirements

Each tool declares what capabilities it needs. The kernel verifies these
against the effective grant before invocation.

```rust
/// A capability requirement for a tool.
enum CapabilityRequirement {
    /// Read access to specific filesystem paths.
    FileRead { paths: Vec<PathPattern> },
    /// Write access to specific filesystem paths.
    FileWrite { paths: Vec<PathPattern> },
    /// Execute a process.
    ProcessExec { commands: Vec<String> },
    /// Network access to specific hosts/ports.
    Network { hosts: Vec<HostPattern> },
    /// Chain read access to specific networks.
    ChainRead { networks: Vec<NetworkId> },
    /// Chain write access (submit transactions).
    ChainWrite { networks: Vec<NetworkId> },
    /// Signer access to specific accounts.
    SignerAccess { accounts: Vec<AccountPattern> },
    /// Memory read/write.
    Memory { read: bool, write: bool },
    /// Custom capability.
    Custom { name: String, params: serde_json::Value },
}
```

---

## 7. Skill taxonomy

A **skill** is a versioned package that tells an agent *how* to accomplish a
task. Unlike a tool (which *does* something), a skill provides instructions,
context, schemas, examples, and test cases that guide the agent's behavior.

### 7.1 What a skill contains

```rust
/// A versioned skill package.
struct SkillManifest {
    /// Unique skill identifier (namespaced).
    /// Example: "polkagent.polkadot.explain-extrinsic"
    id: SkillId,
    /// Human-readable name.
    name: String,
    /// Detailed description.
    description: String,
    /// Skill version (semver).
    version: Version,
    /// Minimum Polkagent version required.
    min_platform_version: Option<Version>,
    /// What this skill contains.
    contents: SkillContents,
    /// What tools this skill needs.
    required_tools: Vec<ToolRequirement>,
    /// What capabilities this skill needs.
    required_capabilities: Vec<CapabilityRequirement>,
    /// What models/capabilities this skill needs.
    model_requirements: Option<ModelCapabilities>,
    /// Compatibility constraints.
    compatibility: SkillCompatibility,
    /// Author and provenance.
    provenance: SkillProvenance,
    /// Cost and resource hints.
    cost_hints: Option<CostHints>,
}
```

### 7.2 Skill contents

```rust
/// The contents of a skill package.
struct SkillContents {
    /// System instructions to inject when this skill is active.
    instructions: String,
    /// Structured schemas for skill-specific inputs and outputs.
    schemas: Vec<SchemaDefinition>,
    /// Example interactions demonstrating correct use.
    examples: Vec<SkillExample>,
    /// Test cases for validating skill behavior.
    tests: Vec<SkillTest>,
    /// Context/knowledge files (reference docs, chain specs, etc.).
    context_files: Vec<ContextFile>,
    /// Prompt templates for common operations within the skill.
    templates: Vec<PromptTemplate>,
}

/// An example interaction for a skill.
struct SkillExample {
    /// Description of the example.
    description: String,
    /// Input messages.
    input: Vec<Message>,
    /// Expected tool calls.
    expected_tools: Vec<ExpectedToolCall>,
    /// Expected output characteristics (not exact match).
    expected_output: OutputExpectation,
}

/// A test case for validating skill behavior.
struct SkillTest {
    /// Test identifier.
    id: String,
    /// Test description.
    description: String,
    /// Test category.
    category: TestCategory,
    /// Input for the test.
    input: TestInput,
    /// Assertions on the output.
    assertions: Vec<TestAssertion>,
    /// Whether this test requires a live model (vs. fixture-based).
    requires_model: bool,
}

enum TestCategory {
    /// Basic functionality test.
    Functional,
    /// Safety and security test (e.g., prompt injection resistance).
    Safety,
    /// Edge case or error handling test.
    EdgeCase,
    /// Performance or cost test.
    Performance,
}
```

### 7.3 Versioning and compatibility

```rust
/// Compatibility constraints for a skill.
struct SkillCompatibility {
    /// Compatible Polkagent versions (semver range).
    platform: VersionRange,
    /// Compatible model families and minimum capabilities.
    models: Vec<ModelCompatibility>,
    /// Skills that this skill depends on.
    dependencies: Vec<SkillDependency>,
    /// Skills that conflict with this one.
    conflicts: Vec<SkillId>,
    /// Chain profiles this skill is designed for (if chain-specific).
    chain_profiles: Vec<ChainProfilePattern>,
}

struct SkillDependency {
    /// The required skill.
    skill: SkillId,
    /// Required version range.
    version: VersionRange,
    /// Whether this dependency is optional (provides enhanced behavior).
    optional: bool,
}
```

Skills follow semantic versioning:
- **Patch** (1.0.x): bug fixes, typo corrections, no behavior change.
- **Minor** (1.x.0): new examples, improved instructions, backward-compatible
  schema additions.
- **Major** (x.0.0): changed schemas, different tool requirements, breaking
  instruction changes.

### 7.4 Skill lifecycle

```text
Author creates skill
  --> Validate manifest and test suite
  --> Package with content-addressed digest
  --> Sign with author key (optional but recommended)
  --> Publish to registry/marketplace
  --> Discover and preview
  --> Install pinned version with lockfile
  --> Activate in agent/product spec
  --> Grant required capabilities separately
  --> Use during runs
  --> Update (staged, rollback-safe)
  --> Deprecate or revoke
```

**Key principle:** installing a skill does not grant it capabilities. The skill
declares what it needs; the operator grants what is appropriate. A skill
that declares `chain.submit` access does not receive it simply by being
installed.

### 7.5 Skill resolver contract

```rust
/// Resolves and loads skills for a run.
#[async_trait]
pub trait SkillResolver: Send + Sync {
    /// Resolve a skill reference to a concrete, loaded skill.
    async fn resolve(&self, reference: &SkillRef) -> Result<ResolvedSkill, SkillError>;

    /// List available skills for an agent.
    async fn available(&self, agent: &AgentId) -> Vec<SkillSummary>;

    /// Validate a skill manifest and test suite.
    async fn validate(&self, manifest: &SkillManifest) -> Result<ValidationReport, SkillError>;
}

/// A resolved, ready-to-use skill.
struct ResolvedSkill {
    /// The skill manifest.
    manifest: SkillManifest,
    /// Loaded instructions (with any template substitutions).
    instructions: String,
    /// Loaded context files.
    context: Vec<LoadedContext>,
    /// Tool schemas this skill requires (resolved from tool registry).
    tools: Vec<ToolSchema>,
    /// Content digest for verification.
    digest: Digest,
}
```

---

## 8. Capability negotiation

When a run begins, the platform must determine which capabilities are
available and whether they satisfy the run's requirements. This negotiation
happens at multiple levels.

### 8.1 Negotiation levels

```text
Agent/Skill requirements
  What capabilities does this agent/skill need?
    (tool calls, vision, streaming, minimum context, etc.)

Model capabilities
  What can the selected model do?
    (from ModelDescriptor.capabilities)

Provider capabilities
  What does the provider actually expose for this model?
    (some providers restrict model features)

Grant constraints
  What is the run allowed to do?
    (from ResolvedGrant: tool access, network, filesystem, etc.)
```

### 8.2 Negotiation algorithm

```rust
/// Result of capability negotiation for a run.
struct NegotiatedCapabilities {
    /// The model selected for this run.
    model: ModelId,
    /// The provider serving this model.
    provider: ProviderId,
    /// Capabilities that are available and granted.
    available: ModelCapabilities,
    /// Capabilities that were required but are missing.
    missing: Vec<MissingCapability>,
    /// Whether the negotiation succeeded (no missing required capabilities).
    satisfied: bool,
    /// Warnings (e.g., capability is Unknown, not confirmed).
    warnings: Vec<String>,
}

struct MissingCapability {
    /// What capability is missing.
    capability: String,
    /// Why it is needed.
    required_by: String, // skill ID, tool ID, or agent spec
    /// Whether this is a hard requirement or a soft preference.
    hard: bool,
}
```

The negotiation proceeds:
1. Collect all capability requirements from the agent spec, active skills,
   and declared tools.
2. Query the model catalog for the selected model's capabilities.
3. Check provider-level restrictions.
4. Intersect with the effective grant.
5. Report missing capabilities. Hard failures block the run; soft failures
   produce warnings.

### 8.3 Capability probing

Some capabilities cannot be determined from static metadata. The platform
may need to probe:

- **Tool call support:** send a test request with tools to verify the model
  handles them correctly.
- **Streaming:** verify the provider's SSE/WebSocket endpoint works.
- **Context limits:** test with progressively larger inputs if limits are
  uncertain.
- **Structured output:** verify JSON mode works for the model/provider
  combination.

Probe results are cached per (provider, model) pair and refreshed
periodically or after configuration changes.

---

## 9. Streaming architecture

Polkagent uses streaming at multiple layers. Streams are not the authority
store; durable records are. Streams provide real-time feedback and
progressive rendering.

### 9.1 Streaming layers

```text
Provider/Model layer
  Vendor SSE or WebSocket stream --> Executor normalizes to ExecutorEvent

Harness layer
  CLI stdout/stderr or framework WebSocket --> Harness normalizes to HarnessEvent

Kernel layer
  ExecutorEvent/HarnessEvent --> Run events (durable + ephemeral)

Presentation layer
  Run events --> Transport-specific delivery (SSE, WebSocket, polling)
```

### 9.2 Vendor stream protocols

| Provider | Protocol | Notes |
|---|---|---|
| Anthropic | SSE (`text/event-stream`) | `message_start`, `content_block_delta`, `message_stop` events |
| OpenAI | SSE (`text/event-stream`) | `data: {...}` lines with `[DONE]` terminator |
| Google | SSE or HTTP chunked | Streaming `generateContent` responses |
| Local (Ollama, llama.cpp) | HTTP chunked JSON lines | Newline-delimited JSON objects |

Each executor adapter handles:
1. Connection establishment and authentication.
2. Parsing vendor-specific stream format.
3. Normalizing chunks to `ExecutorEvent`.
4. Detecting stream end, errors, and timeouts.
5. Reconnection on transport failures (within reason).
6. Backpressure when the consumer is slow.
7. Assembling streaming tool calls from index-keyed fragments. Providers
   (notably OpenAI-compatible endpoints) emit tool-call arguments as
   incremental deltas keyed by tool index rather than tool ID. The adapter
   maintains per-index assembly buffers, merges deltas in order, and emits a
   single `ToolCallComplete` event only when the full call is assembled. The
   kernel never sees partially assembled tool calls.

### 9.3 Stream normalization guarantees

All streams, regardless of source, provide:

| Guarantee | Description |
|---|---|
| Ordered delivery | Events within one execution arrive in causal order. |
| At-most-one terminal | Exactly one `Completed` or `Error` event ends the stream. |
| Lossless text reconstruction | Concatenating all `TextDelta` events produces the complete text. |
| Usage reporting | A `UsageUpdate` arrives before or with the terminal event. |
| No authority | A stream is a projection. Durable state is in the effect store. |

### 9.4 Client-facing streaming

The kernel exposes run progress to clients through:

```rust
/// Client-facing run progress stream.
enum RunProgressEvent {
    /// Run has been accepted and queued.
    Queued { run_id: RunId },
    /// Run is being processed.
    Working { run_id: RunId },
    /// Streaming text from the model.
    TextDelta { run_id: RunId, delta: String },
    /// A tool is being used.
    ToolUse { run_id: RunId, tool: String, status: ToolUseStatus },
    /// An approval is requested.
    ApprovalRequired { run_id: RunId, approval: ApprovalRequest },
    /// An artifact was produced.
    ArtifactProduced { run_id: RunId, artifact: ArtifactSummary },
    /// Run completed.
    Completed { run_id: RunId, summary: RunSummary },
    /// Run failed.
    Failed { run_id: RunId, error: RunError },
}
```

Transport adapters (web SSE, WebSocket, Polkadot chat, CLI) consume these
events and render them appropriately. A disconnected client can reconnect
and replay from durable state; the stream is convenience, not authority.

---

## 10. Usage tracking and cost accounting

Every inference execution, tool invocation, and harness session produces usage
records. These feed cost visibility, budget enforcement, billing, and
optimization.

### 10.1 Usage records

```rust
/// Usage record for one execution or tool invocation.
struct UsageRecord {
    /// Unique record ID.
    id: UsageRecordId,
    /// What produced this usage.
    source: UsageSource,
    /// The run this usage belongs to.
    run_id: RunId,
    /// The effect attempt this usage belongs to.
    attempt_id: AttemptId,
    /// Token counts.
    tokens: TokenUsage,
    /// Computed or reported cost.
    cost: Option<Cost>,
    /// Wall-clock duration.
    duration: Duration,
    /// When this usage occurred.
    timestamp: Timestamp,
}

enum UsageSource {
    /// Model inference.
    Inference { model: ModelId, provider: ProviderId },
    /// Tool invocation.
    Tool { tool: ToolId },
    /// Harness session time.
    Harness { harness: HarnessId, session: SessionId },
}

struct TokenUsage {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    total_tokens: u64,
}

struct Cost {
    /// Cost amount in the smallest currency unit.
    amount: f64,
    /// Currency code (e.g., "USD").
    currency: String,
    /// How this cost was determined.
    source: CostSource,
}

enum CostSource {
    /// Reported by the provider in the response.
    ProviderReported,
    /// Computed from token counts and catalog pricing.
    Computed,
    /// Estimated (pricing data may be stale).
    Estimated,
}
```

### 10.2 Budget enforcement

```rust
/// Budget limits for a run or tenant.
struct Budget {
    /// Maximum tokens (input + output) for this scope.
    max_tokens: Option<u64>,
    /// Maximum cost in the specified currency.
    max_cost: Option<Cost>,
    /// Maximum wall-clock duration.
    max_duration: Option<Duration>,
    /// Maximum number of inference calls.
    max_inferences: Option<u32>,
    /// Maximum number of tool invocations.
    max_tool_calls: Option<u32>,
}
```

Budget checks are performed:
1. Before each inference (estimated cost vs. remaining budget).
2. After each inference (actual cost deducted from budget).
3. Before tool invocations with cost hints.
4. At run boundaries (lifetime vs. rolling budgets).

When a budget is exceeded, the run receives a terminal error with the
specific limit that was hit, enabling clear error messages.

### 10.3 Accounting hierarchy

```text
Platform/operator level
  Total usage across all tenants

Tenant/organization level
  Usage per org, billing period, quotas

User level
  Usage per user, personal budgets

Agent level
  Usage per agent spec

Run level
  Usage per individual run

Turn level
  Usage per inference turn within a run
```

Each level can have its own budget. The effective budget for a run is the
minimum across all applicable levels.

---

## 11. Retry, fallback, and health

### 11.1 Error classification for retry

```rust
enum RetryClass {
    /// Safe to retry immediately (e.g., transient network error).
    RetryImmediate,
    /// Safe to retry after a delay (e.g., rate limit).
    RetryAfter { delay: Duration },
    /// May be retried with a different provider/model.
    Fallback,
    /// Must not be retried (e.g., auth failure, content policy).
    NoRetry,
    /// Unknown; conservative default is no retry.
    Unknown,
}
```

### 11.2 Retry policy

```rust
struct RetryPolicy {
    /// Maximum number of retries for the same provider/model.
    max_retries: u32,
    /// Base delay between retries (exponential backoff).
    base_delay: Duration,
    /// Maximum delay between retries.
    max_delay: Duration,
    /// Jitter factor (0.0 to 1.0).
    jitter: f64,
    /// Which error kinds are retryable.
    retryable_errors: Vec<RetryClass>,
}
```

**Important constraint (from Roko research):** Do not automatically retry
after a tool call, signature, or chain effect has begun. A retry at that
point can produce a different interpretation of partially completed work.
Any reroute creates a new attempt, preserves route evidence, and resumes
only through explicit state transitions.

### 11.3 Model fallback chains

A fallback chain defines alternative models to try when the primary model
fails. The chain is evaluated in order; each entry specifies which errors
trigger fallback.

```rust
struct FallbackChain {
    /// Primary model route.
    primary: ModelRoute,
    /// Fallback entries, tried in order.
    fallbacks: Vec<FallbackEntry>,
}

struct FallbackEntry {
    /// Alternative model route.
    route: ModelRoute,
    /// Which error classes trigger this fallback.
    trigger_on: Vec<RetryClass>,
    /// Maximum times to use this fallback per run.
    max_uses: Option<u32>,
}
```

**Fallback constraints:**
- Fallback is only permitted before a run has produced side effects (tool
  calls, file writes, chain interactions).
- After side effects, the run must complete with the current model or fail
  with a clear error. A new model cannot safely interpret partial tool results
  from a different model.
- Every fallback is recorded in the run's effect history, including why the
  primary failed and which alternative was selected.

### 11.4 Circuit breakers

Each provider has an independent circuit breaker:

```rust
struct CircuitBreaker {
    /// Current state.
    state: CircuitState,
    /// Number of consecutive failures.
    failure_count: u32,
    /// Threshold to open the circuit.
    failure_threshold: u32,
    /// Duration the circuit stays open before a probe.
    open_duration: Duration,
    /// Number of successful probes needed to close.
    half_open_successes: u32,
}

enum CircuitState {
    /// Normal operation.
    Closed,
    /// Failures exceeded threshold; rejecting requests.
    Open { until: Timestamp },
    /// Probing with limited requests.
    HalfOpen { successes: u32 },
}
```

When a circuit opens:
1. New requests to that provider fail fast with `ProviderUnavailable`.
2. The fallback chain is consulted.
3. After `open_duration`, one probe request is sent.
4. If the probe succeeds, the circuit moves to HalfOpen.
5. After enough successes, the circuit closes.

### 11.5 Health monitoring

```rust
/// Health status for a provider or harness.
struct HealthStatus {
    /// Overall health.
    state: HealthState,
    /// Last successful request timestamp.
    last_success: Option<Timestamp>,
    /// Last failure timestamp and error.
    last_failure: Option<(Timestamp, String)>,
    /// Recent latency percentiles.
    latency_p50: Option<Duration>,
    latency_p99: Option<Duration>,
    /// Error rate over the last window.
    error_rate: f64,
    /// Circuit breaker state.
    circuit: CircuitState,
}

enum HealthState {
    Healthy,
    Degraded { reason: String },
    Unhealthy { reason: String },
    Unknown,
}
```

---

## 12. Model routing

When a run begins and needs model inference, the **router** selects a
(provider, model) pair based on requirements, preferences, health, cost, and
policy.

### 12.1 Router contract

```rust
#[async_trait]
pub trait ModelRouter: Send + Sync {
    /// Select a model route for the given requirements.
    async fn route(
        &self,
        request: &RouteRequest,
    ) -> Result<ModelRoute, RouteError>;
}

struct RouteRequest {
    /// Required capabilities (from agent/skill).
    required_capabilities: ModelCapabilities,
    /// Preferred model (from user or agent config).
    preferred_model: Option<ModelId>,
    /// Preferred provider.
    preferred_provider: Option<ProviderId>,
    /// Budget constraints.
    budget: Option<Budget>,
    /// Routing policy (cost, latency, capability preference).
    policy: RoutingPolicy,
}

struct ModelRoute {
    /// Selected model.
    model: ModelId,
    /// Selected provider.
    provider: ProviderId,
    /// Execution configuration.
    config: ExecutionConfig,
    /// Why this route was selected.
    reason: RouteReason,
}

enum RoutingPolicy {
    /// Use the exact specified model and provider; fail if unavailable.
    Exact,
    /// Prefer cost efficiency; fall back to capable alternatives.
    CostOptimized,
    /// Prefer low latency.
    LatencyOptimized,
    /// Prefer the most capable model.
    CapabilityMaximized,
    /// Custom routing rules.
    Custom { rules: serde_json::Value },
}
```

### 12.2 Routing decision record

Every routing decision is recorded as part of the run's effect history:

```rust
struct RouteDecision {
    /// The selected route.
    selected: ModelRoute,
    /// Alternatives that were considered.
    alternatives: Vec<(ModelRoute, String)>, // (route, reason_not_selected)
    /// The routing policy that was applied.
    policy: RoutingPolicy,
    /// When this decision was made.
    timestamp: Timestamp,
}
```

This enables operators to understand why a particular model was selected
and to tune routing policy based on outcomes.

### 12.3 Health-aware and cost/latency-aware routing

The router consults each provider's current circuit breaker state before
selecting a route. A provider with an open circuit is excluded from
consideration; the router falls back to the next eligible (provider, model)
pair automatically. This means circuit breakers and the router are directly
coupled: the router must read health state, not just route on static policy.

When `CostOptimized` or `LatencyOptimized` routing policies are in effect,
the router scores candidate routes using per-provider observed latency
percentiles (`latency_p50`, `latency_p99` from `HealthStatus`) and estimated
cost derived from model pricing and the request's token budget. The selected
route is the one with the best score that also satisfies the capability
requirements and has a healthy circuit.

**Open (validate-next):** The exact scoring weights and the feedback loop
between routing outcomes and policy tuning are not finalized. Implementation
should instrument routing decisions before committing to a specific algorithm.

---

## 13. Three end-to-end examples

### 13.1 Example 1: Single direct model call

**Scenario:** A user asks Polkagent to explain a Polkadot extrinsic.

```text
1. User submits: "Explain this extrinsic: 0x2804..."
   Transport admits the message; kernel creates a Run.

2. Router selects a model route:
   Required capabilities: tool_calls, streaming
   Preferred model: claude-opus-4-6
   Selected: (Anthropic direct provider, claude-opus-4-6)
   Route decision recorded as artifact.

3. Executor created for Anthropic provider:
   - SecretResolver provides API key at execution time.
   - ExecuteRequest built with system instructions (from agent spec),
     user message, and available tools [chain.decode, chain.metadata].

4. Executor connects to Anthropic Messages API (SSE):
   - Emits Started { model: claude-opus-4-6, provider: anthropic }
   - Streams TextDelta events as the model responds.
   - Model requests tool call: chain.decode { bytes: "0x2804..." }

5. Kernel mediates the tool call:
   - PolicyEvaluator checks grant: chain.decode is read-only, permitted.
   - ToolHost.invoke(chain.decode, grant, context) executes.
   - Tool decodes the extrinsic using pinned runtime metadata.
   - ToolResult returned to executor.

6. Executor resumes with tool result:
   - Model produces explanation text with tool result context.
   - Emits more TextDelta events.
   - Emits Completed with final ExecutionResult.

7. Kernel records:
   - Artifact: decoded extrinsic evidence (with metadata hash).
   - Artifact: model explanation (classified as inference, not evidence).
   - UsageRecord: 1,200 input tokens, 800 output tokens, $0.012.
   - EffectOutcome: success.

8. Projection delivers the response to the user through their transport.
```

**What stays common across providers:** Run ID, effect lifecycle, artifact
lineage, usage record format, tool mediation, grant enforcement.

**What is adapter-specific:** SSE parsing, Anthropic message format,
authentication header, error codes.

### 13.2 Example 2: Resumed coding-harness session

**Scenario:** A developer resumes a Claude Code session to continue working
on a Polkadot SDK pallet.

```text
1. Developer opens CLI: `polkagent resume session-abc123`
   Kernel looks up session-abc123: HarnessSession for Claude Code,
   bound to workspace /home/dev/my-pallet, worktree feature/new-call.

2. Harness service checks health:
   - Claude Code process is still running (PID 12345).
   - Health probe succeeds; session is Active.

3. Developer sends: "Add a weight annotation to the new_call extrinsic"
   Kernel creates a new Turn within the existing Run.

4. HarnessService.execute(session-abc123, request, sink):
   - Sends the message to Claude Code's stdin.
   - Claude Code reads the workspace, identifies the pallet.
   - Harness emits HarnessEvent::TextDelta as Claude Code responds.

5. Claude Code decides to edit a file:
   - HarnessEvent::ToolRequest { tool: file.edit, ... }
   - This is a harness-internal tool (Claude Code manages its own
     file editing), so Polkagent receives the event but does not mediate.
   - HarnessEvent::FileChange { path: "src/lib.rs", change_type: Modified }

6. Claude Code runs a shell command:
   - HarnessEvent::ShellExecution { command: "cargo build", exit_code: 0 }
   - HarnessEvent::ShellExecution { command: "cargo test", exit_code: 0 }

7. Claude Code completes:
   - HarnessEvent::Completed with summary of changes.
   - Kernel records artifacts: file diffs, test results, build log.
   - UsageRecord: 5,000 input tokens, 2,000 output tokens.

8. Session remains Active for the next request.
   The developer can disconnect and resume later.
   If the process dies, the harness detects this on the next health
   check and reports SessionState::Error.
```

**What stays common:** Run/Turn lifecycle, artifact recording, usage tracking,
session identity, workspace binding evidence.

**What is adapter-specific:** Claude Code's stdin/stdout protocol, process
management, internal tool behavior, resume token format.

### 13.3 Example 3: Remote agent requesting a policy-gated tool

**Scenario:** A remote ACP-compatible agent service wants to submit a
governance vote, which requires policy evaluation and approval.

```text
1. External agent sends an ACP request to Polkagent's framework bridge:
   "Vote Aye on referendum 42 with conviction x1"
   The bridge authenticates the agent's identity and creates a Run.

2. The run's active skill is "polkagent.polkadot.governance":
   - Required tools: chain.query, chain.decode, chain.simulate, chain.submit
   - Required capabilities: tool_calls

3. Router selects model; executor runs inference:
   - Model produces a structured intent:
     { action: "governance.vote", referendum: 42, vote: "aye",
       conviction: 1, account: "5GrwvaEF..." }

4. Kernel validates the intent into a ChainIntent:
   - ChainClient resolves the chain profile (Polkadot Hub).
   - chain.decode verifies the referendum exists and is votable.
   - chain.simulate dry-runs the vote call.
   - All evidence is recorded as artifacts with metadata hash.

5. PolicyEvaluator evaluates the grant:
   - Agent spec allows governance.vote actions.
   - Deployment policy requires per-action approval for governance.
   - ResolvedGrant: permitted but approval-required.
   - PolicyDecision: RequireApproval { reason: "governance action" }

6. Approval flow:
   - Kernel creates an ApprovalRequest with the canonical action card:
     "Vote Aye on Referendum 42 (conviction x1) from account 5Grw...
      Estimated deposit: 0.1 DOT. Network: Polkadot Hub."
   - Approval request is delivered to the agent owner's inbox.
   - Owner reviews the card (rendered from artifacts, not model text).
   - Owner approves.

7. Signing and submission:
   - EffectIntent::RequestSignature created with canonical payload.
   - Signer signs the exact payload (never sees model text).
   - EffectIntent::Broadcast submits the signed transaction.
   - EffectIntent::ObserveFinality watches for inclusion and finality.

8. Outcome:
   - EffectOutcome::Success with transaction hash and block number.
   - Or EffectOutcome::Unknown if finality is not observed within deadline.
   - Receipt artifact links: intent -> decode -> simulation -> approval
     -> signature -> broadcast -> finality.

9. The ACP bridge returns the result to the remote agent.
   Usage is recorded against the agent's tenant and budget.
```

**What stays common:** Intent validation, policy evaluation, approval flow,
signer isolation, effect lifecycle, artifact lineage, finality semantics.

**What is adapter-specific:** ACP protocol parsing, bridge authentication,
remote agent identity verification.

---

## 14. Rust trait and DTO sketches (complete)

This section consolidates the key traits that define the execution layer's
public contracts. Implementation crates depend on these traits; the kernel
depends on nothing provider- or harness-specific.

### 14.1 Crate organization

```text
polkagent-provider-api     ModelProvider, Executor, ModelCatalog, ModelRouter traits
polkagent-harness-api      HarnessService, HarnessEventSink traits
polkagent-tool-api         ToolHost, ToolSchema, ToolResult types
polkagent-skill-api        SkillManifest, SkillResolver traits
polkagent-execution        ExecuteRequest, ExecutionResult, streaming types

polkagent-provider-anthropic   Anthropic Messages API adapter
polkagent-provider-openai      OpenAI Chat Completions adapter
polkagent-provider-openai-compat  Generic OpenAI-compatible adapter
polkagent-provider-google      Google Gemini adapter
polkagent-provider-ollama      Ollama local adapter
polkagent-provider-openrouter  OpenRouter gateway adapter

polkagent-harness-claude-code  Claude Code CLI runner
polkagent-harness-codex        Codex CLI runner
polkagent-harness-opencode     OpenCode runner
polkagent-harness-acp          ACP framework bridge
polkagent-harness-a2a          A2A framework bridge

polkagent-tool-file            File operation tools
polkagent-tool-shell           Shell execution tools
polkagent-tool-chain           Chain interaction tools
polkagent-tool-mcp             MCP tool bridge
```

### 14.2 Core DTOs

```rust
// --- Identifiers ---

/// Newtype wrappers for type safety.
struct ProviderId(String);
struct ModelId { id: String, version: Option<String>, family: ModelFamily }
struct HarnessId(String);
struct SessionId(String);
struct ToolId(String);
struct SkillId(String);
struct RunId(Uuid);
struct AttemptId(Uuid);
struct ArtifactId(Uuid);
struct McpServerId(String);

// --- Configuration ---

/// Top-level execution configuration for an agent.
struct AgentExecutionConfig {
    /// Default model route.
    default_model: ModelRoute,
    /// Fallback chain.
    fallback: Option<FallbackChain>,
    /// Available harnesses.
    harnesses: Vec<HarnessConfig>,
    /// MCP server configurations.
    mcp_servers: Vec<McpServerConfig>,
    /// Tool policy (which tools are enabled/disabled).
    tool_policy: ToolPolicy,
    /// Budget limits.
    budget: Option<Budget>,
    /// Retry policy.
    retry: RetryPolicy,
}

struct HarnessConfig {
    /// Harness identifier.
    id: HarnessId,
    /// Harness type and connection details.
    kind: HarnessKind,
    /// Default workspace binding.
    workspace: Option<WorkspaceBinding>,
    /// Maximum concurrent sessions.
    max_sessions: Option<u32>,
    /// Health check interval.
    health_interval: Option<Duration>,
}

struct ToolPolicy {
    /// Default policy for tools not explicitly listed.
    default: ToolPolicyAction,
    /// Per-tool overrides.
    overrides: HashMap<ToolId, ToolPolicyAction>,
    /// Per-category overrides.
    category_overrides: HashMap<ToolCategory, ToolPolicyAction>,
}

enum ToolPolicyAction {
    /// Tool is allowed without additional approval.
    Allow,
    /// Tool requires explicit approval for each invocation.
    RequireApproval,
    /// Tool is denied.
    Deny,
}
```

### 14.3 Event sink traits

```rust
/// Receives normalized events from an executor.
#[async_trait]
pub trait ExecutorEventSink: Send + Sync {
    async fn emit(&self, event: ExecutorEvent) -> Result<(), SinkError>;
}

/// Receives normalized events from a harness.
#[async_trait]
pub trait HarnessEventSink: Send + Sync {
    async fn emit(&self, event: HarnessEvent) -> Result<(), SinkError>;
}

/// Receives run-level progress events for client delivery.
#[async_trait]
pub trait RunProgressSink: Send + Sync {
    async fn emit(&self, event: RunProgressEvent) -> Result<(), SinkError>;
}
```

---

## 15. Contract tests and adapter onboarding

Adding a new provider, model, harness, or tool must be guided by contract
tests that verify compliance with the platform's expectations.

### 15.1 Provider contract tests

Every `ModelProvider` implementation must pass:

| Test | What it verifies |
|---|---|
| `test_health_returns` | `health()` returns a valid `ProviderHealth`. |
| `test_discover_models` | `discover_models()` returns at least one model with capabilities. |
| `test_simple_inference` | A basic text request returns a valid `ExecutionResult`. |
| `test_streaming` | Streaming events arrive in order and end with a terminal event. |
| `test_tool_calling` | If `tool_calls` is Supported, a request with tools produces `ToolUse` content. |
| `test_cancellation` | `cancel()` stops an in-progress execution within a bounded time. |
| `test_rate_limit_handling` | Rate limit errors are reported with `RetryAfter` information. |
| `test_auth_failure` | Invalid credentials produce `Authentication` error, not a panic. |
| `test_usage_reporting` | Every completed execution includes a `Usage` record. |
| `test_error_classification` | Known error types map to correct `RetryClass` values. |
| `test_large_context` | Requests near the context limit behave predictably. |
| `test_concurrent_requests` | Multiple simultaneous requests do not interfere. |

### 15.2 Harness contract tests

Every `HarnessService` implementation must pass:

| Test | What it verifies |
|---|---|
| `test_start_and_health` | Harness starts and reports healthy. |
| `test_session_create` | A session can be created and is in Active state. |
| `test_simple_execution` | A basic request produces events and completes. |
| `test_cancellation` | Cancel stops execution within bounded time. |
| `test_session_terminate` | Terminating a session releases resources. |
| `test_shutdown` | Shutdown terminates all sessions and the harness. |
| `test_workspace_binding` | If supported, file operations are confined to workspace. |
| `test_session_resume` | If supported, a session can be resumed after disconnect. |
| `test_health_after_failure` | Health reports accurately after an execution failure. |
| `test_concurrent_sessions` | If `max_sessions > 1`, multiple sessions work. |

### 15.3 Tool contract tests

Every tool implementation must pass:

| Test | What it verifies |
|---|---|
| `test_schema_valid` | Tool schema is valid JSON Schema. |
| `test_invoke_with_valid_input` | Valid input produces a valid result. |
| `test_invoke_with_invalid_input` | Invalid input produces a clear error, not a panic. |
| `test_grant_enforcement` | Invocation without required grant is denied. |
| `test_idempotency` | If `idempotent: true`, repeated calls produce the same result. |
| `test_classification` | Output classification matches the declared level. |

### 15.4 Adapter onboarding checklist

To add a new provider adapter:

1. Implement `ModelProvider` trait in a new `polkagent-provider-{name}` crate.
2. Implement the vendor-specific executor.
3. Map the vendor's error codes to `ExecuteError` variants.
4. Map the vendor's streaming format to `ExecutorEvent` variants.
5. Extract usage data from vendor responses.
6. Pass all provider contract tests.
7. Add configuration schema documentation.
8. Add integration tests with the vendor's API (may require credentials).
9. Document known capability limitations and differences from other providers.

To add a new harness adapter:

1. Implement `HarnessService` trait in a new `polkagent-harness-{name}` crate.
2. Define the I/O protocol (stdio, HTTP, WebSocket, etc.).
3. Map the harness's events to `HarnessEvent` variants.
4. Implement session management (create, resume, terminate).
5. Implement process/connection lifecycle (start, health, shutdown).
6. Pass all harness contract tests.
7. Document workspace binding and sandbox requirements.
8. Document which tools the harness manages internally vs. kernel-mediated.

---

## 16. ACP/A2A integration

### 16.1 Agent Client Protocol (ACP)

ACP provides a standard way for clients to interact with agent services. In
Polkagent, ACP serves dual roles:

**As a harness protocol:** Polkagent can connect to external ACP-compatible
agents through a framework bridge harness adapter.

**As an exposed API:** Polkagent can expose its own agents as ACP-compatible
services, allowing external clients to interact with them.

### 16.2 Agent-to-Agent (A2A) protocol

A2A (v1.0.1) is the inter-agent delegation standard, governed by the Linux
Foundation (donated in 2025) and adopted by 150+ organizations as of April
2026. It defines discovery and interaction through **Agent Cards** — structured
declarations of an agent's identity, capabilities, and endpoints.

The A2A specification defines four official extensions relevant to Polkagent:
- **Secure Passport** — agent identity and credential attestation.
- **Timestamp** — canonical request/response timestamping for audit trails.
- **Traceability** — end-to-end trace propagation across agent hops.
- **Agent Gateway Protocol** — routing A2A requests through gateway nodes.

In Polkagent, A2A enables:

- Multi-agent collaboration where specialized agents handle subtasks.
- External agent services requesting Polkagent capabilities.
- Polkagent agents delegating to external specialized agents.

Agent Cards from external agents are treated as untrusted declarations until
verified through the identity system (PRD-07). The Traceability extension
header is preserved in Polkagent's effect records to support cross-agent audit.

### 16.3 Adjacent agent commerce protocols

Two emerging standards are relevant to agent payment and commerce flows. They
are not part of the core execution model but are noted here for future harness
and bridge work.

**AP2 (Agentic Payment Protocol):** A Google-led initiative with 60+ partners
that represents payment intent as W3C Verifiable Credentials — specifically
Intent, Cart, and Payment Mandate objects. Agents express a desired purchase
or payment as a signed credential that can be verified and authorized by a
human or automated policy before execution. This aligns with Polkagent's
EffectIntent/approval pattern and may inform how agent-initiated payment
effects are modeled in a future PRD.

**x402 (HTTP 402 micropayment rail):** Originally from Coinbase and now
donated to the Linux Foundation, x402 enables stablecoin micropayments over
standard HTTP using the 402 Payment Required status. As of July 2026 the
network processes roughly 75 million transactions and $24 million in monthly
volume (CoinDesk). For Polkagent, x402 is a candidate transport mechanism for
metered tool access or agent service billing, particularly for on-chain payment
rails native to Polkadot. No integration is planned in the current phase; it
is documented here for awareness.

### 16.4 Protocol mapping

| ACP/A2A concept | Polkagent mapping |
|---|---|
| Task | Run |
| Message | Turn input/output |
| Artifact | Artifact |
| Streaming | RunProgressEvent via SSE |
| Authentication | Transport authentication + grant system |
| Agent Card | AgentSpec metadata projection |

### 16.5 Capability negotiation across protocols

When interacting with external agents through ACP/A2A, capability negotiation
must account for:

- The external agent's declared capabilities (from its Agent Card).
- Trust boundaries (the external agent is not trusted by default).
- Grant intersection (the combined operation cannot exceed either agent's
  grants).
- Protocol limitations (not all Polkagent features map to ACP/A2A concepts).

```rust
/// Configuration for ACP/A2A integration.
struct AgentProtocolConfig {
    /// Whether to expose this agent as an ACP service.
    expose_acp: bool,
    /// Whether to expose this agent as an A2A service.
    expose_a2a: bool,
    /// ACP-specific configuration.
    acp: Option<AcpConfig>,
    /// A2A-specific configuration.
    a2a: Option<A2aConfig>,
    /// Trust policy for external agents.
    external_agent_trust: ExternalAgentTrust,
}

enum ExternalAgentTrust {
    /// Deny all external agent requests.
    Deny,
    /// Allow from allowlisted agents only.
    Allowlist { agents: Vec<String> },
    /// Allow with grant intersection (default).
    GrantIntersection,
}
```

---

## 17. Acceptance criteria and verification checklist

### 17.1 Provider requirements

| Req ID | Requirement | Verification |
|---|---|---|
| PROV-01 | At least two direct API providers (Anthropic, OpenAI) pass all contract tests. | Contract test suite green. |
| PROV-02 | OpenAI-compatible adapter works with at least one non-OpenAI endpoint. | Integration test with Together AI or equivalent. |
| PROV-03 | Local provider adapter works with Ollama. | Integration test with local Ollama instance. |
| PROV-04 | Gateway adapter works with OpenRouter. | Integration test with OpenRouter. |
| PROV-05 | Secrets are resolved at execution time, never stored in artifacts or specs. | Secret-leakage test suite. |
| PROV-06 | Provider health and circuit breakers function correctly. | Fault injection tests. |
| PROV-07 | Provider errors map to correct retry classes. | Error classification test suite. |

### 17.2 Model requirements

| Req ID | Requirement | Verification |
|---|---|---|
| MOD-01 | Model catalog lists models from all configured providers. | Catalog integration test. |
| MOD-02 | Capability descriptors accurately reflect model abilities. | Capability probe tests per model. |
| MOD-03 | Capability negotiation correctly identifies missing capabilities. | Negotiation unit tests. |
| MOD-04 | Model routing selects appropriate models based on requirements. | Routing decision tests. |
| MOD-05 | Routing decisions are recorded and explainable. | Audit log inspection. |

### 17.3 Executor requirements

| Req ID | Requirement | Verification |
|---|---|---|
| EXEC-01 | Streaming events arrive in order with at-most-one terminal. | Stream ordering tests. |
| EXEC-02 | Tool calls are correctly parsed and returned. | Tool call round-trip tests. |
| EXEC-03 | Cancellation stops execution within 5 seconds. | Cancellation timing tests. |
| EXEC-04 | Usage is reported for every execution. | Usage completeness tests. |
| EXEC-05 | Errors are classified with correct retry signals. | Error handling tests. |

### 17.4 Harness requirements

| Req ID | Requirement | Verification |
|---|---|---|
| HARN-01 | Claude Code harness starts, executes, and terminates correctly. | Harness lifecycle test. |
| HARN-02 | Session resume works after disconnect (where supported). | Resume test with simulated disconnect. |
| HARN-03 | Workspace binding confines file access. | Sandbox escape test. |
| HARN-04 | Health checks detect harness failures. | Fault injection test. |
| HARN-05 | Normalized events are correct for all supported harness types. | Event normalization tests. |

### 17.5 Tool requirements

| Req ID | Requirement | Verification |
|---|---|---|
| TOOL-01 | Grant enforcement denies tools outside the effective grant. | Grant denial tests. |
| TOOL-02 | MCP tools are discovered and invocable through the tool host. | MCP integration test. |
| TOOL-03 | Tool output classification is enforced. | Classification tests. |
| TOOL-04 | Chain tools (decode, simulate) produce evidence artifacts. | Artifact lineage tests. |
| TOOL-05 | Signer tools are never exposed directly to model context. | Signer isolation test. |

### 17.6 Skill requirements

| Req ID | Requirement | Verification |
|---|---|---|
| SKIL-01 | Skill manifest validates correctly. | Manifest validation tests. |
| SKIL-02 | Skill installation does not grant capabilities. | Grant isolation test. |
| SKIL-03 | Skill versioning follows semver with correct compatibility. | Version resolution tests. |
| SKIL-04 | Skill test suites can be executed. | Test runner integration. |
| SKIL-05 | Skill dependencies are resolved correctly. | Dependency resolution tests. |

### 17.7 Cross-cutting requirements

| Req ID | Requirement | Verification |
|---|---|---|
| XCUT-01 | Usage is tracked at run, turn, and tenant levels. | Usage accounting tests. |
| XCUT-02 | Budget enforcement stops execution before exceeding limits. | Budget enforcement tests. |
| XCUT-03 | Retry policy respects the no-retry-after-side-effects constraint. | Side-effect retry tests. |
| XCUT-04 | Fallback chain respects the no-fallback-after-side-effects constraint. | Fallback constraint tests. |
| XCUT-05 | All adapter-specific data is recorded in effect attempts. | Effect audit tests. |
| XCUT-06 | Contract test suites exist and pass for every adapter type. | CI green for all adapter crates. |
| XCUT-07 | Adding a new provider does not require modifying core kernel crates. | Compilation test with mock provider in isolation. |

---

## 18. Implementation staging

The research findings integrated into this PRD suggest a staged delivery
sequence. This section records that staging for planning purposes.

### 18.1 Do now

- **Internal schema + 2–3 provider adapters.** Define the internal
  message/tool-call schema. Implement Anthropic, OpenAI, and Gemini direct
  adapters. Ship the OpenAI-compatible adapter for local and gateway
  coverage.
- **MCP Streamable HTTP.** Implement the Streamable HTTP transport as the
  default. Retain SSE support behind a compatibility flag but do not build
  new MCP integrations on it.

### 18.2 Validate next

- **Local serving.** Validate Ollama as the development default. Add
  llama.cpp/vLLM/TGI through the OpenAI-compatible adapter. Validate MLX on
  Apple-silicon hardware.
- **Circuit-breaker routing.** Implement the health-aware router described in
  section 12.3. Instrument routing decisions before tuning scoring weights.

### 18.3 Defer

- **Full harness matrix.** Claude Code is the priority harness. Codex,
  OpenCode, and custom CLIs share the same adapter contract and can be added
  incrementally. Do not attempt to support all CLIs simultaneously before the
  adapter contract is stable.

### 18.4 Avoid

- **Leaking provider quirks into the kernel.** All vendor-specific behavior
  (tool-call fragment assembly, `finish_reason` normalization, usage field
  differences) must be absorbed in adapter crates. Nothing in
  `polkagent-provider-api` or the kernel may import from a provider-specific
  crate. Validate this with compilation tests (XCUT-07).

---

## 19. Glossary

| Term | Definition |
|---|---|
| **ACP** | Agent Client Protocol: a standard for client-agent interaction. |
| **A2A** | Agent-to-Agent protocol (v1.0.1, Linux Foundation): a standard for inter-agent interaction via Agent Cards. |
| **Agent Card** | A structured declaration of an agent's identity, capabilities, and endpoints, as defined by A2A. |
| **AP2** | Agentic Payment Protocol: a Google-led standard representing payment intent as W3C Verifiable Credentials. |
| **Artifact** | Durable, attributable content or evidence. |
| **Budget** | Token, cost, or time limits on execution. |
| **Capability** | A specific ability (streaming, tool calls, etc.) that a model or provider may support. |
| **Circuit breaker** | A pattern that stops sending requests to a failing provider. |
| **Classification** | Data sensitivity level: Public, Private, Sensitive, SecretForbidden. |
| **EffectIntent** | A durable command recorded before external I/O. |
| **EffectAttempt** | One claim, lease, retry, and idempotency lifecycle for an intent. |
| **EffectOutcome** | The immutable result observed for one attempt. |
| **Executor** | An adapter that performs one inference request. |
| **Fallback** | An alternative model/provider used when the primary fails. |
| **Grant** | A resolved set of permissions and limits for a run or effect. |
| **Harness** | A long-lived agent process/service with its own lifecycle. |
| **MCP** | Model Context Protocol: a standard for tool and context integration. |
| **Model** | A particular AI model identity and capability set. |
| **Model catalog** | The registry of known models and their availability. |
| **Model route** | A resolved decision about which provider and model to use. |
| **PCA** | Polkadot Chat Agents: the existing reference chat-agent product. |
| **Provider** | A connection to a service that exposes one or more models. |
| **Retry class** | Classification of whether/how an error should be retried. |
| **Router** | The component that selects a model route for a request. |
| **Run** | One durable execution instance. |
| **Skill** | A versioned package of instructions, context, and requirements. |
| **SSE** | Server-Sent Events: a streaming protocol over HTTP. Deprecated as an MCP transport since spec 2025-03-26. |
| **Streamable HTTP** | The current MCP transport for remote servers; supports stateless, routable deployments. |
| **Tool** | A typed operation invoked under a grant. |
| **Tool host** | The kernel component that mediates tool invocations. |
| **Turn** | One inference request/response cycle within a run. |
| **Usage** | Token counts, cost, and duration for an execution. |
| **x402** | HTTP 402-based stablecoin micropayment rail (Linux Foundation); a candidate transport for metered agent services. |

---

*End of PRD-04. This document is a draft for review and must not be treated
as an accepted architecture decision or implementation commitment.*

---

## APPENDIX A: PROVIDER IMPLEMENTATION BLUEPRINT

### A.1 Provider Trait Definition

The complete `ModelProvider` trait, with all methods required for a conformant
adapter. Every leaf crate (`polkagent-provider-anthropic`, `-openai`, etc.)
implements this trait; the kernel and router never import provider-specific types.

```rust
use std::time::Duration;
use async_trait::async_trait;

/// The complete provider contract. Kernel code imports only this trait.
/// Provider-specific types never cross this boundary.
#[async_trait]
pub trait ModelProvider: Send + Sync + 'static {
    // ── Identity ────────────────────────────────────────────────────────

    /// Unique, stable provider identifier for routing and logging.
    fn provider_id(&self) -> &ProviderId;

    /// Human-readable display name.
    fn display_name(&self) -> &str;

    // ── Health ───────────────────────────────────────────────────────────

    /// Current health state. Called by the router and circuit breaker before
    /// every dispatch. Must be cheap — read from cached state, do not probe.
    async fn health(&self) -> ProviderHealth;

    /// Active probe: attempt a lightweight request (e.g., model list or a
    /// minimal chat completion). Used by the probe scheduler, not the hot path.
    async fn probe(&self) -> Result<ProbeResult, ProviderError>;

    // ── Discovery ────────────────────────────────────────────────────────

    /// List models currently available through this provider.
    /// Called during initial setup and periodic refresh.
    async fn discover_models(&self) -> Result<Vec<ModelDescriptor>, ProviderError>;

    /// Fetch detailed capabilities for a specific model.
    /// May return cached data if the provider does not expose a capability API.
    async fn model_capabilities(
        &self,
        model: &ModelId,
    ) -> Result<ModelCapabilities, ProviderError>;

    // ── Inference ────────────────────────────────────────────────────────

    /// Create an executor for one inference turn against `model`.
    /// The executor is consumed after one call to `execute()`.
    async fn executor(
        &self,
        model: &ModelId,
        config: &ExecutionConfig,
    ) -> Result<Box<dyn Executor>, ProviderError>;

    /// Convenience: execute a single turn directly (adapter may delegate to
    /// `executor()` internally). Used for probing and simple one-shot calls.
    async fn chat_completion(
        &self,
        model: &ModelId,
        request: ExecuteRequest,
    ) -> Result<ExecutionResult, ProviderError> {
        let executor = self.executor(model, &request.config).await?;
        let sink = NullSink;
        executor.execute(request, &sink).await.map_err(Into::into)
    }

    /// Streaming variant: create executor and begin streaming, emitting events
    /// through `sink`. The caller drives the event loop.
    async fn streaming_completion(
        &self,
        model: &ModelId,
        request: ExecuteRequest,
        sink: &dyn ExecutorEventSink,
    ) -> Result<ExecutionResult, ProviderError> {
        let executor = self.executor(model, &request.config).await?;
        executor.execute(request, sink).await.map_err(Into::into)
    }

    // ── Rate limits ──────────────────────────────────────────────────────

    /// Current rate-limit status. The router consults this before dispatching.
    fn rate_limit_status(&self) -> RateLimitStatus;

    // ── Usage ────────────────────────────────────────────────────────────

    /// Query usage for a time range. Not all providers expose this;
    /// return `Err(ProviderError::NotSupported)` if unavailable.
    async fn usage_query(
        &self,
        range: TimeRange,
    ) -> Result<Vec<UsageRecord>, ProviderError>;
}

/// Result of an active health probe.
pub struct ProbeResult {
    /// Whether the probe succeeded.
    pub healthy: bool,
    /// Round-trip latency of the probe.
    pub latency: Duration,
    /// Models that were confirmed available.
    pub confirmed_models: Vec<ModelId>,
    /// Any warning messages from the probe.
    pub warnings: Vec<String>,
}

/// Current rate-limit state for a provider.
pub struct RateLimitStatus {
    /// Requests allowed per minute (None = unknown).
    pub rpm_limit: Option<u32>,
    /// Requests used in the current window.
    pub rpm_used: Option<u32>,
    /// Tokens allowed per minute.
    pub tpm_limit: Option<u64>,
    /// Tokens used in the current window.
    pub tpm_used: Option<u64>,
    /// If rate-limited, when the window resets.
    pub reset_at: Option<Timestamp>,
}
```

**Error types:**

```rust
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// Network-level failure (DNS, TCP, TLS). May be retried immediately.
    #[error("transport error: {message}")]
    Transport { message: String, retryable: bool },

    /// HTTP 401/403: credentials invalid or expired. Do not retry; rotate key.
    #[error("authentication failed: {message}")]
    Authentication { message: String },

    /// HTTP 429: rate limit exceeded. Retry after `reset_at`.
    #[error("rate limit exceeded")]
    RateLimit { reset_at: Option<Timestamp> },

    /// HTTP 400 / context too large: request payload too big.
    #[error("request too large: {message}")]
    RequestTooLarge { message: String },

    /// The model ID is not served by this provider.
    #[error("model not available: {model}")]
    ModelNotAvailable { model: ModelId },

    /// The provider does not support a feature required by the request.
    #[error("capability not supported: {capability}")]
    CapabilityNotSupported { capability: String },

    /// HTTP 5xx or equivalent: provider-side failure. May be retried with backoff.
    #[error("provider internal error: {message}")]
    ProviderInternal { message: String },

    /// The provider returned a vendor-specific error code.
    #[error("provider error {code}: {message}")]
    VendorSpecific { code: String, message: String },

    /// Content was refused by the provider's safety filter.
    #[error("content policy: {message}")]
    ContentPolicy { message: String },

    /// Operation is not supported by this provider implementation.
    #[error("not supported")]
    NotSupported,
}

impl ProviderError {
    /// Classify this error for the retry system.
    pub fn retry_class(&self) -> RetryClass {
        match self {
            Self::Transport { retryable: true, .. } => RetryClass::RetryImmediate,
            Self::Transport { retryable: false, .. } => RetryClass::NoRetry,
            Self::RateLimit { reset_at } => RetryClass::RetryAfter {
                delay: reset_at
                    .and_then(|t| t.duration_until())
                    .unwrap_or(Duration::from_secs(60)),
            },
            Self::ProviderInternal { .. } => RetryClass::RetryAfter {
                delay: Duration::from_secs(5),
            },
            Self::Authentication { .. } => RetryClass::NoRetry,
            Self::ContentPolicy { .. } => RetryClass::NoRetry,
            Self::ModelNotAvailable { .. } => RetryClass::Fallback,
            _ => RetryClass::Unknown,
        }
    }
}
```

**Retry behavior:** Adapters do not retry internally. All retry decisions are
made by the outer retry/fallback system (section 11). An adapter reports the
error and its `RetryClass`; the caller decides whether to retry, fallback, or
fail. This keeps retry logic centralized and prevents double-retry.

---

### A.2 Provider Implementations

#### A.2.1 Anthropic Provider

**Authentication flow:**

```rust
// crate: polkagent-provider-anthropic
// file: src/auth.rs

/// Resolve the Anthropic API key at execution time via SecretResolver.
/// Never stores the key; calls the resolver on each executor creation.
pub async fn resolve_api_key(resolver: &dyn SecretResolver) -> Result<String, ProviderError> {
    resolver
        .resolve("ANTHROPIC_API_KEY")
        .await
        .map_err(|e| ProviderError::Authentication {
            message: format!("could not resolve ANTHROPIC_API_KEY: {e}"),
        })
}

/// Construct the Authorization header value.
pub fn auth_header(api_key: &str) -> (&'static str, String) {
    ("x-api-key", api_key.to_string())
}
```

**Request/response mapping:**

```rust
// Anthropic Messages API request builder.
// Internal types -> Anthropic wire format.
pub fn build_anthropic_request(req: &ExecuteRequest) -> AnthropicRequest {
    AnthropicRequest {
        model: req.model.id.clone(),
        max_tokens: req.config.max_tokens.unwrap_or(4096),
        messages: req.messages.iter().map(to_anthropic_message).collect(),
        system: req.system.clone(),
        tools: req.tools.iter().map(to_anthropic_tool).collect(),
        stream: req.config.stream,
        temperature: req.config.temperature,
        thinking: req.config.extended_thinking.as_ref().map(|t| AnthropicThinking {
            budget_tokens: t.budget_tokens,
            enabled: true,
        }),
        ..Default::default()
    }
}

/// Normalize Anthropic's stop_reason to kernel StopReason.
pub fn normalize_stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn"       => StopReason::EndTurn,
        "tool_use"       => StopReason::ToolUse,
        "max_tokens"     => StopReason::MaxTokens,
        "stop_sequence"  => StopReason::StopSequence,
        other            => StopReason::Error(ExecuteError::ProviderSpecific {
            code: other.to_string(),
            message: format!("unknown stop_reason: {other}"),
        }),
    }
}
```

**Streaming chunk parsing (SSE):**

```rust
/// Parse one Anthropic SSE event into zero or more ExecutorEvents.
pub fn parse_anthropic_sse_event(
    event_type: &str,
    data: &serde_json::Value,
) -> Vec<ExecutorEvent> {
    match event_type {
        "message_start" => {
            // Extract initial usage (input tokens) if present.
            let usage = data["message"]["usage"].as_object()
                .map(|u| Usage {
                    input_tokens: u["input_tokens"].as_u64().unwrap_or(0),
                    output_tokens: 0,
                    ..Default::default()
                });
            let mut events = vec![ExecutorEvent::Started {
                model: ModelId::from_str(&data["message"]["model"].as_str().unwrap_or("")),
                provider: ProviderId::anthropic(),
            }];
            if let Some(u) = usage {
                events.push(ExecutorEvent::UsageUpdate { usage: u });
            }
            events
        }
        "content_block_start" => vec![], // Buffer; type known here
        "content_block_delta" => {
            let delta = &data["delta"];
            match delta["type"].as_str() {
                Some("text_delta") => vec![ExecutorEvent::TextDelta {
                    delta: delta["text"].as_str().unwrap_or("").to_string(),
                }],
                Some("thinking_delta") => vec![ExecutorEvent::ThinkingDelta {
                    delta: delta["thinking"].as_str().unwrap_or("").to_string(),
                }],
                Some("input_json_delta") => {
                    let idx = data["index"].as_u64().unwrap_or(0);
                    vec![ExecutorEvent::ToolCallDelta {
                        tool_call_id: format!("idx-{idx}"),
                        name: None,
                        input_delta: delta["partial_json"].as_str().unwrap_or("").to_string(),
                    }]
                }
                _ => vec![],
            }
        }
        "message_delta" => {
            // Final usage report.
            let usage = Usage {
                input_tokens: 0,
                output_tokens: data["usage"]["output_tokens"].as_u64().unwrap_or(0),
                ..Default::default()
            };
            vec![ExecutorEvent::UsageUpdate { usage }]
        }
        "message_stop" => vec![], // Terminal is emitted from Completed
        _ => vec![],
    }
}
```

**Rate limiting and backoff:**

```rust
/// Extract Anthropic rate-limit headers from an HTTP response.
pub fn extract_rate_limit_headers(headers: &HeaderMap) -> RateLimitStatus {
    RateLimitStatus {
        rpm_limit: parse_header_u32(headers, "anthropic-ratelimit-requests-limit"),
        rpm_used:  parse_header_u32(headers, "anthropic-ratelimit-requests-used"),
        tpm_limit: parse_header_u64(headers, "anthropic-ratelimit-tokens-limit"),
        tpm_used:  parse_header_u64(headers, "anthropic-ratelimit-tokens-used"),
        reset_at:  parse_header_timestamp(headers, "anthropic-ratelimit-requests-reset"),
    }
}
```

**Cost tracking:**

```rust
/// Compute cost for an Anthropic response using catalog pricing.
pub fn compute_cost(usage: &Usage, pricing: &ModelPricing) -> Option<Cost> {
    let input_cost = pricing.input_per_million? * (usage.input_tokens as f64 / 1_000_000.0);
    let output_cost = pricing.output_per_million? * (usage.output_tokens as f64 / 1_000_000.0);
    let cache_read = usage.cache_read_tokens
        .and_then(|t| pricing.cached_input_per_million.map(|p| p * t as f64 / 1_000_000.0))
        .unwrap_or(0.0);
    Some(Cost {
        amount: input_cost + output_cost + cache_read,
        currency: pricing.currency.clone(),
        source: CostSource::Computed,
    })
}
```

**Error classification:**

```rust
pub fn classify_anthropic_error(status: u16, body: &serde_json::Value) -> ProviderError {
    let error_type = body["error"]["type"].as_str().unwrap_or("unknown");
    let message = body["error"]["message"].as_str().unwrap_or("").to_string();
    match (status, error_type) {
        (401, _)                   => ProviderError::Authentication { message },
        (403, _)                   => ProviderError::Authentication { message },
        (429, _)                   => ProviderError::RateLimit { reset_at: None },
        (400, "invalid_request_error") if message.contains("too long")
                                   => ProviderError::RequestTooLarge { message },
        (400, _)                   => ProviderError::VendorSpecific {
                                         code: error_type.to_string(), message },
        (529, _) | (503, _) | (500, _)
                                   => ProviderError::ProviderInternal { message },
        _                          => ProviderError::VendorSpecific {
                                         code: status.to_string(), message },
    }
}
```

#### A.2.2 OpenAI Provider

**Key differences from Anthropic:**

```rust
// OpenAI uses "choices[0].finish_reason" not "stop_reason"
pub fn normalize_openai_finish_reason(reason: &str) -> StopReason {
    match reason {
        "stop"            => StopReason::EndTurn,
        "tool_calls"      => StopReason::ToolUse,
        "length"          => StopReason::MaxTokens,
        "content_filter"  => StopReason::Error(ExecuteError::ContentPolicy {
            message: "content_filter".to_string(),
        }),
        other             => StopReason::Error(ExecuteError::ProviderSpecific {
            code: other.to_string(),
            message: format!("unknown finish_reason: {other}"),
        }),
    }
}

// OpenAI streaming: tool_calls arrive as index-keyed delta fragments.
// The adapter maintains per-index assembly buffers and emits
// ToolCallComplete only when the full call is assembled.
pub struct OpenAiToolCallAssembler {
    /// Per-index name buffer (populated from first delta).
    names: HashMap<u64, String>,
    /// Per-index argument accumulation buffer.
    args:  HashMap<u64, String>,
    /// Per-index tool-call ID (from first delta for that index).
    ids:   HashMap<u64, String>,
}

impl OpenAiToolCallAssembler {
    pub fn feed(&mut self, delta: &serde_json::Value) -> Vec<ExecutorEvent> {
        let mut events = vec![];
        if let Some(tool_calls) = delta["tool_calls"].as_array() {
            for tc in tool_calls {
                let idx = tc["index"].as_u64().unwrap_or(0);
                // Capture ID and name on first delta for this index.
                if let Some(id) = tc["id"].as_str() {
                    self.ids.insert(idx, id.to_string());
                }
                if let Some(name) = tc["function"]["name"].as_str() {
                    self.names.insert(idx, name.to_string());
                }
                // Accumulate argument fragment.
                if let Some(frag) = tc["function"]["arguments"].as_str() {
                    self.args.entry(idx).or_default().push_str(frag);
                }
                events.push(ExecutorEvent::ToolCallDelta {
                    tool_call_id: self.ids.get(&idx).cloned().unwrap_or_default(),
                    name: self.names.get(&idx).cloned(),
                    input_delta: tc["function"]["arguments"]
                        .as_str().unwrap_or("").to_string(),
                });
            }
        }
        events
    }

    /// Call when finish_reason == "tool_calls" to emit ToolCallComplete events.
    pub fn finalize(&mut self) -> Vec<ExecutorEvent> {
        self.ids.keys().copied().collect::<Vec<_>>().into_iter()
            .filter_map(|idx| {
                let id   = self.ids.get(&idx)?.clone();
                let name = self.names.get(&idx)?.clone();
                let raw  = self.args.get(&idx).cloned().unwrap_or_default();
                let input = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
                Some(ExecutorEvent::ToolCallComplete {
                    tool_call: ToolCallRequest { id, name, input },
                })
            })
            .collect()
    }
}
```

#### A.2.3 Google (Gemini) Provider

```rust
// Gemini uses a different message role vocabulary:
// "user" -> "user", "assistant" -> "model"
// Parts are typed: TextPart, InlineDataPart, FunctionCallPart, FunctionResponsePart

pub fn to_gemini_content(msg: &Message) -> GeminiContent {
    GeminiContent {
        role: match msg.role {
            Role::User      => "user",
            Role::Assistant => "model",
            Role::System    => "user", // Gemini uses system_instruction separately
        }.to_string(),
        parts: msg.content.iter().map(to_gemini_part).collect(),
    }
}

// Gemini function declarations use a different schema wrapper:
pub fn to_gemini_tool(tool: &ToolSchema) -> GeminiFunctionDeclaration {
    GeminiFunctionDeclaration {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.input_schema.clone(),
    }
}
```

#### A.2.4 OpenAI-Compatible Adapter

The generic adapter accepts a base URL and optional quirk flags. It detects
capability differences at probe time and records them per endpoint.

```rust
pub struct OpenAiCompatConfig {
    pub base_url: String,
    pub auth_header: Option<(String, String)>, // (header-name, value)
    /// Some endpoints return usage; others don't.
    pub emits_usage: bool,
    /// Some endpoints omit finish_reason on last chunk.
    pub finish_reason_may_be_null: bool,
    /// Whether tool_calls are supported by this endpoint.
    pub tool_calls_supported: bool,
    /// Model name mapping: our name -> endpoint name.
    pub model_name_map: HashMap<String, String>,
}
```

#### A.2.5 Local Provider (Ollama)

```rust
pub struct OllamaProvider {
    base_url: String, // default: http://localhost:11434
    http: reqwest::Client,
    health_cache: Arc<RwLock<ProviderHealth>>,
}

impl OllamaProvider {
    /// Check whether a model is downloaded and ready.
    pub async fn model_ready(&self, model: &str) -> bool {
        // GET /api/tags -> models[].name
        // Returns true if model appears in the list.
        todo!()
    }

    /// Pull a model if not present. Progress is streamed.
    pub async fn pull_model(&self, model: &str, sink: &dyn ExecutorEventSink) {
        // POST /api/pull with {"model": ..., "stream": true}
        // Each chunk: {"status": "...", "completed": N, "total": N}
        todo!()
    }
}
```

---

### A.3 Model Routing / Cascade

#### A.3.1 Cascade Router Logic

```rust
/// The CascadeRouter tries the preferred route first; falls back through
/// the fallback chain on specific error classes. It never falls back after
/// side effects have been recorded for the current run.
pub struct CascadeRouter {
    catalog: Arc<dyn ModelCatalog>,
    health:  Arc<dyn HealthRegistry>,
    pricing: Arc<dyn PricingRegistry>,
}

#[async_trait]
impl ModelRouter for CascadeRouter {
    async fn route(&self, req: &RouteRequest) -> Result<ModelRoute, RouteError> {
        // 1. If exact policy: validate preferred and return or fail.
        if matches!(req.policy, RoutingPolicy::Exact) {
            return self.route_exact(req).await;
        }

        // 2. Build candidate set: healthy providers for capable models.
        let candidates = self.build_candidates(req).await?;
        if candidates.is_empty() {
            return Err(RouteError::NoCandidates {
                reason: "no healthy provider serves a model with required capabilities".to_string(),
            });
        }

        // 3. Score candidates under the requested policy.
        let scored = self.score_candidates(candidates, req).await;

        // 4. Select best; record decision.
        let selected = scored.into_iter().next()
            .ok_or(RouteError::NoCandidates { reason: "scoring produced no result".to_string() })?;

        Ok(selected.route)
    }
}

impl CascadeRouter {
    async fn build_candidates(&self, req: &RouteRequest) -> Result<Vec<Candidate>, RouteError> {
        let models = self.catalog.find_capable(&req.required_capabilities).await;
        let mut candidates = vec![];
        for model in models {
            for provider_id in &model.providers {
                let health = self.health.get(provider_id).await;
                // Skip open circuit breakers.
                if matches!(health.circuit, CircuitState::Open { .. }) {
                    continue;
                }
                // Apply preference boost.
                let is_preferred = req.preferred_model.as_ref() == Some(&model.id)
                    && req.preferred_provider.as_ref() == Some(provider_id);
                candidates.push(Candidate {
                    model: model.id.clone(),
                    provider: provider_id.clone(),
                    health,
                    pricing: self.pricing.get(&model.id).await,
                    is_preferred,
                });
            }
        }
        Ok(candidates)
    }

    async fn score_candidates(
        &self,
        candidates: Vec<Candidate>,
        req: &RouteRequest,
    ) -> Vec<ScoredCandidate> {
        let mut scored: Vec<ScoredCandidate> = candidates.into_iter().map(|c| {
            let score = match &req.policy {
                RoutingPolicy::CostOptimized => {
                    let cost_score = c.pricing.as_ref()
                        .and_then(|p| p.input_per_million)
                        .map(|ppm| 1.0 / ppm.max(0.001))
                        .unwrap_or(0.5);
                    if c.is_preferred { cost_score * 1.5 } else { cost_score }
                }
                RoutingPolicy::LatencyOptimized => {
                    let latency_score = c.health.latency_p50
                        .map(|d| 1.0 / d.as_secs_f64().max(0.001))
                        .unwrap_or(1.0);
                    if c.is_preferred { latency_score * 1.5 } else { latency_score }
                }
                RoutingPolicy::CapabilityMaximized => {
                    // Prefer models with more capabilities declared.
                    1.0 + if c.is_preferred { 10.0 } else { 0.0 }
                }
                _ => if c.is_preferred { 10.0 } else { 1.0 },
            };
            ScoredCandidate { candidate: c, score }
        }).collect();
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }
}
```

#### A.3.2 Model Capability Matching

```rust
/// Returns true if `available` satisfies all `required` capabilities.
/// Unknown capabilities in `available` are treated as satisfied with a warning.
pub fn capabilities_satisfied(
    required: &ModelCapabilities,
    available: &ModelCapabilities,
) -> (bool, Vec<String>) {
    let mut warnings = vec![];
    let mut satisfied = true;

    macro_rules! check {
        ($field:ident) => {
            match (&required.$field, &available.$field) {
                (CapabilityState::Supported, CapabilityState::NotSupported) => {
                    satisfied = false;
                }
                (CapabilityState::Supported, CapabilityState::Unknown) => {
                    warnings.push(format!("{} is required but support is unknown", stringify!($field)));
                }
                _ => {}
            }
        };
    }

    check!(streaming);
    check!(tool_calls);
    check!(structured_output);
    check!(vision);
    check!(extended_thinking);
    check!(caching);

    (satisfied, warnings)
}
```

#### A.3.3 Provider Health Tracking

The health registry is updated by a background scheduler that calls `probe()`
on each provider at a configured interval. The router reads from this registry
on every dispatch — no synchronous I/O on the hot path.

```rust
pub struct HealthRegistry {
    states: DashMap<ProviderId, HealthStatus>,
    circuit_breakers: DashMap<ProviderId, CircuitBreaker>,
}

impl HealthRegistry {
    /// Record a successful request. May transition circuit from HalfOpen -> Closed.
    pub fn record_success(&self, provider: &ProviderId, latency: Duration) {
        self.states.entry(provider.clone()).and_modify(|h| {
            h.last_success = Some(Timestamp::now());
            h.latency_p50 = Some(latency); // simplified; use EWMA in practice
            h.error_rate = h.error_rate * 0.95; // decay
        });
        // Update circuit breaker.
        self.circuit_breakers.entry(provider.clone()).and_modify(|cb| cb.on_success());
    }

    /// Record a failure. May open the circuit breaker.
    pub fn record_failure(&self, provider: &ProviderId, error: &ProviderError) {
        self.states.entry(provider.clone()).and_modify(|h| {
            h.last_failure = Some((Timestamp::now(), error.to_string()));
            h.error_rate = h.error_rate * 0.95 + 0.05;
        });
        self.circuit_breakers.entry(provider.clone()).and_modify(|cb| cb.on_failure());
    }
}
```

---

## APPENDIX B: TOOL SYSTEM IMPLEMENTATION

### B.1 Tool Registration and Discovery

#### B.1.1 Tool Descriptor Schema

```rust
/// Complete tool descriptor. This is the canonical representation used
/// throughout the kernel. Provider adapters translate this into vendor-specific
/// tool/function schemas.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDescriptor {
    /// Namespaced tool ID. Convention: "domain.noun.verb"
    /// Examples: "polkagent.file.read", "polkagent.chain.decode"
    pub id: ToolId,
    /// Short name used in model context (snake_case, no namespace).
    pub name: String,
    /// Description shown to the model. Must be clear and unambiguous.
    /// Good: "Read the contents of a file at an absolute path."
    /// Bad: "Reads a file." (too brief) or a 500-word essay (too long).
    pub description: String,
    /// JSON Schema (draft-07) for the tool's input parameters.
    /// Must be an object schema with "required" and "properties".
    pub input_schema: serde_json::Value,
    /// JSON Schema for the tool's output. Optional; used for structured output validation.
    pub output_schema: Option<serde_json::Value>,
    /// Tool category (determines grant namespace and policy bucket).
    pub category: ToolCategory,
    /// Which effects this tool may produce.
    pub effects: Vec<ToolEffect>,
    /// Capabilities required to invoke this tool.
    pub required_capabilities: Vec<CapabilityRequirement>,
    /// Whether the tool is safe to call multiple times with the same inputs.
    pub idempotent: bool,
    /// Rough cost hint for pre-invocation budget checks.
    pub cost_hint: Option<CostHint>,
    /// Semantic version.
    pub version: String,
    /// Whether this tool may return sensitive data.
    pub output_classification: Classification,
    /// Timeout for one invocation.
    pub timeout: Duration,
}

/// Declared effect types for budget and audit purposes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ToolEffect {
    FilesystemRead,
    FilesystemWrite,
    ProcessExecution,
    NetworkRead,
    NetworkWrite,
    ChainRead,
    ChainWrite,   // irreversible; requires EffectIntent lifecycle
    SignerAccess, // high privilege
    MemoryRead,
    MemoryWrite,
    None,
}
```

#### B.1.2 Tool Sandboxing and Capability Declarations

Tools declare their capabilities statically in their descriptor. The kernel
validates these at registration time and enforces them at invocation time.

```rust
/// The kernel validates this at registration. A tool that declares
/// `ChainWrite` but does not also declare `SignerAccess` will be rejected
/// because chain writes always require signing.
pub fn validate_tool_descriptor(desc: &ToolDescriptor) -> Result<(), ToolRegistrationError> {
    // Chain writes must declare signer access.
    if desc.effects.contains(&ToolEffect::ChainWrite)
        && !desc.effects.contains(&ToolEffect::SignerAccess)
    {
        return Err(ToolRegistrationError::InconsistentEffects {
            message: "ChainWrite requires SignerAccess".to_string(),
        });
    }
    // JSON schema must be valid.
    jsonschema::JSONSchema::compile(&desc.input_schema)
        .map_err(|e| ToolRegistrationError::InvalidSchema { message: e.to_string() })?;
    Ok(())
}
```

#### B.1.3 Tool Invocation Protocol

```rust
/// Full invocation lifecycle with timeout and classification enforcement.
pub async fn invoke_tool(
    host: &dyn ToolHost,
    call: ToolCall,
    grant: &ResolvedGrant,
    context: &RequestContext,
) -> Result<ToolResult, ToolError> {
    // 1. Grant check.
    host.check_grant(&call.tool_id, grant)?;

    // 2. Schema validation.
    host.validate_input(&call.tool_id, &call.input)?;

    // 3. Invoke with timeout.
    let desc = host.descriptor(&call.tool_id)?;
    let result = tokio::time::timeout(
        desc.timeout,
        host.invoke(call.clone(), grant, context),
    )
    .await
    .map_err(|_| ToolError::Timeout { tool: call.tool_id.clone() })?
    .map_err(|e| ToolError::InvocationFailed { message: e.to_string() })?;

    // 4. Enforce output classification.
    let effective_class = result.classification.max(desc.output_classification);

    // 5. Record effect.
    context.effect_store.record_tool_invocation(&call, &result).await?;

    Ok(ToolResult { classification: effective_class, ..result })
}
```

#### B.1.4 Built-in vs Plugin Tools

| Category | Registration | Trust Level | Grant Required |
|---|---|---|---|
| Built-in (kernel) | Compiled in; always present | Trusted | Yes (via grant system) |
| MCP (stdio) | Discovered at server start | Semi-trusted | Yes + MCP grant scope |
| MCP (streamable HTTP) | Discovered at server start | Untrusted | Yes + MCP grant scope |
| Plugin (marketplace) | Installed by operator | Operator-verified | Yes |

Built-in tools are registered at startup via a static registry (pattern from
`roko-core/src/tool/registry.rs` `VecToolRegistry`). Plugin tools register
through the same `ToolHost::register()` interface.

---

### B.2 Skill Package Format

#### B.2.1 Directory Structure

A skill package is a directory (or archive) with the following layout:

```
my-skill-1.2.0/
├── skill.toml          # Manifest (required)
├── instructions.md     # System instructions injected when skill is active (required)
├── schemas/
│   ├── input.json      # JSON Schema for skill-level inputs (optional)
│   └── output.json     # JSON Schema for skill-level outputs (optional)
├── examples/
│   ├── 001-basic.yaml  # Example interaction (at least one required)
│   └── 002-edge.yaml   # Additional examples
├── tests/
│   ├── functional/
│   │   ├── test-001.yaml
│   │   └── test-002.yaml
│   ├── safety/
│   │   └── test-injection.yaml
│   └── fixtures/
│       └── extrinsic-sample.hex   # Fixture data for tests
├── context/
│   ├── polkadot-governance.md     # Reference material injected as context
│   └── referendum-format.json     # Structured reference data
└── templates/
    └── vote-intent.md  # Prompt template for the vote intent step
```

#### B.2.2 Manifest Schema (TOML)

```toml
# skill.toml

[skill]
id          = "polkagent.polkadot.governance"
name        = "Polkadot Governance"
description = """
Guides an agent through Polkadot OpenGov actions: explaining referenda,
preparing vote intents, and summarizing outcomes. Requires chain.query
and chain.decode tools; chain.simulate and chain.submit are optional
but enable dry-run validation before submission.
"""
version     = "1.2.0"
min_platform_version = "0.1.0"

[skill.provenance]
author      = "Polkadot Foundation"
homepage    = "https://github.com/polkadot-foundation/polkagent-skills"
license     = "Apache-2.0"

[skill.compatibility]
platform    = ">=0.1.0, <2.0.0"

  [[skill.compatibility.models]]
  family      = "Claude"
  min_context = 50000
  required    = ["tool_calls"]

  [[skill.compatibility.models]]
  family      = "Gpt"
  min_context = 32000
  required    = ["tool_calls"]

[[skill.required_tools]]
tool_id     = "polkagent.chain.query"
required    = true

[[skill.required_tools]]
tool_id     = "polkagent.chain.decode"
required    = true

[[skill.required_tools]]
tool_id     = "polkagent.chain.simulate"
required    = false   # Optional; enables dry-run before submission

[[skill.required_tools]]
tool_id     = "polkagent.chain.submit"
required    = false   # Optional; only needed if the skill will submit

[skill.cost_hints]
typical_input_tokens  = 8000
typical_output_tokens = 1500
typical_tool_calls    = 3
```

#### B.2.3 Version Resolution

The skill resolver applies semver logic with the following precedence:

1. **Pinned version** in the agent spec takes absolute priority: `"1.2.0"`.
2. **Range constraint** resolves to the highest compatible installed version
   matching the range: `">=1.0.0, <2.0.0"`.
3. **Latest** installs the highest non-pre-release version.

Lockfile (`skill.lock`) pins resolved versions after installation:

```toml
# skill.lock — do not edit by hand

[[locked]]
id      = "polkagent.polkadot.governance"
version = "1.2.0"
digest  = "sha256:a3f4..."
source  = "https://registry.polkagent.io/skills"
```

#### B.2.4 Dependency Management

```toml
# In skill.toml: declare dependencies on other skills
[[skill.compatibility.dependencies]]
skill_id = "polkagent.polkadot.chain-core"
version  = ">=1.0.0"
optional = false

[[skill.compatibility.dependencies]]
skill_id = "polkagent.polkadot.xcm"
version  = ">=2.0.0"
optional = true  # Enhances behavior if present; not required
```

The skill resolver performs a topological sort of dependencies and loads them
in order. Circular dependencies are rejected at install time.

#### B.2.5 Test Fixtures Per Skill

```yaml
# tests/functional/test-001.yaml
id: "test-explain-referendum"
description: "Agent correctly explains a Polkadot referendum"
category: functional
requires_model: false   # Can be run against a mock model

input:
  messages:
    - role: user
      content: "Explain referendum 42 on Polkadot."

mock_tool_responses:
  - tool: "polkagent.chain.query"
    input_match: { method: "referenda.referendumInfoFor", args: [42] }
    response: { status: "Ongoing", track: 1, ayes: "10000000000", nays: "5000000" }

assertions:
  - type: "contains_text"
    value: "referendum"
  - type: "tool_called"
    tool: "polkagent.chain.query"
  - type: "no_tool_called"
    tool: "polkagent.chain.submit"  # Must not submit without user action
```

---

## APPENDIX C: HARNESS SYSTEM

### C.1 Harness Lifecycle Management

```rust
/// Harness manager: owns all harness instances and their session pools.
pub struct HarnessManager {
    /// Registered harness configurations.
    configs: HashMap<HarnessId, HarnessConfig>,
    /// Running harness handles, one per HarnessId.
    handles: DashMap<HarnessId, Arc<dyn HarnessService>>,
    /// Active sessions, indexed by SessionId.
    sessions: DashMap<SessionId, HarnessSession>,
    /// Health registry for monitoring.
    health: Arc<HealthRegistry>,
}

impl HarnessManager {
    /// Spawn a harness: instantiate the adapter, start the process/connection,
    /// run initial health check, register in the health registry.
    pub async fn spawn(&self, harness_id: &HarnessId) -> Result<(), HarnessError> {
        let config = self.configs.get(harness_id)
            .ok_or(HarnessError::NotFound)?;
        let service: Arc<dyn HarnessService> = match &config.kind {
            HarnessKind::DirectRunner { command } => {
                Arc::new(DirectRunnerHarness::new(command, config)?)
            }
            HarnessKind::FrameworkBridge { protocol } => {
                Arc::new(FrameworkBridgeHarness::new(protocol, config).await?)
            }
            HarnessKind::CustomProcess { command } => {
                Arc::new(CustomProcessHarness::new(command, config)?)
            }
        };
        service.start(&HarnessStartConfig::from(config)).await?;
        let health = service.health().await;
        self.health.register_harness(harness_id, health);
        self.handles.insert(harness_id.clone(), service);
        Ok(())
    }

    /// Monitor: background task that health-checks all running harnesses.
    pub async fn run_health_monitor(self: Arc<Self>, interval: Duration) {
        loop {
            tokio::time::sleep(interval).await;
            for entry in self.handles.iter() {
                let (id, service) = (entry.key().clone(), entry.value().clone());
                let health = service.health().await;
                self.health.update_harness(&id, health);
            }
        }
    }

    /// Teardown: gracefully terminate all sessions, then shut down all harnesses.
    pub async fn teardown(&self) {
        // Terminate sessions first.
        let session_ids: Vec<SessionId> = self.sessions.iter()
            .map(|e| e.key().clone()).collect();
        for sid in session_ids {
            if let Some(sess) = self.sessions.get(&sid) {
                if let Some(svc) = self.handles.get(&sess.harness_id) {
                    let _ = svc.terminate_session(&sid).await;
                }
            }
        }
        // Shut down harnesses.
        for entry in self.handles.iter() {
            let _ = entry.value().shutdown().await;
        }
    }
}
```

### C.2 Sandbox Isolation

**Process-level isolation (direct runners):**

```rust
/// Sandbox configuration for a direct-runner child process.
pub struct ProcessSandbox {
    /// Allowed read paths (absolute).
    pub read_paths: Vec<PathBuf>,
    /// Allowed write paths (absolute).
    pub write_paths: Vec<PathBuf>,
    /// Denied paths (override; takes priority over allowed).
    pub denied_paths: Vec<PathBuf>,
    /// CPU limit (fraction of one core, e.g. 2.0 = 2 cores).
    pub cpu_limit: Option<f64>,
    /// Memory limit in bytes.
    pub memory_limit: Option<u64>,
    /// Maximum wall-clock time for the session.
    pub time_limit: Option<Duration>,
    /// Network policy.
    pub network: NetworkPolicy,
}

#[derive(Debug, Clone)]
pub enum NetworkPolicy {
    /// No network access.
    None,
    /// Outbound only to listed hosts/ports.
    Allowlist { hosts: Vec<String> },
    /// Full outbound access (developer mode only).
    FullOutbound,
}

/// Platform-specific sandbox enforcement.
///
/// On macOS: generates a Seatbelt (sandbox-exec) profile.
/// On Linux: applies seccomp-bpf + cgroup limits.
/// On other platforms: logs a warning and applies only process group limits.
pub fn apply_sandbox(child: &mut tokio::process::Child, sandbox: &ProcessSandbox) {
    // Platform dispatch omitted for brevity.
    // Reference: Polkagent sandbox crate (polkagent-sandbox).
    let _ = (child, sandbox);
}
```

**Container-level isolation (optional):**

For higher isolation requirements, the direct runner can launch the CLI inside
a container (Docker or podman). The harness adapter builds a container spec from
the workspace binding and runs the CLI as a container entrypoint.

### C.3 IPC Protocol Between Harness and Host

For direct runners (CLI processes), the IPC is stdin/stdout JSON lines.

```
Host -> Harness (stdin):
{"type":"request","id":"req-001","message":"Add weight to the call","context":{...}}

Harness -> Host (stdout, streaming):
{"type":"text_delta","id":"req-001","delta":"I'll examine the pallet source..."}
{"type":"tool_request","id":"req-001","call":{"tool":"file.read","args":{"path":"..."}}}
{"type":"file_change","id":"req-001","path":"src/lib.rs","change_type":"modified"}
{"type":"shell_execution","id":"req-001","command":"cargo test","exit_code":0}
{"type":"completed","id":"req-001","summary":"Added weight annotation to new_call."}
```

The harness adapter parses stdout line-by-line and translates each JSON object
into a `HarnessEvent`. Stderr is captured separately and emitted as `Warning`
events (unless it contains structured JSON, in which case it is parsed as well).

**Error handling:** If the child process exits unexpectedly, the adapter emits
`HarnessEvent::Error` and transitions the session to `SessionState::Error`.
The manager's health monitor detects this within one health-check interval and
marks the harness as degraded.

### C.4 Resource Limits and Monitoring

```rust
/// Resource limit state for one harness session.
/// Updated after every harness event that carries usage data.
pub struct SessionResourceState {
    pub session_id: SessionId,
    pub tokens_used: u64,
    pub cost_used_usd: f64,
    pub wall_time: Duration,
    pub budget: Budget,
}

impl SessionResourceState {
    /// Check all limits. Returns an error if any limit is exceeded.
    pub fn check(&self) -> Result<(), ResourceLimitError> {
        if let Some(max) = self.budget.max_tokens {
            if self.tokens_used >= max {
                return Err(ResourceLimitError::TokensExceeded {
                    used: self.tokens_used, limit: max
                });
            }
        }
        if let Some(ref max_cost) = self.budget.max_cost {
            if self.cost_used_usd >= max_cost.amount {
                return Err(ResourceLimitError::CostExceeded {
                    used: self.cost_used_usd, limit: max_cost.amount
                });
            }
        }
        if let Some(max_dur) = self.budget.max_duration {
            if self.wall_time >= max_dur {
                return Err(ResourceLimitError::DurationExceeded {
                    used: self.wall_time, limit: max_dur
                });
            }
        }
        Ok(())
    }
}
```

### C.5 Crash Recovery

```rust
/// Crash recovery policy for a harness session.
pub enum CrashRecovery {
    /// Do not restart; report error and require manual intervention.
    Fail,
    /// Restart up to `max_restarts` times with `backoff` delay.
    Restart { max_restarts: u32, backoff: Duration },
    /// Try to resume the session (requires harness support).
    Resume { timeout: Duration },
}

/// Called by the health monitor when a harness process exits unexpectedly.
pub async fn handle_harness_crash(
    manager: &HarnessManager,
    harness_id: &HarnessId,
    policy: &CrashRecovery,
) {
    match policy {
        CrashRecovery::Fail => {
            manager.mark_harness_failed(harness_id).await;
        }
        CrashRecovery::Restart { max_restarts, backoff } => {
            let mut attempts = 0;
            while attempts < *max_restarts {
                tokio::time::sleep(*backoff).await;
                if manager.spawn(harness_id).await.is_ok() {
                    return;
                }
                attempts += 1;
            }
            manager.mark_harness_failed(harness_id).await;
        }
        CrashRecovery::Resume { timeout } => {
            if tokio::time::timeout(*timeout, manager.try_resume(harness_id))
                .await
                .is_err()
            {
                manager.mark_harness_failed(harness_id).await;
            }
        }
    }
}
```

---

## APPENDIX D: IMPLEMENTATION CHECKLIST

Tasks are ordered by dependency. Each group must be complete before the next
group begins. Acceptance criteria are the contract tests from section 15.

### D.1 Core Traits (Week 1)

- [ ] **D.1.1** Define `ModelProvider` trait in `polkagent-provider-api`.
      Acceptance: compiles; no implementation required yet.
- [ ] **D.1.2** Define `Executor` + `ExecutorEventSink` traits.
      Acceptance: same.
- [ ] **D.1.3** Define all DTOs: `ExecuteRequest`, `ExecutionResult`,
      `ExecutorEvent`, `Usage`, `ModelDescriptor`, `ModelCapabilities`,
      `ToolSchema`, `ToolResult`.
      Acceptance: all types derive `Debug`, `Clone`, `serde::Serialize`,
      `serde::Deserialize`; no panics in unit tests.
- [ ] **D.1.4** Define `ToolHost` + `ToolRegistry` traits.
      Acceptance: compiles.
- [ ] **D.1.5** Define `HarnessService` + `HarnessEventSink` traits.
      Acceptance: compiles.
- [ ] **D.1.6** Define `SkillManifest` + `SkillResolver` traits.
      Acceptance: compiles.
- [ ] **D.1.7** Define `ModelRouter` + `ModelCatalog` traits.
      Acceptance: compiles.
- [ ] **D.1.8** Write a `MockProvider` in `polkagent-provider-api` tests that
      implements `ModelProvider` with a fixed response.
      Acceptance: mock passes the full provider contract test suite.
- [ ] **D.1.9** Write a `MockHarness` that implements `HarnessService`.
      Acceptance: mock passes full harness contract test suite.
- [ ] **D.1.10** Verify XCUT-07: adding `MockProvider` requires zero changes
      to core kernel crates.
      Acceptance: `cargo check -p polkagent-kernel` succeeds with mock in a
      separate crate.

### D.2 Anthropic Provider (Week 2)

- [ ] **D.2.1** Implement `polkagent-provider-anthropic` crate skeleton.
- [ ] **D.2.2** Implement `SecretResolver` integration: read `ANTHROPIC_API_KEY`
      at execution time. Never log or store the key.
      Acceptance: key does not appear in any artifact, log, or metric label.
- [ ] **D.2.3** Implement non-streaming `chat_completion`.
      Acceptance: round-trip test with a real Anthropic API key in CI (skipped
      if key absent).
- [ ] **D.2.4** Implement streaming (SSE parsing + `ExecutorEvent` emission).
      Acceptance: EXEC-01 (event ordering), EXEC-04 (usage reporting).
- [ ] **D.2.5** Implement tool call parsing (including `content_block_start`
      with `type: tool_use` + `input_json_delta` accumulation).
      Acceptance: EXEC-02 (tool call round-trip).
- [ ] **D.2.6** Map all known Anthropic error codes to `ProviderError` + `RetryClass`.
      Acceptance: PROV-07 (error classification).
- [ ] **D.2.7** Extract rate-limit headers and expose via `rate_limit_status()`.
      Acceptance: PROV-06 (circuit breaker integration).
- [ ] **D.2.8** Compute cost from usage + catalog pricing.
      Acceptance: `UsageRecord.cost` is present and non-zero for non-empty responses.
- [ ] **D.2.9** Pass all provider contract tests (section 15.1).
- [ ] **D.2.10** Add PROV-05 secret-leakage test: assert API key never appears
      in any artifact or log output.

### D.3 OpenAI Provider (Week 2–3)

- [ ] **D.3.1** Implement `polkagent-provider-openai` crate skeleton.
- [ ] **D.3.2** Implement OpenAI-format request builder
      (`messages`, `tools`, `stream`, `response_format`).
- [ ] **D.3.3** Implement streaming with `OpenAiToolCallAssembler`.
      Acceptance: tool call fragments assembled before `ToolCallComplete` emitted.
- [ ] **D.3.4** Map `finish_reason` to `StopReason`.
- [ ] **D.3.5** Map OpenAI error codes to `ProviderError`.
- [ ] **D.3.6** Pass all provider contract tests.

### D.4 OpenAI-Compatible Adapter (Week 3)

- [ ] **D.4.1** Implement `polkagent-provider-openai-compat` with configurable
      base URL and auth header.
- [ ] **D.4.2** Implement quirk flags: `emits_usage`, `finish_reason_may_be_null`,
      `tool_calls_supported`, `model_name_map`.
- [ ] **D.4.3** Integration test against local Ollama.
      Acceptance: PROV-03 (local provider works).
- [ ] **D.4.4** Integration test against OpenRouter.
      Acceptance: PROV-04 (gateway works).

### D.5 Tool System (Week 3–4)

- [ ] **D.5.1** Implement `polkagent-tool-file` crate: `file.read`, `file.write`,
      `file.edit`, `file.search`, `file.glob`, `file.list`.
      Acceptance: all tool contract tests pass (section 15.3).
- [ ] **D.5.2** Implement `polkagent-tool-shell`: `shell.execute`,
      `shell.background`.
      Acceptance: grant enforcement test; no execution outside sandbox.
- [ ] **D.5.3** Implement `polkagent-tool-chain` stubs: `chain.query`,
      `chain.decode`, `chain.simulate`. Leave `chain.submit` as a stub that
      always returns `EffectRequired`.
      Acceptance: TOOL-04 (evidence artifacts produced).
- [ ] **D.5.4** Implement `polkagent-tool-mcp`: MCP tool bridge for Streamable
      HTTP and Stdio transports.
      Acceptance: TOOL-02 (MCP tools discoverable and invocable).
- [ ] **D.5.5** Implement signer tool isolation: `signer.sign` and
      `signer.capabilities` are registered but never included in model context.
      Acceptance: TOOL-05.
- [ ] **D.5.6** Pass TOOL-01 (grant enforcement denies out-of-scope tools).
- [ ] **D.5.7** Pass TOOL-03 (output classification enforced).

### D.6 Skill System (Week 4–5)

- [ ] **D.6.1** Implement `SkillManifest` TOML parser with schema validation.
      Acceptance: SKIL-01.
- [ ] **D.6.2** Implement `SkillResolver`: load from filesystem, verify digest.
      Acceptance: digest mismatch returns error.
- [ ] **D.6.3** Implement version resolution with lockfile generation.
      Acceptance: SKIL-03.
- [ ] **D.6.4** Implement `SkillTest` runner (fixture-based, no model required).
      Acceptance: SKIL-04.
- [ ] **D.6.5** Implement dependency resolution with cycle detection.
      Acceptance: SKIL-05; circular dependency returns clear error.
- [ ] **D.6.6** Verify SKIL-02: installing a skill with `chain.submit` in its
      `required_tools` does not grant chain write access.

### D.7 Harness System (Week 5–6)

- [ ] **D.7.1** Implement `polkagent-harness-claude-code`: spawn Claude Code
      CLI, pipe stdin/stdout, parse JSON line events.
      Acceptance: HARN-01.
- [ ] **D.7.2** Implement session resume for Claude Code.
      Acceptance: HARN-02.
- [ ] **D.7.3** Implement workspace binding and sandbox enforcement.
      Acceptance: HARN-03 (no file access outside workspace root).
- [ ] **D.7.4** Implement health monitoring with crash recovery.
      Acceptance: HARN-04.
- [ ] **D.7.5** Implement `HarnessEvent` normalization for all event types
      (`TextDelta`, `ToolRequest`, `FileChange`, `ShellExecution`, `Completed`).
      Acceptance: HARN-05.
- [ ] **D.7.6** Pass all harness contract tests (section 15.2).

---

## APPENDIX E: REFERENCE FILE MAP

| Component | Roko Files | Bardo Files | Key Patterns |
|---|---|---|---|
| Provider trait | `crates/roko-agent/` (provider impls) | — | `#[async_trait]` trait with `Send + Sync + 'static`; mock impl in tests |
| Secret resolution | `crates/roko-core/src/secrets/resolve.rs` | `screens/command/config.rs` (REDACTED masking) | `SecretResolver` chain (env -> file -> vault -> prompt); secrets never logged |
| Tool registry | `crates/roko-core/src/tool/registry.rs` | — | `ToolRegistry` trait with `for_role()` filtering; `VecToolRegistry` for tests |
| Tool call types | `crates/roko-core/src/tool/call.rs` | — | Typed call/result structs; JSON schema validation |
| Tool role allowlist | `crates/roko-core/src/tool/role_allowlist.rs` | — | Per-role tool filtering before injection into model context |
| Prompt templates | `crates/roko-compose/src/templates/` | — | `RolePromptTemplate` trait; typed `*Input` structs; no filesystem I/O in template code |
| Prompt budgets | `crates/roko-core/src/templates/common.rs` | — | `PromptBudget` per role; `adaptive_budget_for()` scales with model context window |
| Token truncation | `crates/roko-compose/src/templates/mod.rs` | — | `truncate()` and `truncate_tail()` helpers; preserve newline boundaries |
| Resource accounting | `crates/roko-runtime/src/resource.rs` | — | `ResourceAccount` with `BudgetEntry<T>`; check before and after each inference |
| Auth / bearer tokens | `crates/roko-agent-server/src/auth/bearer.rs` | — | SHA-256 hash comparison; never store raw token; `REDACTED` in logs |
| Agent display | — | `screens/command/agents.rs` | `AgentRow`, `ArchetypeDisplay`, `SidecarStatus`; background refresh with `watch::Sender` |
| Skill display | — | `screens/command/skills.rs` | `SkillDisplay` with semver validation; confidence `[0.0, 1.0]`; bloodstain flag |
| Config editor | — | `screens/command/config.rs` | `ConfigSection` / `ConfigScreenState`; sensitive field masking with `mask_sensitive_value()` |
| Health dashboard | — | `screens/heartbeat_status.rs` | `HeartbeatStatusSnapshot`; accuracy sparkline via `VecDeque<f64>`; `BrailleSparkline` widget |
| Token/cost sparkline | `crates/roko-cli/src/tui/widgets/token_sparkline.rs` | — | `render_token_sparkline()`; `fmt_tokens()` + `fmt_rate()` formatters; tier-based color coding |
| Harness sidecar status | — | `screens/command/agents.rs` (`SidecarStatus`) | Connected/disconnected; `last_heartbeat: Option<Instant>`; `skill_count` |
| Cascade routing | — | — | Score candidates by policy (cost/latency/capability); exclude open circuits |
| Circuit breaker | — | — | `CircuitBreaker` with `Closed/Open/HalfOpen`; `failure_threshold`; `open_duration` |

---

## APPENDIX F: TUI SURFACE FOR PROVIDERS/MODELS

The Polkagent TUI follows Bardo's screen structure: a fixed navigation bar
selects screens; each screen has a dedicated `Arc<RwLock<*ScreenState>>` and
a background refresh task.

### F.1 Provider Health Dashboard

Reference: `apps/bardo-terminal/src/screens/heartbeat_status.rs` —
`HeartbeatStatusScreen` with `BrailleSparkline` for accuracy history.

```
 ┌────────────────────────────────────────────────────────────────────────────┐
 │ PROVIDERS / Health                                              [r] refresh │
 ├─────────────────────┬──────────────────────────────┬──────────────────────┤
 │ Provider            │ Status       │ Circuit        │ Latency p50  p99     │
 ├─────────────────────┼──────────────┼────────────────┼──────────────────────┤
 │ anthropic           │ healthy      │ closed         │   320ms    850ms     │
 │ openai              │ healthy      │ closed         │   410ms   1200ms     │
 │ openrouter          │ degraded     │ half-open  [!] │   620ms   3100ms     │
 │ ollama-local        │ healthy      │ closed         │    45ms     80ms     │
 │ together-ai         │ unhealthy    │ open (2m 30s)  │      —       —       │
 └─────────────────────┴──────────────┴────────────────┴──────────────────────┘
 │ Error rate ████░░░░░░░░ 8%    Last probe: 12s ago    Models available: 24  │
 │                                                                             │
 │ Latency (anthropic, last 80 probes):                                        │
 │ ⣾⣿⣿⣾⣿⡿⣷⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⡿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ 320ms avg  │
 └─────────────────────────────────────────────────────────────────────────────┘
```

**Implementation sketch:**

```rust
pub struct ProviderHealthScreen {
    pub providers: Vec<ProviderHealthRow>,
    pub selected_idx: usize,
    pub latency_history: HashMap<ProviderId, VecDeque<Duration>>,
    pub last_refresh: Instant,
    pub focused: bool,
}

#[derive(Debug, Clone)]
pub struct ProviderHealthRow {
    pub provider_id: ProviderId,
    pub display_name: String,
    pub health_state: HealthState,
    pub circuit_state: CircuitState,
    pub latency_p50: Option<Duration>,
    pub latency_p99: Option<Duration>,
    pub error_rate: f64,
    pub last_probe_age: Duration,
    pub model_count: usize,
}

/// Spawn a background task that refreshes provider health every 5 seconds.
pub fn spawn_provider_health_refresh(
    tx: tokio::sync::watch::Sender<Vec<ProviderHealthRow>>,
    registry: Arc<HealthRegistry>,
) {
    tokio::spawn(async move {
        let interval = Duration::from_secs(5);
        loop {
            tokio::time::sleep(interval).await;
            let rows = registry.all_providers().into_iter()
                .map(ProviderHealthRow::from_registry)
                .collect();
            if tx.send(rows).is_err() { break; }
        }
    });
}
```

---

### F.2 Model Selection and Configuration UI

```
 ┌────────────────────────────────────────────────────────────────────────────┐
 │ MODELS / Catalog                                         [/] search [Enter] │
 ├─────────────────────────┬───────────┬──────────┬──────────┬────────────────┤
 │ Model                   │ Provider  │ Context  │ $/1M in  │ Capabilities   │
 ├─────────────────────────┼───────────┼──────────┼──────────┼────────────────┤
 │> claude-opus-4-6        │ anthropic │  200 000 │   $15.00 │ T V S C E M    │
 │  claude-sonnet-4-6      │ anthropic │  200 000 │    $3.00 │ T V S C   M    │
 │  gpt-4o                 │ openai    │  128 000 │    $5.00 │ T V S         │
 │  gemini-2.5-pro         │ google    │  1 000 000│    $3.50 │ T V S G       │
 │  llama-3.3-70b          │ ollama    │   32 000 │    $0.00 │ T             │
 └─────────────────────────┴───────────┴──────────┴──────────┴────────────────┘
 │ Capability key:  T=tool_calls  V=vision  S=streaming  C=caching            │
 │                  E=extended_thinking  M=mcp  G=grounding                   │
 │                                                                             │
 │ Selected: claude-opus-4-6 via anthropic                                     │
 │ Max output: 8192 tokens  │  Cache: supported  │  Status: available          │
 └─────────────────────────────────────────────────────────────────────────────┘
```

**Key bindings:** `j/k` navigate; `/` opens search; `Enter` selects model as
default; `c` opens capability detail panel; `r` refreshes catalog from providers.

---

### F.3 Skill Browser and Installer

Reference: `apps/bardo-terminal/src/screens/command/skills.rs` —
`SkillsScreen` with `SkillDisplay` (name, version, confidence, invocation count,
bloodstain status).

```
 ┌────────────────────────────────────────────────────────────────────────────┐
 │ SKILLS / Library                              installed: 8  stained: 1     │
 ├──────────────────────────────┬─────────┬──────────┬──────────┬────────────┤
 │ Skill                        │ Version │ Category │ Invoked  │ Status     │
 ├──────────────────────────────┼─────────┼──────────┼──────────┼────────────┤
 │> polkadot.governance         │ 1.2.0   │ chain    │      142 │ active     │
 │  polkadot.chain-core         │ 2.0.1   │ chain    │      891 │ active     │
 │  polkadot.xcm                │ 1.0.3   │ chain    │       47 │ active     │
 │  polkadot.explain-extrinsic  │ 1.1.0   │ chain    │      234 │ active     │
 │  coding.rust-conventions     │ 3.0.0   │ coding   │     1203 │ active     │
 │  coding.substrate-pallet     │ 2.2.0   │ coding   │      567 │ ⚔ STAINED  │
 │  meta.task-planning          │ 1.0.0   │ meta     │       89 │ active     │
 │  meta.self-evaluation        │ 0.9.1   │ meta     │       34 │ active     │
 └──────────────────────────────┴─────────┴──────────┴──────────┴────────────┘
 │ [i] install  [u] update  [r] remove  [t] run tests  [Enter] detail         │
 │                                                                             │
 │ polkadot.governance 1.2.0  — Polkadot Foundation                            │
 │ Tools: chain.query (req), chain.decode (req), chain.simulate (opt)          │
 │ Models: Claude >=50k tokens, GPT >=32k tokens                               │
 └─────────────────────────────────────────────────────────────────────────────┘
```

**Skill state struct (extends Bardo pattern):**

```rust
pub struct SkillDisplay {
    pub name: String,
    pub version: String,          // semver, validated by semver_valid()
    pub category: String,
    pub confidence: f64,          // [0.0, 1.0]
    pub invocation_count: u64,
    pub is_bloodstained: bool,    // produced a loss/failure event
    pub required_tools: Vec<String>,
    pub optional_tools: Vec<String>,
    pub status: SkillStatus,
}

pub enum SkillStatus {
    Active,
    Updating { from: String, to: String },
    Error { message: String },
    Bloodstained,
}
```

---

### F.4 Agent Configuration Editor

Reference: `apps/bardo-terminal/src/screens/command/config.rs` —
`ConfigScreenState` with `ConfigSection` groups, inline editing, and
`mask_sensitive_value()` for sensitive fields.

```
 ┌────────────────────────────────────────────────────────────────────────────┐
 │ CONFIG / Agent                                    [Enter] edit  [s] save   │
 ├──────────────────────────────────────────────────────────────────────────┤
 │ [providers]                                                                 │
 │   anthropic.api_key          ***REDACTED***                                 │
 │   anthropic.model            claude-opus-4-6                                │
 │   openai.api_key             ***REDACTED***                                 │
 │   openai.model               gpt-4o                                         │
 │                                                                             │
 │ [routing]                                                                   │
 │   policy                     cost_optimized                                 │
 │   preferred_provider         anthropic                                      │
 │   fallback_enabled           true                                           │
 │                                                                             │
 │ [budget]                                                                    │
 │   max_cost_usd               5.00                                           │
 │   max_tokens                 500000                                         │
 │   max_duration_seconds       300                                            │
 │                                                                             │
 │ [harness.claude_code]                                                       │
 │>  command                    claude                                          │
 │   max_sessions               3                                              │
 │   workspace_root             /home/dev/projects                             │
 └──────────────────────────────────────────────────────────────────────────┘
```

Sensitive field names (`api_key`, `token`, `secret`, `password`) are masked
with `***REDACTED***` in the rendered view. Editing a masked field opens an
inline buffer that writes to the secret store, not to the config file.

---

### F.5 Real-Time Token/Cost Tracking

Reference: `crates/roko-cli/src/tui/widgets/token_sparkline.rs` —
`render_token_sparkline()` with `fmt_tokens()`, `fmt_rate()`, tier-color coding,
and a braille sparkline for the token burn rate over time.

```
 ┌──────────────────────────────────┐  ┌──────────────────────────────────┐
 │ Token Burn                        │  │ Cost Tracker                      │
 │ tokens 124.3k  cost $0.48         │  │ run  $0.48  session  $1.23        │
 │ avg/task 8.2k   rate 12.1k/min    │  │ budget $5.00  remaining $3.77     │
 │                                   │  │                                   │
 │ ⣾⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿ │  │ ████████████████░░░░░░░░░ 75.4%  │
 │ succ 94%   T0 ██ T1 ███ T2 █     │  │                                   │
 └──────────────────────────────────┘  └──────────────────────────────────┘
```

**Implementation sketch:**

```rust
pub struct TokenCostTrackerState {
    /// Token counts per turn, newest last.
    pub token_series: VecDeque<u64>,
    /// Cost per turn, newest last.
    pub cost_series: VecDeque<f64>,
    /// Cumulative tokens for this run.
    pub total_tokens: u64,
    /// Cumulative cost for this run (USD).
    pub total_cost_usd: f64,
    /// Budget limit (USD).
    pub budget_usd: f64,
    /// Model tier distribution (T0=haiku, T1=sonnet, T2=opus).
    pub tier_counts: HashMap<String, u64>,
}

pub fn render_token_tracker(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &TokenCostTrackerState,
) {
    // Mirrors roko-cli render_token_sparkline pattern:
    // 1. Summary line: tokens, cost, avg/task, rate.
    // 2. Braille sparkline of token_series over last N turns.
    // 3. Tier distribution bar.
    // 4. Budget consumption bar.
}
```

---

## APPENDIX G: CONFIGURATION GUIDE

### G.1 Provider Configuration (TOML)

Complete `polkagent.toml` provider block:

```toml
# polkagent.toml

# ── Providers ────────────────────────────────────────────────────────────────

[[providers]]
id           = "anthropic-primary"
name         = "Anthropic (primary)"
kind         = "direct"
vendor       = "anthropic"

[providers.endpoint]
url          = "https://api.anthropic.com"
api_version  = "2023-06-01"

[providers.credential]
# Reference to environment variable. Secret resolved at execution time.
# Never embed the raw key here.
env_var      = "ANTHROPIC_API_KEY"

[providers.rate_limits]
requests_per_minute  = 4000
tokens_per_minute    = 400000
# When the rate limit is hit, retry after this delay minimum.
min_retry_delay_secs = 1

[providers.health]
probe_interval_secs  = 60
failure_threshold    = 5
open_duration_secs   = 120
half_open_successes  = 2


[[providers]]
id           = "openai-primary"
name         = "OpenAI"
kind         = "direct"
vendor       = "openai"

[providers.endpoint]
url          = "https://api.openai.com/v1"

[providers.credential]
env_var      = "OPENAI_API_KEY"

[providers.rate_limits]
requests_per_minute = 10000
tokens_per_minute   = 2000000


[[providers]]
id           = "openrouter"
name         = "OpenRouter (gateway)"
kind         = "gateway"
gateway      = "openrouter"

[providers.endpoint]
url          = "https://openrouter.ai/api/v1"

[providers.credential]
env_var      = "OPENROUTER_API_KEY"

[providers.extra]
# OpenRouter-specific: set site URL for ranking.
http_referer = "https://polkagent.io"


[[providers]]
id           = "ollama-local"
name         = "Ollama (local)"
kind         = "local"
runtime      = "ollama"

[providers.endpoint]
url          = "http://localhost:11434"
# No credential needed for local providers.

[providers.health]
probe_interval_secs = 30
failure_threshold   = 3


# ── OpenAI-compatible custom endpoint example ────────────────────────────────

[[providers]]
id           = "together-ai"
name         = "Together AI"
kind         = "openai_compatible"

[providers.endpoint]
url          = "https://api.together.xyz/v1"

[providers.credential]
env_var      = "TOGETHER_API_KEY"

[providers.extra]
emits_usage              = true
tool_calls_supported     = true
finish_reason_may_be_null = false
```

---

### G.2 Environment Variables for API Keys

All secrets are resolved via the `SecretResolver` chain:
`environment -> secrets file -> vault -> interactive prompt`.

| Variable | Provider | Required |
|---|---|---|
| `ANTHROPIC_API_KEY` | Anthropic | Yes if using Anthropic |
| `OPENAI_API_KEY` | OpenAI | Yes if using OpenAI |
| `GOOGLE_API_KEY` or `GOOGLE_APPLICATION_CREDENTIALS` | Google Gemini | Yes if using Google |
| `OPENROUTER_API_KEY` | OpenRouter | Yes if using OpenRouter |
| `TOGETHER_API_KEY` | Together AI | Yes if using Together AI |
| `FIREWORKS_API_KEY` | Fireworks AI | Yes if using Fireworks |
| `GROQ_API_KEY` | Groq | Yes if using Groq |
| `POLKAGENT_SECRETS_FILE` | All | Optional path to secrets TOML file |
| `POLKAGENT_VAULT_ADDR` | All | Optional HashiCorp Vault address |

**Secrets file format** (`~/.polkagent/secrets.toml`):

```toml
# ~/.polkagent/secrets.toml
# Permissions: chmod 600

[keys]
ANTHROPIC_API_KEY = "sk-ant-..."
OPENAI_API_KEY    = "sk-..."
OPENROUTER_API_KEY = "sk-or-..."
```

**Resolution precedence** (highest to lowest):
1. Environment variable
2. `POLKAGENT_SECRETS_FILE` (TOML file)
3. HashiCorp Vault (`POLKAGENT_VAULT_ADDR` set)
4. Interactive prompt (CLI only; not available in daemon mode)

Secrets are **never** written to artifacts, configuration exports, or log
files. The `mask_sensitive_value()` pattern from
`bardo-terminal/src/screens/command/config.rs` is applied at every TUI render
point. The `SecretValue` type from `roko-core/src/secrets/resolve.rs` carries
`source` and `resolved_at_ms` metadata for audit.

---

### G.3 Model Aliases and Defaults

```toml
# polkagent.toml (continued)

[models]
# Alias -> canonical model ID.
# Allows changing the underlying model without updating agent specs.
default      = "claude-opus-4-6"
fast         = "claude-sonnet-4-6"
local        = "llama3.2:3b"       # Ollama model name
vision       = "claude-opus-4-6"
reasoning    = "claude-opus-4-6"   # extended_thinking enabled

# Override capabilities if the provider does not report them accurately.
[[models.overrides]]
id                    = "llama3.2:3b"
provider              = "ollama-local"
tool_calls_supported  = false       # llama 3.2 3B does not reliably support tools
max_context_tokens    = 128000
```

---

### G.4 Cost Budget Configuration

```toml
# polkagent.toml (continued)

[budget]
# Per-run limits.
max_cost_usd           = 5.00    # Hard stop at $5 per run
max_tokens             = 500000  # Hard stop at 500k total tokens
max_duration_seconds   = 300     # Hard stop at 5 minutes per run
max_inference_calls    = 50      # Hard stop at 50 model calls per run
max_tool_calls         = 200     # Hard stop at 200 tool invocations per run

# Warning thresholds (emit a warning event but do not stop).
warn_cost_usd          = 4.00
warn_tokens            = 400000

# Tenant/session budget (rolling 24h window).
[budget.session]
max_cost_usd_24h       = 50.00
max_tokens_24h         = 5000000
```

---

### G.5 Cascade Routing Rules

```toml
# polkagent.toml (continued)

[routing]
# Default policy: prefer the preferred model, fall back on failure.
policy                 = "cascade"  # cascade | cost_optimized | latency_optimized | exact

# Preferred model for new runs.
preferred_model        = "claude-opus-4-6"
preferred_provider     = "anthropic-primary"

# Fallback chain: tried in order when primary fails.
[[routing.fallbacks]]
model                  = "claude-sonnet-4-6"
provider               = "anthropic-primary"
trigger_on             = ["rate_limit", "transport"]
max_uses_per_run       = 3

[[routing.fallbacks]]
model                  = "gpt-4o"
provider               = "openai-primary"
trigger_on             = ["model_unavailable", "provider_internal"]
max_uses_per_run       = 2

[[routing.fallbacks]]
model                  = "llama3.2:70b"
provider               = "ollama-local"
trigger_on             = ["authentication", "model_unavailable"]
max_uses_per_run       = 5

# Routing constraints.
[routing.constraints]
# Never fall back after side effects (file writes, chain interactions).
# This is always enforced regardless of configuration.
no_fallback_after_side_effects = true

# Exclude open circuit breakers automatically.
exclude_open_circuits = true

# Minimum health score for a candidate (0.0–1.0; 0 = any healthy provider).
min_health_score      = 0.0
```

**Cascade routing algorithm summary:**

1. Check if the preferred `(model, provider)` pair has a closed circuit breaker.
   If healthy, use it.
2. If the preferred pair fails with a retryable error, apply the retry policy
   (exponential backoff, up to `max_retries`).
3. If retries are exhausted or the error is `Fallback`-class, walk the
   fallback list in order. Select the first entry whose `trigger_on` list
   matches the observed error class and whose circuit is closed.
4. If all fallbacks are exhausted, fail the run with
   `RouteError::AllCandidatesFailed`.
5. Record the full routing decision (selected route, alternatives considered,
   reason for each rejection) as a `RouteDecision` artifact on the run.

**Constraint:** Steps 3–5 only occur before the run has recorded any side
effects. Once a `ChainWrite`, `FilesystemWrite`, or `SignerAccess` effect
attempt has been recorded, the run must complete with the current model or
fail. A new model cannot safely interpret partial results from a different
model context.

---

*End of appendices. These sections extend PRD-04 with actionable implementation
detail. All code samples are Rust sketches intended to guide implementation;
they are not production-ready and require full type wiring, error handling, and
testing before use.*

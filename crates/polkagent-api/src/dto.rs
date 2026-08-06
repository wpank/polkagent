//! Request and response DTOs for the public REST API.
//!
//! All shapes follow PRD-14 §4. Every response includes the `version` field
//! (`"v1alpha1"`) and uses `snake_case` field names in JSON.
//!
//! # Pagination
//!
//! List responses use cursor-based pagination per PRD-14 §2.4:
//! ```json
//! { "data": [...], "cursor": { "next": "opaque-token", "has_more": true },
//!   "meta": { "page_size": 50 } }
//! ```

use std::{collections::HashMap, path::PathBuf};

use chrono::{DateTime, Utc};
use polkagent_core::agent::{AgentSpec, AgentState, ModelPreference, ResourceLimits};
use polkagent_core::run::RunState;
use polkagent_core::{AgentId, RunId};
use polkagent_interaction::{
    InteractionConfig, InteractionEventEnvelope, InteractionState, InteractionSummary,
    InteractionTarget, InteractionTurnId, TurnHandle, TurnSummary as InteractionTurnSummary,
};
use polkagent_marketplace::types::{ServiceAvailability, ServicePricing};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// API version string present in every response.
pub const API_VERSION: &str = "v1alpha1";

// ---------------------------------------------------------------------------
// Interactions — requests and responses
// ---------------------------------------------------------------------------

/// Request for `POST /interactions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateHttpInteractionRequest {
    /// Optional presentation title.
    pub title: Option<String>,
    /// Agent or automatic execution target.
    pub target: InteractionTarget,
    /// Absolute project directory supplied to the runtime.
    pub working_directory: PathBuf,
    /// Optional safe client name for correlation.
    pub client_name: Option<String>,
    /// Optional client-local session identity.
    pub client_session_id: Option<String>,
}

/// Query for `GET /interactions`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ListHttpInteractionsQuery {
    /// Page size; defaults to 50 and is capped at 100.
    pub limit: Option<u32>,
    /// Stable numeric offset into the durable recency ordering.
    pub offset: Option<u32>,
    /// Optional interaction lifecycle filter.
    pub state: Option<InteractionState>,
}

/// Request for `POST /interactions/{id}/prompt`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptHttpInteractionRequest {
    /// Optional caller-generated idempotency identity.
    pub turn_id: Option<InteractionTurnId>,
    /// Non-empty user prompt text.
    pub prompt: String,
    /// Absolute project directory supplied to the runtime.
    pub working_directory: PathBuf,
    /// Optional safe client name for correlation.
    pub client_name: Option<String>,
    /// Optional client-local session identity.
    pub client_session_id: Option<String>,
}

/// Atomic update accepted by `PUT /interactions/{id}/config`.
///
/// The HTTP surface intentionally exposes only the two options implemented by
/// the shared runtime. Unknown option tags and extra fields are rejected by
/// Serde instead of being silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "option",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum UpdateHttpInteractionConfigRequest {
    /// Replace the durable agent or automatic target.
    Target(InteractionTarget),
    /// Set a model override, or clear it with JSON `null`.
    Model(Option<String>),
}

/// Compatibility request for `PUT /interactions/{id}/target`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateHttpInteractionTargetRequest {
    /// New agent or automatic execution target.
    pub target: InteractionTarget,
}

/// Query for finite durable interaction event replay.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ReplayInteractionEventsQuery {
    /// Return events strictly after this durable sequence.
    pub after_sequence: Option<u64>,
    /// Restrict replay to one turn while preserving conversation sequences.
    pub turn_id: Option<InteractionTurnId>,
    /// Maximum events returned; defaults to 100 and is capped at 999.
    pub limit: Option<u32>,
}

/// Query for checkpoint-aware live interaction event streaming.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StreamInteractionEventsQuery {
    /// Resume strictly after this durable sequence when `Last-Event-ID` is
    /// absent.
    pub after_sequence: Option<u64>,
    /// Restrict delivery to one turn while retaining interaction-wide IDs.
    pub turn_id: Option<InteractionTurnId>,
}

/// Versioned projection for one interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpInteractionResponse {
    /// API version.
    pub version: String,
    /// Durable interaction projection.
    pub interaction: InteractionSummary,
}

/// Versioned interaction list response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListHttpInteractionsResponse {
    /// API version.
    pub version: String,
    /// Page of durable interactions.
    pub data: Vec<InteractionSummary>,
    /// Pagination cursor metadata.
    pub cursor: CursorInfo,
    /// Page metadata.
    pub meta: PageMeta,
}

/// Versioned turn-list response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListHttpInteractionTurnsResponse {
    /// API version.
    pub version: String,
    /// Turns in ordinal order.
    pub data: Vec<InteractionTurnSummary>,
}

/// Versioned response returned immediately after prompt durability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptHttpInteractionResponse {
    /// API version.
    pub version: String,
    /// Stable turn/run correlation.
    pub handle: TurnHandle,
}

/// Versioned response after cancellation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelHttpInteractionTurnResponse {
    /// API version.
    pub version: String,
    /// Current durable turn projection.
    pub turn: InteractionTurnSummary,
}

/// Supported durable interaction configuration exposed over HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpInteractionConfig {
    /// Effective durable agent target.
    pub target: InteractionTarget,
    /// Canonical model override, or `None` to inherit the selected agent model.
    pub model: Option<String>,
}

impl From<InteractionConfig> for HttpInteractionConfig {
    fn from(config: InteractionConfig) -> Self {
        Self {
            target: config.target,
            model: config.model,
        }
    }
}

/// Versioned response for interaction configuration reads and updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpInteractionConfigResponse {
    /// API version.
    pub version: String,
    /// Effective supported durable configuration.
    pub config: HttpInteractionConfig,
}

/// Compatibility response after a target-only update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateHttpInteractionTargetResponse {
    /// API version.
    pub version: String,
    /// Full effective durable interaction configuration.
    pub config: InteractionConfig,
}

/// Sequence checkpoint returned by finite durable event replay.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct InteractionReplayCheckpoint {
    /// Last sequence in this response, or the requested starting sequence.
    pub next_after_sequence: u64,
    /// Whether another durable page is immediately available.
    pub has_more: bool,
}

/// Versioned finite durable event replay response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayInteractionEventsResponse {
    /// API version.
    pub version: String,
    /// Ordered durable interaction events.
    pub data: Vec<InteractionEventEnvelope>,
    /// Cursor for the next replay request.
    pub checkpoint: InteractionReplayCheckpoint,
}

// ---------------------------------------------------------------------------
// Agents — requests
// ---------------------------------------------------------------------------

/// Request body for `POST /agents`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAgentRequest {
    /// Human-readable name for the agent.
    pub name: String,
    /// Identifier of the AI model this agent should call.
    ///
    /// Format: `"provider/model-id"`, e.g. `"anthropic/claude-opus-4-6"`.
    pub model: String,
    /// Optional description shown in UIs and logs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Names of tools the agent may invoke.
    #[serde(default)]
    pub tools: Vec<String>,
    /// System prompt template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    // ------------------------------------------------------------------
    // PRD-03 optional fields
    // ------------------------------------------------------------------
    /// Capabilities this agent declares (e.g. `"file.read"`, `"chain.query"`).
    ///
    /// Absent means the agent declares no capabilities and inherits none from
    /// the request.
    #[serde(default)]
    pub declared_capabilities: Vec<String>,
    /// Policy file references governing this agent's behaviour.
    #[serde(default)]
    pub policy_refs: Vec<String>,
    /// Runtime resource limits for runs executed by this agent.
    ///
    /// `null` / absent means all limits are inherited from the global config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_limits: Option<ResourceLimits>,
    /// Preferred model configuration overriding provider defaults.
    ///
    /// `null` / absent means the global provider defaults apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_preference: Option<ModelPreference>,
    /// Surfaces on which this agent should be exposed.
    #[serde(default)]
    pub surface_bindings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Agents — responses
// ---------------------------------------------------------------------------

/// Response body for a single agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// API version.
    pub version: String,
    /// Unique agent identifier.
    pub id: AgentId,
    /// Full agent specification.
    pub spec: AgentSpec,
    /// Current operational state.
    pub status: AgentState,
    /// When the agent was first created.
    pub created_at: DateTime<Utc>,
    /// When the agent spec was last updated.
    pub updated_at: DateTime<Utc>,
}

impl AgentResponse {
    /// Build an `AgentResponse` from a stored `AgentSpec`.
    #[must_use]
    pub fn from_spec(spec: AgentSpec) -> Self {
        let created_at = spec.created_at;
        let updated_at = spec.updated_at;
        let id = spec.id;
        Self {
            version: API_VERSION.to_owned(),
            id,
            spec,
            status: AgentState::Created,
            created_at,
            updated_at,
        }
    }
}

/// Response body for `GET /agents`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAgentsResponse {
    /// API version.
    pub version: String,
    /// Page of agents.
    pub data: Vec<AgentResponse>,
    /// Pagination cursor information.
    pub cursor: CursorInfo,
    /// Page metadata.
    pub meta: PageMeta,
}

// ---------------------------------------------------------------------------
// Runs — requests
// ---------------------------------------------------------------------------

/// Request body for `POST /agents/:agent_id/runs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunRequest {
    /// The prompt or structured input for this run.
    ///
    /// This is intentionally an opaque JSON value so that different agent
    /// types can define their own input schemas.
    pub input: serde_json::Value,
    /// Optional idempotency key (24-hour window).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

// ---------------------------------------------------------------------------
// Runs — responses
// ---------------------------------------------------------------------------

/// Response body for a single run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResponse {
    /// API version.
    pub version: String,
    /// Unique run identifier.
    pub id: RunId,
    /// The agent that owns this run.
    pub agent_id: AgentId,
    /// Current lifecycle state.
    pub status: RunState,
    /// The input supplied when the run was created.
    pub input: serde_json::Value,
    /// Number of turns completed.
    pub turns_completed: u32,
    /// When the run was created.
    pub created_at: DateTime<Utc>,
    /// When the run started executing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// When the run reached a terminal state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    /// Human-readable reason for the terminal state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
}

impl From<crate::run::RunRecord> for RunResponse {
    fn from(r: crate::run::RunRecord) -> Self {
        Self {
            version: API_VERSION.to_owned(),
            id: r.id,
            agent_id: r.agent_id,
            status: r.state,
            input: r.input,
            turns_completed: r.turns_completed,
            created_at: r.created_at,
            started_at: r.started_at,
            completed_at: r.completed_at,
            terminal_reason: r.terminal_reason,
        }
    }
}

/// Response body for `GET /runs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRunsResponse {
    /// API version.
    pub version: String,
    /// Page of runs.
    pub data: Vec<RunResponse>,
    /// Pagination cursor information.
    pub cursor: CursorInfo,
    /// Page metadata.
    pub meta: PageMeta,
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Cursor state returned with every list response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorInfo {
    /// Opaque token for the next page, or `null` when this is the last page.
    pub next: Option<String>,
    /// `true` when there are more items beyond this page.
    pub has_more: bool,
}

/// Metadata about the current page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageMeta {
    /// Number of items in this response.
    pub page_size: usize,
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

/// Response body for the health endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// `"ok"` or `"degraded"`.
    pub status: String,
    /// Optional human-readable detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

// ---------------------------------------------------------------------------
// System info
// ---------------------------------------------------------------------------

/// Response body for `GET /system/info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfoResponse {
    /// API version.
    pub version: String,
    /// Polkagent platform version (from `CARGO_PKG_VERSION`).
    pub platform_version: String,
    /// Seconds since the server process started.
    pub uptime_secs: u64,
    /// Brief config summary (non-secret fields only).
    pub config_summary: ConfigSummary,
}

/// A non-secret summary of the active configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSummary {
    /// The bind address from `config.api.bind_address`.
    pub bind_address: String,
    /// The database backend type (`"sqlite"` or `"postgres"`).
    pub database_backend: String,
    /// Maximum concurrent runs.
    pub max_concurrent_runs: u32,
}

// ---------------------------------------------------------------------------
// Error response (documented for OpenAPI; actual serialisation is in error.rs)
// ---------------------------------------------------------------------------

/// The standard error envelope returned on all non-2xx responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Nested error detail.
    pub error: ErrorDetail,
}

/// Detail inside [`ErrorResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetail {
    /// Machine-readable error code (e.g. `"AGENT_NOT_FOUND"`).
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
    /// Server-generated request correlation ID.
    pub request_id: String,
    /// UTC timestamp of the error.
    pub timestamp: String,
}

// ---------------------------------------------------------------------------
// Query parameter structs
// ---------------------------------------------------------------------------

/// Query parameters for `GET /runs`.
#[derive(Debug, Clone, Deserialize)]
pub struct ListRunsQuery {
    /// Filter by agent ID.
    pub agent_id: Option<AgentId>,
    /// Filter by run state.
    pub state: Option<RunState>,
    /// Maximum items per page (default 50, max 100).
    pub limit: Option<u32>,
    /// Cursor from the previous page.
    pub after: Option<RunId>,
}

/// Query parameters for `GET /agents`.
#[derive(Debug, Clone, Deserialize)]
pub struct ListAgentsQuery {
    /// Maximum items per page (default 50, max 100).
    pub limit: Option<u32>,
    /// Cursor from the previous page.
    pub after: Option<AgentId>,
}

// ---------------------------------------------------------------------------
// Turns — responses
// ---------------------------------------------------------------------------

/// Response body for a single turn summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnSummary {
    /// Turn sequence number within the run.
    pub sequence: u32,
    /// When the turn started.
    pub started_at: DateTime<Utc>,
    /// When the turn completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
}

/// Response body for `GET /runs/{id}/turns`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListTurnsResponse {
    /// API version.
    pub version: String,
    /// List of turn summaries.
    pub data: Vec<TurnSummary>,
}

// ---------------------------------------------------------------------------
// Artifacts — responses
// ---------------------------------------------------------------------------

/// Response body for a single artifact's metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactResponse {
    /// API version.
    pub version: String,
    /// Unique artifact identifier.
    pub id: String,
    /// Artifact kind tag (e.g. `"file"`, `"code"`).
    pub kind: String,
    /// Hash algorithm (e.g. `"blake3"`).
    pub algorithm: String,
    /// Hex-encoded content digest.
    pub digest_hex: String,
    /// Data classification (e.g. `"public"`, `"private"`).
    pub classification: String,
    /// The run that produced this artifact, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// When the artifact was created.
    pub created_at: String,
}

/// Response body for `GET /runs/{id}/artifacts` and list operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListArtifactsResponse {
    /// API version.
    pub version: String,
    /// List of artifact metadata.
    pub data: Vec<ArtifactResponse>,
}

/// Response body for `GET /artifacts/{id}/provenance`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceResponse {
    /// API version.
    pub version: String,
    /// Ordered lineage chain (oldest ancestor first).
    pub chain: Vec<ArtifactResponse>,
}

// ---------------------------------------------------------------------------
// Events (REST) — responses
// ---------------------------------------------------------------------------

/// Response body for a single stored event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventResponse {
    /// API version.
    pub version: String,
    /// The stored event record.
    pub data: serde_json::Value,
}

/// Response body for `GET /events`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListEventsResponse {
    /// API version.
    pub version: String,
    /// List of stored events.
    pub data: Vec<serde_json::Value>,
    /// Pagination cursor information.
    pub cursor: CursorInfo,
    /// Page metadata.
    pub meta: PageMeta,
}

/// Query parameters for `GET /events`.
#[derive(Debug, Clone, Deserialize)]
pub struct ListEventsQuery {
    /// Filter by run ID.
    pub run_id: Option<RunId>,
    /// Filter by event type string.
    pub event_type: Option<String>,
    /// Return events with `global_sequence >= since`.
    pub since: Option<u64>,
    /// Maximum items per page (default 50, max 200).
    pub limit: Option<u32>,
}

// ---------------------------------------------------------------------------
// Providers — responses
// ---------------------------------------------------------------------------

/// Response body for a single provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderResponse {
    /// API version.
    pub version: String,
    /// Stable provider identifier.
    pub id: String,
    /// Provider type (e.g. `"anthropic"`, `"openai_compatible"`).
    pub provider_type: String,
    /// Base URL for the provider API.
    pub base_url: String,
    /// Default model when none is specified.
    pub default_model: String,
    /// Per-request timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum automatic retries on transient failures.
    pub max_retries: u32,
}

/// Response body for `GET /providers`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListProvidersResponse {
    /// API version.
    pub version: String,
    /// Configured providers.
    pub data: Vec<ProviderResponse>,
}

// ---------------------------------------------------------------------------
// Memory — requests and responses
// ---------------------------------------------------------------------------

/// Request body for `POST /memory/query`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQueryRequest {
    /// The search query string.
    pub query: String,
    /// Maximum number of results to return.
    #[serde(default = "default_memory_limit")]
    pub limit: u32,
    /// Optional filter by memory type/namespace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

fn default_memory_limit() -> u32 {
    10
}

/// A single memory search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryResult {
    /// Memory item identifier.
    pub id: String,
    /// Content of the memory.
    pub content: String,
    /// Relevance score (0.0 to 1.0).
    pub score: f64,
    /// Namespace/type of the memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// When the memory was created.
    pub created_at: String,
}

/// Response body for `POST /memory/query`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQueryResponse {
    /// API version.
    pub version: String,
    /// Matching memory items.
    pub data: Vec<MemoryResult>,
}

/// Response body for `GET /memory/stats`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStatsResponse {
    /// API version.
    pub version: String,
    /// Total number of stored memories.
    pub total_memories: u64,
    /// Total size in bytes of all stored memories.
    pub total_bytes: u64,
    /// Number of distinct namespaces.
    pub namespaces: u32,
}

/// Request body for `POST /memory/forget`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryForgetRequest {
    /// IDs of entries to delete.
    pub entry_ids: Vec<String>,
}

/// Response body for `POST /memory/forget`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryForgetResponse {
    /// API version.
    pub version: String,
    /// Number of entries actually deleted.
    pub deleted: u32,
}

/// Response body for `GET /memory/entries/:entry_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntryResponse {
    /// API version.
    pub version: String,
    /// The memory entry.
    pub data: MemoryResult,
}

// ---------------------------------------------------------------------------
// Models — responses
// ---------------------------------------------------------------------------

/// Capability flags for a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// Whether the model supports vision/image input.
    pub vision: bool,
    /// Whether the model supports function/tool calling.
    pub tool_use: bool,
    /// Whether the model supports streaming responses.
    pub streaming: bool,
}

/// Response body for a single model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    /// API version.
    pub version: String,
    /// Unique model identifier (e.g. `"claude-opus-4-6"`).
    pub id: String,
    /// Human-readable display name.
    pub name: String,
    /// Provider that hosts this model.
    pub provider: String,
    /// Maximum context window in tokens.
    pub context_window: u32,
    /// Model capability flags.
    pub capabilities: ModelCapabilities,
}

/// Response body for `GET /models` and `GET /providers/:id/models`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListModelsResponse {
    /// API version.
    pub version: String,
    /// List of models.
    pub data: Vec<ModelResponse>,
}

// ---------------------------------------------------------------------------
// Skills — requests and responses
// ---------------------------------------------------------------------------

/// Request body for `POST /skills/install`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSkillRequest {
    /// Filesystem path to the skill package directory.
    pub path: String,
}

/// Request body for `PUT /skills/:skill_id/config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSkillConfigRequest {
    /// Configuration key-value pairs to merge into the skill config.
    pub config: serde_json::Value,
}

/// Response body for a single skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillResponse {
    /// API version.
    pub version: String,
    /// Unique skill identifier (`name@version`).
    pub id: String,
    /// Skill package name.
    pub name: String,
    /// Semver version string.
    pub skill_version: String,
    /// Human-readable description.
    pub description: String,
    /// Required grant patterns.
    pub required_grants: Vec<String>,
    /// Required tool names.
    pub tools: Vec<String>,
    /// Whether the skill is currently active.
    pub active: bool,
}

/// Response body for `GET /skills`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSkillsResponse {
    /// API version.
    pub version: String,
    /// List of skills.
    pub data: Vec<SkillResponse>,
}

// ---------------------------------------------------------------------------
// Tools — responses
// ---------------------------------------------------------------------------

/// Response body for a single tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponse {
    /// API version.
    pub version: String,
    /// Unique tool name (e.g. `"polkagent.file.read"`).
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for input parameters.
    pub input_schema: serde_json::Value,
    /// Required grant pattern, if any.
    pub required_grant: Option<String>,
    /// Output data classification.
    pub output_classification: String,
}

/// Response body for `GET /tools`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListToolsResponse {
    /// API version.
    pub version: String,
    /// List of tools.
    pub data: Vec<ToolResponse>,
}

/// Response body for `GET /tools/:tool_id/grants`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolGrantsResponse {
    /// API version.
    pub version: String,
    /// The tool name.
    pub tool_id: String,
    /// Required grant pattern, if any.
    pub required_grant: Option<String>,
}

// ---------------------------------------------------------------------------
// Payments — responses
// ---------------------------------------------------------------------------

/// Response body for `GET /payments/balance`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentBalanceResponse {
    /// API version.
    pub version: String,
    /// Available balance information.
    pub balance: serde_json::Value,
    /// Current budget configuration.
    pub budget: serde_json::Value,
}

/// Response body for `GET /payments/usage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentUsageResponse {
    /// API version.
    pub version: String,
    /// Total number of runs in this period.
    pub total_runs: u64,
    /// Total tokens consumed.
    pub total_tokens: u64,
    /// Estimated USD cost.
    pub estimated_usd: f64,
    /// Start of the reporting period.
    pub period_start: String,
    /// End of the reporting period.
    pub period_end: String,
}

/// Response body for a single payment receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentReceiptResponse {
    /// API version.
    pub version: String,
    /// The intent ID fulfilled by this receipt.
    pub intent_id: String,
    /// On-chain transaction hash.
    pub tx_hash: String,
    /// Block number of confirmation.
    pub block_number: u64,
    /// Fee paid (human-readable).
    pub fee_paid: String,
    /// When the confirmation was observed.
    pub confirmed_at: String,
}

/// Response body for `GET /payments/receipts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPaymentReceiptsResponse {
    /// API version.
    pub version: String,
    /// List of receipts.
    pub data: Vec<PaymentReceiptResponse>,
}

// ---------------------------------------------------------------------------
// Effects — approval/denial requests and responses (PRD-14 §4)
// ---------------------------------------------------------------------------

/// Request body for `POST /effects/:id/approve`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApproveEffectRequest {
    /// Optional free-text comment explaining the approval.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Optional conditions attached to the approval (e.g. `["max_value:100"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<String>,
}

/// Request body for `POST /effects/:id/deny`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DenyEffectRequest {
    /// Optional free-text comment explaining the denial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Human-readable reason code for the denial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Response body for `POST /effects/:id/approve`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveEffectResponse {
    /// API version.
    pub version: String,
    /// The effect intent ID.
    pub effect_id: String,
    /// The new state after approval (always `"approved"`).
    pub new_state: String,
    /// The durable approval record.
    pub approval: ApprovalRecordDto,
    /// UTC timestamp of when the approval was recorded.
    pub approved_at: DateTime<Utc>,
}

/// Response body for `POST /effects/:id/deny`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenyEffectResponse {
    /// API version.
    pub version: String,
    /// The effect intent ID.
    pub effect_id: String,
    /// The new state after denial (always `"denied"`).
    pub new_state: String,
    /// The durable denial record.
    pub approval: ApprovalRecordDto,
    /// The denial reason, if provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// UTC timestamp of when the denial was recorded.
    pub denied_at: DateTime<Utc>,
}

/// DTO for an `ApprovalRecord` in API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecordDto {
    /// Stable identifier for this record.
    pub id: String,
    /// The effect intent this record belongs to.
    pub effect_id: String,
    /// Who or what made the decision: `"human"`, `"quorum"`, `"service"`, or `"mandate"`.
    pub approval_type: String,
    /// Identifier of the principal that made the decision.
    pub principal_id: String,
    /// Optional comment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Conditions attached to an approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<String>,
    /// `"approved"` or `"denied"`.
    pub decision: String,
    /// When this record was created.
    pub created_at: DateTime<Utc>,
}

/// Response body for a scoped interaction approval query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListInteractionApprovalsResponse {
    /// API version.
    pub version: String,
    /// Exact pending approvals visible to the authenticated interaction scope.
    pub data: Vec<polkagent_interaction::ApprovalView>,
}

/// Optional safe rationale for a denial decision.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DenyInteractionApprovalRequest {
    /// Bounded operator rationale persisted with the durable decision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Response body for a scoped durable approval decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionApprovalResponse {
    /// API version.
    pub version: String,
    /// Durable approval projection after the decision.
    pub approval: polkagent_interaction::ApprovalView,
}

// ---------------------------------------------------------------------------
// Agent lifecycle — requests
// ---------------------------------------------------------------------------

/// Response body for agent lifecycle actions (start/stop/pause/resume).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLifecycleResponse {
    /// API version.
    pub version: String,
    /// The agent ID.
    pub agent_id: String,
    /// The action that was requested.
    pub action: String,
    /// The resulting status message.
    pub status: String,
}

// ---------------------------------------------------------------------------
// Audit — query params and responses
// ---------------------------------------------------------------------------

/// Query parameters for `GET /audit`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditListParams {
    /// Filter by actor ID (exact match).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Filter by action type (`snake_case`, e.g. `"run_started"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Only include entries at or after this RFC-3339 timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// Only include entries at or before this RFC-3339 timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// Maximum number of entries to return (default 100).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Response body for a single audit entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntryResponse {
    /// API version.
    pub version: String,
    /// The audit entry (serialized as-is from the audit crate).
    pub data: polkagent_audit::AuditEntry,
}

/// Response body for `GET /audit` (list).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditListResponse {
    /// API version.
    pub version: String,
    /// List of matching audit entries.
    pub data: Vec<polkagent_audit::AuditEntry>,
    /// Total number of entries returned.
    pub count: usize,
}

/// Response body for `GET /audit/verify`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditVerifyResponse {
    /// API version.
    pub version: String,
    /// Whether the integrity chain is valid.
    pub valid: bool,
    /// Total number of entries verified.
    pub entries_checked: usize,
    /// Error detail when `valid` is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Conversations — requests
// ---------------------------------------------------------------------------

/// Request body for `POST /conversations`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateConversationRequest {
    /// The agent that owns this conversation.
    pub agent_id: String,
    /// Optional human-readable title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional key-value metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Bridge C1 — wire types (PRD-06 §20.4)
// ---------------------------------------------------------------------------

/// C1 bridge inbound delivery (JSON wire format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeDelivery {
    pub delivery_id: String,
    pub lease_id: String,
    pub lease_ms: u64,
    pub chat_id: String,
    pub message_id: String,
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<BridgeAttachment>,
}

/// Attachment metadata inside a [`BridgeDelivery`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeAttachment {
    pub id: String,
    pub mime: String,
    pub size: u64,
    pub media_id: String,
    pub url: String,
}

/// C1 bridge ACK request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeAckRequest {
    pub delivery_id: String,
    pub lease_id: String,
}

/// C1 bridge lease renewal request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeRenewRequest {
    pub delivery_id: String,
    pub lease_id: String,
}

/// C1 bridge send request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeSendRequest {
    pub chat_id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<String>,
}

/// Identity section of [`BridgeHealthResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeIdentityInfo {
    pub bot_id: String,
    pub display_name: String,
}

/// Transport section of [`BridgeHealthResponse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeTransportInfo {
    pub connected: bool,
    pub protocol: String,
}

/// Capabilities section of [`BridgeHealthResponse`].
#[allow(
    clippy::struct_excessive_bools,
    reason = "the bridge wire format represents five independent capability flags"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeCapabilities {
    pub send: bool,
    pub edit: bool,
    pub react: bool,
    pub typing: bool,
    pub media: bool,
}

/// C1 bridge health response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeHealthResponse {
    pub identity: BridgeIdentityInfo,
    pub transport: BridgeTransportInfo,
    pub capabilities: BridgeCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded: Option<Vec<String>>,
}

/// Response returned when polling inbound deliveries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeInboundResponse {
    pub deliveries: Vec<BridgeDelivery>,
}

/// Response returned after a successful ACK.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeAckResponse {
    pub ok: bool,
}

/// Response returned after a successful lease renewal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeRenewResponse {
    pub ok: bool,
    pub new_lease_ms: u64,
}

/// Response returned after a successful send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeSendResponse {
    pub ok: bool,
    pub message_id: String,
}

/// Request body for `POST /conversations/:id/messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMessageRequest {
    /// Message role: `"user"`, `"assistant"`, `"system"`, or `"tool"`.
    pub role: String,
    /// The text content of the message.
    pub content: String,
}

/// Query parameters for `GET /conversations`.
#[derive(Debug, Clone, Deserialize)]
pub struct ListConversationsQuery {
    /// Filter by owning agent ID (required).
    pub agent_id: String,
    /// Maximum items per page (default 50, max 100).
    pub limit: Option<u32>,
    /// Offset for pagination.
    pub offset: Option<u32>,
}

// ---------------------------------------------------------------------------
// Conversations — responses
// ---------------------------------------------------------------------------

/// Response body for `POST /conversations`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateConversationResponse {
    /// API version.
    pub version: String,
    /// The newly created conversation ID.
    pub id: String,
    /// The owning agent ID.
    pub agent_id: String,
    /// Optional title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// When the conversation was created.
    pub created_at: DateTime<Utc>,
}

/// A conversation summary for list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummaryDto {
    /// Conversation identifier.
    pub id: String,
    /// Owning agent identifier.
    pub agent_id: String,
    /// Optional title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Number of messages in the conversation.
    pub message_count: u32,
    /// Timestamp of the most recent message, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message_at: Option<DateTime<Utc>>,
    /// When the conversation was created.
    pub created_at: DateTime<Utc>,
}

/// Response body for `GET /conversations`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListConversationsResponse {
    /// API version.
    pub version: String,
    /// Page of conversation summaries.
    pub data: Vec<ConversationSummaryDto>,
    /// Pagination cursor information.
    pub cursor: CursorInfo,
    /// Page metadata.
    pub meta: PageMeta,
}

/// A single message in a conversation response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDto {
    /// Message identifier.
    pub id: String,
    /// The conversation this message belongs to.
    pub conversation_id: String,
    /// The sender role.
    pub role: String,
    /// The text content of the message.
    pub content: String,
    /// When this message was created.
    pub created_at: DateTime<Utc>,
    /// Approximate token count, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_count: Option<u32>,
}

impl From<polkagent_conversation::Message> for MessageDto {
    fn from(msg: polkagent_conversation::Message) -> Self {
        let content_text = match &msg.content {
            polkagent_conversation::MessageContent::Text { text } => text.clone(),
            other => serde_json::to_string(other).unwrap_or_default(),
        };
        let role_str = match msg.role {
            polkagent_conversation::MessageRole::User => "user",
            polkagent_conversation::MessageRole::Assistant => "assistant",
            polkagent_conversation::MessageRole::System => "system",
            polkagent_conversation::MessageRole::Tool => "tool",
        };
        Self {
            id: msg.id.to_string(),
            conversation_id: msg.conversation_id.to_string(),
            role: role_str.to_owned(),
            content: content_text,
            created_at: msg.created_at,
            token_count: msg.token_count,
        }
    }
}

/// Response body for `GET /conversations/:id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationResponse {
    /// API version.
    pub version: String,
    /// Conversation identifier.
    pub id: String,
    /// Owning agent identifier.
    pub agent_id: String,
    /// Optional title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Number of messages in the conversation.
    pub message_count: u32,
    /// When the conversation was created.
    pub created_at: DateTime<Utc>,
    /// When the conversation was last updated.
    pub updated_at: DateTime<Utc>,
    /// Messages in the conversation.
    pub messages: Vec<MessageDto>,
}

/// Response body for `POST /conversations/:id/messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMessageResponse {
    /// API version.
    pub version: String,
    /// The newly created message ID.
    pub id: String,
    /// The conversation the message belongs to.
    pub conversation_id: String,
    /// The sender role.
    pub role: String,
    /// The text content.
    pub content: String,
    /// When the message was created.
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Registry — service listing requests and responses (PRD-12 §5.5)
// ---------------------------------------------------------------------------

/// Request body for `POST /registry/listings`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateServiceListingRequest {
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub pricing: ServicePricing,
    #[serde(default = "default_service_availability")]
    pub availability: ServiceAvailability,
    #[serde(default)]
    pub protocols: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sla_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_schema: Option<serde_json::Value>,
}

fn default_service_availability() -> ServiceAvailability {
    ServiceAvailability::Available
}

/// Response body for a single service listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceListingResponse {
    pub version: String,
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub service_version: String,
    pub capabilities: Vec<String>,
    pub tags: Vec<String>,
    pub pricing: ServicePricing,
    pub availability: String,
    pub protocols: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sla_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_schema: Option<serde_json::Value>,
    pub created_at: String,
    pub updated_at: String,
}

/// Response body for `GET /registry/search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListServiceListingsResponse {
    pub version: String,
    pub data: Vec<ServiceListingResponse>,
}

/// Query parameters for `GET /registry/search`.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchListingsQuery {
    pub q: Option<String>,
    pub capability: Option<String>,
    pub tag: Option<String>,
    pub author: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

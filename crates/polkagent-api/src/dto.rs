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

use chrono::{DateTime, Utc};
use polkagent_core::{AgentId, RunId};
use polkagent_core::agent::{AgentSpec, AgentState};
use polkagent_core::run::RunState;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// API version string present in every response.
pub const API_VERSION: &str = "v1alpha1";

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
    /// Return events with global_sequence >= since.
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

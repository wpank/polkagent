//! Configuration schema types for Polkagent.
//!
//! All structs correspond to sections of the `polkagent.toml` configuration file.
//! Each type implements [`Default`] with safe, sensible values so that a minimal
//! config file (or even no file at all) always produces a runnable configuration.
//!
//! # Schema version
//!
//! The top-level [`Config`] contains a [`MetaConfig`] with `schema_version = 1`.
//! The version is checked during load; unknown versions are rejected.
//!
//! # Secrets
//!
//! API keys and other secrets must **never** appear in config files.
//! They are injected exclusively via environment variables (see [`crate::env`]).
//! The `api_key_env` field on [`ProviderConfig`] names the environment variable
//! from which the runtime should read the key.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model_registry::ProviderKind;

// ---------------------------------------------------------------------------
// Top-level
// ---------------------------------------------------------------------------

/// Root configuration for the Polkagent platform.
///
/// Corresponds to the top-level keys of `polkagent.toml`. Every field is
/// optional-with-default so that a minimal config file can omit any section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Schema metadata (version check, API version).
    pub meta: MetaConfig,
    /// Logging / tracing settings.
    pub log: LogConfig,
    /// Primary data store settings.
    pub database: DatabaseConfig,
    /// Run execution limits and budget defaults.
    pub execution: ExecutionConfig,
    /// AI provider connections. Supports multiple providers.
    pub providers: Vec<ProviderConfig>,
    /// Model overrides and custom model definitions.
    pub models: Vec<ModelOverrideConfig>,
    /// Policy evaluation settings.
    pub policy: PolicyConfig,
    /// Conversational memory settings.
    pub memory: MemoryConfig,
    /// HTTP API server settings.
    pub api: ApiConfig,
    /// Terminal UI settings.
    pub tui: TuiConfig,
    /// gRPC/HTTP server settings (separate from the REST API server).
    pub server: ServerConfig,
    /// Authentication and authorisation settings.
    pub auth: AuthConfig,
    /// Sandbox and resource-limit settings.
    pub security: SecurityConfig,
    /// Skill discovery and loading settings.
    pub skills: SkillsConfig,
    /// Agent harness settings.
    pub harness: HarnessConfig,
    /// Artifact storage settings.
    pub artifacts: ArtifactConfig,
    /// OpenTelemetry observability settings.
    pub observability: ObservabilityConfig,
}

// ---------------------------------------------------------------------------
// Meta
// ---------------------------------------------------------------------------

/// Schema metadata included at the top of every config file.
///
/// Used to detect version mismatches and reject unknown schemas early.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetaConfig {
    /// Semantic API version string (e.g. `"polkagent.dev/v1alpha1"`).
    pub api_version: String,
    /// Monotonically increasing integer schema version. Currently `1`.
    pub schema_version: u32,
}

impl Default for MetaConfig {
    fn default() -> Self {
        Self {
            api_version: "polkagent.dev/v1alpha1".to_owned(),
            schema_version: 1,
        }
    }
}

/// The only schema version this crate understands.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Log
// ---------------------------------------------------------------------------

/// Logging and structured-tracing configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// Minimum log level: `trace`, `debug`, `info`, `warn`, or `error`.
    pub level: String,
    /// Output format: `pretty` for human-readable or `json` for structured logs.
    pub format: LogFormat,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            format: LogFormat::Pretty,
        }
    }
}

/// Log output format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    /// Human-readable, colourised output for interactive terminals.
    #[default]
    Pretty,
    /// Newline-delimited JSON for log aggregation pipelines.
    Json,
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

/// Database backend configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// Storage backend to use.
    pub backend: DatabaseBackend,
    /// SQLite-specific settings (used when `backend = "sqlite"`).
    pub sqlite: SqliteConfig,
    /// PostgreSQL-specific settings (used when `backend = "postgres"`).
    pub postgres: PostgresConfig,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            backend: DatabaseBackend::Sqlite,
            sqlite: SqliteConfig::default(),
            postgres: PostgresConfig::default(),
        }
    }
}

/// Which storage engine to use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseBackend {
    /// Embedded `SQLite` database — suitable for local/single-node deployments.
    #[default]
    Sqlite,
    /// Remote `PostgreSQL` — required for multi-tenant or highly-available deployments.
    Postgres,
}

/// `SQLite` backend settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SqliteConfig {
    /// Path to the database file. Tilde expansion is applied at load time.
    pub path: String,
    /// Enable WAL (Write-Ahead Logging) journal mode for better concurrency.
    pub wal_mode: bool,
    /// Milliseconds to wait when the database is locked before returning
    /// `SQLITE_BUSY`. Default: `5000`.
    pub busy_timeout_ms: u64,
    /// Run a WAL checkpoint at startup to reduce file size.
    pub checkpoint_on_startup: bool,
}

impl Default for SqliteConfig {
    fn default() -> Self {
        Self {
            path: "~/.local/share/polkagent/polkagent.db".to_owned(),
            wal_mode: true,
            busy_timeout_ms: 5_000,
            checkpoint_on_startup: false,
        }
    }
}

/// `PostgreSQL` backend settings.
///
/// The `url` field must be empty in config files; it is resolved at runtime
/// from the `POLKAGENT_DATABASE_POSTGRES_URL` environment variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PostgresConfig {
    /// Connection URL. **Must not be set in config files** — use the
    /// `POLKAGENT_DATABASE_POSTGRES_URL` environment variable instead.
    pub url: String,
    /// Maximum number of connections in the pool. Default: `20`.
    pub max_connections: u32,
    /// TLS/SSL mode: `disable`, `prefer`, or `require`. Default: `"require"`.
    pub ssl_mode: String,
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            max_connections: 20,
            ssl_mode: "require".to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Execution engine limits and budget defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecutionConfig {
    /// Maximum number of runs that may execute concurrently. Default: `10`.
    pub max_concurrent_runs: u32,
    /// Default per-run timeout in seconds. Default: `600` (10 minutes).
    pub default_timeout_secs: u64,
    /// Default spending limits applied to every run.
    pub budget: BudgetConfig,
    /// Default provider identifier used when no `--provider` CLI flag is given.
    /// When `None`, the system falls back to environment-variable detection.
    #[serde(default)]
    pub default_provider: Option<String>,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_concurrent_runs: 10,
            default_timeout_secs: 600,
            budget: BudgetConfig::default(),
            default_provider: None,
        }
    }
}

/// Spending-limit defaults applied to every run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetConfig {
    /// Maximum model API spend per single run in USD. Default: `5.00`.
    pub max_usd_per_run: f64,
    /// Maximum total model API spend per calendar day in USD. Default: `50.00`.
    pub max_usd_per_day: f64,
    /// Emit a warning log when spend reaches this percentage of the limit.
    /// Value is `0`–`100`. Default: `80`.
    pub warn_threshold_percent: u8,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_usd_per_run: 5.00,
            max_usd_per_day: 50.00,
            warn_threshold_percent: 80,
        }
    }
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

/// Configuration for a single AI model provider.
///
/// Multiple providers can be listed under `[[providers]]` in TOML.
/// The `api_key_env` field names the environment variable that holds the key;
/// the key itself must **never** appear in the config file.
///
/// # Environment variable overrides
///
/// - `POLKAGENT_PROVIDER_{ID}_BASE_URL` — override `base_url` for provider with given id
/// - `POLKAGENT_PROVIDER_{ID}_TIMEOUT` — override `timeout_secs`
/// - `POLKAGENT_PROVIDER_{ID}_MAX_CONCURRENT` — override `max_concurrent`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Stable identifier used to reference this provider in agent specs.
    pub id: String,
    /// Provider type string (e.g. `"anthropic"`, `"openai_compatible"`).
    pub provider_type: String,
    /// Typed provider backend kind (e.g. `anthropic_api`, `openai_compat`).
    /// When `None`, inferred from `provider_type`.
    #[serde(default)]
    pub kind: Option<ProviderKind>,
    /// Name of the environment variable that holds the API key.
    /// Example: `"ANTHROPIC_API_KEY"`.
    pub api_key_env: String,
    /// Base URL for the provider API.
    pub base_url: String,
    /// Default model identifier to use when none is specified in the agent spec.
    pub default_model: String,
    /// Per-request timeout in seconds. Default: `120`.
    pub timeout_secs: u64,
    /// Maximum number of automatic retries on transient failures. Default: `3`.
    pub max_retries: u32,
    /// Time-to-first-token timeout in seconds. Default: `15`.
    #[serde(default = "default_ttft_timeout")]
    pub ttft_timeout_secs: Option<u64>,
    /// TCP connection timeout in seconds. Default: `5`.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: Option<u64>,
    /// Maximum concurrent requests to this provider. Default: `10`.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: Option<u32>,
    /// Extra HTTP headers sent with every request to this provider.
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
    /// Provider-specific extension data (arbitrary JSON).
    #[serde(default)]
    pub extra: Option<serde_json::Value>,
}

fn default_ttft_timeout() -> Option<u64> {
    Some(15)
}

fn default_connect_timeout() -> Option<u64> {
    Some(5)
}

fn default_max_concurrent() -> Option<u32> {
    Some(10)
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            provider_type: String::new(),
            kind: None,
            api_key_env: String::new(),
            base_url: String::new(),
            default_model: String::new(),
            timeout_secs: 120,
            max_retries: 3,
            ttft_timeout_secs: Some(15),
            connect_timeout_secs: Some(5),
            max_concurrent: Some(10),
            extra_headers: HashMap::new(),
            extra: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

/// Configuration for a model override or custom model definition.
///
/// Listed under `[[models]]` in TOML. Allows operators to register custom
/// models or override properties of known models (context window, cost, tool
/// support, etc.).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelOverrideConfig {
    /// Model slug used to reference this model (e.g. `"claude-sonnet-4-6"`).
    pub slug: String,
    /// Provider id this model belongs to (must match a `[[providers]].id`).
    pub provider: String,
    /// Context window size in tokens.
    #[serde(default)]
    pub context_window: Option<u64>,
    /// Maximum output tokens the model can produce.
    #[serde(default)]
    pub max_output: Option<u64>,
    /// Whether the model supports tool/function calling.
    #[serde(default)]
    pub supports_tools: Option<bool>,
    /// Whether the model supports extended thinking / chain-of-thought.
    #[serde(default)]
    pub supports_thinking: Option<bool>,
    /// Tool calling format: `"json"`, `"xml"`, `"native"`, etc.
    #[serde(default)]
    pub tool_format: Option<String>,
    /// Cost per million input tokens in USD.
    #[serde(default)]
    pub cost_input_per_m: Option<f64>,
    /// Cost per million output tokens in USD.
    #[serde(default)]
    pub cost_output_per_m: Option<f64>,
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/// Policy engine settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyConfig {
    /// Directory containing policy definition files. Tilde expansion is applied.
    pub policy_dir: String,
    /// Name of the policy applied when an agent spec does not specify one.
    pub default_policy: String,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            policy_dir: "~/.config/polkagent/policies".to_owned(),
            default_policy: "default".to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

/// Conversational memory settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Whether the memory subsystem is active. Default: `false`.
    pub enabled: bool,
    /// Storage backend for memory entries: `"sqlite"`, `"postgres"`, or
    /// `"external"`.
    pub backend: String,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: "sqlite".to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// API
// ---------------------------------------------------------------------------

/// HTTP API server settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiConfig {
    /// Whether the HTTP API server is started. Default: `true`.
    pub enabled: bool,
    /// Socket address the server binds to. Default: `"127.0.0.1:4840"`.
    pub bind_address: String,
    /// List of allowed CORS origins.
    /// Default: `["http://localhost:*"]`.
    pub cors_origins: Vec<String>,
    /// When `true`, the server rejects all mutating (write) requests.
    /// Default: `false`.
    pub read_only: bool,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind_address: "127.0.0.1:4840".to_owned(),
            cors_origins: vec!["http://localhost:*".to_owned()],
            read_only: false,
        }
    }
}

// ---------------------------------------------------------------------------
// TUI
// ---------------------------------------------------------------------------

/// Terminal User Interface settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TuiConfig {
    /// Visual theme.
    pub theme: TuiTheme,
    /// Enable atmospheric particle / shimmer effects. Default: `true`.
    pub atmospheric_effects: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: TuiTheme::Dark,
            atmospheric_effects: true,
        }
    }
}

/// TUI colour theme.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TuiTheme {
    /// Rich dark palette with colour highlights.
    #[default]
    Dark,
    /// Monochrome output suitable for pipes and accessibility.
    NoColor,
    /// High-contrast palette for visual accessibility.
    HighContrast,
}

// ---------------------------------------------------------------------------
// Server  (PRD-14 §1)
// ---------------------------------------------------------------------------

/// gRPC / HTTP server settings (distinct from the REST `[api]` server).
///
/// Binds the primary agent-facing RPC endpoint with optional TLS, CORS, and
/// rate-limiting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Socket address the server binds to. Default: `"127.0.0.1:9090"`.
    pub bind_address: String,
    /// Optional TLS configuration. When `None` the server runs in plain-text
    /// mode (suitable for loopback-only deployments).
    pub tls: Option<TlsConfig>,
    /// Allowed CORS origins. Default: `["*"]` (all origins).
    pub cors_origins: Vec<String>,
    /// Request rate-limiting configuration.
    pub rate_limit: RateLimitConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1:9090".to_owned(),
            tls: None,
            cors_origins: vec!["*".to_owned()],
            rate_limit: RateLimitConfig::default(),
        }
    }
}

/// TLS certificate and key paths for the server.
///
/// All paths are optional so that a partial TLS configuration can be expressed
/// without validation errors during loading; the validator will reject
/// incomplete TLS configurations before the server starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TlsConfig {
    /// Path to the PEM-encoded TLS certificate file.
    pub cert_path: Option<PathBuf>,
    /// Path to the PEM-encoded private key file.
    pub key_path: Option<PathBuf>,
    /// Path to the CA certificate bundle used for mutual TLS client
    /// authentication. `None` disables mTLS.
    pub ca_path: Option<PathBuf>,
}

/// Token-bucket rate-limiting parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitConfig {
    /// Whether rate limiting is enabled. Default: `true`.
    pub enabled: bool,
    /// Steady-state request rate allowed per second. Default: `100`.
    pub requests_per_second: u32,
    /// Maximum burst above the steady-state rate. Default: `200`.
    pub burst: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_second: 100,
            burst: 200,
        }
    }
}

// ---------------------------------------------------------------------------
// Auth  (PRD-14 §2)
// ---------------------------------------------------------------------------

/// Authentication and authorisation settings.
///
/// API keys stored here are **hashed digests**, not plaintext secrets. The
/// actual keys are distributed out-of-band and hashed before being added to
/// this list. JWT secrets are never stored here; `jwt_secret_env` names the
/// environment variable that holds the secret at runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Whether authentication is required. Default: `false` (open access).
    pub enabled: bool,
    /// List of accepted API key hashes (not the plaintext keys themselves).
    /// Default: empty (no pre-shared keys configured).
    pub api_keys: Vec<String>,
    /// Name of the environment variable that holds the JWT signing secret.
    /// Example: `"POLKAGENT_JWT_SECRET"`. `None` disables JWT authentication.
    pub jwt_secret_env: Option<String>,
    /// How long a session remains valid after issuance, in seconds.
    /// Default: `3600` (1 hour).
    pub session_timeout_secs: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_keys: Vec::new(),
            jwt_secret_env: None,
            session_timeout_secs: 3600,
        }
    }
}

// ---------------------------------------------------------------------------
// Security  (PRD-14 §3)
// ---------------------------------------------------------------------------

/// Sandbox and resource-limit settings.
///
/// Controls filesystem access, memory consumption, and CPU time available to
/// agent runs. `allowed_paths` and `denied_paths` work as a whitelist/blacklist
/// pair: access is granted to paths in `allowed_paths` (empty = all), then
/// refined by denying anything in `denied_paths`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Whether the process-level sandbox is active. Default: `false`.
    pub sandbox_enabled: bool,
    /// Maximum file size an agent may read or write, in bytes.
    /// Default: `10_485_760` (10 MiB).
    pub max_file_size_bytes: u64,
    /// Filesystem paths the agent is allowed to access.
    /// Empty list means all paths are allowed (subject to `denied_paths`).
    pub allowed_paths: Vec<PathBuf>,
    /// Filesystem paths the agent is explicitly denied access to.
    /// Default: common sensitive system paths.
    pub denied_paths: Vec<PathBuf>,
    /// Maximum RSS memory an agent may use, in mebibytes. Default: `512`.
    pub max_memory_mb: u64,
    /// Maximum wall-clock CPU seconds an agent may consume. Default: `300`.
    pub max_cpu_seconds: u64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            sandbox_enabled: false,
            max_file_size_bytes: 10 * 1024 * 1024, // 10 MiB
            allowed_paths: Vec::new(),
            denied_paths: vec![
                PathBuf::from("/etc/shadow"),
                PathBuf::from("/etc/passwd"),
                PathBuf::from("/etc/sudoers"),
                PathBuf::from("/root"),
                PathBuf::from("/proc"),
                PathBuf::from("/sys"),
            ],
            max_memory_mb: 512,
            max_cpu_seconds: 300,
        }
    }
}

// ---------------------------------------------------------------------------
// Skills  (PRD-14 §4)
// ---------------------------------------------------------------------------

/// Skill discovery and loading settings.
///
/// Polkagent searches `directories` for skill bundles at startup. If
/// `auto_load` is `true`, all discovered skills are loaded immediately;
/// otherwise they are registered but not activated until explicitly requested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    /// Directories to search for skill bundles. Tilde expansion is applied.
    /// Default: empty (no extra skill paths).
    pub directories: Vec<PathBuf>,
    /// Automatically load all discovered skills at startup. Default: `true`.
    pub auto_load: bool,
    /// URL of a remote skill registry. `None` disables registry integration.
    pub registry_url: Option<String>,
    /// Local directory used to cache downloaded skill bundles.
    /// `None` uses a platform-appropriate default.
    pub cache_dir: Option<PathBuf>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            directories: Vec::new(),
            auto_load: true,
            registry_url: None,
            cache_dir: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Harness  (PRD-14 §5)
// ---------------------------------------------------------------------------

/// Agent harness settings.
///
/// The harness is the subprocess or in-process executor that runs agent code.
/// `harness_type` selects the backend; `binary_path` overrides the harness
/// executable location when the type requires an external binary.
///
/// # Environment variable overrides
///
/// - `POLKAGENT_HARNESS_DEFAULT` — override `default`
/// - `POLKAGENT_HARNESS_TIMEOUT` — override `timeout_secs`
/// - `POLKAGENT_HARNESS_MAX_CONCURRENT` — override `max_concurrent`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HarnessConfig {
    /// Harness backend identifier. Default: `"claude"`.
    pub harness_type: String,
    /// Path to the harness executable. `None` resolves the binary from `PATH`.
    pub binary_path: Option<PathBuf>,
    /// Maximum seconds a single harness invocation may run. Default: `300`.
    pub timeout_secs: u64,
    /// Maximum number of harness processes that may run simultaneously.
    /// Default: `1`.
    pub max_concurrent: u32,
    /// Default harness name used when none is specified. Default: `None`.
    #[serde(default)]
    pub default: Option<String>,
    /// Per-harness configurations keyed by harness name.
    #[serde(default)]
    pub harnesses: HashMap<String, HarnessEntryConfig>,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            harness_type: "claude".to_owned(),
            binary_path: None,
            timeout_secs: 300,
            max_concurrent: 1,
            default: None,
            harnesses: HashMap::new(),
        }
    }
}

/// Configuration for an individual named harness entry.
///
/// Listed under `[harness.harnesses.<name>]` in TOML.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessEntryConfig {
    /// Path to the harness binary. `None` resolves from `PATH`.
    #[serde(default)]
    pub binary_path: Option<String>,
    /// Transport protocol: `"stdio"`, `"http"`, `"grpc"`, etc.
    #[serde(default)]
    pub transport: Option<String>,
    /// Approval mode: `"auto"`, `"manual"`, `"policy"`, etc.
    #[serde(default)]
    pub approval_mode: Option<String>,
    /// HTTP port for HTTP-transport harnesses.
    #[serde(default)]
    pub http_port: Option<u16>,
}

// ---------------------------------------------------------------------------
// Artifacts  (PRD-14 §6)
// ---------------------------------------------------------------------------

/// Artifact storage settings.
///
/// Controls how run outputs (files, logs, results) are stored, how large they
/// may be, and how long they are retained before automatic cleanup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactConfig {
    /// Maximum size of a single artifact, in bytes.
    /// Default: `104_857_600` (100 MiB).
    pub max_size_bytes: u64,
    /// Number of days artifacts are retained before automatic deletion.
    /// Default: `90`.
    pub retention_days: u32,
    /// Whether artifacts are compressed before storage. Default: `true`.
    pub compression_enabled: bool,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            max_size_bytes: 100 * 1024 * 1024, // 100 MiB
            retention_days: 90,
            compression_enabled: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Observability  (PRD-14 §7)
// ---------------------------------------------------------------------------

/// OpenTelemetry observability settings.
///
/// Controls export of metrics and traces to an OTLP-compatible collector
/// (e.g. OpenTelemetry Collector, Jaeger, Grafana Alloy). When
/// `otlp_endpoint` is `None`, telemetry export is disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObservabilityConfig {
    /// OTLP collector endpoint URL.
    /// Example: `"http://localhost:4317"`. `None` disables export.
    pub otlp_endpoint: Option<String>,
    /// OTLP transport protocol: `"grpc"` or `"http/protobuf"`. Default: `"grpc"`.
    pub otlp_protocol: String,
    /// Whether metric export is active. Default: `true`.
    pub metrics_enabled: bool,
    /// Whether trace export is active. Default: `true`.
    pub traces_enabled: bool,
    /// Service name reported in telemetry. Default: `"polkagent"`.
    pub service_name: String,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            otlp_endpoint: None,
            otlp_protocol: "grpc".to_owned(),
            metrics_enabled: true,
            traces_enabled: true,
            service_name: "polkagent".to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Template
// ---------------------------------------------------------------------------

/// A default `polkagent.toml` template that users can copy as a starting point.
///
/// All secrets are referenced by environment variable name; no actual keys
/// appear here.
pub const DEFAULT_CONFIG_TEMPLATE: &str = r#"# Polkagent configuration
# Schema version: v1alpha1
#
# Copy this file to one of:
#   ~/.config/polkagent/polkagent.toml   (user-global)
#   .polkagent/polkagent.toml            (project-local, takes precedence)
#
# Secrets (API keys, database URLs) must NEVER be stored here.
# Use the environment variables listed in each section comment.

[meta]
api_version = "polkagent.dev/v1alpha1"
schema_version = 1

# ─── Logging ─────────────────────────────────────────────────────────────────
# Override with: POLKAGENT_LOG_LEVEL=debug

[log]
level = "info"     # trace | debug | info | warn | error
format = "pretty"  # pretty | json

# ─── Database ────────────────────────────────────────────────────────────────
# Postgres URL: set POLKAGENT_DATABASE_POSTGRES_URL (never in this file)

[database]
backend = "sqlite"  # sqlite | postgres

[database.sqlite]
path = "~/.local/share/polkagent/polkagent.db"
wal_mode = true
busy_timeout_ms = 5000
checkpoint_on_startup = false

[database.postgres]
# url — set via POLKAGENT_DATABASE_POSTGRES_URL env var
max_connections = 20
ssl_mode = "require"

# ─── Execution ───────────────────────────────────────────────────────────────

[execution]
max_concurrent_runs = 10
default_timeout_secs = 600

[execution.budget]
max_usd_per_run = 5.00
max_usd_per_day = 50.00
warn_threshold_percent = 80

# ─── Providers ───────────────────────────────────────────────────────────────
# API keys are read from the env var named in `api_key_env`.
# Set ANTHROPIC_API_KEY and/or OPENAI_API_KEY in your environment.

[[providers]]
id = "anthropic-default"
provider_type = "anthropic"
# kind = "anthropic_api"
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com"
default_model = "claude-sonnet-4-6"
timeout_secs = 120
max_retries = 3
# ttft_timeout_secs = 15
# connect_timeout_secs = 5
# max_concurrent = 10
# [providers.extra_headers]
# X-Custom-Header = "value"

# [[providers]]
# id = "openai-compat"
# provider_type = "openai_compatible"
# kind = "openai_compat"
# api_key_env = "OPENAI_API_KEY"
# base_url = "https://api.openai.com/v1"
# default_model = "gpt-4o"
# timeout_secs = 120
# max_retries = 3

# ─── Policy ──────────────────────────────────────────────────────────────────

[policy]
policy_dir = "~/.config/polkagent/policies"
default_policy = "default"

# ─── Memory ──────────────────────────────────────────────────────────────────

[memory]
enabled = false
backend = "sqlite"  # sqlite | postgres | external

# ─── API server ──────────────────────────────────────────────────────────────
# Override bind address: POLKAGENT_API_BIND=0.0.0.0:4840

[api]
enabled = true
bind_address = "127.0.0.1:4840"
cors_origins = ["http://localhost:*"]

# ─── TUI ─────────────────────────────────────────────────────────────────────

[tui]
theme = "dark"                # dark | no_color | high_contrast
atmospheric_effects = true

# ─── Server ──────────────────────────────────────────────────────────────────
# gRPC/HTTP agent-facing server (distinct from the REST [api] server above).
# Override bind address: POLKAGENT_SERVER_BIND=0.0.0.0:9090

[server]
bind_address = "127.0.0.1:9090"
cors_origins = ["*"]

[server.rate_limit]
requests_per_second = 100
burst = 200

# TLS is disabled by default. Uncomment to enable:
# [server.tls]
# cert_path = "/etc/polkagent/tls/cert.pem"
# key_path  = "/etc/polkagent/tls/key.pem"
# ca_path   = "/etc/polkagent/tls/ca.pem"   # required only for mTLS

# ─── Auth ────────────────────────────────────────────────────────────────────
# Override: POLKAGENT_AUTH_ENABLED=true
# JWT secret: set POLKAGENT_JWT_SECRET in your environment (never in this file)

[auth]
enabled = false
# api_keys contains HASHED key digests, not plaintext secrets.
api_keys = []
# jwt_secret_env = "POLKAGENT_JWT_SECRET"
session_timeout_secs = 3600

# ─── Security ────────────────────────────────────────────────────────────────
# Override sandbox: POLKAGENT_SECURITY_SANDBOX=true

[security]
sandbox_enabled = false
max_file_size_bytes = 10485760   # 10 MiB
allowed_paths = []               # empty = all paths allowed
denied_paths = [
  "/etc/shadow",
  "/etc/passwd",
  "/etc/sudoers",
  "/root",
  "/proc",
  "/sys",
]
max_memory_mb = 512
max_cpu_seconds = 300

# ─── Skills ──────────────────────────────────────────────────────────────────
# Override skill dir: POLKAGENT_SKILLS_DIR=/path/to/skills

[skills]
directories = []    # extra skill search paths
auto_load = true
# registry_url = "https://registry.polkagent.dev"
# cache_dir = "~/.cache/polkagent/skills"

# ─── Models ──────────────────────────────────────────────────────────────────
# Override or register custom models. Each [[models]] entry maps a slug to a
# provider and optionally overrides context window, cost, tool support, etc.

# [[models]]
# slug = "my-custom-model"
# provider = "anthropic-default"
# context_window = 200000
# max_output = 8192
# supports_tools = true
# supports_thinking = true
# tool_format = "native"
# cost_input_per_m = 3.0
# cost_output_per_m = 15.0

# ─── Harness ─────────────────────────────────────────────────────────────────
# Override: POLKAGENT_HARNESS_DEFAULT=codex
#           POLKAGENT_HARNESS_TIMEOUT=600
#           POLKAGENT_HARNESS_MAX_CONCURRENT=4

[harness]
harness_type = "claude"   # claude | custom
# binary_path = "/usr/local/bin/polkagent-harness"
timeout_secs = 300
max_concurrent = 1
# default = "claude"

# Per-harness entries:
# [harness.harnesses.codex]
# binary_path = "/usr/local/bin/codex"
# transport = "http"
# approval_mode = "auto"
# http_port = 8080

# ─── Artifacts ───────────────────────────────────────────────────────────────

[artifacts]
max_size_bytes = 104857600   # 100 MiB
retention_days = 90
compression_enabled = true

# ─── Observability ───────────────────────────────────────────────────────────
# Override OTLP endpoint: POLKAGENT_OTLP_ENDPOINT=http://localhost:4317

[observability]
# otlp_endpoint = "http://localhost:4317"
otlp_protocol = "grpc"     # grpc | http/protobuf
metrics_enabled = true
traces_enabled = true
service_name = "polkagent"
"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Existing tests — must continue to pass
    // -----------------------------------------------------------------------

    #[test]
    fn default_config_has_expected_values() {
        let cfg = Config::default();
        assert_eq!(cfg.meta.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(cfg.log.level, "info");
        assert_eq!(cfg.database.backend, DatabaseBackend::Sqlite);
        assert_eq!(cfg.execution.max_concurrent_runs, 10);
        assert_eq!(cfg.execution.default_timeout_secs, 600);
        assert!((cfg.execution.budget.max_usd_per_run - 5.00).abs() < f64::EPSILON);
        assert_eq!(cfg.execution.budget.warn_threshold_percent, 80);
        assert_eq!(cfg.api.bind_address, "127.0.0.1:4840");
        assert!(cfg.api.enabled);
        assert!(!cfg.memory.enabled);
        assert_eq!(cfg.tui.theme, TuiTheme::Dark);
        assert!(cfg.tui.atmospheric_effects);
    }

    #[test]
    fn log_format_serde_round_trip() {
        let json = serde_json::to_string(&LogFormat::Json).expect("serialize");
        let back: LogFormat = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, LogFormat::Json);
    }

    #[test]
    fn tui_theme_serde_round_trip() {
        for theme in [TuiTheme::Dark, TuiTheme::NoColor, TuiTheme::HighContrast] {
            let json = serde_json::to_string(&theme).expect("serialize");
            let back: TuiTheme = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, theme);
        }
    }

    #[test]
    fn database_backend_serde_round_trip() {
        for backend in [DatabaseBackend::Sqlite, DatabaseBackend::Postgres] {
            let json = serde_json::to_string(&backend).expect("serialize");
            let back: DatabaseBackend = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, backend);
        }
    }

    #[test]
    fn config_toml_round_trip() {
        let original = Config::default();
        let serialized = toml::to_string_pretty(&original).expect("serialize to TOML");
        let deserialized: Config = toml::from_str(&serialized).expect("deserialize from TOML");
        assert_eq!(original, deserialized);
    }

    #[test]
    fn default_config_template_is_valid_toml() {
        let parsed: Config =
            toml::from_str(DEFAULT_CONFIG_TEMPLATE).expect("template must be valid TOML");
        assert_eq!(parsed.meta.schema_version, CURRENT_SCHEMA_VERSION);
        // Template includes one provider entry.
        assert_eq!(parsed.providers.len(), 1);
    }

    // -----------------------------------------------------------------------
    // ServerConfig defaults
    // -----------------------------------------------------------------------

    #[test]
    fn server_config_defaults() {
        let cfg = ServerConfig::default();
        assert_eq!(cfg.bind_address, "127.0.0.1:9090");
        assert!(cfg.tls.is_none());
        assert_eq!(cfg.cors_origins, vec!["*".to_owned()]);
        assert_eq!(cfg.rate_limit.requests_per_second, 100);
        assert_eq!(cfg.rate_limit.burst, 200);
    }

    #[test]
    fn tls_config_defaults_all_none() {
        let tls = TlsConfig::default();
        assert!(tls.cert_path.is_none());
        assert!(tls.key_path.is_none());
        assert!(tls.ca_path.is_none());
    }

    #[test]
    fn rate_limit_config_serde_round_trip() {
        let original = RateLimitConfig { enabled: true, requests_per_second: 50, burst: 100 };
        let json = serde_json::to_string(&original).expect("serialize");
        let back: RateLimitConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, original);
    }

    #[test]
    fn server_config_with_tls_round_trip() {
        let original = ServerConfig {
            bind_address: "0.0.0.0:9090".to_owned(),
            tls: Some(TlsConfig {
                cert_path: Some(PathBuf::from("/etc/tls/cert.pem")),
                key_path: Some(PathBuf::from("/etc/tls/key.pem")),
                ca_path: None,
            }),
            cors_origins: vec!["https://example.com".to_owned()],
            rate_limit: RateLimitConfig { enabled: true, requests_per_second: 200, burst: 400 },
        };
        let toml = toml::to_string_pretty(&original).expect("serialize");
        let back: ServerConfig = toml::from_str(&toml).expect("deserialize");
        assert_eq!(back, original);
    }

    // -----------------------------------------------------------------------
    // AuthConfig defaults
    // -----------------------------------------------------------------------

    #[test]
    fn auth_config_defaults() {
        let cfg = AuthConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.api_keys.is_empty());
        assert!(cfg.jwt_secret_env.is_none());
        assert_eq!(cfg.session_timeout_secs, 3600);
    }

    #[test]
    fn auth_config_serde_round_trip() {
        let original = AuthConfig {
            enabled: true,
            api_keys: vec!["hash1".to_owned(), "hash2".to_owned()],
            jwt_secret_env: Some("MY_JWT_SECRET".to_owned()),
            session_timeout_secs: 7200,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let back: AuthConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, original);
    }

    // -----------------------------------------------------------------------
    // SecurityConfig defaults
    // -----------------------------------------------------------------------

    #[test]
    fn security_config_defaults() {
        let cfg = SecurityConfig::default();
        assert!(!cfg.sandbox_enabled);
        assert_eq!(cfg.max_file_size_bytes, 10 * 1024 * 1024);
        assert!(cfg.allowed_paths.is_empty());
        assert!(!cfg.denied_paths.is_empty(), "should have default denied paths");
        assert_eq!(cfg.max_memory_mb, 512);
        assert_eq!(cfg.max_cpu_seconds, 300);
    }

    #[test]
    fn security_config_serde_round_trip() {
        let original = SecurityConfig {
            sandbox_enabled: true,
            max_file_size_bytes: 1024,
            allowed_paths: vec![PathBuf::from("/tmp")],
            denied_paths: vec![PathBuf::from("/etc/shadow")],
            max_memory_mb: 256,
            max_cpu_seconds: 60,
        };
        let toml = toml::to_string_pretty(&original).expect("serialize");
        let back: SecurityConfig = toml::from_str(&toml).expect("deserialize");
        assert_eq!(back, original);
    }

    // -----------------------------------------------------------------------
    // SkillsConfig defaults
    // -----------------------------------------------------------------------

    #[test]
    fn skills_config_defaults() {
        let cfg = SkillsConfig::default();
        assert!(cfg.directories.is_empty());
        assert!(cfg.auto_load);
        assert!(cfg.registry_url.is_none());
        assert!(cfg.cache_dir.is_none());
    }

    #[test]
    fn skills_config_serde_round_trip() {
        let original = SkillsConfig {
            directories: vec![PathBuf::from("/opt/skills"), PathBuf::from("~/skills")],
            auto_load: false,
            registry_url: Some("https://registry.example.com".to_owned()),
            cache_dir: Some(PathBuf::from("~/.cache/polkagent/skills")),
        };
        let toml = toml::to_string_pretty(&original).expect("serialize");
        let back: SkillsConfig = toml::from_str(&toml).expect("deserialize");
        assert_eq!(back, original);
    }

    // -----------------------------------------------------------------------
    // HarnessConfig defaults
    // -----------------------------------------------------------------------

    #[test]
    fn harness_config_defaults() {
        let cfg = HarnessConfig::default();
        assert_eq!(cfg.harness_type, "claude");
        assert!(cfg.binary_path.is_none());
        assert_eq!(cfg.timeout_secs, 300);
        assert_eq!(cfg.max_concurrent, 1);
        assert!(cfg.default.is_none());
        assert!(cfg.harnesses.is_empty());
    }

    #[test]
    fn harness_config_serde_round_trip() {
        let original = HarnessConfig {
            harness_type: "custom".to_owned(),
            binary_path: Some(PathBuf::from("/usr/local/bin/my-harness")),
            timeout_secs: 600,
            max_concurrent: 4,
            default: None,
            harnesses: HashMap::new(),
        };
        let toml = toml::to_string_pretty(&original).expect("serialize");
        let back: HarnessConfig = toml::from_str(&toml).expect("deserialize");
        assert_eq!(back, original);
    }

    // -----------------------------------------------------------------------
    // ArtifactConfig defaults
    // -----------------------------------------------------------------------

    #[test]
    fn artifact_config_defaults() {
        let cfg = ArtifactConfig::default();
        assert_eq!(cfg.max_size_bytes, 100 * 1024 * 1024);
        assert_eq!(cfg.retention_days, 90);
        assert!(cfg.compression_enabled);
    }

    #[test]
    fn artifact_config_serde_round_trip() {
        let original = ArtifactConfig {
            max_size_bytes: 50 * 1024 * 1024,
            retention_days: 30,
            compression_enabled: false,
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let back: ArtifactConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, original);
    }

    // -----------------------------------------------------------------------
    // ObservabilityConfig defaults
    // -----------------------------------------------------------------------

    #[test]
    fn observability_config_defaults() {
        let cfg = ObservabilityConfig::default();
        assert!(cfg.otlp_endpoint.is_none());
        assert_eq!(cfg.otlp_protocol, "grpc");
        assert!(cfg.metrics_enabled);
        assert!(cfg.traces_enabled);
        assert_eq!(cfg.service_name, "polkagent");
    }

    #[test]
    fn observability_config_serde_round_trip() {
        let original = ObservabilityConfig {
            otlp_endpoint: Some("http://localhost:4317".to_owned()),
            otlp_protocol: "http/protobuf".to_owned(),
            metrics_enabled: false,
            traces_enabled: true,
            service_name: "my-service".to_owned(),
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let back: ObservabilityConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, original);
    }

    // -----------------------------------------------------------------------
    // Full Config TOML round-trip with new sections
    // -----------------------------------------------------------------------

    #[test]
    fn full_config_with_new_sections_toml_round_trip() {
        let mut original = Config::default();
        original.server.bind_address = "0.0.0.0:9090".to_owned();
        original.auth.enabled = true;
        original.security.sandbox_enabled = true;
        original.skills.auto_load = false;
        original.harness.max_concurrent = 2;
        original.artifacts.retention_days = 30;
        original.observability.otlp_endpoint = Some("http://otel:4317".to_owned());

        let serialized = toml::to_string_pretty(&original).expect("serialize");
        let deserialized: Config = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(original, deserialized);
    }

    // -----------------------------------------------------------------------
    // Template includes new sections
    // -----------------------------------------------------------------------

    #[test]
    fn template_includes_server_section() {
        assert!(
            DEFAULT_CONFIG_TEMPLATE.contains("[server]"),
            "template must contain [server] section"
        );
    }

    #[test]
    fn template_includes_auth_section() {
        assert!(
            DEFAULT_CONFIG_TEMPLATE.contains("[auth]"),
            "template must contain [auth] section"
        );
    }

    #[test]
    fn template_includes_security_section() {
        assert!(
            DEFAULT_CONFIG_TEMPLATE.contains("[security]"),
            "template must contain [security] section"
        );
    }

    #[test]
    fn template_includes_skills_section() {
        assert!(
            DEFAULT_CONFIG_TEMPLATE.contains("[skills]"),
            "template must contain [skills] section"
        );
    }

    #[test]
    fn template_includes_harness_section() {
        assert!(
            DEFAULT_CONFIG_TEMPLATE.contains("[harness]"),
            "template must contain [harness] section"
        );
    }

    #[test]
    fn template_includes_artifacts_section() {
        assert!(
            DEFAULT_CONFIG_TEMPLATE.contains("[artifacts]"),
            "template must contain [artifacts] section"
        );
    }

    #[test]
    fn template_includes_observability_section() {
        assert!(
            DEFAULT_CONFIG_TEMPLATE.contains("[observability]"),
            "template must contain [observability] section"
        );
    }

    // -----------------------------------------------------------------------
    // ProviderConfig expanded fields (PRD-04a §8)
    // -----------------------------------------------------------------------

    #[test]
    fn provider_config_expanded_defaults() {
        let cfg = ProviderConfig::default();
        assert!(cfg.kind.is_none());
        assert_eq!(cfg.ttft_timeout_secs, Some(15));
        assert_eq!(cfg.connect_timeout_secs, Some(5));
        assert_eq!(cfg.max_concurrent, Some(10));
        assert!(cfg.extra_headers.is_empty());
        assert!(cfg.extra.is_none());
    }

    #[test]
    fn provider_config_expanded_toml_round_trip() {
        let mut headers = HashMap::new();
        headers.insert("X-Custom".to_owned(), "value".to_owned());
        let original = ProviderConfig {
            id: "test".to_owned(),
            provider_type: "anthropic".to_owned(),
            kind: Some(crate::model_registry::ProviderKind::AnthropicApi),
            api_key_env: "KEY".to_owned(),
            base_url: "https://api.example.com".to_owned(),
            default_model: "model-1".to_owned(),
            timeout_secs: 60,
            max_retries: 5,
            ttft_timeout_secs: Some(20),
            connect_timeout_secs: Some(10),
            max_concurrent: Some(5),
            extra_headers: headers,
            extra: None,
        };
        let serialized = toml::to_string_pretty(&original).expect("serialize");
        let back: ProviderConfig = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(back, original);
    }

    #[test]
    fn provider_config_backward_compat_minimal_toml() {
        // A minimal TOML with only the old fields should still parse.
        let toml_str = r#"
id = "p1"
provider_type = "anthropic"
api_key_env = "MY_KEY"
base_url = "https://api.anthropic.com"
default_model = "claude-sonnet-4-6"
timeout_secs = 120
max_retries = 3
"#;
        let cfg: ProviderConfig = toml::from_str(toml_str).expect("should parse minimal provider");
        assert_eq!(cfg.id, "p1");
        assert!(cfg.kind.is_none());
        // Defaults should kick in for new fields.
        assert_eq!(cfg.ttft_timeout_secs, Some(15));
        assert_eq!(cfg.connect_timeout_secs, Some(5));
        assert_eq!(cfg.max_concurrent, Some(10));
        assert!(cfg.extra_headers.is_empty());
        assert!(cfg.extra.is_none());
    }

    // -----------------------------------------------------------------------
    // ModelOverrideConfig (PRD-04a §8)
    // -----------------------------------------------------------------------

    #[test]
    fn model_override_config_defaults() {
        let cfg = ModelOverrideConfig::default();
        assert!(cfg.slug.is_empty());
        assert!(cfg.provider.is_empty());
        assert!(cfg.context_window.is_none());
        assert!(cfg.max_output.is_none());
        assert!(cfg.supports_tools.is_none());
        assert!(cfg.supports_thinking.is_none());
        assert!(cfg.tool_format.is_none());
        assert!(cfg.cost_input_per_m.is_none());
        assert!(cfg.cost_output_per_m.is_none());
    }

    #[test]
    fn model_override_config_toml_round_trip() {
        let original = ModelOverrideConfig {
            slug: "my-model".to_owned(),
            provider: "anthropic-default".to_owned(),
            context_window: Some(200_000),
            max_output: Some(8192),
            supports_tools: Some(true),
            supports_thinking: Some(true),
            tool_format: Some("native".to_owned()),
            cost_input_per_m: Some(3.0),
            cost_output_per_m: Some(15.0),
        };
        let serialized = toml::to_string_pretty(&original).expect("serialize");
        let back: ModelOverrideConfig = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(back, original);
    }

    #[test]
    fn models_array_parses_from_toml() {
        let toml_str = r#"
[[models]]
slug = "model-a"
provider = "p1"
context_window = 100000

[[models]]
slug = "model-b"
provider = "p2"
supports_tools = false
cost_input_per_m = 1.5
"#;
        #[derive(Debug, Deserialize)]
        struct Wrapper {
            models: Vec<ModelOverrideConfig>,
        }
        let w: Wrapper = toml::from_str(toml_str).expect("parse");
        assert_eq!(w.models.len(), 2);
        assert_eq!(w.models[0].slug, "model-a");
        assert_eq!(w.models[0].context_window, Some(100_000));
        assert_eq!(w.models[1].slug, "model-b");
        assert_eq!(w.models[1].supports_tools, Some(false));
    }

    // -----------------------------------------------------------------------
    // HarnessEntryConfig (PRD-04a §8)
    // -----------------------------------------------------------------------

    #[test]
    fn harness_entry_config_defaults() {
        let cfg = HarnessEntryConfig::default();
        assert!(cfg.binary_path.is_none());
        assert!(cfg.transport.is_none());
        assert!(cfg.approval_mode.is_none());
        assert!(cfg.http_port.is_none());
    }

    #[test]
    fn harness_entry_config_toml_round_trip() {
        let original = HarnessEntryConfig {
            binary_path: Some("/usr/local/bin/codex".to_owned()),
            transport: Some("http".to_owned()),
            approval_mode: Some("auto".to_owned()),
            http_port: Some(8080),
        };
        let serialized = toml::to_string_pretty(&original).expect("serialize");
        let back: HarnessEntryConfig = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(back, original);
    }

    #[test]
    fn harness_config_with_entries_toml_round_trip() {
        let mut harnesses = HashMap::new();
        harnesses.insert(
            "codex".to_owned(),
            HarnessEntryConfig {
                binary_path: Some("/usr/local/bin/codex".to_owned()),
                transport: Some("http".to_owned()),
                approval_mode: Some("auto".to_owned()),
                http_port: Some(8080),
            },
        );
        let original = HarnessConfig {
            harness_type: "claude".to_owned(),
            binary_path: None,
            timeout_secs: 300,
            max_concurrent: 4,
            default: Some("codex".to_owned()),
            harnesses,
        };
        let serialized = toml::to_string_pretty(&original).expect("serialize");
        let back: HarnessConfig = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(back, original);
    }

    #[test]
    fn harness_config_backward_compat_minimal_toml() {
        // Old-style TOML without new fields should parse fine.
        let toml_str = r#"
harness_type = "claude"
timeout_secs = 300
max_concurrent = 1
"#;
        let cfg: HarnessConfig = toml::from_str(toml_str).expect("should parse minimal harness");
        assert_eq!(cfg.harness_type, "claude");
        assert!(cfg.default.is_none());
        assert!(cfg.harnesses.is_empty());
    }

    // -----------------------------------------------------------------------
    // Full Config with models field
    // -----------------------------------------------------------------------

    #[test]
    fn config_default_has_empty_models() {
        let cfg = Config::default();
        assert!(cfg.models.is_empty());
    }

    #[test]
    fn full_config_with_models_toml_round_trip() {
        let mut cfg = Config::default();
        cfg.providers = vec![ProviderConfig {
            id: "p1".to_owned(),
            provider_type: "anthropic".to_owned(),
            api_key_env: "KEY".to_owned(),
            ..Default::default()
        }];
        cfg.models = vec![ModelOverrideConfig {
            slug: "custom-model".to_owned(),
            provider: "p1".to_owned(),
            context_window: Some(200_000),
            ..Default::default()
        }];
        let serialized = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(cfg, back);
    }
}

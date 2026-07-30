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

use serde::{Deserialize, Serialize};

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
    /// Policy evaluation settings.
    pub policy: PolicyConfig,
    /// Conversational memory settings.
    pub memory: MemoryConfig,
    /// HTTP API server settings.
    pub api: ApiConfig,
    /// Terminal UI settings.
    pub tui: TuiConfig,
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
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_concurrent_runs: 10,
            default_timeout_secs: 600,
            budget: BudgetConfig::default(),
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Stable identifier used to reference this provider in agent specs.
    pub id: String,
    /// Provider type string (e.g. `"anthropic"`, `"openai_compatible"`).
    pub provider_type: String,
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
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            provider_type: String::new(),
            api_key_env: String::new(),
            base_url: String::new(),
            default_model: String::new(),
            timeout_secs: 120,
            max_retries: 3,
        }
    }
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
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind_address: "127.0.0.1:4840".to_owned(),
            cors_origins: vec!["http://localhost:*".to_owned()],
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
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com"
default_model = "claude-sonnet-4-6"
timeout_secs = 120
max_retries = 3

# [[providers]]
# id = "openai-compat"
# provider_type = "openai_compatible"
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
"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
}

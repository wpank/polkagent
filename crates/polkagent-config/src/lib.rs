//! TOML configuration loading with environment variable overrides for the
//! Polkagent platform.
//!
//! # Quick start
//!
//! ```no_run
//! use polkagent_config::{ConfigLoader, validate};
//!
//! // Load and merge all config layers (defaults -> global -> project -> env vars).
//! let config = ConfigLoader::new().load().expect("failed to load config");
//!
//! // Validate the resolved config before using it.
//! validate::validate(&config).expect("invalid configuration");
//!
//! println!("log level: {}", config.log.level);
//! println!("api bind: {}", config.api.bind_address);
//! ```
//!
//! # Config file locations
//!
//! The loader automatically searches the following paths (later layers win):
//!
//! 1. Built-in defaults (compiled into the binary)
//! 2. `~/.config/polkagent/polkagent.toml` (global user config)
//! 3. `.polkagent/polkagent.toml` in the CWD or any ancestor directory
//!    (project-local config)
//! 4. `POLKAGENT_*` environment variables
//!
//! # Environment variable overrides
//!
//! All settings can be overridden via `POLKAGENT_*` environment variables.
//! See the [`env`] module for the full mapping.
//!
//! # Secrets
//!
//! API keys and database credentials must **never** appear in config files.
//! Use the environment variables listed in the [`env`] module instead.

pub mod env;
pub mod error;
pub mod loader;
pub mod model_registry;
pub mod schema;
pub mod validate;
pub mod watch;

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

pub use error::{ConfigError, Result};
pub use loader::{merge, ConfigLoader};
pub use model_registry::{
    synthesize_providers_from_env, BuiltInModelCatalog, ModelCatalog, ModelDescriptor,
    ProviderKind, ToolFormat,
};
pub use schema::{
    ApiConfig, ArtifactConfig, AuthConfig, BudgetConfig, CloudConfig, Config, DataRegion,
    DatabaseBackend, DatabaseConfig, ExecutionConfig, HarnessConfig, HarnessEntryConfig, LogConfig,
    LogFormat, MemoryConfig, MetaConfig, ModelOverrideConfig, ObservabilityConfig, PolicyConfig,
    PostgresConfig, ProviderConfig, RateLimitConfig, SecurityConfig, ServerConfig, SkillsConfig,
    SqliteConfig, TlsConfig, TuiConfig, TuiTheme, WatcherConfig, WatcherSchedule,
    CURRENT_SCHEMA_VERSION, DEFAULT_CONFIG_TEMPLATE,
};
pub use validate::ValidationError;
pub use watch::{
    AtomicConfig, ConfigDiff, ConfigSnapshot, ConfigWatcher, ReloadPolicy, WatchError, WatchEvent,
    WatchEventKind,
};

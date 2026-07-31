//! Environment-variable overrides for [`Config`].
//!
//! # Naming convention
//!
//! All Polkagent environment variables use the prefix `POLKAGENT_` followed by
//! the section and key in `SCREAMING_SNAKE_CASE`. Nested config keys are
//! separated by underscores (single, not double, for readability):
//!
//! ```text
//! POLKAGENT_LOG_LEVEL           → config.log.level
//! POLKAGENT_DATABASE_BACKEND    → config.database.backend
//! POLKAGENT_API_BIND            → config.api.bind_address
//! POLKAGENT_SERVER_BIND         → config.server.bind_address
//! POLKAGENT_AUTH_ENABLED        → config.auth.enabled
//! POLKAGENT_SECURITY_SANDBOX    → config.security.sandbox_enabled
//! POLKAGENT_SKILLS_DIR          → config.skills.directories (single path appended)
//! POLKAGENT_OTLP_ENDPOINT       → config.observability.otlp_endpoint
//! ```
//!
//! # API keys
//!
//! Provider API keys must **only** come from environment variables; they are
//! never stored in config files. Two convenience variables are recognised and
//! applied to matching providers:
//!
//! - `POLKAGENT_ANTHROPIC_API_KEY` — stored in the Anthropic provider entry's
//!   runtime slot (does not modify the TOML field).
//! - `POLKAGENT_OPENAI_API_KEY` — same for OpenAI-compatible providers.
//!
//! These variables are consumed by the provider harnesses at runtime; the
//! config loader records only whether they are present, never their values.

use std::env;
use std::path::PathBuf;

use tracing::debug;

use crate::schema::{Config, DatabaseBackend, LogFormat, TuiTheme};

/// Apply all recognised `POLKAGENT_*` environment variables to `config`.
///
/// Variables that are unset or empty are silently ignored so that partial
/// environment configurations work without error.
///
/// This function is called as the final step of [`crate::loader::ConfigLoader::load`]
/// so environment variables always take precedence over file-based settings.
pub fn apply_env_overrides(config: &mut Config) {
    apply_log_overrides(config);
    apply_database_overrides(config);
    apply_execution_overrides(config);
    apply_api_overrides(config);
    apply_tui_overrides(config);
    apply_memory_overrides(config);
    apply_policy_overrides(config);
    apply_server_overrides(config);
    apply_auth_overrides(config);
    apply_security_overrides(config);
    apply_skills_overrides(config);
    apply_harness_overrides(config);
    apply_artifact_overrides(config);
    apply_observability_overrides(config);
    check_api_key_env_vars();
}

// ---------------------------------------------------------------------------
// Log
// ---------------------------------------------------------------------------

fn apply_log_overrides(config: &mut Config) {
    if let Some(level) = env_str("POLKAGENT_LOG_LEVEL") {
        debug!(variable = "POLKAGENT_LOG_LEVEL", value = %level, "applying env override");
        config.log.level = level;
    }

    if let Some(format) = env_str("POLKAGENT_LOG_FORMAT") {
        match format.to_lowercase().as_str() {
            "json" => {
                debug!(variable = "POLKAGENT_LOG_FORMAT", value = "json", "applying env override");
                config.log.format = LogFormat::Json;
            }
            "pretty" => {
                debug!(
                    variable = "POLKAGENT_LOG_FORMAT",
                    value = "pretty",
                    "applying env override"
                );
                config.log.format = LogFormat::Pretty;
            }
            other => {
                tracing::warn!(
                    variable = "POLKAGENT_LOG_FORMAT",
                    value = %other,
                    "unrecognised value; expected 'pretty' or 'json' — ignoring"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

fn apply_database_overrides(config: &mut Config) {
    if let Some(backend) = env_str("POLKAGENT_DATABASE_BACKEND") {
        match backend.to_lowercase().as_str() {
            "sqlite" => {
                debug!(variable = "POLKAGENT_DATABASE_BACKEND", value = "sqlite", "applying env override");
                config.database.backend = DatabaseBackend::Sqlite;
            }
            "postgres" => {
                debug!(
                    variable = "POLKAGENT_DATABASE_BACKEND",
                    value = "postgres",
                    "applying env override"
                );
                config.database.backend = DatabaseBackend::Postgres;
            }
            other => {
                tracing::warn!(
                    variable = "POLKAGENT_DATABASE_BACKEND",
                    value = %other,
                    "unrecognised value; expected 'sqlite' or 'postgres' — ignoring"
                );
            }
        }
    }

    if let Some(path) = env_str("POLKAGENT_DATABASE_SQLITE_PATH") {
        debug!(variable = "POLKAGENT_DATABASE_SQLITE_PATH", "applying env override");
        config.database.sqlite.path = path;
    }

    if let Some(url) = env_str("POLKAGENT_DATABASE_POSTGRES_URL") {
        debug!(variable = "POLKAGENT_DATABASE_POSTGRES_URL", "applying env override (value redacted)");
        config.database.postgres.url = url;
    }

    if let Some(max) = env_parse::<u32>("POLKAGENT_DATABASE_POSTGRES_MAX_CONNECTIONS") {
        debug!(variable = "POLKAGENT_DATABASE_POSTGRES_MAX_CONNECTIONS", value = max, "applying env override");
        config.database.postgres.max_connections = max;
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

fn apply_execution_overrides(config: &mut Config) {
    if let Some(max) = env_parse::<u32>("POLKAGENT_EXECUTION_MAX_CONCURRENT_RUNS") {
        debug!(
            variable = "POLKAGENT_EXECUTION_MAX_CONCURRENT_RUNS",
            value = max,
            "applying env override"
        );
        config.execution.max_concurrent_runs = max;
    }

    if let Some(secs) = env_parse::<u64>("POLKAGENT_EXECUTION_DEFAULT_TIMEOUT_SECS") {
        debug!(
            variable = "POLKAGENT_EXECUTION_DEFAULT_TIMEOUT_SECS",
            value = secs,
            "applying env override"
        );
        config.execution.default_timeout_secs = secs;
    }

    if let Some(usd) = env_parse::<f64>("POLKAGENT_EXECUTION_BUDGET_MAX_USD_PER_RUN") {
        debug!(
            variable = "POLKAGENT_EXECUTION_BUDGET_MAX_USD_PER_RUN",
            value = usd,
            "applying env override"
        );
        config.execution.budget.max_usd_per_run = usd;
    }

    if let Some(usd) = env_parse::<f64>("POLKAGENT_EXECUTION_BUDGET_MAX_USD_PER_DAY") {
        debug!(
            variable = "POLKAGENT_EXECUTION_BUDGET_MAX_USD_PER_DAY",
            value = usd,
            "applying env override"
        );
        config.execution.budget.max_usd_per_day = usd;
    }

    if let Some(pct) = env_parse::<u8>("POLKAGENT_EXECUTION_BUDGET_WARN_THRESHOLD_PERCENT") {
        debug!(
            variable = "POLKAGENT_EXECUTION_BUDGET_WARN_THRESHOLD_PERCENT",
            value = pct,
            "applying env override"
        );
        config.execution.budget.warn_threshold_percent = pct;
    }
}

// ---------------------------------------------------------------------------
// API server
// ---------------------------------------------------------------------------

fn apply_api_overrides(config: &mut Config) {
    if let Some(addr) = env_str("POLKAGENT_API_BIND") {
        debug!(variable = "POLKAGENT_API_BIND", value = %addr, "applying env override");
        config.api.bind_address = addr;
    }

    if let Some(enabled) = env_parse::<bool>("POLKAGENT_API_ENABLED") {
        debug!(variable = "POLKAGENT_API_ENABLED", value = enabled, "applying env override");
        config.api.enabled = enabled;
    }
}

// ---------------------------------------------------------------------------
// TUI
// ---------------------------------------------------------------------------

fn apply_tui_overrides(config: &mut Config) {
    if let Some(theme) = env_str("POLKAGENT_TUI_THEME") {
        match theme.to_lowercase().as_str() {
            "dark" => {
                debug!(variable = "POLKAGENT_TUI_THEME", value = "dark", "applying env override");
                config.tui.theme = TuiTheme::Dark;
            }
            "no_color" | "nocolor" | "no-color" => {
                debug!(variable = "POLKAGENT_TUI_THEME", value = "no_color", "applying env override");
                config.tui.theme = TuiTheme::NoColor;
            }
            "high_contrast" | "highcontrast" | "high-contrast" => {
                debug!(
                    variable = "POLKAGENT_TUI_THEME",
                    value = "high_contrast",
                    "applying env override"
                );
                config.tui.theme = TuiTheme::HighContrast;
            }
            other => {
                tracing::warn!(
                    variable = "POLKAGENT_TUI_THEME",
                    value = %other,
                    "unrecognised value; expected 'dark', 'no_color', or 'high_contrast' — ignoring"
                );
            }
        }
    }

    if let Some(effects) = env_parse::<bool>("POLKAGENT_TUI_ATMOSPHERIC_EFFECTS") {
        debug!(
            variable = "POLKAGENT_TUI_ATMOSPHERIC_EFFECTS",
            value = effects,
            "applying env override"
        );
        config.tui.atmospheric_effects = effects;
    }
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

fn apply_memory_overrides(config: &mut Config) {
    if let Some(enabled) = env_parse::<bool>("POLKAGENT_MEMORY_ENABLED") {
        debug!(variable = "POLKAGENT_MEMORY_ENABLED", value = enabled, "applying env override");
        config.memory.enabled = enabled;
    }

    if let Some(backend) = env_str("POLKAGENT_MEMORY_BACKEND") {
        debug!(variable = "POLKAGENT_MEMORY_BACKEND", value = %backend, "applying env override");
        config.memory.backend = backend;
    }
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

fn apply_policy_overrides(config: &mut Config) {
    if let Some(dir) = env_str("POLKAGENT_POLICY_DIR") {
        debug!(variable = "POLKAGENT_POLICY_DIR", value = %dir, "applying env override");
        config.policy.policy_dir = dir;
    }

    if let Some(name) = env_str("POLKAGENT_POLICY_DEFAULT") {
        debug!(variable = "POLKAGENT_POLICY_DEFAULT", value = %name, "applying env override");
        config.policy.default_policy = name;
    }
}

// ---------------------------------------------------------------------------
// Server  (PRD-14 §1)
// ---------------------------------------------------------------------------

fn apply_server_overrides(config: &mut Config) {
    if let Some(addr) = env_str("POLKAGENT_SERVER_BIND") {
        debug!(variable = "POLKAGENT_SERVER_BIND", value = %addr, "applying env override");
        config.server.bind_address = addr;
    }

    if let Some(rps) = env_parse::<u32>("POLKAGENT_SERVER_RATE_LIMIT_RPS") {
        debug!(variable = "POLKAGENT_SERVER_RATE_LIMIT_RPS", value = rps, "applying env override");
        config.server.rate_limit.requests_per_second = rps;
    }

    if let Some(burst) = env_parse::<u32>("POLKAGENT_SERVER_RATE_LIMIT_BURST") {
        debug!(
            variable = "POLKAGENT_SERVER_RATE_LIMIT_BURST",
            value = burst,
            "applying env override"
        );
        config.server.rate_limit.burst = burst;
    }
}

// ---------------------------------------------------------------------------
// Auth  (PRD-14 §2)
// ---------------------------------------------------------------------------

fn apply_auth_overrides(config: &mut Config) {
    if let Some(enabled) = env_parse::<bool>("POLKAGENT_AUTH_ENABLED") {
        debug!(variable = "POLKAGENT_AUTH_ENABLED", value = enabled, "applying env override");
        config.auth.enabled = enabled;
    }

    if let Some(env_name) = env_str("POLKAGENT_AUTH_JWT_SECRET_ENV") {
        debug!(variable = "POLKAGENT_AUTH_JWT_SECRET_ENV", value = %env_name, "applying env override");
        config.auth.jwt_secret_env = Some(env_name);
    }

    if let Some(secs) = env_parse::<u64>("POLKAGENT_AUTH_SESSION_TIMEOUT_SECS") {
        debug!(
            variable = "POLKAGENT_AUTH_SESSION_TIMEOUT_SECS",
            value = secs,
            "applying env override"
        );
        config.auth.session_timeout_secs = secs;
    }
}

// ---------------------------------------------------------------------------
// Security  (PRD-14 §3)
// ---------------------------------------------------------------------------

fn apply_security_overrides(config: &mut Config) {
    if let Some(enabled) = env_parse::<bool>("POLKAGENT_SECURITY_SANDBOX") {
        debug!(variable = "POLKAGENT_SECURITY_SANDBOX", value = enabled, "applying env override");
        config.security.sandbox_enabled = enabled;
    }

    if let Some(bytes) = env_parse::<u64>("POLKAGENT_SECURITY_MAX_FILE_SIZE_BYTES") {
        debug!(
            variable = "POLKAGENT_SECURITY_MAX_FILE_SIZE_BYTES",
            value = bytes,
            "applying env override"
        );
        config.security.max_file_size_bytes = bytes;
    }

    if let Some(mb) = env_parse::<u64>("POLKAGENT_SECURITY_MAX_MEMORY_MB") {
        debug!(variable = "POLKAGENT_SECURITY_MAX_MEMORY_MB", value = mb, "applying env override");
        config.security.max_memory_mb = mb;
    }

    if let Some(secs) = env_parse::<u64>("POLKAGENT_SECURITY_MAX_CPU_SECONDS") {
        debug!(
            variable = "POLKAGENT_SECURITY_MAX_CPU_SECONDS",
            value = secs,
            "applying env override"
        );
        config.security.max_cpu_seconds = secs;
    }
}

// ---------------------------------------------------------------------------
// Skills  (PRD-14 §4)
// ---------------------------------------------------------------------------

fn apply_skills_overrides(config: &mut Config) {
    // POLKAGENT_SKILLS_DIR sets a single skill directory, replacing any
    // file-based `directories` list. For multi-path setups use the config file.
    if let Some(dir) = env_str("POLKAGENT_SKILLS_DIR") {
        debug!(variable = "POLKAGENT_SKILLS_DIR", value = %dir, "applying env override");
        config.skills.directories = vec![PathBuf::from(dir)];
    }

    if let Some(auto) = env_parse::<bool>("POLKAGENT_SKILLS_AUTO_LOAD") {
        debug!(variable = "POLKAGENT_SKILLS_AUTO_LOAD", value = auto, "applying env override");
        config.skills.auto_load = auto;
    }

    if let Some(url) = env_str("POLKAGENT_SKILLS_REGISTRY_URL") {
        debug!(variable = "POLKAGENT_SKILLS_REGISTRY_URL", value = %url, "applying env override");
        config.skills.registry_url = Some(url);
    }
}

// ---------------------------------------------------------------------------
// Harness  (PRD-14 §5)
// ---------------------------------------------------------------------------

fn apply_harness_overrides(config: &mut Config) {
    if let Some(harness_type) = env_str("POLKAGENT_HARNESS_TYPE") {
        debug!(variable = "POLKAGENT_HARNESS_TYPE", value = %harness_type, "applying env override");
        config.harness.harness_type = harness_type;
    }

    if let Some(secs) = env_parse::<u64>("POLKAGENT_HARNESS_TIMEOUT_SECS") {
        debug!(variable = "POLKAGENT_HARNESS_TIMEOUT_SECS", value = secs, "applying env override");
        config.harness.timeout_secs = secs;
    }

    if let Some(max) = env_parse::<u32>("POLKAGENT_HARNESS_MAX_CONCURRENT") {
        debug!(variable = "POLKAGENT_HARNESS_MAX_CONCURRENT", value = max, "applying env override");
        config.harness.max_concurrent = max;
    }
}

// ---------------------------------------------------------------------------
// Artifacts  (PRD-14 §6)
// ---------------------------------------------------------------------------

fn apply_artifact_overrides(config: &mut Config) {
    if let Some(bytes) = env_parse::<u64>("POLKAGENT_ARTIFACTS_MAX_SIZE_BYTES") {
        debug!(
            variable = "POLKAGENT_ARTIFACTS_MAX_SIZE_BYTES",
            value = bytes,
            "applying env override"
        );
        config.artifacts.max_size_bytes = bytes;
    }

    if let Some(days) = env_parse::<u32>("POLKAGENT_ARTIFACTS_RETENTION_DAYS") {
        debug!(
            variable = "POLKAGENT_ARTIFACTS_RETENTION_DAYS",
            value = days,
            "applying env override"
        );
        config.artifacts.retention_days = days;
    }

    if let Some(enabled) = env_parse::<bool>("POLKAGENT_ARTIFACTS_COMPRESSION_ENABLED") {
        debug!(
            variable = "POLKAGENT_ARTIFACTS_COMPRESSION_ENABLED",
            value = enabled,
            "applying env override"
        );
        config.artifacts.compression_enabled = enabled;
    }
}

// ---------------------------------------------------------------------------
// Observability  (PRD-14 §7)
// ---------------------------------------------------------------------------

fn apply_observability_overrides(config: &mut Config) {
    if let Some(endpoint) = env_str("POLKAGENT_OTLP_ENDPOINT") {
        debug!(variable = "POLKAGENT_OTLP_ENDPOINT", value = %endpoint, "applying env override");
        config.observability.otlp_endpoint = Some(endpoint);
    }

    if let Some(protocol) = env_str("POLKAGENT_OTLP_PROTOCOL") {
        debug!(variable = "POLKAGENT_OTLP_PROTOCOL", value = %protocol, "applying env override");
        config.observability.otlp_protocol = protocol;
    }

    if let Some(enabled) = env_parse::<bool>("POLKAGENT_OBSERVABILITY_METRICS_ENABLED") {
        debug!(
            variable = "POLKAGENT_OBSERVABILITY_METRICS_ENABLED",
            value = enabled,
            "applying env override"
        );
        config.observability.metrics_enabled = enabled;
    }

    if let Some(enabled) = env_parse::<bool>("POLKAGENT_OBSERVABILITY_TRACES_ENABLED") {
        debug!(
            variable = "POLKAGENT_OBSERVABILITY_TRACES_ENABLED",
            value = enabled,
            "applying env override"
        );
        config.observability.traces_enabled = enabled;
    }

    if let Some(name) = env_str("POLKAGENT_OBSERVABILITY_SERVICE_NAME") {
        debug!(
            variable = "POLKAGENT_OBSERVABILITY_SERVICE_NAME",
            value = %name,
            "applying env override"
        );
        config.observability.service_name = name;
    }
}

// ---------------------------------------------------------------------------
// API key presence check
// ---------------------------------------------------------------------------

/// Log a debug message if platform-level API key env vars are set.
///
/// The keys are **not** stored in the config struct; they are consumed directly
/// by provider harnesses at runtime. This function only checks for their
/// presence so that startup logs can confirm the keys are available.
fn check_api_key_env_vars() {
    if env::var("POLKAGENT_ANTHROPIC_API_KEY").is_ok_and(|v| !v.is_empty()) {
        debug!("POLKAGENT_ANTHROPIC_API_KEY is set");
    }

    if env::var("POLKAGENT_OPENAI_API_KEY").is_ok_and(|v| !v.is_empty()) {
        debug!("POLKAGENT_OPENAI_API_KEY is set");
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the value of `var` if it is set and non-empty, else `None`.
fn env_str(var: &str) -> Option<String> {
    env::var(var).ok().filter(|v| !v.is_empty())
}

/// Parse the value of `var` into type `T`, returning `None` if the variable is
/// unset, empty, or cannot be parsed. Logs a warning on parse failure.
fn env_parse<T>(var: &str) -> Option<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let raw = env_str(var)?;
    match raw.parse::<T>() {
        Ok(val) => Some(val),
        Err(err) => {
            tracing::warn!(variable = var, value = %raw, error = %err, "failed to parse env var — ignoring");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::schema::Config;

    /// Global mutex serialising all tests that mutate process-level env vars.
    ///
    /// Rust unit tests run in parallel threads within the same process, so any
    /// test that calls `std::env::set_var` / `remove_var` must hold this lock
    /// for the duration of the mutate-assert cycle.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Run a closure with an env var temporarily set, then restore the original.
    ///
    /// Acquires [`ENV_LOCK`] for the duration to prevent concurrent tests from
    /// observing intermediate env states.
    fn with_env<F: FnOnce()>(key: &str, value: &str, f: F) {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        let previous = env::var(key).ok();
        env::set_var(key, value);
        f();
        match previous {
            Some(prev) => env::set_var(key, prev),
            None => env::remove_var(key),
        }
    }

    // -----------------------------------------------------------------------
    // Existing tests — must continue to pass
    // -----------------------------------------------------------------------

    #[test]
    fn log_level_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_LOG_LEVEL", "debug", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.log.level, "debug");
    }

    #[test]
    fn log_format_json_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_LOG_FORMAT", "json", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.log.format, LogFormat::Json);
    }

    #[test]
    fn database_backend_postgres_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_DATABASE_BACKEND", "postgres", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.database.backend, DatabaseBackend::Postgres);
    }

    #[test]
    fn database_postgres_url_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_DATABASE_POSTGRES_URL", "postgres://localhost/test", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.database.postgres.url, "postgres://localhost/test");
    }

    #[test]
    fn api_bind_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_API_BIND", "0.0.0.0:8080", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.api.bind_address, "0.0.0.0:8080");
    }

    #[test]
    fn tui_theme_no_color_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_TUI_THEME", "no_color", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.tui.theme, TuiTheme::NoColor);
    }

    #[test]
    fn tui_theme_high_contrast_override_accepts_aliases() {
        for alias in ["high_contrast", "high-contrast", "highcontrast"] {
            let mut cfg = Config::default();
            with_env("POLKAGENT_TUI_THEME", alias, || {
                apply_env_overrides(&mut cfg);
            });
            assert_eq!(cfg.tui.theme, TuiTheme::HighContrast, "alias '{alias}' should work");
        }
    }

    #[test]
    fn memory_enabled_override() {
        let mut cfg = Config::default();
        assert!(!cfg.memory.enabled);
        with_env("POLKAGENT_MEMORY_ENABLED", "true", || {
            apply_env_overrides(&mut cfg);
        });
        assert!(cfg.memory.enabled);
    }

    #[test]
    fn execution_max_concurrent_runs_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_EXECUTION_MAX_CONCURRENT_RUNS", "42", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.execution.max_concurrent_runs, 42);
    }

    #[test]
    fn invalid_u32_env_var_is_silently_ignored() {
        // Use a budget field that no other test touches, so we avoid parallel
        // test interference on shared env vars.
        let default_timeout = Config::default().execution.default_timeout_secs;
        let mut cfg = Config::default();
        with_env("POLKAGENT_EXECUTION_DEFAULT_TIMEOUT_SECS", "not-a-number", || {
            apply_env_overrides(&mut cfg);
        });
        // The unparseable value must leave the field at its default.
        assert_eq!(cfg.execution.default_timeout_secs, default_timeout);
    }

    #[test]
    fn unset_env_var_does_not_override() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        // Ensure the var is absent.
        env::remove_var("POLKAGENT_LOG_LEVEL");
        let mut cfg = Config::default();
        apply_env_overrides(&mut cfg);
        assert_eq!(cfg.log.level, "info");
    }

    // -----------------------------------------------------------------------
    // New env var override tests (PRD-14)
    // -----------------------------------------------------------------------

    #[test]
    fn server_bind_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_SERVER_BIND", "0.0.0.0:9090", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.server.bind_address, "0.0.0.0:9090");
    }

    #[test]
    fn server_rate_limit_rps_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_SERVER_RATE_LIMIT_RPS", "50", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.server.rate_limit.requests_per_second, 50);
    }

    #[test]
    fn server_rate_limit_burst_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_SERVER_RATE_LIMIT_BURST", "500", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.server.rate_limit.burst, 500);
    }

    #[test]
    fn auth_enabled_override() {
        let mut cfg = Config::default();
        assert!(!cfg.auth.enabled);
        with_env("POLKAGENT_AUTH_ENABLED", "true", || {
            apply_env_overrides(&mut cfg);
        });
        assert!(cfg.auth.enabled);
    }

    #[test]
    fn auth_jwt_secret_env_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_AUTH_JWT_SECRET_ENV", "MY_JWT_SECRET", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.auth.jwt_secret_env, Some("MY_JWT_SECRET".to_owned()));
    }

    #[test]
    fn auth_session_timeout_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_AUTH_SESSION_TIMEOUT_SECS", "7200", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.auth.session_timeout_secs, 7200);
    }

    #[test]
    fn security_sandbox_override() {
        let mut cfg = Config::default();
        assert!(!cfg.security.sandbox_enabled);
        with_env("POLKAGENT_SECURITY_SANDBOX", "true", || {
            apply_env_overrides(&mut cfg);
        });
        assert!(cfg.security.sandbox_enabled);
    }

    #[test]
    fn security_max_file_size_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_SECURITY_MAX_FILE_SIZE_BYTES", "1048576", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.security.max_file_size_bytes, 1_048_576);
    }

    #[test]
    fn security_max_memory_mb_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_SECURITY_MAX_MEMORY_MB", "256", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.security.max_memory_mb, 256);
    }

    #[test]
    fn skills_dir_override_replaces_directories() {
        let mut cfg = Config::default();
        cfg.skills.directories = vec![PathBuf::from("/old/path")];
        with_env("POLKAGENT_SKILLS_DIR", "/new/skills", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.skills.directories, vec![PathBuf::from("/new/skills")]);
    }

    #[test]
    fn skills_auto_load_override() {
        let mut cfg = Config::default();
        assert!(cfg.skills.auto_load);
        with_env("POLKAGENT_SKILLS_AUTO_LOAD", "false", || {
            apply_env_overrides(&mut cfg);
        });
        assert!(!cfg.skills.auto_load);
    }

    #[test]
    fn skills_registry_url_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_SKILLS_REGISTRY_URL", "https://registry.example.com", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(
            cfg.skills.registry_url,
            Some("https://registry.example.com".to_owned())
        );
    }

    #[test]
    fn harness_type_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_HARNESS_TYPE", "custom", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.harness.harness_type, "custom");
    }

    #[test]
    fn harness_timeout_secs_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_HARNESS_TIMEOUT_SECS", "600", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.harness.timeout_secs, 600);
    }

    #[test]
    fn harness_max_concurrent_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_HARNESS_MAX_CONCURRENT", "4", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.harness.max_concurrent, 4);
    }

    #[test]
    fn artifacts_retention_days_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_ARTIFACTS_RETENTION_DAYS", "30", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.artifacts.retention_days, 30);
    }

    #[test]
    fn artifacts_compression_disabled_override() {
        let mut cfg = Config::default();
        assert!(cfg.artifacts.compression_enabled);
        with_env("POLKAGENT_ARTIFACTS_COMPRESSION_ENABLED", "false", || {
            apply_env_overrides(&mut cfg);
        });
        assert!(!cfg.artifacts.compression_enabled);
    }

    #[test]
    fn otlp_endpoint_override() {
        let mut cfg = Config::default();
        assert!(cfg.observability.otlp_endpoint.is_none());
        with_env("POLKAGENT_OTLP_ENDPOINT", "http://localhost:4317", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(
            cfg.observability.otlp_endpoint,
            Some("http://localhost:4317".to_owned())
        );
    }

    #[test]
    fn otlp_protocol_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_OTLP_PROTOCOL", "http/protobuf", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.observability.otlp_protocol, "http/protobuf");
    }

    #[test]
    fn observability_service_name_override() {
        let mut cfg = Config::default();
        with_env("POLKAGENT_OBSERVABILITY_SERVICE_NAME", "my-service", || {
            apply_env_overrides(&mut cfg);
        });
        assert_eq!(cfg.observability.service_name, "my-service");
    }
}

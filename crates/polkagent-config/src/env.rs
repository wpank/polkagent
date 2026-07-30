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
}

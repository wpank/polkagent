//! Configuration validation.
//!
//! Validation is intentionally separate from deserialization so that a
//! partially-overridden config (e.g. one built from env-var overrides only)
//! can be validated before use.
//!
//! # Usage
//!
//! ```no_run
//! use polkagent_config::{Config, validate};
//!
//! let cfg = Config::default();
//! validate::validate(&cfg).expect("default config is valid");
//! ```

use std::net::SocketAddr;

use crate::schema::{Config, DatabaseBackend, CURRENT_SCHEMA_VERSION};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// A single validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Dot-separated config path that failed (e.g. `"api.bind_address"`).
    pub field: String,
    /// Human-readable description of the problem.
    pub message: String,
}

impl ValidationError {
    fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

// ---------------------------------------------------------------------------
// Public entry-point
// ---------------------------------------------------------------------------

/// Validate `config`, returning all errors found.
///
/// Returns `Ok(())` if no problems are detected, or `Err(errors)` with the
/// full list of violations. Callers should display all errors rather than
/// stopping at the first.
///
/// # Errors
///
/// Returns `Err` containing one or more [`ValidationError`] values when the
/// configuration contains invalid settings.
pub fn validate(config: &Config) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    validate_meta(config, &mut errors);
    validate_log(config, &mut errors);
    validate_database(config, &mut errors);
    validate_execution(config, &mut errors);
    validate_providers(config, &mut errors);
    validate_api(config, &mut errors);
    validate_no_secrets_in_config(config, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Per-section validators
// ---------------------------------------------------------------------------

fn validate_meta(config: &Config, errors: &mut Vec<ValidationError>) {
    if config.meta.schema_version != CURRENT_SCHEMA_VERSION {
        errors.push(ValidationError::new(
            "meta.schema_version",
            format!(
                "unknown schema version {}; only version {} is supported",
                config.meta.schema_version, CURRENT_SCHEMA_VERSION
            ),
        ));
    }

    if config.meta.api_version.is_empty() {
        errors.push(ValidationError::new("meta.api_version", "must not be empty"));
    }
}

fn validate_log(config: &Config, errors: &mut Vec<ValidationError>) {
    let valid_levels = ["trace", "debug", "info", "warn", "error"];
    if !valid_levels.contains(&config.log.level.to_lowercase().as_str()) {
        errors.push(ValidationError::new(
            "log.level",
            format!(
                "invalid log level '{}'; expected one of: {}",
                config.log.level,
                valid_levels.join(", ")
            ),
        ));
    }
}

fn validate_database(config: &Config, errors: &mut Vec<ValidationError>) {
    match config.database.backend {
        DatabaseBackend::Sqlite => {
            if config.database.sqlite.path.is_empty() {
                errors.push(ValidationError::new(
                    "database.sqlite.path",
                    "path must not be empty",
                ));
            }
            // busy_timeout_ms of 0 is technically valid but suspicious; warn
            // via a constraint rather than a hard error (keep it permissive).
        }
        DatabaseBackend::Postgres => {
            // URL may be empty in the config file when loaded from env var.
            // We only reject it if it is both empty AND the env var is absent.
            let url_from_env = std::env::var("POLKAGENT_DATABASE_POSTGRES_URL")
                .is_ok_and(|v| !v.is_empty());
            if config.database.postgres.url.is_empty() && !url_from_env {
                errors.push(ValidationError::new(
                    "database.postgres.url",
                    "postgres URL is required; set POLKAGENT_DATABASE_POSTGRES_URL or \
                     database.postgres.url in config (note: storing secrets in config files \
                     is discouraged)",
                ));
            }
            if config.database.postgres.max_connections == 0 {
                errors.push(ValidationError::new(
                    "database.postgres.max_connections",
                    "must be at least 1",
                ));
            }
            let valid_ssl_modes = ["disable", "prefer", "require"];
            if !valid_ssl_modes.contains(&config.database.postgres.ssl_mode.to_lowercase().as_str()) {
                errors.push(ValidationError::new(
                    "database.postgres.ssl_mode",
                    format!(
                        "invalid ssl_mode '{}'; expected one of: {}",
                        config.database.postgres.ssl_mode,
                        valid_ssl_modes.join(", ")
                    ),
                ));
            }
        }
    }
}

fn validate_execution(config: &Config, errors: &mut Vec<ValidationError>) {
    if config.execution.max_concurrent_runs == 0 {
        errors.push(ValidationError::new(
            "execution.max_concurrent_runs",
            "must be at least 1",
        ));
    }

    if config.execution.default_timeout_secs == 0 {
        errors.push(ValidationError::new(
            "execution.default_timeout_secs",
            "must be greater than 0",
        ));
    }

    if config.execution.budget.max_usd_per_run < 0.0 {
        errors.push(ValidationError::new(
            "execution.budget.max_usd_per_run",
            "must be non-negative",
        ));
    }

    if config.execution.budget.max_usd_per_day < 0.0 {
        errors.push(ValidationError::new(
            "execution.budget.max_usd_per_day",
            "must be non-negative",
        ));
    }

    if config.execution.budget.warn_threshold_percent > 100 {
        errors.push(ValidationError::new(
            "execution.budget.warn_threshold_percent",
            "must be between 0 and 100",
        ));
    }
}

fn validate_providers(config: &Config, errors: &mut Vec<ValidationError>) {
    let mut seen_ids = std::collections::HashSet::new();
    for (idx, provider) in config.providers.iter().enumerate() {
        let prefix = format!("providers[{idx}]");

        if provider.id.is_empty() {
            errors.push(ValidationError::new(format!("{prefix}.id"), "must not be empty"));
        } else if !seen_ids.insert(provider.id.clone()) {
            errors.push(ValidationError::new(
                format!("{prefix}.id"),
                format!("duplicate provider id '{}'", provider.id),
            ));
        }

        if provider.provider_type.is_empty() {
            errors.push(ValidationError::new(
                format!("{prefix}.provider_type"),
                "must not be empty",
            ));
        }

        if provider.timeout_secs == 0 {
            errors.push(ValidationError::new(
                format!("{prefix}.timeout_secs"),
                "must be greater than 0",
            ));
        }

        // Warn (via a validation error) if both api_key_env is empty and no
        // POLKAGENT_*_API_KEY env var is set.
        if provider.api_key_env.is_empty() {
            errors.push(ValidationError::new(
                format!("{prefix}.api_key_env"),
                "should specify an env var name for the API key (e.g. 'ANTHROPIC_API_KEY')",
            ));
        }
    }
}

fn validate_api(config: &Config, errors: &mut Vec<ValidationError>) {
    if config.api.enabled {
        let addr: Result<SocketAddr, _> = config.api.bind_address.parse();
        match addr {
            Ok(socket_addr) => {
                let port = socket_addr.port();
                if port == 0 {
                    errors.push(ValidationError::new(
                        "api.bind_address",
                        "port 0 is not allowed; specify a port in the range 1–65535",
                    ));
                }
            }
            Err(_) => {
                errors.push(ValidationError::new(
                    "api.bind_address",
                    format!(
                        "'{}' is not a valid socket address (expected format: '127.0.0.1:4840')",
                        config.api.bind_address
                    ),
                ));
            }
        }
    }
}

/// Reject configs that appear to contain literal secret values in fields that
/// should only reference environment variable names.
///
/// Heuristic: if `postgres.url` is non-empty in a config that was not
/// overridden by an env var, the operator has stored a connection string
/// (potentially containing credentials) in the config file.  We flag this.
fn validate_no_secrets_in_config(config: &Config, errors: &mut Vec<ValidationError>) {
    // Postgres URL in the config struct is fine only when it came from an env
    // var override. We cannot distinguish perfectly at this stage, so we apply
    // a content heuristic: if it looks like a connection string with embedded
    // credentials, reject it.
    if !config.database.postgres.url.is_empty()
        && looks_like_connection_string_with_credentials(&config.database.postgres.url)
    {
        errors.push(ValidationError::new(
            "database.postgres.url",
            "appears to contain credentials; use POLKAGENT_DATABASE_POSTGRES_URL env var instead",
        ));
    }
}

/// Returns `true` if `s` looks like a database URL with embedded credentials
/// (i.e. it contains `://user:password@`).
fn looks_like_connection_string_with_credentials(s: &str) -> bool {
    // Match patterns like postgres://user:pass@host or postgresql://user:pass@host
    if let Some(after_scheme) = s.find("://").map(|i| &s[i + 3..]) {
        // If there is an `@` and a `:` before it in the authority section, the
        // URL encodes user:password.
        if let Some(at_pos) = after_scheme.find('@') {
            let authority = &after_scheme[..at_pos];
            return authority.contains(':');
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::schema::{Config, DatabaseBackend};

    /// Serialise tests that read or mutate process-level env vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_config_is_valid() {
        let cfg = Config::default();
        // Default config has no providers, so validation should pass cleanly.
        validate(&cfg).expect("default config must be valid");
    }

    #[test]
    fn invalid_log_level_is_rejected() {
        let mut cfg = Config::default();
        cfg.log.level = "verbose".to_owned(); // not a valid tracing level
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "log.level"),
            "expected log.level error, got: {errs:?}"
        );
    }

    #[test]
    fn bad_api_bind_address_is_rejected() {
        let mut cfg = Config::default();
        cfg.api.bind_address = "not-a-socket-address".to_owned();
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "api.bind_address"),
            "expected api.bind_address error, got: {errs:?}"
        );
    }

    #[test]
    fn port_zero_in_bind_address_is_rejected() {
        let mut cfg = Config::default();
        cfg.api.bind_address = "127.0.0.1:0".to_owned();
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "api.bind_address"),
            "expected api.bind_address port-zero error, got: {errs:?}"
        );
    }

    #[test]
    fn zero_concurrent_runs_is_rejected() {
        let mut cfg = Config::default();
        cfg.execution.max_concurrent_runs = 0;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "execution.max_concurrent_runs"),
            "expected max_concurrent_runs error, got: {errs:?}"
        );
    }

    #[test]
    fn warn_threshold_over_100_is_rejected() {
        let mut cfg = Config::default();
        cfg.execution.budget.warn_threshold_percent = 101;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "execution.budget.warn_threshold_percent"),
            "got: {errs:?}"
        );
    }

    #[test]
    fn postgres_backend_without_url_env_var_is_rejected() {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        // Make sure the env var is absent.
        std::env::remove_var("POLKAGENT_DATABASE_POSTGRES_URL");
        let mut cfg = Config::default();
        cfg.database.backend = DatabaseBackend::Postgres;
        cfg.database.postgres.url = String::new();
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "database.postgres.url"),
            "expected postgres url error, got: {errs:?}"
        );
    }

    #[test]
    fn connection_string_with_credentials_is_rejected() {
        let mut cfg = Config::default();
        cfg.database.postgres.url = "postgres://user:s3cret@localhost/db".to_owned();
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "database.postgres.url"),
            "expected credential-in-url error, got: {errs:?}"
        );
    }

    #[test]
    fn unknown_schema_version_is_rejected() {
        let mut cfg = Config::default();
        cfg.meta.schema_version = 999;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "meta.schema_version"),
            "got: {errs:?}"
        );
    }

    #[test]
    fn duplicate_provider_ids_are_rejected() {
        let mut cfg = Config::default();
        let p1 = crate::schema::ProviderConfig {
            id: "dup".to_owned(),
            provider_type: "anthropic".to_owned(),
            api_key_env: "MY_KEY".to_owned(),
            timeout_secs: 30,
            ..Default::default()
        };
        cfg.providers = vec![p1.clone(), p1];
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.message.contains("duplicate")),
            "expected duplicate provider error, got: {errs:?}"
        );
    }

    #[test]
    fn validation_collects_all_errors() {
        let mut cfg = Config::default();
        cfg.log.level = "bad_level".to_owned();
        cfg.api.bind_address = "bad_address".to_owned();
        cfg.execution.max_concurrent_runs = 0;
        let errs = validate(&cfg).unwrap_err();
        // At least three distinct errors.
        assert!(errs.len() >= 3, "expected multiple errors, got: {errs:?}");
    }

    #[test]
    fn provider_with_zero_timeout_is_rejected() {
        let mut cfg = Config::default();
        cfg.providers = vec![crate::schema::ProviderConfig {
            id: "p1".to_owned(),
            provider_type: "anthropic".to_owned(),
            api_key_env: "KEY".to_owned(),
            timeout_secs: 0,
            ..Default::default()
        }];
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field.ends_with("timeout_secs")),
            "got: {errs:?}"
        );
    }
}

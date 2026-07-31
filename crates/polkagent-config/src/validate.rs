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
    validate_server(config, &mut errors);
    validate_auth(config, &mut errors);
    validate_security(config, &mut errors);
    validate_skills(config, &mut errors);
    validate_harness(config, &mut errors);
    validate_artifact(config, &mut errors);
    validate_observability(config, &mut errors);

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
// Server  (PRD-14 §1)
// ---------------------------------------------------------------------------

fn validate_server(config: &Config, errors: &mut Vec<ValidationError>) {
    // Validate bind address is parseable as a socket address.
    let addr: Result<SocketAddr, _> = config.server.bind_address.parse();
    match addr {
        Ok(socket_addr) => {
            if socket_addr.port() == 0 {
                errors.push(ValidationError::new(
                    "server.bind_address",
                    "port 0 is not allowed; specify a port in the range 1–65535",
                ));
            }
        }
        Err(_) => {
            errors.push(ValidationError::new(
                "server.bind_address",
                format!(
                    "'{}' is not a valid socket address (expected format: '127.0.0.1:9090')",
                    config.server.bind_address
                ),
            ));
        }
    }

    // Validate rate limit values are non-zero.
    if config.server.rate_limit.requests_per_second == 0 {
        errors.push(ValidationError::new(
            "server.rate_limit.requests_per_second",
            "must be greater than 0",
        ));
    }

    if config.server.rate_limit.burst == 0 {
        errors.push(ValidationError::new(
            "server.rate_limit.burst",
            "must be greater than 0",
        ));
    }

    // Validate TLS configuration: if TLS is set, cert and key paths are required.
    if let Some(tls) = &config.server.tls {
        if tls.cert_path.is_none() {
            errors.push(ValidationError::new(
                "server.tls.cert_path",
                "TLS is configured but cert_path is missing",
            ));
        }

        if tls.key_path.is_none() {
            errors.push(ValidationError::new(
                "server.tls.key_path",
                "TLS is configured but key_path is missing",
            ));
        }

        // If both paths are provided, check they exist on the filesystem.
        if let Some(cert) = &tls.cert_path {
            if !cert.exists() {
                errors.push(ValidationError::new(
                    "server.tls.cert_path",
                    format!("certificate file '{}' does not exist", cert.display()),
                ));
            }
        }

        if let Some(key) = &tls.key_path {
            if !key.exists() {
                errors.push(ValidationError::new(
                    "server.tls.key_path",
                    format!("key file '{}' does not exist", key.display()),
                ));
            }
        }

        if let Some(ca) = &tls.ca_path {
            if !ca.exists() {
                errors.push(ValidationError::new(
                    "server.tls.ca_path",
                    format!("CA certificate file '{}' does not exist", ca.display()),
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Auth  (PRD-14 §2)
// ---------------------------------------------------------------------------

fn validate_auth(config: &Config, errors: &mut Vec<ValidationError>) {
    if config.auth.session_timeout_secs == 0 {
        errors.push(ValidationError::new(
            "auth.session_timeout_secs",
            "must be greater than 0",
        ));
    }

    // If authentication is enabled and no API keys are configured, at least a
    // JWT secret env var should be named — warn otherwise.
    if config.auth.enabled
        && config.auth.api_keys.is_empty()
        && config.auth.jwt_secret_env.is_none()
    {
        errors.push(ValidationError::new(
            "auth",
            "authentication is enabled but neither api_keys nor jwt_secret_env is configured",
        ));
    }
}

// ---------------------------------------------------------------------------
// Security  (PRD-14 §3)
// ---------------------------------------------------------------------------

fn validate_security(config: &Config, errors: &mut Vec<ValidationError>) {
    // Sanity-check file size limit: must be at least 1 byte.
    if config.security.max_file_size_bytes == 0 {
        errors.push(ValidationError::new(
            "security.max_file_size_bytes",
            "must be at least 1",
        ));
    }

    // Sanity-check memory limit: at least 1 MiB.
    if config.security.max_memory_mb == 0 {
        errors.push(ValidationError::new(
            "security.max_memory_mb",
            "must be at least 1",
        ));
    }

    // Sanity-check CPU seconds: at least 1 second.
    if config.security.max_cpu_seconds == 0 {
        errors.push(ValidationError::new(
            "security.max_cpu_seconds",
            "must be at least 1",
        ));
    }

    // Warn if sandbox is enabled but limits look suspiciously low.
    if config.security.sandbox_enabled && config.security.max_memory_mb < 16 {
        errors.push(ValidationError::new(
            "security.max_memory_mb",
            "value is very low for a sandboxed agent; consider at least 16 MiB",
        ));
    }
}

// ---------------------------------------------------------------------------
// Skills  (PRD-14 §4)
// ---------------------------------------------------------------------------

fn validate_skills(config: &Config, errors: &mut Vec<ValidationError>) {
    // Check that each declared skill directory exists.
    for (idx, dir) in config.skills.directories.iter().enumerate() {
        if !dir.exists() {
            errors.push(ValidationError::new(
                format!("skills.directories[{idx}]"),
                format!("skill directory '{}' does not exist", dir.display()),
            ));
        }
    }

    // Cache dir, if set, should be an absolute path or tilde-prefixed.
    // We don't expand tildes here but we can flag obviously relative paths.
    if let Some(cache) = &config.skills.cache_dir {
        if cache.is_relative() && !cache.starts_with("~") {
            errors.push(ValidationError::new(
                "skills.cache_dir",
                format!(
                    "'{}' is a relative path; use an absolute path or tilde-prefixed path",
                    cache.display()
                ),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Harness  (PRD-14 §5)
// ---------------------------------------------------------------------------

fn validate_harness(config: &Config, errors: &mut Vec<ValidationError>) {
    if config.harness.harness_type.is_empty() {
        errors.push(ValidationError::new(
            "harness.harness_type",
            "must not be empty",
        ));
    }

    if config.harness.timeout_secs == 0 {
        errors.push(ValidationError::new(
            "harness.timeout_secs",
            "must be greater than 0",
        ));
    }

    if config.harness.max_concurrent == 0 {
        errors.push(ValidationError::new(
            "harness.max_concurrent",
            "must be at least 1",
        ));
    }

    // If a custom binary path is specified, check it exists.
    if let Some(bin) = &config.harness.binary_path {
        if !bin.exists() {
            errors.push(ValidationError::new(
                "harness.binary_path",
                format!("harness binary '{}' does not exist", bin.display()),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Artifact  (PRD-14 §6)
// ---------------------------------------------------------------------------

fn validate_artifact(config: &Config, errors: &mut Vec<ValidationError>) {
    if config.artifacts.max_size_bytes == 0 {
        errors.push(ValidationError::new(
            "artifacts.max_size_bytes",
            "must be at least 1",
        ));
    }

    if config.artifacts.retention_days == 0 {
        errors.push(ValidationError::new(
            "artifacts.retention_days",
            "must be at least 1",
        ));
    }
}

// ---------------------------------------------------------------------------
// Observability  (PRD-14 §7)
// ---------------------------------------------------------------------------

fn validate_observability(config: &Config, errors: &mut Vec<ValidationError>) {
    if config.observability.service_name.is_empty() {
        errors.push(ValidationError::new(
            "observability.service_name",
            "must not be empty",
        ));
    }

    let valid_protocols = ["grpc", "http/protobuf"];
    if !valid_protocols.contains(&config.observability.otlp_protocol.to_lowercase().as_str()) {
        errors.push(ValidationError::new(
            "observability.otlp_protocol",
            format!(
                "invalid otlp_protocol '{}'; expected one of: {}",
                config.observability.otlp_protocol,
                valid_protocols.join(", ")
            ),
        ));
    }

    // If an OTLP endpoint is set, ensure it looks like a URL.
    if let Some(endpoint) = &config.observability.otlp_endpoint {
        if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
            errors.push(ValidationError::new(
                "observability.otlp_endpoint",
                format!(
                    "'{}' does not look like an HTTP/HTTPS URL (expected 'http://' or 'https://' prefix)",
                    endpoint
                ),
            ));
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
    use crate::schema::{
        ArtifactConfig, AuthConfig, Config, DatabaseBackend, HarnessConfig, ObservabilityConfig,
        RateLimitConfig, SecurityConfig, ServerConfig, SkillsConfig, TlsConfig,
    };

    /// Serialise tests that read or mutate process-level env vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // -----------------------------------------------------------------------
    // Existing tests — must continue to pass
    // -----------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Server validation (PRD-14 §1)
    // -----------------------------------------------------------------------

    #[test]
    fn server_bad_bind_address_is_rejected() {
        let mut cfg = Config::default();
        cfg.server.bind_address = "not-valid".to_owned();
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "server.bind_address"),
            "expected server.bind_address error, got: {errs:?}"
        );
    }

    #[test]
    fn server_port_zero_is_rejected() {
        let mut cfg = Config::default();
        cfg.server.bind_address = "0.0.0.0:0".to_owned();
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "server.bind_address"),
            "expected server.bind_address port-zero error, got: {errs:?}"
        );
    }

    #[test]
    fn server_rate_limit_rps_zero_is_rejected() {
        let mut cfg = Config::default();
        cfg.server.rate_limit.requests_per_second = 0;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "server.rate_limit.requests_per_second"),
            "expected rate_limit.requests_per_second error, got: {errs:?}"
        );
    }

    #[test]
    fn server_rate_limit_burst_zero_is_rejected() {
        let mut cfg = Config::default();
        cfg.server.rate_limit.burst = 0;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "server.rate_limit.burst"),
            "expected rate_limit.burst error, got: {errs:?}"
        );
    }

    #[test]
    fn server_tls_missing_cert_path_is_rejected() {
        let mut cfg = Config::default();
        cfg.server.tls = Some(TlsConfig {
            cert_path: None,
            key_path: Some(std::path::PathBuf::from("/etc/tls/key.pem")),
            ca_path: None,
        });
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "server.tls.cert_path"),
            "expected tls.cert_path error, got: {errs:?}"
        );
    }

    #[test]
    fn server_tls_missing_key_path_is_rejected() {
        let mut cfg = Config::default();
        cfg.server.tls = Some(TlsConfig {
            cert_path: Some(std::path::PathBuf::from("/etc/tls/cert.pem")),
            key_path: None,
            ca_path: None,
        });
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "server.tls.key_path"),
            "expected tls.key_path error, got: {errs:?}"
        );
    }

    #[test]
    fn server_tls_nonexistent_cert_path_is_rejected() {
        let mut cfg = Config::default();
        cfg.server.tls = Some(TlsConfig {
            cert_path: Some(std::path::PathBuf::from(
                "/nonexistent/path/to/cert.pem",
            )),
            key_path: Some(std::path::PathBuf::from(
                "/nonexistent/path/to/key.pem",
            )),
            ca_path: None,
        });
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "server.tls.cert_path"),
            "expected tls.cert_path not-exists error, got: {errs:?}"
        );
    }

    #[test]
    fn server_no_tls_is_valid() {
        let cfg = Config::default();
        // Default has no TLS — should be valid.
        assert!(cfg.server.tls.is_none());
        validate(&cfg).expect("config with no TLS should be valid");
    }

    // -----------------------------------------------------------------------
    // Auth validation (PRD-14 §2)
    // -----------------------------------------------------------------------

    #[test]
    fn auth_session_timeout_zero_is_rejected() {
        let mut cfg = Config::default();
        cfg.auth.session_timeout_secs = 0;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "auth.session_timeout_secs"),
            "expected auth.session_timeout_secs error, got: {errs:?}"
        );
    }

    #[test]
    fn auth_enabled_without_credentials_is_rejected() {
        let mut cfg = Config::default();
        cfg.auth.enabled = true;
        cfg.auth.api_keys = Vec::new();
        cfg.auth.jwt_secret_env = None;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "auth"),
            "expected auth configuration error, got: {errs:?}"
        );
    }

    #[test]
    fn auth_enabled_with_api_keys_is_valid() {
        let mut cfg = Config::default();
        cfg.auth.enabled = true;
        cfg.auth.api_keys = vec!["hashed_key_here".to_owned()];
        validate(&cfg).expect("auth with api_keys should be valid");
    }

    #[test]
    fn auth_enabled_with_jwt_secret_env_is_valid() {
        let mut cfg = Config::default();
        cfg.auth.enabled = true;
        cfg.auth.jwt_secret_env = Some("MY_JWT_SECRET".to_owned());
        validate(&cfg).expect("auth with jwt_secret_env should be valid");
    }

    // -----------------------------------------------------------------------
    // Security validation (PRD-14 §3)
    // -----------------------------------------------------------------------

    #[test]
    fn security_zero_max_file_size_is_rejected() {
        let mut cfg = Config::default();
        cfg.security.max_file_size_bytes = 0;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "security.max_file_size_bytes"),
            "expected max_file_size_bytes error, got: {errs:?}"
        );
    }

    #[test]
    fn security_zero_max_memory_is_rejected() {
        let mut cfg = Config::default();
        cfg.security.max_memory_mb = 0;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "security.max_memory_mb"),
            "expected max_memory_mb error, got: {errs:?}"
        );
    }

    #[test]
    fn security_zero_cpu_seconds_is_rejected() {
        let mut cfg = Config::default();
        cfg.security.max_cpu_seconds = 0;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "security.max_cpu_seconds"),
            "expected max_cpu_seconds error, got: {errs:?}"
        );
    }

    #[test]
    fn security_sandbox_with_very_low_memory_is_rejected() {
        let mut cfg = Config::default();
        cfg.security.sandbox_enabled = true;
        cfg.security.max_memory_mb = 4; // below 16 MiB threshold
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "security.max_memory_mb"),
            "expected low memory warning, got: {errs:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Skills validation (PRD-14 §4)
    // -----------------------------------------------------------------------

    #[test]
    fn skills_nonexistent_directory_is_rejected() {
        let mut cfg = Config::default();
        cfg.skills.directories = vec![std::path::PathBuf::from(
            "/nonexistent/skills/path/that/does/not/exist",
        )];
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field.starts_with("skills.directories")),
            "expected skills.directories error, got: {errs:?}"
        );
    }

    #[test]
    fn skills_empty_directories_is_valid() {
        let cfg = Config::default();
        assert!(cfg.skills.directories.is_empty());
        validate(&cfg).expect("empty skill directories should be valid");
    }

    // -----------------------------------------------------------------------
    // Harness validation (PRD-14 §5)
    // -----------------------------------------------------------------------

    #[test]
    fn harness_empty_type_is_rejected() {
        let mut cfg = Config::default();
        cfg.harness.harness_type = String::new();
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "harness.harness_type"),
            "expected harness.harness_type error, got: {errs:?}"
        );
    }

    #[test]
    fn harness_zero_timeout_is_rejected() {
        let mut cfg = Config::default();
        cfg.harness.timeout_secs = 0;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "harness.timeout_secs"),
            "expected harness.timeout_secs error, got: {errs:?}"
        );
    }

    #[test]
    fn harness_zero_max_concurrent_is_rejected() {
        let mut cfg = Config::default();
        cfg.harness.max_concurrent = 0;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "harness.max_concurrent"),
            "expected harness.max_concurrent error, got: {errs:?}"
        );
    }

    #[test]
    fn harness_nonexistent_binary_path_is_rejected() {
        let mut cfg = Config::default();
        cfg.harness.binary_path =
            Some(std::path::PathBuf::from("/nonexistent/harness/binary"));
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "harness.binary_path"),
            "expected harness.binary_path error, got: {errs:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Artifact validation (PRD-14 §6)
    // -----------------------------------------------------------------------

    #[test]
    fn artifact_zero_max_size_is_rejected() {
        let mut cfg = Config::default();
        cfg.artifacts.max_size_bytes = 0;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "artifacts.max_size_bytes"),
            "expected artifacts.max_size_bytes error, got: {errs:?}"
        );
    }

    #[test]
    fn artifact_zero_retention_days_is_rejected() {
        let mut cfg = Config::default();
        cfg.artifacts.retention_days = 0;
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "artifacts.retention_days"),
            "expected artifacts.retention_days error, got: {errs:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Observability validation (PRD-14 §7)
    // -----------------------------------------------------------------------

    #[test]
    fn observability_empty_service_name_is_rejected() {
        let mut cfg = Config::default();
        cfg.observability.service_name = String::new();
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "observability.service_name"),
            "expected observability.service_name error, got: {errs:?}"
        );
    }

    #[test]
    fn observability_invalid_protocol_is_rejected() {
        let mut cfg = Config::default();
        cfg.observability.otlp_protocol = "tcp".to_owned();
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "observability.otlp_protocol"),
            "expected observability.otlp_protocol error, got: {errs:?}"
        );
    }

    #[test]
    fn observability_invalid_endpoint_url_is_rejected() {
        let mut cfg = Config::default();
        cfg.observability.otlp_endpoint = Some("localhost:4317".to_owned()); // missing scheme
        let errs = validate(&cfg).unwrap_err();
        assert!(
            errs.iter().any(|e| e.field == "observability.otlp_endpoint"),
            "expected observability.otlp_endpoint error, got: {errs:?}"
        );
    }

    #[test]
    fn observability_valid_http_endpoint_is_accepted() {
        let mut cfg = Config::default();
        cfg.observability.otlp_endpoint = Some("http://localhost:4317".to_owned());
        validate(&cfg).expect("valid http endpoint should pass");
    }

    #[test]
    fn observability_valid_https_endpoint_is_accepted() {
        let mut cfg = Config::default();
        cfg.observability.otlp_endpoint = Some("https://otel.example.com:4317".to_owned());
        validate(&cfg).expect("valid https endpoint should pass");
    }

    #[test]
    fn observability_http_protobuf_protocol_is_valid() {
        let mut cfg = Config::default();
        cfg.observability.otlp_protocol = "http/protobuf".to_owned();
        validate(&cfg).expect("http/protobuf protocol should be valid");
    }

    // -----------------------------------------------------------------------
    // Used struct imports compile check
    // -----------------------------------------------------------------------

    #[test]
    fn new_config_types_are_importable_via_validate_module() {
        // This test just ensures all PRD-14 types are usable from this module.
        let _s = ServerConfig::default();
        let _a = AuthConfig::default();
        let _sec = SecurityConfig::default();
        let _sk = SkillsConfig::default();
        let _h = HarnessConfig::default();
        let _art = ArtifactConfig::default();
        let _obs = ObservabilityConfig::default();
        let _rl = RateLimitConfig::default();
        let _tls = TlsConfig::default();
    }
}

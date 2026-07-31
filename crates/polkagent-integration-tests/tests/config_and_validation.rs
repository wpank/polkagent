//! Cross-crate integration tests for configuration loading and validation.
//!
//! These tests exercise the `polkagent-config` crate's TOML parsing, schema
//! validation, environment variable overrides, and multi-provider config from
//! outside the crate boundary.

use std::sync::Mutex;

use polkagent_config::{
    validate, BudgetConfig, Config, ConfigLoader, DatabaseBackend, LogFormat,
    ProviderConfig, TuiTheme, CURRENT_SCHEMA_VERSION, DEFAULT_CONFIG_TEMPLATE,
};

/// Global mutex for tests that modify process-level environment variables.
/// Rust tests run in parallel within a single process, so env var mutations
/// must be serialised.
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Temporarily set an env var, run `f`, then restore the previous value.
fn with_env<F: FnOnce()>(key: &str, value: &str, f: F) {
    let _guard = ENV_LOCK.lock().expect("env lock");
    let previous = std::env::var(key).ok();
    std::env::set_var(key, value);
    f();
    match previous {
        Some(prev) => std::env::set_var(key, prev),
        None => std::env::remove_var(key),
    }
}

// ---------------------------------------------------------------------------
// Load config from TOML string
// ---------------------------------------------------------------------------

#[test]
fn load_config_from_toml_string() {
    // Hold env lock to prevent parallel env-var tests from leaking overrides
    let _guard = ENV_LOCK.lock().expect("env lock");

    let toml_str = r#"
[log]
level = "debug"
format = "json"

[execution]
max_concurrent_runs = 42
default_timeout_secs = 300

[execution.budget]
max_usd_per_run = 10.00
max_usd_per_day = 100.00
warn_threshold_percent = 90
"#;

    // Use a temp file with explicit path to avoid picking up host config files
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.toml");
    std::fs::write(&path, toml_str).expect("write");

    let cfg = ConfigLoader::new()
        .with_path(&path)
        .load()
        .expect("loading from TOML string should succeed");

    assert_eq!(cfg.log.level, "debug");
    assert_eq!(cfg.log.format, LogFormat::Json);
    assert_eq!(cfg.execution.max_concurrent_runs, 42);
    assert_eq!(cfg.execution.default_timeout_secs, 300);
    assert!((cfg.execution.budget.max_usd_per_run - 10.0).abs() < f64::EPSILON);
    assert!((cfg.execution.budget.max_usd_per_day - 100.0).abs() < f64::EPSILON);
    assert_eq!(cfg.execution.budget.warn_threshold_percent, 90);
}

#[test]
fn load_minimal_toml_string_uses_defaults() {
    // Hold env lock to prevent parallel env-var tests from leaking overrides
    let _guard = ENV_LOCK.lock().expect("env lock");

    // Use an empty temp file with explicit path to avoid host config pollution
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("empty.toml");
    std::fs::write(&path, "").expect("write");

    let cfg = ConfigLoader::new()
        .with_path(&path)
        .load()
        .expect("empty TOML should produce defaults");

    assert_eq!(cfg.meta.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(cfg.log.level, "info");
    assert_eq!(cfg.database.backend, DatabaseBackend::Sqlite);
}

#[test]
fn load_from_default_config_template() {
    let cfg: Config =
        toml::from_str(DEFAULT_CONFIG_TEMPLATE).expect("default template must be valid TOML");

    assert_eq!(cfg.meta.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(cfg.meta.api_version, "polkagent.dev/v1alpha1");
    assert_eq!(cfg.providers.len(), 1);
    assert_eq!(cfg.providers[0].id, "anthropic-default");
    assert_eq!(cfg.providers[0].provider_type, "anthropic");
}

// ---------------------------------------------------------------------------
// Validate all required fields
// ---------------------------------------------------------------------------

#[test]
fn default_config_passes_validation() {
    let cfg = Config::default();
    validate::validate(&cfg).expect("default config must be valid");
}

#[test]
fn validation_catches_invalid_log_level() {
    let mut cfg = Config::default();
    cfg.log.level = "verbose".into();
    let errs = validate::validate(&cfg).unwrap_err();
    assert!(
        errs.iter().any(|e| e.field == "log.level"),
        "should flag invalid log level"
    );
}

#[test]
fn validation_catches_invalid_bind_address() {
    let mut cfg = Config::default();
    cfg.api.bind_address = "not-a-socket-address".into();
    let errs = validate::validate(&cfg).unwrap_err();
    assert!(
        errs.iter().any(|e| e.field == "api.bind_address"),
        "should flag invalid bind address"
    );
}

#[test]
fn validation_catches_zero_concurrent_runs() {
    let mut cfg = Config::default();
    cfg.execution.max_concurrent_runs = 0;
    let errs = validate::validate(&cfg).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.field == "execution.max_concurrent_runs"),
        "should flag zero concurrent runs"
    );
}

#[test]
fn validation_catches_zero_timeout() {
    let mut cfg = Config::default();
    cfg.execution.default_timeout_secs = 0;
    let errs = validate::validate(&cfg).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.field == "execution.default_timeout_secs"),
        "should flag zero timeout"
    );
}

#[test]
fn validation_catches_unknown_schema_version() {
    let mut cfg = Config::default();
    cfg.meta.schema_version = 999;
    let errs = validate::validate(&cfg).unwrap_err();
    assert!(
        errs.iter().any(|e| e.field == "meta.schema_version"),
        "should flag unknown schema version"
    );
}

#[test]
fn validation_collects_all_errors_at_once() {
    let mut cfg = Config::default();
    cfg.log.level = "invalid".into();
    cfg.api.bind_address = "bad".into();
    cfg.execution.max_concurrent_runs = 0;
    cfg.execution.default_timeout_secs = 0;

    let errs = validate::validate(&cfg).unwrap_err();
    assert!(
        errs.len() >= 4,
        "should collect at least 4 errors, got {}",
        errs.len()
    );
}

// ---------------------------------------------------------------------------
// Environment variable overrides work
// ---------------------------------------------------------------------------

#[test]
fn env_var_overrides_log_level() {
    with_env("POLKAGENT_LOG_LEVEL", "trace", || {
        let cfg = ConfigLoader::new()
            .with_extra_toml("[log]\nlevel = \"info\"")
            .load()
            .expect("load");
        assert_eq!(
            cfg.log.level, "trace",
            "env var should override TOML value"
        );
    });
}

#[test]
fn env_var_overrides_api_bind_address() {
    with_env("POLKAGENT_API_BIND", "0.0.0.0:9999", || {
        let cfg = ConfigLoader::new()
            .with_extra_toml("")
            .load()
            .expect("load");
        assert_eq!(cfg.api.bind_address, "0.0.0.0:9999");
    });
}

#[test]
fn env_var_overrides_database_backend() {
    with_env("POLKAGENT_DATABASE_BACKEND", "postgres", || {
        let cfg = ConfigLoader::new()
            .with_extra_toml("")
            .load()
            .expect("load");
        assert_eq!(cfg.database.backend, DatabaseBackend::Postgres);
    });
}

#[test]
fn env_var_overrides_tui_theme() {
    with_env("POLKAGENT_TUI_THEME", "high_contrast", || {
        let cfg = ConfigLoader::new()
            .with_extra_toml("")
            .load()
            .expect("load");
        assert_eq!(cfg.tui.theme, TuiTheme::HighContrast);
    });
}

#[test]
fn env_var_overrides_memory_enabled() {
    with_env("POLKAGENT_MEMORY_ENABLED", "true", || {
        let cfg = ConfigLoader::new()
            .with_extra_toml("")
            .load()
            .expect("load");
        assert!(cfg.memory.enabled);
    });
}

// ---------------------------------------------------------------------------
// Provider config with multiple providers
// ---------------------------------------------------------------------------

#[test]
fn config_with_multiple_providers() {
    let toml_str = r#"
[[providers]]
id = "anthropic-main"
provider_type = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com"
default_model = "claude-sonnet-4-6"
timeout_secs = 120
max_retries = 3

[[providers]]
id = "openai-compat"
provider_type = "openai_compatible"
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com/v1"
default_model = "gpt-4o"
timeout_secs = 60
max_retries = 2

[[providers]]
id = "local-llm"
provider_type = "local"
api_key_env = "LOCAL_KEY"
base_url = "http://localhost:8000"
default_model = "llama-3"
timeout_secs = 30
max_retries = 1
"#;

    let cfg = ConfigLoader::new()
        .with_extra_toml(toml_str)
        .load()
        .expect("load");

    assert_eq!(cfg.providers.len(), 3);
    assert_eq!(cfg.providers[0].id, "anthropic-main");
    assert_eq!(cfg.providers[0].provider_type, "anthropic");
    assert_eq!(cfg.providers[1].id, "openai-compat");
    assert_eq!(cfg.providers[1].timeout_secs, 60);
    assert_eq!(cfg.providers[2].id, "local-llm");
    assert_eq!(cfg.providers[2].max_retries, 1);
}

#[test]
fn validation_rejects_duplicate_provider_ids() {
    let mut cfg = Config::default();
    let provider = ProviderConfig {
        id: "duplicate".into(),
        provider_type: "anthropic".into(),
        api_key_env: "MY_KEY".into(),
        timeout_secs: 30,
        ..Default::default()
    };
    cfg.providers = vec![provider.clone(), provider];

    let errs = validate::validate(&cfg).unwrap_err();
    assert!(
        errs.iter().any(|e| e.message.contains("duplicate")),
        "should flag duplicate provider IDs"
    );
}

#[test]
fn validation_rejects_provider_with_empty_id() {
    let mut cfg = Config::default();
    cfg.providers = vec![ProviderConfig {
        id: String::new(),
        provider_type: "anthropic".into(),
        api_key_env: "KEY".into(),
        timeout_secs: 30,
        ..Default::default()
    }];

    let errs = validate::validate(&cfg).unwrap_err();
    assert!(
        errs.iter().any(|e| e.field.ends_with(".id")),
        "should flag empty provider id"
    );
}

#[test]
fn validation_rejects_provider_with_zero_timeout() {
    let mut cfg = Config::default();
    cfg.providers = vec![ProviderConfig {
        id: "p1".into(),
        provider_type: "anthropic".into(),
        api_key_env: "KEY".into(),
        timeout_secs: 0,
        ..Default::default()
    }];

    let errs = validate::validate(&cfg).unwrap_err();
    assert!(
        errs.iter().any(|e| e.field.ends_with("timeout_secs")),
        "should flag zero timeout"
    );
}

// ---------------------------------------------------------------------------
// Budget config validation (non-negative values)
// ---------------------------------------------------------------------------

#[test]
fn budget_config_defaults_are_valid() {
    let budget = BudgetConfig::default();
    assert!(budget.max_usd_per_run >= 0.0);
    assert!(budget.max_usd_per_day >= 0.0);
    assert!(budget.warn_threshold_percent <= 100);
}

#[test]
fn validation_rejects_negative_budget_per_run() {
    let mut cfg = Config::default();
    cfg.execution.budget.max_usd_per_run = -1.0;

    let errs = validate::validate(&cfg).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.field == "execution.budget.max_usd_per_run"),
        "should flag negative max_usd_per_run"
    );
}

#[test]
fn validation_rejects_negative_budget_per_day() {
    let mut cfg = Config::default();
    cfg.execution.budget.max_usd_per_day = -0.01;

    let errs = validate::validate(&cfg).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.field == "execution.budget.max_usd_per_day"),
        "should flag negative max_usd_per_day"
    );
}

#[test]
fn validation_rejects_warn_threshold_over_100() {
    let mut cfg = Config::default();
    cfg.execution.budget.warn_threshold_percent = 101;

    let errs = validate::validate(&cfg).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.field == "execution.budget.warn_threshold_percent"),
        "should flag threshold over 100"
    );
}

#[test]
fn validation_accepts_zero_budget_values() {
    let mut cfg = Config::default();
    cfg.execution.budget.max_usd_per_run = 0.0;
    cfg.execution.budget.max_usd_per_day = 0.0;
    cfg.execution.budget.warn_threshold_percent = 0;

    validate::validate(&cfg).expect("zero budget values should be valid");
}

// ---------------------------------------------------------------------------
// Config TOML round-trip
// ---------------------------------------------------------------------------

#[test]
fn config_toml_round_trip_preserves_all_fields() {
    let original = Config::default();
    let serialized = toml::to_string_pretty(&original).expect("serialize");
    let deserialized: Config = toml::from_str(&serialized).expect("deserialize");
    assert_eq!(original, deserialized);
}

// ---------------------------------------------------------------------------
// Config merge semantics
// ---------------------------------------------------------------------------

#[test]
fn config_merge_overlay_wins_on_conflict() {
    let base = Config::default();
    let mut overlay = Config::default();
    overlay.log.level = "error".into();
    overlay.database.backend = DatabaseBackend::Postgres;
    overlay.execution.max_concurrent_runs = 99;

    let merged =
        polkagent_config::merge(base, overlay).expect("merge");
    assert_eq!(merged.log.level, "error");
    assert_eq!(merged.database.backend, DatabaseBackend::Postgres);
    assert_eq!(merged.execution.max_concurrent_runs, 99);
}

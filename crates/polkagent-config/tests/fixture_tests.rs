//! Integration tests that load configuration fixture files from the workspace
//! `fixtures/configs/` directory, verify parsing, validation, and round-trip
//! serialization.

#![allow(
    clippy::expect_used,
    reason = "fixture integration tests fail immediately when required workspace fixtures or expected validation outcomes are absent"
)]

use std::path::PathBuf;

use polkagent_config::schema::{Config, DatabaseBackend, LogFormat, TuiTheme};
use polkagent_config::validate;

/// Return the absolute path to a fixture file under `fixtures/configs/`.
fn fixture_path(name: &str) -> PathBuf {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate is inside crates/")
        .parent()
        .expect("crates/ is inside workspace root")
        .to_path_buf();
    workspace_root.join("fixtures").join("configs").join(name)
}

/// Read and parse a fixture TOML file into a `Config`.
fn load_fixture(name: &str) -> Config {
    let path = fixture_path(name);
    let content =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    toml::from_str(&content).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// minimal.toml
// ---------------------------------------------------------------------------

#[test]
fn minimal_config_parses_successfully() {
    let cfg = load_fixture("minimal.toml");

    // Fields explicitly set in the fixture.
    assert_eq!(cfg.meta.api_version, "v1alpha1");
    assert_eq!(cfg.database.backend, DatabaseBackend::Sqlite);
    assert_eq!(
        cfg.database.sqlite.path,
        "~/.local/share/polkagent/polkagent.db"
    );

    // Fields not set in the fixture should have their defaults.
    assert_eq!(cfg.meta.schema_version, 1);
    assert_eq!(cfg.log.level, "info");
    assert_eq!(cfg.log.format, LogFormat::Pretty);
    assert_eq!(cfg.execution.max_concurrent_runs, 10);
    assert_eq!(cfg.execution.default_timeout_secs, 600);
    assert!(cfg.providers.is_empty());
    assert!(cfg.api.enabled);
    assert_eq!(cfg.tui.theme, TuiTheme::Dark);
}

#[test]
fn minimal_config_passes_validation() {
    let cfg = load_fixture("minimal.toml");
    validate::validate(&cfg).expect("minimal.toml should be valid");
}

// ---------------------------------------------------------------------------
// full.toml
// ---------------------------------------------------------------------------

#[test]
fn full_config_parses_with_all_fields() {
    let cfg = load_fixture("full.toml");

    // Meta
    assert_eq!(cfg.meta.api_version, "v1alpha1");
    assert_eq!(cfg.meta.schema_version, 1);

    // Log
    assert_eq!(cfg.log.level, "info");
    assert_eq!(cfg.log.format, LogFormat::Json);

    // Database
    assert_eq!(cfg.database.backend, DatabaseBackend::Sqlite);
    assert_eq!(
        cfg.database.sqlite.path,
        "~/.local/share/polkagent/polkagent.db"
    );
    assert!(cfg.database.sqlite.wal_mode);
    assert_eq!(cfg.database.sqlite.busy_timeout_ms, 5000);

    // Execution
    assert_eq!(cfg.execution.max_concurrent_runs, 4);
    assert_eq!(cfg.execution.default_timeout_secs, 300);
    assert!((cfg.execution.budget.max_usd_per_run - 1.0).abs() < f64::EPSILON);
    assert!((cfg.execution.budget.max_usd_per_day - 10.0).abs() < f64::EPSILON);
    assert_eq!(cfg.execution.budget.warn_threshold_percent, 80);

    // Providers
    assert_eq!(cfg.providers.len(), 3);

    let anthropic = &cfg.providers[0];
    assert_eq!(anthropic.id, "anthropic");
    assert_eq!(anthropic.provider_type, "anthropic");
    assert_eq!(anthropic.api_key_env, "POLKAGENT_ANTHROPIC_API_KEY");
    assert_eq!(anthropic.default_model, "claude-sonnet-4-20250514");
    assert_eq!(anthropic.timeout_secs, 120);
    assert_eq!(anthropic.max_retries, 3);

    let openai = &cfg.providers[1];
    assert_eq!(openai.id, "openai");
    assert_eq!(openai.provider_type, "openai");
    assert_eq!(openai.api_key_env, "POLKAGENT_OPENAI_API_KEY");
    assert_eq!(openai.default_model, "gpt-4o");
    // Default timeout and retries.
    assert_eq!(openai.timeout_secs, 120);
    assert_eq!(openai.max_retries, 3);

    let local = &cfg.providers[2];
    assert_eq!(local.id, "local");
    assert_eq!(local.provider_type, "openai");
    assert_eq!(local.base_url, "http://localhost:11434/v1");
    assert_eq!(local.default_model, "llama3.2");

    // Policy
    assert_eq!(cfg.policy.policy_dir, "~/.polkagent/policies");
    assert_eq!(cfg.policy.default_policy, "read-only");

    // API
    assert!(cfg.api.enabled);
    assert_eq!(cfg.api.bind_address, "127.0.0.1:9090");
    assert_eq!(cfg.api.cors_origins, vec!["http://localhost:3000"]);

    // TUI
    assert_eq!(cfg.tui.theme, TuiTheme::Dark);
    assert!(cfg.tui.atmospheric_effects);
}

#[test]
fn full_config_validation_reports_missing_api_key_env() {
    let cfg = load_fixture("full.toml");
    // The "local" provider has no api_key_env, which the validator flags.
    let result = validate::validate(&cfg);
    match result {
        Err(errors) => {
            let has_api_key_error = errors
                .iter()
                .any(|e| e.field.contains("api_key_env") && e.field.contains("providers[2]"));
            assert!(
                has_api_key_error,
                "expected api_key_env error for local provider, got: {errors:?}"
            );
        }
        Ok(()) => panic!("expected validation errors for local provider missing api_key_env"),
    }
}

// ---------------------------------------------------------------------------
// invalid.toml
// ---------------------------------------------------------------------------

#[test]
fn invalid_config_parses_but_fails_validation() {
    let cfg = load_fixture("invalid.toml");

    // The file should parse without errors (TOML syntax is valid).
    // But validation should fail with specific errors.
    let errors = validate::validate(&cfg).expect_err("invalid.toml should fail validation");

    // Collect the field names that had errors.
    let error_fields: Vec<&str> = errors.iter().map(|e| e.field.as_str()).collect();

    // log.level = "invalid_level" should be rejected.
    assert!(
        error_fields.contains(&"log.level"),
        "expected log.level error, got: {error_fields:?}"
    );

    // execution.max_concurrent_runs = 0 should be rejected.
    assert!(
        error_fields.contains(&"execution.max_concurrent_runs"),
        "expected max_concurrent_runs error, got: {error_fields:?}"
    );

    // execution.budget.warn_threshold_percent = 200 should be rejected (> 100).
    assert!(
        error_fields.contains(&"execution.budget.warn_threshold_percent"),
        "expected warn_threshold_percent error, got: {error_fields:?}"
    );

    // api.bind_address = "not-a-valid-address" should be rejected.
    assert!(
        error_fields.contains(&"api.bind_address"),
        "expected api.bind_address error, got: {error_fields:?}"
    );
}

#[test]
fn invalid_config_reports_at_least_four_errors() {
    let cfg = load_fixture("invalid.toml");
    let errors = validate::validate(&cfg).expect_err("invalid.toml should fail validation");
    assert!(
        errors.len() >= 4,
        "expected at least 4 validation errors, got {}: {errors:?}",
        errors.len()
    );
}

// ---------------------------------------------------------------------------
// Round-trip serialization
// ---------------------------------------------------------------------------

#[test]
fn minimal_config_round_trips_through_toml() {
    let original = load_fixture("minimal.toml");
    let serialized = toml::to_string_pretty(&original).expect("serialize to TOML");
    let deserialized: Config = toml::from_str(&serialized).expect("deserialize from TOML");
    assert_eq!(original, deserialized);
}

#[test]
fn full_config_round_trips_through_toml() {
    let original = load_fixture("full.toml");
    let serialized = toml::to_string_pretty(&original).expect("serialize to TOML");
    let deserialized: Config = toml::from_str(&serialized).expect("deserialize from TOML");
    assert_eq!(original, deserialized);
}

#[test]
fn invalid_config_round_trips_through_toml() {
    // Even invalid configs should round-trip through serialization; validation
    // is a separate concern from (de)serialization.
    let original = load_fixture("invalid.toml");
    let serialized = toml::to_string_pretty(&original).expect("serialize to TOML");
    let deserialized: Config = toml::from_str(&serialized).expect("deserialize from TOML");
    assert_eq!(original, deserialized);
}

// ---------------------------------------------------------------------------
// ConfigLoader integration
// ---------------------------------------------------------------------------

#[test]
fn config_loader_loads_minimal_fixture() {
    let cfg = polkagent_config::ConfigLoader::new()
        .with_path(fixture_path("minimal.toml"))
        .load()
        .expect("ConfigLoader should load minimal.toml");
    assert_eq!(cfg.meta.api_version, "v1alpha1");
}

#[test]
fn config_loader_loads_full_fixture() {
    let cfg = polkagent_config::ConfigLoader::new()
        .with_path(fixture_path("full.toml"))
        .load()
        .expect("ConfigLoader should load full.toml");
    assert_eq!(cfg.providers.len(), 3);
    assert_eq!(cfg.api.bind_address, "127.0.0.1:9090");
}

// ---------------------------------------------------------------------------
// JSON round-trip (for completeness)
// ---------------------------------------------------------------------------

#[test]
fn minimal_config_round_trips_through_json() {
    let original = load_fixture("minimal.toml");
    let json = serde_json::to_string_pretty(&original).expect("serialize to JSON");
    let deserialized: Config = serde_json::from_str(&json).expect("deserialize from JSON");
    assert_eq!(original, deserialized);
}

#[test]
fn full_config_round_trips_through_json() {
    let original = load_fixture("full.toml");
    let json = serde_json::to_string_pretty(&original).expect("serialize to JSON");
    let deserialized: Config = serde_json::from_str(&json).expect("deserialize from JSON");
    assert_eq!(original, deserialized);
}

//! Config -> execution integration test.
//!
//! Loads a config file from a temp directory, validates it, and uses config
//! values to verify execution parameters are correctly derived.

// This assertion-oriented integration target uses `expect`/`unwrap` to identify
// the exact cross-crate fixture step or behavioral contract that failed.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;

use tempfile::TempDir;

use polkagent_config::{validate, Config, ConfigLoader};

// ---------------------------------------------------------------------------
// Load a config file from a temp directory and validate it
// ---------------------------------------------------------------------------

#[test]
fn load_config_from_temp_dir_and_validate() {
    let tmp = TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("polkagent.toml");

    fs::write(
        &config_path,
        r#"
[log]
level = "debug"
format = "json"

[execution]
max_concurrent_runs = 5
default_timeout_secs = 120

[execution.budget]
max_usd_per_run = 2.0
max_usd_per_day = 50.0
warn_threshold_percent = 80
"#,
    )
    .expect("write config");

    let cfg = ConfigLoader::new()
        .with_path(&config_path)
        .load()
        .expect("load config");

    // Validate the loaded config.
    validate::validate(&cfg).expect("config should be valid");

    // Verify loaded values.
    assert_eq!(cfg.log.level, "debug");
    assert_eq!(cfg.execution.max_concurrent_runs, 5);
    assert_eq!(cfg.execution.default_timeout_secs, 120);
    assert!((cfg.execution.budget.max_usd_per_run - 2.0).abs() < f64::EPSILON);
    assert!((cfg.execution.budget.max_usd_per_day - 50.0).abs() < f64::EPSILON);
    assert_eq!(cfg.execution.budget.warn_threshold_percent, 80);
}

// ---------------------------------------------------------------------------
// Invalid config is rejected by validation
// ---------------------------------------------------------------------------

#[test]
fn invalid_config_detected_by_validation() {
    let tmp = TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("polkagent.toml");

    fs::write(
        &config_path,
        r#"
[log]
level = "verbose"

[execution]
max_concurrent_runs = 0
default_timeout_secs = 0

[execution.budget]
warn_threshold_percent = 150
"#,
    )
    .expect("write config");

    let cfg = ConfigLoader::new()
        .with_path(&config_path)
        .load()
        .expect("load config");

    let errors = validate::validate(&cfg).expect_err("should fail validation");

    // We expect at least 4 errors: log.level, max_concurrent_runs,
    // default_timeout_secs, warn_threshold_percent.
    assert!(
        errors.len() >= 4,
        "expected at least 4 validation errors, got {}: {:?}",
        errors.len(),
        errors
    );

    let fields: Vec<&str> = errors.iter().map(|e| e.field.as_str()).collect();
    assert!(fields.contains(&"log.level"), "missing log.level error");
    assert!(
        fields.contains(&"execution.max_concurrent_runs"),
        "missing max_concurrent_runs error"
    );
    assert!(
        fields.contains(&"execution.default_timeout_secs"),
        "missing default_timeout_secs error"
    );
    assert!(
        fields.contains(&"execution.budget.warn_threshold_percent"),
        "missing warn_threshold_percent error"
    );
}

// ---------------------------------------------------------------------------
// Default config passes validation
// ---------------------------------------------------------------------------

#[test]
fn default_config_passes_validation() {
    let cfg = Config::default();
    validate::validate(&cfg).expect("default config must be valid");
}

// ---------------------------------------------------------------------------
// Config values drive execution parameters
// ---------------------------------------------------------------------------

#[test]
fn config_values_set_execution_parameters() {
    let tmp = TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("polkagent.toml");

    fs::write(
        &config_path,
        r"
[execution]
max_concurrent_runs = 20
default_timeout_secs = 300

[execution.budget]
max_usd_per_run = 10.0
max_usd_per_day = 100.0
",
    )
    .expect("write config");

    let cfg = ConfigLoader::new()
        .with_path(&config_path)
        .load()
        .expect("load config");

    validate::validate(&cfg).expect("config should be valid");

    // Derive execution parameters from config.
    let max_runs = cfg.execution.max_concurrent_runs;
    let timeout = std::time::Duration::from_secs(cfg.execution.default_timeout_secs);
    let budget_per_run = cfg.execution.budget.max_usd_per_run;

    assert_eq!(max_runs, 20);
    assert_eq!(timeout.as_secs(), 300);
    assert!((budget_per_run - 10.0).abs() < f64::EPSILON);
}

// ---------------------------------------------------------------------------
// Extra TOML overlay merges with file config
// ---------------------------------------------------------------------------

#[test]
fn extra_toml_overlay_merges_with_file() {
    let tmp = TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("polkagent.toml");

    fs::write(
        &config_path,
        r#"
[log]
level = "info"

[execution]
max_concurrent_runs = 5
"#,
    )
    .expect("write config");

    // Apply an extra overlay that overrides log level but not max_concurrent_runs.
    let cfg = ConfigLoader::new()
        .with_path(&config_path)
        .with_extra_toml("[log]\nlevel = \"warn\"")
        .load()
        .expect("load");

    assert_eq!(cfg.log.level, "warn", "overlay should override log level");
    assert_eq!(
        cfg.execution.max_concurrent_runs, 5,
        "max_concurrent_runs should come from the file"
    );
}

// ---------------------------------------------------------------------------
// Missing config file returns an error
// ---------------------------------------------------------------------------

#[test]
fn missing_config_file_returns_error() {
    let result = ConfigLoader::new()
        .with_path("/nonexistent/path/polkagent.toml")
        .load();
    assert!(
        result.is_err(),
        "loading a nonexistent config file should fail"
    );
}

// ---------------------------------------------------------------------------
// Provider config validation
// ---------------------------------------------------------------------------

#[test]
fn provider_with_missing_api_key_env_is_flagged() {
    let tmp = TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("polkagent.toml");

    fs::write(
        &config_path,
        r#"
[[providers]]
id = "anthropic"
provider_type = "anthropic"
api_key_env = ""
timeout_secs = 30
"#,
    )
    .expect("write config");

    let cfg = ConfigLoader::new()
        .with_path(&config_path)
        .load()
        .expect("load config");

    let errors = validate::validate(&cfg).expect_err("should fail validation");
    let fields: Vec<&str> = errors.iter().map(|e| e.field.as_str()).collect();
    assert!(
        fields.iter().any(|f| f.ends_with("api_key_env")),
        "expected api_key_env error, got fields: {fields:?}"
    );
}

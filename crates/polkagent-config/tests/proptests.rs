//! Property-based tests for `polkagent-config`.
//!
//! These tests use `proptest` to verify invariants of the configuration
//! schema, TOML round-trip serialization, and validation logic.

use proptest::prelude::*;

use polkagent_config::schema::Config;
use polkagent_config::validate;

// =========================================================================
// Helpers
// =========================================================================

/// The five valid log level strings (case-insensitive in validation).
const VALID_LOG_LEVELS: &[&str] = &["trace", "debug", "info", "warn", "error"];

/// Strategy for a valid log level string (mixed case).
fn arb_valid_log_level() -> impl Strategy<Value = String> {
    prop::sample::select(VALID_LOG_LEVELS)
        .prop_flat_map(|level| {
            // Randomly capitalize characters to test case-insensitivity.
            Just(level.to_string()).prop_map(|s| {
                s.chars()
                    .enumerate()
                    .map(|(i, c)| if i % 2 == 0 { c.to_uppercase().next().unwrap_or(c) } else { c })
                    .collect::<String>()
            })
        })
}

/// Strategy for an invalid log level string.
fn arb_invalid_log_level() -> impl Strategy<Value = String> {
    "[a-z]{3,12}".prop_filter("must not be a valid level", |s| {
        !VALID_LOG_LEVELS.contains(&s.to_lowercase().as_str())
    })
}

// =========================================================================
// 1. Default config always validates
// =========================================================================

proptest! {
    /// The default `Config` always passes validation, regardless of how many
    /// times we construct it (exercised to ensure no hidden randomness).
    #[test]
    fn default_config_always_validates(_ in 0..10u8) {
        let cfg = Config::default();
        let result = validate::validate(&cfg);
        prop_assert!(
            result.is_ok(),
            "default config must always validate, got: {:?}",
            result
        );
    }
}

// =========================================================================
// 2. Config round-trips through TOML serialize/deserialize
// =========================================================================

proptest! {
    /// A `Config` with arbitrary valid budget values round-trips through TOML
    /// without changing.
    #[test]
    fn config_toml_round_trip(
        max_usd_per_run in 0.0f64..100_000.0,
        max_usd_per_day in 0.0f64..100_000.0,
        warn_pct in 0u8..=100,
        max_concurrent in 1u32..1000,
        timeout_secs in 1u64..100_000,
    ) {
        let mut cfg = Config::default();
        cfg.execution.budget.max_usd_per_run = max_usd_per_run;
        cfg.execution.budget.max_usd_per_day = max_usd_per_day;
        cfg.execution.budget.warn_threshold_percent = warn_pct;
        cfg.execution.max_concurrent_runs = max_concurrent;
        cfg.execution.default_timeout_secs = timeout_secs;

        let serialized = toml::to_string_pretty(&cfg).expect("serialize to TOML");
        let deserialized: Config = toml::from_str(&serialized).expect("deserialize from TOML");

        prop_assert_eq!(cfg.execution.max_concurrent_runs, deserialized.execution.max_concurrent_runs);
        prop_assert_eq!(cfg.execution.default_timeout_secs, deserialized.execution.default_timeout_secs);
        prop_assert_eq!(cfg.execution.budget.warn_threshold_percent, deserialized.execution.budget.warn_threshold_percent);
        // Float comparison: check they are close enough (TOML round-trip may
        // introduce negligible precision differences).
        prop_assert!(
            (cfg.execution.budget.max_usd_per_run - deserialized.execution.budget.max_usd_per_run).abs() < 1e-6,
            "max_usd_per_run mismatch"
        );
        prop_assert!(
            (cfg.execution.budget.max_usd_per_day - deserialized.execution.budget.max_usd_per_day).abs() < 1e-6,
            "max_usd_per_day mismatch"
        );
    }
}

// =========================================================================
// 3. Validation collects all errors (not just first)
// =========================================================================

proptest! {
    /// When multiple fields are invalid, validation returns errors for all of
    /// them rather than short-circuiting after the first.
    #[test]
    fn validation_collects_all_errors(
        bad_level in arb_invalid_log_level(),
    ) {
        let mut cfg = Config::default();
        // Introduce three distinct errors.
        cfg.log.level = bad_level;
        cfg.api.bind_address = "not-an-address".to_owned();
        cfg.execution.max_concurrent_runs = 0;

        let errs = validate::validate(&cfg).unwrap_err();

        let fields: Vec<&str> = errs.iter().map(|e| e.field.as_str()).collect();

        prop_assert!(
            fields.contains(&"log.level"),
            "must report log.level error, got fields: {:?}",
            fields
        );
        prop_assert!(
            fields.contains(&"api.bind_address"),
            "must report api.bind_address error, got fields: {:?}",
            fields
        );
        prop_assert!(
            fields.contains(&"execution.max_concurrent_runs"),
            "must report max_concurrent_runs error, got fields: {:?}",
            fields
        );

        // At least 3 errors.
        prop_assert!(
            errs.len() >= 3,
            "expected at least 3 errors, got {} : {:?}",
            errs.len(),
            errs
        );
    }
}

// =========================================================================
// 4. Log level validation accepts only valid levels
// =========================================================================

proptest! {
    /// Valid log levels (in any case) produce no log.level validation error.
    #[test]
    fn valid_log_levels_accepted(level in arb_valid_log_level()) {
        let mut cfg = Config::default();
        cfg.log.level = level;
        let result = validate::validate(&cfg);
        // If there are errors, none should be about log.level.
        match result {
            Ok(()) => {} // fine
            Err(errs) => {
                prop_assert!(
                    !errs.iter().any(|e| e.field == "log.level"),
                    "valid log level must not produce a log.level error, got: {:?}",
                    errs
                );
            }
        }
    }

    /// Invalid log levels always produce a log.level validation error.
    #[test]
    fn invalid_log_levels_rejected(level in arb_invalid_log_level()) {
        let mut cfg = Config::default();
        cfg.log.level = level.clone();
        let errs = validate::validate(&cfg).unwrap_err();
        prop_assert!(
            errs.iter().any(|e| e.field == "log.level"),
            "invalid log level '{}' must be rejected, got: {:?}",
            level,
            errs
        );
    }
}

// =========================================================================
// 5. Budget values are non-negative after validation
// =========================================================================

proptest! {
    /// Non-negative budget values always pass validation (no budget errors).
    #[test]
    fn non_negative_budgets_accepted(
        per_run in 0.0f64..100_000.0,
        per_day in 0.0f64..100_000.0,
    ) {
        let mut cfg = Config::default();
        cfg.execution.budget.max_usd_per_run = per_run;
        cfg.execution.budget.max_usd_per_day = per_day;
        let result = validate::validate(&cfg);
        match result {
            Ok(()) => {} // fine
            Err(errs) => {
                prop_assert!(
                    !errs.iter().any(|e| e.field.contains("max_usd")),
                    "non-negative budgets must not produce budget errors, got: {:?}",
                    errs
                );
            }
        }
    }

    /// Negative budget values are rejected by validation.
    #[test]
    fn negative_budgets_rejected(
        per_run in -100_000.0f64..-0.01,
        per_day in -100_000.0f64..-0.01,
    ) {
        let mut cfg = Config::default();
        cfg.execution.budget.max_usd_per_run = per_run;
        cfg.execution.budget.max_usd_per_day = per_day;
        let errs = validate::validate(&cfg).unwrap_err();
        // At least one budget error must be present.
        prop_assert!(
            errs.iter().any(|e| e.field.contains("max_usd")),
            "negative budgets must produce validation errors, got: {:?}",
            errs
        );
    }
}

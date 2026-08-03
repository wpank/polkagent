//! `polkagent config` subcommand handlers.
//!
//! Implements four subcommands:
//!
//! - `show`     – display the merged, env-overridden config with secrets redacted.
//! - `validate` – validate the config against schema rules, report errors/warnings.
//! - `path`     – show the resolved config file paths (system, user, project).
//! - `get`      – retrieve a single value by dotted key path.

use anyhow::{anyhow, Result};
use serde::Serialize;

use crate::cli::{ConfigCmd, ConfigGetCmd, ConfigPathCmd, ConfigShowCmd, ConfigValidateCmd};

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch the config subcommand.
pub fn run(cmd: &ConfigCmd) -> Result<()> {
    match cmd {
        ConfigCmd::Show(c) => show(c),
        ConfigCmd::Validate(c) => validate(c),
        ConfigCmd::Path(c) => path(c),
        ConfigCmd::Get(c) => get(c),
    }
}

// ---------------------------------------------------------------------------
// show
// ---------------------------------------------------------------------------

/// Redacted placeholder used in place of secret values.
const REDACTED: &str = "***";

fn show(cmd: &ConfigShowCmd) -> Result<()> {
    let cfg = load_config(None)?;
    let redacted = redact_config(cfg);

    if cmd.json {
        println!("{}", serde_json::to_string_pretty(&redacted)?);
    } else if cmd.toml {
        println!("{}", toml::to_string_pretty(&redacted)?);
    } else {
        print_human(&redacted);
    }
    Ok(())
}

/// Pretty-print the most relevant fields in a human-readable format.
fn print_human(cfg: &polkagent_config::schema::Config) {
    println!("Configuration:");
    println!("  schema_version: {}", cfg.meta.schema_version);
    println!("  log.level:      {}", cfg.log.level);
    println!("  database:");
    println!("    backend: {:?}", cfg.database.backend);
    println!("    sqlite.path: {}", cfg.database.sqlite.path);
    println!("  execution:");
    println!(
        "    max_concurrent_runs:  {}",
        cfg.execution.max_concurrent_runs
    );
    println!(
        "    default_timeout_secs: {}",
        cfg.execution.default_timeout_secs
    );
    println!(
        "    budget.max_usd_per_run: ${:.2}",
        cfg.execution.budget.max_usd_per_run
    );
    println!("  api:");
    println!("    enabled:      {}", cfg.api.enabled);
    println!("    bind_address: {}", cfg.api.bind_address);
    println!("  tui:");
    println!("    theme:               {:?}", cfg.tui.theme);
    println!("    atmospheric_effects: {}", cfg.tui.atmospheric_effects);
    if !cfg.providers.is_empty() {
        println!("  providers:");
        for p in &cfg.providers {
            println!(
                "    - {} ({}) default_model={} api_key_env={}",
                p.id, p.provider_type, p.default_model, p.api_key_env
            );
        }
    }
}

/// Return a copy of the config with all secret-like values replaced by `"***"`.
///
/// Currently redacts:
/// - `providers[*].api_key_env` — we show the env-var name but redact any
///   actual key value that might have been inlined (this field holds a name,
///   not a value, but we keep it intact so the user can see which var to set).
///
/// We intentionally do NOT redact `api_key_env` itself (it is an env-var name,
/// not a secret).  If any future field holds a literal key it would be added
/// here.
pub fn redact_config(
    mut cfg: polkagent_config::schema::Config,
) -> polkagent_config::schema::Config {
    // auth.api_keys holds SHA-256 digests, not plaintext keys.  We still
    // redact them to avoid leaking digest information.
    for key in &mut cfg.auth.api_keys {
        *key = REDACTED.to_owned();
    }
    // jwt_secret_env is an env-var name; replace with redacted marker to
    // avoid accidentally revealing internal env-var naming conventions.
    if cfg.auth.jwt_secret_env.is_some() {
        cfg.auth.jwt_secret_env = Some(REDACTED.to_owned());
    }
    // Postgres URL can embed credentials.
    if !cfg.database.postgres.url.is_empty() {
        cfg.database.postgres.url = REDACTED.to_owned();
    }
    cfg
}

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

fn validate(cmd: &ConfigValidateCmd) -> Result<()> {
    let override_path = cmd
        .path
        .as_deref()
        .map(|p| p.to_str().unwrap_or("").to_owned());

    let result = load_config(override_path);
    let cfg = match result {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Configuration is INVALID (load error):");
            eprintln!("  {e}");
            std::process::exit(1);
        }
    };

    // Run structural validation.
    match polkagent_config::validate::validate(&cfg) {
        Ok(()) => {
            println!("Configuration is valid.");
            println!("  schema_version: {}", cfg.meta.schema_version);
            println!("  {} provider(s) configured", cfg.providers.len());
        }
        Err(errors) => {
            eprintln!("Configuration is INVALID:");
            for e in &errors {
                eprintln!("  error: {e}");
            }
            std::process::exit(1);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// path
// ---------------------------------------------------------------------------

/// Structured output for `config path --json`.
#[derive(Debug, Serialize)]
struct ConfigPaths {
    /// User-global config path (may not exist).
    user: Option<String>,
    /// Project-local config path (walks up from CWD; may not exist).
    project: Option<String>,
    /// Which of the above files actually exists and was loaded.
    resolved: Vec<String>,
}

fn path(cmd: &ConfigPathCmd) -> Result<()> {
    use polkagent_config::loader::{find_project_config, global_config_path};

    let user = global_config_path().map(|p| p.display().to_string());
    let project = find_project_config().map(|p| p.display().to_string());

    let mut resolved = Vec::new();
    if let Some(ref u) = user {
        if std::path::Path::new(u).exists() {
            resolved.push(u.clone());
        }
    }
    if let Some(ref p) = project {
        if std::path::Path::new(p).exists() {
            resolved.push(p.clone());
        }
    }

    if cmd.json {
        let out = ConfigPaths {
            user,
            project,
            resolved,
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Config file paths:");
        println!(
            "  user:    {}",
            user.as_deref().unwrap_or("<unknown>")
        );
        println!(
            "  project: {}",
            project.as_deref().unwrap_or("<not found>")
        );
        if resolved.is_empty() {
            println!("  (no config files found; using built-in defaults)");
        } else {
            println!("  loaded (in order of precedence, later wins):");
            for p in &resolved {
                println!("    - {p}");
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// get
// ---------------------------------------------------------------------------

fn get(cmd: &ConfigGetCmd) -> Result<()> {
    let cfg = load_config(None)?;

    // Serialize to a TOML Value so we can walk dotted keys generically.
    let value: toml::Value =
        toml::Value::try_from(&cfg).map_err(|e| anyhow!("serialize config: {e}"))?;

    let result = lookup_dotted(&value, &cmd.key)
        .ok_or_else(|| anyhow!("key '{}' not found in configuration", cmd.key))?;

    if cmd.json {
        // Convert TOML value → serde_json value for consistent JSON output.
        let json_val = toml_to_json(result);
        println!("{}", serde_json::to_string_pretty(&json_val)?);
    } else {
        println!("{}", toml_value_display(result));
    }

    Ok(())
}

/// Walk a TOML value using a dotted path such as `"log.level"`.
fn lookup_dotted<'a>(root: &'a toml::Value, path: &str) -> Option<&'a toml::Value> {
    let mut current = root;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

/// Human-readable display of a `toml::Value`.
fn toml_value_display(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(dt) => dt.to_string(),
        toml::Value::Array(_) | toml::Value::Table(_) => {
            toml::to_string_pretty(v).unwrap_or_else(|_| format!("{v:?}"))
        }
    }
}

/// Recursively convert a `toml::Value` to a `serde_json::Value`.
fn toml_to_json(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(toml_to_json).collect())
        }
        toml::Value::Table(tbl) => {
            let map = tbl
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
    }
}

// ---------------------------------------------------------------------------
// Loader helpers
// ---------------------------------------------------------------------------

/// Load configuration from the standard search paths with env overrides.
pub fn load_config(override_path: Option<String>) -> Result<polkagent_config::schema::Config> {
    let loader = if let Some(path) = override_path {
        polkagent_config::ConfigLoader::new().with_path(path)
    } else {
        polkagent_config::ConfigLoader::new()
    };
    loader.load().map_err(|e| anyhow!("{e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use polkagent_config::schema::Config;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn write_config(dir: &std::path::Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("polkagent.toml");
        fs::write(&path, content).expect("write config");
        path
    }

    // -----------------------------------------------------------------------
    // load_config
    // -----------------------------------------------------------------------

    #[test]
    fn load_config_defaults_when_no_file() {
        // No explicit path → falls back to built-in defaults (no file needed).
        let cfg = load_config(None).expect("load default config");
        assert_eq!(cfg.meta.schema_version, polkagent_config::CURRENT_SCHEMA_VERSION);
        assert_eq!(cfg.log.level, "info");
    }

    #[test]
    fn load_config_from_explicit_path() {
        let tmp = TempDir::new().expect("tempdir");
        let path = write_config(
            tmp.path(),
            "[log]\nlevel = \"debug\"\n",
        );
        let cfg = load_config(Some(path.to_str().unwrap().to_owned()))
            .expect("load config from file");
        assert_eq!(cfg.log.level, "debug");
        // Other fields should still be at defaults.
        assert_eq!(cfg.execution.max_concurrent_runs, 10);
    }

    #[test]
    fn load_config_error_on_missing_file() {
        let result = load_config(Some("/nonexistent/path.toml".to_owned()));
        assert!(result.is_err(), "should error for missing file");
    }

    #[test]
    fn load_config_error_on_bad_toml() {
        let tmp = TempDir::new().expect("tempdir");
        let path = write_config(tmp.path(), "this is not valid TOML !!!$$$");
        let result = load_config(Some(path.to_str().unwrap().to_owned()));
        assert!(result.is_err(), "should error for invalid TOML");
    }

    // -----------------------------------------------------------------------
    // redact_config
    // -----------------------------------------------------------------------

    #[test]
    fn redact_replaces_api_key_hashes() {
        let mut cfg = Config::default();
        cfg.auth.api_keys = vec!["sha256abc".to_owned(), "sha256def".to_owned()];
        let redacted = redact_config(cfg);
        assert!(
            redacted.auth.api_keys.iter().all(|k| k == REDACTED),
            "all api_keys should be redacted"
        );
    }

    #[test]
    fn redact_replaces_jwt_secret_env() {
        let mut cfg = Config::default();
        cfg.auth.jwt_secret_env = Some("MY_JWT_VAR".to_owned());
        let redacted = redact_config(cfg);
        assert_eq!(redacted.auth.jwt_secret_env, Some(REDACTED.to_owned()));
    }

    #[test]
    fn redact_leaves_jwt_secret_env_none_when_absent() {
        let cfg = Config::default();
        assert!(cfg.auth.jwt_secret_env.is_none());
        let redacted = redact_config(cfg);
        assert!(redacted.auth.jwt_secret_env.is_none());
    }

    #[test]
    fn redact_replaces_postgres_url() {
        let mut cfg = Config::default();
        cfg.database.postgres.url = "postgres://user:secret@host/db".to_owned();
        let redacted = redact_config(cfg);
        assert_eq!(
            redacted.database.postgres.url,
            REDACTED,
            "postgres URL should be redacted"
        );
    }

    #[test]
    fn redact_leaves_empty_postgres_url_empty() {
        let cfg = Config::default();
        assert!(cfg.database.postgres.url.is_empty());
        let redacted = redact_config(cfg);
        // Empty string stays empty (nothing to redact).
        assert!(redacted.database.postgres.url.is_empty());
    }

    #[test]
    fn redact_preserves_non_secret_fields() {
        let mut cfg = Config::default();
        cfg.log.level = "trace".to_owned();
        cfg.api.bind_address = "127.0.0.1:9999".to_owned();
        let redacted = redact_config(cfg);
        assert_eq!(redacted.log.level, "trace");
        assert_eq!(redacted.api.bind_address, "127.0.0.1:9999");
    }

    // -----------------------------------------------------------------------
    // lookup_dotted
    // -----------------------------------------------------------------------

    #[test]
    fn lookup_dotted_top_level_key() {
        let cfg = Config::default();
        let val: toml::Value = toml::Value::try_from(&cfg).expect("serialize");
        let result = lookup_dotted(&val, "log");
        assert!(result.is_some(), "top-level 'log' key should exist");
        assert!(result.unwrap().is_table());
    }

    #[test]
    fn lookup_dotted_nested_key() {
        let cfg = Config::default();
        let val: toml::Value = toml::Value::try_from(&cfg).expect("serialize");
        let level = lookup_dotted(&val, "log.level");
        assert!(level.is_some());
        assert_eq!(level.unwrap().as_str(), Some("info"));
    }

    #[test]
    fn lookup_dotted_deeply_nested_key() {
        let cfg = Config::default();
        let val: toml::Value = toml::Value::try_from(&cfg).expect("serialize");
        // execution.budget.max_usd_per_run
        let budget = lookup_dotted(&val, "execution.budget.max_usd_per_run");
        assert!(budget.is_some(), "deeply nested key should be found");
    }

    #[test]
    fn lookup_dotted_missing_key_returns_none() {
        let cfg = Config::default();
        let val: toml::Value = toml::Value::try_from(&cfg).expect("serialize");
        let result = lookup_dotted(&val, "nonexistent.key.path");
        assert!(result.is_none());
    }

    #[test]
    fn lookup_dotted_partial_path_returns_table() {
        let cfg = Config::default();
        let val: toml::Value = toml::Value::try_from(&cfg).expect("serialize");
        let result = lookup_dotted(&val, "execution.budget");
        assert!(result.is_some());
        assert!(result.unwrap().is_table());
    }

    // -----------------------------------------------------------------------
    // toml_value_display
    // -----------------------------------------------------------------------

    #[test]
    fn toml_value_display_string() {
        let v = toml::Value::String("hello".to_owned());
        assert_eq!(toml_value_display(&v), "hello");
    }

    #[test]
    fn toml_value_display_integer() {
        let v = toml::Value::Integer(42);
        assert_eq!(toml_value_display(&v), "42");
    }

    #[test]
    fn toml_value_display_boolean() {
        assert_eq!(toml_value_display(&toml::Value::Boolean(true)), "true");
        assert_eq!(toml_value_display(&toml::Value::Boolean(false)), "false");
    }

    // -----------------------------------------------------------------------
    // toml_to_json
    // -----------------------------------------------------------------------

    #[test]
    fn toml_to_json_string() {
        let v = toml::Value::String("world".to_owned());
        let j = toml_to_json(&v);
        assert_eq!(j, serde_json::Value::String("world".to_owned()));
    }

    #[test]
    fn toml_to_json_integer() {
        let v = toml::Value::Integer(7);
        let j = toml_to_json(&v);
        assert_eq!(j, serde_json::json!(7));
    }

    #[test]
    fn toml_to_json_bool() {
        let j = toml_to_json(&toml::Value::Boolean(true));
        assert_eq!(j, serde_json::Value::Bool(true));
    }

    #[test]
    fn toml_to_json_array() {
        let v = toml::Value::Array(vec![
            toml::Value::Integer(1),
            toml::Value::Integer(2),
        ]);
        let j = toml_to_json(&v);
        assert_eq!(j, serde_json::json!([1, 2]));
    }

    #[test]
    fn toml_to_json_table() {
        let mut tbl = toml::map::Map::new();
        tbl.insert("x".to_owned(), toml::Value::Integer(99));
        let v = toml::Value::Table(tbl);
        let j = toml_to_json(&v);
        assert_eq!(j["x"], serde_json::json!(99));
    }

    // -----------------------------------------------------------------------
    // show (output format smoke tests)
    // -----------------------------------------------------------------------

    #[test]
    fn show_human_does_not_panic() {
        let cfg = Config::default();
        // Just confirm the function body doesn't panic.
        print_human(&cfg);
    }

    // -----------------------------------------------------------------------
    // validate smoke test
    // -----------------------------------------------------------------------

    #[test]
    fn default_config_passes_validation() {
        let cfg = Config::default();
        let result = polkagent_config::validate::validate(&cfg);
        assert!(
            result.is_ok(),
            "default config should be valid: {:?}",
            result.err()
        );
    }

    // -----------------------------------------------------------------------
    // get — integration-level: load then dotted lookup
    // -----------------------------------------------------------------------

    #[test]
    fn get_key_from_file_overridden_value() {
        let tmp = TempDir::new().expect("tempdir");
        let path = write_config(tmp.path(), "[log]\nlevel = \"warn\"\n");
        let cfg = load_config(Some(path.to_str().unwrap().to_owned())).expect("load");
        let val: toml::Value = toml::Value::try_from(&cfg).expect("serialize");
        let result = lookup_dotted(&val, "log.level");
        assert_eq!(result.and_then(|v| v.as_str()), Some("warn"));
    }

    #[test]
    fn get_api_enabled_default() {
        let cfg = load_config(None).expect("load");
        let val: toml::Value = toml::Value::try_from(&cfg).expect("serialize");
        let result = lookup_dotted(&val, "api.enabled");
        assert!(result.is_some(), "api.enabled should exist");
    }
}

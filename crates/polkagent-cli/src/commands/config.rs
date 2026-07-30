//! `polkagent config` subcommand handlers.

use anyhow::Result;

use crate::cli::{ConfigCmd, ConfigShowCmd, ConfigValidateCmd};

/// Dispatch the config subcommand.
pub fn run(cmd: &ConfigCmd) -> Result<()> {
    match cmd {
        ConfigCmd::Show(c)     => show(c),
        ConfigCmd::Validate(c) => validate(c),
    }
}

// ---------------------------------------------------------------------------
// show
// ---------------------------------------------------------------------------

fn show(cmd: &ConfigShowCmd) -> Result<()> {
    // Load the resolved config (file + env overrides).
    let cfg = load_config(None)?;

    if cmd.json {
        println!("{}", serde_json::to_string_pretty(&cfg)?);
    } else if cmd.toml {
        println!("{}", toml::to_string_pretty(&cfg)?);
    } else {
        // Pretty-print the most relevant fields.
        println!("Configuration:");
        println!("  schema_version: {}", cfg.meta.schema_version);
        println!("  log.level:      {}", cfg.log.level);
        println!("  database:");
        println!("    backend: {:?}", cfg.database.backend);
        println!("    sqlite.path: {}", cfg.database.sqlite.path);
        println!("  execution:");
        println!("    max_concurrent_runs:  {}", cfg.execution.max_concurrent_runs);
        println!("    default_timeout_secs: {}", cfg.execution.default_timeout_secs);
        println!("    budget.max_usd_per_run: ${:.2}", cfg.execution.budget.max_usd_per_run);
        println!("  api:");
        println!("    enabled:      {}", cfg.api.enabled);
        println!("    bind_address: {}", cfg.api.bind_address);
        println!("  tui:");
        println!("    theme:                {:?}", cfg.tui.theme);
        println!("    atmospheric_effects:  {}", cfg.tui.atmospheric_effects);
        if !cfg.providers.is_empty() {
            println!("  providers:");
            for p in &cfg.providers {
                println!("    - {} ({}) default_model={}", p.id, p.provider_type, p.default_model);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

fn validate(cmd: &ConfigValidateCmd) -> Result<()> {
    let result = load_config(cmd.path.as_deref().map(|p| p.to_str().unwrap_or("")).map(String::from));
    match result {
        Ok(cfg) => {
            println!("Configuration is valid.");
            println!("  schema_version: {}", cfg.meta.schema_version);
            println!("  {} provider(s) configured", cfg.providers.len());
        }
        Err(e) => {
            eprintln!("Configuration is INVALID:");
            eprintln!("  {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Loader helper
// ---------------------------------------------------------------------------

/// Load configuration from the standard search paths with env overrides.
fn load_config(
    override_path: Option<String>,
) -> Result<polkagent_config::schema::Config> {
    use polkagent_config::{env::apply_env_overrides, schema::Config};

    // Try to load from file if a path is given or discovered.
    let mut cfg = if let Some(path) = override_path {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("reading {path}: {e}"))?;
        toml::from_str::<Config>(&content)
            .map_err(|e| anyhow::anyhow!("parsing {path}: {e}"))?
    } else {
        // Try standard locations in order of precedence.
        find_and_load_config().unwrap_or_default()
    };

    // Apply environment variable overrides.
    apply_env_overrides(&mut cfg);
    Ok(cfg)
}

/// Search for a config file in standard locations.
fn find_and_load_config() -> Option<polkagent_config::schema::Config> {
    let candidates = [
        // Project-local (highest precedence).
        ".polkagent/polkagent.toml".to_owned(),
        // User-global.
        dirs_config_path(),
    ];

    for path in &candidates {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(cfg) = toml::from_str(&content) {
                return Some(cfg);
            }
        }
    }

    None
}

fn dirs_config_path() -> String {
    if let Ok(home) = std::env::var("HOME") {
        format!("{home}/.config/polkagent/polkagent.toml")
    } else {
        "~/.config/polkagent/polkagent.toml".to_owned()
    }
}

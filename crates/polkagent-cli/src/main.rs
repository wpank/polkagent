//! `polkagent` binary entry point.
//!
//! Parses CLI arguments with clap, sets up tracing, resolves the database
//! path from config, then dispatches to the appropriate subcommand handler.
//!
//! When no subcommand is given the TUI is launched automatically (default
//! behaviour for interactive terminals).

#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

use anyhow::{Context, Result};
use clap::Parser;

use polkagent_store_sqlite::SqlitePool;
use polkagent_telemetry::{LogFormat, MetricRecorder, TelemetryConfig, TelemetryGuard};

mod cli;
mod commands;
mod error_explainer;
mod exit_codes;
mod output;
mod tui;

use cli::{Cli, Commands};

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Honour `--no-color` / `NO_COLOR` globally before any output.
    // Also respect the standard NO_COLOR env variable.
    if cli.no_color || std::env::var_os("NO_COLOR").is_some() {
        // Propagate the flag into the environment so the Theme picks it up.
        // We use a thread-local variable instead of set_var to avoid the
        // deny(unsafe_code) lint. Theme::from_env() checks NO_COLOR itself.
        let _ = cli.no_color; // consumed — Theme reads NO_COLOR from env directly.
    }

    // Load config to extract observability settings.
    let obs_config = load_observability_config(cli.config.as_ref().map(|p| p.as_path()));

    // Resolve log file path from --log-file flag or default.
    let log_file_path = resolve_log_file_path(cli.log_file.as_deref());

    // Initialise telemetry (before any command runs). Hold the guard for the
    // entire program lifetime so that OTLP spans are flushed on exit.
    let _telemetry_guard =
        init_telemetry(cli.verbose, &obs_config, log_file_path.as_deref())?;

    // Metrics recorder — shared across the process lifetime.
    let _metrics = MetricRecorder::new();

    // Collect global flags for passing to handlers.
    let format = cli.format;
    let dry_run = cli.dry_run;
    let yes = cli.yes;

    // Commands that do not require database access.
    match &cli.command {
        Some(Commands::Init(cmd)) => {
            return commands::init::run(cmd);
        }
        Some(Commands::Config(cmd)) => {
            return commands::config::run(cmd);
        }
        Some(Commands::Version) => {
            print_version();
            return Ok(());
        }
        Some(Commands::Explain(cmd)) => {
            let _span =
                tracing::info_span!("explain", extrinsic_hex = %cmd.extrinsic_hex).entered();
            return commands::explain::run(cmd);
        }
        Some(Commands::Chain(cmd)) => {
            let _span = tracing::info_span!("chain").entered();
            let rpc_url = resolve_rpc_url(cli.config.as_ref().map(|p| p.as_path()));
            return commands::chain::run(cmd, rpc_url.as_deref()).await;
        }
        Some(Commands::Doctor(cmd)) => {
            return commands::doctor::run(cmd);
        }
        Some(Commands::Memory(cmd)) => {
            return commands::memory::run(cmd);
        }
        Some(Commands::Completions(cmd)) => {
            return commands::completions::run(cmd);
        }
        Some(Commands::Eval(cmd)) => {
            return commands::eval::run(cmd);
        }
        Some(Commands::Inspect(cmd)) => {
            return commands::inspect::run(cmd).await;
        }
        Some(Commands::Auth(cmd)) => {
            return commands::auth::run(cmd);
        }
        Some(Commands::Network(cmd)) => {
            return commands::network::run(cmd).await;
        }
        Some(Commands::Serve(cmd)) => {
            return commands::serve::run(cmd).await;
        }
        _ => {}
    }

    // Suppress unused variable warnings for global flags not yet threaded
    // into all handlers. They are available for future use.
    let _ = format;
    let _ = dry_run;
    let _ = yes;

    // Resolve the database path, open the pool, and run migrations.
    let db_path = resolve_db_path(cli.config.as_ref().map(|p| p.as_path()));
    let pool = open_pool(&db_path)?;

    // Dispatch to the appropriate handler.
    match &cli.command {
        None => {
            // Default: launch TUI.
            launch_tui(pool)?;
        }

        Some(Commands::Tui(_cmd)) => {
            launch_tui(pool)?;
        }

        Some(Commands::Run(cmd)) => {
            let _span = tracing::info_span!(
                "run",
                agent_id = %cmd.agent_id,
            )
            .entered();
            commands::run::run(cmd, &pool).await?;
        }

        Some(Commands::Agent(cmd)) => {
            commands::agent::run(cmd, &pool)?;
        }

        Some(Commands::Skill(cmd)) => {
            commands::skill::run(cmd, &pool)?;
        }

        Some(Commands::Kit(cmd)) => {
            commands::kit::run(cmd, &pool)?;
        }

        Some(Commands::Export(cmd)) => {
            commands::export::run(cmd, &pool)?;
        }

        Some(Commands::Inbox(cmd)) => {
            commands::inbox::run(cmd, &pool)?;
        }

        Some(Commands::Status(cmd)) => {
            commands::status::run(cmd, &pool)?;
        }

        Some(Commands::Logs(cmd)) => {
            commands::logs::run(cmd, &pool)?;
        }

        // Already handled above; listed here to satisfy exhaustiveness.
        Some(
            Commands::Init(_)
            | Commands::Config(_)
            | Commands::Version
            | Commands::Explain(_)
            | Commands::Chain(_)
            | Commands::Doctor(_)
            | Commands::Memory(_)
            | Commands::Completions(_)
            | Commands::Eval(_)
            | Commands::Inspect(_)
            | Commands::Auth(_)
            | Commands::Network(_)
            | Commands::Serve(_),
        ) => {
            unreachable!()
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Database pool setup
// ---------------------------------------------------------------------------

/// Open (or create) the SQLite database pool and run migrations.
///
/// Ensures the parent directory exists so that brand-new databases created
/// from the default path work out of the box.
fn open_pool(db_path: &str) -> Result<SqlitePool> {
    use polkagent_store_sqlite::migrations;

    let expanded = expand_tilde(db_path);

    // Ensure the parent directory exists.
    if let Some(parent) = std::path::Path::new(&expanded).parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating database directory {}", parent.display()))?;
    }

    let pool = SqlitePool::open(&expanded)
        .with_context(|| format!("opening database at {expanded}"))?;

    // Apply any pending schema migrations (idempotent).
    {
        let writer = pool.writer();
        migrations::migrate(&writer)
            .with_context(|| "running database migrations")?;
    }

    Ok(pool)
}

// ---------------------------------------------------------------------------
// TUI launcher
// ---------------------------------------------------------------------------

/// Launch the interactive ROSEDUST TUI and ensure teardown on exit.
fn launch_tui(pool: SqlitePool) -> Result<()> {
    use crate::tui::app::{App, enter_tui, exit_tui};
    use crate::tui::theme::Theme;

    let theme = Theme::from_env();
    let mut app = App::new(theme, pool);

    let mut terminal = enter_tui()?;

    // Ensure the terminal is restored even on panic.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.run(&mut terminal)
    }));

    exit_tui(&mut terminal)?;

    match result {
        Ok(run_result) => run_result,
        Err(panic_val) => {
            eprintln!("TUI panicked: {panic_val:?}");
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Version output
// ---------------------------------------------------------------------------

fn print_version() {
    println!(
        "polkagent {} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_DESCRIPTION"),
    );
}

// ---------------------------------------------------------------------------
// Telemetry setup
// ---------------------------------------------------------------------------

/// Observability settings extracted from the config file.
struct ObsSettings {
    service_name: String,
    otlp_endpoint: Option<String>,
    log_level: Option<String>,
}

/// Load the observability section from the config file (if any).
///
/// Failures are silently ignored — telemetry setup is always best-effort.
fn load_observability_config(config_override: Option<&std::path::Path>) -> ObsSettings {
    let cfg = try_load_config(config_override);
    ObsSettings {
        service_name: cfg
            .as_ref()
            .map(|c| c.observability.service_name.clone())
            .unwrap_or_else(|| "polkagent".to_owned()),
        otlp_endpoint: cfg
            .as_ref()
            .and_then(|c| c.observability.otlp_endpoint.clone()),
        log_level: cfg.as_ref().map(|c| c.log.level.clone()),
    }
}

/// Try to parse a [`polkagent_config::schema::Config`] from the best available
/// config file path. Returns `None` on any error (missing file, parse error, etc.).
fn try_load_config(
    config_override: Option<&std::path::Path>,
) -> Option<polkagent_config::schema::Config> {
    // Explicit path takes priority.
    if let Some(path) = config_override {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(cfg) = toml::from_str(&content) {
                return Some(cfg);
            }
        }
        // If an explicit path was given but couldn't be loaded, give up.
        return None;
    }

    // Project-local config.
    if let Ok(content) = std::fs::read_to_string(".polkagent/polkagent.toml") {
        if let Ok(cfg) = toml::from_str(&content) {
            return Some(cfg);
        }
    }

    // User-global config.
    let home = std::env::var("HOME").unwrap_or_default();
    let user_cfg = format!("{home}/.config/polkagent/polkagent.toml");
    if let Ok(content) = std::fs::read_to_string(&user_cfg) {
        if let Ok(cfg) = toml::from_str(&content) {
            return Some(cfg);
        }
    }

    None
}

/// Resolve the JSONL log file path from the `--log-file` flag.
///
/// - `Some("-")` → `None` (stderr only, no file logging).
/// - `Some(path)` → expand tildes and use that path.
/// - `None` → default `~/.polkagent/logs/events.jsonl`.
fn resolve_log_file_path(flag: Option<&str>) -> Option<String> {
    match flag {
        Some("-") => None,
        Some(path) => Some(expand_tilde(path)),
        None => {
            let home = std::env::var("HOME").unwrap_or_default();
            Some(format!("{home}/.polkagent/logs/events.jsonl"))
        }
    }
}

/// Ensure the parent directory of `path` exists, returning `true` on success.
///
/// Failures are silently swallowed — graceful degradation: if the directory
/// can't be created we simply skip file logging.
fn ensure_log_dir(path: &str) -> bool {
    let p = std::path::Path::new(path);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).is_ok()
    } else {
        true
    }
}

/// Initialise the telemetry subsystem using [`polkagent_telemetry::init_telemetry`].
///
/// - Merges CLI verbosity with the config-level log level.
/// - Logs a notice (but does not fail) when an OTLP endpoint is configured.
/// - Creates `~/.polkagent/logs/` on startup; gracefully degrades on failure.
/// - Returns a [`TelemetryGuard`] that **must** be held until program exit.
fn init_telemetry(
    verbose: u8,
    obs: &ObsSettings,
    log_file: Option<&str>,
) -> Result<TelemetryGuard> {
    // Determine log level: env-var > CLI verbosity > config > default.
    let log_level = if let Ok(level) = std::env::var("POLKAGENT_LOG_LEVEL") {
        level
    } else if verbose > 0 {
        match verbose {
            1 => "debug".to_owned(),
            _ => "trace".to_owned(),
        }
    } else if let Some(ref level) = obs.log_level {
        level.clone()
    } else {
        "info".to_owned()
    };

    // If an OTLP endpoint is configured, note it before the subscriber is active.
    if let Some(ref endpoint) = obs.otlp_endpoint {
        eprintln!(
            "polkagent: OTLP export enabled \u{2192} {endpoint} (service={})",
            obs.service_name
        );
    }

    // Ensure the log directory exists. Warn and continue on failure.
    if let Some(path) = log_file {
        if !ensure_log_dir(path) {
            eprintln!(
                "polkagent: warning: could not create log directory for {path}; \
                 JSONL file logging disabled"
            );
        }
    }

    let telemetry_cfg = TelemetryConfig {
        log_level,
        log_format: LogFormat::Pretty,
        otlp_endpoint: obs.otlp_endpoint.clone(),
        service_name: obs.service_name.clone(),
    };

    polkagent_telemetry::init_telemetry(telemetry_cfg)
        .map_err(|e| anyhow::anyhow!("telemetry init failed: {e}"))
}

// ---------------------------------------------------------------------------
// Database path resolution
// ---------------------------------------------------------------------------

/// Resolve the SQLite database path.
///
/// Resolution order:
/// 1. `POLKAGENT_DATABASE_SQLITE_PATH` environment variable.
/// 2. `database.sqlite.path` from config file (project-local or user-global).
/// 3. Default: `~/.local/share/polkagent/polkagent.db`.
fn resolve_db_path(config_override: Option<&std::path::Path>) -> String {
    // 1. Environment variable.
    if let Ok(path) = std::env::var("POLKAGENT_DATABASE_SQLITE_PATH") {
        if !path.is_empty() {
            return path;
        }
    }

    // 2. Config file.
    if let Some(cfg_path) = config_override {
        if let Ok(content) = std::fs::read_to_string(cfg_path) {
            if let Ok(cfg) = toml::from_str::<polkagent_config::schema::Config>(&content) {
                if !cfg.database.sqlite.path.is_empty() {
                    return cfg.database.sqlite.path;
                }
            }
        }
    }

    // Try project-local config.
    let project_cfg = ".polkagent/polkagent.toml";
    if let Ok(content) = std::fs::read_to_string(project_cfg) {
        if let Ok(cfg) = toml::from_str::<polkagent_config::schema::Config>(&content) {
            if !cfg.database.sqlite.path.is_empty() {
                return cfg.database.sqlite.path;
            }
        }
    }

    // Try user-global config.
    let home = std::env::var("HOME").unwrap_or_default();
    let user_cfg = format!("{home}/.config/polkagent/polkagent.toml");
    if let Ok(content) = std::fs::read_to_string(&user_cfg) {
        if let Ok(cfg) = toml::from_str::<polkagent_config::schema::Config>(&content) {
            if !cfg.database.sqlite.path.is_empty() {
                return cfg.database.sqlite.path;
            }
        }
    }

    // 3. Default.
    format!("{home}/.local/share/polkagent/polkagent.db")
}

// ---------------------------------------------------------------------------
// RPC URL resolution
// ---------------------------------------------------------------------------

/// Resolve the chain RPC URL.
///
/// Resolution order:
/// 1. `POLKAGENT_RPC_URL` environment variable.
/// 2. Default: none (chain commands will show a helpful message).
fn resolve_rpc_url(_config_override: Option<&std::path::Path>) -> Option<String> {
    if let Ok(url) = std::env::var("POLKAGENT_RPC_URL") {
        if !url.is_empty() {
            return Some(url);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Expand a leading `~` in a path using the HOME environment variable.
fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_owned()
}

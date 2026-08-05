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
use clap::{CommandFactory, Parser};
use std::io::IsTerminal as _;

use polkagent_store_sqlite::SqlitePool;
use polkagent_telemetry::{LogFormat, MetricRecorder, TelemetryConfig, TelemetryGuard};

mod cli;
mod commands;
mod error_explainer;
mod exit_codes;
mod output;
mod tui;

use cli::{Cli, Commands};
use output::OutputFormat;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let (code, error) = run_main().await;
    if let Some(ref err) = error {
        eprintln!("Error: {err:#}");
    }
    std::process::exit(code);
}

/// Core application logic. Returns `(exit_code, optional_error)`.
///
/// Separating this from `main()` lets us control the process exit code
/// directly via [`exit_codes`] constants rather than relying on the default
/// anyhow error-printing behaviour.
async fn run_main() -> (i32, Option<anyhow::Error>) {
    let cli = Cli::parse();

    // Honour `--no-color` / `NO_COLOR` globally before any output.
    // Propagate --no-color into the NO_COLOR env var so that the Theme and
    // downstream code (e.g. tracing-subscriber) picks it up automatically.
    if cli.no_color {
        // SAFETY: called before any Tokio worker threads are spawned.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("NO_COLOR", "1");
        }
    }

    // Resolve the config path: CLI flag takes priority over POLKAGENT_CONFIG
    // env var.  Both are optional; absence triggers auto-discovery.
    let config_path: Option<std::path::PathBuf> = cli.config.clone().or_else(|| {
        std::env::var("POLKAGENT_CONFIG")
            .ok()
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
    });

    // Load config to extract observability settings.
    let obs_config = load_observability_config(config_path.as_deref());

    // Resolve log file path from --log-file flag or default.
    let log_file_path = resolve_log_file_path(cli.log_file.as_deref());

    // Initialise telemetry (before any command runs). Hold the guard for the
    // entire program lifetime so that OTLP spans are flushed on exit.
    let _telemetry_guard = match init_telemetry(
        cli.verbose,
        cli.no_color,
        &obs_config,
        log_file_path.as_deref(),
    ) {
        Ok(guard) => guard,
        Err(e) => return (exit_codes::CONFIG_ERROR, Some(e)),
    };

    // Metrics recorder — shared across the process lifetime.
    let _metrics = MetricRecorder::new();

    // Collect global flags for passing to handlers.
    let format = cli.format;
    let dry_run = cli.dry_run;
    let yes = cli.yes;

    // Commands that do not require database access.
    let (cmd_name, result) = match &cli.command {
        Some(Commands::Init(cmd)) => ("init", commands::init::run(cmd)),
        Some(Commands::Config(cmd)) => ("config", commands::config::run(cmd)),
        Some(Commands::Version) => {
            print_version();
            return (exit_codes::SUCCESS, None);
        }
        Some(Commands::Explain(cmd)) => {
            let _span =
                tracing::info_span!("explain", extrinsic_hex = %cmd.extrinsic_hex).entered();
            ("explain", commands::explain::run(cmd))
        }
        Some(Commands::Chain(cmd)) => {
            let _span = tracing::info_span!("chain").entered();
            let rpc_url = resolve_rpc_url(config_path.as_deref());
            ("chain", commands::chain::run(cmd, rpc_url.as_deref()).await)
        }
        Some(Commands::Doctor(cmd)) => {
            ("doctor", commands::doctor::run(cmd, config_path.as_deref()))
        }
        Some(Commands::Memory(cmd)) => ("memory", commands::memory::run(cmd)),
        Some(Commands::Completions(cmd)) => ("completions", commands::completions::run(cmd)),
        Some(Commands::Eval(cmd)) => ("eval", commands::eval::run(cmd).await),
        Some(Commands::Inspect(cmd)) => ("inspect", commands::inspect::run(cmd).await),
        Some(Commands::Auth(cmd)) => ("auth", commands::auth::run(cmd)),
        Some(Commands::Network(cmd)) => ("network", commands::network::run(cmd).await),
        Some(Commands::Serve(cmd)) => (
            "serve",
            commands::serve::run(cmd, config_path.as_deref()).await,
        ),
        Some(Commands::Package(cmd)) => (
            "package",
            commands::package::run(cmd, format, config_path.as_deref(), dry_run, yes),
        ),
        _ => {
            // Suppress unused variable warnings for global flags not yet threaded
            // into all handlers. They are available for future use.
            let _ = yes;

            // Resolve the database path, open the pool, and run migrations.
            let db_path = resolve_db_path(config_path.as_deref());
            let pool = match open_pool(&db_path) {
                Ok(p) => p,
                Err(e) => return (exit_codes::CONFIG_ERROR, Some(e)),
            };

            // Dispatch to the appropriate handler.
            let (name, res) = match &cli.command {
                None => {
                    if std::io::stdout().is_terminal() {
                        ("tui", launch_tui(pool, "dashboard"))
                    } else {
                        let _ = Cli::command().print_help();
                        println!();
                        ("help", Ok(()))
                    }
                }
                Some(Commands::Tui(cmd)) => {
                    if std::io::stdout().is_terminal() {
                        ("tui", launch_tui(pool, &cmd.tab))
                    } else {
                        (
                            "tui",
                            Err(anyhow::anyhow!(
                                "the TUI requires an interactive terminal; stdout is not a TTY"
                            )),
                        )
                    }
                }
                Some(Commands::Run(cmd)) => {
                    let _span = tracing::info_span!(
                        "run",
                        agent_id = %cmd.agent_id,
                    )
                    .entered();
                    ("run", commands::run::run(cmd, &pool, dry_run).await)
                }
                Some(Commands::Agent(cmd)) => ("agent", commands::agent::run(cmd, &pool, format)),
                Some(Commands::Skill(cmd)) => ("skill", commands::skill::run(cmd, &pool)),
                Some(Commands::Kit(cmd)) => ("kit", commands::kit::run(cmd, &pool)),
                Some(Commands::Export(cmd)) => ("export", commands::export::run(cmd, &pool)),
                Some(Commands::Inbox(cmd)) => ("inbox", commands::inbox::run(cmd, &pool)),
                Some(Commands::Status(cmd)) => {
                    ("status", commands::status::run(cmd, &pool, format))
                }
                Some(Commands::Logs(cmd)) => ("logs", commands::logs::run(cmd, &pool)),
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
                    | Commands::Serve(_)
                    | Commands::Package(_),
                ) => unreachable!(),
            };
            (name, res)
        }
    };

    // Format output for successful JSON/JSON-pretty responses when the
    // global --format flag was set to a JSON variant.
    finish_command(cmd_name, format, result)
}

// ---------------------------------------------------------------------------
// Output formatting + exit code wiring
// ---------------------------------------------------------------------------

/// Translate a command result into an exit code, optionally emitting JSON
/// output when the global `--format` flag selects a JSON variant.
fn finish_command(
    cmd_name: &str,
    format: OutputFormat,
    result: Result<()>,
) -> (i32, Option<anyhow::Error>) {
    match result {
        Ok(()) => {
            // When the user asked for JSON output via `--format`, emit a
            // success envelope so scripts can parse a consistent shape.
            if cmd_name != "package"
                && matches!(format, OutputFormat::Json | OutputFormat::JsonPretty)
            {
                let envelope = serde_json::json!({ "ok": true });
                println!("{}", output::format_output(&envelope, format, cmd_name));
            }
            (exit_codes::SUCCESS, None)
        }
        Err(e) => {
            let code = classify_exit_code(&e);

            // When the user asked for JSON output, emit a structured error
            // envelope instead of the default human-readable message.
            if matches!(format, OutputFormat::Json | OutputFormat::JsonPretty) {
                let envelope = serde_json::json!({
                    "ok": false,
                    "error": format!("{e:#}"),
                    "exit_code": code,
                });
                eprintln!("{}", output::format_output(&envelope, format, cmd_name));
                // Error already rendered as JSON; suppress the default print.
                (code, None)
            } else {
                (code, Some(e))
            }
        }
    }
}

/// Classify an `anyhow::Error` into one of the [`exit_codes`] constants by
/// inspecting the error message for well-known patterns.
///
/// This reuses the keyword-matching approach from [`error_explainer`] but
/// maps to process exit codes instead of user-facing explanations.
fn classify_exit_code(err: &anyhow::Error) -> i32 {
    let msg = format!("{err:#}").to_lowercase();

    // Approval / policy checks.
    if msg.contains("approval")
        || msg.contains("requires approval")
        || msg.contains("awaiting approval")
    {
        return exit_codes::APPROVAL_REQUIRED;
    }
    if msg.contains("policy")
        || msg.contains("denied")
        || msg.contains("not allowed")
        || msg.contains("forbidden")
        || msg.contains("permission")
    {
        return exit_codes::POLICY_DENIED;
    }

    // Configuration errors.
    if msg.contains("config")
        || msg.contains("configuration")
        || msg.contains("missing")
        || msg.contains("invalid")
        || msg.contains("cannot be read")
        || msg.contains("polkagent.toml")
        || msg.contains("schema")
        || msg.contains("database")
        || msg.contains("migration")
    {
        return exit_codes::CONFIG_ERROR;
    }

    // Network / connectivity errors.
    if msg.contains("network")
        || msg.contains("connection")
        || msg.contains("unreachable")
        || msg.contains("timed out")
        || msg.contains("timeout")
        || msg.contains("rpc")
        || msg.contains("endpoint")
        || msg.contains("dns")
        || msg.contains("refused")
    {
        return exit_codes::NETWORK_ERROR;
    }

    // Unknown outcome (daemon communication failures, etc.).
    if msg.contains("unknown outcome") || msg.contains("daemon unreachable") {
        return exit_codes::UNKNOWN_OUTCOME;
    }

    // Fallback: general error.
    exit_codes::ERROR
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

    let pool =
        SqlitePool::open(&expanded).with_context(|| format!("opening database at {expanded}"))?;

    // Apply any pending schema migrations (idempotent).
    {
        let writer = pool.writer();
        migrations::migrate(&writer).with_context(|| "running database migrations")?;
    }

    Ok(pool)
}

// ---------------------------------------------------------------------------
// TUI launcher
// ---------------------------------------------------------------------------

/// Launch the interactive ROSEDUST TUI and ensure teardown on exit.
///
/// `tab` is the `--tab` flag value (e.g. `"dashboard"`, `"memory"`).
fn launch_tui(pool: SqlitePool, tab: &str) -> Result<()> {
    use crate::tui::app::{enter_tui, exit_tui, App, Tab};
    use crate::tui::theme::Theme;

    let theme = Theme::from_env();
    let initial_tab = Tab::from_cli_str(tab);
    let mut app = App::new(theme, pool, initial_tab);

    let mut terminal = enter_tui()?;

    // Ensure the terminal is restored even on panic.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.run(&mut terminal)));

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
    println!("polkagent {}", env!("CARGO_PKG_VERSION"));
}

// ---------------------------------------------------------------------------
// Telemetry setup
// ---------------------------------------------------------------------------

/// Observability settings extracted from the config file.
struct ObsSettings {
    service_name: String,
    otlp_endpoint: Option<String>,
    log_level: Option<String>,
    /// `log.format` from the config file, if present.
    log_format: Option<polkagent_config::schema::LogFormat>,
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
        log_format: cfg.as_ref().map(|c| c.log.format),
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
    no_color: bool,
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
        "warn".to_owned()
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

    let ansi = !no_color && std::env::var_os("NO_COLOR").is_none();

    // Determine log format: env var > config file > default (Pretty).
    let log_format = match std::env::var("POLKAGENT_LOG_FORMAT")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "json" => LogFormat::Json,
        "pretty" => LogFormat::Pretty,
        _ => match obs.log_format {
            Some(polkagent_config::schema::LogFormat::Json) => LogFormat::Json,
            _ => LogFormat::Pretty,
        },
    };

    let telemetry_cfg = TelemetryConfig {
        log_level,
        log_format,
        otlp_endpoint: obs.otlp_endpoint.clone(),
        service_name: obs.service_name.clone(),
        ansi,
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
/// 2. `database.sqlite.path` from config file (explicit override, then
///    project-local, then user-global — same discovery order as
///    [`try_load_config`]).
/// 3. Default: `~/.local/share/polkagent/polkagent.db`.
fn resolve_db_path(config_override: Option<&std::path::Path>) -> String {
    // 1. Environment variable wins unconditionally.
    if let Ok(path) = std::env::var("POLKAGENT_DATABASE_SQLITE_PATH") {
        if !path.is_empty() {
            return path;
        }
    }

    // 2. Config file (reuse the layered loader so discovery is consistent).
    if let Some(cfg) = try_load_config(config_override) {
        if !cfg.database.sqlite.path.is_empty() {
            return cfg.database.sqlite.path;
        }
    }

    // 3. Default.
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.local/share/polkagent/polkagent.db")
}

// ---------------------------------------------------------------------------
// RPC URL resolution
// ---------------------------------------------------------------------------

/// Resolve the chain RPC URL.
///
/// Resolution order:
/// 1. `POLKAGENT_CHAIN_RPC_URL` environment variable (preferred alias).
/// 2. `POLKAGENT_RPC_URL` environment variable (legacy name).
/// 3. Default: none (chain commands will show a helpful message).
fn resolve_rpc_url(_config_override: Option<&std::path::Path>) -> Option<String> {
    // Check the preferred alias first.
    if let Ok(url) = std::env::var("POLKAGENT_CHAIN_RPC_URL") {
        if !url.is_empty() {
            return Some(url);
        }
    }
    // Fall back to the legacy name.
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

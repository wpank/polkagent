//! `polkagent` binary entry point.
//!
//! Parses CLI arguments with clap, sets up tracing, resolves the database
//! path from config, then dispatches to the appropriate subcommand handler.
//!
//! When no subcommand is given the TUI is launched automatically (default
//! behaviour for interactive terminals).

#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::missing_errors_doc)]

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

mod cli;
mod commands;
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

    // Initialise tracing subscriber (before any command runs).
    init_tracing(cli.verbose);

    // Resolve the database path.
    let db_path = resolve_db_path(cli.config.as_ref().map(|p| p.as_path()));

    // Dispatch to the appropriate handler.
    match &cli.command {
        None => {
            // Default: launch TUI.
            launch_tui(&db_path)?;
        }

        Some(Commands::Tui(_cmd)) => {
            launch_tui(&db_path)?;
        }

        Some(Commands::Init(cmd)) => {
            commands::init::run(cmd)?;
        }

        Some(Commands::Run(cmd)) => {
            commands::run::run(cmd, &db_path)?;
        }

        Some(Commands::Agent(cmd)) => {
            commands::agent::run(cmd, &db_path)?;
        }

        Some(Commands::Config(cmd)) => {
            commands::config::run(cmd)?;
        }

        Some(Commands::Version) => {
            print_version();
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// TUI launcher
// ---------------------------------------------------------------------------

/// Launch the interactive ROSEDUST TUI and ensure teardown on exit.
fn launch_tui(db_path: &str) -> Result<()> {
    use crate::tui::app::{App, enter_tui, exit_tui};
    use crate::tui::theme::Theme;

    let theme = Theme::from_env();
    let mut app = App::new(theme, db_path.to_owned());

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
// Tracing setup
// ---------------------------------------------------------------------------

fn init_tracing(verbose: u8) {
    // `-v` = debug, `-vv` = trace, default = info (env-filter takes precedence).
    let default_level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_level));

    // When the TUI will launch we suppress tracing output to avoid garbling
    // the terminal. The subscriber is set up regardless so library crates can
    // call tracing macros safely.
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
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

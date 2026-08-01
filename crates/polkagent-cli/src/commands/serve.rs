//! `polkagent serve` -- start the HTTP API + WebSocket server (daemon mode).
//!
//! Reads the active configuration for the bind address, CORS settings, and
//! database path, then starts the Axum-backed polkagent-api server. Handles
//! SIGTERM and SIGINT for graceful shutdown.
//!
//! # Flags
//!
//! | Flag              | Description                                          |
//! |-------------------|------------------------------------------------------|
//! | `--port`          | TCP port to bind (default 8080)                      |
//! | `--host`          | Bind host (default 0.0.0.0)                          |
//! | `--cors-origin`   | Allowed CORS origins (repeatable; overrides config)   |
//! | `--read-only`     | Reject all mutating requests                         |

use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::info;

use polkagent_api::{ApiServer, InMemoryAgentStore, InMemoryRunManager};
use polkagent_event::EventBus;
use polkagent_store_sqlite::SqlitePool;
use polkagent_store_trait::EffectStore;

use crate::cli::ServeCmd;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Execute the `serve` subcommand.
///
/// Opens the SQLite database, runs pending migrations, constructs the API
/// server with production-grade stores, binds to the configured address, and
/// serves until a shutdown signal is received.
pub async fn run(cmd: &ServeCmd) -> Result<()> {
    // -----------------------------------------------------------------------
    // 1. Resolve configuration.
    // -----------------------------------------------------------------------
    let mut config = resolve_config(None)?;

    // -----------------------------------------------------------------------
    // 2. Apply CLI overrides to config.
    // -----------------------------------------------------------------------
    apply_cli_overrides(&mut config, cmd);

    // -----------------------------------------------------------------------
    // 3. Determine bind address from CLI flags.
    // -----------------------------------------------------------------------
    let bind_addr = build_bind_addr(cmd);

    // -----------------------------------------------------------------------
    // 4. Open (or create) the SQLite database and run migrations.
    // -----------------------------------------------------------------------
    let db_path = resolve_db_path(None);
    let pool = open_pool(&db_path)?;

    // -----------------------------------------------------------------------
    // 5. Construct stores.
    // -----------------------------------------------------------------------
    // EffectStore is implemented on SqlitePool directly; use Arc<SqlitePool>.
    let effect_store = Arc::new(pool.clone());

    // The AgentStore and RunManager are in-memory for now; a future task will
    // wire in the durable SQLite-backed implementations.
    let agent_store = Arc::new(InMemoryAgentStore::new());
    let run_manager = Arc::new(InMemoryRunManager::new());
    let event_bus = EventBus::with_default_capacity();

    // -----------------------------------------------------------------------
    // 6. Build and start the server.
    // -----------------------------------------------------------------------
    let server = ApiServer::new(
        config,
        agent_store,
        run_manager,
        effect_store as Arc<dyn EffectStore>,
        event_bus,
    );

    println!("Polkagent API server listening on {bind_addr}");
    println!("{}", "-".repeat(40));
    println!("  Database:    {db_path}");
    if cmd.read_only {
        println!("  Mode:        read-only");
    }
    if !cmd.cors_origins.is_empty() {
        println!("  CORS:        {}", cmd.cors_origins.join(", "));
    }
    println!("  Press Ctrl+C to stop.");
    println!();

    info!(
        bind_addr = %bind_addr,
        db_path = %db_path,
        read_only = cmd.read_only,
        "polkagent serve starting"
    );

    // Wrap the serve future with a graceful shutdown signal.
    let serve_fut = server.serve(&bind_addr);

    tokio::select! {
        result = serve_fut => {
            result.map_err(|e| anyhow::anyhow!("server error: {e}"))?;
        }
        () = shutdown_signal() => {
            println!();
            println!("Shutdown signal received. Stopping server...");
            info!("shutdown signal received");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Apply CLI-level overrides to the resolved config.
///
/// - `--cors-origin` values replace the config's `api.cors_origins` list.
/// - `--read-only` sets `api.read_only = true`.
fn apply_cli_overrides(config: &mut polkagent_config::Config, cmd: &ServeCmd) {
    if !cmd.cors_origins.is_empty() {
        config.api.cors_origins = cmd.cors_origins.clone();
    }
    if cmd.read_only {
        config.api.read_only = true;
    }
}

/// Resolve the active polkagent configuration, applying CLI-level overrides.
///
/// Resolution order:
/// 1. `--config` flag path.
/// 2. Project-local `.polkagent/polkagent.toml`.
/// 3. User-global `~/.config/polkagent/polkagent.toml`.
/// 4. Default `Config::default()`.
fn resolve_config(config_override: Option<&str>) -> Result<polkagent_config::Config> {
    if let Some(path) = config_override {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading config file {path}"))?;
        return toml::from_str(&content).with_context(|| format!("parsing config file {path}"));
    }

    // Project-local config.
    if let Ok(content) = std::fs::read_to_string(".polkagent/polkagent.toml") {
        if let Ok(cfg) = toml::from_str(&content) {
            return Ok(cfg);
        }
    }

    // User-global config.
    let home = std::env::var("HOME").unwrap_or_default();
    let user_cfg = format!("{home}/.config/polkagent/polkagent.toml");
    if let Ok(content) = std::fs::read_to_string(&user_cfg) {
        if let Ok(cfg) = toml::from_str(&content) {
            return Ok(cfg);
        }
    }

    Ok(polkagent_config::Config::default())
}

/// Build the `host:port` bind address from the CLI flags.
///
/// With the new `ServeCmd` struct, `host` and `port` always have values
/// (either from the user or from clap defaults).
fn build_bind_addr(cmd: &ServeCmd) -> String {
    format!("{}:{}", cmd.host, cmd.port)
}

/// Resolve the SQLite database path from environment, config, or default.
fn resolve_db_path(config_override: Option<&str>) -> String {
    if let Ok(path) = std::env::var("POLKAGENT_DATABASE_SQLITE_PATH") {
        if !path.is_empty() {
            return path;
        }
    }

    if let Some(cfg_path) = config_override {
        if let Ok(content) = std::fs::read_to_string(cfg_path) {
            if let Ok(cfg) = toml::from_str::<polkagent_config::schema::Config>(&content) {
                if !cfg.database.sqlite.path.is_empty() {
                    return cfg.database.sqlite.path;
                }
            }
        }
    }

    if let Ok(content) = std::fs::read_to_string(".polkagent/polkagent.toml") {
        if let Ok(cfg) = toml::from_str::<polkagent_config::schema::Config>(&content) {
            if !cfg.database.sqlite.path.is_empty() {
                return cfg.database.sqlite.path;
            }
        }
    }

    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.local/share/polkagent/polkagent.db")
}

/// Open (or create) the SQLite pool and run schema migrations.
fn open_pool(db_path: &str) -> Result<SqlitePool> {
    use polkagent_store_sqlite::migrations;

    let expanded = expand_tilde(db_path);

    if let Some(parent) = std::path::Path::new(&expanded).parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating database directory {}", parent.display()))?;
    }

    let pool =
        SqlitePool::open(&expanded).with_context(|| format!("opening database at {expanded}"))?;

    {
        let writer = pool.writer();
        migrations::migrate(&writer).context("running database migrations")?;
    }

    Ok(pool)
}

/// Expand a leading `~/` in a path.
fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_owned()
}

/// Wait for SIGTERM or SIGINT (Ctrl+C).
async fn shutdown_signal() {
    // Ctrl+C (SIGINT).
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    // SIGTERM (e.g. `kill <pid>` or systemd stop).
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Helper to parse CLI args into a `ServeCmd` via the full `Cli` parser.
    fn parse_serve(args: &[&str]) -> crate::cli::ServeCmd {
        let mut full_args = vec!["polkagent", "serve"];
        full_args.extend_from_slice(args);
        let cli = crate::cli::Cli::parse_from(full_args);
        match cli.command {
            Some(crate::cli::Commands::Serve(cmd)) => cmd,
            _ => panic!("expected Serve command"),
        }
    }

    // -----------------------------------------------------------------------
    // Default values
    // -----------------------------------------------------------------------

    #[test]
    fn default_host_is_all_interfaces() {
        let cmd = parse_serve(&[]);
        assert_eq!(cmd.host, "0.0.0.0");
    }

    #[test]
    fn default_port_is_8080() {
        let cmd = parse_serve(&[]);
        assert_eq!(cmd.port, 8080);
    }

    #[test]
    fn default_bind_addr() {
        let cmd = parse_serve(&[]);
        let addr = build_bind_addr(&cmd);
        assert_eq!(addr, "0.0.0.0:8080");
    }

    #[test]
    fn default_read_only_is_false() {
        let cmd = parse_serve(&[]);
        assert!(!cmd.read_only);
    }

    #[test]
    fn default_cors_origins_is_empty() {
        let cmd = parse_serve(&[]);
        assert!(cmd.cors_origins.is_empty());
    }

    // -----------------------------------------------------------------------
    // Argument parsing
    // -----------------------------------------------------------------------

    #[test]
    fn parse_port_override() {
        let cmd = parse_serve(&["--port", "9999"]);
        assert_eq!(cmd.port, 9999);
        let addr = build_bind_addr(&cmd);
        assert_eq!(addr, "0.0.0.0:9999");
    }

    #[test]
    fn parse_host_override() {
        let cmd = parse_serve(&["--host", "127.0.0.1"]);
        assert_eq!(cmd.host, "127.0.0.1");
        let addr = build_bind_addr(&cmd);
        assert_eq!(addr, "127.0.0.1:8080");
    }

    #[test]
    fn parse_host_and_port_override() {
        let cmd = parse_serve(&["--host", "10.0.0.1", "--port", "3000"]);
        assert_eq!(cmd.host, "10.0.0.1");
        assert_eq!(cmd.port, 3000);
        let addr = build_bind_addr(&cmd);
        assert_eq!(addr, "10.0.0.1:3000");
    }

    #[test]
    fn parse_short_port_flag() {
        let cmd = parse_serve(&["-p", "4321"]);
        assert_eq!(cmd.port, 4321);
    }

    // -----------------------------------------------------------------------
    // --read-only flag
    // -----------------------------------------------------------------------

    #[test]
    fn read_only_flag_sets_true() {
        let cmd = parse_serve(&["--read-only"]);
        assert!(cmd.read_only);
    }

    #[test]
    fn read_only_propagates_to_config() {
        let cmd = parse_serve(&["--read-only"]);
        let mut config = polkagent_config::Config::default();
        assert!(!config.api.read_only);
        apply_cli_overrides(&mut config, &cmd);
        assert!(config.api.read_only);
    }

    #[test]
    fn read_only_false_does_not_override_config() {
        // If the user does NOT pass --read-only, the config value is preserved.
        let cmd = parse_serve(&[]);
        let mut config = polkagent_config::Config::default();
        config.api.read_only = false;
        apply_cli_overrides(&mut config, &cmd);
        assert!(!config.api.read_only);
    }

    // -----------------------------------------------------------------------
    // --cors-origin flag
    // -----------------------------------------------------------------------

    #[test]
    fn parse_single_cors_origin() {
        let cmd = parse_serve(&["--cors-origin", "https://example.com"]);
        assert_eq!(cmd.cors_origins, vec!["https://example.com"]);
    }

    #[test]
    fn parse_multiple_cors_origins() {
        let cmd = parse_serve(&[
            "--cors-origin",
            "https://a.com",
            "--cors-origin",
            "https://b.com",
        ]);
        assert_eq!(cmd.cors_origins, vec!["https://a.com", "https://b.com"]);
    }

    #[test]
    fn cors_origins_override_config() {
        let cmd = parse_serve(&["--cors-origin", "https://override.com"]);
        let mut config = polkagent_config::Config::default();
        // Default config has cors_origins = ["http://localhost:*"]
        assert_eq!(config.api.cors_origins, vec!["http://localhost:*"]);
        apply_cli_overrides(&mut config, &cmd);
        assert_eq!(config.api.cors_origins, vec!["https://override.com"]);
    }

    #[test]
    fn empty_cors_origins_preserves_config() {
        let cmd = parse_serve(&[]);
        let mut config = polkagent_config::Config::default();
        let original = config.api.cors_origins.clone();
        apply_cli_overrides(&mut config, &cmd);
        assert_eq!(config.api.cors_origins, original);
    }

    // -----------------------------------------------------------------------
    // Combined flags
    // -----------------------------------------------------------------------

    #[test]
    fn parse_all_flags_together() {
        let cmd = parse_serve(&[
            "--host",
            "192.168.1.1",
            "--port",
            "5555",
            "--cors-origin",
            "https://x.com",
            "--read-only",
        ]);
        assert_eq!(cmd.host, "192.168.1.1");
        assert_eq!(cmd.port, 5555);
        assert_eq!(cmd.cors_origins, vec!["https://x.com"]);
        assert!(cmd.read_only);
        let addr = build_bind_addr(&cmd);
        assert_eq!(addr, "192.168.1.1:5555");
    }
}

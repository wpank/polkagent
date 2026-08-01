//! `polkagent serve` — start the HTTP API server (daemon mode).
//!
//! Reads the active configuration for the bind address, CORS settings, and
//! database path, then starts the Axum-backed polkagent-api server. Handles
//! SIGTERM and SIGINT for graceful shutdown.
//!
//! # Flags
//!
//! | Flag       | Description                                     |
//! |------------|-------------------------------------------------|
//! | `--port`   | Override the TCP port from config (default 4840)|
//! | `--host`   | Override the bind host from config              |

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
    let config = resolve_config(cmd.config.as_deref())?;

    // -----------------------------------------------------------------------
    // 2. Determine bind address (CLI flags override config).
    // -----------------------------------------------------------------------
    let bind_addr = build_bind_addr(cmd, &config.api.bind_address);

    // -----------------------------------------------------------------------
    // 3. Open (or create) the SQLite database and run migrations.
    // -----------------------------------------------------------------------
    let db_path = resolve_db_path(cmd.config.as_deref());
    let pool = open_pool(&db_path)?;

    // -----------------------------------------------------------------------
    // 4. Construct stores.
    // -----------------------------------------------------------------------
    // EffectStore is implemented on SqlitePool directly; use Arc<SqlitePool>.
    let effect_store = Arc::new(pool.clone());

    // The AgentStore and RunManager are in-memory for now; a future task will
    // wire in the durable SQLite-backed implementations.
    let agent_store = Arc::new(InMemoryAgentStore::new());
    let run_manager = Arc::new(InMemoryRunManager::new());
    let event_bus = EventBus::with_default_capacity();

    // -----------------------------------------------------------------------
    // 5. Build and start the server.
    // -----------------------------------------------------------------------
    let server = ApiServer::new(
        config,
        agent_store,
        run_manager,
        effect_store as Arc<dyn EffectStore>,
        event_bus,
    );

    println!("Polkagent API server");
    println!("{}", "-".repeat(40));
    println!("  Listening on http://{bind_addr}");
    println!("  Database:  {db_path}");
    println!("  Press Ctrl+C to stop.");
    println!();

    info!(bind_addr = %bind_addr, db_path = %db_path, "polkagent serve starting");

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

/// Resolve the active polkagent configuration, applying CLI-level overrides.
///
/// Resolution order:
/// 1. `--config` flag path.
/// 2. Project-local `.polkagent/polkagent.toml`.
/// 3. User-global `~/.config/polkagent/polkagent.toml`.
/// 4. Default `Config::default()`.
fn resolve_config(config_override: Option<&str>) -> Result<polkagent_config::Config> {
    if let Some(path) = config_override {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading config file {path}"))?;
        return toml::from_str(&content)
            .with_context(|| format!("parsing config file {path}"));
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

/// Build the `host:port` bind address, applying `--host` and `--port` overrides.
fn build_bind_addr(cmd: &ServeCmd, config_bind: &str) -> String {
    // Parse the configured address, then apply overrides.
    let (config_host, config_port) = parse_host_port(config_bind);

    let host = cmd.host.as_deref().unwrap_or(&config_host);
    let port = cmd.port.unwrap_or(config_port);

    format!("{host}:{port}")
}

/// Split a `host:port` string into its components.
fn parse_host_port(addr: &str) -> (String, u16) {
    if let Some(colon_pos) = addr.rfind(':') {
        let host = &addr[..colon_pos];
        let port: u16 = addr[colon_pos + 1..].parse().unwrap_or(4840);
        (host.to_owned(), port)
    } else {
        (addr.to_owned(), 4840)
    }
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

    let pool = SqlitePool::open(&expanded)
        .with_context(|| format!("opening database at {expanded}"))?;

    {
        let writer = pool.writer();
        migrations::migrate(&writer)
            .context("running database migrations")?;
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

    #[test]
    fn parse_host_port_standard() {
        let (host, port) = parse_host_port("127.0.0.1:4840");
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 4840);
    }

    #[test]
    fn parse_host_port_ipv6() {
        let (host, port) = parse_host_port("::1:9090");
        // rfind(':') finds the last colon → port=9090, host=::1
        assert_eq!(port, 9090);
        assert!(!host.is_empty());
    }

    #[test]
    fn parse_host_port_no_port() {
        let (host, port) = parse_host_port("0.0.0.0");
        assert_eq!(host, "0.0.0.0");
        assert_eq!(port, 4840); // default
    }

    #[test]
    fn build_bind_addr_no_overrides() {
        let cmd = crate::cli::ServeCmd {
            port: None,
            host: None,
            config: None,
        };
        let addr = build_bind_addr(&cmd, "127.0.0.1:4840");
        assert_eq!(addr, "127.0.0.1:4840");
    }

    #[test]
    fn build_bind_addr_port_override() {
        let cmd = crate::cli::ServeCmd {
            port: Some(9999),
            host: None,
            config: None,
        };
        let addr = build_bind_addr(&cmd, "127.0.0.1:4840");
        assert_eq!(addr, "127.0.0.1:9999");
    }

    #[test]
    fn build_bind_addr_host_override() {
        let cmd = crate::cli::ServeCmd {
            port: None,
            host: Some("0.0.0.0".to_owned()),
            config: None,
        };
        let addr = build_bind_addr(&cmd, "127.0.0.1:4840");
        assert_eq!(addr, "0.0.0.0:4840");
    }

    #[test]
    fn build_bind_addr_both_overrides() {
        let cmd = crate::cli::ServeCmd {
            port: Some(8080),
            host: Some("0.0.0.0".to_owned()),
            config: None,
        };
        let addr = build_bind_addr(&cmd, "127.0.0.1:4840");
        assert_eq!(addr, "0.0.0.0:8080");
    }
}

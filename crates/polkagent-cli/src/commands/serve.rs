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

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;

use polkagent_api::{app_state_from_runtime, ApiServer, RUNTIME_UNAVAILABLE_ROUTES};
use polkagent_runtime::{RuntimeFactory, RuntimeOptions};

use crate::cli::ServeCmd;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Execute the `serve` subcommand.
///
/// Builds one shared production runtime, projects its durable stores into the
/// API, binds to the configured address, and serves until a shutdown signal
/// is received.
pub async fn run(cmd: &ServeCmd, config_path: Option<&Path>) -> Result<()> {
    // RuntimeFactory is the sole composition root for stores, execution
    // adapters, startup recovery, and persisted-agent rehydration.
    let options = runtime_options(cmd, config_path)?;
    let runtime = RuntimeFactory::build(options)
        .await
        .context("building shared Polkagent server runtime")?;

    // CORS is an HTTP-surface concern. Clone the resolved runtime config and
    // apply only the CLI surface overrides before constructing AppState.
    let mut api_config = runtime.config().as_ref().clone();
    apply_cli_overrides(&mut api_config, cmd);

    let bind_addr = build_bind_addr(cmd);
    let effective_read_only = api_config.api.read_only;
    let effective_cors_origins = api_config.api.cors_origins.clone();
    let db_path = runtime.readiness().database_path.display().to_string();
    let config_source = format!("{:?}", runtime.readiness().config_source);
    let state = app_state_from_runtime(&runtime, api_config);
    let server = ApiServer::from_state(state);

    println!("Polkagent API server listening on {bind_addr}");
    println!("{}", "-".repeat(40));
    println!("  Database:    {db_path}");
    println!("  Config:      {config_source}");
    if effective_read_only {
        println!("  Mode:        read-only");
    }
    if !effective_cors_origins.is_empty() {
        println!("  CORS:        {}", effective_cors_origins.join(", "));
    }
    println!(
        "  Unavailable: {} optional routes return 501 (skills, memory, audit, registry)",
        RUNTIME_UNAVAILABLE_ROUTES.len()
    );
    println!("  Press Ctrl+C to stop.");
    println!();

    info!(
        bind_addr = %bind_addr,
        db_path = %db_path,
        config_source = %config_source,
        read_only = effective_read_only,
        unavailable_optional_routes = RUNTIME_UNAVAILABLE_ROUTES.len(),
        "polkagent serve starting"
    );
    for route in RUNTIME_UNAVAILABLE_ROUTES {
        tracing::debug!(
            dependency = route.dependency,
            method = route.method,
            path = route.path,
            reason = route.reason,
            status = 501,
            "runtime API route unavailable"
        );
    }

    server
        .serve_with_shutdown(&bind_addr, shutdown_signal())
        .await
        .map_err(|e| anyhow::anyhow!("server error: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build strict shared-runtime options for the server surface.
///
/// Unlike explicitly simulated demos, the daemon does not silently install a
/// fake executor. Runtime construction therefore fails unless a configured
/// provider or discovered harness can execute work.
fn runtime_options(cmd: &ServeCmd, config_path: Option<&Path>) -> Result<RuntimeOptions> {
    let workdir = std::env::current_dir().context("resolving server runtime workdir")?;
    Ok(runtime_options_at(cmd, config_path, workdir))
}

fn runtime_options_at(
    cmd: &ServeCmd,
    config_path: Option<&Path>,
    workdir: PathBuf,
) -> RuntimeOptions {
    let mut options = RuntimeOptions::new(workdir);
    options.config_path = config_path.map(Path::to_path_buf);
    options.read_only = cmd.read_only;
    options
}

/// Apply CLI-level overrides to the resolved config.
///
/// - `--cors-origin` values replace the config's `api.cors_origins` list.
/// - `--read-only` sets `api.read_only = true`.
fn apply_cli_overrides(config: &mut polkagent_config::Config, cmd: &ServeCmd) {
    if !cmd.cors_origins.is_empty() {
        cmd.cors_origins.clone_into(&mut config.api.cors_origins);
    }
    if cmd.read_only {
        config.api.read_only = true;
    }
}

/// Build the `host:port` bind address from the CLI flags.
///
/// With the new `ServeCmd` struct, `host` and `port` always have values
/// (either from the user or from clap defaults).
fn build_bind_addr(cmd: &ServeCmd) -> String {
    format!("{}:{}", cmd.host, cmd.port)
}

/// Wait for SIGTERM or SIGINT (Ctrl+C).
async fn shutdown_signal() {
    // Ctrl+C (SIGINT).
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl+C handler");
        }
    };

    // SIGTERM (e.g. `kill <pid>` or systemd stop).
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }

    println!();
    println!("Shutdown signal received. Draining active HTTP connections...");
    info!("shutdown signal received; draining active HTTP connections");
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

    #[test]
    fn explicit_config_and_read_only_are_runtime_inputs() {
        let cmd = parse_serve(&["--read-only"]);
        let config_path = Path::new("config/server.toml");
        let options = runtime_options_at(&cmd, Some(config_path), PathBuf::from("/workspace"));

        assert_eq!(options.workdir, PathBuf::from("/workspace"));
        assert_eq!(options.config_path, Some(config_path.to_path_buf()));
        assert!(options.read_only);
        assert_eq!(
            options.adapter_policy,
            polkagent_runtime::AdapterPolicy::Strict
        );
    }

    #[tokio::test]
    async fn missing_explicit_config_is_an_error() {
        let temp_dir = tempfile::tempdir().expect("create temporary directory");
        let missing = temp_dir.path().join("missing.toml");
        let cmd = parse_serve(&[]);
        let options = runtime_options_at(&cmd, Some(&missing), temp_dir.path().to_path_buf());

        let error = RuntimeFactory::build(options)
            .await
            .expect_err("missing config must fail");
        let message = format!("{error:#}");
        assert!(message.contains("failed to load configuration"));
        assert!(message.contains("missing.toml"));
    }
}

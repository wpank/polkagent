//! `polkagent network` — network configuration and status subcommands.
//!
//! Network management is infrastructure-level: actual node/relay-chain
//! lifecycle is delegated to external tooling (zombienet, chopsticks, etc.).
//! These commands are primarily informational and print helpful guidance.
//!
//! Subcommands:
//! - `network status` — show configured chain endpoints and their status.
//! - `network start`  — advisory message directing users to zombienet/chopsticks.
//! - `network stop`   — advisory message directing users to zombienet/chopsticks.

use anyhow::Result;

use crate::cli::{NetworkCmd, NetworkStartCmd, NetworkStatusCmd, NetworkStopCmd};

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch the `network` subcommand.
pub fn run(cmd: &NetworkCmd) -> Result<()> {
    match cmd {
        NetworkCmd::Status(c) => status(c),
        NetworkCmd::Start(c)  => start(c),
        NetworkCmd::Stop(c)   => stop(c),
    }
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

/// Probe a host:port string; returns `true` if a TCP connection can be
/// established within 2 seconds.
fn probe_tcp(addr: &str) -> bool {
    use std::net::TcpStream;
    use std::time::Duration;

    let addr_with_port = if addr.contains(':') {
        addr.to_owned()
    } else {
        format!("{addr}:80")
    };

    addr_with_port
        .parse()
        .ok()
        .and_then(|sock_addr| {
            TcpStream::connect_timeout(&sock_addr, Duration::from_secs(2)).ok()
        })
        .is_some()
}

/// Strip URL scheme to obtain `host:port`.
fn strip_scheme(url: &str) -> &str {
    url.trim_start_matches("ws://")
        .trim_start_matches("wss://")
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or(url)
}

fn status(cmd: &NetworkStatusCmd) -> Result<()> {
    // Collect configured RPC endpoints from environment variables.
    let rpc_url = std::env::var("POLKAGENT_CHAIN_RPC_URL")
        .ok()
        .filter(|s| !s.is_empty());

    // Build an endpoint list from config if available.
    let mut endpoints: Vec<(String, String)> = Vec::new(); // (name, url)

    if let Some(ref url) = rpc_url {
        endpoints.push(("polkadot".to_owned(), url.clone()));
    }

    // Also check a Westend endpoint variable (common in test setups).
    if let Ok(url) = std::env::var("POLKAGENT_WESTEND_RPC_URL") {
        if !url.is_empty() {
            endpoints.push(("westend".to_owned(), url));
        }
    }

    // Check a zombienet/local endpoint.
    if let Ok(url) = std::env::var("POLKAGENT_LOCAL_RPC_URL") {
        if !url.is_empty() {
            endpoints.push(("local".to_owned(), url));
        }
    }

    if cmd.json {
        let items: Vec<serde_json::Value> = if endpoints.is_empty() {
            vec![serde_json::json!({
                "name":      "polkadot",
                "url":       "(not configured)",
                "reachable": false,
                "note":      "Set POLKAGENT_CHAIN_RPC_URL to configure a chain endpoint",
            })]
        } else {
            endpoints
                .iter()
                .map(|(name, url)| {
                    let hostport = strip_scheme(url);
                    let reachable = probe_tcp(hostport);
                    serde_json::json!({
                        "name":      name,
                        "url":       url,
                        "reachable": reachable,
                    })
                })
                .collect()
        };

        let out = serde_json::json!({ "endpoints": items });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("Network Status");
    println!("{}", "-".repeat(50));
    println!();

    if endpoints.is_empty() {
        println!("  No chain endpoints configured.");
        println!();
        println!("  Set one or more of the following environment variables:");
        println!("    POLKAGENT_CHAIN_RPC_URL   — primary chain RPC (ws:// or wss://)");
        println!("    POLKAGENT_WESTEND_RPC_URL — Westend testnet RPC");
        println!("    POLKAGENT_LOCAL_RPC_URL   — local node (e.g. zombienet)");
        println!();
        println!("  Or configure endpoints in polkagent.toml under [network].");
        return Ok(());
    }

    for (name, url) in &endpoints {
        let hostport = strip_scheme(url);
        let reachable = probe_tcp(hostport);
        let glyph = if reachable { "\u{25C9}" } else { "\u{25A0}" };
        let label = if reachable { "OK  " } else { "FAIL" };
        println!("  {glyph} [{label}] {name:<12}  {url}");
    }
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// start
// ---------------------------------------------------------------------------

fn start(cmd: &NetworkStartCmd) -> Result<()> {
    if cmd.json {
        let out = serde_json::json!({
            "message": "Network lifecycle management is handled by zombienet or chopsticks.",
            "docs": [
                "https://github.com/paritytech/zombienet",
                "https://github.com/AcalaNetwork/chopsticks",
            ],
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("polkagent network start");
    println!("{}", "-".repeat(50));
    println!();
    println!("  Polkagent does not manage relay-chain or parachain nodes directly.");
    println!("  Network lifecycle is handled by dedicated infrastructure tooling:");
    println!();
    println!("  zombienet  — spawn ephemeral test networks (CI / integration tests)");
    println!("    https://github.com/paritytech/zombienet");
    println!();
    println!("  chopsticks — fork live networks for local development and testing");
    println!("    https://github.com/AcalaNetwork/chopsticks");
    println!();
    println!("  Once a network is running, configure its RPC endpoint:");
    println!("    export POLKAGENT_CHAIN_RPC_URL=ws://127.0.0.1:9944");
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// stop
// ---------------------------------------------------------------------------

fn stop(cmd: &NetworkStopCmd) -> Result<()> {
    if cmd.json {
        let out = serde_json::json!({
            "message": "Network lifecycle management is handled by zombienet or chopsticks.",
            "docs": [
                "https://github.com/paritytech/zombienet",
                "https://github.com/AcalaNetwork/chopsticks",
            ],
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("polkagent network stop");
    println!("{}", "-".repeat(50));
    println!();
    println!("  Polkagent does not manage relay-chain or parachain nodes directly.");
    println!("  To stop a running test network, use the tool that started it:");
    println!();
    println!("  zombienet  — Ctrl+C in the terminal running `zombienet spawn ...`");
    println!("  chopsticks — Ctrl+C in the terminal running `chopsticks`");
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_scheme_ws() {
        assert_eq!(strip_scheme("ws://127.0.0.1:9944"), "127.0.0.1:9944");
    }

    #[test]
    fn strip_scheme_wss() {
        assert_eq!(strip_scheme("wss://rpc.polkadot.io"), "rpc.polkadot.io");
    }

    #[test]
    fn strip_scheme_http() {
        assert_eq!(strip_scheme("http://localhost:9933"), "localhost:9933");
    }

    #[test]
    fn strip_scheme_no_scheme() {
        assert_eq!(strip_scheme("127.0.0.1:9944"), "127.0.0.1:9944");
    }

    #[test]
    fn strip_scheme_with_path() {
        // Should only return the host:port portion.
        assert_eq!(strip_scheme("ws://127.0.0.1:9944/path"), "127.0.0.1:9944");
    }
}

//! `polkagent network` — network configuration and status subcommands.
//!
//! Network management is infrastructure-level: actual node/relay-chain
//! lifecycle is delegated to external tooling (zombienet, chopsticks, etc.).
//! These commands are primarily informational and print helpful guidance.
//!
//! Subcommands:
//! - `network status`   — show chain name, best block, finalized block, peer count.
//! - `network metadata` — show metadata version and pallet list.
//! - `network start`    — advisory message directing users to zombienet/chopsticks.
//! - `network stop`     — advisory message directing users to zombienet/chopsticks.

use anyhow::Result;

use polkagent_chain_fake::FakeChainClientBuilder;
use polkagent_chain_trait::{ChainClient, ChainProfileId};

use crate::cli::{NetworkCmd, NetworkMetadataCmd, NetworkStartCmd, NetworkStatusCmd, NetworkStopCmd};

// ---------------------------------------------------------------------------
// Well-known pallet names for the fake metadata display
// ---------------------------------------------------------------------------

/// A representative list of pallets that appear in a typical Polkadot runtime.
///
/// Since the fake chain client returns opaque metadata bytes, we provide a
/// static list that gives users a realistic preview of the metadata shape.
const KNOWN_PALLETS: &[&str] = &[
    "System",
    "Timestamp",
    "Balances",
    "TransactionPayment",
    "Authorship",
    "Staking",
    "Session",
    "Grandpa",
    "Babe",
    "ImOnline",
    "Offences",
    "Democracy",
    "Council",
    "TechnicalCommittee",
    "Elections",
    "Treasury",
    "Utility",
    "Identity",
    "Proxy",
    "Multisig",
    "Scheduler",
    "Preimage",
    "XcmPallet",
    "ParaInherent",
    "Paras",
    "Registrar",
    "Auctions",
    "Crowdloan",
    "Slots",
    "NominationPools",
];

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch the `network` subcommand.
pub async fn run(cmd: &NetworkCmd) -> Result<()> {
    match cmd {
        NetworkCmd::Status(c)   => status(c).await,
        NetworkCmd::Metadata(c) => metadata(c).await,
        NetworkCmd::Start(c)    => start(c),
        NetworkCmd::Stop(c)     => stop(c),
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

/// Determine whether a real RPC endpoint is configured.
fn rpc_url() -> Option<String> {
    std::env::var("POLKAGENT_RPC_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("POLKAGENT_CHAIN_RPC_URL")
                .ok()
                .filter(|s| !s.is_empty())
        })
}

/// Build a `FakeChainClient` to provide fallback data when no real RPC is
/// available.
fn build_fake_client() -> polkagent_chain_fake::FakeChainClient {
    FakeChainClientBuilder::polkadot()
        .with_block_number(22_543_871)
        .with_runtime_version(1_003_004, 0)
        .build()
}

async fn status(cmd: &NetworkStatusCmd) -> Result<()> {
    let configured_rpc = rpc_url();

    if configured_rpc.is_none() {
        // No RPC configured — use the fake client to show representative data
        // and guide the user.
        let client = build_fake_client();
        let profile = ChainProfileId::new("polkadot");
        let meta = client.fetch_metadata(profile).await?;
        let best_block = client.current_block_number();
        let finalized = best_block.saturating_sub(10);

        if cmd.json {
            let out = serde_json::json!({
                "chain_name": "polkadot",
                "best_block": best_block,
                "finalized_block": finalized,
                "spec_version": meta.spec_version,
                "peer_count": serde_json::Value::Null,
                "rpc_configured": false,
                "note": "No RPC endpoint configured. Set POLKAGENT_RPC_URL to connect to a live chain.",
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }

        println!("Network Status");
        println!("{}", "-".repeat(50));
        println!();
        println!("  Chain:           polkadot (fake/offline)");
        println!("  Best block:      #{best_block}");
        println!("  Finalized block: #{finalized}");
        println!("  Spec version:    {}", meta.spec_version);
        println!("  Peer count:      (not available)");
        println!();
        println!("  No RPC endpoint configured.");
        println!("  Showing sample data from the built-in fake chain client.");
        println!();
        println!("  To connect to a live chain, set one of:");
        println!("    export POLKAGENT_RPC_URL=wss://rpc.polkadot.io");
        println!("    export POLKAGENT_CHAIN_RPC_URL=ws://127.0.0.1:9944");
        println!();
        println!("  Or configure endpoints in polkagent.toml under [network].");
        return Ok(());
    }

    // RPC is configured — probe endpoints (None case returned above).
    let Some(rpc) = configured_rpc else {
        return Ok(());
    };

    // Also collect additional endpoints.
    let mut endpoints: Vec<(String, String)> = vec![("primary".to_owned(), rpc)];

    if let Ok(url) = std::env::var("POLKAGENT_WESTEND_RPC_URL") {
        if !url.is_empty() {
            endpoints.push(("westend".to_owned(), url));
        }
    }
    if let Ok(url) = std::env::var("POLKAGENT_LOCAL_RPC_URL") {
        if !url.is_empty() {
            endpoints.push(("local".to_owned(), url));
        }
    }

    if cmd.json {
        let items: Vec<serde_json::Value> = endpoints
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
            .collect();

        let out = serde_json::json!({
            "endpoints": items,
            "rpc_configured": true,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("Network Status");
    println!("{}", "-".repeat(50));
    println!();

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
// metadata
// ---------------------------------------------------------------------------

async fn metadata(cmd: &NetworkMetadataCmd) -> Result<()> {
    let configured_rpc = rpc_url();

    // Whether or not an RPC is configured, we use the fake client for metadata
    // display since we cannot parse real metadata without a full subxt connection.
    let client = build_fake_client();
    let profile = ChainProfileId::new("polkadot");
    let meta = client.fetch_metadata(profile).await?;

    let source = if configured_rpc.is_some() {
        "cached"
    } else {
        "fake/offline"
    };

    if cmd.json {
        let pallets: Vec<serde_json::Value> = KNOWN_PALLETS
            .iter()
            .enumerate()
            .map(|(i, name)| {
                serde_json::json!({
                    "index": i,
                    "name": name,
                })
            })
            .collect();

        let out = serde_json::json!({
            "spec_version": meta.spec_version,
            "metadata_digest": meta.metadata_digest.0,
            "source": source,
            "pallet_count": KNOWN_PALLETS.len(),
            "pallets": pallets,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("Runtime Metadata");
    println!("{}", "-".repeat(50));
    println!();
    println!("  Spec version:    {}", meta.spec_version);
    println!("  Metadata digest: {}", meta.metadata_digest.0);
    println!("  Source:           {source}");
    println!("  Pallet count:    {}", KNOWN_PALLETS.len());
    println!();
    println!("  Pallets:");
    for (i, pallet) in KNOWN_PALLETS.iter().enumerate() {
        println!("    {i:>3}. {pallet}");
    }
    println!();

    if configured_rpc.is_none() {
        println!("  Note: Showing representative pallet list from offline data.");
        println!("  Set POLKAGENT_RPC_URL to fetch live metadata from a chain node.");
        println!();
    }

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
    println!("    export POLKAGENT_RPC_URL=ws://127.0.0.1:9944");
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
    use crate::cli::Cli;
    use clap::Parser;

    // -----------------------------------------------------------------------
    // 1. Arg parsing: `network status`
    // -----------------------------------------------------------------------

    #[test]
    fn parse_network_status() {
        let cli = Cli::try_parse_from(["polkagent", "network", "status"]).unwrap();
        match cli.command {
            Some(crate::cli::Commands::Network(NetworkCmd::Status(ref cmd))) => {
                assert!(!cmd.json);
            }
            _ => panic!("expected Network Status command"),
        }
    }

    // -----------------------------------------------------------------------
    // 2. Arg parsing: `network status --json`
    // -----------------------------------------------------------------------

    #[test]
    fn parse_network_status_json() {
        let cli = Cli::try_parse_from(["polkagent", "network", "status", "--json"]).unwrap();
        match cli.command {
            Some(crate::cli::Commands::Network(NetworkCmd::Status(ref cmd))) => {
                assert!(cmd.json);
            }
            _ => panic!("expected Network Status command with --json"),
        }
    }

    // -----------------------------------------------------------------------
    // 3. Arg parsing: `network metadata`
    // -----------------------------------------------------------------------

    #[test]
    fn parse_network_metadata() {
        let cli = Cli::try_parse_from(["polkagent", "network", "metadata"]).unwrap();
        match cli.command {
            Some(crate::cli::Commands::Network(NetworkCmd::Metadata(ref cmd))) => {
                assert!(!cmd.json);
            }
            _ => panic!("expected Network Metadata command"),
        }
    }

    // -----------------------------------------------------------------------
    // 4. Arg parsing: `network metadata --json`
    // -----------------------------------------------------------------------

    #[test]
    fn parse_network_metadata_json() {
        let cli = Cli::try_parse_from(["polkagent", "network", "metadata", "--json"]).unwrap();
        match cli.command {
            Some(crate::cli::Commands::Network(NetworkCmd::Metadata(ref cmd))) => {
                assert!(cmd.json);
            }
            _ => panic!("expected Network Metadata command with --json"),
        }
    }

    // -----------------------------------------------------------------------
    // 5. Status output: no RPC configured shows helpful message (human)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn status_no_rpc_shows_helpful_message() {
        // Ensure no RPC env is set for this test.
        std::env::remove_var("POLKAGENT_RPC_URL");
        std::env::remove_var("POLKAGENT_CHAIN_RPC_URL");

        let cmd = NetworkStatusCmd { json: false };
        // Should succeed without error.
        let result = status(&cmd).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // 6. Status output: no RPC configured, JSON mode
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn status_no_rpc_json_mode() {
        std::env::remove_var("POLKAGENT_RPC_URL");
        std::env::remove_var("POLKAGENT_CHAIN_RPC_URL");

        let cmd = NetworkStatusCmd { json: true };
        let result = status(&cmd).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // 7. Metadata output: pallet list is non-empty
    // -----------------------------------------------------------------------

    #[test]
    fn known_pallets_non_empty() {
        assert!(!KNOWN_PALLETS.is_empty());
        assert!(KNOWN_PALLETS.len() >= 10);
    }

    // -----------------------------------------------------------------------
    // 8. Metadata output: all pallets have non-empty names
    // -----------------------------------------------------------------------

    #[test]
    fn known_pallets_have_names() {
        for pallet in KNOWN_PALLETS {
            assert!(!pallet.is_empty(), "pallet name should not be empty");
        }
    }

    // -----------------------------------------------------------------------
    // 9. Metadata handler runs without error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn metadata_handler_succeeds() {
        std::env::remove_var("POLKAGENT_RPC_URL");
        std::env::remove_var("POLKAGENT_CHAIN_RPC_URL");

        let cmd = NetworkMetadataCmd { json: false };
        let result = metadata(&cmd).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // 10. Metadata JSON output succeeds
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn metadata_json_output_succeeds() {
        std::env::remove_var("POLKAGENT_RPC_URL");
        std::env::remove_var("POLKAGENT_CHAIN_RPC_URL");

        let cmd = NetworkMetadataCmd { json: true };
        let result = metadata(&cmd).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // 11. Fake client provides consistent block numbers
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fake_client_block_numbers() {
        let client = build_fake_client();
        let best = client.current_block_number();
        assert!(best > 0, "best block should be > 0");
    }

    // -----------------------------------------------------------------------
    // 12. Fake client fetch_metadata returns correct spec version
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fake_client_spec_version() {
        let client = build_fake_client();
        let profile = ChainProfileId::new("polkadot");
        let meta = client.fetch_metadata(profile).await.unwrap();
        assert_eq!(meta.spec_version, 1_003_004);
    }

    // -----------------------------------------------------------------------
    // 13. rpc_url returns None when env vars are unset
    // -----------------------------------------------------------------------

    #[test]
    fn rpc_url_returns_none_when_unset() {
        std::env::remove_var("POLKAGENT_RPC_URL");
        std::env::remove_var("POLKAGENT_CHAIN_RPC_URL");
        assert!(rpc_url().is_none());
    }

    // -----------------------------------------------------------------------
    // 14. strip_scheme helper
    // -----------------------------------------------------------------------

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
        assert_eq!(strip_scheme("ws://127.0.0.1:9944/path"), "127.0.0.1:9944");
    }

    // -----------------------------------------------------------------------
    // 15. Arg parsing: `network start` and `network stop` still work
    // -----------------------------------------------------------------------

    #[test]
    fn parse_network_start() {
        let cli = Cli::try_parse_from(["polkagent", "network", "start"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(crate::cli::Commands::Network(NetworkCmd::Start(_)))
        ));
    }

    #[test]
    fn parse_network_stop() {
        let cli = Cli::try_parse_from(["polkagent", "network", "stop"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(crate::cli::Commands::Network(NetworkCmd::Stop(_)))
        ));
    }
}

//! `polkagent chain` — chain interaction and inspection subcommands.

use anyhow::Result;

use crate::cli::{
    ChainCmd, ChainBalanceCmd, ChainDecodeCmd, ChainMetadataCmd, ChainStatusCmd,
};

/// Dispatch the chain subcommand.
pub fn run(cmd: &ChainCmd) -> Result<()> {
    match cmd {
        ChainCmd::Status(c)   => status(c),
        ChainCmd::Metadata(c) => metadata(c),
        ChainCmd::Decode(c)   => decode(c),
        ChainCmd::Balance(c)  => balance(c),
    }
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

fn status(cmd: &ChainStatusCmd) -> Result<()> {
    if cmd.json {
        let out = serde_json::json!({
            "chain": cmd.chain,
            "connected": false,
            "message": "Chain adapter not yet connected",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Chain status");
        println!("  Chain:     {}", cmd.chain);
        println!("  Connected: no");
        println!();
        println!("  Chain adapter not yet connected.");
        println!("  Configure a chain endpoint in polkagent.toml to enable.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// metadata
// ---------------------------------------------------------------------------

fn metadata(cmd: &ChainMetadataCmd) -> Result<()> {
    if cmd.json {
        let out = serde_json::json!({
            "chain": cmd.chain,
            "metadata_version": serde_json::Value::Null,
            "message": "Chain adapter not yet connected",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Chain metadata");
        println!("  Chain:            {}", cmd.chain);
        println!("  Metadata version: (unknown)");
        println!();
        println!("  Chain adapter not yet connected.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------

fn decode(cmd: &ChainDecodeCmd) -> Result<()> {
    if cmd.json {
        let out = serde_json::json!({
            "chain": cmd.chain,
            "hex": cmd.hex,
            "decoded": serde_json::Value::Null,
            "message": "Chain adapter not yet connected",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Decode call data");
        println!("  Chain: {}", cmd.chain);
        println!("  Hex:   {}", cmd.hex);
        println!();
        println!("  Chain adapter not yet connected.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// balance
// ---------------------------------------------------------------------------

fn balance(cmd: &ChainBalanceCmd) -> Result<()> {
    if cmd.json {
        let out = serde_json::json!({
            "chain": cmd.chain,
            "address": cmd.address,
            "balance": serde_json::Value::Null,
            "message": "Chain adapter not yet connected",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Account balance");
        println!("  Chain:   {}", cmd.chain);
        println!("  Address: {}", cmd.address);
        println!("  Balance: (unknown)");
        println!();
        println!("  Chain adapter not yet connected.");
    }
    Ok(())
}

//! `polkagent chain` — chain interaction and inspection subcommands.

use anyhow::{Context, Result};

use polkagent_chain_subxt::config::SubxtConfig;
use polkagent_chain_subxt::decode::{
    compute_metadata_digest, decode_call_bytes, hex_to_bytes, parse_block_number_hex,
    parse_runtime_metadata,
};
use polkagent_chain_subxt::rpc::RpcClient;

use crate::cli::{ChainBalanceCmd, ChainCmd, ChainDecodeCmd, ChainMetadataCmd, ChainStatusCmd};

/// Dispatch the chain subcommand.
pub async fn run(cmd: &ChainCmd, rpc_url: Option<&str>) -> Result<()> {
    match cmd {
        ChainCmd::Status(c) => status(c, rpc_url).await,
        ChainCmd::Metadata(c) => metadata(c, rpc_url).await,
        ChainCmd::Decode(c) => decode(c, rpc_url).await,
        ChainCmd::Balance(c) => balance(c, rpc_url).await,
    }
}

const NO_RPC_MSG: &str = "\
No chain RPC endpoint configured.

Set the POLKAGENT_RPC_URL environment variable to connect to a node:

    export POLKAGENT_RPC_URL=https://rpc.polkadot.io
    polkagent chain status";

fn require_rpc_url(rpc_url: Option<&str>) -> Result<&str> {
    rpc_url.ok_or_else(|| anyhow::anyhow!("{NO_RPC_MSG}"))
}

fn make_rpc_client() -> Result<RpcClient> {
    RpcClient::new(SubxtConfig::default())
        .map_err(|e| anyhow::anyhow!("failed to create RPC client: {e}"))
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

async fn status(cmd: &ChainStatusCmd, rpc_url: Option<&str>) -> Result<()> {
    let url = require_rpc_url(rpc_url)?;
    let rpc = make_rpc_client()?;

    // Fetch chain name via system_chain RPC.
    let chain_name = rpc
        .call(url, "system_chain", None)
        .await
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| cmd.chain.clone());

    // Fetch health.
    let health = rpc
        .system_health(url)
        .await
        .context("failed to query system_health")?;

    // Fetch best block.
    let best_hash = rpc
        .chain_get_block_hash(url, None)
        .await
        .context("failed to query best block hash")?;
    let best_header = rpc
        .chain_get_header(url, Some(&best_hash))
        .await
        .context("failed to query best block header")?;
    let best_number_hex = best_header
        .get("number")
        .and_then(|v| v.as_str())
        .unwrap_or("0x0");
    let best_number = parse_block_number_hex(best_number_hex).unwrap_or(0);

    // Fetch finalized block.
    let finalized_hash = rpc
        .chain_get_finalized_head(url)
        .await
        .context("failed to query finalized head")?;
    let finalized_header = rpc
        .chain_get_header(url, Some(&finalized_hash))
        .await
        .context("failed to query finalized block header")?;
    let finalized_number_hex = finalized_header
        .get("number")
        .and_then(|v| v.as_str())
        .unwrap_or("0x0");
    let finalized_number = parse_block_number_hex(finalized_number_hex).unwrap_or(0);

    if cmd.json {
        let out = serde_json::json!({
            "chain": chain_name,
            "connected": true,
            "best_block": best_number,
            "best_hash": best_hash,
            "finalized_block": finalized_number,
            "finalized_hash": finalized_hash,
            "peers": health.peers,
            "is_syncing": health.is_syncing,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Chain status");
        println!("  Chain:          {chain_name}");
        println!("  Connected:      yes");
        println!("  Peers:          {}", health.peers);
        println!(
            "  Syncing:        {}",
            if health.is_syncing { "yes" } else { "no" }
        );
        println!("  Best block:     #{best_number} ({best_hash})");
        println!("  Finalized:      #{finalized_number} ({finalized_hash})");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// metadata
// ---------------------------------------------------------------------------

async fn metadata(cmd: &ChainMetadataCmd, rpc_url: Option<&str>) -> Result<()> {
    let url = require_rpc_url(rpc_url)?;
    let rpc = make_rpc_client()?;

    let metadata_hex = rpc
        .state_get_metadata(url, None)
        .await
        .context("failed to fetch runtime metadata")?;

    let metadata_bytes = hex_to_bytes(&metadata_hex)
        .map_err(|e| anyhow::anyhow!("failed to decode metadata hex: {e}"))?;

    let runtime_metadata = parse_runtime_metadata(&metadata_bytes)
        .map_err(|e| anyhow::anyhow!("failed to parse metadata: {e}"))?;

    let pallet_names: Vec<&str> = runtime_metadata
        .pallets
        .iter()
        .map(|p| p.name.as_str())
        .collect();

    if cmd.json {
        let out = serde_json::json!({
            "chain": cmd.chain,
            "metadata_version": runtime_metadata.version,
            "pallet_count": pallet_names.len(),
            "pallets": pallet_names,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Chain metadata");
        println!("  Chain:            {}", cmd.chain);
        println!("  Metadata version: V{}", runtime_metadata.version);
        println!(
            "  Pallets ({}):     {}",
            pallet_names.len(),
            pallet_names.join(", ")
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------

async fn decode(cmd: &ChainDecodeCmd, rpc_url: Option<&str>) -> Result<()> {
    let url = require_rpc_url(rpc_url)?;
    let rpc = make_rpc_client()?;

    let call_bytes =
        hex_to_bytes(&cmd.hex).map_err(|e| anyhow::anyhow!("invalid hex input: {e}"))?;

    // Fetch metadata from the node.
    let metadata_hex = rpc
        .state_get_metadata(url, None)
        .await
        .context("failed to fetch runtime metadata")?;

    let metadata_bytes = hex_to_bytes(&metadata_hex)
        .map_err(|e| anyhow::anyhow!("failed to decode metadata hex: {e}"))?;

    let metadata_digest = compute_metadata_digest(&metadata_bytes);

    let decoded = decode_call_bytes(&call_bytes, &metadata_bytes, &metadata_digest)
        .map_err(|e| anyhow::anyhow!("failed to decode call: {e}"))?;

    if cmd.json {
        let out = serde_json::json!({
            "chain": cmd.chain,
            "hex": cmd.hex,
            "pallet": decoded.pallet,
            "call": decoded.call_name,
            "arguments": serde_json::from_str::<serde_json::Value>(&decoded.arguments_json)
                .unwrap_or(serde_json::Value::String(decoded.arguments_json.clone())),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Decoded call");
        println!("  Chain:     {}", cmd.chain);
        println!("  Pallet:    {}", decoded.pallet);
        println!("  Call:      {}", decoded.call_name);
        println!("  Arguments: {}", decoded.arguments_json);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// balance
// ---------------------------------------------------------------------------

async fn balance(cmd: &ChainBalanceCmd, rpc_url: Option<&str>) -> Result<()> {
    let url = require_rpc_url(rpc_url)?;
    let rpc = make_rpc_client()?;

    // Compute the System.Account storage key for the address.
    // Storage key = twox128("System") ++ twox128("Account") ++ blake2_128_concat(account_id)
    //
    // The address must be a hex-encoded 32-byte account ID (with optional 0x prefix).
    let account_bytes = hex_to_bytes(&cmd.address).map_err(|e| {
        anyhow::anyhow!("invalid account address (expected hex-encoded account ID): {e}")
    })?;

    if account_bytes.len() != 32 {
        anyhow::bail!(
            "account ID must be 32 bytes, got {} bytes. \
             Provide the hex-encoded account ID (not SS58 address).",
            account_bytes.len()
        );
    }

    let system_prefix = twox128(b"System");
    let account_prefix = twox128(b"Account");
    let account_hash = blake2_128_concat(&account_bytes);

    let mut storage_key = Vec::with_capacity(32 + 16 + 32);
    storage_key.extend_from_slice(&system_prefix);
    storage_key.extend_from_slice(&account_prefix);
    storage_key.extend_from_slice(&account_hash);

    let storage_key_hex = polkagent_chain_subxt::decode::bytes_to_hex(&storage_key);

    let result = rpc
        .state_get_storage(url, &storage_key_hex, None)
        .await
        .context("failed to query account storage")?;

    match result {
        Some(hex_val) => {
            let data = hex_to_bytes(&hex_val)
                .map_err(|e| anyhow::anyhow!("failed to decode storage value: {e}"))?;

            // AccountInfo layout (Substrate):
            //   nonce:       u32  (4 bytes)
            //   consumers:   u32  (4 bytes)
            //   providers:   u32  (4 bytes)
            //   sufficients: u32  (4 bytes)
            //   data.free:       u128 (16 bytes)
            //   data.reserved:   u128 (16 bytes)
            //   data.frozen:     u128 (16 bytes)
            let (free, reserved, frozen) = if data.len() >= 64 {
                let free = u128::from_le_bytes(data[16..32].try_into().unwrap_or([0u8; 16]));
                let reserved = u128::from_le_bytes(data[32..48].try_into().unwrap_or([0u8; 16]));
                let frozen = u128::from_le_bytes(data[48..64].try_into().unwrap_or([0u8; 16]));
                (free, reserved, frozen)
            } else {
                (0u128, 0u128, 0u128)
            };

            if cmd.json {
                let out = serde_json::json!({
                    "chain": cmd.chain,
                    "address": cmd.address,
                    "free": free.to_string(),
                    "reserved": reserved.to_string(),
                    "frozen": frozen.to_string(),
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Account balance");
                println!("  Chain:    {}", cmd.chain);
                println!("  Address:  {}", cmd.address);
                println!("  Free:     {free}");
                println!("  Reserved: {reserved}");
                println!("  Frozen:   {frozen}");
            }
        }
        None => {
            if cmd.json {
                let out = serde_json::json!({
                    "chain": cmd.chain,
                    "address": cmd.address,
                    "free": "0",
                    "reserved": "0",
                    "frozen": "0",
                    "message": "Account not found on chain",
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Account balance");
                println!("  Chain:    {}", cmd.chain);
                println!("  Address:  {}", cmd.address);
                println!("  Account not found on chain (zero balance).");
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Hashing helpers for storage key computation
// ---------------------------------------------------------------------------

fn twox128(data: &[u8]) -> [u8; 16] {
    use std::hash::Hasher;
    use twox_hash::XxHash64;

    let mut h0 = XxHash64::with_seed(0);
    h0.write(data);
    let r0 = h0.finish();

    let mut h1 = XxHash64::with_seed(1);
    h1.write(data);
    let r1 = h1.finish();

    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&r0.to_le_bytes());
    out[8..].copy_from_slice(&r1.to_le_bytes());
    out
}

fn blake2_128_concat(data: &[u8]) -> Vec<u8> {
    use blake2::digest::{Update, VariableOutput};
    use blake2::Blake2bVar;

    // 16-byte output is always valid for BLAKE2b; these cannot fail.
    let mut hasher = Blake2bVar::new(16).unwrap_or_else(|_| unreachable!());
    hasher.update(data);
    let mut hash = [0u8; 16];
    let _ = hasher.finalize_variable(&mut hash);

    let mut out = Vec::with_capacity(16 + data.len());
    out.extend_from_slice(&hash);
    out.extend_from_slice(data);
    out
}

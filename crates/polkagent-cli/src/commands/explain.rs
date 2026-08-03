//! `polkagent explain` — decode and preview an extrinsic.

use anyhow::{bail, Context, Result};

use polkagent_card::builder::ActionCardBuilder;
use polkagent_card::render::render_text;
use polkagent_card::sections::SectionSource;
use polkagent_codec::decode::{DecodedExtrinsic, DecodedField, FieldValue};
use polkagent_codec::{decode_extrinsic, is_transfer_call, extract_transfer_amount};

use crate::cli::ExplainCmd;

/// Execute the `explain` subcommand.
pub fn run(cmd: &ExplainCmd) -> Result<()> {
    let bytes = parse_hex(&cmd.extrinsic_hex)
        .context("failed to parse extrinsic hex — expected a hex-encoded extrinsic (with or without 0x prefix)")?;

    if bytes.len() < 3 {
        bail!(
            "extrinsic too short ({} bytes): need at least a length prefix, version byte, \
             pallet index, and call index",
            bytes.len()
        );
    }

    let decoded = decode_extrinsic(&bytes)
        .context("SCALE decode failed — the hex data may not be a valid Substrate extrinsic")?;

    if cmd.json {
        print_json(cmd, &decoded);
    } else {
        print_text(cmd, &decoded);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Hex parsing
// ---------------------------------------------------------------------------

fn parse_hex(hex: &str) -> Result<Vec<u8>, anyhow::Error> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    if hex.is_empty() {
        bail!("empty hex string");
    }
    if hex.len() % 2 != 0 {
        bail!("hex string has odd length");
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.as_bytes().chunks(2) {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, anyhow::Error> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => bail!("invalid hex character: {:?}", char::from(b)),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

fn pallet_display(ext: &DecodedExtrinsic) -> String {
    match &ext.pallet_name {
        Some(name) => format!("{name} (index {idx})", idx = ext.pallet_index),
        None => format!("index {}", ext.pallet_index),
    }
}

fn call_display(ext: &DecodedExtrinsic) -> String {
    match &ext.call_name {
        Some(name) => format!("{name} (index {idx})", idx = ext.call_index),
        None => format!("index {}", ext.call_index),
    }
}

fn format_field(field: &DecodedField) -> String {
    match &field.name {
        Some(name) => format!("{name}: {}", field.value),
        None => format!("{}", field.value),
    }
}

// ---------------------------------------------------------------------------
// JSON output
// ---------------------------------------------------------------------------

fn print_json(cmd: &ExplainCmd, decoded: &DecodedExtrinsic) {
    let args_json: Vec<serde_json::Value> = decoded
        .args
        .iter()
        .map(|f| {
            serde_json::json!({
                "name": f.name,
                "value": format!("{}", f.value),
            })
        })
        .collect();

    let out = serde_json::json!({
        "extrinsic": cmd.extrinsic_hex,
        "chain": cmd.chain,
        "metadata_version": cmd.metadata_version,
        "status": "decoded",
        "pallet_index": decoded.pallet_index,
        "pallet_name": decoded.pallet_name,
        "call_index": decoded.call_index,
        "call_name": decoded.call_name,
        "arguments": args_json,
        "is_transfer": is_transfer_call(decoded),
        "transfer_amount": extract_transfer_amount(decoded),
    });

    // unwrap is safe: serde_json::json! always produces valid JSON
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}

// ---------------------------------------------------------------------------
// Plain-text output
// ---------------------------------------------------------------------------

fn print_text(cmd: &ExplainCmd, decoded: &DecodedExtrinsic) {
    println!("Decoded extrinsic");
    println!("  Chain:   {}", cmd.chain);
    println!("  Pallet:  {}", pallet_display(decoded));
    println!("  Call:    {}", call_display(decoded));

    if decoded.args.is_empty() {
        println!("  Args:    (none)");
    } else {
        println!("  Args:");
        for field in &decoded.args {
            match &field.value {
                FieldValue::Bytes(b) if b.len() > 64 => {
                    println!("    - {} ({}B raw)", hex_abbrev(b), b.len());
                }
                _ => {
                    println!("    - {}", format_field(field));
                }
            }
        }
    }

    if is_transfer_call(decoded) {
        if let Some(amount) = extract_transfer_amount(decoded) {
            println!("  Transfer amount: {amount}");
        }
    }

    println!();

    // Action card preview
    let card_title = format!(
        "{}.{}",
        decoded.pallet_name.as_deref().unwrap_or(&format!("pallet#{}", decoded.pallet_index)),
        decoded.call_name.as_deref().unwrap_or(&format!("call#{}", decoded.call_index)),
    );

    let mut builder = ActionCardBuilder::new(&card_title)
        .add_canonical("Pallet", pallet_display(decoded), SectionSource::Metadata)
        .add_canonical("Call", call_display(decoded), SectionSource::Metadata)
        .with_payload_hash(hex_encode(
            &[decoded.pallet_index, decoded.call_index],
        ));

    if !decoded.args.is_empty() {
        let args_summary: String = decoded
            .args
            .iter()
            .map(|f| format_field(f))
            .collect::<Vec<_>>()
            .join(", ");
        let truncated = if args_summary.len() > 120 {
            format!("{}…", &args_summary[..120])
        } else {
            args_summary
        };
        builder = builder.add_canonical_unverified("Arguments", truncated, SectionSource::Metadata);
    }

    if is_transfer_call(decoded) {
        if let Some(amount) = extract_transfer_amount(decoded) {
            builder = builder.add_canonical("Transfer amount", format!("{amount}"), SectionSource::Metadata);
        }
    }

    let card = builder.build();
    let rendered = render_text(&card);
    print!("{rendered}");
}

fn hex_abbrev(bytes: &[u8]) -> String {
    if bytes.len() <= 8 {
        hex_encode(bytes)
    } else {
        format!("{}…{}", hex_encode(&bytes[..4]), hex_encode(&bytes[bytes.len()-4..]))
    }
}

//! `polkagent explain` — decode and preview an extrinsic.

use anyhow::Result;

use crate::cli::ExplainCmd;

/// Execute the `explain` subcommand.
pub fn run(cmd: &ExplainCmd) -> Result<()> {
    if cmd.json {
        let out = serde_json::json!({
            "extrinsic": cmd.extrinsic_hex,
            "chain": cmd.chain,
            "metadata_version": cmd.metadata_version,
            "status": "not_decoded",
            "message": "Extrinsic decode not yet connected to chain adapter",
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Explain extrinsic");
        println!("  Hex:              {}", cmd.extrinsic_hex);
        println!("  Chain:            {}", cmd.chain);
        if let Some(v) = cmd.metadata_version {
            println!("  Metadata version: {v}");
        }
        println!();

        // Action card preview (stub).
        println!("  +-----------------------------------------------+");
        println!("  |  ACTION CARD PREVIEW                          |");
        println!("  |                                               |");
        println!("  |  Extrinsic decode not yet connected to        |");
        println!("  |  chain adapter.                               |");
        println!("  |                                               |");
        println!("  |  Once connected, this will show:              |");
        println!("  |    - Pallet & call name                       |");
        println!("  |    - Decoded parameters                       |");
        println!("  |    - Estimated fees                           |");
        println!("  |    - Signer address                           |");
        println!("  +-----------------------------------------------+");
    }

    Ok(())
}

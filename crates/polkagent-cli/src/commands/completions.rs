//! `polkagent completions` — generate shell completion scripts.

use anyhow::Result;
use clap::CommandFactory;
use clap_complete::generate;

use crate::cli::{Cli, CompletionsCmd};

/// Execute the `completions` subcommand.
pub fn run(cmd: &CompletionsCmd) -> Result<()> {
    let mut app = Cli::command();
    let bin_name = "polkagent";

    generate(cmd.shell, &mut app, bin_name, &mut std::io::stdout());

    Ok(())
}

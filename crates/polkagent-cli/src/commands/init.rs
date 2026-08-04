//! `polkagent init` — initialize a .polkagent/ project directory.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};
use tracing::info;

use crate::cli::InitCmd;

/// Execute the `init` subcommand.
pub fn run(cmd: &InitCmd) -> Result<()> {
    let dir = &cmd.directory;
    let polkagent_dir = dir.join(".polkagent");

    if polkagent_dir.exists() && !cmd.force {
        bail!(
            ".polkagent/ already exists at {}. Use --force to overwrite.",
            polkagent_dir.display()
        );
    }

    // Create the directory structure.
    create_dir(&polkagent_dir)?;
    create_dir(&polkagent_dir.join("agents"))?;
    create_dir(&polkagent_dir.join("runs"))?;

    // Write default config.
    let config_path = polkagent_dir.join("polkagent.toml");
    write_file(
        &config_path,
        polkagent_config::schema::DEFAULT_CONFIG_TEMPLATE,
    )?;

    // Write .gitignore to avoid committing the database.
    let gitignore_path = polkagent_dir.join(".gitignore");
    write_file(&gitignore_path, "*.db\n*.db-shm\n*.db-wal\n")?;

    println!("Initialized Polkagent project at {}", dir.display());
    println!("  {}", polkagent_dir.join("polkagent.toml").display());
    println!();
    println!("Next steps:");
    println!("  1. Edit .polkagent/polkagent.toml");
    println!("  2. Set ANTHROPIC_API_KEY in your environment");
    println!("  3. Run `polkagent agent create <NAME>` to create your first agent");

    info!(dir = %dir.display(), "polkagent project initialized");
    Ok(())
}

fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("creating directory {}", path.display()))
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

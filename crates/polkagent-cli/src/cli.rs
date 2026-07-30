//! Clap CLI definitions for the `polkagent` binary.
//!
//! All subcommands are documented here. When no subcommand is provided the
//! binary defaults to launching the interactive TUI (see `main.rs`).

use clap::{Args, Parser, Subcommand};

// ---------------------------------------------------------------------------
// Root CLI
// ---------------------------------------------------------------------------

/// Polkagent — Rust-first, Polkadot-native agent platform.
#[derive(Debug, Parser)]
#[command(
    name = "polkagent",
    version,
    about = "Polkagent — Rust-first, Polkadot-native agent platform",
    long_about = None,
)]
pub struct Cli {
    /// Disable colours and TUI effects (equivalent to NO_COLOR=1).
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Override the config file path.
    #[arg(long, short = 'c', global = true, value_name = "PATH")]
    pub config: Option<std::path::PathBuf>,

    /// Logging verbosity (-v = debug, -vv = trace).
    #[arg(short = 'v', long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

// ---------------------------------------------------------------------------
// Top-level subcommands
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialize a new .polkagent/ project directory.
    Init(InitCmd),

    /// Execute a run against an agent.
    Run(RunCmd),

    /// Manage agents (create, list, show, delete).
    #[command(subcommand)]
    Agent(AgentCmd),

    /// Launch the interactive terminal UI.
    Tui(TuiCmd),

    /// Show or validate the current configuration.
    #[command(subcommand)]
    Config(ConfigCmd),

    /// Print version information and exit.
    Version,
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

/// Initialize a new .polkagent/ project directory.
///
/// Creates `.polkagent/polkagent.toml` with defaults and a SQLite database
/// in the current working directory.
#[derive(Debug, Args)]
pub struct InitCmd {
    /// Directory to initialize (defaults to current directory).
    #[arg(value_name = "DIR", default_value = ".")]
    pub directory: std::path::PathBuf,

    /// Overwrite an existing .polkagent/ directory.
    #[arg(long)]
    pub force: bool,
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

/// Execute a single run against an agent and stream its output.
#[derive(Debug, Args)]
pub struct RunCmd {
    /// Agent to invoke (name or UUID).
    #[arg(long, short = 'a', value_name = "AGENT")]
    pub agent_id: String,

    /// Prompt to send to the agent.
    #[arg(long, short = 'p', value_name = "TEXT")]
    pub prompt: String,

    /// Emit structured JSON output instead of pretty-printed text.
    #[arg(long)]
    pub json: bool,

    /// Wait for the run to complete and exit with its status code.
    #[arg(long, default_value_t = true)]
    pub wait: bool,

    /// Maximum seconds to wait for run completion (0 = no limit).
    #[arg(long, value_name = "SECS", default_value_t = 0)]
    pub timeout: u64,
}

// ---------------------------------------------------------------------------
// agent
// ---------------------------------------------------------------------------

/// Agent management subcommands.
#[derive(Debug, Subcommand)]
pub enum AgentCmd {
    /// Create a new agent from a spec file or interactive wizard.
    Create(AgentCreateCmd),

    /// List all agents with their current state.
    List(AgentListCmd),

    /// Show detailed information about a specific agent.
    Show(AgentShowCmd),

    /// Delete an agent (archives it; data is retained).
    Delete(AgentDeleteCmd),
}

#[derive(Debug, Args)]
pub struct AgentCreateCmd {
    /// Agent name.
    #[arg(value_name = "NAME")]
    pub name: String,

    /// AI model identifier (e.g. `anthropic/claude-sonnet-4-6`).
    #[arg(long, short = 'm', value_name = "MODEL", default_value = "anthropic/claude-sonnet-4-6")]
    pub model: String,

    /// Optional description.
    #[arg(long, short = 'd', value_name = "TEXT")]
    pub description: Option<String>,

    /// Emit JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentListCmd {
    /// Show only agents in this state (active, idle, error, all).
    #[arg(long, value_name = "STATE", default_value = "all")]
    pub filter: String,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentShowCmd {
    /// Agent name or UUID.
    #[arg(value_name = "AGENT")]
    pub agent: String,

    /// Include full spec JSON.
    #[arg(long)]
    pub full: bool,

    /// Emit JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct AgentDeleteCmd {
    /// Agent name or UUID.
    #[arg(value_name = "AGENT")]
    pub agent: String,

    /// Skip confirmation prompt.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

// ---------------------------------------------------------------------------
// tui
// ---------------------------------------------------------------------------

/// Launch the interactive ROSEDUST terminal UI.
#[derive(Debug, Args)]
pub struct TuiCmd {
    /// Start on a specific tab: dashboard, agents, runs, system.
    #[arg(long, value_name = "TAB", default_value = "dashboard")]
    pub tab: String,
}

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

/// Configuration management subcommands.
#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    /// Print the resolved configuration (file + env overrides).
    Show(ConfigShowCmd),

    /// Validate the configuration file without starting the daemon.
    Validate(ConfigValidateCmd),
}

#[derive(Debug, Args)]
pub struct ConfigShowCmd {
    /// Emit raw TOML output.
    #[arg(long)]
    pub toml: bool,

    /// Emit JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ConfigValidateCmd {
    /// Path to validate (defaults to the auto-discovered config).
    #[arg(value_name = "PATH")]
    pub path: Option<std::path::PathBuf>,
}

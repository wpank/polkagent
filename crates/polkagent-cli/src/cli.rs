//! Clap CLI definitions for the `polkagent` binary.
//!
//! All subcommands are documented here. When no subcommand is provided the
//! binary defaults to launching the interactive TUI (see `main.rs`).

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;

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

    /// Decode and preview an extrinsic.
    Explain(ExplainCmd),

    /// Manage pending effects awaiting approval.
    #[command(subcommand)]
    Inbox(InboxCmd),

    /// Chain interaction and inspection.
    #[command(subcommand)]
    Chain(ChainCmd),

    /// Run system health checks.
    Doctor(DoctorCmd),

    /// Manage agent memory (search, list, forget, stats).
    #[command(subcommand)]
    Memory(MemoryCmd),

    /// Generate shell completion scripts.
    Completions(CompletionsCmd),

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

// ---------------------------------------------------------------------------
// explain
// ---------------------------------------------------------------------------

/// Decode and preview a hex-encoded extrinsic.
#[derive(Debug, Args)]
pub struct ExplainCmd {
    /// Hex-encoded extrinsic to decode.
    #[arg(value_name = "EXTRINSIC_HEX")]
    pub extrinsic_hex: String,

    /// Target chain (default: polkadot).
    #[arg(long, default_value = "polkadot")]
    pub chain: String,

    /// Metadata version to use for decoding.
    #[arg(long, value_name = "VERSION")]
    pub metadata_version: Option<u32>,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// inbox
// ---------------------------------------------------------------------------

/// Manage pending effects awaiting approval.
#[derive(Debug, Subcommand)]
pub enum InboxCmd {
    /// List pending effects awaiting approval.
    List(InboxListCmd),

    /// Show detailed information about an effect.
    Show(InboxShowCmd),

    /// Approve an effect for execution.
    Approve(InboxApproveCmd),

    /// Deny an effect (reject it from execution).
    Deny(InboxDenyCmd),

    /// Show recently resolved effects.
    History(InboxHistoryCmd),
}

#[derive(Debug, Args)]
pub struct InboxListCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct InboxShowCmd {
    /// Effect ID to show.
    #[arg(value_name = "EFFECT_ID")]
    pub effect_id: String,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct InboxApproveCmd {
    /// Effect ID to approve.
    #[arg(value_name = "EFFECT_ID")]
    pub effect_id: String,
}

#[derive(Debug, Args)]
pub struct InboxDenyCmd {
    /// Effect ID to deny.
    #[arg(value_name = "EFFECT_ID")]
    pub effect_id: String,

    /// Reason for denying the effect.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
}

#[derive(Debug, Args)]
pub struct InboxHistoryCmd {
    /// Maximum number of resolved effects to show.
    #[arg(long, value_name = "N", default_value_t = 20)]
    pub limit: usize,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// chain
// ---------------------------------------------------------------------------

/// Chain interaction and inspection subcommands.
#[derive(Debug, Subcommand)]
pub enum ChainCmd {
    /// Show chain connection status.
    Status(ChainStatusCmd),

    /// Show current metadata version.
    Metadata(ChainMetadataCmd),

    /// Decode call data from hex.
    Decode(ChainDecodeCmd),

    /// Check account balance.
    Balance(ChainBalanceCmd),
}

#[derive(Debug, Args)]
pub struct ChainStatusCmd {
    /// Target chain.
    #[arg(long, default_value = "polkadot")]
    pub chain: String,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ChainMetadataCmd {
    /// Target chain.
    #[arg(long, default_value = "polkadot")]
    pub chain: String,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ChainDecodeCmd {
    /// Hex-encoded call data to decode.
    #[arg(value_name = "HEX")]
    pub hex: String,

    /// Target chain.
    #[arg(long, default_value = "polkadot")]
    pub chain: String,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ChainBalanceCmd {
    /// Account address to check.
    #[arg(value_name = "ADDRESS")]
    pub address: String,

    /// Target chain.
    #[arg(long, default_value = "polkadot")]
    pub chain: String,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

/// Run system health checks and diagnostics.
#[derive(Debug, Args)]
pub struct DoctorCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// memory
// ---------------------------------------------------------------------------

/// Agent memory management subcommands.
#[derive(Debug, Subcommand)]
pub enum MemoryCmd {
    /// Search memory entries.
    Search(MemorySearchCmd),

    /// List memory entries.
    List(MemoryListCmd),

    /// Delete a memory entry (forget it).
    Forget(MemoryForgetCmd),

    /// Show memory usage statistics.
    Stats(MemoryStatsCmd),
}

#[derive(Debug, Args)]
pub struct MemorySearchCmd {
    /// Search query text.
    #[arg(value_name = "QUERY")]
    pub query: String,

    /// Maximum number of results.
    #[arg(long, value_name = "N", default_value_t = 10)]
    pub limit: usize,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct MemoryListCmd {
    /// Filter by memory type.
    #[arg(long, value_name = "TYPE", value_parser = parse_memory_type)]
    pub memory_type: Option<polkagent_memory::types::MemoryType>,

    /// Maximum number of entries to show.
    #[arg(long, value_name = "N", default_value_t = 20)]
    pub limit: usize,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct MemoryForgetCmd {
    /// Memory ID to delete.
    #[arg(value_name = "ID")]
    pub memory_id: String,
}

#[derive(Debug, Args)]
pub struct MemoryStatsCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Parse a memory type string into a `MemoryType`.
fn parse_memory_type(s: &str) -> Result<polkagent_memory::types::MemoryType, String> {
    s.parse()
}

// ---------------------------------------------------------------------------
// completions
// ---------------------------------------------------------------------------

/// Generate shell completion scripts.
///
/// Output the completion script to stdout. Redirect to a file or pipe to
/// `source` to install:
///
/// ```sh
/// polkagent completions bash > ~/.local/share/bash-completion/completions/polkagent
/// polkagent completions zsh > ~/.zfunc/_polkagent
/// polkagent completions fish > ~/.config/fish/completions/polkagent.fish
/// ```
#[derive(Debug, Args)]
pub struct CompletionsCmd {
    /// Shell to generate completions for.
    #[arg(value_name = "SHELL")]
    pub shell: Shell,
}

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

    /// Output format (human, json, json-pretty, table).
    #[arg(long, global = true, value_name = "FORMAT", default_value = "human")]
    pub format: crate::output::OutputFormat,

    /// Show what would happen without actually executing (no writes).
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Skip interactive approval prompts (assume yes).
    #[arg(long, global = true)]
    pub yes: bool,

    /// Override the log file path. Use `-` for stderr only (no file logging).
    ///
    /// Default: `~/.polkagent/logs/events.jsonl`
    #[arg(long, global = true, value_name = "PATH")]
    pub log_file: Option<String>,

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

    /// Manage agents (create, list, show, delete, start, stop, pause, resume).
    #[command(subcommand)]
    Agent(AgentCmd),

    /// Manage skills (list, install, update, remove, show).
    #[command(subcommand)]
    Skill(SkillCmd),

    /// Manage product kits (install, uninstall, list).
    #[command(subcommand)]
    Kit(KitCmd),

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

    /// Show agent count, active runs, effect queue depth, and memory usage.
    Status(StatusCmd),

    /// Tail the event log.
    Logs(LogsCmd),

    /// Generate shell completion scripts.
    Completions(CompletionsCmd),

    /// Print version information and exit.
    Version,

    /// Run, list, report, and compare evaluation suites.
    #[command(subcommand)]
    Eval(EvalCmd),

    /// Export data from the Polkagent database (runs, effects, artifacts, events, audit, config).
    #[command(subcommand)]
    Export(crate::commands::export::ExportArgs),

    /// Inspect runs, effects, artifacts, agents, policies, and database stats.
    #[command(subcommand)]
    Inspect(crate::commands::inspect::InspectCmd),

    /// Manage API keys and authentication credentials.
    #[command(subcommand)]
    Auth(AuthCmd),

    /// Show network endpoint status and infrastructure guidance.
    #[command(subcommand)]
    Network(NetworkCmd),

    /// Start the API server (daemon mode).
    Serve(ServeCmd),
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

    /// Provider to use for model inference (e.g. `anthropic`, `openai`).
    ///
    /// Resolution order: CLI flag > config `[execution] default_provider` >
    /// environment detection (first API key found) > fake executor fallback.
    #[arg(long, value_name = "PROVIDER")]
    pub provider: Option<String>,

    /// Override the model for this run (e.g. `anthropic/claude-opus-4-6`).
    #[arg(long, short = 'm', value_name = "MODEL")]
    pub model: Option<String>,

    /// Harness to use for agent execution (e.g. `claude-code`, `codex`, `cursor`, `goose`).
    ///
    /// Resolution order: CLI flag > config `[harness] default` >
    /// first available harness on PATH > executor-only mode.
    #[arg(long, value_name = "HARNESS")]
    pub harness: Option<String>,

    /// Stream live token output as it arrives (default: enabled).
    #[arg(long, default_value_t = true, action = clap::ArgAction::SetTrue)]
    pub stream: bool,

    /// Disable streaming; wait for completion then print the final response.
    #[arg(long = "no-stream", overrides_with = "stream", action = clap::ArgAction::SetFalse)]
    pub no_stream: bool,

    /// Cancel the run after this many seconds (0 = no limit, default: 300).
    #[arg(long, value_name = "SECS", default_value_t = 300)]
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

    /// Start an agent (transition to active state).
    Start(AgentStartCmd),

    /// Stop a running agent (transition to configured state).
    Stop(AgentStopCmd),

    /// Pause a running agent.
    Pause(AgentPauseCmd),

    /// Resume a paused agent.
    Resume(AgentResumeCmd),
}

#[derive(Debug, Args)]
pub struct AgentCreateCmd {
    /// Agent name.
    #[arg(value_name = "NAME")]
    pub name: String,

    /// AI model identifier (e.g. `anthropic/claude-sonnet-4-6`).
    #[arg(
        long,
        short = 'm',
        value_name = "MODEL",
        default_value = "anthropic/claude-sonnet-4-6"
    )]
    pub model: String,

    /// Optional description.
    #[arg(long, short = 'd', value_name = "TEXT")]
    pub description: Option<String>,

    /// Emit JSON output.
    #[arg(long)]
    pub json: bool,

    // ------------------------------------------------------------------
    // PRD-03 flags
    // ------------------------------------------------------------------
    /// Declare a capability for this agent (repeatable).
    ///
    /// Example: `--capability file.read --capability chain.query`
    #[arg(long = "capability", value_name = "CAP", action = clap::ArgAction::Append)]
    pub capabilities: Vec<String>,

    /// Maximum number of agentic turns per run.
    #[arg(long, value_name = "N")]
    pub max_turns: Option<u32>,

    /// Preferred model ID within the provider (e.g. `claude-opus-4-6`).
    ///
    /// Sets `model_preference.model_id`. Use `--model` (with provider prefix)
    /// for the canonical model field; use this flag to specify just the model
    /// ID within the provider selected at runtime.
    #[arg(long = "preferred-model", value_name = "MODEL_ID")]
    pub preferred_model: Option<String>,

    /// Wall-clock timeout in seconds for each run.
    #[arg(long, value_name = "SECS")]
    pub timeout: Option<u64>,

    /// Maximum tokens the model may generate in a single turn.
    #[arg(long, value_name = "N")]
    pub max_tokens_per_turn: Option<u32>,
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

#[derive(Debug, Args)]
pub struct AgentStartCmd {
    /// Agent name or UUID.
    #[arg(value_name = "AGENT")]
    pub agent: String,
}

#[derive(Debug, Args)]
pub struct AgentStopCmd {
    /// Agent name or UUID.
    #[arg(value_name = "AGENT")]
    pub agent: String,
}

#[derive(Debug, Args)]
pub struct AgentPauseCmd {
    /// Agent name or UUID.
    #[arg(value_name = "AGENT")]
    pub agent: String,
}

#[derive(Debug, Args)]
pub struct AgentResumeCmd {
    /// Agent name or UUID.
    #[arg(value_name = "AGENT")]
    pub agent: String,
}

// ---------------------------------------------------------------------------
// skill
// ---------------------------------------------------------------------------

/// Skill management subcommands.
#[derive(Debug, Subcommand)]
pub enum SkillCmd {
    /// List all loaded skills.
    List(SkillListCmd),

    /// Install a skill from a local path (validates the manifest).
    Install(SkillInstallCmd),

    /// Update an already-installed skill.
    Update(SkillUpdateCmd),

    /// Uninstall a skill.
    Remove(SkillRemoveCmd),

    /// Show manifest details for a skill.
    Show(SkillShowCmd),
}

#[derive(Debug, Args)]
pub struct SkillListCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct SkillInstallCmd {
    /// Local path to the skill directory or manifest file.
    #[arg(value_name = "PATH")]
    pub path: std::path::PathBuf,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct SkillUpdateCmd {
    /// Name of the skill to update.
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct SkillRemoveCmd {
    /// Name of the skill to uninstall.
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Skip confirmation prompt.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct SkillShowCmd {
    /// Name of the skill to inspect.
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// kit
// ---------------------------------------------------------------------------

/// Product kit management subcommands.
#[derive(Debug, Subcommand)]
pub enum KitCmd {
    /// Install a product kit from a local path.
    Install(KitInstallCmd),

    /// Uninstall a product kit by name.
    Uninstall(KitUninstallCmd),

    /// List all installed product kits.
    List(KitListCmd),
}

#[derive(Debug, Args)]
pub struct KitInstallCmd {
    /// Local path to the kit directory or kit.toml manifest file.
    #[arg(value_name = "PATH")]
    pub path: std::path::PathBuf,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct KitUninstallCmd {
    /// Name of the kit to uninstall.
    #[arg(value_name = "NAME")]
    pub name: String,

    /// Skip confirmation prompt.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct KitListCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

/// Show system status: agent count, active runs, effect queue depth, memory usage.
#[derive(Debug, Args)]
pub struct StatusCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// logs
// ---------------------------------------------------------------------------

/// Tail the event log.
#[derive(Debug, Args)]
pub struct LogsCmd {
    /// Follow the log stream (like `tail -f`).
    #[arg(long, short = 'f')]
    pub follow: bool,

    /// Filter events for a specific run ID.
    #[arg(long, value_name = "RUN_ID")]
    pub run_id: Option<String>,

    /// Minimum log level to show (trace, debug, info, warn, error).
    #[arg(long, value_name = "LEVEL", default_value = "info")]
    pub level: String,

    /// Maximum number of lines to show (0 = no limit).
    #[arg(long, value_name = "N", default_value_t = 100)]
    pub lines: usize,
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

    /// Show the resolved config file paths (system, user, project).
    Path(ConfigPathCmd),

    /// Get a specific configuration value by dotted key path.
    Get(ConfigGetCmd),
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

/// Show resolved config file paths.
#[derive(Debug, Args)]
pub struct ConfigPathCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Get a single config value by dotted key path (e.g. `log.level`).
#[derive(Debug, Args)]
pub struct ConfigGetCmd {
    /// Dotted key path to look up (e.g. `log.level`, `api.bind_address`).
    #[arg(value_name = "KEY")]
    pub key: String,

    /// Emit raw JSON for the value (instead of plain text).
    #[arg(long)]
    pub json: bool,
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

    /// Export all memories for an agent to a JSON archive file.
    Export(MemoryExportCmd),

    /// Import memories from a JSON archive file.
    Import(MemoryImportCmd),

    /// Run a retention sweep to remove old or low-relevance entries.
    Sweep(MemorySweepCmd),
}

#[derive(Debug, Args)]
pub struct MemorySearchCmd {
    /// Search query text.
    #[arg(value_name = "QUERY")]
    pub query: String,

    /// Agent ID to search within (searches all agents when omitted).
    #[arg(long, value_name = "AGENT_ID")]
    pub agent_id: Option<String>,

    /// Maximum number of results.
    #[arg(long, value_name = "N", default_value_t = 10)]
    pub limit: usize,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct MemoryListCmd {
    /// Agent ID to list memories for.
    #[arg(long, value_name = "AGENT_ID")]
    pub agent_id: Option<String>,

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

/// Export memories for an agent to a JSON archive file.
#[derive(Debug, Args)]
pub struct MemoryExportCmd {
    /// Agent ID whose memories to export.
    #[arg(value_name = "AGENT_ID")]
    pub agent_id: String,

    /// Output file path for the JSON archive.
    #[arg(value_name = "OUTPUT_PATH")]
    pub output_path: std::path::PathBuf,

    /// Emit structured JSON output (summary only).
    #[arg(long)]
    pub json: bool,
}

/// Import memories from a JSON archive file.
#[derive(Debug, Args)]
pub struct MemoryImportCmd {
    /// Input JSON archive file path.
    #[arg(value_name = "INPUT_PATH")]
    pub input_path: std::path::PathBuf,

    /// Emit structured JSON output (summary only).
    #[arg(long)]
    pub json: bool,
}

/// Run a retention sweep for an agent.
#[derive(Debug, Args)]
pub struct MemorySweepCmd {
    /// Agent ID to sweep (required).
    #[arg(value_name = "AGENT_ID")]
    pub agent_id: String,

    /// Show what would be deleted without actually deleting.
    #[arg(long)]
    pub dry_run: bool,

    /// Maximum age in days before entries are deleted (default: 90).
    #[arg(long, value_name = "DAYS", default_value_t = 90)]
    pub max_age_days: u64,

    /// Minimum relevance score; entries below this are deleted (default: 0.1).
    #[arg(long, value_name = "SCORE", default_value_t = 0.1)]
    pub min_relevance: f64,

    /// Maximum entries to keep per agent (default: 10000).
    #[arg(long, value_name = "N", default_value_t = 10_000)]
    pub max_entries: usize,

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

// ---------------------------------------------------------------------------
// eval
// ---------------------------------------------------------------------------

/// Evaluation suite subcommands.
#[derive(Debug, Subcommand)]
pub enum EvalCmd {
    /// Run an evaluation suite against the configured executor.
    Run(EvalRunCmd),

    /// List available evaluation suites from the fixtures/evals/ directory.
    List(EvalListCmd),

    /// Display a previously saved evaluation report.
    Report(EvalReportCmd),

    /// Compare two evaluation reports for regressions.
    Compare(EvalCompareCmd),
}

/// Run a single evaluation suite from a JSON file.
#[derive(Debug, Args)]
pub struct EvalRunCmd {
    /// Path to the suite JSON file (or directory containing a suite.json).
    #[arg(value_name = "SUITE_PATH")]
    pub suite_path: std::path::PathBuf,

    /// Agent name or ID to tag the run with.
    #[arg(long, short = 'a', value_name = "AGENT")]
    pub agent: Option<String>,

    /// Save the JSON report to this path.
    #[arg(long, value_name = "PATH")]
    pub output: Option<std::path::PathBuf>,

    /// Maximum number of cases to run concurrently.
    #[arg(long, value_name = "N", default_value_t = 4)]
    pub concurrency: usize,

    /// Model identifier to use for the executor.
    #[arg(long, value_name = "MODEL", default_value = "claude-opus-4-6")]
    pub model: String,

    /// Emit the full JSON report to stdout instead of Markdown.
    #[arg(long)]
    pub json: bool,

    /// Show detailed per-case scoring output.
    #[arg(long)]
    pub details: bool,
}

/// List available evaluation suites under fixtures/evals/.
#[derive(Debug, Args)]
pub struct EvalListCmd {
    /// Root directory to search for suites (defaults to fixtures/evals/).
    #[arg(value_name = "DIR", default_value = "fixtures/evals")]
    pub dir: std::path::PathBuf,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Display a previously saved evaluation report from a JSON file.
#[derive(Debug, Args)]
pub struct EvalReportCmd {
    /// Path to the saved report JSON file.
    #[arg(value_name = "REPORT_PATH")]
    pub report_path: std::path::PathBuf,

    /// Emit the raw JSON report instead of Markdown.
    #[arg(long)]
    pub json: bool,
}

/// Compare two evaluation reports for regressions and improvements.
#[derive(Debug, Args)]
pub struct EvalCompareCmd {
    /// Path to the baseline report JSON file.
    #[arg(value_name = "BASELINE")]
    pub baseline: std::path::PathBuf,

    /// Path to the current report JSON file.
    #[arg(value_name = "CURRENT")]
    pub current: std::path::PathBuf,

    /// Minimum absolute score delta to count as a regression or improvement.
    #[arg(long, value_name = "DELTA", default_value_t = 0.001)]
    pub min_delta: f64,

    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

/// Authentication and credential management subcommands.
#[derive(Debug, Subcommand)]
pub enum AuthCmd {
    /// Prompt for API keys and store them securely.
    Login(AuthLoginCmd),

    /// Remove stored credentials.
    Logout(AuthLogoutCmd),

    /// Show current identity: masked API keys, auth method, and agent identity.
    Whoami(AuthWhoamiCmd),

    /// Show which providers are configured and their auth method.
    Status(AuthStatusCmd),
}

/// Log in: prompt for API keys and save them to `~/.polkagent/credentials`.
#[derive(Debug, Args)]
pub struct AuthLoginCmd {
    /// Only prompt for the specified provider (e.g. `anthropic`, `openai`).
    ///
    /// When omitted, all supported providers are prompted in sequence.
    #[arg(long, value_name = "PROVIDER")]
    pub provider: Option<String>,

    /// Show what would be stored without actually writing the credentials file.
    #[arg(long)]
    pub dry_run: bool,
}

/// Log out: remove the `~/.polkagent/credentials` file.
#[derive(Debug, Args)]
pub struct AuthLogoutCmd {
    /// Show what would be removed without actually deleting the file.
    #[arg(long)]
    pub dry_run: bool,
}

/// Show identity and key configuration (all keys masked).
#[derive(Debug, Args)]
pub struct AuthWhoamiCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Show auth status: which providers have valid keys and where they come from.
#[derive(Debug, Args)]
pub struct AuthStatusCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// network
// ---------------------------------------------------------------------------

/// Network endpoint status and infrastructure guidance subcommands.
#[derive(Debug, Subcommand)]
pub enum NetworkCmd {
    /// Show chain status: name, best block, finalized block, peer count.
    Status(NetworkStatusCmd),

    /// Show metadata version and pallet list.
    Metadata(NetworkMetadataCmd),

    /// Print guidance on starting a local test network with zombienet/chopsticks.
    Start(NetworkStartCmd),

    /// Print guidance on stopping a local test network.
    Stop(NetworkStopCmd),
}

/// Show chain status: name, best block, finalized block, peer count.
#[derive(Debug, Args)]
pub struct NetworkStatusCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Show metadata version and pallet list.
#[derive(Debug, Args)]
pub struct NetworkMetadataCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Print guidance on starting a local test network.
#[derive(Debug, Args)]
pub struct NetworkStartCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

/// Print guidance on stopping a local test network.
#[derive(Debug, Args)]
pub struct NetworkStopCmd {
    /// Emit structured JSON output.
    #[arg(long)]
    pub json: bool,
}

// ---------------------------------------------------------------------------
// serve
// ---------------------------------------------------------------------------

/// Start the Polkagent HTTP API + WebSocket server.
///
/// Reads bind address, CORS, and database settings from the active
/// configuration, then starts the Axum-backed server. Handles SIGTERM/SIGINT
/// for graceful shutdown.
#[derive(Debug, Clone, Args)]
pub struct ServeCmd {
    /// Override the TCP port (default: 8080).
    #[arg(long, short = 'p', value_name = "PORT", default_value_t = 8080)]
    pub port: u16,

    /// Override the bind host (default: 0.0.0.0).
    #[arg(long, value_name = "HOST", default_value = "0.0.0.0")]
    pub host: String,

    /// Allowed CORS origins (repeatable). When omitted, uses the value from
    /// the config file.
    #[arg(long = "cors-origin", value_name = "ORIGIN", action = clap::ArgAction::Append)]
    pub cors_origins: Vec<String>,

    /// Start the server in read-only mode (reject all mutating requests).
    #[arg(long)]
    pub read_only: bool,
}

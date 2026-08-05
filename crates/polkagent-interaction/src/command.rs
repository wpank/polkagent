//! Typed slash-command catalog, parser, and execution boundary.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use polkagent_core::ids::{AgentId, ApprovalId, ConversationId, RunId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::InteractionError;
use crate::ids::InteractionTurnId;
use crate::model::{
    ApprovalDecision, ClientContext, InteractionSummary, InteractionTarget, TurnHandle,
};

/// Stable names for the required MVP domain commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandName {
    /// Discover commands.
    Help,
    /// Show interaction status.
    Status,
    /// List configured agents.
    Agents,
    /// Select an agent target.
    Agent,
    /// Create a new interaction.
    New,
    /// Resume an interaction.
    Resume,
    /// List recent or active runs.
    Runs,
    /// Inspect one run.
    Inspect,
    /// Cancel current or specified work.
    Cancel,
    /// Approve a pending effect.
    Approve,
    /// Deny a pending effect.
    Deny,
    /// Show or select the model.
    Model,
}

impl CommandName {
    /// Canonical command spelling without the leading slash.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Status => "status",
            Self::Agents => "agents",
            Self::Agent => "agent",
            Self::New => "new",
            Self::Resume => "resume",
            Self::Runs => "runs",
            Self::Inspect => "inspect",
            Self::Cancel => "cancel",
            Self::Approve => "approve",
            Self::Deny => "deny",
            Self::Model => "model",
        }
    }
}

/// Functional grouping for help and completion UIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandCategory {
    /// Discovery and status.
    General,
    /// Agent and model configuration.
    Configuration,
    /// Interaction creation and resumption.
    Interaction,
    /// Run inspection and cancellation.
    Execution,
    /// Approval decisions.
    Approval,
}

/// Whether command execution can mutate durable state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandMutability {
    /// Pure discovery or projection query.
    ReadOnly,
    /// Arguments determine whether the invocation is a query or mutation.
    Conditional,
    /// May change configuration or durable state.
    Mutating,
}

/// Conditions under which a command may be advertised or executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CommandAvailability {
    /// A selected interaction is required.
    pub interaction: bool,
    /// An active turn is required.
    pub active_turn: bool,
    /// At least one pending approval is required.
    pub pending_approval: bool,
}

/// Dynamic state used to filter the shared catalog for one client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandContext {
    /// Current interaction, when selected.
    pub conversation_id: Option<ConversationId>,
    /// Whether that interaction has active work.
    pub has_active_turn: bool,
    /// Number of real pending approvals visible to the caller.
    pub pending_approval_count: usize,
    /// Whether the caller/surface may request mutations.
    pub can_mutate: bool,
}

/// Surface-neutral command metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    /// Typed canonical identity.
    pub command: CommandName,
    /// Canonical name without a slash.
    pub name: String,
    /// Alternative names without slashes.
    pub aliases: Vec<String>,
    /// Human-readable purpose.
    pub description: String,
    /// Optional completion hint for arguments.
    pub input_hint: Option<String>,
    /// Help grouping.
    pub category: CommandCategory,
    /// Mutation classification.
    pub mutability: CommandMutability,
    /// Dynamic availability requirements.
    pub availability: CommandAvailability,
}

impl CommandSpec {
    /// Whether this command is valid in the supplied dynamic context.
    pub fn is_available(&self, context: &CommandContext) -> bool {
        if self.mutability == CommandMutability::Mutating && !context.can_mutate {
            return false;
        }
        if self.availability.interaction && context.conversation_id.is_none() {
            return false;
        }
        if self.availability.active_turn && !context.has_active_turn {
            return false;
        }
        if self.availability.pending_approval && context.pending_approval_count == 0 {
            return false;
        }
        true
    }
}

/// Typed target for `/cancel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", content = "id", rename_all = "snake_case")]
pub enum CancelTarget {
    /// Cancel the selected interaction's active turn.
    CurrentTurn,
    /// Cancel one specific run.
    Run(RunId),
    /// Cancel all active work visible in the selected interaction.
    All,
}

/// Parsed MVP command with typed identifiers and arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionCommand {
    /// `/help [command]`.
    Help {
        /// Optional command whose detailed help is requested.
        command: Option<CommandName>,
    },
    /// `/status`.
    Status,
    /// `/agents`.
    Agents,
    /// `/agent <name-or-id>`.
    Agent {
        /// Stable ID or configured name resolved by the service.
        selector: String,
    },
    /// `/new [title]`.
    New {
        /// Optional presentation title.
        title: Option<String>,
    },
    /// `/resume <conversation-id>`.
    Resume {
        /// Durable interaction identity.
        conversation_id: ConversationId,
    },
    /// `/runs`.
    Runs,
    /// `/inspect <run-id>`.
    Inspect {
        /// Run to inspect.
        run_id: RunId,
    },
    /// `/cancel [run-id|all]`.
    Cancel {
        /// Cancellation scope.
        target: CancelTarget,
    },
    /// `/approve <approval-id>`.
    Approve {
        /// Real approval identity.
        approval_id: ApprovalId,
    },
    /// `/deny <approval-id> [reason]`.
    Deny {
        /// Real approval identity.
        approval_id: ApprovalId,
        /// Optional safe operator rationale.
        reason: Option<String>,
    },
    /// `/model [id]`.
    Model {
        /// New model, or `None` to query the current selection.
        model: Option<String>,
    },
}

impl InteractionCommand {
    /// Return the canonical identity of this command.
    pub const fn name(&self) -> CommandName {
        match self {
            Self::Help { .. } => CommandName::Help,
            Self::Status => CommandName::Status,
            Self::Agents => CommandName::Agents,
            Self::Agent { .. } => CommandName::Agent,
            Self::New { .. } => CommandName::New,
            Self::Resume { .. } => CommandName::Resume,
            Self::Runs => CommandName::Runs,
            Self::Inspect { .. } => CommandName::Inspect,
            Self::Cancel { .. } => CommandName::Cancel,
            Self::Approve { .. } => CommandName::Approve,
            Self::Deny { .. } => CommandName::Deny,
            Self::Model { .. } => CommandName::Model,
        }
    }
}

/// A parsed command plus the spelling supplied by the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInvocation {
    /// Name or alias typed by the caller, without `/`.
    pub invoked_as: String,
    /// Typed command.
    pub command: InteractionCommand,
}

/// Parser output that keeps ordinary prompts separate from commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ParsedLine {
    /// Ordinary prompt text, preserved byte-for-byte.
    Prompt(String),
    /// Typed slash command.
    Command(CommandInvocation),
}

/// Safe parser or catalog error suitable for direct surface rendering.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommandParseError {
    /// `/` was supplied without a command name.
    #[error("command name is missing; use /help to list commands")]
    MissingCommand,
    /// The command is not registered.
    #[error("unknown command '/{name}'; use /help to list commands")]
    UnknownCommand {
        /// Unknown name without the slash.
        name: String,
    },
    /// A required argument was omitted.
    #[error("/{command} requires {expected}")]
    MissingArgument {
        /// Canonical command name.
        command: String,
        /// User-facing argument description.
        expected: String,
    },
    /// More arguments were supplied than the command accepts.
    #[error("/{command} accepts {expected}")]
    TooManyArguments {
        /// Canonical command name.
        command: String,
        /// User-facing arity description.
        expected: String,
    },
    /// A typed identifier could not be parsed.
    #[error("invalid {kind} '{value}' for /{command}")]
    InvalidIdentifier {
        /// Canonical command name.
        command: String,
        /// Identifier kind.
        kind: String,
        /// Rejected value.
        value: String,
    },
    /// A quote was not closed.
    #[error("unterminated quoted command argument")]
    UnterminatedQuote,
    /// A trailing escape did not escape another character.
    #[error("command line ends with an incomplete escape")]
    IncompleteEscape,
}

/// Canonical registry used for discovery, completion, and parsing.
#[derive(Debug, Clone)]
pub struct CommandRegistry {
    specs: BTreeMap<CommandName, CommandSpec>,
    names: BTreeMap<String, CommandName>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::mvp()
    }
}

impl CommandRegistry {
    /// Build the required FND-02 MVP catalog.
    pub fn mvp() -> Self {
        let specs = mvp_specs();
        let mut by_command = BTreeMap::new();
        let mut names = BTreeMap::new();
        for spec in specs {
            names.insert(spec.name.clone(), spec.command);
            for alias in &spec.aliases {
                names.insert(alias.clone(), spec.command);
            }
            by_command.insert(spec.command, spec);
        }
        Self {
            specs: by_command,
            names,
        }
    }

    /// Return all canonical specs in stable command-enum order.
    pub fn specs(&self) -> Vec<&CommandSpec> {
        self.specs.values().collect()
    }

    /// Resolve a canonical name or alias, with or without a leading slash.
    pub fn resolve(&self, name: &str) -> Option<&CommandSpec> {
        let normalized = normalize_name(name);
        self.names
            .get(&normalized)
            .and_then(|command| self.specs.get(command))
    }

    /// Return only commands valid in the supplied dynamic context.
    pub fn available(&self, context: &CommandContext) -> Vec<&CommandSpec> {
        self.specs
            .values()
            .filter(|spec| spec.is_available(context))
            .collect()
    }

    /// Parse an ordinary prompt or registered slash command.
    pub fn parse(&self, line: &str) -> Result<ParsedLine, CommandParseError> {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('/') {
            return Ok(ParsedLine::Prompt(line.to_owned()));
        }

        let tokens = tokenize(&trimmed[1..])?;
        let Some(invoked_as) = tokens.first() else {
            return Err(CommandParseError::MissingCommand);
        };
        if invoked_as.is_empty() {
            return Err(CommandParseError::MissingCommand);
        }
        let normalized = normalize_name(invoked_as);
        let Some(spec) = self.resolve(&normalized) else {
            return Err(CommandParseError::UnknownCommand { name: normalized });
        };
        let command = parse_command(spec.command, &tokens[1..], self)?;
        Ok(ParsedLine::Command(CommandInvocation {
            invoked_as: invoked_as.clone(),
            command,
        }))
    }

    /// Verify the catalog has no duplicate canonical names or aliases.
    pub fn validate(&self) -> Result<(), InteractionError> {
        let mut names = BTreeSet::new();
        for spec in self.specs.values() {
            if spec.name != spec.command.as_str() {
                return Err(InteractionError::invalid_config(format!(
                    "command {:?} has non-canonical name '{}'",
                    spec.command, spec.name
                )));
            }
            for name in std::iter::once(&spec.name).chain(&spec.aliases) {
                if !is_valid_command_name(name) {
                    return Err(InteractionError::invalid_config(format!(
                        "invalid command name '{name}'"
                    )));
                }
                if !names.insert(name) {
                    return Err(InteractionError::invalid_config(format!(
                        "duplicate command name or alias '{name}'"
                    )));
                }
            }
        }
        Ok(())
    }
}

fn normalize_name(name: &str) -> String {
    name.trim_start_matches('/').to_ascii_lowercase()
}

fn is_valid_command_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
}

fn spec(
    command: CommandName,
    aliases: &[&str],
    description: &str,
    input_hint: Option<&str>,
    category: CommandCategory,
    mutability: CommandMutability,
    availability: CommandAvailability,
) -> CommandSpec {
    CommandSpec {
        command,
        name: command.as_str().to_owned(),
        aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
        description: description.to_owned(),
        input_hint: input_hint.map(str::to_owned),
        category,
        mutability,
        availability,
    }
}

#[allow(clippy::too_many_lines)]
fn mvp_specs() -> Vec<CommandSpec> {
    let interaction = CommandAvailability {
        interaction: true,
        ..CommandAvailability::default()
    };
    vec![
        spec(
            CommandName::Help,
            &["commands"],
            "List available commands or show help for one command",
            Some("[command]"),
            CommandCategory::General,
            CommandMutability::ReadOnly,
            CommandAvailability::default(),
        ),
        spec(
            CommandName::Status,
            &["st"],
            "Show the current target, model, runs, approvals, usage, and budget",
            None,
            CommandCategory::General,
            CommandMutability::ReadOnly,
            interaction,
        ),
        spec(
            CommandName::Agents,
            &[],
            "List configured agents",
            None,
            CommandCategory::Configuration,
            CommandMutability::ReadOnly,
            CommandAvailability::default(),
        ),
        spec(
            CommandName::Agent,
            &["use"],
            "Select an agent by name or ID",
            Some("<name-or-id>"),
            CommandCategory::Configuration,
            CommandMutability::Mutating,
            interaction,
        ),
        spec(
            CommandName::New,
            &[],
            "Create and select a new interaction",
            Some("[title]"),
            CommandCategory::Interaction,
            CommandMutability::Mutating,
            CommandAvailability::default(),
        ),
        spec(
            CommandName::Resume,
            &[],
            "Load and select a durable interaction",
            Some("<conversation-id>"),
            CommandCategory::Interaction,
            CommandMutability::Mutating,
            CommandAvailability::default(),
        ),
        spec(
            CommandName::Runs,
            &[],
            "List recent and active runs for the interaction",
            None,
            CommandCategory::Execution,
            CommandMutability::ReadOnly,
            interaction,
        ),
        spec(
            CommandName::Inspect,
            &[],
            "Inspect one run, including artifacts and errors",
            Some("<run-id>"),
            CommandCategory::Execution,
            CommandMutability::ReadOnly,
            CommandAvailability::default(),
        ),
        spec(
            CommandName::Cancel,
            &["stop"],
            "Cancel the active turn, one run, or all visible work",
            Some("[run-id|all]"),
            CommandCategory::Execution,
            CommandMutability::Mutating,
            CommandAvailability {
                interaction: true,
                active_turn: true,
                pending_approval: false,
            },
        ),
        spec(
            CommandName::Approve,
            &[],
            "Approve one pending effect",
            Some("<approval-id>"),
            CommandCategory::Approval,
            CommandMutability::Mutating,
            CommandAvailability {
                interaction: true,
                active_turn: false,
                pending_approval: true,
            },
        ),
        spec(
            CommandName::Deny,
            &[],
            "Deny one pending effect",
            Some("<approval-id> [reason]"),
            CommandCategory::Approval,
            CommandMutability::Mutating,
            CommandAvailability {
                interaction: true,
                active_turn: false,
                pending_approval: true,
            },
        ),
        spec(
            CommandName::Model,
            &[],
            "Show or select the interaction model",
            Some("[id]"),
            CommandCategory::Configuration,
            CommandMutability::Conditional,
            interaction,
        ),
    ]
}

fn parse_command(
    name: CommandName,
    arguments: &[String],
    registry: &CommandRegistry,
) -> Result<InteractionCommand, CommandParseError> {
    match name {
        CommandName::Help => {
            at_most(arguments, 1, name, "zero or one command name")?;
            let command = arguments
                .first()
                .map(|argument| {
                    registry
                        .resolve(argument)
                        .map(|spec| spec.command)
                        .ok_or_else(|| CommandParseError::UnknownCommand {
                            name: normalize_name(argument),
                        })
                })
                .transpose()?;
            Ok(InteractionCommand::Help { command })
        }
        CommandName::Status => {
            none(arguments, name)?;
            Ok(InteractionCommand::Status)
        }
        CommandName::Agents => {
            none(arguments, name)?;
            Ok(InteractionCommand::Agents)
        }
        CommandName::Agent => {
            exactly(arguments, 1, name, "one agent name or ID")?;
            Ok(InteractionCommand::Agent {
                selector: arguments[0].clone(),
            })
        }
        CommandName::New => Ok(InteractionCommand::New {
            title: join_optional(arguments),
        }),
        CommandName::Resume => {
            exactly(arguments, 1, name, "one conversation ID")?;
            Ok(InteractionCommand::Resume {
                conversation_id: parse_id(name, "conversation ID", &arguments[0])?,
            })
        }
        CommandName::Runs => {
            none(arguments, name)?;
            Ok(InteractionCommand::Runs)
        }
        CommandName::Inspect => {
            exactly(arguments, 1, name, "one run ID")?;
            Ok(InteractionCommand::Inspect {
                run_id: parse_id(name, "run ID", &arguments[0])?,
            })
        }
        CommandName::Cancel => {
            at_most(arguments, 1, name, "zero or one run ID or 'all'")?;
            let target = match arguments.first().map(String::as_str) {
                None => CancelTarget::CurrentTurn,
                Some(value) if value.eq_ignore_ascii_case("all") => CancelTarget::All,
                Some(value) => CancelTarget::Run(parse_id(name, "run ID", value)?),
            };
            Ok(InteractionCommand::Cancel { target })
        }
        CommandName::Approve => {
            exactly(arguments, 1, name, "one approval ID")?;
            Ok(InteractionCommand::Approve {
                approval_id: parse_id(name, "approval ID", &arguments[0])?,
            })
        }
        CommandName::Deny => {
            at_least(arguments, 1, name, "an approval ID")?;
            Ok(InteractionCommand::Deny {
                approval_id: parse_id(name, "approval ID", &arguments[0])?,
                reason: join_optional(&arguments[1..]),
            })
        }
        CommandName::Model => {
            at_most(arguments, 1, name, "zero or one model ID")?;
            Ok(InteractionCommand::Model {
                model: arguments.first().cloned(),
            })
        }
    }
}

fn parse_id<T: std::str::FromStr>(
    command: CommandName,
    kind: &str,
    value: &str,
) -> Result<T, CommandParseError> {
    value
        .parse()
        .map_err(|_| CommandParseError::InvalidIdentifier {
            command: command.as_str().to_owned(),
            kind: kind.to_owned(),
            value: value.to_owned(),
        })
}

fn none(arguments: &[String], command: CommandName) -> Result<(), CommandParseError> {
    at_most(arguments, 0, command, "no arguments")
}

fn exactly(
    arguments: &[String],
    count: usize,
    command: CommandName,
    expected: &str,
) -> Result<(), CommandParseError> {
    at_least(arguments, count, command, expected)?;
    at_most(arguments, count, command, expected)
}

fn at_least(
    arguments: &[String],
    count: usize,
    command: CommandName,
    expected: &str,
) -> Result<(), CommandParseError> {
    if arguments.len() < count {
        return Err(CommandParseError::MissingArgument {
            command: command.as_str().to_owned(),
            expected: expected.to_owned(),
        });
    }
    Ok(())
}

fn at_most(
    arguments: &[String],
    count: usize,
    command: CommandName,
    expected: &str,
) -> Result<(), CommandParseError> {
    if arguments.len() > count {
        return Err(CommandParseError::TooManyArguments {
            command: command.as_str().to_owned(),
            expected: expected.to_owned(),
        });
    }
    Ok(())
}

fn join_optional(arguments: &[String]) -> Option<String> {
    (!arguments.is_empty()).then(|| arguments.join(" "))
}

fn tokenize(input: &str) -> Result<Vec<String>, CommandParseError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;

    for character in input.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            started = true;
            continue;
        }
        if character == '\\' {
            escaped = true;
            started = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            } else {
                current.push(character);
            }
            started = true;
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            started = true;
        } else if character.is_whitespace() {
            if started {
                tokens.push(std::mem::take(&mut current));
                started = false;
            }
        } else {
            current.push(character);
            started = true;
        }
    }

    if escaped {
        return Err(CommandParseError::IncompleteEscape);
    }
    if quote.is_some() {
        return Err(CommandParseError::UnterminatedQuote);
    }
    if started {
        tokens.push(current);
    }
    Ok(tokens)
}

/// Compact agent projection returned by `/agents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTargetView {
    /// Stable agent identity.
    pub agent_id: AgentId,
    /// Configured name.
    pub name: String,
    /// Durable lifecycle state.
    pub state: String,
    /// Whether this agent is ready to accept prompts.
    pub ready: bool,
}

/// Compact run projection returned by `/runs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummaryView {
    /// Stable run identity.
    pub run_id: RunId,
    /// Owning agent.
    pub agent_id: AgentId,
    /// Durable lifecycle state label.
    pub state: String,
    /// Optional safe summary.
    pub summary: Option<String>,
}

/// Structured run inspection result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDetailView {
    /// Compact run fields.
    pub run: RunSummaryView,
    /// Produced artifact identities rendered as stable strings by adapters.
    pub artifacts: Vec<String>,
    /// Safe error detail, when present.
    pub error: Option<String>,
}

/// Typed output from domain command execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandOutput {
    /// Available command catalog.
    Help {
        /// Specs valid in the current context.
        commands: Vec<CommandSpec>,
    },
    /// Current interaction configuration and active handles.
    Status {
        /// Selected interaction.
        interaction: InteractionSummary,
        /// Active turns.
        active_turns: Vec<TurnHandle>,
        /// Pending approval IDs.
        pending_approvals: Vec<ApprovalId>,
    },
    /// Configured agent targets.
    Agents {
        /// Agent projections.
        agents: Vec<AgentTargetView>,
    },
    /// Session target changed.
    TargetChanged {
        /// New target.
        target: InteractionTarget,
    },
    /// A new interaction was created and selected.
    InteractionCreated {
        /// New interaction.
        interaction: InteractionSummary,
    },
    /// A durable interaction was loaded and selected.
    InteractionResumed {
        /// Loaded interaction.
        interaction: InteractionSummary,
    },
    /// Recent or active runs.
    Runs {
        /// Run projections.
        runs: Vec<RunSummaryView>,
    },
    /// Detailed run projection.
    RunInspected {
        /// Run details.
        run: RunDetailView,
    },
    /// Cancellation was accepted for durable processing.
    CancellationRequested {
        /// Turn being cancelled, when cancellation is turn-scoped.
        turn_id: Option<InteractionTurnId>,
        /// Known affected runs.
        run_ids: Vec<RunId>,
    },
    /// An approval decision was durably recorded.
    ApprovalResolved {
        /// Real approval identity.
        approval_id: ApprovalId,
        /// Recorded decision.
        decision: ApprovalDecision,
    },
    /// Current or newly selected model.
    Model {
        /// Effective model override.
        model: Option<String>,
        /// Whether this output reflects a configuration mutation.
        changed: bool,
    },
}

/// Surface-neutral request passed to a command executor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRequest {
    /// Parsed command invocation.
    pub invocation: CommandInvocation,
    /// Dynamic availability context used for authorization and routing.
    pub context: CommandContext,
    /// Initiating client context.
    pub client_context: ClientContext,
}

/// Runtime boundary for executing typed domain commands.
#[async_trait]
pub trait CommandExecutor: Send + Sync {
    /// Execute a parsed, authorized command and return structured output.
    async fn execute(&self, request: CommandRequest) -> Result<CommandOutput, InteractionError>;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::InteractionConfig;

    fn mutable_context() -> CommandContext {
        CommandContext {
            conversation_id: Some(ConversationId::new()),
            has_active_turn: true,
            pending_approval_count: 1,
            can_mutate: true,
        }
    }

    fn command(registry: &CommandRegistry, input: &str) -> InteractionCommand {
        let parsed = registry.parse(input).expect("parse command");
        let ParsedLine::Command(invocation) = parsed else {
            panic!("expected command")
        };
        invocation.command
    }

    #[test]
    fn catalog_contains_every_required_mvp_command() {
        let registry = CommandRegistry::mvp();
        assert_eq!(registry.specs().len(), 12);
        for name in [
            "help", "status", "agents", "agent", "new", "resume", "runs", "inspect", "cancel",
            "approve", "deny", "model",
        ] {
            assert!(registry.resolve(name).is_some(), "missing /{name}");
        }
        assert!(registry.validate().is_ok());
    }

    #[test]
    fn aliases_resolve_case_insensitively() {
        let registry = CommandRegistry::mvp();
        assert_eq!(
            registry.resolve("/COMMANDS").map(|spec| spec.command),
            Some(CommandName::Help)
        );
        assert_eq!(
            registry.resolve("use").map(|spec| spec.command),
            Some(CommandName::Agent)
        );
        assert_eq!(
            command(&registry, "/STOP"),
            InteractionCommand::Cancel {
                target: CancelTarget::CurrentTurn,
            }
        );
    }

    #[test]
    fn ordinary_prompt_is_preserved_exactly() {
        let registry = CommandRegistry::mvp();
        let input = "  explain /help literally  ";
        assert_eq!(
            registry.parse(input),
            Ok(ParsedLine::Prompt(input.to_owned()))
        );
    }

    #[test]
    fn empty_and_unknown_commands_are_actionable_errors() {
        let registry = CommandRegistry::mvp();
        assert_eq!(registry.parse("/"), Err(CommandParseError::MissingCommand));
        assert_eq!(
            registry.parse("/wat"),
            Err(CommandParseError::UnknownCommand {
                name: "wat".to_owned(),
            })
        );
    }

    #[test]
    fn quoted_titles_and_denial_reasons_are_preserved() {
        let registry = CommandRegistry::mvp();
        assert_eq!(
            command(&registry, "/new \"Treasury review\" phase 2"),
            InteractionCommand::New {
                title: Some("Treasury review phase 2".to_owned()),
            }
        );
        let approval = ApprovalId::new();
        assert_eq!(
            command(&registry, &format!("/deny {approval} 'amount too high'")),
            InteractionCommand::Deny {
                approval_id: approval,
                reason: Some("amount too high".to_owned()),
            }
        );
    }

    #[test]
    fn tokenizer_supports_escaped_spaces_and_rejects_bad_quotes() {
        let registry = CommandRegistry::mvp();
        assert_eq!(
            command(&registry, "/agent treasury\\ bot"),
            InteractionCommand::Agent {
                selector: "treasury bot".to_owned(),
            }
        );
        assert_eq!(
            registry.parse("/new 'open"),
            Err(CommandParseError::UnterminatedQuote)
        );
        assert_eq!(
            registry.parse("/new trailing\\"),
            Err(CommandParseError::IncompleteEscape)
        );
    }

    #[test]
    fn typed_ids_are_parsed_for_resume_inspect_and_approval() {
        let registry = CommandRegistry::mvp();
        let conversation_id = ConversationId::new();
        let run_id = RunId::new();
        let approval_id = ApprovalId::new();
        assert_eq!(
            command(&registry, &format!("/resume {conversation_id}")),
            InteractionCommand::Resume { conversation_id }
        );
        assert_eq!(
            command(&registry, &format!("/inspect {run_id}")),
            InteractionCommand::Inspect { run_id }
        );
        assert_eq!(
            command(&registry, &format!("/approve {approval_id}")),
            InteractionCommand::Approve { approval_id }
        );
    }

    #[test]
    fn invalid_ids_and_arity_are_rejected() {
        let registry = CommandRegistry::mvp();
        assert!(matches!(
            registry.parse("/inspect nope"),
            Err(CommandParseError::InvalidIdentifier { .. })
        ));
        assert!(matches!(
            registry.parse("/agent"),
            Err(CommandParseError::MissingArgument { .. })
        ));
        assert!(matches!(
            registry.parse("/status extra"),
            Err(CommandParseError::TooManyArguments { .. })
        ));
    }

    #[test]
    fn cancel_targets_are_unambiguous() {
        let registry = CommandRegistry::mvp();
        let run_id = RunId::new();
        assert_eq!(
            command(&registry, "/cancel"),
            InteractionCommand::Cancel {
                target: CancelTarget::CurrentTurn,
            }
        );
        assert_eq!(
            command(&registry, "/cancel all"),
            InteractionCommand::Cancel {
                target: CancelTarget::All,
            }
        );
        assert_eq!(
            command(&registry, &format!("/cancel {run_id}")),
            InteractionCommand::Cancel {
                target: CancelTarget::Run(run_id),
            }
        );
    }

    #[test]
    fn help_uses_registry_for_alias_resolution() {
        let registry = CommandRegistry::mvp();
        assert_eq!(
            command(&registry, "/help stop"),
            InteractionCommand::Help {
                command: Some(CommandName::Cancel),
            }
        );
    }

    #[test]
    fn availability_filters_context_and_mutability() {
        let registry = CommandRegistry::mvp();
        let all = registry.available(&mutable_context());
        assert_eq!(all.len(), 12);

        let read_only = CommandContext {
            conversation_id: None,
            has_active_turn: false,
            pending_approval_count: 0,
            can_mutate: false,
        };
        let names: BTreeSet<_> = registry
            .available(&read_only)
            .into_iter()
            .map(|spec| spec.command)
            .collect();
        assert_eq!(
            names,
            BTreeSet::from([CommandName::Help, CommandName::Agents, CommandName::Inspect])
        );
    }

    #[tokio::test]
    async fn executor_trait_is_surface_neutral() {
        struct Fake;

        #[async_trait]
        impl CommandExecutor for Fake {
            async fn execute(
                &self,
                request: CommandRequest,
            ) -> Result<CommandOutput, InteractionError> {
                Ok(CommandOutput::Model {
                    model: match request.invocation.command {
                        InteractionCommand::Model { model } => model,
                        _ => None,
                    },
                    changed: true,
                })
            }
        }

        let registry = CommandRegistry::mvp();
        let ParsedLine::Command(invocation) = registry.parse("/model opus").expect("parse") else {
            panic!("expected command")
        };
        let output = Fake
            .execute(CommandRequest {
                invocation,
                context: mutable_context(),
                client_context: ClientContext::new(PathBuf::from("/tmp")).expect("context"),
            })
            .await
            .expect("execute");
        assert_eq!(
            output,
            CommandOutput::Model {
                model: Some("opus".to_owned()),
                changed: true,
            }
        );
    }

    #[test]
    fn command_config_views_are_serializable() {
        let config = InteractionConfig::new(InteractionTarget::Auto);
        let value = serde_json::to_value(config).expect("serialize config");
        assert_eq!(value["target"]["kind"], "auto");
    }
}

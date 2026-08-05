//! `polkagent chat` — durable, line-oriented terminal conversations.

use std::io::{IsTerminal as _, Write as _};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use polkagent_core::{AgentId, ApprovalId, ConversationId, RunId};
use polkagent_interaction::{
    AgentTargetView, CancelTarget, ClientContext, CommandContext, CommandExecutor, CommandName,
    CommandOutput, CommandRegistry, CommandRequest, CreateInteractionRequest, InteractionCommand,
    InteractionCommandRuntime, InteractionConfig, InteractionContent, InteractionError,
    InteractionErrorCode, InteractionEvent, InteractionOverrides, InteractionService,
    InteractionSummary, InteractionTarget, ParsedLine, PromptRequest, RunDetailView,
    RunSummaryView, ServiceCommandExecutor, StartedTurn, StreamError, SubscriptionRequest,
    TranscriptRequest, TurnHandle, UsageView,
};
use polkagent_runtime::{
    AdapterPolicy, PolkagentRuntime, RuntimeFactory, RuntimeOptions, WarningCode,
};
use polkagent_store_sqlite::{SqlitePool, SqliteRunStore};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader, Lines, Stdin};

use crate::cli::ChatCmd;

const SUPPORTED_COMMANDS: [CommandName; 5] = [
    CommandName::Help,
    CommandName::Status,
    CommandName::Cancel,
    CommandName::New,
    CommandName::Resume,
];

/// Run a durable terminal chat session.
#[allow(clippy::too_many_lines)]
pub async fn run(cmd: &ChatCmd, pool: &SqlitePool, config_path: Option<&Path>) -> Result<()> {
    let runtime = build_runtime(pool, config_path).await?;
    let agent = resolve_active_agent(&runtime, &cmd.agent)?;
    report_runtime(&runtime);

    let service: Arc<dyn InteractionService> = runtime.interactions().clone();
    let registry = CommandRegistry::mvp();
    let command_runtime: Arc<dyn InteractionCommandRuntime> = Arc::new(ChatCommandRuntime {
        service: Arc::clone(&service),
        agent_id: agent.agent_id,
    });
    let executor =
        ServiceCommandExecutor::new(registry.clone(), Arc::clone(&service), command_runtime)
            .context("composing terminal slash commands")?;
    let client_context = terminal_client_context(&runtime)?;

    let interaction = if let Some(raw_id) = &cmd.resume {
        let conversation_id = raw_id
            .parse::<ConversationId>()
            .with_context(|| format!("invalid conversation ID supplied to --resume: {raw_id}"))?;
        let interaction = service
            .load_interaction(conversation_id)
            .await
            .context("loading durable chat interaction")?;
        validate_agent_target(&interaction, agent.agent_id)?;
        interaction
    } else {
        service
            .new_interaction(CreateInteractionRequest {
                title: cmd
                    .title
                    .clone()
                    .or_else(|| Some(format!("terminal chat · {}", agent.name))),
                config: InteractionConfig::new(InteractionTarget::Agent(agent.agent_id)),
                client_context: client_context.clone(),
            })
            .await
            .context("creating durable chat interaction")?
    };

    let mut session = ChatSession {
        service,
        registry,
        executor,
        client_context,
        agent_id: agent.agent_id,
        agent_name: agent.name,
        conversation_id: interaction.conversation_id,
    };
    session.announce_session();

    if cmd.resume.is_some() {
        session.render_transcript().await?;
    }

    if std::io::stdin().is_terminal() {
        session.run_interactive().await
    } else {
        session.run_non_interactive().await
    }
}

async fn build_runtime(pool: &SqlitePool, config_path: Option<&Path>) -> Result<PolkagentRuntime> {
    let workdir = std::env::current_dir().context("resolving terminal chat workdir")?;
    let mut options = RuntimeOptions::new(workdir);
    options.config_path = config_path.map(Path::to_path_buf);
    options.database_path = Some(pool.path().to_path_buf());
    options.disable_harness = true;
    // Match the established local-first CLI path while reporting simulation
    // explicitly on stderr. Explicit provider/model selection is intentionally
    // absent from this surface until the durable interaction service supports it.
    options.adapter_policy = AdapterPolicy::AllowSimulated;
    RuntimeFactory::build(options)
        .await
        .context("building shared terminal chat runtime")
}

fn report_runtime(runtime: &PolkagentRuntime) {
    let readiness = runtime.readiness();
    if readiness
        .warnings
        .iter()
        .any(|warning| warning.code == WarningCode::SimulatedExecutor)
    {
        eprintln!("chat runtime: no concrete model executor found; using simulated responses");
    } else {
        eprintln!("chat runtime: {}", readiness.executor.detail);
    }
    for warning in &readiness.warnings {
        if warning.code != WarningCode::SimulatedExecutor {
            eprintln!("chat runtime warning: {}", warning.message);
        }
    }
}

struct SelectedAgent {
    agent_id: AgentId,
    name: String,
}

fn resolve_active_agent(runtime: &PolkagentRuntime, selector: &str) -> Result<SelectedAgent> {
    let row = SqliteRunStore::new(runtime.pool().clone())
        .get_agent_by_name_or_id(selector)
        .with_context(|| format!("active agent not found: {selector}"))?;
    if row.state != "active" {
        anyhow::bail!(
            "agent '{}' is in state '{}' and cannot accept terminal prompts",
            row.name,
            row.state
        );
    }
    let agent_id = row
        .id
        .parse::<AgentId>()
        .with_context(|| format!("invalid stored agent ID: {}", row.id))?;
    Ok(SelectedAgent {
        agent_id,
        name: row.name,
    })
}

fn terminal_client_context(runtime: &PolkagentRuntime) -> Result<ClientContext> {
    let mut context = ClientContext::new(runtime.workdir().to_path_buf())?;
    context.client_name = Some("terminal".to_owned());
    Ok(context)
}

fn validate_agent_target(interaction: &InteractionSummary, agent_id: AgentId) -> Result<()> {
    match &interaction.config.target {
        InteractionTarget::Agent(target) if *target == agent_id => Ok(()),
        InteractionTarget::Agent(target) => anyhow::bail!(
            "conversation {} targets agent {target}, not selected agent {agent_id}",
            interaction.conversation_id
        ),
        InteractionTarget::Group(_) => {
            anyhow::bail!("terminal chat does not support group interactions")
        }
        InteractionTarget::Auto => {
            anyhow::bail!("terminal chat requires a durable single-agent interaction target")
        }
    }
}

struct ChatSession {
    service: Arc<dyn InteractionService>,
    registry: CommandRegistry,
    executor: ServiceCommandExecutor,
    client_context: ClientContext,
    agent_id: AgentId,
    agent_name: String,
    conversation_id: ConversationId,
}

impl ChatSession {
    fn announce_session(&self) {
        eprintln!("chat agent: {} ({})", self.agent_name, self.agent_id);
        eprintln!("chat session: {}", self.conversation_id);
    }

    async fn run_non_interactive(&mut self) -> Result<()> {
        let mut input = String::new();
        tokio::io::stdin()
            .read_to_string(&mut input)
            .await
            .context("reading prompt from stdin")?;
        let input = input.trim_end_matches(['\r', '\n']);
        if input.trim().is_empty() {
            return Ok(());
        }
        if input.trim_start().starts_with('/') {
            self.execute_command_line(input, false).await
        } else {
            match self.run_turn_non_interactive(input.to_owned()).await? {
                TurnTerminal::Completed | TurnTerminal::Cancelled => Ok(()),
                TurnTerminal::Failed(error) => anyhow::bail!("interaction turn failed: {error}"),
                TurnTerminal::TimedOut => anyhow::bail!("interaction turn timed out"),
            }
        }
    }

    async fn run_interactive(&mut self) -> Result<()> {
        eprintln!("line mode: enter a blank line to submit; /help lists supported commands");
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        let mut prompt = Vec::new();
        loop {
            eprint!("{}", if prompt.is_empty() { "you> " } else { "...  " });
            std::io::stderr().flush().context("flushing chat prompt")?;
            let line = tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal.context("waiting for Ctrl-C")?;
                    eprintln!("\nchat session retained: {}", self.conversation_id);
                    return Ok(());
                }
                line = lines.next_line() => line.context("reading terminal input")?,
            };
            let Some(line) = line else {
                if !prompt.is_empty() {
                    let text = prompt.join("\n");
                    self.finish_interactive_turn(text, &mut lines).await?;
                }
                eprintln!();
                return Ok(());
            };

            if line.is_empty() {
                if !prompt.is_empty() {
                    let text = std::mem::take(&mut prompt).join("\n");
                    self.finish_interactive_turn(text, &mut lines).await?;
                }
            } else if prompt.is_empty() && line.trim_start().starts_with('/') {
                if let Err(error) = self.execute_command_line(&line, false).await {
                    eprintln!("command error: {error:#}");
                }
            } else {
                prompt.push(line);
            }
        }
    }

    async fn finish_interactive_turn(
        &mut self,
        prompt: String,
        lines: &mut Lines<BufReader<Stdin>>,
    ) -> Result<()> {
        match self.run_turn_interactive(prompt, lines).await? {
            TurnTerminal::Completed => {}
            TurnTerminal::Cancelled => eprintln!("turn cancelled; session is still active"),
            TurnTerminal::Failed(error) => eprintln!("turn failed: {error}"),
            TurnTerminal::TimedOut => eprintln!("turn timed out"),
        }
        Ok(())
    }

    async fn start_turn(&self, prompt: String) -> Result<StartedTurn> {
        self.service
            .prompt(PromptRequest {
                turn_id: None,
                conversation_id: self.conversation_id,
                content: vec![InteractionContent::Text { text: prompt }],
                config_overrides: InteractionOverrides::default(),
                client_context: self.client_context.clone(),
            })
            .await
            .context("starting durable interaction turn")
    }

    async fn run_turn_non_interactive(&mut self, prompt: String) -> Result<TurnTerminal> {
        let mut started = self.start_turn(prompt).await?;
        eprintln!("chat turn: {}", started.handle.turn_id);
        let mut renderer = TurnRenderer::default();
        loop {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal.context("waiting for Ctrl-C")?;
                    self.service.cancel_turn(started.handle.turn_id).await
                        .context("cancelling active chat turn")?;
                    eprintln!("cancellation requested for turn {}", started.handle.turn_id);
                }
                event = started.events.recv() => {
                    match event {
                        Err(StreamError::Lagged { last_seen_sequence, resume_after_sequence }) => {
                            self.recover_lag(&mut started, last_seen_sequence, resume_after_sequence).await?;
                        }
                        event => {
                            if let Some(terminal) = renderer.render(event).context("rendering interaction event")? {
                                return Ok(terminal);
                            }
                        }
                    }
                }
            }
        }
    }

    async fn run_turn_interactive(
        &mut self,
        prompt: String,
        lines: &mut Lines<BufReader<Stdin>>,
    ) -> Result<TurnTerminal> {
        let mut started = self.start_turn(prompt).await?;
        eprintln!(
            "chat turn: {} (Ctrl-C or /cancel stops this turn)",
            started.handle.turn_id
        );
        let mut renderer = TurnRenderer::default();
        let mut stdin_open = true;
        loop {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => {
                    signal.context("waiting for Ctrl-C")?;
                    self.service.cancel_turn(started.handle.turn_id).await
                        .context("cancelling active chat turn")?;
                    eprintln!("cancellation requested for turn {}", started.handle.turn_id);
                }
                line = lines.next_line(), if stdin_open => {
                    match line.context("reading terminal input during active turn")? {
                        Some(line) if is_cancel_line(&line) => {
                            if let Err(error) = self.execute_command_line(&line, true).await {
                                eprintln!("command error: {error:#}");
                            }
                        }
                        Some(line) if line.trim().is_empty() => {}
                        Some(_) => eprintln!("the turn is active; only /cancel is accepted until it finishes"),
                        None => stdin_open = false,
                    }
                }
                event = started.events.recv() => {
                    match event {
                        Err(StreamError::Lagged { last_seen_sequence, resume_after_sequence }) => {
                            self.recover_lag(&mut started, last_seen_sequence, resume_after_sequence).await?;
                        }
                        event => {
                            if let Some(terminal) = renderer.render(event).context("rendering interaction event")? {
                                return Ok(terminal);
                            }
                        }
                    }
                }
            }
        }
    }

    async fn recover_lag(
        &self,
        started: &mut StartedTurn,
        last_seen_sequence: Option<u64>,
        resume_after_sequence: u64,
    ) -> Result<()> {
        eprintln!(
            "interaction stream lagged at sequence {resume_after_sequence}; replaying durable events"
        );
        started.events = self
            .service
            .subscribe(SubscriptionRequest {
                conversation_id: started.handle.conversation_id,
                turn_id: Some(started.handle.turn_id),
                after_sequence: last_seen_sequence,
                capacity: 256,
            })
            .await
            .context("resubscribing to durable interaction events")?;
        Ok(())
    }

    async fn execute_command_line(&mut self, line: &str, has_active_turn: bool) -> Result<()> {
        refuse_unregistered_configuration_command(line)?;
        let ParsedLine::Command(invocation) = self.registry.parse(line)? else {
            anyhow::bail!("expected a slash command");
        };
        refuse_unsupported_command(&invocation.command)?;
        let output = self
            .executor
            .execute(CommandRequest {
                invocation,
                context: CommandContext {
                    conversation_id: Some(self.conversation_id),
                    has_active_turn,
                    pending_approval_count: 0,
                    can_mutate: true,
                },
                client_context: self.client_context.clone(),
            })
            .await?;
        self.apply_command_output(output).await
    }

    async fn apply_command_output(&mut self, output: CommandOutput) -> Result<()> {
        match output {
            CommandOutput::Help { commands } => {
                println!("Supported terminal chat commands:");
                let full_catalog = commands.len() > 1;
                let mut commands = commands
                    .into_iter()
                    .filter(|spec| SUPPORTED_COMMANDS.contains(&spec.command))
                    .collect::<Vec<_>>();
                if full_catalog
                    && !commands
                        .iter()
                        .any(|spec| spec.command == CommandName::Cancel)
                {
                    if let Some(cancel) = self.registry.resolve("cancel") {
                        commands.push(cancel.clone());
                    }
                }
                commands.sort_by_key(|spec| spec.command);
                for command in commands {
                    let (input_hint, description) = match command.command {
                        CommandName::Status => (
                            None,
                            "Show durable session, target, turn, and active-work status",
                        ),
                        CommandName::Cancel => (
                            Some("[all]"),
                            "Cancel the current durable turn (or all active turns)",
                        ),
                        _ => (command.input_hint.as_deref(), command.description.as_str()),
                    };
                    let hint = input_hint.map_or(String::new(), |hint| format!(" {hint}"));
                    println!("  /{}{hint} — {description}", command.name);
                }
                println!(
                    "Agent/model/provider/autonomy, approval, run-inspection, and group commands are not available in terminal chat."
                );
            }
            CommandOutput::Status {
                interaction,
                active_turns,
                pending_approvals: _,
            } => {
                println!("session: {}", interaction.conversation_id);
                println!("target: {}", display_target(&interaction.config.target));
                println!("state: {:?}", interaction.state);
                println!("turns: {}", interaction.turn_count);
                println!("active turns: {}", active_turns.len());
                println!("pending approvals: visibility unavailable in terminal chat");
                println!(
                    "model/provider: {}/{} (read-only status; selection is unsupported here)",
                    interaction
                        .config
                        .model
                        .as_deref()
                        .unwrap_or("runtime default"),
                    interaction
                        .config
                        .provider
                        .as_deref()
                        .unwrap_or("runtime default")
                );
            }
            CommandOutput::CancellationRequested { turn_id, run_ids } => {
                let target = turn_id.map_or_else(|| "visible work".to_owned(), |id| id.to_string());
                eprintln!(
                    "cancellation requested for {target} ({} linked run(s))",
                    run_ids.len()
                );
            }
            CommandOutput::InteractionCreated { interaction } => {
                validate_agent_target(&interaction, self.agent_id)?;
                self.conversation_id = interaction.conversation_id;
                self.announce_session();
            }
            CommandOutput::InteractionResumed { interaction } => {
                validate_agent_target(&interaction, self.agent_id)?;
                self.conversation_id = interaction.conversation_id;
                self.announce_session();
                self.render_transcript().await?;
            }
            _ => anyhow::bail!("terminal chat received an unsupported command result"),
        }
        std::io::stdout().flush().context("flushing chat output")?;
        Ok(())
    }

    async fn render_transcript(&self) -> Result<()> {
        let mut offset = 0_u32;
        loop {
            let turns = self
                .service
                .load_transcript(TranscriptRequest {
                    conversation_id: self.conversation_id,
                    limit: 1_000,
                    offset,
                })
                .await
                .context("loading durable turn-correlated chat transcript")?;
            if turns.is_empty() {
                break;
            }
            let page_len = u32::try_from(turns.len())
                .context("chat transcript page exceeds the supported size")?;
            for turn in turns {
                println!("user> {}", turn.user_text);
                if let Some(text) = turn.assistant_text {
                    println!("assistant> {text}");
                }
            }
            offset = offset
                .checked_add(page_len)
                .context("chat transcript offset overflowed")?;
            if page_len < 1_000 {
                break;
            }
        }
        std::io::stdout()
            .flush()
            .context("flushing durable transcript")?;
        Ok(())
    }
}

fn display_target(target: &InteractionTarget) -> String {
    match target {
        InteractionTarget::Agent(id) => format!("agent {id}"),
        InteractionTarget::Group(id) => format!("group {id}"),
        InteractionTarget::Auto => "automatic".to_owned(),
    }
}

fn is_cancel_line(line: &str) -> bool {
    matches!(line.split_whitespace().next(), Some("/cancel" | "/stop"))
}

fn refuse_unregistered_configuration_command(line: &str) -> Result<()> {
    let command = line
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match command.as_str() {
        "/provider" => anyhow::bail!(
            "terminal chat does not support provider selection; configure the runtime and restart"
        ),
        "/autonomy" => anyhow::bail!(
            "terminal chat does not support autonomy changes; configure the agent outside chat"
        ),
        _ => Ok(()),
    }
}

fn refuse_unsupported_command(command: &InteractionCommand) -> Result<()> {
    match command {
        InteractionCommand::Help {
            command: Some(name),
        } if !SUPPORTED_COMMANDS.contains(name) => {
            anyhow::bail!("/{} is not supported by terminal chat", name.as_str())
        }
        InteractionCommand::Agents | InteractionCommand::Agent { .. } => anyhow::bail!(
            "terminal chat does not change agents; start a new process with --agent <name-or-id>"
        ),
        InteractionCommand::Runs | InteractionCommand::Inspect { .. } => anyhow::bail!(
            "terminal chat does not expose run inspection; use the top-level inspect commands"
        ),
        InteractionCommand::Cancel {
            target: CancelTarget::Run(_),
        } => anyhow::bail!(
            "terminal chat cancels the current turn only; run-scoped cancellation is unsupported"
        ),
        InteractionCommand::Approve { .. } | InteractionCommand::Deny { .. } => {
            anyhow::bail!("terminal chat cannot resolve approvals; use the durable inbox commands")
        }
        InteractionCommand::Model { .. } => anyhow::bail!(
            "terminal chat does not support model selection; configure the agent and restart"
        ),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TurnTerminal {
    Completed,
    Cancelled,
    Failed(String),
    TimedOut,
}

#[derive(Default)]
struct TurnRenderer {
    streamed_text: String,
    line_open: bool,
    usage: Option<UsageView>,
}

impl TurnRenderer {
    fn render(
        &mut self,
        event: std::result::Result<polkagent_interaction::InteractionEventEnvelope, StreamError>,
    ) -> Result<Option<TurnTerminal>> {
        let event = match event {
            Ok(envelope) => envelope.event,
            Err(StreamError::Lagged {
                last_seen_sequence,
                resume_after_sequence,
            }) => anyhow::bail!(
                "interaction stream lagged after {last_seen_sequence:?}; durable replay is required after sequence {resume_after_sequence}"
            ),
            Err(StreamError::Closed) => {
                anyhow::bail!("interaction event stream closed before a terminal event")
            }
            Err(StreamError::Backend(error)) => {
                anyhow::bail!("interaction event backend failed: {error}")
            }
        };
        match event {
            InteractionEvent::TurnStarted { target, runs } => {
                eprintln!(
                    "turn started: {} ({} linked run(s))",
                    display_target(&target),
                    runs.len()
                );
            }
            InteractionEvent::AgentMessageDelta { text, .. } => {
                print!("{text}");
                std::io::stdout()
                    .flush()
                    .context("streaming assistant text")?;
                self.streamed_text.push_str(&text);
                self.line_open = true;
            }
            InteractionEvent::ThoughtDelta { text, .. } => {
                eprintln!("[thought] {text}");
            }
            InteractionEvent::ToolCallStarted { call }
            | InteractionEvent::ToolCallUpdated { call } => {
                eprintln!("[tool] {}: {:?}", call.title, call.status);
            }
            InteractionEvent::PlanUpdated { entries } => {
                eprintln!("[plan] {} item(s) updated", entries.len());
            }
            InteractionEvent::ApprovalRequested { request } => {
                eprintln!(
                    "[approval required] {} — use `polkagent inbox` outside terminal chat",
                    request.title
                );
            }
            InteractionEvent::ApprovalResolved {
                approval_id,
                decision,
            } => {
                eprintln!("[approval] {approval_id}: {decision:?}");
            }
            InteractionEvent::UsageUpdated { usage } => {
                Self::report_usage(&usage);
                self.usage = Some(usage);
            }
            InteractionEvent::RunStateChanged { run_id, state } => {
                eprintln!("[run {run_id}] {state:?}");
            }
            InteractionEvent::TurnCompleted { result } => {
                let suffix = result
                    .text
                    .strip_prefix(&self.streamed_text)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "durable completed text does not match streamed assistant output"
                        )
                    })?;
                if !suffix.is_empty() {
                    print!("{suffix}");
                    self.line_open = true;
                }
                self.finish_line()?;
                if self.usage.as_ref() != Some(&result.usage) {
                    Self::report_usage(&result.usage);
                }
                return Ok(Some(TurnTerminal::Completed));
            }
            InteractionEvent::TurnFailed { error } => {
                self.finish_line()?;
                return Ok(Some(TurnTerminal::Failed(error.to_string())));
            }
            InteractionEvent::TurnCancelled { reason } => {
                self.finish_line()?;
                if let Some(reason) = reason {
                    eprintln!("cancellation reason: {reason}");
                }
                return Ok(Some(TurnTerminal::Cancelled));
            }
            InteractionEvent::TurnTimedOut => {
                self.finish_line()?;
                return Ok(Some(TurnTerminal::TimedOut));
            }
        }
        Ok(None)
    }

    fn finish_line(&mut self) -> Result<()> {
        if self.line_open {
            println!();
            self.line_open = false;
        }
        std::io::stdout().flush().context("flushing assistant text")
    }

    fn report_usage(usage: &UsageView) {
        let cost = usage
            .cost_usd
            .map_or_else(|| "unknown".to_owned(), |cost| format!("${cost:.6}"));
        eprintln!(
            "[usage] input={} output={} total={} cost={cost}",
            usage.input_tokens,
            usage.output_tokens,
            usage.total_tokens()
        );
    }
}

struct ChatCommandRuntime {
    service: Arc<dyn InteractionService>,
    agent_id: AgentId,
}

#[async_trait]
impl InteractionCommandRuntime for ChatCommandRuntime {
    async fn default_interaction_config(&self) -> Result<InteractionConfig, InteractionError> {
        Ok(InteractionConfig::new(InteractionTarget::Agent(
            self.agent_id,
        )))
    }

    async fn list_agents(&self) -> Result<Vec<AgentTargetView>, InteractionError> {
        Err(chat_unsupported("agent discovery"))
    }

    async fn resolve_agent(&self, _selector: &str) -> Result<AgentTargetView, InteractionError> {
        Err(chat_unsupported("agent selection"))
    }

    async fn list_runs(
        &self,
        _conversation_id: ConversationId,
    ) -> Result<Vec<RunSummaryView>, InteractionError> {
        Err(chat_unsupported("run listing"))
    }

    async fn inspect_run(&self, _run_id: RunId) -> Result<RunDetailView, InteractionError> {
        Err(chat_unsupported("run inspection"))
    }

    async fn cancel_run(&self, _run_id: RunId) -> Result<(), InteractionError> {
        Err(chat_unsupported("run-scoped cancellation"))
    }

    async fn active_turns(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<TurnHandle>, InteractionError> {
        Ok(self
            .service
            .list_turns(conversation_id)
            .await?
            .into_iter()
            .filter(|turn| !turn.state.is_terminal())
            .map(|turn| turn.handle)
            .collect())
    }

    async fn pending_approvals(
        &self,
        _conversation_id: ConversationId,
    ) -> Result<Vec<ApprovalId>, InteractionError> {
        Ok(Vec::new())
    }
}

fn chat_unsupported(capability: &str) -> InteractionError {
    InteractionError::new(
        InteractionErrorCode::Unsupported,
        format!("terminal chat does not support {capability}"),
    )
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;
    use crate::cli::{Cli, Commands};

    #[test]
    fn chat_cli_parses_agent_resume_and_title_conflict() {
        let resume_id = ConversationId::new().to_string();
        let parsed = Cli::try_parse_from([
            "polkagent",
            "chat",
            "--agent",
            "alice",
            "--resume",
            &resume_id,
        ])
        .expect("chat arguments parse");
        let Some(Commands::Chat(cmd)) = parsed.command else {
            panic!("expected chat command")
        };
        assert_eq!(cmd.agent, "alice");
        assert_eq!(cmd.resume.as_deref(), Some(resume_id.as_str()));
        assert!(Cli::try_parse_from([
            "polkagent",
            "chat",
            "--agent",
            "alice",
            "--resume",
            &resume_id,
            "--title",
            "wrong",
        ])
        .is_err());
    }

    #[test]
    fn no_subcommand_and_acp_parsing_remain_unchanged() {
        assert!(Cli::try_parse_from(["polkagent"])
            .expect("root CLI parses")
            .command
            .is_none());
        assert!(matches!(
            Cli::try_parse_from(["polkagent", "acp"])
                .expect("ACP parses")
                .command,
            Some(Commands::Acp(_))
        ));
    }

    #[test]
    fn unsupported_surface_commands_are_explicit() {
        assert!(refuse_unregistered_configuration_command("/provider openai").is_err());
        assert!(refuse_unregistered_configuration_command("/autonomy autonomous").is_err());
        assert!(refuse_unsupported_command(&InteractionCommand::Agents).is_err());
        assert!(refuse_unsupported_command(&InteractionCommand::Model { model: None }).is_err());
        assert!(refuse_unsupported_command(&InteractionCommand::Status).is_ok());
    }

    #[test]
    fn cancel_alias_is_accepted_during_an_active_turn() {
        assert!(is_cancel_line("/cancel"));
        assert!(is_cancel_line("  /stop all"));
        assert!(!is_cancel_line("/status"));
    }
}

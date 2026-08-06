//! `polkagent chat` — durable, line-oriented terminal conversations.

use std::io::{IsTerminal as _, Write as _};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use polkagent_core::{AgentId, ApprovalId, ConversationId, RunId};
use polkagent_interaction::{
    format_run_command_output, AgentTargetView, ApprovalDecision, ApprovalView, CancelTarget,
    ClientContext, CommandContext, CommandExecutor, CommandName, CommandOutput, CommandRegistry,
    CommandRequest, CreateInteractionRequest, InteractionCommand, InteractionCommandRuntime,
    InteractionConfig, InteractionContent, InteractionError, InteractionErrorCode,
    InteractionEvent, InteractionOverrides, InteractionService, InteractionSummary,
    InteractionTarget, ParsedLine, PromptRequest, RunDetailView, RunSummaryView,
    ServiceCommandExecutor, StartedTurn, StreamError, SubscriptionRequest, ToolCallView,
    TranscriptRequest, TurnHandle, UsageView,
};
use polkagent_runtime::{
    AdapterPolicy, PolkagentRuntime, RunCommandReadModel, RuntimeFactory, RuntimeOptions,
    WarningCode,
};
use polkagent_store_sqlite::SqlitePool;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader, Lines, Stdin};

use crate::cli::ChatCmd;
use crate::commands::interaction_agents::{
    active_agent_by_id, list_active_agent_targets, registered_agent_by_id,
    resolve_active_agent_target,
};

const MAX_PENDING_APPROVALS: usize = 100;
const MAX_DENIAL_REASON_CHARS: usize = 4_096;

const SUPPORTED_COMMANDS: [CommandName; 12] = [
    CommandName::Help,
    CommandName::Status,
    CommandName::Agents,
    CommandName::Agent,
    CommandName::Runs,
    CommandName::Inspect,
    CommandName::Cancel,
    CommandName::Approve,
    CommandName::Deny,
    CommandName::New,
    CommandName::Resume,
    CommandName::Model,
];

/// Run a durable terminal chat session.
#[allow(clippy::too_many_lines)]
pub async fn run(cmd: &ChatCmd, pool: &SqlitePool, config_path: Option<&Path>) -> Result<()> {
    let runtime = Box::pin(build_runtime(pool, config_path)).await?;
    report_runtime(&runtime);

    let service: Arc<dyn InteractionService> = runtime.interactions().clone();
    let registry = CommandRegistry::mvp();
    let client_context = terminal_client_context(&runtime)?;

    let (interaction, agent) = if let Some(raw_id) = &cmd.resume {
        let conversation_id = raw_id
            .parse::<ConversationId>()
            .with_context(|| format!("invalid conversation ID supplied to --resume: {raw_id}"))?;
        let interaction = service
            .load_interaction(conversation_id)
            .await
            .context("loading durable chat interaction")?;
        let agent_id = interaction_agent_id(&interaction)?;
        let agent = active_agent_by_id(runtime.pool(), agent_id)
            .context("resolving the durable chat target from the retained runtime database")?;
        (interaction, agent)
    } else {
        let agent = resolve_active_agent_target(runtime.pool(), &cmd.agent)
            .context("resolving the initial terminal chat agent")?;
        let interaction = service
            .new_interaction(CreateInteractionRequest {
                title: cmd
                    .title
                    .clone()
                    .or_else(|| Some(format!("terminal chat · {}", agent.name))),
                config: InteractionConfig::new(InteractionTarget::Agent(agent.agent_id)),
                client_context: client_context.clone(),
            })
            .await
            .context("creating durable chat interaction")?;
        (interaction, agent)
    };

    let command_runtime: Arc<dyn InteractionCommandRuntime> = Arc::new(ChatCommandRuntime {
        service: Arc::clone(&service),
        pool: runtime.pool().clone(),
        run_commands: RunCommandReadModel::new(runtime.pool().clone()),
        agent_id: agent.agent_id,
    });
    let executor =
        ServiceCommandExecutor::new(registry.clone(), Arc::clone(&service), command_runtime)
            .context("composing terminal slash commands")?;

    let mut session = ChatSession {
        service,
        pool: runtime.pool().clone(),
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
    // explicitly on stderr. Durable interaction-scoped model selection is
    // handled by the shared interaction service after composition.
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

fn terminal_client_context(runtime: &PolkagentRuntime) -> Result<ClientContext> {
    let mut context = ClientContext::new(runtime.workdir().to_path_buf())?;
    context.client_name = Some("terminal".to_owned());
    Ok(context)
}

fn interaction_agent_id(interaction: &InteractionSummary) -> Result<AgentId> {
    match interaction.config.target {
        InteractionTarget::Agent(agent_id) => Ok(agent_id),
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
    pool: SqlitePool,
    registry: CommandRegistry,
    executor: ServiceCommandExecutor,
    client_context: ClientContext,
    agent_id: AgentId,
    agent_name: String,
    conversation_id: ConversationId,
}

enum ApprovalAvailability {
    Available(Vec<ApprovalView>),
    Unavailable,
}

impl ApprovalAvailability {
    fn pending_count(&self) -> usize {
        match self {
            Self::Available(approvals) => approvals.len(),
            Self::Unavailable => 0,
        }
    }
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
                    if let Err(error) = self.cancel_turn_safely(started.handle.turn_id).await {
                        eprintln!("cancellation unavailable: {error:#}");
                    } else {
                        eprintln!("cancellation requested for turn {}", started.handle.turn_id);
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
                    if let Err(error) = self.cancel_turn_safely(started.handle.turn_id).await {
                        eprintln!("cancellation unavailable: {error:#}");
                    } else {
                        eprintln!("cancellation requested for turn {}", started.handle.turn_id);
                    }
                }
                line = lines.next_line(), if stdin_open => {
                    match line.context("reading terminal input during active turn")? {
                        Some(line) if is_active_turn_command(&line) => {
                            if let Err(error) = self.execute_command_line(&line, true).await {
                                eprintln!("command error: {error:#}");
                            }
                        }
                        Some(line) if line.trim().is_empty() => {}
                        Some(_) => eprintln!(
                            "the turn is active; only /status, /approve, /deny, or /cancel is accepted until it finishes"
                        ),
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

    async fn cancel_turn_safely(
        &self,
        turn_id: polkagent_interaction::InteractionTurnId,
    ) -> Result<()> {
        match self
            .service
            .list_pending_approvals(self.conversation_id)
            .await
        {
            Ok(approvals) if !approvals.is_empty() => anyhow::bail!(
                "coordinator-backed turn cancellation is not yet available while this conversation has a pending approval; use /deny for the pending request"
            ),
            Ok(_) => self
                .service
                .cancel_turn(turn_id)
                .await
                .context("cancelling active chat turn"),
            Err(error) if approval_surface_unavailable(&error) => self
                .service
                .cancel_turn(turn_id)
                .await
                .context("cancelling active grantless chat turn"),
            Err(error) => Err(error.into()),
        }
    }

    async fn execute_command_line(&mut self, line: &str, has_active_turn: bool) -> Result<()> {
        refuse_unregistered_configuration_command(line)?;
        let ParsedLine::Command(invocation) = self.registry.parse(line)? else {
            anyhow::bail!("expected a slash command");
        };
        refuse_unsupported_command(&invocation.command)?;
        validate_approval_command(&invocation.command)?;
        if matches!(invocation.command, InteractionCommand::Agent { .. }) {
            self.ensure_agent_switch_is_idle(has_active_turn).await?;
        }
        let approvals_available = match self
            .service
            .list_pending_approvals(self.conversation_id)
            .await
        {
            Ok(approvals) => ApprovalAvailability::Available(
                approvals.into_iter().take(MAX_PENDING_APPROVALS).collect(),
            ),
            Err(error) if approval_surface_unavailable(&error) => ApprovalAvailability::Unavailable,
            Err(error) => return Err(error.into()),
        };
        if matches!(
            invocation.command,
            InteractionCommand::Approve { .. } | InteractionCommand::Deny { .. }
        ) && matches!(approvals_available, ApprovalAvailability::Unavailable)
        {
            anyhow::bail!(
                "terminal approval resolution is unavailable because this runtime has no authenticated approval authority"
            );
        }
        if matches!(invocation.command, InteractionCommand::Cancel { .. })
            && approvals_available.pending_count() > 0
        {
            anyhow::bail!(
                "coordinator-backed turn cancellation is not yet available while this conversation has a pending approval; use /deny for the pending request"
            );
        }
        let output = self
            .executor
            .execute(CommandRequest {
                invocation,
                context: CommandContext {
                    conversation_id: Some(self.conversation_id),
                    has_active_turn,
                    pending_approval_count: approvals_available.pending_count(),
                    can_mutate: true,
                },
                client_context: self.client_context.clone(),
            })
            .await?;
        self.apply_command_output(output, approvals_available).await
    }

    async fn ensure_agent_switch_is_idle(&self, has_active_turn: bool) -> Result<()> {
        if has_active_turn
            || self
                .service
                .list_turns(self.conversation_id)
                .await
                .context("checking durable active work before changing agents")?
                .into_iter()
                .any(|turn| !turn.state.is_terminal())
        {
            anyhow::bail!(
                "cannot change agents while this durable conversation has active work; cancel or wait for it to finish"
            );
        }
        Ok(())
    }

    fn select_agent(&mut self, agent: AgentTargetView) -> Result<()> {
        self.agent_id = agent.agent_id;
        self.agent_name = agent.name;
        let runtime: Arc<dyn InteractionCommandRuntime> = Arc::new(ChatCommandRuntime {
            service: Arc::clone(&self.service),
            pool: self.pool.clone(),
            run_commands: RunCommandReadModel::new(self.pool.clone()),
            agent_id: self.agent_id,
        });
        self.executor =
            ServiceCommandExecutor::new(self.registry.clone(), Arc::clone(&self.service), runtime)
                .context("recomposing terminal slash commands for the selected agent")?;
        Ok(())
    }

    fn select_interaction_target(&mut self, interaction: &InteractionSummary) -> Result<()> {
        let agent_id = interaction_agent_id(interaction)?;
        let agent = active_agent_by_id(&self.pool, agent_id)
            .context("resolving the interaction's active agent target")?;
        self.select_agent(agent)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive surface projection keeps every typed shared-command result auditable"
    )]
    async fn apply_command_output(
        &mut self,
        output: CommandOutput,
        approvals_available: ApprovalAvailability,
    ) -> Result<()> {
        if let Some(rendered) = format_run_command_output(&output) {
            println!("{rendered}");
            std::io::stdout().flush().context("flushing chat output")?;
            return Ok(());
        }
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
                        CommandName::Agents => {
                            (None, "List active durable agent targets (lifecycle only)")
                        }
                        CommandName::Agent => (
                            Some("<name-or-id>"),
                            "Persist this conversation's agent target",
                        ),
                        CommandName::Cancel => (
                            Some("[all]"),
                            "Cancel the current durable turn (or all active turns)",
                        ),
                        CommandName::Approve => {
                            (Some("<approval-id>"), "Approve one exact pending approval")
                        }
                        CommandName::Deny => (
                            Some("<approval-id> [reason]"),
                            "Deny one exact pending approval",
                        ),
                        CommandName::Model => (
                            Some("[model-id]"),
                            "Show or persist the model for this durable conversation",
                        ),
                        _ => (command.input_hint.as_deref(), command.description.as_str()),
                    };
                    let hint = input_hint.map_or(String::new(), |hint| format!(" {hint}"));
                    println!("  /{}{hint} — {description}", command.name);
                }
                println!(
                    "Provider/harness/autonomy and group commands are not available in terminal chat."
                );
                println!(
                    "Approval commands appear only when scoped pending approvals are available."
                );
            }
            CommandOutput::Status {
                interaction,
                active_turns,
                pending_approvals,
            } => {
                println!("session: {}", interaction.conversation_id);
                let target = match interaction.config.target {
                    InteractionTarget::Agent(agent_id) if agent_id == self.agent_id => {
                        format!("{} ({agent_id})", self.agent_name)
                    }
                    _ => display_target(&interaction.config.target),
                };
                println!("target: {target}");
                println!("state: {:?}", interaction.state);
                println!("turns: {}", interaction.turn_count);
                println!("active turns: {}", active_turns.len());
                match approvals_available {
                    ApprovalAvailability::Available(_) => {
                        println!("pending approvals: {}", pending_approvals.len());
                        for approval_id in pending_approvals {
                            println!("  {approval_id}");
                        }
                    }
                    ApprovalAvailability::Unavailable => {
                        println!("pending approvals: unavailable (no authenticated authority)");
                    }
                }
                println!(
                    "model: {} (durable conversation selection)",
                    interaction
                        .config
                        .model
                        .as_deref()
                        .unwrap_or("runtime/agent default")
                );
                println!(
                    "provider: {} (selection unavailable in terminal chat)",
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
            CommandOutput::Agents { agents } => {
                if agents.is_empty() {
                    println!("no active agents");
                } else {
                    println!("active agents:");
                    for agent in agents {
                        let selected = if agent.agent_id == self.agent_id {
                            "*"
                        } else {
                            " "
                        };
                        println!(
                            "  {selected} {} ({}) [{}]",
                            agent.name, agent.agent_id, agent.state
                        );
                    }
                }
                println!("readiness: not projected; lifecycle is from the durable registry");
            }
            CommandOutput::TargetChanged { target } => {
                let agent_id = match target {
                    InteractionTarget::Agent(agent_id) => agent_id,
                    InteractionTarget::Group(_) | InteractionTarget::Auto => {
                        anyhow::bail!("terminal chat received a non-agent target")
                    }
                };
                let agent = registered_agent_by_id(&self.pool, agent_id)
                    .context("resolving the persisted terminal chat target")?;
                self.select_agent(agent)?;
                println!("target: {} ({})", self.agent_name, self.agent_id);
                println!("conversation: {}", self.conversation_id);
                println!("persistence: updated for this durable conversation");
                self.announce_session();
            }
            CommandOutput::InteractionCreated { interaction } => {
                self.select_interaction_target(&interaction)?;
                self.conversation_id = interaction.conversation_id;
                self.announce_session();
            }
            CommandOutput::InteractionResumed { interaction } => {
                self.select_interaction_target(&interaction)?;
                self.conversation_id = interaction.conversation_id;
                self.announce_session();
                self.render_transcript().await?;
            }
            CommandOutput::Model { model, changed } => {
                println!(
                    "model: {}",
                    model.as_deref().unwrap_or("runtime/agent default")
                );
                println!("conversation: {}", self.conversation_id);
                println!(
                    "persistence: {}",
                    if changed {
                        "updated for this durable conversation"
                    } else {
                        "current durable conversation selection"
                    }
                );
            }
            CommandOutput::ApprovalResolved {
                approval_id,
                decision,
            } => match decision {
                ApprovalDecision::Approve => println!("approval {approval_id}: approved"),
                ApprovalDecision::Deny { .. } => println!("approval {approval_id}: denied"),
            },
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

fn is_active_turn_command(line: &str) -> bool {
    matches!(
        line.split_whitespace().next(),
        Some("/status" | "/st" | "/approve" | "/deny" | "/cancel" | "/stop")
    )
}

fn terminal_tool_progress(call: &ToolCallView) -> String {
    format!("[tool {}] {}: {:?}", call.call_id, call.title, call.status)
}

fn terminal_approval_request(request: &ApprovalView) -> String {
    format!(
        "[approval required {}] run={} effect={}; use `/approve {}` or `/deny {} [reason]`",
        request.approval_id,
        request.run_id,
        request.effect_id,
        request.approval_id,
        request.approval_id
    )
}

fn approval_surface_unavailable(error: &InteractionError) -> bool {
    matches!(
        error.code,
        InteractionErrorCode::Unsupported | InteractionErrorCode::Unavailable
    )
}

fn validate_approval_command(command: &InteractionCommand) -> Result<()> {
    if let InteractionCommand::Deny {
        reason: Some(reason),
        ..
    } = command
    {
        anyhow::ensure!(
            reason.chars().count() <= MAX_DENIAL_REASON_CHARS,
            "approval denial reason must be at most {MAX_DENIAL_REASON_CHARS} characters"
        );
    }
    Ok(())
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
        "/harness" => anyhow::bail!(
            "terminal chat does not support harness selection; configure the runtime and restart"
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
        InteractionCommand::Cancel {
            target: CancelTarget::Run(_),
        } => anyhow::bail!(
            "terminal chat cancels the current turn only; run-scoped cancellation is unsupported"
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
                eprintln!("{}", terminal_tool_progress(&call));
            }
            InteractionEvent::PlanUpdated { entries } => {
                eprintln!("[plan] {} item(s) updated", entries.len());
            }
            InteractionEvent::ApprovalRequested { request } => {
                eprintln!("{}", terminal_approval_request(&request));
            }
            InteractionEvent::ApprovalResolved {
                approval_id,
                decision,
            } => match decision {
                ApprovalDecision::Approve => eprintln!("[approval {approval_id}] approved"),
                ApprovalDecision::Deny { .. } => eprintln!("[approval {approval_id}] denied"),
            },
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
    pool: SqlitePool,
    run_commands: RunCommandReadModel,
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
        list_active_agent_targets(&self.pool)
    }

    async fn resolve_agent(&self, selector: &str) -> Result<AgentTargetView, InteractionError> {
        resolve_active_agent_target(&self.pool, selector)
    }

    async fn list_runs(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<RunSummaryView>, InteractionError> {
        self.run_commands.list_runs(conversation_id).await
    }

    async fn inspect_run(&self, _run_id: RunId) -> Result<RunDetailView, InteractionError> {
        Err(chat_unsupported("run inspection"))
    }

    async fn inspect_run_for_conversation(
        &self,
        conversation_id: ConversationId,
        run_id: RunId,
    ) -> Result<RunDetailView, InteractionError> {
        self.run_commands.inspect_run(conversation_id, run_id).await
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
        conversation_id: ConversationId,
    ) -> Result<Vec<ApprovalId>, InteractionError> {
        match self.service.list_pending_approvals(conversation_id).await {
            Ok(approvals) => Ok(approvals
                .into_iter()
                .take(MAX_PENDING_APPROVALS)
                .map(|approval| approval.approval_id)
                .collect()),
            Err(error) if approval_surface_unavailable(&error) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
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
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    use clap::Parser as _;
    use futures::Stream;
    use polkagent_config::{Config, ModelOverrideConfig, SecurityConfig};
    use polkagent_core::{AgentSpec, DataClassification, EffectId, PrincipalId, StepId, TurnId};
    use polkagent_event::{EventBus, EventRecorder};
    use polkagent_executor_fake::FakeExecutor;
    use polkagent_executor_trait::{
        ExecutorError, InferenceRequest, InferenceResponse, ModelExecutor, StreamEvent, TokenUsage,
    };
    use polkagent_harness_trait::{
        CancelMode, Harness, HarnessCapabilities, HarnessError, HarnessEvent, HarnessId,
        HarnessStatus, McpMode, SessionConfig, SessionId, SessionResumeMode, ToolInjection,
    };
    use polkagent_interaction::{
        InteractionApprovalAuthority, InteractionEventId, InteractionRunLink, InteractionStore,
        NewInteraction, NewInteractionTurn, RunRole,
    };
    use polkagent_runtime::DurableInteractionService;
    use polkagent_service::{AppService, ApprovalRuntimeConfig};
    use polkagent_store_sqlite::{migrations, SqliteInteractionStore, SqliteRunStore};
    use polkagent_store_trait::{
        approval::{
            ApprovalCoordinatorStore, ApprovalRequestMetadata, ApprovalSubject, CheckpointEffect,
            CheckpointEffectStatus, ExecutionCheckpoint, PauseForApproval,
            APPROVAL_SUBJECT_SCHEMA_VERSION, EXECUTION_CHECKPOINT_SCHEMA_VERSION,
        },
        StoreRetryClass,
    };
    use uuid::Uuid;

    use super::*;
    use crate::cli::{Cli, Commands};

    #[derive(Default)]
    struct CapturingExecutor {
        requests: Mutex<Vec<InferenceRequest>>,
    }

    struct FixedHarness {
        id: HarnessId,
    }

    impl FixedHarness {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                id: HarnessId::new("fixed-test-harness"),
            })
        }
    }

    #[async_trait]
    impl Harness for FixedHarness {
        fn id(&self) -> &HarnessId {
            &self.id
        }

        fn capabilities(&self) -> HarnessCapabilities {
            HarnessCapabilities {
                supports_streaming: false,
                supports_tools: false,
                supports_sessions: true,
                max_context_tokens: 1_000,
                models: vec!["fake/model-a".to_owned()],
                transport: None,
                model_override: None,
                session_resume: SessionResumeMode::default(),
                mcp_passthrough: McpMode::default(),
                tool_injection: ToolInjection::default(),
                cancel: CancelMode::default(),
                multiplex_safe: false,
            }
        }

        fn status(&self) -> HarnessStatus {
            HarnessStatus::Idle
        }

        async fn start_session(
            &self,
            _config: SessionConfig,
        ) -> std::result::Result<SessionId, HarnessError> {
            Err(HarnessError::Internal {
                message: "fixed harness must not execute".to_owned(),
            })
        }

        async fn send_message(
            &self,
            _session_id: SessionId,
            _message: &str,
        ) -> std::result::Result<(), HarnessError> {
            Err(HarnessError::Internal {
                message: "fixed harness must not execute".to_owned(),
            })
        }

        async fn receive_events(
            &self,
            _session_id: SessionId,
        ) -> std::result::Result<
            std::pin::Pin<Box<dyn Stream<Item = HarnessEvent> + Send>>,
            HarnessError,
        > {
            Err(HarnessError::Internal {
                message: "fixed harness must not execute".to_owned(),
            })
        }

        async fn end_session(
            &self,
            _session_id: SessionId,
        ) -> std::result::Result<(), HarnessError> {
            Ok(())
        }

        async fn health(&self) -> std::result::Result<bool, HarnessError> {
            Ok(true)
        }
    }

    impl CapturingExecutor {
        fn requests(&self) -> Vec<InferenceRequest> {
            self.requests.lock().expect("capture lock").clone()
        }
    }

    #[async_trait]
    impl ModelExecutor for CapturingExecutor {
        async fn complete(
            &self,
            request: InferenceRequest,
        ) -> std::result::Result<InferenceResponse, ExecutorError> {
            self.requests.lock().expect("capture lock").push(request);
            Ok(InferenceResponse {
                text: "captured assistant".to_owned(),
                tool_calls: Vec::new(),
                stop_reason: "end_turn".to_owned(),
                usage: TokenUsage::default(),
                provider_request_id: None,
            })
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> std::result::Result<
            Box<dyn Stream<Item = std::result::Result<StreamEvent, ExecutorError>> + Send + Unpin>,
            ExecutorError,
        > {
            Err(ExecutorError::Internal {
                message: "chat interaction tests use non-streaming execution".to_owned(),
            })
        }

        async fn health(&self) -> std::result::Result<(), ExecutorError> {
            Ok(())
        }
    }

    fn model_config() -> Config {
        Config {
            models: [
                ("model-a", "fake"),
                ("model-b", "fake"),
                ("other-model", "other"),
            ]
            .into_iter()
            .map(|(slug, provider)| ModelOverrideConfig {
                slug: slug.to_owned(),
                provider: provider.to_owned(),
                ..ModelOverrideConfig::default()
            })
            .collect(),
            ..Config::default()
        }
    }

    fn persist_agent(pool: &SqlitePool, spec: &AgentSpec) {
        pool.writer()
            .execute(
                "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                 VALUES (?1, ?2, 'active', ?3, ?4, ?4)",
                rusqlite::params![
                    spec.id.to_string(),
                    &spec.name,
                    serde_json::to_string(spec).expect("encode model command agent"),
                    "2026-01-01T00:00:00Z"
                ],
            )
            .expect("persist model command agent");
    }

    fn model_service(
        pool: &SqlitePool,
        spec: AgentSpec,
        executor: Arc<dyn ModelExecutor>,
    ) -> (Arc<AppService>, Arc<DurableInteractionService>) {
        persist_agent(pool, &spec);
        let bus = EventBus::new(32);
        let shared = Arc::new(pool.clone());
        let recorder = EventRecorder::new(shared.clone(), bus.clone());
        let app = Arc::new(
            AppService::builder()
                .with_config(model_config())
                .with_executor(executor)
                .with_run_store(shared.clone())
                .with_effect_store(shared.clone())
                .with_conversation_store(shared.clone())
                .with_payment_store(shared)
                .with_event_bus(bus)
                .with_event_recorder(recorder)
                .build()
                .expect("build model command service"),
        );
        app.create_agent(spec)
            .expect("register model command agent");
        let service = Arc::new(DurableInteractionService::new(
            Arc::clone(&app),
            pool.clone(),
        ));
        (app, service)
    }

    fn harness_model_service(
        pool: &SqlitePool,
        spec: AgentSpec,
        executor: Arc<dyn ModelExecutor>,
        harness: Arc<dyn Harness>,
    ) -> Arc<DurableInteractionService> {
        persist_agent(pool, &spec);
        let bus = EventBus::new(32);
        let shared = Arc::new(pool.clone());
        let recorder = EventRecorder::new(shared.clone(), bus.clone());
        let app = Arc::new(
            AppService::builder()
                .with_config(model_config())
                .with_executor(executor)
                .with_harness(harness)
                .with_run_store(shared.clone())
                .with_effect_store(shared.clone())
                .with_conversation_store(shared.clone())
                .with_payment_store(shared)
                .with_event_bus(bus)
                .with_event_recorder(recorder)
                .build()
                .expect("build harness model command service"),
        );
        app.create_agent(spec)
            .expect("register harness model command agent");
        Arc::new(DurableInteractionService::new(app, pool.clone()))
    }

    fn chat_session(
        service: Arc<dyn InteractionService>,
        pool: &SqlitePool,
        agent_id: AgentId,
        conversation_id: ConversationId,
    ) -> ChatSession {
        let registry = CommandRegistry::mvp();
        let command_runtime: Arc<dyn InteractionCommandRuntime> = Arc::new(ChatCommandRuntime {
            service: Arc::clone(&service),
            pool: pool.clone(),
            run_commands: RunCommandReadModel::new(pool.clone()),
            agent_id,
        });
        let executor =
            ServiceCommandExecutor::new(registry.clone(), Arc::clone(&service), command_runtime)
                .expect("compose chat command executor");
        ChatSession {
            service,
            pool: pool.clone(),
            registry,
            executor,
            client_context: ClientContext::new(PathBuf::from("/tmp/chat-model-test"))
                .expect("chat client context"),
            agent_id,
            agent_name: "model-agent".to_owned(),
            conversation_id,
        }
    }

    struct ChatApprovalFixture {
        authority: InteractionApprovalAuthority,
        conversation_id: ConversationId,
        other_conversation_id: ConversationId,
        interaction_turn_id: polkagent_interaction::InteractionTurnId,
        approval_id: ApprovalId,
        run_id: RunId,
        effect_id: EffectId,
    }

    fn seed_conversation_row(
        pool: &SqlitePool,
        conversation_id: ConversationId,
        agent_id: AgentId,
        now: chrono::DateTime<chrono::Utc>,
    ) {
        pool.writer()
            .execute(
                "INSERT INTO conversations
                    (id, agent_id, title, message_count, metadata_json, created_at, updated_at)
                 VALUES (?1, ?2, NULL, 0, '{}', ?3, ?3)",
                rusqlite::params![
                    conversation_id.to_string(),
                    agent_id.to_string(),
                    now.to_rfc3339(),
                ],
            )
            .expect("seed approval conversation");
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the exact persisted approval lineage remains auditable in one test fixture"
    )]
    async fn seed_chat_approval(pool: &SqlitePool, agent_id: AgentId) -> ChatApprovalFixture {
        let conversation_id = ConversationId::new();
        let other_conversation_id = ConversationId::new();
        let interaction_turn_id = polkagent_interaction::InteractionTurnId::new();
        let run_id = RunId::new();
        let executor_turn_id = TurnId::new();
        let step_id = StepId::new();
        let effect_id = EffectId::new();
        let approval_id = ApprovalId::new();
        let principal_id = PrincipalId::new();
        let now = chrono::Utc::now();
        let deadline = now + chrono::Duration::minutes(10);

        for id in [conversation_id, other_conversation_id] {
            seed_conversation_row(pool, id, agent_id, now);
        }
        let interaction_store = SqliteInteractionStore::new(pool.clone());
        for id in [conversation_id, other_conversation_id] {
            interaction_store
                .create_interaction(NewInteraction {
                    conversation_id: id,
                    config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                    origin_working_directory: PathBuf::from("/tmp/chat-approval-test"),
                    created_at: now,
                })
                .await
                .expect("seed approval interaction");
        }
        {
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO runs
                        (id, agent_id, conversation_id, state, params_json,
                         created_at, updated_at, started_at)
                     VALUES (?1, ?2, ?3, 'running', '{}', ?4, ?4, ?4)",
                    rusqlite::params![
                        run_id.to_string(),
                        agent_id.to_string(),
                        conversation_id.to_string(),
                        now.to_rfc3339(),
                    ],
                )
                .expect("seed approval run");
            writer
                .execute(
                    "INSERT INTO turns (id, run_id, sequence, role, started_at)
                     VALUES (?1, ?2, 1, 'assistant', ?3)",
                    rusqlite::params![
                        executor_turn_id.to_string(),
                        run_id.to_string(),
                        now.to_rfc3339(),
                    ],
                )
                .expect("seed approval executor turn");
            writer
                .execute(
                    "INSERT INTO steps (id, turn_id, sequence, kind, started_at)
                     VALUES (?1, ?2, 1, 'tool_call', ?3)",
                    rusqlite::params![
                        step_id.to_string(),
                        executor_turn_id.to_string(),
                        now.to_rfc3339(),
                    ],
                )
                .expect("seed approval step");
        }
        interaction_store
            .create_turn(NewInteractionTurn {
                turn_id: interaction_turn_id,
                conversation_id,
                ordinal: 1,
                target: InteractionTarget::Agent(agent_id),
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                user_message_id: Uuid::now_v7(),
                user_message_text: "approval fixture".to_owned(),
                runs: vec![InteractionRunLink {
                    run_id,
                    role: RunRole::Primary,
                    ordinal: 1,
                }],
                initial_event_id: InteractionEventId::new(),
                started_at: now,
            })
            .await
            .expect("seed approval interaction turn");

        let subject = ApprovalSubject {
            schema_version: APPROVAL_SUBJECT_SCHEMA_VERSION,
            conversation_id,
            turn_id: executor_turn_id,
            step_id,
            run_id,
            agent_id,
            effect_id,
            tool_call_id: "call-write-1".to_owned(),
            tool_name: "filesystem.write".to_owned(),
            validated_arguments: serde_json::json!({"path":"notes.txt"}),
            required_action: "write".to_owned(),
            required_resource: "workspace/notes.txt".to_owned(),
            working_directory: Some("/workspace".to_owned()),
            security_scope: serde_json::json!({
                "tenant_id":"tenant-a",
                "workspace_id":"workspace-a"
            }),
            tool_spec_digest: "fixture-tool-digest".to_owned(),
            policy_snapshot_digest: "fixture-policy-digest".to_owned(),
            subject_digest: "fixture-subject-digest".to_owned(),
        };
        let checkpoint = ExecutionCheckpoint {
            schema_version: EXECUTION_CHECKPOINT_SCHEMA_VERSION,
            run_id,
            turn_id: executor_turn_id,
            conversation_id,
            agent_id,
            model_id: "fake/model".to_owned(),
            executor_id: "fake".to_owned(),
            messages: serde_json::json!([]),
            tool_calls: serde_json::json!([]),
            next_model_turn: 2,
            next_step_sequence: 2,
            next_effect_sequence: 2,
            accumulated_usage: serde_json::json!({}),
            accumulated_cost: None,
            deadline_at: Some(deadline),
            retry_class: StoreRetryClass::NoAutoRetry,
            effects: vec![CheckpointEffect {
                effect_id,
                status: CheckpointEffectStatus::AwaitingApproval,
            }],
            version: 1,
            classification: DataClassification::Private,
            retention_expires_at: None,
            integrity_digest: "fixture-checkpoint-digest".to_owned(),
        };
        ApprovalCoordinatorStore::pause_for_approval(
            pool,
            PauseForApproval {
                approval_id,
                expected_run_state_version: 0,
                subject,
                effect_payload: serde_json::json!({
                    "kind":"tool_call",
                    "tool_name":"filesystem.write"
                }),
                effect_idempotency_key: format!("approval-{effect_id}"),
                retry_class: StoreRetryClass::NoAutoRetry,
                checkpoint,
                metadata: ApprovalRequestMetadata {
                    title: "Write notes.txt".to_owned(),
                    description: "Write one file in the selected workspace".to_owned(),
                    reason: "filesystem write grant requires approval".to_owned(),
                    tenant_id: "tenant-a".to_owned(),
                    workspace_id: "workspace-a".to_owned(),
                    authorized_principal_id: principal_id,
                },
                deadline_at: deadline,
            },
        )
        .await
        .expect("pause approval fixture");

        ChatApprovalFixture {
            authority: InteractionApprovalAuthority {
                tenant_id: "tenant-a".to_owned(),
                workspace_id: "workspace-a".to_owned(),
                principal_id,
                surface: "terminal-chat-test".to_owned(),
            },
            conversation_id,
            other_conversation_id,
            interaction_turn_id,
            approval_id,
            run_id,
            effect_id,
        }
    }

    fn chat_approval_service(
        pool: &SqlitePool,
        authority: &InteractionApprovalAuthority,
    ) -> Arc<DurableInteractionService> {
        let bus = EventBus::new(32);
        let shared = Arc::new(pool.clone());
        let app = Arc::new(
            AppService::builder()
                .with_config(Config::default())
                .with_executor(FakeExecutor::new())
                .with_run_store(shared.clone())
                .with_effect_store(shared.clone())
                .with_conversation_store(shared.clone())
                .with_event_bus(bus.clone())
                .with_event_recorder(EventRecorder::new(shared.clone(), bus))
                .with_tool_registry(Arc::new(polkagent_tool::ToolRegistry::new()))
                .with_approval_runtime(
                    shared.clone(),
                    shared,
                    ApprovalRuntimeConfig {
                        tenant_id: authority.tenant_id.clone(),
                        workspace_id: authority.workspace_id.clone(),
                        authorized_principal_id: authority.principal_id,
                        service_principal_id: PrincipalId::new(),
                        working_directory: PathBuf::from("/workspace"),
                        security_config: SecurityConfig::default(),
                        approval_timeout: Duration::from_secs(60),
                        recovery_lease: Duration::from_secs(1),
                        poll_interval: Duration::from_millis(10),
                    },
                )
                .build()
                .expect("build chat approval service"),
        );
        Arc::new(
            DurableInteractionService::new(app, pool.clone())
                .with_approval_authority(authority.clone())
                .expect("bind terminal chat approval authority"),
        )
    }

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
        assert!(refuse_unregistered_configuration_command("/harness codex").is_err());
        assert!(refuse_unsupported_command(&InteractionCommand::Agents).is_ok());
        assert!(refuse_unsupported_command(&InteractionCommand::Agent {
            selector: "alice".to_owned(),
        })
        .is_ok());
        assert!(refuse_unsupported_command(&InteractionCommand::Model { model: None }).is_ok());
        assert!(refuse_unsupported_command(&InteractionCommand::Status).is_ok());
        assert!(refuse_unsupported_command(&InteractionCommand::Runs).is_ok());
        assert!(refuse_unsupported_command(&InteractionCommand::Inspect {
            run_id: RunId::new(),
        })
        .is_ok());
        assert!(refuse_unsupported_command(&InteractionCommand::Approve {
            approval_id: ApprovalId::new(),
        })
        .is_ok());
        assert!(refuse_unsupported_command(&InteractionCommand::Deny {
            approval_id: ApprovalId::new(),
            reason: None,
        })
        .is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    #[allow(
        clippy::too_many_lines,
        reason = "one end-to-end fixture proves terminal dispatch never bypasses the durable coordinator"
    )]
    async fn approval_commands_are_scoped_durable_retryable_and_cancellation_safe() {
        let pool = SqlitePool::open_in_memory().expect("open chat approval database");
        migrations::migrate(&pool.writer()).expect("migrate chat approval database");
        let agent_id = AgentId::new();
        persist_agent(
            &pool,
            &AgentSpec::new(agent_id, "approval-agent", "fake/model"),
        );
        let fixture = seed_chat_approval(&pool, agent_id).await;
        let service = chat_approval_service(&pool, &fixture.authority);
        let erased: Arc<dyn InteractionService> = service.clone();
        let mut session = chat_session(erased, &pool, agent_id, fixture.conversation_id);

        let pending = service
            .list_pending_approvals(fixture.conversation_id)
            .await
            .expect("list terminal pending approval");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].approval_id, fixture.approval_id);
        assert_eq!(pending[0].run_id, fixture.run_id);
        assert_eq!(pending[0].effect_id, fixture.effect_id);
        session
            .execute_command_line("/status", false)
            .await
            .expect("render scoped pending identity");

        let cancel_error = session
            .execute_command_line("/cancel", true)
            .await
            .expect_err("generic cancellation must not strand a pending approval");
        assert!(
            format!("{cancel_error:#}").contains("pending approval"),
            "{cancel_error:#}"
        );
        let signal_cancel_error = session
            .cancel_turn_safely(fixture.interaction_turn_id)
            .await
            .expect_err("signal cancellation must fail closed while approval is pending");
        assert!(
            format!("{signal_cancel_error:#}").contains("pending approval"),
            "{signal_cancel_error:#}"
        );
        assert_eq!(
            service
                .list_pending_approvals(fixture.conversation_id)
                .await
                .expect("approval remains pending")
                .len(),
            1
        );

        let wrong_erased: Arc<dyn InteractionService> = service.clone();
        let mut wrong_scope =
            chat_session(wrong_erased, &pool, agent_id, fixture.other_conversation_id);
        let wrong_error = wrong_scope
            .execute_command_line(&format!("/approve {}", fixture.approval_id), false)
            .await
            .expect_err("foreign conversation must not resolve approval");
        assert!(format!("{wrong_error:#}").contains("permission_denied"));

        session
            .execute_command_line(&format!("/approve {}", fixture.approval_id), false)
            .await
            .expect("approve exact pending request");
        assert!(service
            .list_pending_approvals(fixture.conversation_id)
            .await
            .expect("list after approval")
            .is_empty());

        drop(session);
        drop(service);
        let restarted = chat_approval_service(&pool, &fixture.authority);
        let restarted_erased: Arc<dyn InteractionService> = restarted.clone();
        let mut restarted_session =
            chat_session(restarted_erased, &pool, agent_id, fixture.conversation_id);
        restarted_session
            .execute_command_line(&format!("/approve {}", fixture.approval_id), false)
            .await
            .expect("identical approval retry after restart");
        let conflict = restarted_session
            .execute_command_line(
                &format!("/deny {} changed decision", fixture.approval_id),
                false,
            )
            .await
            .expect_err("opposite decision must conflict");
        assert!(format!("{conflict:#}").contains("conflict"));

        restarted_session
            .cancel_turn_safely(fixture.interaction_turn_id)
            .await
            .expect("approval-capable session with no pending request can cancel normally");
    }

    #[tokio::test(flavor = "current_thread")]
    #[allow(clippy::too_many_lines)]
    async fn model_commands_persist_isolate_and_drive_real_executor_requests_without_turns() {
        let pool = SqlitePool::open_in_memory().expect("open model command database");
        migrations::migrate(&pool.writer()).expect("migrate model command database");
        let agent_id = AgentId::new();
        let agent_spec = AgentSpec::new(agent_id, "model-agent", "fake/default-model");
        let capture = Arc::new(CapturingExecutor::default());
        let erased: Arc<dyn ModelExecutor> = capture.clone();
        let (app, service) = model_service(&pool, agent_spec, erased);
        let client_context =
            ClientContext::new(PathBuf::from("/tmp/chat-model-test")).expect("client context");
        let first = service
            .new_interaction(CreateInteractionRequest {
                title: Some("first model session".to_owned()),
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                client_context: client_context.clone(),
            })
            .await
            .expect("create first model session");
        let second = service
            .new_interaction(CreateInteractionRequest {
                title: Some("second model session".to_owned()),
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                client_context,
            })
            .await
            .expect("create second model session");

        let first_service: Arc<dyn InteractionService> = service.clone();
        let second_service: Arc<dyn InteractionService> = service.clone();
        let mut first_session = chat_session(first_service, &pool, agent_id, first.conversation_id);
        let mut second_session =
            chat_session(second_service, &pool, agent_id, second.conversation_id);
        first_session
            .execute_command_line("/model model-a", false)
            .await
            .expect("select first model");
        second_session
            .execute_command_line("/model model-b", false)
            .await
            .expect("select second model");

        for (line, code, detail) in [
            ("/model missing-model", "invalid_request", "unknown model"),
            ("/model other-model", "unsupported", "provider switching"),
        ] {
            let error = first_session
                .execute_command_line(line, false)
                .await
                .expect_err("invalid model must be refused by interaction service");
            let rendered = format!("{error:#}");
            assert!(rendered.contains(code), "{rendered}");
            assert!(rendered.contains(detail), "{rendered}");
        }
        first_session
            .execute_command_line("/help model", false)
            .await
            .expect("model help is truthful");
        assert_eq!(
            service
                .list_turns(first.conversation_id)
                .await
                .expect("first command-only transcript")
                .len(),
            0
        );
        assert_eq!(
            service
                .list_turns(second.conversation_id)
                .await
                .expect("second command-only transcript")
                .len(),
            0
        );

        drop(first_session);
        drop(second_session);
        drop(service);
        let restarted = Arc::new(DurableInteractionService::new(app, pool.clone()));
        assert_eq!(
            restarted
                .load_interaction(first.conversation_id)
                .await
                .expect("reload first interaction")
                .config
                .model
                .as_deref(),
            Some("fake/model-a")
        );
        assert_eq!(
            restarted
                .load_interaction(second.conversation_id)
                .await
                .expect("reload second interaction")
                .config
                .model
                .as_deref(),
            Some("fake/model-b")
        );

        let first_service: Arc<dyn InteractionService> = restarted.clone();
        let second_service: Arc<dyn InteractionService> = restarted.clone();
        let mut first_session = chat_session(first_service, &pool, agent_id, first.conversation_id);
        let mut second_session =
            chat_session(second_service, &pool, agent_id, second.conversation_id);
        first_session
            .execute_command_line("/model", false)
            .await
            .expect("query persisted first model");
        assert_eq!(
            first_session
                .run_turn_non_interactive("first captured prompt".to_owned())
                .await
                .expect("run first captured prompt"),
            TurnTerminal::Completed
        );
        assert_eq!(
            second_session
                .run_turn_non_interactive("second captured prompt".to_owned())
                .await
                .expect("run second captured prompt"),
            TurnTerminal::Completed
        );

        let requests = capture.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].model_id, "fake/model-a");
        assert_eq!(requests[1].model_id, "fake/model-b");
        assert_eq!(
            restarted
                .list_turns(first.conversation_id)
                .await
                .expect("first transcript")
                .len(),
            1
        );
        assert_eq!(
            restarted
                .list_turns(second.conversation_id)
                .await
                .expect("second transcript")
                .len(),
            1
        );
        let stored_agent = SqliteRunStore::new(pool)
            .get_agent_by_name_or_id("model-agent")
            .expect("load unchanged agent");
        let stored_spec: AgentSpec =
            serde_json::from_str(&stored_agent.spec_json).expect("decode unchanged agent spec");
        assert_eq!(stored_spec.model, "fake/default-model");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn model_command_surfaces_typed_harness_refusal_without_a_turn() {
        let pool = SqlitePool::open_in_memory().expect("open harness command database");
        migrations::migrate(&pool.writer()).expect("migrate harness command database");
        let agent_id = AgentId::new();
        let capture: Arc<dyn ModelExecutor> = Arc::new(CapturingExecutor::default());
        let harness: Arc<dyn Harness> = FixedHarness::new();
        let service = harness_model_service(
            &pool,
            AgentSpec::new(agent_id, "harness-agent", "fake/default-model"),
            capture,
            harness,
        );
        let interaction = service
            .new_interaction(CreateInteractionRequest {
                title: Some("harness model refusal".to_owned()),
                config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
                client_context: ClientContext::new(PathBuf::from("/tmp/chat-harness-test"))
                    .expect("harness client context"),
            })
            .await
            .expect("create harness interaction");
        let erased: Arc<dyn InteractionService> = service.clone();
        let mut session = chat_session(erased, &pool, agent_id, interaction.conversation_id);
        let error = session
            .execute_command_line("/model model-a", false)
            .await
            .expect_err("dynamic harness model selection must be refused");
        let rendered = format!("{error:#}");
        assert!(rendered.contains("unsupported"), "{rendered}");
        assert!(rendered.contains("fixed-test-harness"), "{rendered}");
        assert!(
            rendered.contains("cannot apply a dynamic model"),
            "{rendered}"
        );
        assert!(service
            .list_turns(interaction.conversation_id)
            .await
            .expect("harness command-only transcript")
            .is_empty());
    }

    #[test]
    fn scoped_commands_are_accepted_during_an_active_turn() {
        for line in [
            "/cancel",
            "  /stop all",
            "/status",
            "/st",
            "/approve 00000000-0000-0000-0000-000000000000",
            "/deny 00000000-0000-0000-0000-000000000000 reason",
        ] {
            assert!(is_active_turn_command(line), "rejected {line}");
        }
        assert!(!is_active_turn_command("/model fake/model"));
    }

    #[test]
    fn approval_progress_uses_only_bounded_stable_fields() {
        let request = ApprovalView {
            approval_id: ApprovalId::new(),
            effect_id: EffectId::new(),
            run_id: RunId::new(),
            tool_call_id: None,
            title: "untrusted title\n/approve attacker".to_owned(),
            description: "untrusted description".to_owned(),
            status: polkagent_interaction::ApprovalStatus::Pending,
            policy_reason: Some("untrusted policy reason".to_owned()),
            expires_at: None,
        };
        let rendered = terminal_approval_request(&request);
        for identity in [
            request.approval_id.to_string(),
            request.run_id.to_string(),
            request.effect_id.to_string(),
        ] {
            assert!(rendered.contains(&identity), "{rendered}");
        }
        assert!(rendered.contains("/approve"));
        assert!(rendered.contains("/deny"));
        for untrusted in [
            "untrusted title",
            "attacker",
            "description",
            "policy reason",
        ] {
            assert!(!rendered.contains(untrusted), "{rendered}");
        }
        assert_eq!(rendered.lines().count(), 1);
    }

    #[test]
    fn approval_denial_reason_is_unicode_bounded() {
        let approval_id = ApprovalId::new();
        assert!(validate_approval_command(&InteractionCommand::Deny {
            approval_id,
            reason: Some("é".repeat(MAX_DENIAL_REASON_CHARS)),
        })
        .is_ok());
        let error = validate_approval_command(&InteractionCommand::Deny {
            approval_id,
            reason: Some("é".repeat(MAX_DENIAL_REASON_CHARS + 1)),
        })
        .expect_err("overlong denial reason");
        assert!(format!("{error:#}").contains("4096"));
    }

    #[test]
    fn terminal_tool_progress_preserves_durable_identity_and_status() {
        let call_id = polkagent_interaction::ToolCallId::new();
        let call = ToolCallView {
            call_id,
            run_id: RunId::new(),
            name: "test.observe".to_owned(),
            title: "test.observe".to_owned(),
            kind: polkagent_interaction::ToolCallKind::Other,
            status: polkagent_interaction::ToolCallStatus::Failed,
            arguments: None,
            summary: Some("failed safely".to_owned()),
            output: None,
            locations: Vec::new(),
            diff: None,
            error: Some("Tool execution failed (server_error)".to_owned()),
        };
        assert_eq!(
            terminal_tool_progress(&call),
            format!("[tool {call_id}] test.observe: Failed")
        );
    }
}

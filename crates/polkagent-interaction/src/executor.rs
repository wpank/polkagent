//! Shared implementation of the MVP slash-command handlers.

use std::sync::Arc;

use async_trait::async_trait;
use polkagent_core::ids::{ApprovalId, ConversationId, RunId};

use crate::command::{
    AgentTargetView, CancelTarget, CommandExecutor, CommandMutability, CommandOutput,
    CommandRegistry, CommandRequest, InteractionCommand, RunDetailView, RunSummaryView,
};
use crate::error::{InteractionError, InteractionErrorCode};
use crate::model::{
    ApprovalDecision, ConfigOption, ConfigOptionValue, ConfigUpdate, CreateInteractionRequest,
    InteractionConfig, InteractionTarget, TurnHandle,
};
use crate::service::InteractionService;

/// Runtime/read-model operations needed beyond [`InteractionService`].
///
/// Keeping this port surface-neutral lets the same command executor serve a
/// terminal, TUI, HTTP API, or editor protocol without importing their types.
#[async_trait]
pub trait InteractionCommandRuntime: Send + Sync {
    /// Return a safe default configuration for a newly created interaction.
    async fn default_interaction_config(&self) -> Result<InteractionConfig, InteractionError>;

    /// List configured agent targets.
    async fn list_agents(&self) -> Result<Vec<AgentTargetView>, InteractionError>;

    /// Resolve an agent name or ID to one configured target.
    async fn resolve_agent(&self, selector: &str) -> Result<AgentTargetView, InteractionError>;

    /// List recent and active runs for one interaction.
    async fn list_runs(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<RunSummaryView>, InteractionError>;

    /// Inspect one run.
    async fn inspect_run(&self, run_id: RunId) -> Result<RunDetailView, InteractionError>;

    /// Request cancellation of one run outside turn-scoped cancellation.
    async fn cancel_run(&self, run_id: RunId) -> Result<(), InteractionError>;

    /// Return active turns for one interaction.
    async fn active_turns(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<TurnHandle>, InteractionError>;

    /// Return real pending approval identities for one interaction.
    async fn pending_approvals(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<ApprovalId>, InteractionError>;
}

/// Executes every typed MVP command against shared application ports.
#[derive(Clone)]
pub struct ServiceCommandExecutor {
    registry: CommandRegistry,
    interactions: Arc<dyn InteractionService>,
    runtime: Arc<dyn InteractionCommandRuntime>,
}

impl std::fmt::Debug for ServiceCommandExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServiceCommandExecutor")
            .field("registry", &self.registry)
            .field("interactions", &"<interaction service>")
            .field("runtime", &"<command runtime>")
            .finish()
    }
}

impl ServiceCommandExecutor {
    /// Compose handlers over an interaction service and runtime read model.
    pub fn new(
        registry: CommandRegistry,
        interactions: Arc<dyn InteractionService>,
        runtime: Arc<dyn InteractionCommandRuntime>,
    ) -> Result<Self, InteractionError> {
        registry.validate()?;
        Ok(Self {
            registry,
            interactions,
            runtime,
        })
    }

    /// Access the canonical registry used for authorization and help output.
    #[must_use]
    pub const fn registry(&self) -> &CommandRegistry {
        &self.registry
    }
}

#[async_trait]
impl CommandExecutor for ServiceCommandExecutor {
    async fn execute(&self, request: CommandRequest) -> Result<CommandOutput, InteractionError> {
        self.authorize(&request)?;
        let command = request.invocation.command.clone();
        match command {
            InteractionCommand::Help { command } => {
                let commands = match command {
                    Some(command) => self
                        .registry
                        .specs()
                        .into_iter()
                        .filter(|spec| spec.command == command)
                        .cloned()
                        .collect(),
                    None => self
                        .registry
                        .available(&request.context)
                        .into_iter()
                        .cloned()
                        .collect(),
                };
                Ok(CommandOutput::Help { commands })
            }
            InteractionCommand::Status => {
                let conversation_id = selected_interaction(&request)?;
                let (interaction, active_turns, pending_approvals) = tokio::try_join!(
                    self.interactions.load_interaction(conversation_id),
                    self.runtime.active_turns(conversation_id),
                    self.runtime.pending_approvals(conversation_id),
                )?;
                Ok(CommandOutput::Status {
                    interaction,
                    active_turns,
                    pending_approvals,
                })
            }
            InteractionCommand::Agents => Ok(CommandOutput::Agents {
                agents: self.runtime.list_agents().await?,
            }),
            InteractionCommand::Agent { selector } => {
                let conversation_id = selected_interaction(&request)?;
                let agent = self.runtime.resolve_agent(&selector).await?;
                let config = self
                    .interactions
                    .set_config_option(
                        conversation_id,
                        ConfigUpdate {
                            option: ConfigOption::Target,
                            value: ConfigOptionValue::Target(InteractionTarget::Agent(
                                agent.agent_id,
                            )),
                        },
                    )
                    .await?;
                Ok(CommandOutput::TargetChanged {
                    target: config.target,
                })
            }
            InteractionCommand::New { title } => {
                let config = self.runtime.default_interaction_config().await?;
                let interaction = self
                    .interactions
                    .new_interaction(CreateInteractionRequest {
                        title,
                        config,
                        client_context: request.client_context,
                    })
                    .await?;
                Ok(CommandOutput::InteractionCreated { interaction })
            }
            InteractionCommand::Resume { conversation_id } => {
                let interaction = self.interactions.load_interaction(conversation_id).await?;
                Ok(CommandOutput::InteractionResumed { interaction })
            }
            InteractionCommand::Runs => {
                let conversation_id = selected_interaction(&request)?;
                Ok(CommandOutput::Runs {
                    runs: self.runtime.list_runs(conversation_id).await?,
                })
            }
            InteractionCommand::Inspect { run_id } => Ok(CommandOutput::RunInspected {
                run: self.runtime.inspect_run(run_id).await?,
            }),
            InteractionCommand::Cancel { target } => self.cancel(&request, target).await,
            InteractionCommand::Approve { approval_id } => {
                let conversation_id = selected_interaction(&request)?;
                self.interactions
                    .approve(conversation_id, approval_id)
                    .await?;
                Ok(CommandOutput::ApprovalResolved {
                    approval_id,
                    decision: ApprovalDecision::Approve,
                })
            }
            InteractionCommand::Deny {
                approval_id,
                reason,
            } => {
                let conversation_id = selected_interaction(&request)?;
                self.interactions
                    .deny(conversation_id, approval_id, reason.clone())
                    .await?;
                Ok(CommandOutput::ApprovalResolved {
                    approval_id,
                    decision: ApprovalDecision::Deny { reason },
                })
            }
            InteractionCommand::Model { model } => {
                let conversation_id = selected_interaction(&request)?;
                let changed = model.is_some();
                let model = if changed {
                    self.interactions
                        .set_config_option(
                            conversation_id,
                            ConfigUpdate {
                                option: ConfigOption::Model,
                                value: ConfigOptionValue::Model(model),
                            },
                        )
                        .await?
                        .model
                } else {
                    self.interactions
                        .load_interaction(conversation_id)
                        .await?
                        .config
                        .model
                };
                Ok(CommandOutput::Model { model, changed })
            }
        }
    }
}

impl ServiceCommandExecutor {
    fn authorize(&self, request: &CommandRequest) -> Result<(), InteractionError> {
        let command = request.invocation.command.name();
        let spec = self
            .registry
            .specs()
            .into_iter()
            .find(|spec| spec.command == command)
            .ok_or_else(|| {
                InteractionError::new(
                    InteractionErrorCode::Internal,
                    "parsed command is absent from the command registry",
                )
            })?;
        if !spec.is_available(&request.context) {
            let code =
                if spec.mutability == CommandMutability::Mutating && !request.context.can_mutate {
                    InteractionErrorCode::PermissionDenied
                } else {
                    InteractionErrorCode::Conflict
                };
            return Err(InteractionError::new(
                code,
                format!("/{} is unavailable in the current context", spec.name),
            ));
        }
        if matches!(
            request.invocation.command,
            InteractionCommand::Model { model: Some(_) }
        ) && !request.context.can_mutate
        {
            return Err(InteractionError::new(
                InteractionErrorCode::PermissionDenied,
                "/model mutation is unavailable in read-only mode",
            ));
        }
        Ok(())
    }

    async fn cancel(
        &self,
        request: &CommandRequest,
        target: CancelTarget,
    ) -> Result<CommandOutput, InteractionError> {
        let conversation_id = selected_interaction(request)?;
        match target {
            CancelTarget::Run(run_id) => {
                self.runtime.cancel_run(run_id).await?;
                Ok(CommandOutput::CancellationRequested {
                    turn_id: None,
                    run_ids: vec![run_id],
                })
            }
            CancelTarget::CurrentTurn => {
                let active = self.runtime.active_turns(conversation_id).await?;
                let [turn] = active.as_slice() else {
                    let message = if active.is_empty() {
                        "the selected interaction has no active turn"
                    } else {
                        "the selected interaction has multiple active turns; use /cancel all"
                    };
                    return Err(InteractionError::new(
                        InteractionErrorCode::Conflict,
                        message,
                    ));
                };
                self.interactions.cancel_turn(turn.turn_id).await?;
                Ok(CommandOutput::CancellationRequested {
                    turn_id: Some(turn.turn_id),
                    run_ids: turn.run_ids.clone(),
                })
            }
            CancelTarget::All => {
                let active = self.runtime.active_turns(conversation_id).await?;
                if active.is_empty() {
                    return Err(InteractionError::new(
                        InteractionErrorCode::Conflict,
                        "the selected interaction has no active turns",
                    ));
                }
                let mut run_ids = Vec::new();
                for turn in &active {
                    self.interactions.cancel_turn(turn.turn_id).await?;
                    run_ids.extend(turn.run_ids.iter().copied());
                }
                Ok(CommandOutput::CancellationRequested {
                    turn_id: None,
                    run_ids,
                })
            }
        }
    }
}

fn selected_interaction(request: &CommandRequest) -> Result<ConversationId, InteractionError> {
    request.context.conversation_id.ok_or_else(|| {
        InteractionError::new(
            InteractionErrorCode::Conflict,
            "select or create an interaction before using this command",
        )
    })
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "command-handler tests intentionally fail at exact fake service and typed-output boundaries"
)]
mod tests {
    use std::path::PathBuf;

    use chrono::Utc;
    use polkagent_core::ids::{AgentId, ApprovalId};
    use tokio::sync::Mutex;

    use super::*;
    use crate::command::{CommandContext, ParsedLine};
    use crate::ids::InteractionTurnId;
    use crate::model::{
        ClientContext, InteractionState, InteractionSummary, ListInteractionsRequest,
        PromptRequest, SubscriptionRequest, TurnSummary,
    };
    use crate::service::{BoxInteractionEventStream, StartedTurn};

    struct FakeInteractions {
        summary: Mutex<InteractionSummary>,
        cancelled: Mutex<Vec<InteractionTurnId>>,
        decisions: Mutex<Vec<(ApprovalId, ApprovalDecision)>>,
    }

    #[async_trait]
    impl InteractionService for FakeInteractions {
        async fn new_interaction(
            &self,
            request: CreateInteractionRequest,
        ) -> Result<InteractionSummary, InteractionError> {
            let now = Utc::now();
            let summary = InteractionSummary {
                conversation_id: ConversationId::new(),
                title: request.title,
                config: request.config,
                state: InteractionState::Active,
                turn_count: 0,
                created_at: now,
                updated_at: now,
            };
            *self.summary.lock().await = summary.clone();
            Ok(summary)
        }

        async fn list_interactions(
            &self,
            _request: ListInteractionsRequest,
        ) -> Result<Vec<InteractionSummary>, InteractionError> {
            Ok(vec![self.summary.lock().await.clone()])
        }

        async fn load_interaction(
            &self,
            conversation_id: ConversationId,
        ) -> Result<InteractionSummary, InteractionError> {
            let summary = self.summary.lock().await.clone();
            if summary.conversation_id == conversation_id {
                Ok(summary)
            } else {
                Err(InteractionError::new(
                    InteractionErrorCode::NotFound,
                    "interaction not found",
                ))
            }
        }

        async fn list_turns(
            &self,
            _conversation_id: ConversationId,
        ) -> Result<Vec<TurnSummary>, InteractionError> {
            Ok(Vec::new())
        }

        async fn delete_interaction(
            &self,
            _conversation_id: ConversationId,
        ) -> Result<(), InteractionError> {
            Ok(())
        }

        async fn prompt(&self, _request: PromptRequest) -> Result<StartedTurn, InteractionError> {
            Err(InteractionError::new(
                InteractionErrorCode::Unsupported,
                "not used by command tests",
            ))
        }

        async fn cancel_turn(&self, turn_id: InteractionTurnId) -> Result<(), InteractionError> {
            self.cancelled.lock().await.push(turn_id);
            Ok(())
        }

        async fn set_config_option(
            &self,
            conversation_id: ConversationId,
            update: ConfigUpdate,
        ) -> Result<InteractionConfig, InteractionError> {
            update.validate()?;
            let mut summary = self.summary.lock().await;
            if summary.conversation_id != conversation_id {
                return Err(InteractionError::new(
                    InteractionErrorCode::NotFound,
                    "interaction not found",
                ));
            }
            match update.value {
                ConfigOptionValue::Target(value) => summary.config.target = value,
                ConfigOptionValue::Model(value) => summary.config.model = value,
                ConfigOptionValue::Provider(value) => summary.config.provider = value,
                ConfigOptionValue::Harness(value) => summary.config.harness = value,
                ConfigOptionValue::Autonomy(value) => summary.config.autonomy = value,
                ConfigOptionValue::MaxTurns(value) => summary.config.max_turns = value,
                ConfigOptionValue::Budget(value) => summary.config.budget = value,
            }
            Ok(summary.config.clone())
        }

        async fn approve(
            &self,
            _conversation_id: ConversationId,
            approval_id: ApprovalId,
        ) -> Result<(), InteractionError> {
            self.decisions
                .lock()
                .await
                .push((approval_id, ApprovalDecision::Approve));
            Ok(())
        }

        async fn deny(
            &self,
            _conversation_id: ConversationId,
            approval_id: ApprovalId,
            reason: Option<String>,
        ) -> Result<(), InteractionError> {
            self.decisions
                .lock()
                .await
                .push((approval_id, ApprovalDecision::Deny { reason }));
            Ok(())
        }

        async fn subscribe(
            &self,
            _request: SubscriptionRequest,
        ) -> Result<BoxInteractionEventStream, InteractionError> {
            Err(InteractionError::new(
                InteractionErrorCode::Unsupported,
                "not used by command tests",
            ))
        }
    }

    struct FakeRuntime {
        agent: AgentTargetView,
        active: Vec<TurnHandle>,
        approval_id: ApprovalId,
        run: RunSummaryView,
        cancelled_runs: Mutex<Vec<RunId>>,
    }

    #[async_trait]
    impl InteractionCommandRuntime for FakeRuntime {
        async fn default_interaction_config(&self) -> Result<InteractionConfig, InteractionError> {
            Ok(InteractionConfig::new(InteractionTarget::Agent(
                self.agent.agent_id,
            )))
        }

        async fn list_agents(&self) -> Result<Vec<AgentTargetView>, InteractionError> {
            Ok(vec![self.agent.clone()])
        }

        async fn resolve_agent(&self, selector: &str) -> Result<AgentTargetView, InteractionError> {
            if selector == self.agent.name || selector == self.agent.agent_id.to_string() {
                Ok(self.agent.clone())
            } else {
                Err(InteractionError::new(
                    InteractionErrorCode::NotFound,
                    "agent not found",
                ))
            }
        }

        async fn list_runs(
            &self,
            _conversation_id: ConversationId,
        ) -> Result<Vec<RunSummaryView>, InteractionError> {
            Ok(vec![self.run.clone()])
        }

        async fn inspect_run(&self, run_id: RunId) -> Result<RunDetailView, InteractionError> {
            if run_id != self.run.run_id {
                return Err(InteractionError::new(
                    InteractionErrorCode::NotFound,
                    "run not found",
                ));
            }
            Ok(RunDetailView {
                run: self.run.clone(),
                artifacts: vec!["artifact-1".to_owned()],
                error: None,
            })
        }

        async fn cancel_run(&self, run_id: RunId) -> Result<(), InteractionError> {
            self.cancelled_runs.lock().await.push(run_id);
            Ok(())
        }

        async fn active_turns(
            &self,
            _conversation_id: ConversationId,
        ) -> Result<Vec<TurnHandle>, InteractionError> {
            Ok(self.active.clone())
        }

        async fn pending_approvals(
            &self,
            _conversation_id: ConversationId,
        ) -> Result<Vec<ApprovalId>, InteractionError> {
            Ok(vec![self.approval_id])
        }
    }

    struct Fixture {
        executor: ServiceCommandExecutor,
        interactions: Arc<FakeInteractions>,
        runtime: Arc<FakeRuntime>,
        conversation_id: ConversationId,
    }

    fn fixture() -> Fixture {
        let conversation_id = ConversationId::new();
        let agent_id = AgentId::new();
        let run_id = RunId::new();
        let turn_id = InteractionTurnId::new();
        let now = Utc::now();
        let summary = InteractionSummary {
            conversation_id,
            title: Some("Existing".to_owned()),
            config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
            state: InteractionState::Active,
            turn_count: 1,
            created_at: now,
            updated_at: now,
        };
        let interactions = Arc::new(FakeInteractions {
            summary: Mutex::new(summary),
            cancelled: Mutex::new(Vec::new()),
            decisions: Mutex::new(Vec::new()),
        });
        let approval_id = ApprovalId::new();
        let runtime = Arc::new(FakeRuntime {
            agent: AgentTargetView {
                agent_id,
                name: "primary".to_owned(),
                state: "active".to_owned(),
                ready: true,
            },
            active: vec![TurnHandle {
                turn_id,
                conversation_id,
                run_ids: vec![run_id],
                first_event_sequence: 1,
            }],
            approval_id,
            run: RunSummaryView {
                run_id,
                agent_id,
                state: "working".to_owned(),
                summary: None,
            },
            cancelled_runs: Mutex::new(Vec::new()),
        });
        let interaction_service: Arc<dyn InteractionService> = interactions.clone();
        let command_runtime: Arc<dyn InteractionCommandRuntime> = runtime.clone();
        let executor = ServiceCommandExecutor::new(
            CommandRegistry::mvp(),
            interaction_service,
            command_runtime,
        )
        .expect("construct command executor");
        Fixture {
            executor,
            interactions,
            runtime,
            conversation_id,
        }
    }

    fn request(line: &str, conversation_id: Option<ConversationId>) -> CommandRequest {
        let registry = CommandRegistry::mvp();
        let ParsedLine::Command(invocation) = registry.parse(line).expect("parse command") else {
            panic!("expected slash command")
        };
        CommandRequest {
            invocation,
            context: CommandContext {
                conversation_id,
                has_active_turn: conversation_id.is_some(),
                pending_approval_count: usize::from(conversation_id.is_some()),
                can_mutate: true,
            },
            client_context: ClientContext::new(PathBuf::from("/tmp/project"))
                .expect("absolute client context"),
        }
    }

    #[tokio::test]
    async fn discovery_creation_resume_and_config_handlers_are_wired() {
        let fixture = fixture();
        let help = fixture
            .executor
            .execute(request("/help", None))
            .await
            .expect("help");
        let CommandOutput::Help { commands } = help else {
            panic!("expected help output")
        };
        assert!(commands.iter().any(|command| command.name == "new"));
        assert!(!commands.iter().any(|command| command.name == "status"));

        let agents = fixture
            .executor
            .execute(request("/agents", None))
            .await
            .expect("agents");
        assert!(matches!(agents, CommandOutput::Agents { agents } if agents.len() == 1));

        let created = fixture
            .executor
            .execute(request("/new Fresh session", None))
            .await
            .expect("new interaction");
        let CommandOutput::InteractionCreated { interaction } = created else {
            panic!("expected created interaction")
        };
        assert_eq!(interaction.title.as_deref(), Some("Fresh session"));

        let resumed = fixture
            .executor
            .execute(request(
                &format!("/resume {}", interaction.conversation_id),
                None,
            ))
            .await
            .expect("resume interaction");
        assert!(matches!(resumed, CommandOutput::InteractionResumed { .. }));

        let target = fixture
            .executor
            .execute(request("/agent primary", Some(interaction.conversation_id)))
            .await
            .expect("select agent");
        assert!(matches!(target, CommandOutput::TargetChanged { .. }));
        let model = fixture
            .executor
            .execute(request("/model model-x", Some(interaction.conversation_id)))
            .await
            .expect("select model");
        assert!(matches!(
            model,
            CommandOutput::Model {
                model: Some(model),
                changed: true
            } if model == "model-x"
        ));
    }

    #[tokio::test]
    async fn status_run_cancel_and_approval_handlers_use_real_ids() {
        let fixture = fixture();
        let context = Some(fixture.conversation_id);
        assert!(matches!(
            fixture
                .executor
                .execute(request("/status", context))
                .await
                .expect("status"),
            CommandOutput::Status { .. }
        ));
        assert!(matches!(
            fixture
                .executor
                .execute(request("/runs", context))
                .await
                .expect("runs"),
            CommandOutput::Runs { runs } if runs.len() == 1
        ));
        assert!(matches!(
            fixture
                .executor
                .execute(request(
                    &format!("/inspect {}", fixture.runtime.run.run_id),
                    context,
                ))
                .await
                .expect("inspect"),
            CommandOutput::RunInspected { .. }
        ));

        let cancelled = fixture
            .executor
            .execute(request("/cancel", context))
            .await
            .expect("cancel current turn");
        assert!(matches!(
            cancelled,
            CommandOutput::CancellationRequested {
                turn_id: Some(_),
                ..
            }
        ));
        assert_eq!(fixture.interactions.cancelled.lock().await.len(), 1);

        let approval_id = fixture.runtime.approval_id;
        fixture
            .executor
            .execute(request(&format!("/approve {approval_id}"), context))
            .await
            .expect("approve");
        fixture
            .executor
            .execute(request(
                &format!("/deny {approval_id} needs review"),
                context,
            ))
            .await
            .expect("deny");
        assert_eq!(fixture.interactions.decisions.lock().await.len(), 2);
    }

    #[tokio::test]
    async fn conditional_model_mutation_respects_read_only_context() {
        let fixture = fixture();
        let mut request = request("/model model-x", Some(fixture.conversation_id));
        request.context.can_mutate = false;
        let error = fixture
            .executor
            .execute(request)
            .await
            .expect_err("read-only model mutation");
        assert_eq!(error.code, InteractionErrorCode::PermissionDenied);
    }
}

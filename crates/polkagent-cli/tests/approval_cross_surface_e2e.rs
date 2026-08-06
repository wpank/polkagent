//! Cross-surface evidence for one real durable approval.
//!
//! The fixture uses the production `RuntimeFactory`, a file-backed `SQLite`
//! database, a registered governance tool, a policy that requires approval,
//! and a deterministic local model provider. Approval reads and decisions go
//! only through the TUI controller and the shared slash-command executor.

#![allow(
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "one contiguous fixture makes the cross-surface durable identities and exactly-once evidence auditable"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use polkagent_api::{AgentStore as _, RuntimeAgentStore};
use polkagent_cli::tui::interaction::{
    ApprovalDecisionRequest, ApprovalListRequest, ControllerEvent, RunController,
};
use polkagent_core::{AgentId, AgentSpec, ApprovalId, ConversationId, PrincipalId, RunId};
use polkagent_interaction::{
    AgentTargetView, ApprovalDecision, ApprovalStatus, ApprovalView, BoxInteractionEventStream,
    ClientContext, CommandContext, CommandExecutor as _, CommandOutput, CommandRegistry,
    CommandRequest, CreateInteractionRequest, InteractionApprovalAuthority,
    InteractionCommandRuntime, InteractionConfig, InteractionContent, InteractionError,
    InteractionErrorCode, InteractionEvent, InteractionOverrides, InteractionService as _,
    InteractionTarget, ParsedLine, PromptRequest, RunDetailView, RunSummaryView,
    ServiceCommandExecutor, StreamError, TurnHandle,
};
use polkagent_runtime::{AdapterPolicy, PolkagentRuntime, RuntimeFactory, RuntimeOptions};
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};

const AGENT_NAME: &str = "apr08-approval-agent";
const PROVIDER_ID: &str = "apr08-provider";
const MODEL: &str = "fixture-model";
const TOOL: &str = "polkagent.governance.referendum_lookup";
const PROVIDER_TOOL_CALL_ID: &str = "call_apr08_sensitive_1";
const SENSITIVE_TOOL_ARGUMENT: &str = "APR08-RAW-ARG-MUST-NOT-REACH-SURFACES";
const TENANT: &str = "apr08-tenant";
const WORKSPACE: &str = "apr08-workspace";
const API_KEY_ENV: &str = "POLKAGENT_APR08_FIXTURE_KEY";

struct ProviderFixture {
    base_url: String,
    requests: tokio::sync::mpsc::Receiver<serde_json::Value>,
    task: tokio::task::JoinHandle<()>,
}

struct EnvironmentVariableGuard(&'static str);

impl EnvironmentVariableGuard {
    fn set(name: &'static str, value: &str) -> Self {
        std::env::set_var(name, value);
        Self(name)
    }
}

impl Drop for EnvironmentVariableGuard {
    fn drop(&mut self) {
        std::env::remove_var(self.0);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct DurableDatabaseEvidence {
    approval_status: String,
    run_state: String,
    effect_attempts: i64,
    effect_outcomes: i64,
    pending_approvals: i64,
    orphan_approvals: i64,
    subject_json: String,
}

#[derive(Debug)]
struct ApprovalIdentity {
    conversation: ConversationId,
    approval: ApprovalId,
    effect: polkagent_core::EffectId,
    run: RunId,
    tool_call: polkagent_interaction::ToolCallId,
}

impl ApprovalIdentity {
    fn from_view(conversation_id: ConversationId, view: &ApprovalView) -> Self {
        Self {
            conversation: conversation_id,
            approval: view.approval_id,
            effect: view.effect_id,
            run: view.run_id,
            tool_call: view.tool_call_id.expect("approval tool-call identity"),
        }
    }

    fn assert_view(&self, conversation_id: ConversationId, view: &ApprovalView) {
        assert_eq!(conversation_id, self.conversation);
        assert_eq!(view.approval_id, self.approval);
        assert_eq!(view.effect_id, self.effect);
        assert_eq!(view.run_id, self.run);
        assert_eq!(view.tool_call_id, Some(self.tool_call));
    }
}

fn authority(principal_id: PrincipalId) -> InteractionApprovalAuthority {
    InteractionApprovalAuthority {
        tenant_id: TENANT.to_owned(),
        workspace_id: WORKSPACE.to_owned(),
        principal_id,
        surface: "apr08-tui-chat".to_owned(),
    }
}

fn write_policy(root: &Path) -> PathBuf {
    let directory = root.join("policies");
    std::fs::create_dir_all(&directory).expect("create APR-08 policy directory");
    std::fs::write(
        directory.join("approval.toml"),
        format!(
            "[[rules]]\n\
             id = \"apr08-review-registered-tool\"\n\
             effect = \"require_approval\"\n\
             action_patterns = [\"chain.query\"]\n\
             resource_patterns = [\"tool/{TOOL}\"]\n"
        ),
    )
    .expect("write APR-08 approval policy");
    directory
}

fn write_config(root: &Path, provider_base_url: &str, policy_directory: &Path) -> PathBuf {
    let path = root.join("polkagent.toml");
    std::fs::write(
        &path,
        format!(
            "[[providers]]\n\
             id = \"{PROVIDER_ID}\"\n\
             provider_type = \"local\"\n\
             base_url = \"{provider_base_url}\"\n\
             api_key_env = \"{API_KEY_ENV}\"\n\
             default_model = \"{MODEL}\"\n\
             max_retries = 0\n\
             \n\
             [policy]\n\
             enabled = true\n\
             policy_dir = \"{}\"\n\
             default_policy = \"approval\"\n",
            policy_directory.display()
        ),
    )
    .expect("write APR-08 runtime config");
    path
}

fn runtime_options(
    workdir: &Path,
    database_path: &Path,
    config_path: &Path,
    approval_authority: InteractionApprovalAuthority,
) -> RuntimeOptions {
    let mut options = RuntimeOptions::new(workdir);
    options.config_path = Some(config_path.to_path_buf());
    options.database_path = Some(database_path.to_path_buf());
    options.provider_override = Some(PROVIDER_ID.to_owned());
    options.disable_harness = true;
    options.adapter_policy = AdapterPolicy::AllowSimulated;
    options.discover_environment_providers = false;
    options.approval_authority = Some(approval_authority);
    options
}

async fn build_runtime(
    workdir: &Path,
    database_path: &Path,
    config_path: &Path,
    approval_authority: InteractionApprovalAuthority,
) -> PolkagentRuntime {
    Box::pin(RuntimeFactory::build(runtime_options(
        workdir,
        database_path,
        config_path,
        approval_authority,
    )))
    .await
    .expect("build APR-08 RuntimeFactory fixture")
}

async fn deterministic_provider() -> ProviderFixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind APR-08 provider");
    let address = listener.local_addr().expect("APR-08 provider address");
    let (requests_tx, requests_rx) = tokio::sync::mpsc::channel(2);
    let task = tokio::spawn(async move {
        for index in 0..2 {
            let (stream, _) = listener.accept().await.expect("accept provider request");
            let mut reader = BufReader::new(stream);
            let request = read_http_json_request(&mut reader).await;
            requests_tx
                .send(request)
                .await
                .expect("record APR-08 provider request");
            let message = if index == 0 {
                serde_json::json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": PROVIDER_TOOL_CALL_ID,
                        "type": "function",
                        "function": {
                            "name": TOOL,
                            "arguments": serde_json::json!({
                                "index": 1,
                                "operator_note": SENSITIVE_TOOL_ARGUMENT
                            }).to_string()
                        }
                    }]
                })
            } else {
                serde_json::json!({
                    "role": "assistant",
                    "content": "APR-08 approval flow completed"
                })
            };
            let body = serde_json::json!({
                "id": format!("chatcmpl-apr08-{index}"),
                "object": "chat.completion",
                "model": MODEL,
                "choices": [{
                    "index": 0,
                    "message": message,
                    "finish_reason": if index == 0 { "tool_calls" } else { "stop" }
                }],
                "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5}
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            reader
                .get_mut()
                .write_all(response.as_bytes())
                .await
                .expect("write APR-08 provider response");
            reader
                .get_mut()
                .shutdown()
                .await
                .expect("close APR-08 provider response");
        }
    });
    ProviderFixture {
        base_url: format!("http://{address}/v1"),
        requests: requests_rx,
        task,
    }
}

async fn read_http_json_request(
    reader: &mut BufReader<tokio::net::TcpStream>,
) -> serde_json::Value {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .expect("read provider request header");
        assert!(!line.is_empty(), "provider closed before request headers");
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
            .map(str::trim)
        {
            content_length = Some(value.parse::<usize>().expect("provider content length"));
        }
    }
    let mut body = vec![0; content_length.expect("provider request content length")];
    reader
        .read_exact(&mut body)
        .await
        .expect("read provider request body");
    serde_json::from_slice(&body).expect("parse provider request JSON")
}

async fn wait_for_pending(
    runtime: &PolkagentRuntime,
    conversation_id: ConversationId,
) -> ApprovalView {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let approvals = runtime
                .interactions()
                .list_pending_approvals(conversation_id)
                .await
                .expect("list pending APR-08 approvals");
            if let [approval] = approvals.as_slice() {
                return approval.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("durable approval did not become pending")
}

async fn next_controller_event(controller: &mut RunController) -> ControllerEvent {
    tokio::time::timeout(Duration::from_secs(5), controller.recv())
        .await
        .expect("TUI approval controller timeout")
        .expect("TUI approval controller channel closed")
}

async fn wait_for_turn_completion(events: &mut BoxInteractionEventStream) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match events.recv().await {
                Ok(envelope) => match envelope.event {
                    InteractionEvent::TurnCompleted { result } => {
                        assert_eq!(result.text, "APR-08 approval flow completed");
                        return;
                    }
                    InteractionEvent::TurnFailed { error } => {
                        panic!("APR-08 interaction turn failed: {error}")
                    }
                    InteractionEvent::TurnCancelled { reason } => {
                        panic!("APR-08 interaction turn cancelled: {reason:?}")
                    }
                    _ => {}
                },
                Err(StreamError::Lagged { .. }) => panic!("APR-08 event stream lagged"),
                Err(error) => panic!("APR-08 event stream failed: {error}"),
            }
        }
    })
    .await
    .expect("APR-08 turn completion timeout");
}

fn assert_safe_bounded_projection(view: &ApprovalView) {
    assert!(view.title.len() <= 200);
    assert!(view.description.len() <= 2_000);
    assert!(view
        .policy_reason
        .as_ref()
        .is_none_or(|reason| reason.len() <= 2_000));
    let public = serde_json::to_string(view).expect("serialize public approval projection");
    assert!(!public.contains(SENSITIVE_TOOL_ARGUMENT), "{public}");
    assert!(!public.contains(PROVIDER_TOOL_CALL_ID), "{public}");
    assert_eq!(view.status, ApprovalStatus::Pending);
}

fn database_evidence(
    database_path: &Path,
    effect_id: polkagent_core::EffectId,
) -> DurableDatabaseEvidence {
    let connection = rusqlite::Connection::open(database_path).expect("open APR-08 evidence DB");
    let (approval_status, run_state, subject_json): (String, String, String) = connection
        .query_row(
            "SELECT approval.status, run.state, approval.subject_json
             FROM approval_requests approval
             JOIN runs run ON run.id = approval.run_id
             WHERE approval.effect_id = ?1",
            [effect_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("load APR-08 durable approval evidence");
    let count = |sql: &str| {
        connection
            .query_row(sql, [effect_id.to_string()], |row| row.get::<_, i64>(0))
            .expect("count APR-08 durable evidence")
    };
    let effect_attempts = count("SELECT COUNT(*) FROM effect_attempts WHERE intent_id = ?1");
    let effect_outcomes = count("SELECT COUNT(*) FROM effect_outcomes WHERE intent_id = ?1");
    let pending_approvals: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM approval_requests WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )
        .expect("count pending approvals");
    let orphan_approvals: i64 = connection
        .query_row(
            "SELECT COUNT(*)
             FROM approval_requests approval
             LEFT JOIN effect_intents effect ON effect.id = approval.effect_id
             LEFT JOIN runs run ON run.id = approval.run_id
             LEFT JOIN turns turn_row ON turn_row.id = approval.turn_id
             LEFT JOIN agents agent ON agent.id = approval.agent_id
             WHERE effect.id IS NULL OR run.id IS NULL OR turn_row.id IS NULL OR agent.id IS NULL",
            [],
            |row| row.get(0),
        )
        .expect("count orphan approvals");
    DurableDatabaseEvidence {
        approval_status,
        run_state,
        effect_attempts,
        effect_outcomes,
        pending_approvals,
        orphan_approvals,
        subject_json,
    }
}

struct ApprovalOnlyCommandRuntime;

fn unused_command_error() -> InteractionError {
    InteractionError::new(
        InteractionErrorCode::Unsupported,
        "the APR-08 fixture exercises only shared approval commands",
    )
}

#[async_trait]
impl InteractionCommandRuntime for ApprovalOnlyCommandRuntime {
    async fn default_interaction_config(&self) -> Result<InteractionConfig, InteractionError> {
        Err(unused_command_error())
    }

    async fn list_agents(&self) -> Result<Vec<AgentTargetView>, InteractionError> {
        Err(unused_command_error())
    }

    async fn resolve_agent(&self, _selector: &str) -> Result<AgentTargetView, InteractionError> {
        Err(unused_command_error())
    }

    async fn list_runs(
        &self,
        _conversation_id: ConversationId,
    ) -> Result<Vec<RunSummaryView>, InteractionError> {
        Err(unused_command_error())
    }

    async fn inspect_run(&self, _run_id: RunId) -> Result<RunDetailView, InteractionError> {
        Err(unused_command_error())
    }

    async fn cancel_run(&self, _run_id: RunId) -> Result<(), InteractionError> {
        Err(unused_command_error())
    }

    async fn active_turns(
        &self,
        _conversation_id: ConversationId,
    ) -> Result<Vec<TurnHandle>, InteractionError> {
        Err(unused_command_error())
    }

    async fn pending_approvals(
        &self,
        _conversation_id: ConversationId,
    ) -> Result<Vec<ApprovalId>, InteractionError> {
        Err(unused_command_error())
    }
}

fn slash_executor(runtime: &PolkagentRuntime) -> (CommandRegistry, ServiceCommandExecutor) {
    let registry = CommandRegistry::mvp();
    let service: Arc<dyn polkagent_interaction::InteractionService> =
        runtime.interactions().clone();
    let executor = ServiceCommandExecutor::new(
        registry.clone(),
        service,
        Arc::new(ApprovalOnlyCommandRuntime),
    )
    .expect("compose shared slash-command executor");
    (registry, executor)
}

async fn execute_slash(
    registry: &CommandRegistry,
    executor: &ServiceCommandExecutor,
    line: &str,
    conversation_id: ConversationId,
    pending_approval_count: usize,
    client_context: ClientContext,
) -> Result<CommandOutput, InteractionError> {
    let ParsedLine::Command(invocation) = registry.parse(line).expect("parse approval command")
    else {
        panic!("approval slash input parsed as a prompt")
    };
    executor
        .execute(CommandRequest {
            invocation,
            context: CommandContext {
                conversation_id: Some(conversation_id),
                has_active_turn: false,
                pending_approval_count,
                can_mutate: true,
            },
            client_context,
        })
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_runtime_approval_is_identical_once_only_and_safe_across_tui_chat_and_restart() {
    let temporary = tempfile::tempdir().expect("APR-08 temporary directory");
    let workdir = std::fs::canonicalize(temporary.path()).expect("canonical APR-08 workdir");
    let database_path = workdir.join("apr08.db");
    let policy_directory = write_policy(&workdir);
    let mut provider = deterministic_provider().await;
    let config_path = write_config(&workdir, &provider.base_url, &policy_directory);
    let _api_key = EnvironmentVariableGuard::set(API_KEY_ENV, "apr08-fixture-key");

    let principal_id = "018f4d71-46c7-7a31-8c63-b9020f278b08"
        .parse::<PrincipalId>()
        .expect("APR-08 principal ID");
    let runtime = build_runtime(
        &workdir,
        &database_path,
        &config_path,
        authority(principal_id),
    )
    .await;
    assert!(runtime.interactions().approval_authority_bound());
    assert!(runtime.app().approval_executor_ready());

    let agent_id = AgentId::new();
    let mut agent = AgentSpec::new(agent_id, AGENT_NAME, MODEL);
    agent.tools = vec![TOOL.to_owned()];
    RuntimeAgentStore::from_runtime(&runtime)
        .insert(agent)
        .await
        .expect("persist registered-tool APR-08 agent");

    let mut client_context = ClientContext::new(workdir.clone()).expect("APR-08 client context");
    client_context.client_name = Some("apr08-cross-surface".to_owned());
    let interaction = runtime
        .interactions()
        .new_interaction(CreateInteractionRequest {
            title: Some("APR-08 exact approval".to_owned()),
            config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
            client_context: client_context.clone(),
        })
        .await
        .expect("create APR-08 interaction");
    let other_interaction = runtime
        .interactions()
        .new_interaction(CreateInteractionRequest {
            title: Some("APR-08 wrong scope".to_owned()),
            config: InteractionConfig::new(InteractionTarget::Agent(agent_id)),
            client_context: client_context.clone(),
        })
        .await
        .expect("create APR-08 foreign interaction");
    let mut started = runtime
        .interactions()
        .prompt(PromptRequest {
            turn_id: None,
            conversation_id: interaction.conversation_id,
            content: vec![InteractionContent::Text {
                text: "Use the registered tool; keep raw arguments off approval surfaces."
                    .to_owned(),
            }],
            config_overrides: InteractionOverrides::default(),
            client_context: client_context.clone(),
        })
        .await
        .expect("start APR-08 approval turn");
    assert_eq!(started.handle.conversation_id, interaction.conversation_id);
    assert_eq!(started.handle.run_ids.len(), 1);

    let pending = wait_for_pending(&runtime, interaction.conversation_id).await;
    assert_eq!(pending.run_id, started.handle.run_ids[0]);
    assert_safe_bounded_projection(&pending);

    let mut tui = RunController::new(runtime.clone());
    let list_request_id = uuid::Uuid::now_v7();
    tui.list_approvals(ApprovalListRequest {
        request_id: list_request_id,
        conversation_id: interaction.conversation_id,
    })
    .expect("dispatch TUI approval list");
    let listed = match next_controller_event(&mut tui).await {
        ControllerEvent::ApprovalListLoaded {
            request_id,
            conversation_id,
            approvals,
            total_count,
        } => {
            assert_eq!(request_id, list_request_id);
            assert_eq!(conversation_id, interaction.conversation_id);
            assert_eq!(total_count, 1);
            assert!(approvals.len() <= 100);
            let [approval] = approvals.as_slice() else {
                panic!("TUI must list one exact durable approval")
            };
            assert_eq!(approval, &pending);
            approval.clone()
        }
        event => panic!("unexpected TUI list event: {event:?}"),
    };
    let identity = ApprovalIdentity::from_view(interaction.conversation_id, &listed);

    let (initial_registry, initial_slash) = slash_executor(&runtime);
    let wrong_conversation = execute_slash(
        &initial_registry,
        &initial_slash,
        &format!("/approve {}", identity.approval),
        other_interaction.conversation_id,
        1,
        client_context.clone(),
    )
    .await
    .expect_err("foreign conversation must not resolve approval");
    assert_eq!(
        wrong_conversation.code,
        InteractionErrorCode::PermissionDenied
    );
    assert_eq!(
        runtime
            .interactions()
            .list_pending_approvals(interaction.conversation_id)
            .await
            .expect("approval remains after wrong conversation")
            .len(),
        1
    );

    let decision_request_id = uuid::Uuid::now_v7();
    tui.decide_approval(ApprovalDecisionRequest {
        request_id: decision_request_id,
        conversation_id: identity.conversation,
        approval_id: identity.approval,
        decision: ApprovalDecision::Approve,
    })
    .expect("dispatch TUI approval decision");
    let decided = match next_controller_event(&mut tui).await {
        ControllerEvent::ApprovalDecisionCompleted {
            request_id,
            conversation_id,
            approval_id,
            decision,
            approval,
        } => {
            assert_eq!(request_id, decision_request_id);
            assert_eq!(approval_id, identity.approval);
            assert_eq!(decision, ApprovalDecision::Approve);
            identity.assert_view(conversation_id, &approval);
            approval
        }
        event => panic!("unexpected TUI decision event: {event:?}"),
    };
    assert_eq!(decided.status, ApprovalStatus::Approved);

    wait_for_turn_completion(&mut started.events).await;
    let first_request = provider
        .requests
        .recv()
        .await
        .expect("first APR-08 provider request");
    let second_request = provider
        .requests
        .recv()
        .await
        .expect("second APR-08 provider request");
    provider.task.await.expect("APR-08 provider completed");
    assert!(first_request
        .to_string()
        .contains("Use the registered tool"));
    assert!(second_request.to_string().contains(PROVIDER_TOOL_CALL_ID));
    assert!(second_request.to_string().contains(SENSITIVE_TOOL_ARGUMENT));

    let before_restart = database_evidence(&database_path, identity.effect);
    assert_eq!(before_restart.approval_status, "approved");
    assert_eq!(before_restart.run_state, "completed");
    assert_eq!(before_restart.effect_attempts, 1);
    assert_eq!(before_restart.effect_outcomes, 1);
    assert_eq!(before_restart.pending_approvals, 0);
    assert_eq!(before_restart.orphan_approvals, 0);
    assert!(before_restart
        .subject_json
        .contains(SENSITIVE_TOOL_ARGUMENT));
    assert!(before_restart.subject_json.contains(PROVIDER_TOOL_CALL_ID));

    tui.shutdown().await;
    drop(initial_slash);
    drop(runtime);

    let restarted = build_runtime(
        &workdir,
        &database_path,
        &config_path,
        authority(principal_id),
    )
    .await;
    let turns = restarted
        .interactions()
        .list_turns(identity.conversation)
        .await
        .expect("load exact interaction after restart");
    let [turn] = turns.as_slice() else {
        panic!("restart must retain one exact interaction turn")
    };
    assert_eq!(turn.handle, started.handle);
    assert_eq!(turn.handle.run_ids, vec![identity.run]);

    let mut restarted_tui = RunController::new(restarted.clone());
    let retry_request_id = uuid::Uuid::now_v7();
    restarted_tui
        .decide_approval(ApprovalDecisionRequest {
            request_id: retry_request_id,
            conversation_id: identity.conversation,
            approval_id: identity.approval,
            decision: ApprovalDecision::Approve,
        })
        .expect("dispatch same TUI decision after restart");
    match next_controller_event(&mut restarted_tui).await {
        ControllerEvent::ApprovalDecisionCompleted {
            request_id,
            conversation_id,
            approval,
            ..
        } => {
            assert_eq!(request_id, retry_request_id);
            identity.assert_view(conversation_id, &approval);
            assert_eq!(approval.status, ApprovalStatus::Approved);
        }
        event => panic!("unexpected restarted TUI retry event: {event:?}"),
    }

    let (registry, slash) = slash_executor(&restarted);
    let same = execute_slash(
        &registry,
        &slash,
        &format!("/approve {}", identity.approval),
        identity.conversation,
        0,
        client_context.clone(),
    )
    .await
    .expect("same slash decision is idempotent after restart");
    assert_eq!(
        same,
        CommandOutput::ApprovalResolved {
            approval_id: identity.approval,
            decision: ApprovalDecision::Approve,
        }
    );
    let opposite = execute_slash(
        &registry,
        &slash,
        &format!("/deny {} opposite-decision", identity.approval),
        identity.conversation,
        0,
        client_context.clone(),
    )
    .await
    .expect_err("opposite slash decision must conflict after restart");
    assert_eq!(opposite.code, InteractionErrorCode::Conflict);
    assert!(restarted
        .interactions()
        .list_pending_approvals(identity.conversation)
        .await
        .expect("list after restart retries")
        .is_empty());
    assert_eq!(
        database_evidence(&database_path, identity.effect),
        before_restart
    );

    restarted_tui.shutdown().await;
    drop(slash);
    drop(restarted);

    let wrong_principal = PrincipalId::new();
    let wrong_runtime = build_runtime(
        &workdir,
        &database_path,
        &config_path,
        authority(wrong_principal),
    )
    .await;
    let mut wrong_tui = RunController::new(wrong_runtime.clone());
    let wrong_principal_request_id = uuid::Uuid::now_v7();
    wrong_tui
        .decide_approval(ApprovalDecisionRequest {
            request_id: wrong_principal_request_id,
            conversation_id: identity.conversation,
            approval_id: identity.approval,
            decision: ApprovalDecision::Approve,
        })
        .expect("dispatch wrong-principal TUI decision");
    match next_controller_event(&mut wrong_tui).await {
        ControllerEvent::ApprovalDecisionFailed {
            request_id,
            code,
            reason,
            ..
        } => {
            assert_eq!(request_id, wrong_principal_request_id);
            assert_eq!(code, InteractionErrorCode::PermissionDenied);
            assert!(!reason.contains(SENSITIVE_TOOL_ARGUMENT));
        }
        event => panic!("unexpected wrong-principal TUI event: {event:?}"),
    }
    wrong_tui.shutdown().await;
    assert_eq!(
        database_evidence(&database_path, identity.effect),
        before_restart
    );
}

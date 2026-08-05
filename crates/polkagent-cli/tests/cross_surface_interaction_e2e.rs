//! Cross-surface conformance for one durable interaction.
//!
//! The fixture deliberately reconstructs the production runtime between the
//! HTTP router, terminal chat, ACP subprocess, TUI controller, and final HTTP
//! readback. Configuration refusal is exercised through ACP. Cancellation is
//! kept in the existing adapter-specific process tests because cancelling this
//! one shared conversation would make the later continuation assertions
//! impossible; those tests use the same `InteractionService::cancel_turn`
//! boundary and assert both linked run and interaction-turn terminal state.

#![allow(
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "one deterministic integration scenario keeps every public adapter projection and its exact durable identities contiguous"
)]

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, LoadSessionRequest, PromptRequest as AcpPromptRequest,
    PromptResponse, SessionConfigKind, SessionConfigOption, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, StopReason, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, LineDirection};
use axum_test::TestServer;
use polkagent_api::{app_state_from_runtime, ApiServer};
use polkagent_cli::tui::{
    input::InputMode,
    interaction::{ControllerEvent, InteractionState, RunController, SessionPickerStatus},
    state::TuiState,
    theme::Theme,
    views::console,
};
use polkagent_interaction::{
    InteractionEvent, InteractionService as _, TranscriptRequest, TurnState,
};
use polkagent_runtime::{AdapterPolicy, PolkagentRuntime, RuntimeFactory, RuntimeOptions};
use polkagent_store_sqlite::SqlitePool;
use ratatui::{backend::TestBackend, Terminal};

const AGENT_NAME: &str = "cross-surface-agent";
const MODEL: &str = "fake/model-a";
const ASSISTANT: &str = "I am a fake assistant";
const CHAT_PROMPT: &str = "First prompt from terminal chat.";
const ACP_PROMPT: &str = "Second prompt from the ACP editor.";
const TUI_PROMPT: &str = "Third prompt from the TUI Console.";
const INPUT_TOKENS: u64 = 10;
const OUTPUT_TOKENS: u64 = 5;
const CONTEXT_WINDOW: u64 = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
struct DurableIdentity {
    conversation_id: String,
    turn_id: String,
    run_ids: Vec<String>,
    checkpoint: u64,
}

#[derive(Default)]
struct AcpObserved {
    command_names: Vec<String>,
    user_messages: Vec<String>,
    agent_messages: Vec<String>,
    usage_updates: Vec<(u64, u64)>,
    stdout_lines: Vec<String>,
    stderr_lines: Vec<String>,
}

fn write_config(path: &Path) {
    std::fs::write(
        path,
        format!(
            "[[providers]]\n\
             id = \"fake\"\n\
             provider_type = \"fake\"\n\
             api_key_env = \"POLKAGENT_CROSS_SURFACE_UNUSED_KEY\"\n\
             default_model = \"default-model\"\n\
             \n\
             [[models]]\n\
             slug = \"model-a\"\n\
             provider = \"fake\"\n\
             context_window = {CONTEXT_WINDOW}\n"
        ),
    )
    .expect("write cross-surface config");
}

async fn runtime_at(workdir: &Path, database_path: &Path, config_path: &Path) -> PolkagentRuntime {
    let mut options = RuntimeOptions::new(workdir);
    options.config_path = Some(config_path.to_path_buf());
    options.database_path = Some(database_path.to_path_buf());
    options.disable_harness = true;
    options.adapter_policy = AdapterPolicy::AllowSimulated;
    options.discover_environment_providers = false;
    RuntimeFactory::build(options)
        .await
        .expect("build cross-surface runtime")
}

fn http_server(runtime: &PolkagentRuntime) -> TestServer {
    TestServer::new(
        ApiServer::from_state(app_state_from_runtime(
            runtime,
            runtime.config().as_ref().clone(),
        ))
        .into_router(),
    )
}

fn chat_command(database_path: &Path, config_path: &Path, log_path: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_polkagent"));
    command
        .arg("--config")
        .arg(config_path)
        .arg("--log-file")
        .arg(log_path)
        .current_dir(
            config_path
                .parent()
                .expect("cross-surface config has a working directory"),
        )
        .env("NO_COLOR", "1")
        .env("POLKAGENT_DATABASE_SQLITE_PATH", database_path)
        .env_remove("POLKAGENT_CONFIG")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("PERPLEXITY_API_KEY")
        .env_remove("CEREBRAS_API_KEY")
        .env_remove("OLLAMA_URL")
        .env_remove("OLLAMA_MODEL")
        .env_remove("POLKAGENT_CHAIN_RPC_URL")
        .env_remove("POLKAGENT_RPC_URL");
    command
}

fn run_chat(mut command: Command, input: &str) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn terminal chat");
    child
        .stdin
        .take()
        .expect("terminal chat stdin")
        .write_all(input.as_bytes())
        .expect("write terminal chat input");
    child.wait_with_output().expect("wait for terminal chat")
}

fn assert_process_success(output: &Output, surface: &str) {
    assert!(
        output.status.success(),
        "{surface} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn line_value<'a>(text: &'a str, prefix: &str) -> &'a str {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(prefix))
        .expect("expected public adapter line")
}

fn chat_run_ids(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|line| line.trim().strip_prefix("[run "))
        .filter_map(|line| line.split_once(']').map(|(id, _)| id.to_owned()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn acp_agent(
    database_path: &Path,
    config_path: &Path,
    observed: Arc<Mutex<AcpObserved>>,
) -> AcpAgent {
    let config_arg = config_path.to_string_lossy().into_owned();
    let database_arg = database_path.to_string_lossy().into_owned();
    let config = AcpAgentConfig::new(env!("CARGO_BIN_EXE_polkagent"))
        .args([
            "--config",
            config_arg.as_str(),
            "acp",
            "--agent",
            AGENT_NAME,
            "--timeout",
            "20",
        ])
        .env("POLKAGENT_DATABASE_SQLITE_PATH", database_arg)
        .env("ANTHROPIC_API_KEY", "")
        .env("OPENAI_API_KEY", "")
        .env("GEMINI_API_KEY", "")
        .env("GOOGLE_API_KEY", "")
        .env("OPENROUTER_API_KEY", "")
        .env("PERPLEXITY_API_KEY", "")
        .env("CEREBRAS_API_KEY", "")
        .env("OLLAMA_URL", "")
        .env("OLLAMA_MODEL", "");
    AcpAgent::new(config).with_debug(move |line, direction| {
        let mut observed = observed.lock().expect("ACP observation lock");
        match direction {
            LineDirection::Stdout => observed.stdout_lines.push(line.to_owned()),
            LineDirection::Stderr => observed.stderr_lines.push(line.to_owned()),
            LineDirection::Stdin => {}
        }
    })
}

fn config_current(options: &[SessionConfigOption], id: &str) -> String {
    let option = options
        .iter()
        .find(|option| option.id.0.as_ref() == id)
        .expect("advertised ACP configuration option");
    let SessionConfigKind::Select(select) = &option.kind else {
        panic!("ACP configuration option must be a select")
    };
    select.current_value.0.to_string()
}

fn prompt_with_turn_id(
    session_id: &agent_client_protocol::schema::v1::SessionId,
    prompt: &str,
    turn_id: &str,
) -> AcpPromptRequest {
    let mut meta = serde_json::Map::new();
    meta.insert(
        "polkagent.turnId".to_owned(),
        serde_json::Value::String(turn_id.to_owned()),
    );
    AcpPromptRequest::new(
        session_id.clone(),
        vec![ContentBlock::Text(TextContent::new(prompt))],
    )
    .meta(meta)
}

fn durable_identity(response: &PromptResponse) -> DurableIdentity {
    let metadata = response
        .meta
        .as_ref()
        .and_then(|meta| meta.get("polkagent"))
        .expect("Polkagent durable ACP metadata");
    DurableIdentity {
        conversation_id: metadata["conversationId"]
            .as_str()
            .expect("ACP conversation identity")
            .to_owned(),
        turn_id: metadata["turnId"]
            .as_str()
            .expect("ACP turn identity")
            .to_owned(),
        run_ids: metadata["runIds"]
            .as_array()
            .expect("ACP linked run identities")
            .iter()
            .map(|run_id| run_id.as_str().expect("ACP run identity").to_owned())
            .collect(),
        checkpoint: metadata["checkpoint"]
            .as_u64()
            .expect("ACP durable checkpoint"),
    }
}

fn assert_protocol_stdout(lines: &[String]) {
    assert!(!lines.is_empty(), "ACP subprocess emitted no stdout frames");
    for line in lines {
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("non-JSON ACP stdout {line:?}: {error}"));
        assert_eq!(value["jsonrpc"], "2.0", "unexpected ACP frame: {line}");
    }
}

fn controller_terminal(event: &ControllerEvent) -> bool {
    matches!(
        event,
        ControllerEvent::Completed { .. }
            | ControllerEvent::Failed(_)
            | ControllerEvent::Cancelled(_)
            | ControllerEvent::TimedOut
            | ControllerEvent::CommandCompleted { .. }
            | ControllerEvent::CommandFailed { .. }
            | ControllerEvent::SessionListLoaded { .. }
            | ControllerEvent::SessionListFailed { .. }
            | ControllerEvent::SessionSelected { .. }
            | ControllerEvent::SessionSelectionFailed { .. }
    )
}

async fn wait_for_controller(controller: &mut RunController) -> Vec<ControllerEvent> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        loop {
            if let Some(event) = controller.try_recv() {
                let terminal = controller_terminal(&event);
                events.push(event);
                if terminal {
                    return events;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("TUI controller terminal event timeout")
}

fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_interaction_conforms_across_http_chat_acp_and_tui_restarts() {
    let temporary = tempfile::tempdir().expect("cross-surface tempdir");
    // macOS reports /private/var from getcwd even when tempfile yielded the
    // lexical /var symlink. Use the subprocess-visible lexical identity for
    // the durable origin shared by every surface in this fixture.
    let subprocess_workdir =
        std::fs::canonicalize(temporary.path()).expect("resolve subprocess working directory");
    let workdir = subprocess_workdir.as_path();
    let database_path = workdir.join("cross-surface.db");
    let config_path = workdir.join("polkagent.toml");
    let chat_log_path = workdir.join("chat.jsonl");
    write_config(&config_path);

    // HTTP is the first public owner: it creates the agent and exact durable
    // interaction, then persists the model without creating a turn.
    let api_runtime = runtime_at(workdir, &database_path, &config_path).await;
    let server = http_server(&api_runtime);
    let created_agent = server
        .post("/api/v1alpha1/agents")
        .json(&serde_json::json!({
            "name": AGENT_NAME,
            "model": "fake/default-model"
        }))
        .await;
    created_agent.assert_status_success();
    let agent_id = created_agent.json::<serde_json::Value>()["id"]
        .as_str()
        .expect("HTTP-created agent identity")
        .to_owned();
    let created = server
        .post("/api/v1alpha1/interactions")
        .json(&serde_json::json!({
            "title": "EVD-11 cross-surface interaction",
            "target": {"kind": "agent", "id": agent_id},
            "working_directory": workdir,
            "client_name": "cross-surface-http"
        }))
        .await;
    created.assert_status_success();
    let created_body = created.json::<serde_json::Value>();
    let conversation_id = created_body["interaction"]["conversation_id"]
        .as_str()
        .expect("HTTP-created conversation identity")
        .to_owned();
    assert_eq!(created_body["interaction"]["turn_count"], 0);
    assert_eq!(
        created_body["interaction"]["config"]["target"]["id"],
        agent_id
    );

    let configured = server
        .put(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/config"
        ))
        .json(&serde_json::json!({"option": "model", "value": "model-a"}))
        .await;
    configured.assert_status_success();
    let configured = configured.json::<serde_json::Value>();
    assert_eq!(configured["config"]["model"], MODEL);
    assert_eq!(configured["config"]["target"]["id"], agent_id);
    let initial_turns = server
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/turns"
        ))
        .await;
    initial_turns.assert_status_success();
    assert_eq!(
        initial_turns.json::<serde_json::Value>()["data"],
        serde_json::json!([])
    );
    drop(server);
    drop(api_runtime);

    // The real line-oriented subprocess resumes the HTTP-created identity.
    let mut chat = chat_command(&database_path, &config_path, &chat_log_path);
    chat.args([
        "chat",
        "--agent",
        AGENT_NAME,
        "--resume",
        conversation_id.as_str(),
    ]);
    let chat = run_chat(chat, &format!("{CHAT_PROMPT}\n"));
    assert_process_success(&chat, "terminal chat prompt");
    assert_eq!(
        String::from_utf8_lossy(&chat.stdout),
        format!("{ASSISTANT}\n")
    );
    let chat_stderr = String::from_utf8(chat.stderr).expect("UTF-8 terminal chat diagnostics");
    assert_eq!(line_value(&chat_stderr, "chat session: "), conversation_id);
    let chat_turn_id = line_value(&chat_stderr, "chat turn: ").to_owned();
    let chat_run_ids = chat_run_ids(&chat_stderr);
    assert_eq!(chat_run_ids.len(), 1, "chat diagnostics: {chat_stderr}");
    assert!(chat_stderr.contains(&format!(
        "[usage] input={INPUT_TOKENS} output={OUTPUT_TOKENS} total={} ",
        INPUT_TOKENS + OUTPUT_TOKENS
    )));

    // A separate process renders the exact durable transcript and model. The
    // slash command is public output but must not create another model turn.
    let mut chat_query = chat_command(&database_path, &config_path, &chat_log_path);
    chat_query.args([
        "chat",
        "--agent",
        AGENT_NAME,
        "--resume",
        conversation_id.as_str(),
    ]);
    let chat_query = run_chat(chat_query, "/model\n");
    assert_process_success(&chat_query, "terminal chat command");
    assert_eq!(
        String::from_utf8(chat_query.stdout).expect("UTF-8 terminal chat projection"),
        format!(
            "user> {CHAT_PROMPT}\nassistant> {ASSISTANT}\nmodel: {MODEL}\nconversation: {conversation_id}\npersistence: current durable conversation selection\n"
        )
    );
    let chat_query_stderr =
        String::from_utf8(chat_query.stderr).expect("UTF-8 terminal chat command diagnostics");
    assert_eq!(
        line_value(&chat_query_stderr, "chat session: "),
        conversation_id
    );
    assert!(!chat_query_stderr.contains("chat turn:"));

    // The official ACP client loads the same identity after another runtime
    // reconstruction, observes transcript/config, executes command/refusal
    // paths without turns, and creates exactly one follow-up.
    let observed = Arc::new(Mutex::new(AcpObserved::default()));
    let observed_notifications = Arc::clone(&observed);
    let observed_loaded = Arc::clone(&observed);
    let observed_debug = Arc::clone(&observed);
    let acp_identity = Arc::new(Mutex::new(None));
    let acp_identity_client = Arc::clone(&acp_identity);
    let acp_turn_id = uuid::Uuid::now_v7().to_string();
    let acp_turn_id_client = acp_turn_id.clone();
    let acp_session_id = agent_client_protocol::schema::v1::SessionId::new(conversation_id.clone());
    let acp_conversation_id = conversation_id.clone();
    let acp_agent_id = agent_id.clone();
    let acp_workdir = workdir.to_path_buf();
    let agent = acp_agent(&database_path, &config_path, observed_debug);

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                let mut observed = observed_notifications
                    .lock()
                    .expect("ACP notification observation lock");
                match notification.update {
                    SessionUpdate::AvailableCommandsUpdate(update) => {
                        observed.command_names.extend(
                            update
                                .available_commands
                                .into_iter()
                                .map(|command| command.name),
                        );
                    }
                    SessionUpdate::UserMessageChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            observed.user_messages.push(text.text);
                        }
                    }
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            observed.agent_messages.push(text.text);
                        }
                    }
                    SessionUpdate::UsageUpdate(usage) => {
                        observed.usage_updates.push((usage.used, usage.size));
                    }
                    _ => {}
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(
            agent,
            |connection: agent_client_protocol::ConnectionTo<Agent>| async move {
                let initialized = connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                assert!(initialized.agent_capabilities.load_session);
                let loaded = connection
                    .send_request(LoadSessionRequest::new(acp_session_id.clone(), acp_workdir))
                    .block_task()
                    .await?;
                let config = loaded
                    .config_options
                    .as_deref()
                    .expect("ACP loaded configuration projection");
                assert_eq!(config_current(config, "polkagent.agent"), acp_agent_id);
                assert_eq!(config_current(config, "model"), MODEL);
                {
                    let loaded_projection = observed_loaded
                        .lock()
                        .expect("ACP loaded transcript observation lock");
                    assert_eq!(loaded_projection.user_messages, [CHAT_PROMPT]);
                    assert_eq!(loaded_projection.agent_messages, [ASSISTANT]);
                    assert!(loaded_projection.usage_updates.is_empty());
                }

                let status = connection
                    .send_request(AcpPromptRequest::new(
                        acp_session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new("/status"))],
                    ))
                    .block_task()
                    .await?;
                assert_eq!(status.stop_reason, StopReason::EndTurn);
                let refusal = connection
                    .send_request(SetSessionConfigOptionRequest::new(
                        acp_session_id.clone(),
                        "model",
                        "missing-model",
                    ))
                    .block_task()
                    .await
                    .expect_err("ACP must refuse an unadvertised model");
                assert_eq!(
                    refusal.code,
                    agent_client_protocol::Error::invalid_params().code
                );
                assert!(
                    refusal
                        .data
                        .as_ref()
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|detail| detail.contains("missing-model")),
                    "unexpected ACP refusal: {refusal:?}"
                );

                let response = connection
                    .send_request(prompt_with_turn_id(
                        &acp_session_id,
                        ACP_PROMPT,
                        &acp_turn_id_client,
                    ))
                    .block_task()
                    .await?;
                assert_eq!(response.stop_reason, StopReason::EndTurn);
                let identity = durable_identity(&response);
                assert_eq!(identity.conversation_id, acp_conversation_id);
                assert_eq!(identity.turn_id, acp_turn_id_client);
                assert_eq!(identity.run_ids.len(), 1);
                assert!(identity.checkpoint > 0);
                *acp_identity_client
                    .lock()
                    .expect("ACP identity observation lock") = Some(identity);
                Ok(())
            },
        )
        .await
        .expect("official ACP client cross-surface session");
    let acp_identity = acp_identity
        .lock()
        .expect("ACP identity observation lock")
        .clone()
        .expect("ACP follow-up identity");
    {
        let observed = observed.lock().expect("ACP observation lock");
        assert_eq!(observed.user_messages, [CHAT_PROMPT]);
        assert_eq!(
            observed.agent_messages.first().map(String::as_str),
            Some(ASSISTANT)
        );
        assert_eq!(
            observed.agent_messages.last().map(String::as_str),
            Some(ASSISTANT)
        );
        assert!(observed
            .agent_messages
            .iter()
            .any(|message| message.contains(&format!("Session: {conversation_id}"))));
        assert_eq!(
            observed.usage_updates,
            [(INPUT_TOKENS + OUTPUT_TOKENS, CONTEXT_WINDOW)]
        );
        assert!(observed.command_names.iter().any(|name| name == "status"));
        assert_protocol_stdout(&observed.stdout_lines);
    }

    // The TUI uses its actual asynchronous controller and session selector,
    // then projects the resulting state through a ratatui TestBackend.
    let tui_pool = SqlitePool::open(&database_path).expect("open TUI database");
    let mut tui_options =
        polkagent_cli::tui::interaction::tui_runtime_options(&tui_pool, Some(&config_path))
            .expect("build TUI runtime options");
    tui_options.workdir = workdir.to_path_buf();
    tui_options.disable_harness = true;
    tui_options.discover_environment_providers = false;
    drop(tui_pool);
    let tui_runtime = RuntimeFactory::build(tui_options)
        .await
        .expect("reconstruct TUI runtime");
    let mut controller = RunController::new(tui_runtime.clone());
    let mut interaction = InteractionState::default();
    interaction.select_agent(agent_id.clone(), AGENT_NAME);

    let list_request = interaction
        .begin_session_picker()
        .expect("open real TUI session selector");
    controller
        .list_sessions(list_request)
        .expect("start TUI session listing");
    let list_events = wait_for_controller(&mut controller).await;
    for event in list_events {
        interaction.apply(event);
    }
    let picker = interaction
        .session_picker
        .as_ref()
        .expect("TUI session selector projection");
    assert_eq!(picker.status, SessionPickerStatus::Ready);
    assert_eq!(picker.sessions.len(), 1);
    assert_eq!(picker.sessions[0].conversation_id, conversation_id);
    assert_eq!(picker.sessions[0].turn_count, 2);

    let load_request = interaction
        .begin_session_selection()
        .expect("select cross-surface TUI session");
    controller
        .load_session(load_request)
        .expect("start TUI session load");
    let load_events = wait_for_controller(&mut controller).await;
    for event in load_events {
        interaction.apply(event);
    }
    assert_eq!(
        interaction.conversation_id.as_deref(),
        Some(conversation_id.as_str())
    );
    assert_eq!(interaction.selected_model.as_deref(), Some(MODEL));
    let loaded_turns = interaction
        .transcript
        .iter()
        .chain(interaction.run.iter())
        .collect::<Vec<_>>();
    assert_eq!(loaded_turns.len(), 2);
    assert_eq!(loaded_turns[0].prompt, CHAT_PROMPT);
    assert_eq!(loaded_turns[1].prompt, ACP_PROMPT);
    assert!(loaded_turns.iter().all(|turn| turn.output == ASSISTANT));
    assert!(
        loaded_turns
            .iter()
            .all(|turn| turn.input_tokens == INPUT_TOKENS && turn.output_tokens == OUTPUT_TOKENS),
        "restored TUI turns: {loaded_turns:?}"
    );
    assert_eq!(
        loaded_turns[0].turn_id.as_deref(),
        Some(chat_turn_id.as_str())
    );
    assert_eq!(
        loaded_turns[0].run_id.as_deref(),
        Some(chat_run_ids[0].as_str())
    );
    assert_eq!(
        loaded_turns[1].turn_id.as_deref(),
        Some(acp_identity.turn_id.as_str())
    );
    assert_eq!(
        loaded_turns[1].run_id.as_deref(),
        Some(acp_identity.run_ids[0].as_str())
    );

    interaction.prompt_buffer = TUI_PROMPT.to_owned();
    let prompt = interaction.submit().expect("submit TUI follow-up");
    assert_eq!(
        prompt.conversation_id.as_deref(),
        Some(conversation_id.as_str())
    );
    controller.start(prompt).expect("start TUI follow-up");
    let prompt_events = wait_for_controller(&mut controller).await;
    let (tui_turn_id, tui_run_id) = prompt_events
        .iter()
        .find_map(|event| match event {
            ControllerEvent::Started {
                conversation_id: started_conversation,
                model,
                turn_id,
                run_id,
                ..
            } => {
                assert_eq!(started_conversation, &conversation_id);
                assert_eq!(model.as_deref(), Some(MODEL));
                Some((turn_id.clone(), run_id.clone()))
            }
            _ => None,
        })
        .expect("TUI public started correlation");
    assert!(prompt_events.iter().any(|event| matches!(
        event,
        ControllerEvent::Completed {
            text,
            input_tokens: INPUT_TOKENS,
            output_tokens: OUTPUT_TOKENS
        } if text == ASSISTANT
    )));
    for event in prompt_events {
        interaction.apply(event);
    }
    assert_eq!(interaction.transcript.len(), 2);
    let current = interaction
        .run
        .as_ref()
        .expect("TUI current turn projection");
    assert_eq!(current.prompt, TUI_PROMPT);
    assert_eq!(current.output, ASSISTANT);
    assert_eq!(current.turn_id.as_deref(), Some(tui_turn_id.as_str()));
    assert_eq!(current.run_id.as_deref(), Some(tui_run_id.as_str()));

    let tui_state = TuiState {
        interaction: interaction.clone(),
        ..TuiState::default()
    };
    let backend = TestBackend::new(140, 36);
    let mut terminal = Terminal::new(backend).expect("construct TUI TestBackend");
    terminal
        .draw(|frame| {
            console::render(
                frame,
                frame.area(),
                &tui_state,
                InputMode::Normal,
                &Theme::dark(),
            );
        })
        .expect("render cross-surface TUI projection");
    let rendered = buffer_text(&terminal);
    assert!(rendered.contains(AGENT_NAME), "{rendered}");
    assert!(rendered.contains(MODEL), "{rendered}");
    assert!(rendered.contains(TUI_PROMPT), "{rendered}");
    assert!(rendered.contains(ASSISTANT), "{rendered}");
    assert!(
        rendered.contains("10 input / 5 output tokens"),
        "{rendered}"
    );

    let typed_conversation_id = conversation_id.parse().expect("typed conversation ID");
    let service_transcript = tui_runtime
        .interactions()
        .load_transcript(TranscriptRequest {
            conversation_id: typed_conversation_id,
            limit: 100,
            offset: 0,
        })
        .await
        .expect("load final service transcript");
    assert_eq!(service_transcript.len(), 3);
    for (ordinal, (turn, expected_prompt)) in service_transcript
        .iter()
        .zip([CHAT_PROMPT, ACP_PROMPT, TUI_PROMPT])
        .enumerate()
    {
        assert_eq!(
            turn.turn.ordinal,
            u32::try_from(ordinal + 1).expect("ordinal")
        );
        assert_eq!(turn.turn.state, TurnState::Completed);
        assert_eq!(turn.user_text, expected_prompt);
        assert_eq!(turn.assistant_text.as_deref(), Some(ASSISTANT));
    }
    drop(controller);
    drop(tui_runtime);

    // Reconstruct HTTP one final time and compare its public projection to the
    // identities already observed from chat, ACP, and TUI.
    let final_api_runtime = runtime_at(workdir, &database_path, &config_path).await;
    let final_server = http_server(&final_api_runtime);
    let interaction_response = final_server
        .get(&format!("/api/v1alpha1/interactions/{conversation_id}"))
        .await;
    interaction_response.assert_status_success();
    let interaction_response = interaction_response.json::<serde_json::Value>();
    assert_eq!(
        interaction_response["interaction"]["conversation_id"],
        conversation_id
    );
    assert_eq!(
        interaction_response["interaction"]["config"]["model"],
        MODEL
    );
    assert_eq!(interaction_response["interaction"]["turn_count"], 3);

    let turn_response = final_server
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/turns"
        ))
        .await;
    turn_response.assert_status_success();
    let http_turns = turn_response.json::<serde_json::Value>()["data"]
        .as_array()
        .expect("HTTP turn projection")
        .clone();
    assert_eq!(http_turns.len(), 3);
    let expected_turn_ids = [&chat_turn_id, &acp_identity.turn_id, &tui_turn_id];
    let expected_run_ids = [&chat_run_ids[0], &acp_identity.run_ids[0], &tui_run_id];
    for (index, turn) in http_turns.iter().enumerate() {
        assert_eq!(turn["handle"]["conversation_id"], conversation_id);
        assert_eq!(turn["handle"]["turn_id"], expected_turn_ids[index].as_str());
        assert_eq!(
            turn["handle"]["run_ids"][0],
            expected_run_ids[index].as_str()
        );
        assert_eq!(turn["ordinal"], index + 1);
        assert_eq!(turn["state"], "completed");
    }

    let transcript_response = final_server
        .get(&format!("/api/v1alpha1/conversations/{conversation_id}"))
        .await;
    transcript_response.assert_status_success();
    let transcript_response = transcript_response.json::<serde_json::Value>();
    let messages = transcript_response["messages"]
        .as_array()
        .expect("HTTP conversation messages");
    assert_eq!(messages.len(), 6);
    for (index, expected_prompt) in [CHAT_PROMPT, ACP_PROMPT, TUI_PROMPT].iter().enumerate() {
        assert_eq!(messages[index * 2]["role"], "user");
        assert_eq!(messages[index * 2]["content"], *expected_prompt);
        assert_eq!(messages[index * 2 + 1]["role"], "assistant");
        assert_eq!(messages[index * 2 + 1]["content"], ASSISTANT);
    }

    let replay_response = final_server
        .get(&format!(
            "/api/v1alpha1/interactions/{conversation_id}/events?after_sequence=0&limit=100"
        ))
        .await;
    replay_response.assert_status_success();
    let replay = replay_response.json::<serde_json::Value>();
    let terminal_events = replay["data"]
        .as_array()
        .expect("HTTP event replay")
        .iter()
        .filter(|event| event["event"]["type"] == "turn_completed")
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 3);
    for (index, event) in terminal_events.iter().enumerate() {
        assert_eq!(event["conversation_id"], conversation_id);
        assert_eq!(event["turn_id"], expected_turn_ids[index].as_str());
        assert_eq!(event["event"]["result"]["text"], ASSISTANT);
        assert_eq!(
            event["event"]["result"]["run_ids"][0],
            expected_run_ids[index].as_str()
        );
        assert_eq!(
            event["event"]["result"]["usage"]["input_tokens"],
            INPUT_TOKENS
        );
        assert_eq!(
            event["event"]["result"]["usage"]["output_tokens"],
            OUTPUT_TOKENS
        );
    }
    drop(final_server);
    drop(final_api_runtime);

    // The database is the final causal audit, not the source of adapter
    // expectations: every public identity above must map one-to-one to an
    // ordinal turn, one linked run, and one terminal event.
    let connection =
        rusqlite::Connection::open(&database_path).expect("open final cross-surface database");
    let (session_count, config_json): (i64, String) = connection
        .query_row(
            "SELECT COUNT(*), config_json FROM interaction_sessions WHERE conversation_id = ?1",
            [&conversation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("load final interaction session");
    assert_eq!(session_count, 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&config_json).expect("decode session config")
            ["model"],
        MODEL
    );
    let total_interactions: i64 = connection
        .query_row("SELECT COUNT(*) FROM interaction_sessions", [], |row| {
            row.get(0)
        })
        .expect("count interaction sessions");
    let total_turns: i64 = connection
        .query_row("SELECT COUNT(*) FROM interaction_turns", [], |row| {
            row.get(0)
        })
        .expect("count interaction turns");
    let total_links: i64 = connection
        .query_row("SELECT COUNT(*) FROM interaction_turn_runs", [], |row| {
            row.get(0)
        })
        .expect("count interaction run links");
    let total_runs: i64 = connection
        .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))
        .expect("count durable runs");
    let terminal_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM interaction_events WHERE is_terminal = 1",
            [],
            |row| row.get(0),
        )
        .expect("count terminal interaction events");
    assert_eq!(
        (total_interactions, total_turns, total_links, total_runs, terminal_count),
        (1, 3, 3, 3, 3),
        "HTTP config, chat /model, ACP /status, and the refused ACP model must create no turns or runs"
    );

    let mut statement = connection
        .prepare(
            "SELECT t.id, links.run_id, t.ordinal, t.state, t.config_json, events.payload_json
             FROM interaction_turns t
             JOIN interaction_turn_runs links ON links.turn_id = t.id
             JOIN interaction_events events ON events.turn_id = t.id AND events.is_terminal = 1
             WHERE t.conversation_id = ?1
             ORDER BY t.ordinal ASC, links.ordinal ASC",
        )
        .expect("prepare final causal query");
    let rows = statement
        .query_map([&conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .expect("query final causal rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect final causal rows");
    assert_eq!(rows.len(), 3);
    for (index, (turn_id, run_id, ordinal, state, turn_config, terminal)) in rows.iter().enumerate()
    {
        assert_eq!(turn_id, expected_turn_ids[index].as_str());
        assert_eq!(run_id, expected_run_ids[index].as_str());
        assert_eq!(*ordinal, i64::try_from(index + 1).expect("causal ordinal"));
        assert_eq!(state, "completed");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(turn_config)
                .expect("decode effective turn config")["model"],
            MODEL
        );
        let terminal: InteractionEvent =
            serde_json::from_str(terminal).expect("decode terminal interaction event");
        let InteractionEvent::TurnCompleted { result } = terminal else {
            panic!("causal terminal event was not completed: {terminal:?}")
        };
        assert_eq!(result.text, ASSISTANT);
        assert_eq!(result.run_ids[0].to_string(), *run_id);
        assert_eq!(result.usage.input_tokens, INPUT_TOKENS);
        assert_eq!(result.usage.output_tokens, OUTPUT_TOKENS);
    }
}

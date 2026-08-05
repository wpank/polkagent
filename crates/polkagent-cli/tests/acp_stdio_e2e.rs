//! Executable ACP proof through the real `polkagent acp` subprocess.

use std::process::Command;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, SessionNotification,
    SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent};

#[derive(Default)]
struct ObservedUpdates {
    command_names: Vec<String>,
    messages: Vec<String>,
}

#[tokio::test]
async fn official_client_drives_editor_commands_and_a_real_run() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let db_path = temp.path().join("polkagent.db");
    let binary = env!("CARGO_BIN_EXE_polkagent");

    assert_cli_success(
        binary,
        &db_path,
        &["agent", "create", "editor-fixture", "--model", "fake/test"],
    );
    assert_cli_success(binary, &db_path, &["agent", "start", "editor-fixture"]);

    let observed = Arc::new(Mutex::new(ObservedUpdates::default()));
    let observed_by_client = Arc::clone(&observed);
    let agent = AcpAgent::new(
        AcpAgentConfig::new(binary)
            .args(["acp", "--agent", "editor-fixture", "--timeout", "20"])
            .env(
                "POLKAGENT_DATABASE_SQLITE_PATH",
                db_path.to_string_lossy().into_owned(),
            ),
    );

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                let mut observed = observed_by_client.lock().expect("observed updates lock");
                match notification.update {
                    SessionUpdate::AvailableCommandsUpdate(update) => {
                        observed.command_names.extend(
                            update
                                .available_commands
                                .into_iter()
                                .map(|command| command.name),
                        );
                    }
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            observed.messages.push(text.text);
                        }
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
                assert_eq!(initialized.protocol_version, ProtocolVersion::V1);
                assert_eq!(
                    initialized
                        .agent_info
                        .as_ref()
                        .map(|info| info.name.as_str()),
                    Some("polkagent")
                );

                let session = connection
                    .send_request(NewSessionRequest::new(temp.path()))
                    .block_task()
                    .await?;

                let help = connection
                    .send_request(PromptRequest::new(
                        session.session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new("/help"))],
                    ))
                    .block_task()
                    .await?;
                assert_eq!(help.stop_reason, StopReason::EndTurn);

                let run = connection
                    .send_request(PromptRequest::new(
                        session.session_id,
                        vec![ContentBlock::Text(TextContent::new(
                            "Return a short editor integration greeting.",
                        ))],
                    ))
                    .block_task()
                    .await?;
                assert_eq!(run.stop_reason, StopReason::EndTurn);
                Ok(())
            },
        )
        .await
        .expect("official ACP client completed the subprocess session");

    let observed = observed.lock().expect("observed updates lock");
    assert_eq!(
        observed.command_names,
        vec!["help", "status", "agents", "agent"]
    );
    assert!(
        observed
            .messages
            .iter()
            .any(|message| message.contains("Polkagent ACP commands")),
        "slash-command response was not streamed: {:?}",
        observed.messages
    );
    assert!(
        observed.messages.len() >= 2,
        "the real Polkagent run did not stream an agent message: {:?}",
        observed.messages
    );
}

fn assert_cli_success(binary: &str, db_path: &std::path::Path, args: &[&str]) {
    let output = Command::new(binary)
        .args(args)
        .env("POLKAGENT_DATABASE_SQLITE_PATH", db_path)
        .output()
        .expect("run Polkagent fixture command");
    assert!(
        output.status.success(),
        "polkagent {} failed\nstdout: {}\nstderr: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

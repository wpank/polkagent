//! Contract checks for the checked-in Zed custom-agent configuration.

#![allow(
    clippy::expect_used,
    reason = "configuration contract fixtures fail with field-specific diagnostics"
)]

use std::path::Path;
use std::process::Command;

const EXAMPLE: &str = include_str!("../../../docs/examples/polkagent-zed-agent-server.json");
const GUIDE: &str = include_str!("../../../docs/acp-zed.md");

fn json_code_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = None::<String>;
    for line in markdown.lines() {
        if line == "```json" {
            assert!(current.is_none(), "nested JSON code block");
            current = Some(String::new());
        } else if line == "```" {
            if let Some(block) = current.take() {
                blocks.push(block);
            }
        } else if let Some(block) = current.as_mut() {
            block.push_str(line);
            block.push('\n');
        }
    }
    assert!(current.is_none(), "unterminated JSON code block");
    blocks
}

fn zed_server(value: &serde_json::Value) -> &serde_json::Value {
    value
        .get("agent_servers")
        .and_then(|servers| servers.get("polkagent"))
        .expect("agent_servers.polkagent object")
}

#[test]
fn checked_in_zed_example_is_valid_safe_and_accepted_by_the_cli_parser() {
    let settings: serde_json::Value = serde_json::from_str(EXAMPLE).expect("valid example JSON");
    let server = zed_server(&settings);
    assert_eq!(
        server.get("type").and_then(|value| value.as_str()),
        Some("custom")
    );

    let command = server
        .get("command")
        .and_then(|value| value.as_str())
        .expect("custom agent command");
    assert!(Path::new(command).is_absolute(), "command must be absolute");
    let env = server
        .get("env")
        .and_then(|value| value.as_object())
        .expect("custom agent env object");
    assert!(
        env.is_empty(),
        "checked-in settings must contain no environment secrets"
    );

    let args = server
        .get("args")
        .and_then(|value| value.as_array())
        .expect("custom agent argument array")
        .iter()
        .map(|value| value.as_str().expect("string argument"))
        .collect::<Vec<_>>();
    assert_eq!(args.iter().filter(|arg| **arg == "acp").count(), 1);
    let acp_index = args
        .iter()
        .position(|arg| *arg == "acp")
        .expect("acp subcommand");
    for global in ["--config", "--log-file"] {
        let index = args
            .iter()
            .position(|arg| *arg == global)
            .expect("global option");
        assert!(
            index < acp_index,
            "{global} must precede the acp subcommand"
        );
        assert!(
            Path::new(args[index + 1]).is_absolute(),
            "{global} path must be absolute"
        );
    }
    assert!(!args.contains(&"--workdir"));
    for required in [
        "--agent",
        "--approval-tenant",
        "--approval-workspace",
        "--approval-principal",
    ] {
        assert!(args.contains(&required), "missing {required}");
    }

    let output = Command::new(env!("CARGO_BIN_EXE_polkagent"))
        .args(&args)
        .arg("--help")
        .output()
        .expect("run example arguments through the real CLI parser");
    assert!(
        output.status.success(),
        "example arguments were rejected: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8_lossy(&output.stdout);
    for option in [
        "--agent",
        "--model",
        "--provider",
        "--timeout",
        "--approval-tenant",
        "--approval-workspace",
        "--approval-principal",
    ] {
        assert!(help.contains(option), "ACP help omitted {option}");
    }
}

#[test]
fn every_zed_guide_json_block_is_valid_and_uses_the_custom_agent_shape() {
    let blocks = json_code_blocks(GUIDE);
    assert!(!blocks.is_empty(), "guide must contain JSON examples");
    for block in blocks {
        let value: serde_json::Value =
            serde_json::from_str(&block).expect("valid guide JSON block");
        let server = zed_server(&value);
        assert_eq!(
            server.get("type").and_then(|value| value.as_str()),
            Some("custom")
        );
        assert!(Path::new(
            server
                .get("command")
                .and_then(|value| value.as_str())
                .expect("absolute command")
        )
        .is_absolute());
        assert!(server
            .get("args")
            .and_then(|value| value.as_array())
            .is_some_and(|args| args.iter().any(|arg| arg.as_str() == Some("acp"))));
        assert!(server
            .get("env")
            .and_then(|value| value.as_object())
            .is_some_and(serde_json::Map::is_empty));

        let args = server
            .get("args")
            .and_then(|value| value.as_array())
            .expect("custom agent arguments")
            .iter()
            .map(|value| value.as_str().expect("string argument"));
        let output = Command::new(env!("CARGO_BIN_EXE_polkagent"))
            .args(args)
            .arg("--help")
            .output()
            .expect("run guide arguments through the real CLI parser");
        assert!(
            output.status.success(),
            "guide arguments were rejected: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

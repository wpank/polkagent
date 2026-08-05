use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn binary(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_polkagent"));
    command
        .arg("--log-file")
        .arg("-")
        .arg("--format")
        .arg("json")
        .env("HOME", home)
        .env_remove("POLKAGENT_CONFIG")
        .env_remove("POLKAGENT_PACKAGE_STORE");
    command
}

fn run(home: &Path, arguments: &[&str]) -> Output {
    binary(home)
        .args(arguments)
        .output()
        .expect("run polkagent")
}

fn success_json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "command failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid JSON stdout")
}

fn write_plugin(directory: &Path, version: &str, payload: &str, digest: Option<&str>) {
    fs::create_dir_all(directory).expect("create source");
    fs::write(directory.join("payload.txt"), payload).expect("write payload");
    let digest = digest
        .map(|value| format!("content_digest = \"{value}\""))
        .unwrap_or_default();
    fs::write(
        directory.join("plugin.toml"),
        format!(
            r#"[plugin]
name = "cli-plugin"
version = "{version}"
description = "CLI lifecycle test"

[provenance]
{digest}
"#
        ),
    )
    .expect("write manifest");
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn package_args<'a>(store: &'a str, tail: &'a [&'a str]) -> Vec<&'a str> {
    let mut arguments = vec!["package", "--store", store, "--trust-policy", "development"];
    arguments.extend_from_slice(tail);
    arguments
}

#[test]
fn all_package_commands_are_durable_and_structured() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let store = temp.path().join("store");
    let version_one = temp.path().join("v1");
    let version_two = temp.path().join("v2");
    fs::create_dir_all(&home).expect("home");
    write_plugin(&version_one, "1.0.0", "one", None);
    write_plugin(&version_two, "1.1.0", "two", None);
    let store_text = path_text(&store);
    let one_text = path_text(&version_one);
    let two_text = path_text(&version_two);

    let strict = run(
        &home,
        &["package", "--store", &store_text, "install", &one_text],
    );
    assert_eq!(
        strict.status.code(),
        Some(polkagent_cli::exit_codes::POLICY_DENIED)
    );
    let error: Value = serde_json::from_slice(&strict.stderr).expect("JSON error");
    assert_eq!(error["ok"], false);
    assert!(error["error"]
        .as_str()
        .expect("error string")
        .contains("--trust-policy development"));
    assert!(
        !store.exists(),
        "trust rejection must not initialize a store"
    );

    let installed = success_json(run(
        &home,
        &package_args(&store_text, &["install", &one_text]),
    ));
    assert_eq!(installed["status"], "installed");
    assert_eq!(installed["package"]["active"]["version"], "1.0.0");
    assert_eq!(installed["trust_policy"], "development");
    assert_eq!(installed["_meta"]["command"], "package");

    // Each invocation is a fresh process, exercising persisted restart state.
    let listed = success_json(run(&home, &package_args(&store_text, &["list"])));
    assert_eq!(listed["count"], 1);
    assert_eq!(listed["packages"][0]["name"], "cli-plugin");

    let fetched = success_json(run(
        &home,
        &package_args(&store_text, &["get", "cli-plugin"]),
    ));
    assert_eq!(fetched["package"]["active"]["version"], "1.0.0");

    let updated = success_json(run(
        &home,
        &package_args(&store_text, &["update", &two_text]),
    ));
    assert_eq!(updated["status"], "updated");
    assert_eq!(updated["package"]["active"]["version"], "1.1.0");
    assert_eq!(updated["package"]["history"][0]["version"], "1.0.0");

    let rolled_back = success_json(run(
        &home,
        &package_args(&store_text, &["rollback", "cli-plugin", "--to", "1.0.0"]),
    ));
    assert_eq!(rolled_back["status"], "rolled_back");
    assert_eq!(rolled_back["package"]["active"]["version"], "1.0.0");

    let confirmation_required = run(
        &home,
        &package_args(&store_text, &["uninstall", "cli-plugin"]),
    );
    assert!(!confirmation_required.status.success());
    let confirmation_error: Value =
        serde_json::from_slice(&confirmation_required.stderr).expect("JSON confirmation error");
    assert!(confirmation_error["error"]
        .as_str()
        .expect("error string")
        .contains("pass --yes"));

    let removed = success_json(run(
        &home,
        &package_args(&store_text, &["uninstall", "cli-plugin", "--yes"]),
    ));
    assert_eq!(removed["status"], "uninstalled");

    let empty = success_json(run(&home, &package_args(&store_text, &["list"])));
    assert_eq!(empty["count"], 0);
}

#[test]
fn failed_update_keeps_selected_version() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let store = temp.path().join("store");
    let version_one = temp.path().join("v1");
    let bad_version = temp.path().join("bad");
    fs::create_dir_all(&home).expect("home");
    write_plugin(&version_one, "1.0.0", "one", None);
    write_plugin(&bad_version, "1.1.0", "tampered", Some(&"0".repeat(64)));
    let store_text = path_text(&store);
    let one_text = path_text(&version_one);
    let bad_text = path_text(&bad_version);

    success_json(run(
        &home,
        &package_args(&store_text, &["install", &one_text]),
    ));
    let failed = run(&home, &package_args(&store_text, &["update", &bad_text]));
    assert!(!failed.status.success());
    let error: Value = serde_json::from_slice(&failed.stderr).expect("JSON error");
    assert!(error["error"]
        .as_str()
        .expect("error string")
        .contains("content digest mismatch"));

    let fetched = success_json(run(
        &home,
        &package_args(&store_text, &["get", "cli-plugin"]),
    ));
    assert_eq!(fetched["package"]["active"]["version"], "1.0.0");
}

#[test]
fn dry_run_install_does_not_create_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let store = temp.path().join("never-created");
    let source = temp.path().join("source");
    fs::create_dir_all(&home).expect("home");
    write_plugin(&source, "1.0.0", "one", None);
    let store_text = path_text(&store);
    let source_text = path_text(&source);

    let preview = success_json(run(
        &home,
        &[
            "--dry-run",
            "package",
            "--store",
            &store_text,
            "--trust-policy",
            "development",
            "install",
            &source_text,
        ],
    ));
    assert_eq!(preview["status"], "would_install");
    assert!(!store.exists());
}

#[test]
fn environment_selects_store_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let store = temp.path().join("environment-store");
    fs::create_dir_all(&home).expect("home");
    let output = binary(&home)
        .env("POLKAGENT_PACKAGE_STORE", &store)
        .args(["package", "list"])
        .output()
        .expect("run list");
    let value = success_json(output);
    assert_eq!(value["store"], path_text(&store));
    assert!(
        !store.exists(),
        "read-only list should not create the store"
    );
}

#[test]
fn project_config_selects_project_local_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    let config_directory = project.join(".polkagent");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&config_directory).expect("config directory");
    fs::write(config_directory.join("polkagent.toml"), "").expect("project config");
    let output = binary(&home)
        .current_dir(&project)
        .args(["package", "list"])
        .output()
        .expect("run list");
    let value = success_json(output);
    let expected = project
        .canonicalize()
        .expect("canonical project")
        .join(".polkagent")
        .join("packages");
    assert_eq!(value["store"], path_text(&expected));
    assert!(
        !expected.exists(),
        "read-only list should not create the store"
    );
}

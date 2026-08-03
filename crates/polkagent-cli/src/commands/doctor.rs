//! `polkagent doctor` — system health checks.
//!
//! Probes database, config, providers, harnesses, and infrastructure
//! components, then prints a summary report.

use anyhow::Result;

use crate::cli::DoctorCmd;

/// Known harness entries: (name, binary, api_key_env).
const KNOWN_HARNESSES: &[(&str, &str, &str)] = &[
    ("claude-code", "claude", "ANTHROPIC_API_KEY"),
    ("codex", "codex", "OPENAI_API_KEY"),
    ("cursor", "cursor", ""),
    ("goose", "goose", ""),
];

/// Default provider definitions used when no config file is present.
const DEFAULT_PROVIDERS: &[(&str, &str, &str, &str)] = &[
    // (id, type, api_key_env, default_model)
    ("anthropic", "anthropic", "ANTHROPIC_API_KEY", "claude-sonnet-4-6"),
    ("openai", "openai", "OPENAI_API_KEY", "gpt-5.4-mini"),
    ("gemini", "gemini", "GEMINI_API_KEY", "gemini-2.5-flash"),
];

// ---------------------------------------------------------------------------
// Check types
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Check {
    name: String,
    status: CheckStatus,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckStatus {
    Ok,
    Fail,
    Skip,
}

impl CheckStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Fail => "FAIL",
            Self::Skip => "SKIP",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Ok => "\u{25C9}",   // filled circle
            Self::Fail => "\u{25A0}", // filled square
            Self::Skip => "\u{25A0}", // filled square
        }
    }

    fn is_pass(self) -> bool {
        self == Self::Ok
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Execute the `doctor` subcommand.
pub fn run(cmd: &DoctorCmd) -> Result<()> {
    // Load config (best-effort).
    let config = load_config();

    // System checks.
    let mut system_checks: Vec<Check> = Vec::new();
    system_checks.push(check_database());
    system_checks.push(check_config());
    system_checks.push(check_disk_space());
    system_checks.push(check_signer());
    system_checks.push(check_chain_rpc());
    system_checks.push(check_daemon());

    // Provider checks.
    let provider_checks = check_providers(&config);

    // Harness checks.
    let harness_checks = check_harnesses(&config);

    // Output.
    if cmd.json {
        print_json(&system_checks, &provider_checks, &harness_checks)?;
    } else {
        print_human(&system_checks, &provider_checks, &harness_checks);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// JSON output
// ---------------------------------------------------------------------------

fn print_json(
    system: &[Check],
    providers: &[Check],
    harnesses: &[Check],
) -> Result<()> {
    let to_json = |checks: &[Check]| -> Vec<serde_json::Value> {
        checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "status": c.status.label(),
                    "passed": c.status.is_pass(),
                    "message": c.message,
                })
            })
            .collect()
    };

    let all: Vec<&Check> = system.iter().chain(providers).chain(harnesses).collect();
    let passed = all.iter().filter(|c| c.status.is_pass()).count();
    let total = all.len();

    let out = serde_json::json!({
        "system_checks": to_json(system),
        "provider_checks": to_json(providers),
        "harness_checks": to_json(harnesses),
        "passed": passed,
        "total": total,
        "healthy": passed == total,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Human-readable output (§ 10.3)
// ---------------------------------------------------------------------------

fn print_human(system: &[Check], providers: &[Check], harnesses: &[Check]) {
    println!("Polkagent Doctor");
    println!("{}", "-".repeat(50));

    // System checks.
    println!("  System");
    for c in system {
        let glyph = c.status.glyph();
        let label = c.status.label();
        println!("    {glyph} [{label:<4}] {}: {}", c.name, c.message);
    }
    println!();

    // Providers.
    println!("  Providers");
    for c in providers {
        let glyph = c.status.glyph();
        let label = c.status.label();
        println!("    {glyph} [{label:<4}] {}: {}", c.name, c.message);
    }
    println!();

    // Harnesses.
    println!("  Harnesses");
    for c in harnesses {
        let glyph = c.status.glyph();
        let label = c.status.label();
        println!("    {glyph} [{label:<4}] {}: {}", c.name, c.message);
    }

    // Summary.
    println!();
    let provider_pass = providers.iter().filter(|c| c.status.is_pass()).count();
    let provider_total = providers.len();
    println!("  {provider_pass}/{provider_total} provider checks passed.");

    let harness_pass = harnesses.iter().filter(|c| c.status.is_pass()).count();
    let harness_total = harnesses.len();
    println!("  {harness_pass}/{harness_total} harness checks passed.");
}

// ---------------------------------------------------------------------------
// Provider checks (§ 10.1)
// ---------------------------------------------------------------------------

fn check_providers(config: &polkagent_config::schema::Config) -> Vec<Check> {
    let mut checks = Vec::new();

    if config.providers.is_empty() {
        // No config providers — probe defaults.
        for &(id, _ptype, key_env, default_model) in DEFAULT_PROVIDERS {
            checks.push(check_single_provider(id, key_env, default_model, ""));
        }
        // Also probe local/ollama.
        checks.push(check_local_provider());
    } else {
        for pc in &config.providers {
            let base_url = if pc.base_url.is_empty() {
                ""
            } else {
                &pc.base_url
            };
            checks.push(check_single_provider(
                &pc.id,
                &pc.api_key_env,
                &pc.default_model,
                base_url,
            ));
        }
    }

    checks
}

fn check_single_provider(
    id: &str,
    api_key_env: &str,
    default_model: &str,
    base_url: &str,
) -> Check {
    if api_key_env.is_empty() {
        return Check {
            name: id.to_owned(),
            status: CheckStatus::Skip,
            message: "no API key env configured".to_owned(),
        };
    }

    match std::env::var(api_key_env) {
        Ok(val) if !val.is_empty() => {
            let url_part = if base_url.is_empty() {
                String::new()
            } else {
                format!(" ({base_url})")
            };
            Check {
                name: id.to_owned(),
                status: CheckStatus::Ok,
                message: format!("{default_model}{url_part}"),
            }
        }
        _ => Check {
            name: id.to_owned(),
            status: CheckStatus::Fail,
            message: format!("{api_key_env} not set"),
        },
    }
}

fn check_local_provider() -> Check {
    let ollama_url = std::env::var("OLLAMA_URL").ok().filter(|s| !s.is_empty());
    let ollama_model = std::env::var("OLLAMA_MODEL").ok().filter(|s| !s.is_empty());

    if let Some(url) = ollama_url {
        let model = ollama_model.as_deref().unwrap_or("llama3.2");
        Check {
            name: "local-ollama".to_owned(),
            status: CheckStatus::Ok,
            message: format!("{model} ({url})"),
        }
    } else if ollama_model.is_some() {
        let model = ollama_model.as_deref().unwrap_or("llama3.2");
        Check {
            name: "local-ollama".to_owned(),
            status: CheckStatus::Ok,
            message: format!("{model} (localhost:11434)"),
        }
    } else {
        Check {
            name: "local-ollama".to_owned(),
            status: CheckStatus::Skip,
            message: "OLLAMA_URL / OLLAMA_MODEL not set".to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Harness checks (§ 10.2)
// ---------------------------------------------------------------------------

fn check_harnesses(config: &polkagent_config::schema::Config) -> Vec<Check> {
    let mut checks = Vec::new();

    for &(name, binary, api_key_env) in KNOWN_HARNESSES {
        // Check if harness is configured (either in known list or config).
        let configured = config.harness.harnesses.contains_key(name)
            || config.harness.default.as_deref() == Some(name)
            || config.harness.harnesses.is_empty(); // probe all when no config

        if !configured && !config.harness.harnesses.is_empty() {
            checks.push(Check {
                name: name.to_owned(),
                status: CheckStatus::Skip,
                message: "not configured".to_owned(),
            });
            continue;
        }

        // Look up binary — check config for custom path first.
        let binary_to_check = config
            .harness
            .harnesses
            .get(name)
            .and_then(|e| e.binary_path.as_deref())
            .unwrap_or(binary);

        match which_binary(binary_to_check) {
            Some(path) => {
                let version = get_binary_version(binary_to_check);
                let version_str = version.as_deref().unwrap_or("unknown version");

                // Check auth if applicable.
                let auth_ok = api_key_env.is_empty()
                    || std::env::var(api_key_env)
                        .ok()
                        .filter(|k| !k.is_empty())
                        .is_some();

                if auth_ok {
                    checks.push(Check {
                        name: name.to_owned(),
                        status: CheckStatus::Ok,
                        message: format!("{binary_to_check} {version_str} ({path})"),
                    });
                } else {
                    checks.push(Check {
                        name: name.to_owned(),
                        status: CheckStatus::Fail,
                        message: format!(
                            "binary found at {path} but {api_key_env} not set"
                        ),
                    });
                }
            }
            None => {
                checks.push(Check {
                    name: name.to_owned(),
                    status: CheckStatus::Fail,
                    message: "binary not found".to_owned(),
                });
            }
        }
    }

    checks
}

// ---------------------------------------------------------------------------
// System checks (existing)
// ---------------------------------------------------------------------------

fn check_database() -> Check {
    let home = std::env::var("HOME").unwrap_or_default();
    let default_path = format!("{home}/.local/share/polkagent/polkagent.db");

    let db_path = std::env::var("POLKAGENT_DATABASE_SQLITE_PATH").unwrap_or(default_path);

    let path = expand_tilde(&db_path);

    if std::path::Path::new(&path).exists() {
        match rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(conn) => {
                match conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
                    row.get::<_, i64>(0)
                }) {
                    Ok(table_count) => Check {
                        name: "Database".to_owned(),
                        status: CheckStatus::Ok,
                        message: format!("SQLite database at {path} ({table_count} tables)"),
                    },
                    Err(e) => Check {
                        name: "Database".to_owned(),
                        status: CheckStatus::Fail,
                        message: format!("SQLite database exists but query failed: {e}"),
                    },
                }
            }
            Err(e) => Check {
                name: "Database".to_owned(),
                status: CheckStatus::Fail,
                message: format!("Cannot open database at {path}: {e}"),
            },
        }
    } else {
        Check {
            name: "Database".to_owned(),
            status: CheckStatus::Fail,
            message: format!("Database not found at {path}. Run `polkagent init` first."),
        }
    }
}

fn check_config() -> Check {
    let candidates = vec![
        ".polkagent/polkagent.toml".to_owned(),
        {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{home}/.config/polkagent/polkagent.toml")
        },
    ];

    for path in &candidates {
        if std::path::Path::new(path).exists() {
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    match toml::from_str::<polkagent_config::schema::Config>(&content) {
                        Ok(_) => {
                            return Check {
                                name: "Config".to_owned(),
                                status: CheckStatus::Ok,
                                message: format!("Valid config at {path}"),
                            }
                        }
                        Err(e) => {
                            return Check {
                                name: "Config".to_owned(),
                                status: CheckStatus::Fail,
                                message: format!("Config at {path} is invalid: {e}"),
                            }
                        }
                    }
                }
                Err(e) => {
                    return Check {
                        name: "Config".to_owned(),
                        status: CheckStatus::Fail,
                        message: format!("Cannot read config at {path}: {e}"),
                    }
                }
            }
        }
    }

    Check {
        name: "Config".to_owned(),
        status: CheckStatus::Fail,
        message: "No config file found. Run `polkagent init` or create ~/.config/polkagent/polkagent.toml".to_owned(),
    }
}

fn check_disk_space() -> Check {
    let home = std::env::var("HOME").unwrap_or_default();
    let db_dir = format!("{home}/.local/share/polkagent");

    let dir_path = std::path::Path::new(&db_dir);

    if dir_path.exists() {
        let test_path = dir_path.join(".doctor_check");
        match std::fs::write(&test_path, b"ok") {
            Ok(()) => {
                let _ = std::fs::remove_file(&test_path);
                Check {
                    name: "Disk Space".to_owned(),
                    status: CheckStatus::Ok,
                    message: format!("Database directory {db_dir} is writable"),
                }
            }
            Err(e) => Check {
                name: "Disk Space".to_owned(),
                status: CheckStatus::Fail,
                message: format!("Database directory {db_dir} is not writable: {e}"),
            },
        }
    } else {
        match std::fs::create_dir_all(dir_path) {
            Ok(()) => Check {
                name: "Disk Space".to_owned(),
                status: CheckStatus::Ok,
                message: format!("Database directory {db_dir} can be created"),
            },
            Err(e) => Check {
                name: "Disk Space".to_owned(),
                status: CheckStatus::Fail,
                message: format!("Cannot create database directory {db_dir}: {e}"),
            },
        }
    }
}

fn check_signer() -> Check {
    if let Ok(url) = std::env::var("POLKAGENT_SIGNER_URL") {
        if !url.is_empty() {
            return Check {
                name: "Signer".to_owned(),
                status: CheckStatus::Ok,
                message: format!("POLKAGENT_SIGNER_URL is set: {url}"),
            };
        }
    }

    Check {
        name: "Signer".to_owned(),
        status: CheckStatus::Fail,
        message: "POLKAGENT_SIGNER_URL is not set. On-chain signing will not be available. \
                  Set this to your signer service URL (e.g. http://localhost:7474)."
            .to_owned(),
    }
}

fn check_chain_rpc() -> Check {
    let rpc_url = std::env::var("POLKAGENT_CHAIN_RPC_URL")
        .ok()
        .filter(|s| !s.is_empty());

    if let Some(url) = rpc_url {
        let reachable = probe_tcp(&url);
        if reachable {
            Check {
                name: "Chain RPC".to_owned(),
                status: CheckStatus::Ok,
                message: format!("RPC endpoint reachable: {url}"),
            }
        } else {
            Check {
                name: "Chain RPC".to_owned(),
                status: CheckStatus::Fail,
                message: format!("RPC endpoint configured but not reachable: {url}"),
            }
        }
    } else {
        Check {
            name: "Chain RPC".to_owned(),
            status: CheckStatus::Ok,
            message: "No chain RPC endpoint configured (POLKAGENT_CHAIN_RPC_URL not set). \
                      Chain interactions will be unavailable."
                .to_owned(),
        }
    }
}

fn check_daemon() -> Check {
    let bind_addr = std::env::var("POLKAGENT_API_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_owned());

    let reachable = probe_tcp_addr(&bind_addr);

    if reachable {
        Check {
            name: "Daemon".to_owned(),
            status: CheckStatus::Ok,
            message: format!("polkagent-serve is running and reachable at {bind_addr}"),
        }
    } else {
        Check {
            name: "Daemon".to_owned(),
            status: CheckStatus::Fail,
            message: format!(
                "polkagent-serve is not reachable at {bind_addr}. \
                 Start the daemon with `polkagent-serve` or check POLKAGENT_API_BIND."
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Try to load the Polkagent config from standard locations.
fn load_config() -> polkagent_config::schema::Config {
    if let Ok(content) = std::fs::read_to_string(".polkagent/polkagent.toml") {
        if let Ok(cfg) = toml::from_str(&content) {
            return cfg;
        }
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let user_cfg = format!("{home}/.config/polkagent/polkagent.toml");
    if let Ok(content) = std::fs::read_to_string(&user_cfg) {
        if let Ok(cfg) = toml::from_str(&content) {
            return cfg;
        }
    }

    polkagent_config::schema::Config::default()
}

fn probe_tcp(url: &str) -> bool {
    let hostport = url
        .trim_start_matches("ws://")
        .trim_start_matches("wss://")
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or(url);

    probe_tcp_addr(hostport)
}

fn probe_tcp_addr(addr: &str) -> bool {
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = if addr.contains(':') {
        addr.to_owned()
    } else {
        format!("{addr}:80")
    };

    TcpStream::connect_timeout(
        &addr
            .parse()
            .unwrap_or_else(|_| ([127, 0, 0, 1], 80).into()),
        Duration::from_secs(2),
    )
    .is_ok()
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_owned()
}

/// Check if a binary is available on PATH. Returns the full path if found.
fn which_binary(name: &str) -> Option<String> {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let path = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            if path.is_empty() {
                None
            } else {
                Some(path)
            }
        })
}

/// Try to get the version string from a binary by running `<binary> --version`.
fn get_binary_version(binary: &str) -> Option<String> {
    std::process::Command::new(binary)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            // Take the first line and trim it.
            out.lines().next().unwrap_or("").trim().to_owned()
        })
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_status_labels() {
        assert_eq!(CheckStatus::Ok.label(), "OK");
        assert_eq!(CheckStatus::Fail.label(), "FAIL");
        assert_eq!(CheckStatus::Skip.label(), "SKIP");
    }

    #[test]
    fn check_status_is_pass() {
        assert!(CheckStatus::Ok.is_pass());
        assert!(!CheckStatus::Fail.is_pass());
        assert!(!CheckStatus::Skip.is_pass());
    }

    #[test]
    fn check_status_glyphs_are_nonempty() {
        assert!(!CheckStatus::Ok.glyph().is_empty());
        assert!(!CheckStatus::Fail.glyph().is_empty());
        assert!(!CheckStatus::Skip.glyph().is_empty());
    }

    #[test]
    fn check_single_provider_missing_key() {
        let check = check_single_provider(
            "test-provider",
            "NONEXISTENT_KEY_XYZ_123",
            "test-model",
            "",
        );
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.message.contains("not set"));
    }

    #[test]
    fn check_single_provider_no_key_env_skips() {
        let check = check_single_provider("test-provider", "", "test-model", "");
        assert_eq!(check.status, CheckStatus::Skip);
    }

    #[test]
    fn known_harnesses_has_expected_entries() {
        let names: Vec<&str> = KNOWN_HARNESSES.iter().map(|&(n, _, _)| n).collect();
        assert!(names.contains(&"claude-code"));
        assert!(names.contains(&"codex"));
        assert!(names.contains(&"cursor"));
        assert!(names.contains(&"goose"));
    }

    #[test]
    fn which_binary_finds_sh() {
        let result = which_binary("sh");
        assert!(result.is_some(), "expected `sh` to be found on PATH");
    }

    #[test]
    fn which_binary_returns_none_for_nonexistent() {
        let result = which_binary("nonexistent-binary-abc123xyz");
        assert!(result.is_none());
    }

    #[test]
    fn expand_tilde_no_tilde() {
        assert_eq!(expand_tilde("/foo/bar"), "/foo/bar");
    }

    #[test]
    fn default_providers_has_entries() {
        assert!(!DEFAULT_PROVIDERS.is_empty());
        let ids: Vec<&str> = DEFAULT_PROVIDERS.iter().map(|&(id, _, _, _)| id).collect();
        assert!(ids.contains(&"anthropic"));
        assert!(ids.contains(&"openai"));
    }

    #[test]
    fn check_providers_with_default_config() {
        let config = polkagent_config::schema::Config::default();
        let checks = check_providers(&config);
        // Should produce checks for default providers + local-ollama.
        assert!(
            checks.len() >= 3,
            "expected at least 3 provider checks, got {}",
            checks.len()
        );
    }

    #[test]
    fn check_harnesses_with_default_config() {
        let config = polkagent_config::schema::Config::default();
        let checks = check_harnesses(&config);
        assert_eq!(
            checks.len(),
            KNOWN_HARNESSES.len(),
            "should probe all known harnesses"
        );
    }

    #[test]
    fn doctor_output_format_json() {
        // Ensure JSON serialization does not panic.
        let system = vec![Check {
            name: "Test".to_owned(),
            status: CheckStatus::Ok,
            message: "all good".to_owned(),
        }];
        let providers = vec![Check {
            name: "anthropic".to_owned(),
            status: CheckStatus::Fail,
            message: "ANTHROPIC_API_KEY not set".to_owned(),
        }];
        let harnesses = vec![Check {
            name: "claude-code".to_owned(),
            status: CheckStatus::Skip,
            message: "not configured".to_owned(),
        }];

        let result = print_json(&system, &providers, &harnesses);
        assert!(result.is_ok());
    }
}

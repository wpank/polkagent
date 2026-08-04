//! `polkagent doctor` — system health checks.
//!
//! Probes database, config, providers, harnesses, and infrastructure
//! components, then prints a summary report.

use anyhow::Result;

use crate::cli::DoctorCmd;

/// Known harness entries: (name, binary, api_key_env, min_version).
///
/// `min_version` is a `(major, minor, patch)` tuple. When `None`, any version
/// is accepted.
const KNOWN_HARNESSES: &[HarnessDef] = &[
    HarnessDef {
        name: "claude-code",
        binary: "claude",
        api_key_env: "ANTHROPIC_API_KEY",
        min_version: Some((1, 0, 0)),
    },
    HarnessDef {
        name: "codex",
        binary: "codex",
        api_key_env: "OPENAI_API_KEY",
        min_version: Some((0, 1, 0)),
    },
    HarnessDef {
        name: "cursor",
        binary: "cursor",
        api_key_env: "",
        min_version: None,
    },
    HarnessDef {
        name: "goose",
        binary: "goose",
        api_key_env: "",
        min_version: None,
    },
    HarnessDef {
        name: "gh-copilot",
        binary: "gh",
        api_key_env: "",
        min_version: None,
    },
    HarnessDef {
        name: "kiro",
        binary: "kiro-cli",
        api_key_env: "",
        min_version: None,
    },
];

struct HarnessDef {
    name: &'static str,
    binary: &'static str,
    api_key_env: &'static str,
    min_version: Option<(u32, u32, u32)>,
}

/// Default provider definitions used when no config file is present.
///
/// Tuple: (id, type, api_key_env, default_model, health_url).
/// `health_url` is a base URL whose TCP reachability we probe.
const DEFAULT_PROVIDERS: &[ProviderDef] = &[
    ProviderDef {
        id: "anthropic",
        api_key_env: "ANTHROPIC_API_KEY",
        default_model: "claude-sonnet-4-6",
        base_url: "https://api.anthropic.com",
    },
    ProviderDef {
        id: "openai",
        api_key_env: "OPENAI_API_KEY",
        default_model: "gpt-5.4-mini",
        base_url: "https://api.openai.com",
    },
    ProviderDef {
        id: "gemini",
        api_key_env: "GEMINI_API_KEY",
        default_model: "gemini-2.5-flash",
        base_url: "https://generativelanguage.googleapis.com",
    },
    ProviderDef {
        id: "openrouter",
        api_key_env: "OPENROUTER_API_KEY",
        default_model: "claude-sonnet-4-6",
        base_url: "https://openrouter.ai",
    },
    ProviderDef {
        id: "perplexity",
        api_key_env: "PERPLEXITY_API_KEY",
        default_model: "sonar",
        base_url: "https://api.perplexity.ai",
    },
    ProviderDef {
        id: "cerebras",
        api_key_env: "CEREBRAS_API_KEY",
        default_model: "llama-4-scout-17b-16e",
        base_url: "https://api.cerebras.ai",
    },
];

struct ProviderDef {
    id: &'static str,
    api_key_env: &'static str,
    default_model: &'static str,
    base_url: &'static str,
}

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
    Warn,
    Skip,
}

impl CheckStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Fail => "FAIL",
            Self::Warn => "WARN",
            Self::Skip => "SKIP",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Ok => "\u{25C9}",   // filled circle
            Self::Fail => "\u{25A0}", // filled square
            Self::Warn => "\u{25B3}", // triangle
            Self::Skip => "\u{25A0}", // filled square
        }
    }

    fn is_pass(self) -> bool {
        self == Self::Ok || self == Self::Warn
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
    system_checks.extend(check_metadata_drift(&config));

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

fn print_json(system: &[Check], providers: &[Check], harnesses: &[Check]) -> Result<()> {
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
        print_check(c);
    }
    println!();

    // Providers.
    println!("  Providers");
    for c in providers {
        print_check(c);
    }
    println!();

    // Harnesses.
    println!("  Harnesses");
    for c in harnesses {
        print_check(c);
    }

    // Summary.
    println!();
    let all: Vec<&Check> = system.iter().chain(providers).chain(harnesses).collect();
    let passed = all.iter().filter(|c| c.status.is_pass()).count();
    let total = all.len();

    let provider_pass = providers.iter().filter(|c| c.status.is_pass()).count();
    let provider_total = providers.len();
    let harness_pass = harnesses.iter().filter(|c| c.status.is_pass()).count();
    let harness_total = harnesses.len();

    println!("  {provider_pass}/{provider_total} provider checks passed.");
    println!("  {harness_pass}/{harness_total} harness checks passed.");
    println!("  {passed}/{total} total checks passed.");
}

fn print_check(c: &Check) {
    let glyph = c.status.glyph();
    let label = c.status.label();
    println!("    {glyph} [{label:<4}] {}: {}", c.name, c.message);
}

// ---------------------------------------------------------------------------
// Provider checks (§ 10.1)
// ---------------------------------------------------------------------------

fn check_providers(config: &polkagent_config::schema::Config) -> Vec<Check> {
    let mut checks = Vec::new();

    if config.providers.is_empty() {
        // No config providers — probe defaults.
        for def in DEFAULT_PROVIDERS {
            checks.extend(check_provider_full(
                def.id,
                def.api_key_env,
                def.default_model,
                def.base_url,
            ));
        }
        // Also probe local/ollama.
        checks.push(check_local_provider());
    } else {
        for pc in &config.providers {
            let base_url = if pc.base_url.is_empty() {
                // Look up in default defs.
                DEFAULT_PROVIDERS
                    .iter()
                    .find(|d| d.id == pc.id)
                    .map(|d| d.base_url)
                    .unwrap_or("")
            } else {
                &pc.base_url
            };
            checks.extend(check_provider_full(
                &pc.id,
                &pc.api_key_env,
                &pc.default_model,
                base_url,
            ));
        }
    }

    checks
}

/// Produce multiple checks for a single provider:
/// 1. API key env var is set and non-empty
/// 2. Endpoint reachability (TCP connect to base URL host)
fn check_provider_full(
    id: &str,
    api_key_env: &str,
    default_model: &str,
    base_url: &str,
) -> Vec<Check> {
    let mut checks = Vec::new();

    // (1) API key probe.
    let key_ok = if api_key_env.is_empty() {
        checks.push(Check {
            name: format!("{id}/api-key"),
            status: CheckStatus::Skip,
            message: "no API key env configured".to_owned(),
        });
        false
    } else {
        match std::env::var(api_key_env) {
            Ok(val) if !val.is_empty() => {
                checks.push(Check {
                    name: format!("{id}/api-key"),
                    status: CheckStatus::Ok,
                    message: format!("{api_key_env} is set (model: {default_model})"),
                });
                true
            }
            _ => {
                checks.push(Check {
                    name: format!("{id}/api-key"),
                    status: CheckStatus::Fail,
                    message: format!("{api_key_env} not set"),
                });
                false
            }
        }
    };

    // (2) Endpoint reachability probe.
    if base_url.is_empty() {
        checks.push(Check {
            name: format!("{id}/endpoint"),
            status: CheckStatus::Skip,
            message: "no base URL configured".to_owned(),
        });
    } else if !key_ok {
        // No point probing the endpoint if we don't have a key.
        checks.push(Check {
            name: format!("{id}/endpoint"),
            status: CheckStatus::Skip,
            message: format!("skipped (no API key) - {base_url}"),
        });
    } else {
        let reachable = probe_tcp(base_url);
        if reachable {
            checks.push(Check {
                name: format!("{id}/endpoint"),
                status: CheckStatus::Ok,
                message: format!("reachable: {base_url}"),
            });
        } else {
            checks.push(Check {
                name: format!("{id}/endpoint"),
                status: CheckStatus::Fail,
                message: format!("not reachable: {base_url}"),
            });
        }
    }

    checks
}

fn check_local_provider() -> Check {
    let ollama_url = std::env::var("OLLAMA_URL").ok().filter(|s| !s.is_empty());
    let ollama_model = std::env::var("OLLAMA_MODEL").ok().filter(|s| !s.is_empty());

    if let Some(url) = ollama_url {
        let model = ollama_model.as_deref().unwrap_or("llama3.2");
        let reachable = probe_tcp(&url);
        if reachable {
            Check {
                name: "local-ollama".to_owned(),
                status: CheckStatus::Ok,
                message: format!("{model} ({url}) reachable"),
            }
        } else {
            Check {
                name: "local-ollama".to_owned(),
                status: CheckStatus::Warn,
                message: format!("{model} ({url}) configured but not reachable"),
            }
        }
    } else if ollama_model.is_some() {
        let model = ollama_model.as_deref().unwrap_or("llama3.2");
        let default_url = "http://localhost:11434";
        let reachable = probe_tcp(default_url);
        if reachable {
            Check {
                name: "local-ollama".to_owned(),
                status: CheckStatus::Ok,
                message: format!("{model} ({default_url}) reachable"),
            }
        } else {
            Check {
                name: "local-ollama".to_owned(),
                status: CheckStatus::Warn,
                message: format!("{model} ({default_url}) configured but not reachable"),
            }
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

    for def in KNOWN_HARNESSES {
        // Check if harness is configured (either in known list or config).
        let configured = config.harness.harnesses.contains_key(def.name)
            || config.harness.default.as_deref() == Some(def.name)
            || config.harness.harnesses.is_empty(); // probe all when no config

        if !configured && !config.harness.harnesses.is_empty() {
            checks.push(Check {
                name: def.name.to_owned(),
                status: CheckStatus::Skip,
                message: "not configured".to_owned(),
            });
            continue;
        }

        // Look up binary — check config for custom path first.
        let binary_to_check = config
            .harness
            .harnesses
            .get(def.name)
            .and_then(|e| e.binary_path.as_deref())
            .unwrap_or(def.binary);

        // Special case for gh-copilot: check `gh copilot --version` not just `gh`.
        let is_gh_copilot = def.name == "gh-copilot";

        // (1) Binary existence.
        match which_binary(binary_to_check) {
            Some(path) => {
                checks.push(Check {
                    name: format!("{}/binary", def.name),
                    status: CheckStatus::Ok,
                    message: format!("{binary_to_check} found at {path}"),
                });

                // (2) Version check.
                let version = if is_gh_copilot {
                    get_gh_copilot_version()
                } else {
                    get_binary_version(binary_to_check)
                };

                match &version {
                    Some(ver_str) => {
                        if let Some(min) = def.min_version {
                            if let Some(parsed) = parse_semver(ver_str) {
                                if parsed >= min {
                                    checks.push(Check {
                                        name: format!("{}/version", def.name),
                                        status: CheckStatus::Ok,
                                        message: format!(
                                            "{ver_str} (>= {}.{}.{})",
                                            min.0, min.1, min.2,
                                        ),
                                    });
                                } else {
                                    checks.push(Check {
                                        name: format!("{}/version", def.name),
                                        status: CheckStatus::Warn,
                                        message: format!(
                                            "{ver_str} (< {}.{}.{} minimum)",
                                            min.0, min.1, min.2,
                                        ),
                                    });
                                }
                            } else {
                                checks.push(Check {
                                    name: format!("{}/version", def.name),
                                    status: CheckStatus::Ok,
                                    message: format!("{ver_str} (could not parse semver)"),
                                });
                            }
                        } else {
                            checks.push(Check {
                                name: format!("{}/version", def.name),
                                status: CheckStatus::Ok,
                                message: ver_str.clone(),
                            });
                        }
                    }
                    None => {
                        if is_gh_copilot {
                            // gh exists but copilot extension might not be installed.
                            checks.push(Check {
                                name: format!("{}/version", def.name),
                                status: CheckStatus::Fail,
                                message: "gh copilot extension not installed (run `gh extension install github/gh-copilot`)".to_owned(),
                            });
                            // Skip auth check for gh-copilot if extension missing.
                            continue;
                        } else {
                            checks.push(Check {
                                name: format!("{}/version", def.name),
                                status: CheckStatus::Warn,
                                message: "could not determine version".to_owned(),
                            });
                        }
                    }
                }

                // (3) Auth validation.
                if !def.api_key_env.is_empty() {
                    let auth_ok = std::env::var(def.api_key_env)
                        .ok()
                        .filter(|k| !k.is_empty())
                        .is_some();
                    if auth_ok {
                        checks.push(Check {
                            name: format!("{}/auth", def.name),
                            status: CheckStatus::Ok,
                            message: format!("{} is set", def.api_key_env),
                        });
                    } else {
                        checks.push(Check {
                            name: format!("{}/auth", def.name),
                            status: CheckStatus::Fail,
                            message: format!("{} not set", def.api_key_env),
                        });
                    }
                }

                // (4) Daemon health for HTTP-transport harnesses.
                if let Some(entry) = config.harness.harnesses.get(def.name) {
                    if entry.transport.as_deref() == Some("http") {
                        let port = entry.http_port.unwrap_or(9090);
                        let addr = format!("127.0.0.1:{port}");
                        let reachable = probe_tcp_addr(&addr);
                        checks.push(Check {
                            name: format!("{}/daemon", def.name),
                            status: if reachable {
                                CheckStatus::Ok
                            } else {
                                CheckStatus::Fail
                            },
                            message: if reachable {
                                format!("HTTP daemon reachable at {addr}")
                            } else {
                                format!("HTTP daemon not reachable at {addr}")
                            },
                        });
                    }
                }
            }
            None => {
                checks.push(Check {
                    name: format!("{}/binary", def.name),
                    status: CheckStatus::Fail,
                    message: format!("{binary_to_check} not found on PATH"),
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
    let candidates = vec![".polkagent/polkagent.toml".to_owned(), {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/.config/polkagent/polkagent.toml")
    }];

    for path in &candidates {
        if std::path::Path::new(path).exists() {
            match std::fs::read_to_string(path) {
                Ok(content) => match toml::from_str::<polkagent_config::schema::Config>(&content) {
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
                },
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
    let bind_addr =
        std::env::var("POLKAGENT_API_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());

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
// Metadata drift checks (PRD-05 §4.4)
// ---------------------------------------------------------------------------

fn check_metadata_drift(_config: &polkagent_config::schema::Config) -> Vec<Check> {
    use polkagent_metadata::{ChainId, MetadataService};
    use polkagent_service::metadata_watcher::check_drift_all;

    let svc = MetadataService::new();

    // Use well-known Polkadot ecosystem chains for drift detection.
    let chain_ids: Vec<ChainId> = vec![ChainId::new("polkadot"), ChainId::new("kusama")];

    let drifts = check_drift_all(&svc, &chain_ids);

    if drifts.is_empty() {
        vec![Check {
            name: "Metadata Drift".to_owned(),
            status: CheckStatus::Ok,
            message: format!(
                "no metadata drift detected across {} chain(s)",
                chain_ids.len()
            ),
        }]
    } else {
        drifts
            .iter()
            .map(|d| Check {
                name: format!("Metadata Drift/{}", d.chain_id),
                status: CheckStatus::Warn,
                message: format!(
                    "drift detected: pinned={} current={}",
                    d.pinned_hash, d.current_hash
                ),
            })
            .collect()
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

    // Determine port from scheme context.
    let addr = if addr.contains(':') {
        addr.to_owned()
    } else {
        format!("{addr}:443")
    };

    // Try DNS resolution first, then connect.
    use std::net::ToSocketAddrs;
    if let Ok(mut addrs) = addr.to_socket_addrs() {
        if let Some(socket_addr) = addrs.next() {
            return TcpStream::connect_timeout(&socket_addr, Duration::from_secs(3)).is_ok();
        }
    }
    false
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

/// Get `gh copilot --version` output.
fn get_gh_copilot_version() -> Option<String> {
    std::process::Command::new("gh")
        .args(["copilot", "--version"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            out.lines().next().unwrap_or("").trim().to_owned()
        })
        .filter(|s| !s.is_empty())
}

/// Parse a version string into `(major, minor, patch)`.
///
/// Handles formats like:
/// - `1.2.3`
/// - `v1.2.3`
/// - `claude-code 1.2.3`
/// - `codex v0.1.2-beta`
fn parse_semver(version_str: &str) -> Option<(u32, u32, u32)> {
    // Find the first sequence that looks like digits.digits.digits.
    let re_like = version_str
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .find(|s| {
            s.contains('.')
                && s.chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false)
        })?;

    let mut parts = re_like.splitn(3, '.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    // Patch may contain trailing non-digit chars (e.g. "3-beta").
    let patch_str = parts.next().unwrap_or("0");
    let patch: u32 = patch_str
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0);

    Some((major, minor, patch))
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
        assert_eq!(CheckStatus::Warn.label(), "WARN");
        assert_eq!(CheckStatus::Skip.label(), "SKIP");
    }

    #[test]
    fn check_status_is_pass() {
        assert!(CheckStatus::Ok.is_pass());
        assert!(CheckStatus::Warn.is_pass());
        assert!(!CheckStatus::Fail.is_pass());
        assert!(!CheckStatus::Skip.is_pass());
    }

    #[test]
    fn check_status_glyphs_are_nonempty() {
        assert!(!CheckStatus::Ok.glyph().is_empty());
        assert!(!CheckStatus::Fail.glyph().is_empty());
        assert!(!CheckStatus::Warn.glyph().is_empty());
        assert!(!CheckStatus::Skip.glyph().is_empty());
    }

    #[test]
    fn parse_semver_simple() {
        assert_eq!(parse_semver("1.2.3"), Some((1, 2, 3)));
    }

    #[test]
    fn parse_semver_with_v_prefix() {
        assert_eq!(parse_semver("v1.2.3"), Some((1, 2, 3)));
    }

    #[test]
    fn parse_semver_with_program_name() {
        assert_eq!(parse_semver("claude-code 1.0.12"), Some((1, 0, 12)));
    }

    #[test]
    fn parse_semver_with_prerelease() {
        assert_eq!(parse_semver("codex v0.1.2-beta"), Some((0, 1, 2)));
    }

    #[test]
    fn parse_semver_invalid() {
        assert_eq!(parse_semver("no version here"), None);
    }

    #[test]
    fn parse_semver_two_part() {
        // Two-part versions: patch defaults to 0.
        assert_eq!(parse_semver("1.2"), Some((1, 2, 0)));
    }

    #[test]
    fn known_harnesses_has_expected_entries() {
        let names: Vec<&str> = KNOWN_HARNESSES.iter().map(|d| d.name).collect();
        assert!(names.contains(&"claude-code"));
        assert!(names.contains(&"codex"));
        assert!(names.contains(&"cursor"));
        assert!(names.contains(&"goose"));
        assert!(names.contains(&"gh-copilot"));
        assert!(names.contains(&"kiro"));
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
        let ids: Vec<&str> = DEFAULT_PROVIDERS.iter().map(|d| d.id).collect();
        assert!(ids.contains(&"anthropic"));
        assert!(ids.contains(&"openai"));
        assert!(ids.contains(&"gemini"));
        assert!(ids.contains(&"openrouter"));
        assert!(ids.contains(&"perplexity"));
        assert!(ids.contains(&"cerebras"));
    }

    #[test]
    fn check_provider_full_missing_key() {
        let checks = check_provider_full(
            "test-provider",
            "NONEXISTENT_KEY_XYZ_123",
            "test-model",
            "https://example.com",
        );
        // Should have api-key (FAIL) and endpoint (SKIP, no key).
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].status, CheckStatus::Fail);
        assert!(checks[0].name.contains("api-key"));
        assert_eq!(checks[1].status, CheckStatus::Skip);
        assert!(checks[1].name.contains("endpoint"));
    }

    #[test]
    fn check_provider_full_no_key_env_skips() {
        let checks = check_provider_full("test-provider", "", "test-model", "");
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].status, CheckStatus::Skip);
        assert_eq!(checks[1].status, CheckStatus::Skip);
    }

    #[test]
    fn check_providers_with_default_config() {
        let config = polkagent_config::schema::Config::default();
        let checks = check_providers(&config);
        // Should produce checks for default providers (2 per provider) + local-ollama.
        assert!(
            checks.len() >= DEFAULT_PROVIDERS.len() * 2 + 1,
            "expected at least {} provider checks, got {}",
            DEFAULT_PROVIDERS.len() * 2 + 1,
            checks.len()
        );
    }

    #[test]
    fn check_harnesses_with_default_config() {
        let config = polkagent_config::schema::Config::default();
        let checks = check_harnesses(&config);
        // Each harness produces at least 1 check (binary existence).
        assert!(
            checks.len() >= KNOWN_HARNESSES.len(),
            "should probe all known harnesses, got {} checks",
            checks.len()
        );
    }

    #[test]
    fn version_comparison_works() {
        assert!((1, 0, 0) >= (1, 0, 0));
        assert!((1, 1, 0) >= (1, 0, 0));
        assert!((2, 0, 0) >= (1, 0, 0));
        assert!(!((0, 9, 0) >= (1, 0, 0)));
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
            name: "anthropic/api-key".to_owned(),
            status: CheckStatus::Fail,
            message: "ANTHROPIC_API_KEY not set".to_owned(),
        }];
        let harnesses = vec![Check {
            name: "claude-code/binary".to_owned(),
            status: CheckStatus::Skip,
            message: "not configured".to_owned(),
        }];

        let result = print_json(&system, &providers, &harnesses);
        assert!(result.is_ok());
    }
}

//! `polkagent doctor` — system health checks.

use anyhow::Result;

use crate::cli::DoctorCmd;

/// Execute the `doctor` subcommand.
pub fn run(cmd: &DoctorCmd) -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();

    // 1. SQLite database.
    checks.push(check_database());

    // 2. Config file.
    checks.push(check_config());

    // 3. API key environment variables.
    checks.push(check_api_keys());

    // 4. Disk space.
    checks.push(check_disk_space());

    // Output.
    if cmd.json {
        let items: Vec<serde_json::Value> = checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "passed": c.passed,
                    "message": c.message,
                })
            })
            .collect();
        let passed = checks.iter().filter(|c| c.passed).count();
        let total = checks.len();
        let out = serde_json::json!({
            "checks": items,
            "passed": passed,
            "total": total,
            "healthy": passed == total,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Polkagent Doctor");
        println!("{}", "-".repeat(50));
        println!();

        for c in &checks {
            let glyph = if c.passed {
                "\u{25C9}" // ROSEDUST filled circle
            } else {
                "\u{25A0}" // ROSEDUST filled square (error)
            };
            let status = if c.passed { "OK" } else { "FAIL" };
            println!("  {glyph} [{status:<4}] {}: {}", c.name, c.message);
        }

        println!();
        let passed = checks.iter().filter(|c| c.passed).count();
        let total = checks.len();
        if passed == total {
            println!("All {total} checks passed. System is healthy.");
        } else {
            println!(
                "{passed}/{total} checks passed. {} issue(s) found.",
                total - passed
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Check type
// ---------------------------------------------------------------------------

struct Check {
    name: &'static str,
    passed: bool,
    message: String,
}

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

fn check_database() -> Check {
    // Check the default database path.
    let home = std::env::var("HOME").unwrap_or_default();
    let default_path = format!("{home}/.local/share/polkagent/polkagent.db");

    let db_path = std::env::var("POLKAGENT_DATABASE_SQLITE_PATH")
        .unwrap_or(default_path);

    let path = expand_tilde(&db_path);

    if std::path::Path::new(&path).exists() {
        // Try to open and query it.
        match rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(conn) => {
                match conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
                    row.get::<_, i64>(0)
                }) {
                    Ok(table_count) => Check {
                        name: "Database",
                        passed: true,
                        message: format!("SQLite database at {path} ({table_count} tables)"),
                    },
                    Err(e) => Check {
                        name: "Database",
                        passed: false,
                        message: format!("SQLite database exists but query failed: {e}"),
                    },
                }
            }
            Err(e) => Check {
                name: "Database",
                passed: false,
                message: format!("Cannot open database at {path}: {e}"),
            },
        }
    } else {
        Check {
            name: "Database",
            passed: false,
            message: format!("Database not found at {path}. Run `polkagent init` first."),
        }
    }
}

fn check_config() -> Check {
    // Search for config in standard locations.
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
                                name: "Config",
                                passed: true,
                                message: format!("Valid config at {path}"),
                            }
                        }
                        Err(e) => {
                            return Check {
                                name: "Config",
                                passed: false,
                                message: format!("Config at {path} is invalid: {e}"),
                            }
                        }
                    }
                }
                Err(e) => {
                    return Check {
                        name: "Config",
                        passed: false,
                        message: format!("Cannot read config at {path}: {e}"),
                    }
                }
            }
        }
    }

    Check {
        name: "Config",
        passed: false,
        message: "No config file found. Run `polkagent init` or create ~/.config/polkagent/polkagent.toml".to_owned(),
    }
}

fn check_api_keys() -> Check {
    let keys = [
        "POLKAGENT_ANTHROPIC_API_KEY",
        "ANTHROPIC_API_KEY",
        "POLKAGENT_OPENAI_API_KEY",
        "OPENAI_API_KEY",
    ];

    let found: Vec<&str> = keys
        .iter()
        .filter(|k| std::env::var(k).is_ok())
        .copied()
        .collect();

    if found.is_empty() {
        Check {
            name: "API Keys",
            passed: false,
            message: "No API key env vars set. Set ANTHROPIC_API_KEY or OPENAI_API_KEY.".to_owned(),
        }
    } else {
        Check {
            name: "API Keys",
            passed: true,
            message: format!("Found: {}", found.join(", ")),
        }
    }
}

fn check_disk_space() -> Check {
    let home = std::env::var("HOME").unwrap_or_default();
    let db_dir = format!("{home}/.local/share/polkagent");

    // Use a simple heuristic: check if we can create a temp file in the directory.
    let dir_path = std::path::Path::new(&db_dir);

    if dir_path.exists() {
        // On macOS/Linux, use statvfs-like info. For now, just check the dir is writable.
        let test_path = dir_path.join(".doctor_check");
        match std::fs::write(&test_path, b"ok") {
            Ok(()) => {
                let _ = std::fs::remove_file(&test_path);
                Check {
                    name: "Disk Space",
                    passed: true,
                    message: format!("Database directory {db_dir} is writable"),
                }
            }
            Err(e) => Check {
                name: "Disk Space",
                passed: false,
                message: format!("Database directory {db_dir} is not writable: {e}"),
            },
        }
    } else {
        // Directory doesn't exist yet; check if we can create it.
        match std::fs::create_dir_all(dir_path) {
            Ok(()) => Check {
                name: "Disk Space",
                passed: true,
                message: format!("Database directory {db_dir} can be created"),
            },
            Err(e) => Check {
                name: "Disk Space",
                passed: false,
                message: format!("Cannot create database directory {db_dir}: {e}"),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_owned()
}

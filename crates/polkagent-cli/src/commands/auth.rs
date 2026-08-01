//! `polkagent auth` — authentication and credential management.
//!
//! Subcommands:
//! - `auth login`   — prompt for API keys and store them securely.
//! - `auth logout`  — remove stored credentials.
//! - `auth whoami`  — show the current identity and key configuration.
//! - `auth status`  — summarise which providers have valid keys.

use anyhow::{Context, Result};
use std::io::{self, Write};
use std::path::PathBuf;

use crate::cli::{AuthCmd, AuthLoginCmd, AuthLogoutCmd, AuthStatusCmd, AuthWhoamiCmd};

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch the `auth` subcommand.
pub fn run(cmd: &AuthCmd) -> Result<()> {
    match cmd {
        AuthCmd::Login(c)  => login(c),
        AuthCmd::Logout(c) => logout(c),
        AuthCmd::Whoami(c) => whoami(c),
        AuthCmd::Status(c) => status(c),
    }
}

// ---------------------------------------------------------------------------
// Key masking helper
// ---------------------------------------------------------------------------

/// Mask a secret key so only the first 6 and last 4 characters are visible.
///
/// Example: `"sk-ant-api03-AbCdEf"` → `"sk-ant-...AbCd"`
///
/// If the key is too short to safely mask (fewer than 11 characters) the
/// entire value is replaced with `"[REDACTED]"`.
pub fn mask_key(key: &str) -> String {
    const MIN_LEN: usize = 11; // 6 prefix + "..." + 4 suffix requires at least 10 chars
    if key.len() < MIN_LEN {
        return "[REDACTED]".to_owned();
    }
    let prefix = &key[..6];
    let suffix = &key[key.len() - 4..];
    format!("{prefix}...{suffix}")
}

// ---------------------------------------------------------------------------
// Credentials file path
// ---------------------------------------------------------------------------

/// Return the path to `~/.polkagent/credentials`.
fn credentials_path() -> Result<PathBuf> {
    let home = home_dir().context("cannot determine home directory")?;
    Ok(home.join(".polkagent").join("credentials"))
}

/// Expand `~/` paths using the `HOME` environment variable.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// Provider key configuration
// ---------------------------------------------------------------------------

/// All provider key environment variable pairs checked by Polkagent.
///
/// Each tuple is `(provider_name, primary_env_var, polkagent_prefixed_env_var)`.
const PROVIDER_KEYS: &[(&str, &str, &str)] = &[
    ("Anthropic", "ANTHROPIC_API_KEY", "POLKAGENT_ANTHROPIC_API_KEY"),
    ("OpenAI",    "OPENAI_API_KEY",    "POLKAGENT_OPENAI_API_KEY"),
];

/// Determine whether an API key is set for the given pair of env var names.
///
/// Returns `Some((value, source_env_var_name))` if found, `None` otherwise.
fn resolve_api_key(primary: &str, prefixed: &str) -> Option<(String, String)> {
    // Prefer the polkagent-prefixed variant first.
    if let Ok(v) = std::env::var(prefixed) {
        if !v.is_empty() {
            return Some((v, prefixed.to_owned()));
        }
    }
    if let Ok(v) = std::env::var(primary) {
        if !v.is_empty() {
            return Some((v, primary.to_owned()));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// login
// ---------------------------------------------------------------------------

fn login(cmd: &AuthLoginCmd) -> Result<()> {
    println!("Polkagent Auth Login");
    println!("{}", "-".repeat(40));
    println!();

    // Check which keys are already set via environment variables.
    let mut any_env_key = false;
    for (provider, primary, prefixed) in PROVIDER_KEYS {
        if let Some((_, source)) = resolve_api_key(primary, prefixed) {
            println!("  {provider} key is already set via environment variable ({source}).");
            any_env_key = true;
        }
    }

    if any_env_key {
        println!();
        println!("  Environment variables take precedence over stored credentials.");
        println!("  You can still store additional keys below.");
        println!();
    }

    let creds_path = credentials_path()?;

    // Ensure the parent directory exists.
    if let Some(parent) = creds_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating credentials directory {}", parent.display()))?;

        // Set directory permissions to 0700 on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o700);
            std::fs::set_permissions(parent, perms)
                .context("setting directory permissions to 0700")?;
        }
    }

    let mut lines: Vec<String> = Vec::new();

    for (provider, primary, _prefixed) in PROVIDER_KEYS {
        if cmd.provider.as_deref().map_or(false, |p| {
            !p.eq_ignore_ascii_case(provider)
        }) {
            continue;
        }

        print!("  Enter {provider} API key (leave blank to skip): ");
        io::stdout().flush().ok();

        let key = read_secret_from_stdin()?;
        if key.is_empty() {
            println!("  Skipping {provider}.");
            continue;
        }

        // Never display the full key.
        println!("  Stored {provider} key: {}", mask_key(&key));

        // Use the standard env var name as the key name in the file.
        lines.push(format!("{primary}={key}"));
    }

    if lines.is_empty() {
        println!();
        println!("No keys stored.");
        return Ok(());
    }

    if cmd.dry_run {
        println!();
        println!("  [dry-run] Would write {} key(s) to {}", lines.len(), creds_path.display());
        return Ok(());
    }

    // Append to (or create) the credentials file.
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&creds_path)
        .with_context(|| format!("opening credentials file {}", creds_path.display()))?;

    for line in &lines {
        writeln!(file, "{line}")
            .with_context(|| format!("writing credentials to {}", creds_path.display()))?;
    }
    drop(file);

    // Restrict file permissions to owner-only (0600).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&creds_path, perms)
            .context("setting credentials file permissions to 0600")?;
    }

    println!();
    println!("  Stored {} key(s) to {}", lines.len(), creds_path.display());
    println!("  Permissions set to 0600 (owner read/write only).");

    Ok(())
}

// ---------------------------------------------------------------------------
// logout
// ---------------------------------------------------------------------------

fn logout(cmd: &AuthLogoutCmd) -> Result<()> {
    println!("Polkagent Auth Logout");
    println!("{}", "-".repeat(40));
    println!();

    let creds_path = credentials_path()?;

    if !creds_path.exists() {
        println!("  No credentials file found at {}.", creds_path.display());
        println!("  Nothing to remove.");
        return Ok(());
    }

    if cmd.dry_run {
        println!("  [dry-run] Would remove {}", creds_path.display());
        return Ok(());
    }

    std::fs::remove_file(&creds_path)
        .with_context(|| format!("removing credentials file {}", creds_path.display()))?;

    println!("  Credentials file removed: {}", creds_path.display());
    println!();
    println!("  Note: Environment variables (ANTHROPIC_API_KEY, OPENAI_API_KEY, etc.)");
    println!("        are not affected. Unset them manually if needed.");

    Ok(())
}

// ---------------------------------------------------------------------------
// whoami
// ---------------------------------------------------------------------------

fn whoami(cmd: &AuthWhoamiCmd) -> Result<()> {
    // Collect key information for all providers.
    let mut key_infos: Vec<serde_json::Value> = Vec::new();

    for (provider, primary, prefixed) in PROVIDER_KEYS {
        let (masked, method) = match resolve_api_key(primary, prefixed) {
            Some((ref key, ref source)) => (mask_key(key), source.clone()),
            None => {
                // Check the credentials file as well.
                match read_key_from_credentials(primary) {
                    Some(ref key) => {
                        let creds = credentials_path()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| "~/.polkagent/credentials".to_owned());
                        (mask_key(key), format!("file:{creds}"))
                    }
                    None => ("[not configured]".to_owned(), "none".to_owned()),
                }
            }
        };

        key_infos.push(serde_json::json!({
            "provider": provider,
            "key": masked,
            "method": method,
        }));
    }

    if cmd.json {
        let out = serde_json::json!({ "providers": key_infos });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("Polkagent Identity");
    println!("{}", "-".repeat(40));
    println!();
    println!("  API Keys:");
    for info in &key_infos {
        let provider = info["provider"].as_str().unwrap_or("?");
        let key      = info["key"].as_str().unwrap_or("[not configured]");
        let method   = info["method"].as_str().unwrap_or("none");
        println!("    {provider:<12} {key:<20}  (source: {method})");
    }
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

fn status(cmd: &AuthStatusCmd) -> Result<()> {
    let mut providers: Vec<serde_json::Value> = Vec::new();

    for (provider, primary, prefixed) in PROVIDER_KEYS {
        let (configured, method) = match resolve_api_key(primary, prefixed) {
            Some((_, source)) => (true, source),
            None => match read_key_from_credentials(primary) {
                Some(_) => {
                    let creds = credentials_path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "~/.polkagent/credentials".to_owned());
                    (true, format!("file:{creds}"))
                }
                None => (false, "none".to_owned()),
            },
        };

        providers.push(serde_json::json!({
            "provider":   provider,
            "configured": configured,
            "method":     method,
        }));
    }

    let creds_path = credentials_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "~/.polkagent/credentials".to_owned());
    let creds_exists = std::path::Path::new(&creds_path).exists();

    if cmd.json {
        let out = serde_json::json!({
            "providers": providers,
            "credentials_file": {
                "path":   creds_path,
                "exists": creds_exists,
            },
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("Auth Status");
    println!("{}", "-".repeat(40));
    println!();
    println!("  Providers:");
    for p in &providers {
        let name       = p["provider"].as_str().unwrap_or("?");
        let configured = p["configured"].as_bool().unwrap_or(false);
        let method     = p["method"].as_str().unwrap_or("none");
        let glyph = if configured { "\u{25C9}" } else { "\u{25A0}" };
        let label = if configured { "OK  " } else { "MISS" };
        println!("    {glyph} [{label}] {name:<12}  source: {method}");
    }
    println!();
    println!("  Credentials file: {creds_path}");
    println!("    exists: {creds_exists}");
    println!();

    if providers.iter().all(|p| !p["configured"].as_bool().unwrap_or(false)) {
        println!("  No API keys configured. Run `polkagent auth login` to add keys.");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read a single line from stdin.
///
/// On Unix we attempt to use `stty -echo` via a child process so the key is
/// not visible while typing. On non-Unix platforms (or if `stty` is not
/// available) we fall back to plain `read_line` and the user will see their
/// input. We deliberately avoid pulling in an extra dependency (e.g. rpassword)
/// to keep the CLI crate lean.
fn read_secret_from_stdin() -> Result<String> {
    // Attempt echo-off via stty on Unix.
    #[cfg(unix)]
    let echo_disabled = {
        std::process::Command::new("stty")
            .arg("-echo")
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    #[cfg(not(unix))]
    let echo_disabled = false;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("reading from stdin")?;

    // Re-enable echo and print a newline if we disabled it.
    #[cfg(unix)]
    if echo_disabled {
        let _ = std::process::Command::new("stty")
            .arg("echo")
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        println!();
    }

    let _ = echo_disabled; // suppress unused warning on non-Unix

    Ok(input.trim_end_matches(['\n', '\r']).to_owned())
}

/// Try to read a specific key from the credentials file.
///
/// The file is expected to have `KEY=value` lines (shell-style).
/// Returns `None` if the file doesn't exist or the key is not found.
fn read_key_from_credentials(key_name: &str) -> Option<String> {
    let path = credentials_path().ok()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let prefix = format!("{key_name}=");
    for line in content.lines() {
        if let Some(value) = line.strip_prefix(&prefix) {
            let v = value.trim().to_owned();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_key_standard() {
        let masked = mask_key("sk-ant-api03-AbCdEf");
        assert!(masked.starts_with("sk-ant"));
        assert!(masked.contains("..."));
        assert!(masked.ends_with("CdEf"));
    }

    #[test]
    fn mask_key_too_short() {
        assert_eq!(mask_key("short"), "[REDACTED]");
    }

    #[test]
    fn mask_key_minimum_length() {
        // Exactly 11 chars: "ABCDEF...WXYZ" -> prefix=ABCDEF suffix=WXYZ
        let key = "ABCDEFGHIJK"; // 11 chars
        let masked = mask_key(key);
        assert!(masked.starts_with("ABCDEF"));
        assert!(masked.ends_with("HIJK"));
    }

    #[test]
    fn mask_key_openai_style() {
        let key = "sk-proj-1234567890ABCDefgh";
        let masked = mask_key(key);
        assert!(masked.starts_with("sk-pro"));
        assert!(masked.ends_with("efgh"));
        assert!(masked.contains("..."));
    }

    #[test]
    fn mask_key_exact_10_chars_is_redacted() {
        assert_eq!(mask_key("1234567890"), "[REDACTED]");
    }
}

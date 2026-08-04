//! Stderr noise suppression for the Codex CLI.
//!
//! The Codex app-server emits many benign log lines on stderr during normal
//! operation. This module identifies and suppresses known-benign patterns so
//! they don't pollute Polkagent's logs.

/// Known benign stderr prefixes and substrings from the Codex CLI.
///
/// Lines matching any of these patterns are considered noise and should be
/// suppressed (logged at TRACE level at most).
static BENIGN_PATTERNS: &[&str] = &[
    // Node.js / npm lifecycle
    "npm warn",
    "npm WARN",
    "npm notice",
    "DeprecationWarning",
    "ExperimentalWarning",
    "(node:",
    "Warning: ",
    // Codex startup
    "Starting Codex",
    "Codex is ready",
    "Loading model",
    "Model loaded",
    "Initializing",
    "Server listening",
    // Telemetry / analytics
    "Sending telemetry",
    "telemetry",
    "analytics",
    "Uploading crash report",
    // Git / filesystem
    "git: ",
    ".gitignore",
    "Watching files",
    "File watcher",
    // Network
    "Connecting to",
    "Connected to",
    "Retrying connection",
    "proxy",
    "HTTPS",
    "certificate",
];

/// Returns `true` if the stderr line is known-benign noise that should be
/// suppressed.
pub fn is_benign(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return true;
    }
    BENIGN_PATTERNS
        .iter()
        .any(|pattern| trimmed.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_line_is_benign() {
        assert!(is_benign(""));
        assert!(is_benign("   "));
    }

    #[test]
    fn npm_warnings_are_benign() {
        assert!(is_benign("npm warn deprecated package@1.0.0"));
        assert!(is_benign(
            "npm WARN config global `--global`, `--local` are deprecated"
        ));
        assert!(is_benign("npm notice New major version available!"));
    }

    #[test]
    fn node_warnings_are_benign() {
        assert!(is_benign("(node:12345) DeprecationWarning: something"));
        assert!(is_benign("(node:12345) ExperimentalWarning: something"));
        assert!(is_benign("Warning: some deprecation"));
    }

    #[test]
    fn codex_startup_messages_are_benign() {
        assert!(is_benign("Starting Codex app-server..."));
        assert!(is_benign("Codex is ready to accept connections"));
        assert!(is_benign("Loading model gpt-4.1"));
        assert!(is_benign("Model loaded in 2.3s"));
        assert!(is_benign("Initializing workspace..."));
    }

    #[test]
    fn telemetry_messages_are_benign() {
        assert!(is_benign("Sending telemetry data..."));
        assert!(is_benign("analytics: session started"));
    }

    #[test]
    fn real_errors_are_not_benign() {
        assert!(!is_benign("Error: ENOENT: no such file or directory"));
        assert!(!is_benign("SyntaxError: Unexpected token"));
        assert!(!is_benign(
            "TypeError: Cannot read property 'x' of undefined"
        ));
        assert!(!is_benign("fatal: something bad happened"));
    }
}

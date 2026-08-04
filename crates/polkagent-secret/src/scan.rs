//! Secret scanning utilities: detection and redaction of secrets in text.
//!
//! This module provides two complementary functions:
//!
//! - [`scrub_secrets`] — replace known secret values with `***REDACTED***`.
//! - [`detect_secrets`] — scan for patterns that *look* like secrets, even
//!   when the raw secret values are not known in advance.
//!
//! # Usage
//!
//! ```rust
//! use polkagent_secret::scan::{scrub_secrets, detect_secrets};
//!
//! // Scrub known secrets from a log line.
//! let line = "API key: sk-ant-abc123";
//! let clean = scrub_secrets(line, &["sk-ant-abc123"]);
//! assert_eq!(clean, "API key: ***REDACTED***");
//!
//! // Detect patterns without knowing the exact values.
//! let findings = detect_secrets("Authorization: Bearer eyJhb...");
//! assert!(!findings.is_empty());
//! ```

// ---------------------------------------------------------------------------
// SecretDetection
// ---------------------------------------------------------------------------

/// The category of a detected potential secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretKind {
    /// Anthropic API key (`sk-ant-*`).
    AnthropicApiKey,
    /// Generic OpenAI-style API key (`sk-*`, not `sk-ant-*`).
    ApiKey,
    /// HTTP `Authorization: Bearer <token>` header value.
    BearerToken,
    /// 64-byte hex-encoded private key (`0x` followed by 128 hex chars, or
    /// 128 raw hex chars).
    HexPrivateKey,
    /// `password=` parameter value in a URL query string or config line.
    Password,
}

impl std::fmt::Display for SecretKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AnthropicApiKey => write!(f, "AnthropicApiKey"),
            Self::ApiKey => write!(f, "ApiKey"),
            Self::BearerToken => write!(f, "BearerToken"),
            Self::HexPrivateKey => write!(f, "HexPrivateKey"),
            Self::Password => write!(f, "Password"),
        }
    }
}

/// A single detection result produced by [`detect_secrets`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretDetection {
    /// What kind of secret was detected.
    pub kind: SecretKind,
    /// Byte offset of the first character of the matched token in the input.
    pub start: usize,
    /// Byte offset of the first character *after* the matched token.
    pub end: usize,
    /// A safe preview of the match with the sensitive part replaced by `***`.
    pub redacted_preview: String,
}

// ---------------------------------------------------------------------------
// scrub_secrets
// ---------------------------------------------------------------------------

/// Replace every literal occurrence of each secret in `secrets` with
/// `"***REDACTED***"`.
///
/// The replacement is performed left-to-right using a simple string search.
/// Overlapping occurrences are handled correctly.  The comparison is
/// case-sensitive and byte-exact.
///
/// Returns an owned `String` with all occurrences replaced.
///
/// # Example
///
/// ```rust
/// use polkagent_secret::scan::scrub_secrets;
///
/// let text = "token: sk-ant-abc123 and also sk-ant-abc123";
/// let clean = scrub_secrets(text, &["sk-ant-abc123"]);
/// assert_eq!(clean, "token: ***REDACTED*** and also ***REDACTED***");
/// ```
pub fn scrub_secrets(text: &str, secrets: &[&str]) -> String {
    let mut result = text.to_owned();
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        result = result.replace(secret, "***REDACTED***");
    }
    result
}

// ---------------------------------------------------------------------------
// detect_secrets
// ---------------------------------------------------------------------------

/// Scan `text` for patterns that resemble secrets.
///
/// Each rule below is a simple string/prefix scan without regex dependencies:
///
/// | Pattern | Kind |
/// |---------|------|
/// | `sk-ant-` followed by non-whitespace | [`SecretKind::AnthropicApiKey`] |
/// | `sk-` (not `sk-ant-`) followed by non-whitespace | [`SecretKind::ApiKey`] |
/// | `Bearer ` followed by non-whitespace | [`SecretKind::BearerToken`] |
/// | `0x` + 128 hex chars OR 128 consecutive hex chars | [`SecretKind::HexPrivateKey`] |
/// | `password=` followed by non-whitespace | [`SecretKind::Password`] |
///
/// Returns a `Vec<SecretDetection>` sorted by `start` offset.  May return
/// multiple detections for different (or even the same) position if more than
/// one pattern matches.
pub fn detect_secrets(text: &str) -> Vec<SecretDetection> {
    let mut findings: Vec<SecretDetection> = Vec::new();

    findings.extend(detect_anthropic_keys(text));
    findings.extend(detect_generic_api_keys(text));
    findings.extend(detect_bearer_tokens(text));
    findings.extend(detect_hex_private_keys(text));
    findings.extend(detect_passwords(text));

    findings.sort_by_key(|f| f.start);
    findings
}

// ---------------------------------------------------------------------------
// Internal detection helpers
// ---------------------------------------------------------------------------

/// Scan for `sk-ant-<non-whitespace>` patterns.
fn detect_anthropic_keys(text: &str) -> Vec<SecretDetection> {
    let prefix = "sk-ant-";
    find_prefix_matches(text, prefix, SecretKind::AnthropicApiKey, "sk-ant-")
}

/// Scan for `sk-<non-whitespace>` patterns that are *not* `sk-ant-`.
fn detect_generic_api_keys(text: &str) -> Vec<SecretDetection> {
    let mut findings = Vec::new();
    let prefix = "sk-";
    let mut search_start = 0;

    while let Some(pos) = text[search_start..].find(prefix) {
        let abs_pos = search_start + pos;
        let rest = &text[abs_pos..];

        // Skip if this is actually an Anthropic key.
        if rest.starts_with("sk-ant-") {
            search_start = abs_pos + "sk-ant-".len();
            continue;
        }

        let token_end = token_end(rest);
        if token_end > prefix.len() {
            let matched = &rest[..token_end];
            let detection = SecretDetection {
                kind: SecretKind::ApiKey,
                start: abs_pos,
                end: abs_pos + token_end,
                redacted_preview: build_preview(matched, prefix.len()),
            };
            findings.push(detection);
        }

        search_start = abs_pos + prefix.len().max(1);
    }

    findings
}

/// Scan for `Bearer <token>` patterns.
///
/// Unlike other prefix matchers, `Bearer ` contains a trailing space that
/// `token_end` would stop at. So we search for `"Bearer "` and then measure
/// the non-whitespace token *after* the space to decide the match extent.
fn detect_bearer_tokens(text: &str) -> Vec<SecretDetection> {
    let prefix = "Bearer ";
    let mut findings = Vec::new();
    let mut search_start = 0;

    while let Some(pos) = text[search_start..].find(prefix) {
        let abs_pos = search_start + pos;
        let after_prefix = &text[abs_pos + prefix.len()..];
        let value_len = token_end(after_prefix);

        if value_len > 0 {
            let end = abs_pos + prefix.len() + value_len;
            let matched = &text[abs_pos..end];
            findings.push(SecretDetection {
                kind: SecretKind::BearerToken,
                start: abs_pos,
                end,
                redacted_preview: build_preview(matched, prefix.len()),
            });
        }

        search_start = abs_pos + prefix.len().max(1);
    }

    findings
}

/// Scan for 64-byte hex private keys (128 hex chars, optionally `0x`-prefixed).
fn detect_hex_private_keys(text: &str) -> Vec<SecretDetection> {
    const HEX_LEN: usize = 128; // 64 bytes × 2 nibbles
    let mut findings = Vec::new();
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = 0;

    while i < n {
        // Check for optional `0x` prefix.
        let (prefix_len, hex_start) =
            if i + 1 < n && bytes[i] == b'0' && (bytes[i + 1] == b'x' || bytes[i + 1] == b'X') {
                (2usize, i + 2)
            } else {
                (0usize, i)
            };

        // Verify there are at least 128 hex chars starting at hex_start.
        if hex_start + HEX_LEN <= n
            && bytes[hex_start..hex_start + HEX_LEN]
                .iter()
                .all(|b| b.is_ascii_hexdigit())
        {
            // Ensure the character before is a word boundary (not more hex).
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_hexdigit();
            // Ensure the character after the key is a word boundary.
            let after_idx = hex_start + HEX_LEN;
            let after_ok = after_idx >= n || !bytes[after_idx].is_ascii_hexdigit();

            if before_ok && after_ok {
                let end = i + prefix_len + HEX_LEN;
                let preview_prefix = if prefix_len > 0 { "0x" } else { "" };
                let redacted_preview = format!("{preview_prefix}***REDACTED_PRIVATE_KEY***");
                findings.push(SecretDetection {
                    kind: SecretKind::HexPrivateKey,
                    start: i,
                    end,
                    redacted_preview,
                });
                i = end;
                continue;
            }
        }

        i += 1;
    }

    // Suppress the unused `matched` warning: it is read via to_owned() in the
    // preview. Keep a reference to prevent the warning.
    let _ = findings.len(); // trivial use to satisfy clippy
    findings
}

/// Scan for `password=<non-whitespace>` patterns.
fn detect_passwords(text: &str) -> Vec<SecretDetection> {
    find_prefix_matches(text, "password=", SecretKind::Password, "password=")
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

/// Find all occurrences of `prefix` in `text` followed by at least one
/// non-whitespace character.
fn find_prefix_matches(
    text: &str,
    prefix: &str,
    kind: SecretKind,
    display_prefix: &str,
) -> Vec<SecretDetection> {
    let mut findings = Vec::new();
    let mut search_start = 0;

    while let Some(pos) = text[search_start..].find(prefix) {
        let abs_pos = search_start + pos;
        let rest = &text[abs_pos..];
        let token_end = token_end(rest);

        if token_end > prefix.len() {
            let matched = &rest[..token_end];
            findings.push(SecretDetection {
                kind: kind.clone(),
                start: abs_pos,
                end: abs_pos + token_end,
                redacted_preview: build_preview(matched, display_prefix.len()),
            });
        }

        search_start = abs_pos + prefix.len().max(1);
    }

    findings
}

/// Return the byte length of the first "token" (non-whitespace run) in `s`.
///
/// Starts from index 0 of `s` and extends until a whitespace character or
/// the end of the string.
fn token_end(s: &str) -> usize {
    s.char_indices()
        .take_while(|(_, c)| !c.is_whitespace())
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0)
}

/// Build a redacted preview: keep `prefix_len` bytes visible, replace the
/// rest with `***`.
fn build_preview(matched: &str, prefix_len: usize) -> String {
    if matched.len() <= prefix_len {
        matched.to_owned()
    } else {
        format!("{}***", &matched[..prefix_len])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- scrub_secrets ---

    #[test]
    fn scrub_replaces_known_secret() {
        let text = "Authorization: Bearer sk-ant-abc123";
        let cleaned = scrub_secrets(text, &["sk-ant-abc123"]);
        assert_eq!(cleaned, "Authorization: Bearer ***REDACTED***");
        assert!(!cleaned.contains("sk-ant-abc123"));
    }

    #[test]
    fn scrub_replaces_multiple_occurrences() {
        let text = "key1=secret1 key2=secret1";
        let cleaned = scrub_secrets(text, &["secret1"]);
        assert_eq!(cleaned, "key1=***REDACTED*** key2=***REDACTED***");
    }

    #[test]
    fn scrub_with_multiple_secrets() {
        let text = "a=foo b=bar c=baz";
        let cleaned = scrub_secrets(text, &["foo", "baz"]);
        assert_eq!(cleaned, "a=***REDACTED*** b=bar c=***REDACTED***");
    }

    #[test]
    fn scrub_empty_secret_is_ignored() {
        let text = "hello world";
        let cleaned = scrub_secrets(text, &[""]);
        assert_eq!(cleaned, "hello world");
    }

    #[test]
    fn scrub_no_secrets_returns_original() {
        let text = "nothing to scrub here";
        let cleaned = scrub_secrets(text, &[]);
        assert_eq!(cleaned, text);
    }

    // --- detect_secrets: API keys ---

    #[test]
    fn detect_anthropic_api_key() {
        let text = "key: sk-ant-api03-verylongkey12345";
        let findings = detect_secrets(text);
        assert!(!findings.is_empty(), "should detect anthropic key");
        let first = &findings[0];
        assert_eq!(first.kind, SecretKind::AnthropicApiKey);
        assert!(first.redacted_preview.starts_with("sk-ant-"));
    }

    #[test]
    fn detect_generic_api_key() {
        let text = "OPENAI_KEY=sk-proj-abcdefghij";
        let findings = detect_secrets(text);
        let api_keys: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == SecretKind::ApiKey)
            .collect();
        assert!(!api_keys.is_empty(), "should detect generic API key");
    }

    #[test]
    fn anthropic_key_not_double_counted_as_generic() {
        let text = "sk-ant-abc123";
        let findings = detect_secrets(text);
        // Should only produce AnthropicApiKey, not also ApiKey.
        let generic: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == SecretKind::ApiKey)
            .collect();
        assert!(
            generic.is_empty(),
            "Anthropic key should not be flagged as generic ApiKey"
        );
    }

    // --- detect_secrets: Bearer tokens ---

    #[test]
    fn detect_bearer_token() {
        let text = "Authorization: Bearer eyJhbGciOiJSUzI1NiJ9.payload.sig";
        let findings = detect_secrets(text);
        let bearer: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == SecretKind::BearerToken)
            .collect();
        assert!(!bearer.is_empty(), "should detect Bearer token");
        assert!(bearer[0].redacted_preview.starts_with("Bearer "));
    }

    #[test]
    fn no_detection_for_bearer_without_value() {
        // "Bearer " with only trailing whitespace / end-of-string.
        let text = "Bearer ";
        let findings = detect_secrets(text);
        let bearer: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == SecretKind::BearerToken)
            .collect();
        assert!(
            bearer.is_empty(),
            "should not detect Bearer without a token value"
        );
    }

    // --- detect_secrets: hex private keys ---

    #[test]
    fn detect_hex_private_key_with_0x_prefix() {
        let key = format!("0x{}", "a".repeat(128));
        let text = format!("key={key}");
        let findings = detect_secrets(&text);
        let hex: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == SecretKind::HexPrivateKey)
            .collect();
        assert!(
            !hex.is_empty(),
            "should detect hex private key with 0x prefix"
        );
        assert!(hex[0].redacted_preview.starts_with("0x"));
    }

    #[test]
    fn detect_hex_private_key_without_prefix() {
        let key = "b".repeat(128);
        let text = format!("privkey={key}");
        let findings = detect_secrets(&text);
        let hex: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == SecretKind::HexPrivateKey)
            .collect();
        assert!(!hex.is_empty(), "should detect raw hex private key");
    }

    #[test]
    fn short_hex_not_detected_as_private_key() {
        let key = "a".repeat(64); // only 32 bytes, not 64
        let text = format!("key={key}");
        let findings = detect_secrets(&text);
        let hex: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == SecretKind::HexPrivateKey)
            .collect();
        assert!(
            hex.is_empty(),
            "short hex should not be flagged as private key"
        );
    }

    // --- detect_secrets: passwords ---

    #[test]
    fn detect_password_field() {
        let text = "jdbc:postgresql://host/db?password=s3cr3tP@ss";
        let findings = detect_secrets(text);
        let passwords: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == SecretKind::Password)
            .collect();
        assert!(!passwords.is_empty(), "should detect password= field");
        assert!(passwords[0].redacted_preview.starts_with("password="));
    }

    // --- redacted_preview sanity ---

    #[test]
    fn redacted_preview_shows_prefix_only() {
        let text = "Authorization: Bearer supersecrettoken";
        let findings = detect_secrets(text);
        let bearer = findings.iter().find(|f| f.kind == SecretKind::BearerToken);
        assert!(bearer.is_some());
        let preview = &bearer.unwrap().redacted_preview;
        // Preview starts with "Bearer " and ends with "***".
        assert!(preview.starts_with("Bearer "));
        assert!(preview.ends_with("***"));
        // Preview does not contain the full token.
        assert!(!preview.contains("supersecrettoken"));
    }
}

//! Secret redaction utilities.
//!
//! [`Redacted<T>`] is a newtype wrapper that prevents accidental logging or
//! serialization of sensitive values. Pattern-based redaction is available
//! via [`redact_string`].

use std::fmt;

use serde::Serialize;
use zeroize::Zeroize;

// ---------------------------------------------------------------------------
// Redacted<T>
// ---------------------------------------------------------------------------

/// Newtype wrapper that hides a value from `Display`, `Debug`, and `Serialize`.
///
/// The inner value is only accessible through [`Redacted::inner`]. The
/// wrapped value is securely wiped (zeroized) when the wrapper is dropped.
pub struct Redacted<T: Zeroize> {
    value: T,
}

impl<T: Zeroize> Redacted<T> {
    /// Wrap a value so that it cannot be accidentally logged.
    pub fn new(value: T) -> Self {
        Self { value }
    }

    /// Access the inner value.
    ///
    /// Use sparingly -- this defeats the purpose of redaction.
    pub fn inner(&self) -> &T {
        &self.value
    }
}

impl<T: Zeroize> Drop for Redacted<T> {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

impl<T: Zeroize> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl<T: Zeroize> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Redacted(***)")
    }
}

impl<T: Zeroize> Serialize for Redacted<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str("[REDACTED]")
    }
}

// ---------------------------------------------------------------------------
// Pattern-based redaction
// ---------------------------------------------------------------------------

/// Redact known secret patterns from a string.
///
/// Patterns handled:
/// - API keys starting with `sk-` or `pk-` followed by alphanumeric/dash chars
/// - Hex seeds: `0x` followed by exactly 64 hex characters
/// - BIP-39 style mnemonics: sequences of 12 or 24 lowercase words
pub fn redact_string(s: &str) -> String {
    let mut result = s.to_owned();

    // API keys: sk-... or pk-... (at least 8 chars after prefix)
    let api_key_re = regex_lite::Regex::new(r"(sk-|pk-)[A-Za-z0-9_\-]{8,}").expect("valid regex");
    result = api_key_re
        .replace_all(&result, "${1}***REDACTED***")
        .into_owned();

    // Hex seeds: 0x followed by exactly 64 hex chars, not followed by more hex
    let hex_seed_re =
        regex_lite::Regex::new(r"0x[0-9a-fA-F]{64}(?:[^0-9a-fA-F]|$)").expect("valid regex");
    result = hex_seed_re
        .replace_all(&result, |caps: &regex_lite::Captures<'_>| {
            let m = caps.get(0).expect("match exists");
            let text = m.as_str();
            // text is 66 chars (0x + 64 hex) or 67 if there is a trailing non-hex char.
            // If 67, we need to preserve the trailing char.
            if text.len() > 66 {
                format!("0x***REDACTED***{}", &text[66..])
            } else {
                "0x***REDACTED***".to_owned()
            }
        })
        .into_owned();

    // Mnemonics: 24 lowercase words first, then 12 (greedy match order)
    let mnemonic_24_re = regex_lite::Regex::new(r"(?:^|\s)([a-z]{2,}(?:\s+[a-z]{2,}){23})(?:\s|$)")
        .expect("valid regex");
    result = mnemonic_24_re
        .replace_all(&result, " ***MNEMONIC_REDACTED*** ")
        .into_owned();

    let mnemonic_12_re = regex_lite::Regex::new(r"(?:^|\s)([a-z]{2,}(?:\s+[a-z]{2,}){11})(?:\s|$)")
        .expect("valid regex");
    result = mnemonic_12_re
        .replace_all(&result, " ***MNEMONIC_REDACTED*** ")
        .into_owned();

    result.trim().to_owned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_prints_redacted() {
        let r = Redacted::new("super-secret-key".to_owned());
        assert_eq!(format!("{r}"), "[REDACTED]");
    }

    #[test]
    fn debug_prints_redacted() {
        let r = Redacted::new(42_i32);
        assert_eq!(format!("{r:?}"), "Redacted(***)");
    }

    #[test]
    fn serialize_outputs_redacted_string() {
        let r = Redacted::new("sensitive".to_owned());
        let json = serde_json::to_string(&r).expect("serialize");
        assert_eq!(json, r#""[REDACTED]""#);
    }

    #[test]
    fn inner_returns_reference_to_value() {
        let r = Redacted::new("hello".to_owned());
        assert_eq!(r.inner(), "hello");
    }

    #[test]
    fn zeroize_on_drop() {
        // String implements Zeroize, so dropping runs the Zeroize-based Drop.
        // We verify the Drop impl runs without panic.
        let r = Redacted::new("sensitive".to_owned());
        drop(r);
    }

    #[test]
    fn redact_api_key_sk() {
        let input = "my key is sk-abc123def456ghi789";
        let result = redact_string(input);
        assert!(result.contains("sk-***REDACTED***"));
        assert!(!result.contains("abc123"));
    }

    #[test]
    fn redact_api_key_pk() {
        let input = "public key pk-XYZW1234ABCD5678";
        let result = redact_string(input);
        assert!(result.contains("pk-***REDACTED***"));
        assert!(!result.contains("XYZW1234"));
    }

    #[test]
    fn redact_hex_seed() {
        let hex = "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let input = format!("seed is {hex}");
        let result = redact_string(&input);
        assert!(result.contains("0x***REDACTED***"));
        assert!(!result.contains("abcdef1234567890"));
    }

    #[test]
    fn redact_12_word_mnemonic() {
        let mnemonic =
            "abandon ability able about above absent absorb abstract absurd abuse access acid";
        let result = redact_string(mnemonic);
        assert!(result.contains("***MNEMONIC_REDACTED***"));
        assert!(!result.contains("abandon ability"));
    }

    #[test]
    fn does_not_redact_short_sk_prefix() {
        // Keys shorter than 8 chars after prefix should not be redacted.
        let input = "sk-short";
        let result = redact_string(input);
        assert_eq!(result, input);
    }

    #[test]
    fn does_not_redact_normal_text() {
        let input = "Hello, this is a normal log message.";
        let result = redact_string(input);
        assert_eq!(result, input);
    }

    #[test]
    fn redact_hex_seed_wrong_length_not_redacted() {
        // Only exactly 64 hex chars after 0x should be redacted.
        let input = "0xabcdef12345678"; // too short
        let result = redact_string(input);
        assert_eq!(result, input);
    }
}

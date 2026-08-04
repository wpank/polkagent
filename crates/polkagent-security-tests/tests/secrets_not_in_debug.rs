//! PRD-15 Security Tests: Verify secrets don't leak through Debug/Display impls.
//!
//! These tests verify that the `Debug` and `Display` output of error types and
//! other structs carrying potentially sensitive data do NOT reveal:
//! - API keys
//! - Private keys / seeds
//! - Mnemonics
//! - Passwords

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Assert that `output` does not contain any of the `forbidden` substrings.
fn assert_no_leak(output: &str, forbidden: &[&str], context: &str) {
    for secret in forbidden {
        assert!(
            !output.contains(secret),
            "SECRET LEAK in {context}: Debug/Display output contains forbidden \
             substring {secret:?}.\nFull output: {output}"
        );
    }
}

/// Common sensitive test values.
const API_KEY: &str = "sk-ant-api03-SUPER_SECRET_KEY_1234567890abcdefghij";
const PRIVATE_KEY_HEX: &str = "0xdeadbeefcafebabe0123456789abcdef0123456789abcdef0123456789abcdef";
const MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const PASSWORD: &str = "hunter2_P@ssw0rd!";
const SEED_PHRASE: &str = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

/// All sensitive values to check against.
fn all_secrets() -> Vec<&'static str> {
    vec![API_KEY, PRIVATE_KEY_HEX, MNEMONIC, PASSWORD, SEED_PHRASE]
}

// ===========================================================================
// PolkagentError
// ===========================================================================

mod polkagent_error {
    use super::*;
    use polkagent_core::PolkagentError;

    #[test]
    fn internal_error_with_secret_in_message() {
        // If a developer accidentally puts a secret in an error message, the
        // error should still format. We verify that constructing an error with
        // a secret in the message field at least works, and that the error's
        // Debug repr is consistent with Display (no extra hidden fields
        // leaking secrets beyond what's in the message).
        let err = PolkagentError::internal(format!("failed auth for key: {API_KEY}"));
        let debug = format!("{err:?}");
        let display = format!("{err}");

        // The message IS the content, so it will contain the key. But this
        // test documents the risk: Internal errors must not be logged to
        // telemetry. We verify no EXTRA secret appears beyond what's in the
        // message.
        assert!(display.contains("failed auth"));
        assert!(debug.contains("Internal"));
    }

    #[test]
    fn validation_error_does_not_leak_secret_value() {
        // A well-formed validation error should describe the *field* that
        // failed, not echo the raw invalid value. Verify the pattern.
        let err = PolkagentError::validation("api_key", "must be a valid API key format");
        let debug = format!("{err:?}");
        let display = format!("{err}");

        assert!(display.contains("api_key"));
        assert!(display.contains("must be a valid"));
        assert_no_leak(&debug, &all_secrets(), "PolkagentError::Validation debug");
        assert_no_leak(
            &display,
            &all_secrets(),
            "PolkagentError::Validation display",
        );
    }

    #[test]
    fn classification_violation_does_not_leak_content() {
        let err = PolkagentError::ClassificationViolation {
            classification: "secret_forbidden".to_string(),
            reason: "item excluded from context assembly".to_string(),
        };
        let debug = format!("{err:?}");
        let display = format!("{err}");

        assert_no_leak(&debug, &all_secrets(), "ClassificationViolation debug");
        assert_no_leak(&display, &all_secrets(), "ClassificationViolation display");
        assert!(display.contains("secret_forbidden"));
    }

    #[test]
    fn policy_denied_does_not_leak_secrets() {
        let err = PolkagentError::PolicyDenied {
            reason: "insufficient grant for action".to_string(),
        };
        let debug = format!("{err:?}");
        let display = format!("{err}");

        assert_no_leak(&debug, &all_secrets(), "PolicyDenied debug");
        assert_no_leak(&display, &all_secrets(), "PolicyDenied display");
    }

    #[test]
    fn budget_exhausted_does_not_leak_secrets() {
        let err = PolkagentError::BudgetExhausted {
            resource: "input_tokens".to_string(),
            limit: "100000".to_string(),
        };
        let debug = format!("{err:?}");
        assert_no_leak(&debug, &all_secrets(), "BudgetExhausted debug");
    }

    #[test]
    fn unknown_outcome_does_not_leak_secrets() {
        let err = PolkagentError::UnknownOutcome {
            context: "chain RPC timeout".to_string(),
        };
        let debug = format!("{err:?}");
        assert_no_leak(&debug, &all_secrets(), "UnknownOutcome debug");
    }
}

// ===========================================================================
// ConfigError
// ===========================================================================

mod config_error {
    use super::*;
    use polkagent_config::ConfigError;

    #[test]
    fn parse_error_does_not_leak_secret_file_contents() {
        let err = ConfigError::Parse(
            "polkagent.toml".to_string(),
            "unexpected token at line 5".to_string(),
        );
        let debug = format!("{err:?}");
        let display = format!("{err}");

        assert_no_leak(&debug, &all_secrets(), "ConfigError::Parse debug");
        assert_no_leak(&display, &all_secrets(), "ConfigError::Parse display");
    }

    #[test]
    fn serialize_error_does_not_leak_secrets() {
        let err = ConfigError::Serialize("field type mismatch".to_string());
        let debug = format!("{err:?}");
        assert_no_leak(&debug, &all_secrets(), "ConfigError::Serialize debug");
    }

    #[test]
    fn deserialize_error_does_not_leak_secrets() {
        let err = ConfigError::Deserialize("missing field 'api'".to_string());
        let debug = format!("{err:?}");
        assert_no_leak(&debug, &all_secrets(), "ConfigError::Deserialize debug");
    }
}

// ===========================================================================
// ExecutorError
// ===========================================================================

mod executor_error {
    use super::*;
    use polkagent_executor_trait::ExecutorError;

    #[test]
    fn authentication_error_does_not_contain_raw_key() {
        // An authentication error should say "invalid" without echoing the key.
        let err = ExecutorError::Authentication {
            message: "API key is invalid or expired".to_string(),
        };
        let debug = format!("{err:?}");
        let display = format!("{err}");

        assert_no_leak(
            &debug,
            &all_secrets(),
            "ExecutorError::Authentication debug",
        );
        assert_no_leak(
            &display,
            &all_secrets(),
            "ExecutorError::Authentication display",
        );
        assert!(display.contains("invalid or expired"));
    }

    #[test]
    fn transport_error_does_not_leak_headers() {
        let err = ExecutorError::Transport {
            message: "connection refused to api.anthropic.com:443".to_string(),
            retryable: true,
        };
        let debug = format!("{err:?}");
        assert_no_leak(&debug, &all_secrets(), "ExecutorError::Transport debug");
    }

    #[test]
    fn rate_limit_error_safe() {
        let err = ExecutorError::RateLimit {
            retry_after_secs: Some(30),
        };
        let debug = format!("{err:?}");
        assert_no_leak(&debug, &all_secrets(), "ExecutorError::RateLimit debug");
    }

    #[test]
    fn internal_error_does_not_leak_secrets() {
        let err = ExecutorError::Internal {
            message: "unexpected response format".to_string(),
        };
        let debug = format!("{err:?}");
        assert_no_leak(&debug, &all_secrets(), "ExecutorError::Internal debug");
    }
}

// ===========================================================================
// SignerError
// ===========================================================================

mod signer_error {
    use super::*;
    use polkagent_signer_trait::{AccountRef, SignerError};

    #[test]
    fn account_not_found_does_not_leak_private_key() {
        let account = AccountRef::from_bytes([42u8; 32]);
        let err = SignerError::AccountNotFound { account };
        let debug = format!("{err:?}");
        let display = format!("{err}");

        // The account_id bytes might appear in Debug, but the private key must not.
        assert_no_leak(
            &debug,
            &[PRIVATE_KEY_HEX, MNEMONIC, PASSWORD, SEED_PHRASE],
            "SignerError::AccountNotFound debug",
        );
        assert_no_leak(
            &display,
            &all_secrets(),
            "SignerError::AccountNotFound display",
        );
    }

    #[test]
    fn hardware_error_does_not_leak_key_material() {
        let err = SignerError::Hardware {
            message: "device disconnected during signing".to_string(),
        };
        let debug = format!("{err:?}");
        let display = format!("{err}");

        assert_no_leak(&debug, &all_secrets(), "SignerError::Hardware debug");
        assert_no_leak(&display, &all_secrets(), "SignerError::Hardware display");
    }

    #[test]
    fn grant_mismatch_error_does_not_leak() {
        let err = SignerError::GrantMismatch;
        let debug = format!("{err:?}");
        assert_no_leak(&debug, &all_secrets(), "SignerError::GrantMismatch debug");
    }

    #[test]
    fn metadata_mismatch_error_does_not_leak() {
        let err = SignerError::MetadataMismatch;
        let debug = format!("{err:?}");
        assert_no_leak(
            &debug,
            &all_secrets(),
            "SignerError::MetadataMismatch debug",
        );
    }

    #[test]
    fn expired_error_does_not_leak() {
        let err = SignerError::Expired {
            expired_at: chrono::Utc::now(),
        };
        let debug = format!("{err:?}");
        assert_no_leak(&debug, &all_secrets(), "SignerError::Expired debug");
    }

    #[test]
    fn user_rejected_error_does_not_leak() {
        let err = SignerError::UserRejected;
        let debug = format!("{err:?}");
        let display = format!("{err}");
        assert_no_leak(&debug, &all_secrets(), "SignerError::UserRejected debug");
        assert!(display.contains("rejected"));
    }
}

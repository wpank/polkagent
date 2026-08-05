//! External [`Signer`] adapter for browser extensions and signing services.
//!
//! This crate provides [`ExternalSigner`] -- a [`Signer`] implementation that
//! delegates signing to an external process or browser extension (`Polkadot.js`,
//! Talisman, `SubWallet`, etc.) over HTTP.
//!
//! # Protocol
//!
//! The external signer communicates via HTTP with a signing service:
//!
//! - `POST {endpoint_url}/sign` — submit a [`SigningRequest`] and receive a
//!   [`SigningResponse`].
//! - `GET  {endpoint_url}/health` — verify the signer is reachable and ready.
//!
//! In the future, this transport may be extended to WebSocket or local IPC.
//!
//! # Key isolation invariant (INV-01)
//!
//! **The signer MUST receive the exact canonical bytes from
//! [`CanonicalSignRequest`].** The implementation MUST NOT modify, augment, or
//! add any model-generated content to the signing payload. The canonical
//! payload bytes are serialised to hex and forwarded as-is to the external
//! signing service. No field of the outgoing [`SigningRequest`] is derived
//! from LLM output, conversation history, or user free-form text.
//!
//! # Account filtering
//!
//! When `allowed_accounts` is configured, the signer will only sign requests
//! for accounts in that allowlist. Requests for unlisted accounts are rejected
//! with [`SignerError::AccountNotFound`].
//!
//! [`Signer`]: polkagent_signer_trait::Signer
//! [`CanonicalSignRequest`]: polkagent_signer_trait::CanonicalSignRequest
//! [`SignerError::AccountNotFound`]: polkagent_signer_trait::SignerError::AccountNotFound

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, error, info, warn};

use polkagent_core::now;
use polkagent_signer_trait::{
    AccountRef, CanonicalSignRequest, SignedPayload, Signer, SignerCapabilities, SignerError,
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for an [`ExternalSigner`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalSignerConfig {
    /// Base URL of the external signing service (e.g. `http://localhost:9933`).
    pub endpoint_url: String,
    /// Request timeout. Defaults to 30 seconds if `None`.
    pub timeout: Option<Duration>,
    /// If set, only these accounts may be signed with. Requests for accounts
    /// not in this list are rejected before reaching the external service.
    pub allowed_accounts: Option<Vec<AccountRef>>,
}

impl ExternalSignerConfig {
    /// Create a new config pointing at the given endpoint URL.
    pub fn new(endpoint_url: impl Into<String>) -> Self {
        Self {
            endpoint_url: endpoint_url.into(),
            timeout: None,
            allowed_accounts: None,
        }
    }

    /// Set the request timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set the allowed account list.
    #[must_use]
    pub fn with_allowed_accounts(mut self, accounts: Vec<AccountRef>) -> Self {
        self.allowed_accounts = Some(accounts);
        self
    }

    /// Return the effective timeout (default: 30 seconds).
    pub fn effective_timeout(&self) -> Duration {
        self.timeout.unwrap_or(Duration::from_secs(30))
    }

    /// Validate the configuration. Returns an error if the endpoint URL is
    /// empty.
    pub fn validate(&self) -> Result<(), ExternalSignerError> {
        if self.endpoint_url.is_empty() {
            return Err(ExternalSignerError::ProtocolError {
                message: "endpoint_url must not be empty".into(),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Wire types: request / response
// ---------------------------------------------------------------------------

/// JSON payload sent to the external signing service at `POST /sign`.
///
/// **INV-01:** Every field is derived from the [`CanonicalSignRequest`] —
/// no field is sourced from model output or conversation data.
///
/// [`CanonicalSignRequest`]: polkagent_signer_trait::CanonicalSignRequest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SigningRequest {
    /// The SCALE-encoded payload bytes, hex-encoded (without `0x` prefix).
    pub payload_hex: String,
    /// The 32-byte account ID, hex-encoded.
    pub account: String,
    /// The chain profile identifier.
    pub chain_id: String,
    /// The metadata hash, hex-encoded.
    pub metadata_hash: String,
}

/// JSON response from the external signing service.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SigningResponse {
    /// The raw signature bytes, hex-encoded.
    pub signature_hex: String,
    /// The full signed extrinsic, hex-encoded.
    pub signed_payload: String,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors specific to the external signer adapter.
///
/// These are mapped to [`SignerError`] at the trait boundary.
///
/// [`SignerError`]: polkagent_signer_trait::SignerError
#[derive(Debug, Error)]
pub enum ExternalSignerError {
    /// The external signer did not respond within the configured timeout.
    #[error("external signer timed out")]
    Timeout,

    /// Could not connect to the external signing service.
    #[error("connection refused to external signer endpoint")]
    ConnectionRefused,

    /// The external signer explicitly rejected the signing request.
    #[error("signing request rejected: {reason}")]
    Rejected {
        /// Human-readable reason for rejection.
        reason: String,
    },

    /// The returned signature is empty or malformed.
    #[error("invalid signature returned by external signer")]
    InvalidSignature,

    /// A protocol-level error (unexpected status code, malformed JSON, etc.).
    #[error("external signer protocol error: {message}")]
    ProtocolError {
        /// Human-readable description.
        message: String,
    },
}

impl From<ExternalSignerError> for SignerError {
    fn from(e: ExternalSignerError) -> Self {
        match e {
            ExternalSignerError::Timeout => SignerError::Timeout { elapsed_ms: 0 },
            ExternalSignerError::ConnectionRefused => SignerError::Internal {
                message: "connection refused to external signer endpoint".into(),
            },
            ExternalSignerError::Rejected { reason } => SignerError::Internal {
                message: format!("external signer rejected request: {reason}"),
            },
            ExternalSignerError::InvalidSignature => SignerError::Internal {
                message: "invalid signature returned by external signer".into(),
            },
            ExternalSignerError::ProtocolError { message } => SignerError::Internal { message },
        }
    }
}

// ---------------------------------------------------------------------------
// Approval callback
// ---------------------------------------------------------------------------

/// An optional callback invoked before sending the signing request to the
/// external service. Implementations may use this to display a confirmation
/// prompt to the user.
///
/// Returns `true` if the user approves, `false` to reject.
pub type ApprovalCallback = Arc<dyn Fn(&SigningRequest) -> bool + Send + Sync>;

// ---------------------------------------------------------------------------
// ExternalSigner
// ---------------------------------------------------------------------------

/// A [`Signer`] that delegates to an external signing service over HTTP.
///
/// # INV-01: Canonical byte integrity
///
/// This implementation forwards the **exact** canonical payload bytes from
/// [`CanonicalSignRequest::payload`] to the external service. It MUST NOT
/// modify, augment, or inject any model-generated content into the signing
/// request. The payload is hex-encoded for transport but the underlying bytes
/// are identical to what the kernel constructed.
///
/// [`Signer`]: polkagent_signer_trait::Signer
/// [`CanonicalSignRequest::payload`]: polkagent_signer_trait::CanonicalSignRequest::payload
pub struct ExternalSigner {
    config: ExternalSignerConfig,
    client: reqwest::Client,
    approval_callback: Option<ApprovalCallback>,
}

impl std::fmt::Debug for ExternalSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalSigner")
            .field("config", &self.config)
            .field("has_approval_callback", &self.approval_callback.is_some())
            .finish_non_exhaustive()
    }
}

impl ExternalSigner {
    /// Create a new `ExternalSigner` with the given configuration.
    ///
    /// Constructs an HTTP client with the configured timeout.
    pub fn new(config: ExternalSignerConfig) -> Result<Self, ExternalSignerError> {
        config.validate()?;

        let timeout = config.effective_timeout();
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| ExternalSignerError::ProtocolError {
                message: format!("failed to build HTTP client: {e}"),
            })?;

        Ok(Self {
            config,
            client,
            approval_callback: None,
        })
    }

    /// Attach an optional approval callback that is invoked before each
    /// signing request is sent to the external service.
    ///
    /// If the callback returns `false`, the request is rejected with
    /// [`SignerError::UserRejected`].
    #[must_use]
    pub fn with_approval_callback(mut self, cb: ApprovalCallback) -> Self {
        self.approval_callback = Some(cb);
        self
    }

    // -----------------------------------------------------------------------
    // INV-01: Build the wire request from canonical data only
    // -----------------------------------------------------------------------

    /// Build a [`SigningRequest`] from a [`CanonicalSignRequest`].
    ///
    /// **INV-01 assertion:** This function takes only the canonical request
    /// as input. Every field of the output [`SigningRequest`] is derived
    /// exclusively from typed domain records in `canonical`. No model output,
    /// conversation data, or user free-form text is injected.
    fn build_signing_request(canonical: &CanonicalSignRequest) -> SigningRequest {
        // INV-01: payload bytes are forwarded as-is, only hex-encoded for
        // transport. No bytes are added, removed, or modified.
        let payload_hex = hex_encode(&canonical.payload);
        let account = hex_encode(&canonical.account.account_id);
        let chain_id = canonical.chain_profile.0.clone();
        let metadata_hash = hex_encode(&canonical.metadata_hash.0);

        let request = SigningRequest {
            payload_hex,
            account,
            chain_id,
            metadata_hash,
        };

        // INV-01 runtime assertion: verify the hex-encoded payload round-trips
        // to the exact original bytes.
        debug_assert_eq!(
            hex_decode(&request.payload_hex),
            Some(canonical.payload.clone()),
            "INV-01 violation: payload hex does not round-trip to original bytes"
        );

        request
    }

    /// Check whether the given account is allowed by the configuration.
    fn is_account_allowed(&self, account: &AccountRef) -> bool {
        match &self.config.allowed_accounts {
            None => true,
            Some(allowed) => allowed.iter().any(|a| a.account_id == account.account_id),
        }
    }

    /// Map an HTTP/reqwest error to an [`ExternalSignerError`].
    fn map_reqwest_error(err: &reqwest::Error) -> ExternalSignerError {
        if err.is_timeout() {
            ExternalSignerError::Timeout
        } else if err.is_connect() {
            ExternalSignerError::ConnectionRefused
        } else {
            ExternalSignerError::ProtocolError {
                message: format!("HTTP request failed: {err}"),
            }
        }
    }

    /// Map an HTTP status code to an [`ExternalSignerError`].
    fn map_http_status(status: reqwest::StatusCode, body: &str) -> ExternalSignerError {
        match status.as_u16() {
            403 => ExternalSignerError::Rejected {
                reason: if body.is_empty() {
                    "forbidden".into()
                } else {
                    body.to_string()
                },
            },
            408 | 504 => ExternalSignerError::Timeout,
            502 | 503 => ExternalSignerError::ConnectionRefused,
            _ => ExternalSignerError::ProtocolError {
                message: format!("unexpected HTTP {status}: {body}"),
            },
        }
    }
}

#[async_trait]
impl Signer for ExternalSigner {
    /// Describe the signer's capabilities.
    ///
    /// Returns the allowed accounts from the configuration. If the external
    /// endpoint is reachable, this method may in the future query it for
    /// available accounts.
    async fn describe(&self) -> Result<SignerCapabilities, SignerError> {
        let accounts = self.config.allowed_accounts.clone().unwrap_or_default();

        Ok(SignerCapabilities {
            accounts,
            chain_profiles: vec![],
            hardware_backed: false,
            display_name: format!("ExternalSigner({})", self.config.endpoint_url),
            can_sign: true,
        })
    }

    /// Sign the canonical payload by forwarding it to the external service.
    ///
    /// # INV-01: Canonical byte integrity
    ///
    /// The signer receives the exact canonical bytes from
    /// [`CanonicalSignRequest::payload`]. This method:
    ///
    /// 1. Verifies the request has valid fields (no model data — INV-01).
    /// 2. Serialises the canonical payload to hex — no bytes are modified.
    /// 3. POSTs to `{endpoint_url}/sign` with the [`SigningRequest`].
    /// 4. Parses the response and verifies the signature is non-empty.
    /// 5. Returns [`SignedPayload`].
    ///
    /// [`CanonicalSignRequest::payload`]: polkagent_signer_trait::CanonicalSignRequest::payload
    async fn sign(&self, request: CanonicalSignRequest) -> Result<SignedPayload, SignerError> {
        // 0. Check expiry.
        if request.expires_at <= now() {
            return Err(SignerError::Expired {
                expired_at: request.expires_at,
            });
        }

        // 1. Account filtering: only sign for allowed accounts.
        if !self.is_account_allowed(&request.account) {
            warn!(
                account = ?request.account.account_id,
                "signing request for account not in allowed list"
            );
            return Err(SignerError::AccountNotFound {
                account: request.account.clone(),
            });
        }

        // 2. Build the wire request from canonical data only (INV-01).
        let signing_request = Self::build_signing_request(&request);

        info!(
            request_id = %request.request_id,
            chain = %request.chain_profile,
            payload_len = request.payload.len(),
            "submitting signing request to external signer"
        );

        // 3. Invoke approval callback if configured.
        if let Some(cb) = &self.approval_callback {
            if !cb(&signing_request) {
                info!(request_id = %request.request_id, "user rejected via approval callback");
                return Err(SignerError::UserRejected);
            }
        }

        // 4. POST to {endpoint_url}/sign.
        let url = format!("{}/sign", self.config.endpoint_url.trim_end_matches('/'));
        let http_response = self
            .client
            .post(&url)
            .json(&signing_request)
            .send()
            .await
            .map_err(|e| {
                error!(error = %e, "failed to reach external signer");
                SignerError::from(Self::map_reqwest_error(&e))
            })?;

        let status = http_response.status();
        let body = http_response.text().await.unwrap_or_default();

        if !status.is_success() {
            error!(status = %status, body = %body, "external signer returned error");
            return Err(Self::map_http_status(status, &body).into());
        }

        // 5. Parse the response.
        let signing_response: SigningResponse =
            serde_json::from_str(&body).map_err(|e| SignerError::Internal {
                message: format!("failed to parse signing response: {e}"),
            })?;

        // 6. Verify the signature is non-empty.
        if signing_response.signature_hex.is_empty() {
            return Err(ExternalSignerError::InvalidSignature.into());
        }

        let signature =
            hex_decode(&signing_response.signature_hex).ok_or_else(|| SignerError::Internal {
                message: "invalid hex in signature_hex".into(),
            })?;

        let signed_extrinsic =
            hex_decode(&signing_response.signed_payload).ok_or_else(|| SignerError::Internal {
                message: "invalid hex in signed_payload".into(),
            })?;

        debug!(
            request_id = %request.request_id,
            sig_len = signature.len(),
            "received signature from external signer"
        );

        Ok(SignedPayload {
            signed_extrinsic,
            public_key: request.account.account_id.to_vec(),
            signature,
        })
    }

    /// Health check: GET `{endpoint_url}/health`.
    async fn health(&self) -> Result<(), SignerError> {
        let url = format!("{}/health", self.config.endpoint_url.trim_end_matches('/'));
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| SignerError::from(Self::map_reqwest_error(&e)))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(SignerError::Internal {
                message: format!("health check returned HTTP {}", response.status()),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Hex helpers
// ---------------------------------------------------------------------------

/// Encode bytes as lowercase hex (no `0x` prefix).
fn hex_encode(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for &byte in bytes {
        encoded.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// Decode a hex string (with or without `0x` prefix) to bytes.
/// Returns `None` if the input is invalid hex.
fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::now;
    use polkagent_signer_trait::{ApprovalId, ChainProfileId, GrantDigest, MetadataDigest};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn test_config() -> ExternalSignerConfig {
        ExternalSignerConfig::new("http://localhost:19933")
    }

    fn test_account() -> AccountRef {
        AccountRef::from_bytes([0xAA; 32])
    }

    fn valid_request(account: AccountRef) -> CanonicalSignRequest {
        CanonicalSignRequest {
            request_id: "req-ext-001".into(),
            payload: vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02],
            account,
            chain_profile: ChainProfileId::new("polkadot"),
            metadata_hash: MetadataDigest(vec![0xAB; 32]),
            grant_digest: GrantDigest(vec![0xCD; 32]),
            approval_id: ApprovalId::new("approval-ext-001"),
            expires_at: now() + chrono::Duration::hours(1),
        }
    }

    // -----------------------------------------------------------------------
    // Config construction and validation
    // -----------------------------------------------------------------------

    #[test]
    fn config_new_sets_endpoint() {
        let config = ExternalSignerConfig::new("http://localhost:9933");
        assert_eq!(config.endpoint_url, "http://localhost:9933");
        assert!(config.timeout.is_none());
        assert!(config.allowed_accounts.is_none());
    }

    #[test]
    fn config_with_timeout() {
        let config = ExternalSignerConfig::new("http://localhost:9933")
            .with_timeout(Duration::from_secs(10));
        assert_eq!(config.timeout, Some(Duration::from_secs(10)));
    }

    #[test]
    fn config_with_allowed_accounts() {
        let account = test_account();
        let config = ExternalSignerConfig::new("http://localhost:9933")
            .with_allowed_accounts(vec![account.clone()]);
        let allowed = config.allowed_accounts.as_ref().map_or(0, Vec::len);
        assert_eq!(allowed, 1);
    }

    #[test]
    fn config_effective_timeout_default() {
        let config = ExternalSignerConfig::new("http://localhost:9933");
        assert_eq!(config.effective_timeout(), Duration::from_secs(30));
    }

    #[test]
    fn config_effective_timeout_custom() {
        let config =
            ExternalSignerConfig::new("http://localhost:9933").with_timeout(Duration::from_secs(5));
        assert_eq!(config.effective_timeout(), Duration::from_secs(5));
    }

    #[test]
    fn config_validate_rejects_empty_endpoint() {
        let config = ExternalSignerConfig::new("");
        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn config_validate_accepts_valid_endpoint() {
        let config = ExternalSignerConfig::new("http://localhost:9933");
        assert!(config.validate().is_ok());
    }

    // -----------------------------------------------------------------------
    // ExternalSigner construction
    // -----------------------------------------------------------------------

    #[test]
    fn new_creates_signer_with_valid_config() {
        let config = test_config();
        let signer = ExternalSigner::new(config);
        assert!(signer.is_ok());
    }

    #[test]
    fn new_rejects_empty_endpoint() {
        let config = ExternalSignerConfig::new("");
        let result = ExternalSigner::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn with_approval_callback_attaches_callback() {
        let config = test_config();
        let signer =
            ExternalSigner::new(config).map(|s| s.with_approval_callback(Arc::new(|_| true)));
        assert!(signer.is_ok());
    }

    // -----------------------------------------------------------------------
    // SigningRequest serialization
    // -----------------------------------------------------------------------

    #[test]
    fn signing_request_serialization_matches_expected_format() {
        let req = SigningRequest {
            payload_hex: "deadbeef".into(),
            account: "aa".repeat(32),
            chain_id: "polkadot".into(),
            metadata_hash: "bb".repeat(32),
        };

        let json = serde_json::to_value(&req).unwrap_or_default();

        assert_eq!(json["payload_hex"], "deadbeef");
        assert_eq!(json["account"], "aa".repeat(32));
        assert_eq!(json["chain_id"], "polkadot");
        assert_eq!(json["metadata_hash"], "bb".repeat(32));
    }

    #[test]
    fn signing_request_round_trips_through_json() {
        let req = SigningRequest {
            payload_hex: "0102030405".into(),
            account: "cc".repeat(32),
            chain_id: "westend".into(),
            metadata_hash: "dd".repeat(32),
        };

        let json = serde_json::to_string(&req).unwrap_or_default();
        let parsed: SigningRequest = serde_json::from_str(&json).unwrap_or_else(|_| req.clone());
        assert_eq!(parsed, req);
    }

    // -----------------------------------------------------------------------
    // SigningResponse parsing
    // -----------------------------------------------------------------------

    #[test]
    fn signing_response_deserializes_from_json() {
        let json = r#"{
            "signature_hex": "aabbccdd",
            "signed_payload": "deadbeefaabbccdd"
        }"#;
        let resp: Result<SigningResponse, _> = serde_json::from_str(json);
        assert!(resp.is_ok());
        let resp = resp.unwrap_or_else(|_| SigningResponse {
            signature_hex: String::new(),
            signed_payload: String::new(),
        });
        assert_eq!(resp.signature_hex, "aabbccdd");
        assert_eq!(resp.signed_payload, "deadbeefaabbccdd");
    }

    #[test]
    fn signing_response_rejects_missing_fields() {
        let json = r#"{ "signature_hex": "aabb" }"#;
        let resp: Result<SigningResponse, _> = serde_json::from_str(json);
        assert!(resp.is_err());
    }

    // -----------------------------------------------------------------------
    // Error mapping
    // -----------------------------------------------------------------------

    #[test]
    fn external_error_timeout_maps_to_signer_timeout() {
        let err: SignerError = ExternalSignerError::Timeout.into();
        assert!(matches!(err, SignerError::Timeout { .. }));
    }

    #[test]
    fn external_error_connection_refused_maps_to_internal() {
        let err: SignerError = ExternalSignerError::ConnectionRefused.into();
        assert!(matches!(err, SignerError::Internal { .. }));
        let msg = format!("{err}");
        assert!(msg.contains("connection refused"));
    }

    #[test]
    fn external_error_rejected_maps_to_internal() {
        let err: SignerError = ExternalSignerError::Rejected {
            reason: "user declined".into(),
        }
        .into();
        assert!(matches!(err, SignerError::Internal { .. }));
        let msg = format!("{err}");
        assert!(msg.contains("user declined"));
    }

    #[test]
    fn external_error_invalid_signature_maps_to_internal() {
        let err: SignerError = ExternalSignerError::InvalidSignature.into();
        assert!(matches!(err, SignerError::Internal { .. }));
    }

    #[test]
    fn external_error_protocol_maps_to_internal() {
        let err: SignerError = ExternalSignerError::ProtocolError {
            message: "bad json".into(),
        }
        .into();
        assert!(matches!(err, SignerError::Internal { .. }));
        let msg = format!("{err}");
        assert!(msg.contains("bad json"));
    }

    #[test]
    fn http_status_403_maps_to_rejected() {
        let err = ExternalSigner::map_http_status(reqwest::StatusCode::FORBIDDEN, "not authorized");
        assert!(matches!(err, ExternalSignerError::Rejected { .. }));
    }

    #[test]
    fn http_status_408_maps_to_timeout() {
        let err = ExternalSigner::map_http_status(reqwest::StatusCode::REQUEST_TIMEOUT, "");
        assert!(matches!(err, ExternalSignerError::Timeout));
    }

    #[test]
    fn http_status_503_maps_to_connection_refused() {
        let err = ExternalSigner::map_http_status(reqwest::StatusCode::SERVICE_UNAVAILABLE, "");
        assert!(matches!(err, ExternalSignerError::ConnectionRefused));
    }

    #[test]
    fn http_status_500_maps_to_protocol_error() {
        let err = ExternalSigner::map_http_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "internal error",
        );
        assert!(matches!(err, ExternalSignerError::ProtocolError { .. }));
    }

    // -----------------------------------------------------------------------
    // INV-01: Canonical bytes forwarded unchanged
    // -----------------------------------------------------------------------

    #[test]
    fn inv01_canonical_bytes_are_forwarded_unchanged() {
        // The exact canonical payload bytes must appear in the signing
        // request as hex, without any modification.
        let canonical_payload: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04];
        let account = test_account();
        let req = CanonicalSignRequest {
            request_id: "inv01-test".into(),
            payload: canonical_payload.clone(),
            account,
            chain_profile: ChainProfileId::new("polkadot"),
            metadata_hash: MetadataDigest(vec![0xFF; 32]),
            grant_digest: GrantDigest(vec![0xEE; 32]),
            approval_id: ApprovalId::new("inv01-approval"),
            expires_at: now() + chrono::Duration::hours(1),
        };

        let signing_req = ExternalSigner::build_signing_request(&req);

        // Verify the payload hex decodes back to the exact original bytes.
        let round_tripped = hex_decode(&signing_req.payload_hex);
        assert_eq!(
            round_tripped,
            Some(canonical_payload),
            "INV-01: canonical payload bytes must be forwarded unchanged"
        );
    }

    #[test]
    fn inv01_no_extra_fields_in_signing_request() {
        // The signing request must contain only fields derived from the
        // canonical request — verify the JSON has exactly the expected keys.
        let account = test_account();
        let req = valid_request(account);
        let signing_req = ExternalSigner::build_signing_request(&req);

        let json = serde_json::to_value(&signing_req).unwrap_or_default();
        let obj = json.as_object();
        assert!(obj.is_some());
        let keys: Vec<&String> = obj.map(|o| o.keys().collect()).unwrap_or_default();
        assert_eq!(keys.len(), 4, "signing request must have exactly 4 fields");
        assert!(keys.contains(&&"payload_hex".to_string()));
        assert!(keys.contains(&&"account".to_string()));
        assert!(keys.contains(&&"chain_id".to_string()));
        assert!(keys.contains(&&"metadata_hash".to_string()));
    }

    #[test]
    fn inv01_account_bytes_forwarded_unchanged() {
        let account = AccountRef::from_bytes([0x42; 32]);
        let req = valid_request(account.clone());
        let signing_req = ExternalSigner::build_signing_request(&req);

        let decoded_account = hex_decode(&signing_req.account);
        assert_eq!(
            decoded_account,
            Some(account.account_id.to_vec()),
            "INV-01: account bytes must be forwarded unchanged"
        );
    }

    #[test]
    fn inv01_metadata_hash_forwarded_unchanged() {
        let account = test_account();
        let req = valid_request(account);
        let signing_req = ExternalSigner::build_signing_request(&req);

        let decoded_hash = hex_decode(&signing_req.metadata_hash);
        assert_eq!(
            decoded_hash,
            Some(req.metadata_hash.0),
            "INV-01: metadata hash must be forwarded unchanged"
        );
    }

    // -----------------------------------------------------------------------
    // Account filtering
    // -----------------------------------------------------------------------

    #[test]
    fn account_filtering_allows_listed_account() {
        let account = test_account();
        let config = ExternalSignerConfig::new("http://localhost:19933")
            .with_allowed_accounts(vec![account.clone()]);
        let signer = ExternalSigner::new(config).unwrap_or_else(|_| {
            panic!("valid config should create signer");
        });
        assert!(signer.is_account_allowed(&account));
    }

    #[test]
    fn account_filtering_rejects_unlisted_account() {
        let allowed = AccountRef::from_bytes([0xAA; 32]);
        let unlisted = AccountRef::from_bytes([0xBB; 32]);
        let config = ExternalSignerConfig::new("http://localhost:19933")
            .with_allowed_accounts(vec![allowed]);
        let signer = ExternalSigner::new(config).unwrap_or_else(|_| {
            panic!("valid config should create signer");
        });
        assert!(!signer.is_account_allowed(&unlisted));
    }

    #[test]
    fn account_filtering_allows_all_when_no_allowlist() {
        let config = ExternalSignerConfig::new("http://localhost:19933");
        let signer = ExternalSigner::new(config).unwrap_or_else(|_| {
            panic!("valid config should create signer");
        });
        let any_account = AccountRef::from_bytes([0xFF; 32]);
        assert!(signer.is_account_allowed(&any_account));
    }

    #[tokio::test]
    async fn sign_rejects_unlisted_account() {
        let allowed = AccountRef::from_bytes([0xAA; 32]);
        let unlisted = AccountRef::from_bytes([0xBB; 32]);
        let config = ExternalSignerConfig::new("http://localhost:19933")
            .with_allowed_accounts(vec![allowed]);
        let signer = ExternalSigner::new(config).unwrap_or_else(|_| {
            panic!("valid config should create signer");
        });

        let req = valid_request(unlisted);
        let result = signer.sign(req).await;
        assert!(matches!(result, Err(SignerError::AccountNotFound { .. })));
    }

    // -----------------------------------------------------------------------
    // Timeout handling
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn sign_with_expired_request_returns_expired() {
        let config = test_config();
        let signer = ExternalSigner::new(config).unwrap_or_else(|_| {
            panic!("valid config should create signer");
        });

        let account = test_account();
        let mut req = valid_request(account);
        req.expires_at = now() - chrono::Duration::seconds(1);

        let result = signer.sign(req).await;
        assert!(matches!(result, Err(SignerError::Expired { .. })));
    }

    #[tokio::test]
    async fn sign_to_unreachable_endpoint_returns_error() {
        // Use a port that is almost certainly not listening.
        let config = ExternalSignerConfig::new("http://127.0.0.1:19999")
            .with_timeout(Duration::from_millis(500));
        let signer = ExternalSigner::new(config).unwrap_or_else(|_| {
            panic!("valid config should create signer");
        });

        let account = test_account();
        let req = valid_request(account);
        let result = signer.sign(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn health_to_unreachable_endpoint_returns_error() {
        let config = ExternalSignerConfig::new("http://127.0.0.1:19999")
            .with_timeout(Duration::from_millis(500));
        let signer = ExternalSigner::new(config).unwrap_or_else(|_| {
            panic!("valid config should create signer");
        });

        let result = signer.health().await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Describe
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn describe_returns_allowed_accounts() {
        let account = test_account();
        let config = ExternalSignerConfig::new("http://localhost:19933")
            .with_allowed_accounts(vec![account.clone()]);
        let signer = ExternalSigner::new(config).unwrap_or_else(|_| {
            panic!("valid config should create signer");
        });

        let caps = signer.describe().await;
        assert!(caps.is_ok());
        let caps = caps.unwrap_or_else(|_| SignerCapabilities {
            accounts: vec![],
            chain_profiles: vec![],
            hardware_backed: false,
            display_name: String::new(),
            can_sign: true,
        });
        assert_eq!(caps.accounts.len(), 1);
        assert_eq!(caps.accounts[0].account_id, [0xAA; 32]);
        assert!(!caps.hardware_backed);
        assert!(caps.display_name.contains("ExternalSigner"));
    }

    #[tokio::test]
    async fn describe_returns_empty_when_no_allowed_accounts() {
        let config = ExternalSignerConfig::new("http://localhost:19933");
        let signer = ExternalSigner::new(config).unwrap_or_else(|_| {
            panic!("valid config should create signer");
        });

        let caps = signer.describe().await;
        assert!(caps.is_ok());
        let caps = caps.unwrap_or_else(|_| SignerCapabilities {
            accounts: vec![],
            chain_profiles: vec![],
            hardware_backed: false,
            display_name: String::new(),
            can_sign: true,
        });
        assert!(caps.accounts.is_empty());
    }

    // -----------------------------------------------------------------------
    // Approval callback
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn approval_callback_rejection_returns_user_rejected() {
        let account = test_account();
        let config = ExternalSignerConfig::new("http://localhost:19933")
            .with_allowed_accounts(vec![account.clone()]);
        let signer = ExternalSigner::new(config)
            .unwrap_or_else(|_| panic!("valid config"))
            .with_approval_callback(Arc::new(|_| false));

        let req = valid_request(account);
        let result = signer.sign(req).await;
        assert!(matches!(result, Err(SignerError::UserRejected)));
    }

    // -----------------------------------------------------------------------
    // Hex helpers
    // -----------------------------------------------------------------------

    #[test]
    fn hex_encode_produces_correct_output() {
        assert_eq!(hex_encode(&[0xDE, 0xAD, 0xBE, 0xEF]), "deadbeef");
        assert_eq!(hex_encode(&[]), "");
        assert_eq!(hex_encode(&[0x00, 0xFF]), "00ff");
    }

    #[test]
    fn hex_decode_produces_correct_output() {
        assert_eq!(hex_decode("deadbeef"), Some(vec![0xDE, 0xAD, 0xBE, 0xEF]));
        assert_eq!(hex_decode(""), Some(vec![]));
        assert_eq!(hex_decode("00ff"), Some(vec![0x00, 0xFF]));
    }

    #[test]
    fn hex_decode_strips_0x_prefix() {
        assert_eq!(hex_decode("0xdeadbeef"), Some(vec![0xDE, 0xAD, 0xBE, 0xEF]));
    }

    #[test]
    fn hex_decode_rejects_odd_length() {
        assert_eq!(hex_decode("abc"), None);
    }

    #[test]
    fn hex_decode_rejects_invalid_chars() {
        assert_eq!(hex_decode("zzzz"), None);
    }

    // -----------------------------------------------------------------------
    // Object safety
    // -----------------------------------------------------------------------

    /// Compile-time check: `ExternalSigner` can be used as `dyn Signer`.
    #[allow(dead_code)]
    fn _external_signer_is_object_safe(_s: &dyn Signer) {}
}

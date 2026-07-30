//! Shared test fixtures and builder helpers for the Polkagent workspace.
//!
//! This crate provides deterministic factory functions and builder types for
//! constructing domain values used across the workspace's test suites. All
//! functions return values that are stable, predictable, and safe to use in
//! parallel tests.
//!
//! # Design principles
//!
//! - Every fixture function is pure and deterministic: same inputs always
//!   produce structurally equivalent values.
//! - Fixture values are valid by construction; they pass all type-level
//!   invariants without requiring extra setup.
//! - Builder patterns are preferred over long argument lists so that tests
//!   can override only the fields they care about.
//!
//! # Usage
//!
//! ```rust
//! use polkagent_test_fixtures::*;
//!
//! // Create minimal valid domain values for tests.
//! let run_id = test_run_id();
//! let request = test_inference_request();
//! let tool = test_tool_definition();
//! ```

use chrono::{Duration, Utc};
use polkagent_executor_trait::{
    ContentBlock, InferenceMessage, InferenceRequest, InferenceResponse, MessageRole,
    ToolCall, ToolDefinition, TokenUsage,
};
use polkagent_signer_trait::{
    AccountRef, ApprovalId, CanonicalSignRequest, ChainProfileId, GrantDigest, MetadataDigest,
    SignerCapabilities,
};
use polkagent_transport_trait::{
    AuthenticatedSender, Classification,
    DeliveryId, IncomingMessage, MessageBody, OutgoingBody, OutgoingMessage, SenderTrustTier,
    UserId,
};

// ---------------------------------------------------------------------------
// Re-exports for convenience
// ---------------------------------------------------------------------------

pub use polkagent_core::{RunId, StepId, ConversationId};

// ---------------------------------------------------------------------------
// Core ID fixtures
// ---------------------------------------------------------------------------

/// Return a deterministic `RunId` for use in tests.
///
/// The value is unique per call (UUID v7), but callers that need a fixed
/// value should store it once and reuse it.
#[must_use]
pub fn test_run_id() -> RunId {
    RunId::new()
}

/// Return a deterministic `StepId` for use in tests.
#[must_use]
pub fn test_step_id() -> StepId {
    StepId::new()
}

/// Return a deterministic `ConversationId` for use in tests.
#[must_use]
pub fn test_conversation_id() -> ConversationId {
    ConversationId::new()
}

// ---------------------------------------------------------------------------
// Executor trait fixtures
// ---------------------------------------------------------------------------

/// Return a minimal valid [`InferenceRequest`].
///
/// The request contains a single user message (`"What is Polkadot?"`),
/// no tools, and no system prompt.
#[must_use]
pub fn test_inference_request() -> InferenceRequest {
    InferenceRequest {
        run_id: test_run_id(),
        step_id: test_step_id(),
        messages: vec![InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text {
                text: "What is Polkadot?".into(),
            }],
        }],
        system: None,
        tools: vec![],
        model_id: "claude-opus-4-6".into(),
        max_tokens: 1024,
        temperature: Some(0.7),
    }
}

/// Return a minimal valid [`InferenceResponse`] with a text body.
///
/// The response has `stop_reason = "end_turn"` and zeroed token usage.
#[must_use]
pub fn test_inference_response() -> InferenceResponse {
    InferenceResponse {
        text: "Polkadot is a multi-chain network.".into(),
        tool_calls: vec![],
        stop_reason: "end_turn".into(),
        usage: TokenUsage {
            input_tokens: 10,
            output_tokens: 8,
            ..Default::default()
        },
        provider_request_id: Some("test-provider-req-001".into()),
    }
}

/// Return a minimal valid [`ToolDefinition`].
///
/// The tool is `polkagent.file.read` with a simple path parameter.
#[must_use]
pub fn test_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "polkagent.file.read".into(),
        description: "Read the contents of a file at the given path.".into(),
        input_schema_json: r#"{"type":"object","properties":{"path":{"type":"string","description":"Absolute path to the file"}},"required":["path"]}"#.into(),
    }
}

/// Return a minimal valid [`ToolCall`] for `polkagent.file.read`.
#[must_use]
pub fn test_tool_call() -> ToolCall {
    ToolCall {
        tool_call_id: "tc-test-001".into(),
        tool_name: "polkagent.file.read".into(),
        arguments_json: r#"{"path":"/tmp/test.txt"}"#.into(),
    }
}

// ---------------------------------------------------------------------------
// Signer trait fixtures
// ---------------------------------------------------------------------------

/// Return a test [`AccountRef`] using the all-zeros 32-byte account ID.
#[must_use]
pub fn test_account_ref() -> AccountRef {
    AccountRef::from_bytes([0u8; 32])
}

/// Return a test [`AccountRef`] with a non-zero account ID.
///
/// The account ID byte is set to `seed % 256` across all 32 bytes.
#[must_use]
pub fn test_account_ref_seeded(seed: u8) -> AccountRef {
    AccountRef::from_bytes([seed; 32])
}

/// Return a test [`ChainProfileId`] for Polkadot mainnet.
#[must_use]
pub fn test_chain_profile_id() -> ChainProfileId {
    ChainProfileId::new("polkadot")
}

/// Return a minimal valid [`CanonicalSignRequest`].
///
/// The request uses the all-zeros account, Polkadot chain profile, and a
/// fake payload of `[0x01, 0x02, 0x03, 0x04]`. The expiry is set one hour
/// from now.
#[must_use]
pub fn test_sign_request() -> CanonicalSignRequest {
    CanonicalSignRequest {
        request_id: "test-sign-req-001".into(),
        payload: vec![0x01, 0x02, 0x03, 0x04],
        account: test_account_ref(),
        chain_profile: test_chain_profile_id(),
        metadata_hash: MetadataDigest(vec![0xAB; 32]),
        grant_digest: GrantDigest(vec![0xCD; 32]),
        approval_id: ApprovalId::new("test-approval-001"),
        expires_at: Utc::now() + Duration::hours(1),
    }
}

/// Return a [`SignerCapabilities`] describing the fake signer.
#[must_use]
pub fn test_signer_capabilities() -> SignerCapabilities {
    SignerCapabilities {
        accounts: vec![test_account_ref()],
        chain_profiles: vec![test_chain_profile_id()],
        hardware_backed: false,
        display_name: "TestSigner".into(),
    }
}

// ---------------------------------------------------------------------------
// Transport trait fixtures
// ---------------------------------------------------------------------------

/// Return a test [`DeliveryId`].
#[must_use]
pub fn test_delivery_id() -> DeliveryId {
    DeliveryId::new("test-delivery-001")
}

/// Return a test [`UserId`].
#[must_use]
pub fn test_user_id() -> UserId {
    UserId::new("test-user-alice")
}

/// Return a test [`AuthenticatedSender`] for Alice.
#[must_use]
pub fn test_sender() -> AuthenticatedSender {
    AuthenticatedSender {
        user_id: test_user_id(),
        display_name: Some("Alice".into()),
        trust_tier: SenderTrustTier::Authenticated,
    }
}

/// Return a test [`IncomingMessage`] containing a plain-text body.
#[must_use]
pub fn test_incoming_message() -> IncomingMessage {
    IncomingMessage {
        delivery_id: test_delivery_id(),
        conversation_id: ConversationId::new(),
        sender: test_sender(),
        body: MessageBody::Text { content: "What is Polkadot?".into() },
        received_at: Utc::now(),
    }
}

/// Return a test [`IncomingMessage`] with a specific text body.
#[must_use]
pub fn test_incoming_message_with_text(text: impl Into<String>) -> IncomingMessage {
    IncomingMessage {
        delivery_id: DeliveryId::new(format!("test-delivery-{}", Utc::now().timestamp_millis())),
        conversation_id: ConversationId::new(),
        sender: test_sender(),
        body: MessageBody::Text { content: text.into() },
        received_at: Utc::now(),
    }
}

/// Return a test [`OutgoingMessage`] containing a plain-text body.
#[must_use]
pub fn test_outgoing_message() -> OutgoingMessage {
    OutgoingMessage {
        conversation_id: ConversationId::new(),
        run_id: None,
        body: OutgoingBody::Text { content: "Polkadot is a multi-chain network.".into() },
        classification: Classification::Public,
    }
}

// ---------------------------------------------------------------------------
// Builder patterns
// ---------------------------------------------------------------------------

/// A builder for [`InferenceRequest`] that allows overriding individual fields.
///
/// # Example
///
/// ```rust
/// use polkagent_test_fixtures::InferenceRequestBuilder;
///
/// let request = InferenceRequestBuilder::new()
///     .model_id("gpt-4o")
///     .max_tokens(2048)
///     .system("You are a test assistant.")
///     .build();
/// ```
pub struct InferenceRequestBuilder {
    inner: InferenceRequest,
}

impl InferenceRequestBuilder {
    /// Create a builder pre-populated with [`test_inference_request`] defaults.
    #[must_use]
    pub fn new() -> Self {
        Self { inner: test_inference_request() }
    }

    /// Override the model identifier.
    #[must_use]
    pub fn model_id(mut self, model_id: impl Into<String>) -> Self {
        self.inner.model_id = model_id.into();
        self
    }

    /// Override the maximum output tokens.
    #[must_use]
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.inner.max_tokens = max_tokens;
        self
    }

    /// Override the system prompt.
    #[must_use]
    pub fn system(mut self, system: impl Into<String>) -> Self {
        self.inner.system = Some(system.into());
        self
    }

    /// Override the sampling temperature.
    #[must_use]
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.inner.temperature = Some(temperature);
        self
    }

    /// Add a tool definition to the request.
    #[must_use]
    pub fn with_tool(mut self, tool: ToolDefinition) -> Self {
        self.inner.tools.push(tool);
        self
    }

    /// Add a user message to the conversation history.
    #[must_use]
    pub fn user_message(mut self, content: impl Into<String>) -> Self {
        self.inner.messages.push(InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::Text { text: content.into() }],
        });
        self
    }

    /// Set an explicit `run_id`.
    #[must_use]
    pub fn run_id(mut self, run_id: RunId) -> Self {
        self.inner.run_id = run_id;
        self
    }

    /// Set an explicit `step_id`.
    #[must_use]
    pub fn step_id(mut self, step_id: StepId) -> Self {
        self.inner.step_id = step_id;
        self
    }

    /// Consume the builder and produce an [`InferenceRequest`].
    #[must_use]
    pub fn build(self) -> InferenceRequest {
        self.inner
    }
}

impl Default for InferenceRequestBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A builder for [`CanonicalSignRequest`].
///
/// # Example
///
/// ```rust
/// use polkagent_test_fixtures::{SignRequestBuilder, test_account_ref_seeded};
///
/// let request = SignRequestBuilder::new()
///     .account(test_account_ref_seeded(0x42))
///     .payload(vec![0xCA, 0xFE, 0xBA, 0xBE])
///     .build();
/// ```
pub struct SignRequestBuilder {
    inner: CanonicalSignRequest,
}

impl SignRequestBuilder {
    /// Create a builder pre-populated with [`test_sign_request`] defaults.
    #[must_use]
    pub fn new() -> Self {
        Self { inner: test_sign_request() }
    }

    /// Override the account to sign for.
    #[must_use]
    pub fn account(mut self, account: AccountRef) -> Self {
        self.inner.account = account;
        self
    }

    /// Override the SCALE-encoded payload bytes.
    #[must_use]
    pub fn payload(mut self, payload: Vec<u8>) -> Self {
        self.inner.payload = payload;
        self
    }

    /// Override the request ID.
    #[must_use]
    pub fn request_id(mut self, request_id: impl Into<String>) -> Self {
        self.inner.request_id = request_id.into();
        self
    }

    /// Override the expiry timestamp.
    #[must_use]
    pub fn expires_at(mut self, expires_at: polkagent_core::Timestamp) -> Self {
        self.inner.expires_at = expires_at;
        self
    }

    /// Consume the builder and produce a [`CanonicalSignRequest`].
    #[must_use]
    pub fn build(self) -> CanonicalSignRequest {
        self.inner
    }
}

impl Default for SignRequestBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A builder for [`OutgoingMessage`].
pub struct OutgoingMessageBuilder {
    inner: OutgoingMessage,
}

impl OutgoingMessageBuilder {
    /// Create a builder pre-populated with [`test_outgoing_message`] defaults.
    #[must_use]
    pub fn new() -> Self {
        Self { inner: test_outgoing_message() }
    }

    /// Override the conversation ID.
    #[must_use]
    pub fn conversation_id(mut self, id: ConversationId) -> Self {
        self.inner.conversation_id = id;
        self
    }

    /// Override the content classification.
    #[must_use]
    pub fn classification(mut self, cls: Classification) -> Self {
        self.inner.classification = cls;
        self
    }

    /// Override the text body content.
    #[must_use]
    pub fn text(mut self, content: impl Into<String>) -> Self {
        self.inner.body = OutgoingBody::Text { content: content.into() };
        self
    }

    /// Consume the builder and produce an [`OutgoingMessage`].
    #[must_use]
    pub fn build(self) -> OutgoingMessage {
        self.inner
    }
}

impl Default for OutgoingMessageBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_id_is_unique() {
        let a = test_run_id();
        let b = test_run_id();
        assert_ne!(a, b);
    }

    #[test]
    fn test_step_id_is_unique() {
        let a = test_step_id();
        let b = test_step_id();
        assert_ne!(a, b);
    }

    #[test]
    fn test_conversation_id_is_unique() {
        let a = test_conversation_id();
        let b = test_conversation_id();
        assert_ne!(a, b);
    }

    #[test]
    fn test_inference_request_has_user_message() {
        let req = test_inference_request();
        assert_eq!(req.messages.len(), 1);
        assert!(matches!(req.messages[0].role, MessageRole::User));
        assert_eq!(req.model_id, "claude-opus-4-6");
        assert_eq!(req.max_tokens, 1024);
    }

    #[test]
    fn test_inference_response_has_end_turn() {
        let resp = test_inference_response();
        assert_eq!(resp.stop_reason, "end_turn");
        assert!(!resp.text.is_empty());
    }

    #[test]
    fn test_tool_definition_has_valid_schema() {
        let tool = test_tool_definition();
        assert_eq!(tool.name, "polkagent.file.read");
        let schema: serde_json::Value =
            serde_json::from_str(&tool.input_schema_json).expect("valid JSON schema");
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn test_tool_call_has_valid_arguments() {
        let call = test_tool_call();
        let args: serde_json::Value =
            serde_json::from_str(&call.arguments_json).expect("valid JSON");
        assert_eq!(args["path"], "/tmp/test.txt");
    }

    #[test]
    fn test_account_ref_seeded_differs_from_default() {
        let default = test_account_ref();
        let seeded = test_account_ref_seeded(0x42);
        assert_ne!(default.account_id, seeded.account_id);
        assert_eq!(seeded.account_id, [0x42u8; 32]);
    }

    #[test]
    fn test_sign_request_expiry_is_future() {
        let req = test_sign_request();
        assert!(req.expires_at > Utc::now());
    }

    #[test]
    fn test_sign_request_payload_is_nonempty() {
        let req = test_sign_request();
        assert!(!req.payload.is_empty());
    }

    #[test]
    fn test_incoming_message_sender_is_alice() {
        let msg = test_incoming_message();
        assert_eq!(msg.sender.user_id.0, "test-user-alice");
        assert_eq!(msg.sender.display_name.as_deref(), Some("Alice"));
    }

    #[test]
    fn test_incoming_message_with_text_sets_body() {
        let msg = test_incoming_message_with_text("custom body");
        if let MessageBody::Text { content } = &msg.body {
            assert_eq!(content, "custom body");
        } else {
            panic!("expected Text body");
        }
    }

    #[test]
    fn inference_request_builder_overrides_model_id() {
        let req = InferenceRequestBuilder::new().model_id("gpt-4o").build();
        assert_eq!(req.model_id, "gpt-4o");
    }

    #[test]
    fn inference_request_builder_overrides_max_tokens() {
        let req = InferenceRequestBuilder::new().max_tokens(512).build();
        assert_eq!(req.max_tokens, 512);
    }

    #[test]
    fn inference_request_builder_adds_system_prompt() {
        let req = InferenceRequestBuilder::new().system("You are a Polkadot expert.").build();
        assert_eq!(req.system.as_deref(), Some("You are a Polkadot expert."));
    }

    #[test]
    fn inference_request_builder_appends_user_message() {
        let req = InferenceRequestBuilder::new().user_message("second question").build();
        assert_eq!(req.messages.len(), 2);
    }

    #[test]
    fn inference_request_builder_adds_tool() {
        let tool = test_tool_definition();
        let req = InferenceRequestBuilder::new().with_tool(tool).build();
        assert_eq!(req.tools.len(), 1);
    }

    #[test]
    fn sign_request_builder_overrides_payload() {
        let req = SignRequestBuilder::new().payload(vec![0xCA, 0xFE]).build();
        assert_eq!(req.payload, vec![0xCA, 0xFE]);
    }

    #[test]
    fn sign_request_builder_overrides_account() {
        let acct = test_account_ref_seeded(0x77);
        let req = SignRequestBuilder::new().account(acct.clone()).build();
        assert_eq!(req.account.account_id, acct.account_id);
    }

    #[test]
    fn outgoing_message_builder_overrides_text() {
        let msg = OutgoingMessageBuilder::new().text("custom response").build();
        if let OutgoingBody::Text { content } = &msg.body {
            assert_eq!(content, "custom response");
        } else {
            panic!("expected Text body");
        }
    }

    #[test]
    fn outgoing_message_builder_overrides_classification() {
        let msg = OutgoingMessageBuilder::new().classification(Classification::Sensitive).build();
        assert_eq!(msg.classification, Classification::Sensitive);
    }

    #[test]
    fn signer_capabilities_fixture_has_one_account() {
        let caps = test_signer_capabilities();
        assert_eq!(caps.accounts.len(), 1);
        assert!(!caps.hardware_backed);
        assert_eq!(caps.display_name, "TestSigner");
    }

    #[test]
    fn chain_profile_id_fixture_is_polkadot() {
        let id = test_chain_profile_id();
        assert_eq!(id.0, "polkadot");
    }
}

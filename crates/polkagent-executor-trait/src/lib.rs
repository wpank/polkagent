//! Model executor port trait for the Polkagent platform.
//!
//! This crate defines the [`ModelExecutor`] trait — the narrow adapter
//! boundary between the Polkagent kernel and any concrete AI model provider.
//! All provider-specific encoding, authentication, and wire-format translation
//! lives in leaf adapter crates; the kernel depends only on this trait.
//!
//! # Contract
//!
//! - Implementations must be `Send + Sync + 'static`.
//! - [`ModelExecutor::complete`] blocks until the full response is received.
//! - [`ModelExecutor::complete`] should be idempotent for the same
//!   `step_id` within an [`InferenceRequest`]; implementations may use
//!   `step_id` as an idempotency hint to de-duplicate retried requests.
//! - Implementations must **never** log raw prompt content at verbosity
//!   levels that reach telemetry exporters. Only token counts and request
//!   metadata are safe to log.
//! - No implementation of this trait may place raw key material, wallet
//!   seeds, or `SecretForbidden`-classified data into any field of
//!   [`InferenceRequest`].

#[cfg(feature = "test-contracts")]
pub mod contracts;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use polkagent_core::{RunId, StepId};

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// A single message in an inference conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceMessage {
    /// The sender role for this message.
    pub role: MessageRole,
    /// Ordered content blocks that make up this message.
    pub content: Vec<ContentBlock>,
}

/// The role of the sender in a conversation turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// A human or ingress-layer message.
    User,
    /// A model-generated message.
    Assistant,
}

/// A typed block of content within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text content.
    Text { text: String },
    /// A tool result returned to the model after a previous tool call.
    ToolResult {
        /// The tool call this result answers.
        tool_call_id: String,
        /// Serialized result content.
        content: String,
        /// Whether the tool invocation produced an error.
        is_error: bool,
    },
    /// A tool use request recorded in assistant history.
    ToolUse {
        /// Unique identifier for this tool call.
        tool_call_id: String,
        /// The tool that was (or is being) called.
        tool_name: String,
        /// JSON-serialized arguments.
        arguments_json: String,
    },
}

/// A JSON-Schema description of a tool the model may call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique, namespaced tool identifier presented to the model.
    ///
    /// Examples: `"polkagent.file.read"`, `"polkagent.chain.decode"`.
    pub name: String,
    /// Human-readable description sent to the model.
    pub description: String,
    /// JSON Schema (serialized as a string) for the tool's input parameters.
    pub input_schema_json: String,
}

/// A complete, provider-agnostic inference request.
///
/// The adapter translates this into the wire format required by its model
/// provider. No provider-specific types appear here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceRequest {
    /// The run this inference belongs to.
    pub run_id: RunId,
    /// The execution step this inference belongs to.
    ///
    /// Used as an idempotency hint: the provider may de-duplicate requests
    /// that share the same `step_id`.
    pub step_id: StepId,
    /// Assembled conversation history including the current user turn.
    pub messages: Vec<InferenceMessage>,
    /// System prompt assembled by the context layer.
    ///
    /// `None` means no system prompt is sent.
    pub system: Option<String>,
    /// Tool schemas available for this inference call.
    pub tools: Vec<ToolDefinition>,
    /// The model identifier in a provider-agnostic format.
    ///
    /// Examples: `"claude-opus-4-6"`, `"gpt-4o"`.
    pub model_id: String,
    /// Maximum output tokens for this request.
    pub max_tokens: u32,
    /// Sampling temperature in `[0.0, 2.0]`.
    ///
    /// `None` defers to the provider/model default.
    pub temperature: Option<f32>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// A tool call request produced by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned unique identifier for this tool call.
    pub tool_call_id: String,
    /// Name of the tool the model wants to invoke.
    pub tool_name: String,
    /// JSON-serialized arguments provided by the model.
    pub arguments_json: String,
}

/// Token usage breakdown for one inference call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input (prompt) tokens consumed.
    pub input_tokens: u32,
    /// Output (completion) tokens produced.
    pub output_tokens: u32,
    /// Prompt tokens read from provider cache (if applicable).
    pub cache_read_tokens: Option<u32>,
    /// Prompt tokens written to provider cache (if applicable).
    pub cache_write_tokens: Option<u32>,
}

/// The complete, non-streaming result of one inference call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceResponse {
    /// Full text content of the response.
    ///
    /// May be empty when the model only produced tool calls.
    pub text: String,
    /// Tool calls requested by the model, if any.
    pub tool_calls: Vec<ToolCall>,
    /// Why the model stopped generating.
    ///
    /// Known values: `"end_turn"`, `"max_tokens"`, `"tool_use"`,
    /// `"stop_sequence"`.
    pub stop_reason: String,
    /// Token accounting for billing and budget enforcement.
    pub usage: TokenUsage,
    /// Stable provider-assigned request identifier for audit / idempotency.
    pub provider_request_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Streaming event types
// ---------------------------------------------------------------------------

/// A single streaming event emitted by [`ModelExecutor::stream`].
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A text chunk has been generated.
    TextDelta {
        /// The incremental text content.
        delta: String,
    },
    /// A tool call is being assembled (partial JSON input delta).
    ToolCallDelta {
        /// Provider-assigned tool call identifier.
        tool_call_id: String,
        /// Tool name (present only on first delta for a given call).
        name: Option<String>,
        /// Incremental JSON fragment of the tool's input arguments.
        input_delta: String,
    },
    /// A complete tool call has been assembled.
    ToolCallComplete {
        /// The fully assembled tool call.
        call: ToolCall,
    },
    /// Partial or final token usage update.
    UsageUpdate {
        /// Current usage totals (may be intermediate during streaming).
        usage: TokenUsage,
    },
    /// The stream has completed; this is always the last event.
    Completed {
        /// The terminal result matching what [`ModelExecutor::complete`]
        /// would return for the same request.
        result: InferenceResponse,
    },
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that a [`ModelExecutor`] implementation may return.
#[derive(Debug, Error)]
pub enum ExecutorError {
    /// Provider endpoint is unreachable or returned a transport-level error.
    #[error("transport error: {message} (retryable: {retryable})")]
    Transport {
        /// Human-readable description.
        message: String,
        /// Whether the outbox worker should schedule a retry.
        retryable: bool,
    },

    /// API credentials are invalid or expired.
    #[error("authentication failed: {message}")]
    Authentication {
        /// Human-readable description.
        message: String,
    },

    /// The provider's rate limit has been hit.
    #[error("rate limit exceeded; retry after {retry_after_secs:?} seconds")]
    RateLimit {
        /// Number of seconds to wait before retrying, if known.
        retry_after_secs: Option<u64>,
    },

    /// The assembled context exceeds the model's context window.
    #[error("context window exceeded: {tokens_requested} tokens > {tokens_allowed} allowed")]
    ContextWindowExceeded {
        /// Number of tokens in the request.
        tokens_requested: u32,
        /// Maximum tokens the model accepts.
        tokens_allowed: u32,
    },

    /// The provider returned a response that could not be parsed.
    #[error("invalid response from provider: {message}")]
    InvalidResponse {
        /// Human-readable description.
        message: String,
    },

    /// The request timed out.
    #[error("request timed out after {elapsed_ms}ms")]
    Timeout {
        /// How long the request waited before timing out.
        elapsed_ms: u64,
    },

    /// The execution was cancelled.
    #[error("execution cancelled")]
    Cancelled,

    /// An unexpected internal error.
    #[error("internal executor error: {message}")]
    Internal {
        /// Human-readable description.
        message: String,
    },
}

impl ExecutorError {
    /// Returns `true` if the outbox worker should schedule an automatic retry.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimit { .. }
                | Self::Timeout { .. }
                | Self::Transport { retryable: true, .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Trait definition
// ---------------------------------------------------------------------------

/// The narrow model executor port.
///
/// One implementation exists per AI provider (Anthropic, OpenAI-compatible,
/// local, gateway). The kernel depends only on this trait; all
/// provider-specific details are confined to adapter crates.
///
/// # Contract
///
/// - Implementations must be `Send + Sync + 'static`.
/// - [`complete`] blocks until the full response is available.
/// - [`stream`] returns an ordered stream of [`StreamEvent`]s, always
///   terminated by a [`StreamEvent::Completed`] variant.
/// - Neither method must log raw prompt content at telemetry-visible log
///   levels. Token counts and request metadata are safe to log.
///
/// [`complete`]: ModelExecutor::complete
/// [`stream`]: ModelExecutor::stream
#[async_trait]
pub trait ModelExecutor: Send + Sync + 'static {
    /// Execute one inference call and return the complete response.
    ///
    /// The `step_id` within `request` may be used as an idempotency hint.
    async fn complete(
        &self,
        request: InferenceRequest,
    ) -> Result<InferenceResponse, ExecutorError>;

    /// Execute one inference call and return a stream of events.
    ///
    /// The stream always ends with [`StreamEvent::Completed`], even on
    /// cancellation (in which case the completed result reflects partial
    /// content). Implementations that do not support streaming may
    /// collect the full response and yield it as a single
    /// [`StreamEvent::Completed`].
    async fn stream(
        &self,
        request: InferenceRequest,
    ) -> Result<
        Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
        ExecutorError,
    >;

    /// Health check: return `Ok(())` if the provider is reachable.
    async fn health(&self) -> Result<(), ExecutorError>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_error_rate_limit_is_retryable() {
        let e = ExecutorError::RateLimit { retry_after_secs: Some(30) };
        assert!(e.is_retryable());
    }

    #[test]
    fn executor_error_authentication_not_retryable() {
        let e = ExecutorError::Authentication { message: "invalid key".into() };
        assert!(!e.is_retryable());
    }

    #[test]
    fn executor_error_timeout_is_retryable() {
        let e = ExecutorError::Timeout { elapsed_ms: 5000 };
        assert!(e.is_retryable());
    }

    #[test]
    fn executor_error_cancelled_not_retryable() {
        let e = ExecutorError::Cancelled;
        assert!(!e.is_retryable());
    }

    #[test]
    fn inference_request_serializes() {
        let req = InferenceRequest {
            run_id: polkagent_core::RunId::new(),
            step_id: polkagent_core::StepId::new(),
            messages: vec![InferenceMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::Text { text: "hello".into() }],
            }],
            system: Some("You are a helpful assistant.".into()),
            tools: vec![],
            model_id: "claude-opus-4-6".into(),
            max_tokens: 1024,
            temperature: Some(0.7),
        };
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(json.contains("claude-opus-4-6"));
        assert!(json.contains("hello"));
    }

    #[test]
    fn token_usage_default_is_zero() {
        let usage = TokenUsage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
    }

    #[test]
    fn inference_response_serializes() {
        let resp = InferenceResponse {
            text: "result".into(),
            tool_calls: vec![],
            stop_reason: "end_turn".into(),
            usage: TokenUsage { input_tokens: 10, output_tokens: 5, ..Default::default() },
            provider_request_id: Some("req-123".into()),
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        assert!(json.contains("end_turn"));
        assert!(json.contains("req-123"));
    }

    #[test]
    fn message_role_serde_lowercase() {
        let role = MessageRole::User;
        let json = serde_json::to_string(&role).expect("serialize");
        assert_eq!(json, r#""user""#);
    }

    /// Compile-time check: `ModelExecutor` can be used as a `dyn` trait object.
    #[allow(dead_code)]
    fn _model_executor_is_object_safe(_e: &dyn ModelExecutor) {}
}

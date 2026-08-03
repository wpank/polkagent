//! Anthropic Messages API adapter for the [`ModelExecutor`] trait.
//!
//! This crate provides [`AnthropicExecutor`] -- a production adapter that
//! translates the provider-agnostic [`InferenceRequest`] into the Anthropic
//! Messages API wire format, handles authentication, rate-limit retries with
//! exponential backoff, and maps responses back to [`InferenceResponse`].
//!
//! # Quick start
//!
//! ```rust,no_run
//! use polkagent_executor_anthropic::AnthropicExecutor;
//!
//! let executor = AnthropicExecutor::new(
//!     "sk-ant-...".to_string(),
//!     "claude-sonnet-4-20250514".to_string(),
//! );
//! ```

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, Stream};
use polkagent_executor_trait::{
    ContentBlock, ExecutorError, InferenceMessage, InferenceRequest, InferenceResponse,
    MessageRole, ModelExecutor, StreamEvent, TokenUsage, ToolCall, ToolDefinition,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default Anthropic API base URL.
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// Default request timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Default maximum number of retries for retryable errors.
const DEFAULT_MAX_RETRIES: u32 = 3;

/// Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Base delay for exponential backoff in milliseconds.
const BACKOFF_BASE_MS: u64 = 500;

// ---------------------------------------------------------------------------
// Anthropic API types (private)
// ---------------------------------------------------------------------------

/// Request body for the Anthropic Messages API.
#[derive(Debug, Clone, Serialize)]
struct CreateMessageRequest {
    model: String,
    messages: Vec<ApiMessage>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

/// A message in the Anthropic conversation format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiMessage {
    role: String,
    content: Vec<ApiContentBlock>,
}

/// A content block in the Anthropic wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum ApiContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

/// A tool definition in the Anthropic wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiToolDefinition {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

/// Top-level response from the Anthropic Messages API.
#[derive(Debug, Clone, Deserialize)]
struct MessageResponse {
    id: String,
    content: Vec<ApiContentBlock>,
    #[allow(dead_code)]
    model: String,
    stop_reason: Option<String>,
    usage: ApiUsage,
}

/// Token usage from the Anthropic API response.
#[derive(Debug, Clone, Deserialize)]
struct ApiUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
}

/// Error response body from the Anthropic API.
#[derive(Debug, Clone, Deserialize)]
struct ApiErrorResponse {
    error: ApiErrorDetail,
}

/// Inner error detail from the Anthropic API.
#[derive(Debug, Clone, Deserialize)]
struct ApiErrorDetail {
    #[allow(dead_code)]
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

// ---------------------------------------------------------------------------
// SSE event types for streaming
// ---------------------------------------------------------------------------

/// Raw SSE event types from the Anthropic streaming API.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
enum SseEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: MessageStartData },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        #[allow(dead_code)]
        index: usize,
        content_block: ApiContentBlock,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        #[allow(dead_code)]
        index: usize,
        delta: DeltaBlock,
    },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop {
        #[allow(dead_code)]
        index: usize,
    },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaData,
        usage: Option<DeltaUsage>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
}

#[derive(Debug, Clone, Deserialize)]
struct MessageStartData {
    id: String,
    #[allow(dead_code)]
    model: String,
    usage: ApiUsage,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
enum DeltaBlock {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Clone, Deserialize)]
struct MessageDeltaData {
    stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeltaUsage {
    output_tokens: u32,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a provider-agnostic `InferenceMessage` to the Anthropic wire format.
fn to_api_message(msg: &InferenceMessage) -> ApiMessage {
    let role = match msg.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    };

    let content = msg
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => ApiContentBlock::Text { text: text.clone() },
            ContentBlock::ToolUse {
                tool_call_id,
                tool_name,
                arguments_json,
            } => {
                let input: serde_json::Value =
                    serde_json::from_str(arguments_json).unwrap_or(serde_json::Value::Object(
                        serde_json::Map::new(),
                    ));
                ApiContentBlock::ToolUse {
                    id: tool_call_id.clone(),
                    name: tool_name.clone(),
                    input,
                }
            }
            ContentBlock::ToolResult {
                tool_call_id,
                content,
                is_error,
            } => ApiContentBlock::ToolResult {
                tool_use_id: tool_call_id.clone(),
                content: content.clone(),
                is_error: if *is_error { Some(true) } else { None },
            },
        })
        .collect();

    ApiMessage {
        role: role.to_string(),
        content,
    }
}

/// Convert a provider-agnostic `ToolDefinition` to the Anthropic wire format.
fn to_api_tool(tool: &ToolDefinition) -> ApiToolDefinition {
    let input_schema: serde_json::Value = serde_json::from_str(&tool.input_schema_json)
        .unwrap_or_else(|_| {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
        });

    ApiToolDefinition {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema,
    }
}

/// Build the API request body from an `InferenceRequest`.
fn build_request_body(request: &InferenceRequest, stream: bool) -> CreateMessageRequest {
    CreateMessageRequest {
        model: request.model_id.clone(),
        messages: request.messages.iter().map(to_api_message).collect(),
        max_tokens: request.max_tokens,
        system: request.system.clone(),
        tools: request.tools.iter().map(to_api_tool).collect(),
        stream: if stream { Some(true) } else { None },
        temperature: request.temperature,
    }
}

/// Convert an `ApiUsage` to the trait-level `TokenUsage`.
fn to_token_usage(api_usage: &ApiUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: api_usage.input_tokens,
        output_tokens: api_usage.output_tokens,
        cache_read_tokens: api_usage.cache_read_input_tokens,
        cache_write_tokens: api_usage.cache_creation_input_tokens,
    }
}

/// Convert a `MessageResponse` to the trait-level `InferenceResponse`.
fn to_inference_response(resp: &MessageResponse) -> InferenceResponse {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    for block in &resp.content {
        match block {
            ApiContentBlock::Text { text } => text_parts.push(text.clone()),
            ApiContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(ToolCall {
                    tool_call_id: id.clone(),
                    tool_name: name.clone(),
                    arguments_json: serde_json::to_string(input).unwrap_or_default(),
                });
            }
            ApiContentBlock::ToolResult { .. } => {
                // Tool results should not appear in assistant responses.
            }
        }
    }

    InferenceResponse {
        text: text_parts.join(""),
        tool_calls,
        stop_reason: resp.stop_reason.clone().unwrap_or_else(|| "end_turn".to_string()),
        usage: to_token_usage(&resp.usage),
        provider_request_id: Some(resp.id.clone()),
    }
}

/// Map an HTTP status code and optional error body to an `ExecutorError`.
fn map_api_error(status: u16, body: &str) -> ExecutorError {
    let detail = serde_json::from_str::<ApiErrorResponse>(body)
        .map(|e| e.error.message)
        .unwrap_or_else(|_| body.to_string());

    match status {
        401 => ExecutorError::Authentication { message: detail },
        429 => {
            // Try to extract retry-after hint from the error message.
            ExecutorError::RateLimit {
                retry_after_secs: None,
            }
        }
        400 => {
            // Check for context window errors.
            if body.contains("context_length_exceeded") || body.contains("too many tokens") {
                ExecutorError::ContextWindowExceeded {
                    tokens_requested: 0,
                    tokens_allowed: 0,
                }
            } else {
                ExecutorError::Transport {
                    message: format!("bad request: {detail}"),
                    retryable: false,
                }
            }
        }
        403 => ExecutorError::Authentication {
            message: format!("forbidden: {detail}"),
        },
        status if status >= 500 => ExecutorError::Transport {
            message: format!("server error ({status}): {detail}"),
            retryable: true,
        },
        _ => ExecutorError::Transport {
            message: format!("unexpected status ({status}): {detail}"),
            retryable: false,
        },
    }
}

/// Parse SSE lines from a response body chunk.
///
/// Returns a list of parsed `SseEvent` values. Lines that are not valid
/// `data:` lines or cannot be parsed are silently skipped.
fn parse_sse_events(chunk: &str) -> Vec<SseEvent> {
    let mut events = Vec::new();
    for line in chunk.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                continue;
            }
            match serde_json::from_str::<SseEvent>(data) {
                Ok(event) => events.push(event),
                Err(e) => {
                    debug!(data = data, error = %e, "skipping unparseable SSE data line");
                }
            }
        }
    }
    events
}

/// Process raw SSE events into a list of `StreamEvent` values.
///
/// This function handles text deltas, tool call assembly from partial JSON
/// deltas, usage tracking, and produces the terminal `Completed` event.
fn process_sse_events(sse_events: Vec<SseEvent>) -> Vec<Result<StreamEvent, ExecutorError>> {
    let mut stream_events: Vec<Result<StreamEvent, ExecutorError>> = Vec::new();
    let mut accumulated_text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut current_tool_id: Option<String> = None;
    let mut current_tool_name: Option<String> = None;
    let mut current_tool_input = String::new();
    let mut usage = TokenUsage::default();
    let mut stop_reason = String::from("end_turn");
    let mut provider_request_id: Option<String> = None;

    for event in sse_events {
        match event {
            SseEvent::MessageStart { message } => {
                provider_request_id = Some(message.id);
                usage = to_token_usage(&message.usage);
            }
            SseEvent::ContentBlockStart {
                content_block, ..
            } => match content_block {
                ApiContentBlock::ToolUse { id, name, .. } => {
                    // Finish previous tool if any.
                    if let Some(prev_id) = current_tool_id.take() {
                        let call = ToolCall {
                            tool_call_id: prev_id,
                            tool_name: current_tool_name
                                .take()
                                .unwrap_or_default(),
                            arguments_json: current_tool_input.clone(),
                        };
                        stream_events
                            .push(Ok(StreamEvent::ToolCallComplete { call: call.clone() }));
                        tool_calls.push(call);
                        current_tool_input.clear();
                    }

                    current_tool_id = Some(id.clone());
                    current_tool_name = Some(name.clone());
                    current_tool_input.clear();

                    stream_events.push(Ok(StreamEvent::ToolCallDelta {
                        tool_call_id: id,
                        name: Some(name),
                        input_delta: String::new(),
                    }));
                }
                ApiContentBlock::Text { .. } => {
                    // Text block started; deltas will follow.
                }
                ApiContentBlock::ToolResult { .. } => {
                    // Should not appear in streaming responses.
                }
            },
            SseEvent::ContentBlockDelta { delta, .. } => match delta {
                DeltaBlock::TextDelta { text } => {
                    accumulated_text.push_str(&text);
                    stream_events.push(Ok(StreamEvent::TextDelta { delta: text }));
                }
                DeltaBlock::InputJsonDelta { partial_json } => {
                    current_tool_input.push_str(&partial_json);
                    stream_events.push(Ok(StreamEvent::ToolCallDelta {
                        tool_call_id: current_tool_id
                            .clone()
                            .unwrap_or_default(),
                        name: None,
                        input_delta: partial_json,
                    }));
                }
            },
            SseEvent::ContentBlockStop { .. } => {
                // If we have a pending tool call, finalize it.
                if let Some(tool_id) = current_tool_id.take() {
                    let call = ToolCall {
                        tool_call_id: tool_id,
                        tool_name: current_tool_name.take().unwrap_or_default(),
                        arguments_json: current_tool_input.clone(),
                    };
                    stream_events
                        .push(Ok(StreamEvent::ToolCallComplete { call: call.clone() }));
                    tool_calls.push(call);
                    current_tool_input.clear();
                }
            }
            SseEvent::MessageDelta {
                delta,
                usage: delta_usage,
            } => {
                if let Some(reason) = delta.stop_reason {
                    stop_reason = reason;
                }
                if let Some(du) = delta_usage {
                    usage.output_tokens = du.output_tokens;
                }
            }
            SseEvent::MessageStop | SseEvent::Ping => {
                // No action needed.
            }
        }
    }

    // Emit usage update.
    stream_events.push(Ok(StreamEvent::UsageUpdate {
        usage: usage.clone(),
    }));

    // Emit the terminal Completed event.
    let response = InferenceResponse {
        text: accumulated_text,
        tool_calls,
        stop_reason,
        usage,
        provider_request_id,
    };
    stream_events.push(Ok(StreamEvent::Completed { result: response }));

    stream_events
}

// ---------------------------------------------------------------------------
// AnthropicExecutor
// ---------------------------------------------------------------------------

/// A [`ModelExecutor`] adapter for the Anthropic Messages API.
///
/// Handles authentication, request/response mapping, rate-limit retries with
/// exponential backoff, and streaming via SSE.
pub struct AnthropicExecutor {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
    max_retries: u32,
    concurrency_semaphore: Option<Arc<tokio::sync::Semaphore>>,
}

impl AnthropicExecutor {
    /// Create a new `AnthropicExecutor` with the given API key and model.
    ///
    /// Uses the default Anthropic API base URL, a 120-second timeout, and
    /// up to 3 retries for retryable errors.
    pub fn new(api_key: String, model: String) -> Arc<Self> {
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());

        Arc::new(Self {
            client,
            api_key,
            model,
            base_url: DEFAULT_BASE_URL.to_string(),
            max_retries: DEFAULT_MAX_RETRIES,
            concurrency_semaphore: None,
        })
    }

    /// Override the base URL (for testing or proxies).
    #[must_use]
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Override the maximum number of retries for retryable errors.
    #[must_use]
    pub fn with_max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    /// Override the HTTP request timeout.
    #[must_use]
    pub fn with_timeout(mut self, duration: Duration) -> Self {
        self.client = Client::builder()
            .timeout(duration)
            .build()
            .unwrap_or_else(|_| Client::new());
        self
    }

    /// Set the maximum number of concurrent requests to the provider.
    ///
    /// When set, a [`tokio::sync::Semaphore`] is used to limit concurrency
    /// in [`complete`](ModelExecutor::complete) and
    /// [`stream`](ModelExecutor::stream).
    #[must_use]
    pub fn with_max_concurrent(mut self, n: u32) -> Self {
        self.concurrency_semaphore = Some(Arc::new(tokio::sync::Semaphore::new(n as usize)));
        self
    }

    /// Build an `Arc<Self>` after applying builder methods.
    pub fn build(self) -> Arc<Self> {
        Arc::new(self)
    }

    /// Construct a raw (non-Arc) executor for builder chaining.
    pub fn new_builder(api_key: String, model: String) -> Self {
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            api_key,
            model,
            base_url: DEFAULT_BASE_URL.to_string(),
            max_retries: DEFAULT_MAX_RETRIES,
            concurrency_semaphore: None,
        }
    }

    /// Execute the HTTP request with retry logic for retryable errors.
    async fn execute_with_retries(
        &self,
        body: &CreateMessageRequest,
    ) -> Result<MessageResponse, ExecutorError> {
        let url = format!("{}/v1/messages", self.base_url);
        let mut last_error: Option<ExecutorError> = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay_ms = BACKOFF_BASE_MS * 2u64.pow(attempt - 1);
                debug!(attempt, delay_ms, "retrying after backoff");
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            let result = self
                .client
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(body)
                .send()
                .await;

            let response = match result {
                Ok(resp) => resp,
                Err(e) => {
                    if e.is_timeout() {
                        let err = ExecutorError::Timeout {
                            elapsed_ms: DEFAULT_TIMEOUT.as_millis() as u64,
                        };
                        if attempt < self.max_retries {
                            warn!(attempt, "request timed out, will retry");
                            last_error = Some(err);
                            continue;
                        }
                        return Err(err);
                    }
                    let err = ExecutorError::Transport {
                        message: format!("HTTP transport error: {e}"),
                        retryable: true,
                    };
                    if attempt < self.max_retries {
                        warn!(attempt, error = %e, "transport error, will retry");
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
            };

            let status = response.status().as_u16();
            if status == 200 {
                let response_body = response.text().await.map_err(|e| {
                    ExecutorError::InvalidResponse {
                        message: format!("failed to read response body: {e}"),
                    }
                })?;

                let parsed: MessageResponse =
                    serde_json::from_str(&response_body).map_err(|e| {
                        ExecutorError::InvalidResponse {
                            message: format!("failed to parse response JSON: {e}"),
                        }
                    })?;

                return Ok(parsed);
            }

            let error_body = response.text().await.unwrap_or_default();
            let error = map_api_error(status, &error_body);

            if error.is_retryable() && attempt < self.max_retries {
                warn!(attempt, status, "retryable API error, will retry");
                last_error = Some(error);
                continue;
            }

            return Err(error);
        }

        Err(last_error.unwrap_or_else(|| ExecutorError::Internal {
            message: "retry loop exhausted without error".to_string(),
        }))
    }

    /// Execute a streaming request and collect SSE events into `StreamEvent`s.
    async fn execute_streaming(
        &self,
        body: &CreateMessageRequest,
    ) -> Result<Vec<Result<StreamEvent, ExecutorError>>, ExecutorError> {
        let url = format!("{}/v1/messages", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ExecutorError::Timeout {
                        elapsed_ms: DEFAULT_TIMEOUT.as_millis() as u64,
                    }
                } else {
                    ExecutorError::Transport {
                        message: format!("HTTP transport error: {e}"),
                        retryable: true,
                    }
                }
            })?;

        let status = response.status().as_u16();
        if status != 200 {
            let error_body = response.text().await.unwrap_or_default();
            return Err(map_api_error(status, &error_body));
        }

        let full_body = response.text().await.map_err(|e| {
            ExecutorError::InvalidResponse {
                message: format!("failed to read SSE stream body: {e}"),
            }
        })?;

        let sse_events = parse_sse_events(&full_body);
        Ok(process_sse_events(sse_events))
    }
}

// ---------------------------------------------------------------------------
// ModelExecutor implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ModelExecutor for AnthropicExecutor {
    async fn complete(
        &self,
        request: InferenceRequest,
    ) -> Result<InferenceResponse, ExecutorError> {
        debug!(
            model = %self.model,
            run_id = %request.run_id,
            step_id = %request.step_id,
            message_count = request.messages.len(),
            tool_count = request.tools.len(),
            "executing anthropic inference"
        );

        let _permit = match &self.concurrency_semaphore {
            Some(sem) => Some(sem.acquire().await.map_err(|_| ExecutorError::Transport {
                message: "concurrency semaphore closed".to_string(),
                retryable: false,
            })?),
            None => None,
        };

        let body = build_request_body(&request, false);
        let api_response = self.execute_with_retries(&body).await?;
        let response = to_inference_response(&api_response);

        debug!(
            input_tokens = response.usage.input_tokens,
            output_tokens = response.usage.output_tokens,
            stop_reason = %response.stop_reason,
            "inference complete"
        );

        Ok(response)
    }

    async fn stream(
        &self,
        request: InferenceRequest,
    ) -> Result<
        Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
        ExecutorError,
    > {
        debug!(
            model = %self.model,
            run_id = %request.run_id,
            step_id = %request.step_id,
            "starting anthropic streaming inference"
        );

        let _permit = match &self.concurrency_semaphore {
            Some(sem) => Some(sem.acquire().await.map_err(|_| ExecutorError::Transport {
                message: "concurrency semaphore closed".to_string(),
                retryable: false,
            })?),
            None => None,
        };

        let body = build_request_body(&request, true);
        let events = self.execute_streaming(&body).await?;

        Ok(Box::new(stream::iter(events)))
    }

    async fn health(&self) -> Result<(), ExecutorError> {
        // Perform a minimal request to verify connectivity and auth.
        // We send a tiny message with max_tokens=1 to minimize cost.
        let body = CreateMessageRequest {
            model: self.model.clone(),
            messages: vec![ApiMessage {
                role: "user".to_string(),
                content: vec![ApiContentBlock::Text {
                    text: "ping".to_string(),
                }],
            }],
            max_tokens: 1,
            system: None,
            tools: vec![],
            stream: None,
            temperature: Some(0.0),
        };

        let url = format!("{}/v1/messages", self.base_url);
        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ExecutorError::Transport {
                message: format!("health check failed: {e}"),
                retryable: true,
            })?;

        let status = response.status().as_u16();
        if status == 200 {
            Ok(())
        } else {
            let error_body = response.text().await.unwrap_or_default();
            Err(map_api_error(status, &error_body))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::{RunId, StepId};

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn minimal_request() -> InferenceRequest {
        InferenceRequest {
            run_id: RunId::new(),
            step_id: StepId::new(),
            messages: vec![InferenceMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            }],
            system: Some("You are a helpful assistant.".into()),
            tools: vec![],
            model_id: "claude-sonnet-4-20250514".into(),
            max_tokens: 1024,
            temperature: Some(0.7),
        }
    }

    fn request_with_tools() -> InferenceRequest {
        InferenceRequest {
            run_id: RunId::new(),
            step_id: StepId::new(),
            messages: vec![InferenceMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: "read the file".into(),
                }],
            }],
            system: None,
            tools: vec![ToolDefinition {
                name: "file_read".to_string(),
                description: "Read a file from disk".to_string(),
                input_schema_json: r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#.to_string(),
            }],
            model_id: "claude-sonnet-4-20250514".into(),
            max_tokens: 4096,
            temperature: None,
        }
    }

    fn request_with_tool_result() -> InferenceRequest {
        InferenceRequest {
            run_id: RunId::new(),
            step_id: StepId::new(),
            messages: vec![
                InferenceMessage {
                    role: MessageRole::User,
                    content: vec![ContentBlock::Text {
                        text: "read the file".into(),
                    }],
                },
                InferenceMessage {
                    role: MessageRole::Assistant,
                    content: vec![ContentBlock::ToolUse {
                        tool_call_id: "tc-001".into(),
                        tool_name: "file_read".into(),
                        arguments_json: r#"{"path":"/tmp/test"}"#.into(),
                    }],
                },
                InferenceMessage {
                    role: MessageRole::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_call_id: "tc-001".into(),
                        content: "file contents here".into(),
                        is_error: false,
                    }],
                },
            ],
            system: None,
            tools: vec![],
            model_id: "claude-sonnet-4-20250514".into(),
            max_tokens: 1024,
            temperature: None,
        }
    }

    fn sample_text_response_json() -> String {
        serde_json::json!({
            "id": "msg_01XFDUDYJgAACzvnptvVoYEL",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-20250514",
            "content": [
                {
                    "type": "text",
                    "text": "Hello! How can I help you today?"
                }
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 25,
                "output_tokens": 12
            }
        })
        .to_string()
    }

    fn sample_tool_use_response_json() -> String {
        serde_json::json!({
            "id": "msg_02ABCdef",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-20250514",
            "content": [
                {
                    "type": "text",
                    "text": "I'll read that file for you."
                },
                {
                    "type": "tool_use",
                    "id": "toolu_01A09q90qw90lq917835lq9",
                    "name": "file_read",
                    "input": {"path": "/tmp/test.txt"}
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 50,
                "output_tokens": 30,
                "cache_read_input_tokens": 10,
                "cache_creation_input_tokens": 5
            }
        })
        .to_string()
    }

    fn sample_error_response_json(error_type: &str, message: &str) -> String {
        serde_json::json!({
            "type": "error",
            "error": {
                "type": error_type,
                "message": message
            }
        })
        .to_string()
    }

    fn sample_sse_text_stream() -> String {
        [
            "event: message_start",
            r#"data: {"type":"message_start","message":{"id":"msg_stream_01","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","content":[],"stop_reason":null,"usage":{"input_tokens":20,"output_tokens":0}}}"#,
            "",
            "event: content_block_start",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "",
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            "",
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}"#,
            "",
            "event: content_block_stop",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "",
            "event: message_delta",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
            "",
            "event: message_stop",
            r#"data: {"type":"message_stop"}"#,
        ]
        .join("\n")
    }

    fn sample_sse_tool_stream() -> String {
        [
            "event: message_start",
            r#"data: {"type":"message_start","message":{"id":"msg_stream_02","type":"message","role":"assistant","model":"claude-sonnet-4-20250514","content":[],"stop_reason":null,"usage":{"input_tokens":30,"output_tokens":0}}}"#,
            "",
            "event: content_block_start",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "",
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me read that."}}"#,
            "",
            "event: content_block_stop",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "",
            "event: content_block_start",
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc123","name":"file_read","input":{}}}"#,
            "",
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"pa"}}"#,
            "",
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"th\":\"/tmp/test\"}"}}"#,
            "",
            "event: content_block_stop",
            r#"data: {"type":"content_block_stop","index":1}"#,
            "",
            "event: message_delta",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":15}}"#,
            "",
            "event: message_stop",
            r#"data: {"type":"message_stop"}"#,
        ]
        .join("\n")
    }

    // -----------------------------------------------------------------------
    // Request serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn request_body_includes_model_and_max_tokens() {
        let req = minimal_request();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize request body");

        assert_eq!(json["model"], "claude-sonnet-4-20250514");
        assert_eq!(json["max_tokens"], 1024);
        assert!(json.get("stream").is_none());
    }

    #[test]
    fn request_body_includes_system_when_present() {
        let req = minimal_request();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        assert_eq!(json["system"], "You are a helpful assistant.");
    }

    #[test]
    fn request_body_omits_system_when_none() {
        let mut req = minimal_request();
        req.system = None;
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        assert!(json.get("system").is_none());
    }

    #[test]
    fn request_body_includes_temperature_when_set() {
        let req = minimal_request();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let temp = json["temperature"].as_f64().expect("temperature is a number");
        assert!((temp - 0.7).abs() < 0.001, "temperature should be ~0.7, got {temp}");
    }

    #[test]
    fn request_body_omits_temperature_when_none() {
        let mut req = minimal_request();
        req.temperature = None;
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        assert!(json.get("temperature").is_none());
    }

    #[test]
    fn request_body_sets_stream_flag() {
        let req = minimal_request();
        let body = build_request_body(&req, true);
        let json = serde_json::to_value(&body).expect("serialize");

        assert_eq!(json["stream"], true);
    }

    #[test]
    fn request_body_maps_user_message_correctly() {
        let req = minimal_request();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"][0]["type"], "text");
        assert_eq!(messages[0]["content"][0]["text"], "hello");
    }

    #[test]
    fn request_body_maps_tools_correctly() {
        let req = request_with_tools();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let tools = json["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "file_read");
        assert_eq!(tools[0]["description"], "Read a file from disk");
        assert_eq!(tools[0]["input_schema"]["type"], "object");
        assert!(tools[0]["input_schema"]["properties"]["path"].is_object());
    }

    #[test]
    fn request_body_omits_tools_when_empty() {
        let req = minimal_request();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        // tools should be omitted entirely (skip_serializing_if = "Vec::is_empty")
        assert!(json.get("tools").is_none());
    }

    #[test]
    fn request_body_maps_tool_use_content_block() {
        let req = request_with_tool_result();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages");
        // Second message is the assistant with a tool_use block.
        let assistant_msg = &messages[1];
        assert_eq!(assistant_msg["role"], "assistant");
        assert_eq!(assistant_msg["content"][0]["type"], "tool_use");
        assert_eq!(assistant_msg["content"][0]["id"], "tc-001");
        assert_eq!(assistant_msg["content"][0]["name"], "file_read");
        assert_eq!(
            assistant_msg["content"][0]["input"],
            serde_json::json!({"path": "/tmp/test"})
        );
    }

    #[test]
    fn request_body_maps_tool_result_content_block() {
        let req = request_with_tool_result();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages");
        // Third message is the user with a tool_result block.
        let tool_result_msg = &messages[2];
        assert_eq!(tool_result_msg["role"], "user");
        assert_eq!(tool_result_msg["content"][0]["type"], "tool_result");
        assert_eq!(tool_result_msg["content"][0]["tool_use_id"], "tc-001");
        assert_eq!(
            tool_result_msg["content"][0]["content"],
            "file contents here"
        );
        // is_error is false, so it should be omitted (None).
        assert!(tool_result_msg["content"][0].get("is_error").is_none());
    }

    #[test]
    fn request_body_includes_is_error_when_true() {
        let mut req = request_with_tool_result();
        // Set is_error to true on the tool result.
        if let ContentBlock::ToolResult { is_error, .. } = &mut req.messages[2].content[0] {
            *is_error = true;
        }
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let tool_result = &json["messages"][2]["content"][0];
        assert_eq!(tool_result["is_error"], true);
    }

    // -----------------------------------------------------------------------
    // Response deserialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn deserialize_text_response() {
        let json = sample_text_response_json();
        let parsed: MessageResponse = serde_json::from_str(&json).expect("parse response");

        assert_eq!(parsed.id, "msg_01XFDUDYJgAACzvnptvVoYEL");
        assert_eq!(parsed.content.len(), 1);
        assert!(matches!(&parsed.content[0], ApiContentBlock::Text { text } if text == "Hello! How can I help you today?"));
        assert_eq!(parsed.stop_reason, Some("end_turn".to_string()));
        assert_eq!(parsed.usage.input_tokens, 25);
        assert_eq!(parsed.usage.output_tokens, 12);
    }

    #[test]
    fn deserialize_tool_use_response() {
        let json = sample_tool_use_response_json();
        let parsed: MessageResponse = serde_json::from_str(&json).expect("parse response");

        assert_eq!(parsed.content.len(), 2);

        // First block: text.
        assert!(matches!(&parsed.content[0], ApiContentBlock::Text { text } if text.contains("read that file")));

        // Second block: tool_use.
        match &parsed.content[1] {
            ApiContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_01A09q90qw90lq917835lq9");
                assert_eq!(name, "file_read");
                assert_eq!(input["path"], "/tmp/test.txt");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }

        assert_eq!(parsed.stop_reason, Some("tool_use".to_string()));
    }

    #[test]
    fn deserialize_response_with_cache_tokens() {
        let json = sample_tool_use_response_json();
        let parsed: MessageResponse = serde_json::from_str(&json).expect("parse");

        assert_eq!(parsed.usage.cache_read_input_tokens, Some(10));
        assert_eq!(parsed.usage.cache_creation_input_tokens, Some(5));
    }

    #[test]
    fn to_inference_response_maps_text_correctly() {
        let json = sample_text_response_json();
        let parsed: MessageResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.text, "Hello! How can I help you today?");
        assert!(response.tool_calls.is_empty());
        assert_eq!(response.stop_reason, "end_turn");
        assert_eq!(
            response.provider_request_id.as_deref(),
            Some("msg_01XFDUDYJgAACzvnptvVoYEL")
        );
    }

    #[test]
    fn to_inference_response_maps_tool_calls() {
        let json = sample_tool_use_response_json();
        let parsed: MessageResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.text, "I'll read that file for you.");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(
            response.tool_calls[0].tool_call_id,
            "toolu_01A09q90qw90lq917835lq9"
        );
        assert_eq!(response.tool_calls[0].tool_name, "file_read");
        assert_eq!(response.stop_reason, "tool_use");
    }

    #[test]
    fn to_inference_response_maps_cache_tokens() {
        let json = sample_tool_use_response_json();
        let parsed: MessageResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.usage.input_tokens, 50);
        assert_eq!(response.usage.output_tokens, 30);
        assert_eq!(response.usage.cache_read_tokens, Some(10));
        assert_eq!(response.usage.cache_write_tokens, Some(5));
    }

    #[test]
    fn tool_call_arguments_json_is_valid() {
        let json = sample_tool_use_response_json();
        let parsed: MessageResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        let args: serde_json::Value =
            serde_json::from_str(&response.tool_calls[0].arguments_json).expect("valid JSON");
        assert_eq!(args["path"], "/tmp/test.txt");
    }

    // -----------------------------------------------------------------------
    // Error mapping tests
    // -----------------------------------------------------------------------

    #[test]
    fn error_mapping_401_authentication() {
        let body = sample_error_response_json("authentication_error", "invalid x-api-key");
        let err = map_api_error(401, &body);
        assert!(matches!(err, ExecutorError::Authentication { .. }));
        assert!(!err.is_retryable());
    }

    #[test]
    fn error_mapping_429_rate_limit() {
        let body = sample_error_response_json("rate_limit_error", "rate limit exceeded");
        let err = map_api_error(429, &body);
        assert!(matches!(err, ExecutorError::RateLimit { .. }));
        assert!(err.is_retryable());
    }

    #[test]
    fn error_mapping_400_bad_request() {
        let body = sample_error_response_json("invalid_request_error", "messages: required");
        let err = map_api_error(400, &body);
        assert!(matches!(
            err,
            ExecutorError::Transport {
                retryable: false,
                ..
            }
        ));
        assert!(!err.is_retryable());
    }

    #[test]
    fn error_mapping_400_context_window() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"context_length_exceeded: too many tokens"}}"#;
        let err = map_api_error(400, body);
        assert!(matches!(err, ExecutorError::ContextWindowExceeded { .. }));
    }

    #[test]
    fn error_mapping_403_forbidden() {
        let body = sample_error_response_json("permission_error", "not allowed");
        let err = map_api_error(403, &body);
        assert!(matches!(err, ExecutorError::Authentication { .. }));
        assert!(!err.is_retryable());
    }

    #[test]
    fn error_mapping_500_server_error() {
        let body = sample_error_response_json("api_error", "internal server error");
        let err = map_api_error(500, &body);
        assert!(matches!(
            err,
            ExecutorError::Transport {
                retryable: true,
                ..
            }
        ));
        assert!(err.is_retryable());
    }

    #[test]
    fn error_mapping_502_server_error() {
        let err = map_api_error(502, "Bad Gateway");
        assert!(matches!(
            err,
            ExecutorError::Transport {
                retryable: true,
                ..
            }
        ));
        assert!(err.is_retryable());
    }

    #[test]
    fn error_mapping_503_server_error() {
        let body = sample_error_response_json("overloaded_error", "API is overloaded");
        let err = map_api_error(503, &body);
        assert!(matches!(
            err,
            ExecutorError::Transport {
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn error_mapping_unknown_status() {
        let err = map_api_error(418, "I'm a teapot");
        assert!(matches!(
            err,
            ExecutorError::Transport {
                retryable: false,
                ..
            }
        ));
    }

    #[test]
    fn error_mapping_handles_invalid_json_body() {
        let err = map_api_error(500, "not json at all");
        assert!(matches!(
            err,
            ExecutorError::Transport {
                retryable: true,
                ..
            }
        ));
    }

    // -----------------------------------------------------------------------
    // SSE parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_sse_text_stream_events() {
        let raw = sample_sse_text_stream();
        let events = parse_sse_events(&raw);

        // Should parse: message_start, content_block_start, 2x content_block_delta,
        // content_block_stop, message_delta, message_stop = 7 events
        assert_eq!(events.len(), 7);
        assert!(matches!(events[0], SseEvent::MessageStart { .. }));
        assert!(matches!(events[1], SseEvent::ContentBlockStart { .. }));
        assert!(matches!(events[2], SseEvent::ContentBlockDelta { .. }));
        assert!(matches!(events[3], SseEvent::ContentBlockDelta { .. }));
        assert!(matches!(events[4], SseEvent::ContentBlockStop { .. }));
        assert!(matches!(events[5], SseEvent::MessageDelta { .. }));
        assert!(matches!(events[6], SseEvent::MessageStop));
    }

    #[test]
    fn parse_sse_extracts_text_deltas() {
        let raw = sample_sse_text_stream();
        let events = parse_sse_events(&raw);

        let text_deltas: Vec<String> = events
            .iter()
            .filter_map(|e| {
                if let SseEvent::ContentBlockDelta {
                    delta: DeltaBlock::TextDelta { text },
                    ..
                } = e
                {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(text_deltas, vec!["Hello", " world"]);
    }

    #[test]
    fn parse_sse_tool_stream_events() {
        let raw = sample_sse_tool_stream();
        let events = parse_sse_events(&raw);

        // Check that we have tool use events.
        let has_tool_start = events.iter().any(|e| {
            matches!(
                e,
                SseEvent::ContentBlockStart {
                    content_block: ApiContentBlock::ToolUse { .. },
                    ..
                }
            )
        });
        assert!(has_tool_start, "should have a tool_use content_block_start");

        let input_deltas: Vec<String> = events
            .iter()
            .filter_map(|e| {
                if let SseEvent::ContentBlockDelta {
                    delta: DeltaBlock::InputJsonDelta { partial_json },
                    ..
                } = e
                {
                    Some(partial_json.clone())
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(input_deltas.len(), 2);
        let full_json: String = input_deltas.concat();
        assert_eq!(full_json, r#"{"path":"/tmp/test"}"#);
    }

    #[test]
    fn parse_sse_extracts_message_id() {
        let raw = sample_sse_text_stream();
        let events = parse_sse_events(&raw);

        if let SseEvent::MessageStart { message } = &events[0] {
            assert_eq!(message.id, "msg_stream_01");
        } else {
            panic!("first event should be MessageStart");
        }
    }

    #[test]
    fn parse_sse_extracts_stop_reason() {
        let raw = sample_sse_text_stream();
        let events = parse_sse_events(&raw);

        let stop_reason = events.iter().find_map(|e| {
            if let SseEvent::MessageDelta { delta, .. } = e {
                delta.stop_reason.clone()
            } else {
                None
            }
        });

        assert_eq!(stop_reason, Some("end_turn".to_string()));
    }

    #[test]
    fn parse_sse_extracts_usage() {
        let raw = sample_sse_text_stream();
        let events = parse_sse_events(&raw);

        // Input tokens from message_start.
        if let SseEvent::MessageStart { message } = &events[0] {
            assert_eq!(message.usage.input_tokens, 20);
        }

        // Output tokens from message_delta.
        let output_tokens = events.iter().find_map(|e| {
            if let SseEvent::MessageDelta {
                usage: Some(du), ..
            } = e
            {
                Some(du.output_tokens)
            } else {
                None
            }
        });
        assert_eq!(output_tokens, Some(5));
    }

    #[test]
    fn parse_sse_skips_non_data_lines() {
        let raw = "event: message_start\n: comment\nretry: 1000\n\ndata: {\"type\":\"ping\"}\n";
        let events = parse_sse_events(raw);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SseEvent::Ping));
    }

    #[test]
    fn parse_sse_skips_done_marker() {
        let raw = "data: {\"type\":\"ping\"}\ndata: [DONE]\n";
        let events = parse_sse_events(raw);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn parse_sse_handles_empty_input() {
        let events = parse_sse_events("");
        assert!(events.is_empty());
    }

    #[test]
    fn parse_sse_handles_malformed_json() {
        let raw = "data: {not valid json}\ndata: {\"type\":\"ping\"}\n";
        let events = parse_sse_events(raw);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SseEvent::Ping));
    }

    // -----------------------------------------------------------------------
    // Full SSE-to-StreamEvent mapping tests
    // -----------------------------------------------------------------------

    #[test]
    fn sse_text_stream_produces_correct_stream_events() {
        let raw = sample_sse_text_stream();
        let sse_events = parse_sse_events(&raw);
        let stream_events = process_sse_events(sse_events);

        // Verify text deltas.
        let text_deltas: Vec<String> = stream_events
            .iter()
            .filter_map(|e| {
                if let Ok(StreamEvent::TextDelta { delta }) = e {
                    Some(delta.clone())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(text_deltas.concat(), "Hello world");

        // Last event should be Completed.
        if let Ok(StreamEvent::Completed { result }) = stream_events.last().expect("events") {
            assert_eq!(result.text, "Hello world");
            assert_eq!(result.stop_reason, "end_turn");
            assert_eq!(result.usage.input_tokens, 20);
            assert_eq!(result.usage.output_tokens, 5);
            assert_eq!(
                result.provider_request_id.as_deref(),
                Some("msg_stream_01")
            );
        } else {
            panic!("last event should be Completed");
        }
    }

    #[test]
    fn sse_tool_stream_produces_tool_call_events() {
        let raw = sample_sse_tool_stream();
        let sse_events = parse_sse_events(&raw);
        let stream_events = process_sse_events(sse_events);

        // Verify text delta.
        let text_deltas: Vec<String> = stream_events
            .iter()
            .filter_map(|e| {
                if let Ok(StreamEvent::TextDelta { delta }) = e {
                    Some(delta.clone())
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(text_deltas.concat(), "Let me read that.");

        // Verify tool call complete events.
        let tool_completes: Vec<&ToolCall> = stream_events
            .iter()
            .filter_map(|e| {
                if let Ok(StreamEvent::ToolCallComplete { call }) = e {
                    Some(call)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(tool_completes.len(), 1);
        assert_eq!(tool_completes[0].tool_call_id, "toolu_abc123");
        assert_eq!(tool_completes[0].tool_name, "file_read");
        assert_eq!(
            tool_completes[0].arguments_json,
            r#"{"path":"/tmp/test"}"#
        );

        // Verify final Completed event.
        if let Ok(StreamEvent::Completed { result }) = stream_events.last().expect("events") {
            assert_eq!(result.stop_reason, "tool_use");
            assert_eq!(result.tool_calls.len(), 1);
            assert_eq!(result.usage.output_tokens, 15);
        } else {
            panic!("last event should be Completed");
        }
    }

    #[test]
    fn sse_stream_emits_usage_update() {
        let raw = sample_sse_text_stream();
        let sse_events = parse_sse_events(&raw);
        let stream_events = process_sse_events(sse_events);

        let has_usage_update = stream_events.iter().any(|e| {
            matches!(e, Ok(StreamEvent::UsageUpdate { .. }))
        });
        assert!(has_usage_update, "should have a UsageUpdate event");
    }

    #[test]
    fn sse_tool_call_deltas_have_name_on_first_only() {
        let raw = sample_sse_tool_stream();
        let sse_events = parse_sse_events(&raw);
        let stream_events = process_sse_events(sse_events);

        let tool_deltas: Vec<(Option<String>, String)> = stream_events
            .iter()
            .filter_map(|e| {
                if let Ok(StreamEvent::ToolCallDelta { name, input_delta, .. }) = e {
                    Some((name.clone(), input_delta.clone()))
                } else {
                    None
                }
            })
            .collect();

        // First delta (from content_block_start) should have the name.
        assert!(tool_deltas[0].0.is_some());
        assert_eq!(tool_deltas[0].0.as_deref(), Some("file_read"));

        // Subsequent deltas should not have the name.
        for delta in &tool_deltas[1..] {
            assert!(delta.0.is_none());
        }
    }

    // -----------------------------------------------------------------------
    // Token usage tests
    // -----------------------------------------------------------------------

    #[test]
    fn token_usage_mapped_from_api_usage() {
        let api_usage = ApiUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_input_tokens: Some(20),
            cache_creation_input_tokens: Some(10),
        };
        let usage = to_token_usage(&api_usage);

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, Some(20));
        assert_eq!(usage.cache_write_tokens, Some(10));
    }

    #[test]
    fn token_usage_handles_missing_cache_fields() {
        let api_usage = ApiUsage {
            input_tokens: 42,
            output_tokens: 17,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        };
        let usage = to_token_usage(&api_usage);

        assert_eq!(usage.input_tokens, 42);
        assert_eq!(usage.output_tokens, 17);
        assert!(usage.cache_read_tokens.is_none());
        assert!(usage.cache_write_tokens.is_none());
    }

    // -----------------------------------------------------------------------
    // Builder / constructor tests
    // -----------------------------------------------------------------------

    #[test]
    fn new_uses_default_base_url() {
        let executor = AnthropicExecutor::new("key".into(), "model".into());
        assert_eq!(executor.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn new_uses_default_max_retries() {
        let executor = AnthropicExecutor::new("key".into(), "model".into());
        assert_eq!(executor.max_retries, DEFAULT_MAX_RETRIES);
    }

    #[test]
    fn with_base_url_overrides_default() {
        let executor = AnthropicExecutor::new_builder("key".into(), "model".into())
            .with_base_url("http://localhost:8080".into());
        assert_eq!(executor.base_url, "http://localhost:8080");
    }

    #[test]
    fn with_max_retries_overrides_default() {
        let executor = AnthropicExecutor::new_builder("key".into(), "model".into())
            .with_max_retries(5);
        assert_eq!(executor.max_retries, 5);
    }

    #[test]
    fn builder_chain_works() {
        let executor = AnthropicExecutor::new_builder("key".into(), "model".into())
            .with_base_url("http://localhost".into())
            .with_max_retries(10)
            .with_timeout(Duration::from_secs(30))
            .build();
        assert_eq!(executor.base_url, "http://localhost");
        assert_eq!(executor.max_retries, 10);
    }

    #[test]
    fn with_max_concurrent_creates_semaphore() {
        let executor = AnthropicExecutor::new_builder("key".into(), "model".into())
            .with_max_concurrent(5);
        assert!(executor.concurrency_semaphore.is_some());
        let sem = executor.concurrency_semaphore.as_ref().unwrap();
        assert_eq!(sem.available_permits(), 5);
    }

    #[test]
    fn without_max_concurrent_no_semaphore() {
        let executor = AnthropicExecutor::new_builder("key".into(), "model".into());
        assert!(executor.concurrency_semaphore.is_none());
    }

    #[test]
    fn new_constructor_has_no_semaphore() {
        let executor = AnthropicExecutor::new("key".into(), "model".into());
        assert!(executor.concurrency_semaphore.is_none());
    }

    #[test]
    fn builder_chain_with_max_concurrent() {
        let executor = AnthropicExecutor::new_builder("key".into(), "model".into())
            .with_base_url("http://localhost".into())
            .with_max_retries(2)
            .with_max_concurrent(10)
            .build();
        assert_eq!(executor.base_url, "http://localhost");
        assert_eq!(executor.max_retries, 2);
        assert!(executor.concurrency_semaphore.is_some());
        assert_eq!(
            executor.concurrency_semaphore.as_ref().unwrap().available_permits(),
            10
        );
    }

    // -----------------------------------------------------------------------
    // Conversion edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn to_api_message_handles_invalid_json_in_tool_use() {
        let msg = InferenceMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                tool_call_id: "tc-1".into(),
                tool_name: "test".into(),
                arguments_json: "not valid json".into(),
            }],
        };
        let api_msg = to_api_message(&msg);
        // Should not panic; should fall back to empty object.
        if let ApiContentBlock::ToolUse { input, .. } = &api_msg.content[0] {
            assert!(input.is_object());
        } else {
            panic!("expected ToolUse");
        }
    }

    #[test]
    fn to_api_tool_handles_invalid_schema_json() {
        let tool = ToolDefinition {
            name: "test".into(),
            description: "a test tool".into(),
            input_schema_json: "{{invalid}}".into(),
        };
        let api_tool = to_api_tool(&tool);
        // Should not panic; should fall back to a basic object schema.
        assert_eq!(api_tool.input_schema["type"], "object");
    }

    #[test]
    fn response_with_no_stop_reason_defaults_to_end_turn() {
        let resp = MessageResponse {
            id: "msg_1".into(),
            content: vec![ApiContentBlock::Text {
                text: "hi".into(),
            }],
            model: "test".into(),
            stop_reason: None,
            usage: ApiUsage {
                input_tokens: 1,
                output_tokens: 1,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        };
        let inference = to_inference_response(&resp);
        assert_eq!(inference.stop_reason, "end_turn");
    }

    #[test]
    fn multiple_text_blocks_are_concatenated() {
        let resp = MessageResponse {
            id: "msg_multi".into(),
            content: vec![
                ApiContentBlock::Text {
                    text: "Hello ".into(),
                },
                ApiContentBlock::Text {
                    text: "world!".into(),
                },
            ],
            model: "test".into(),
            stop_reason: Some("end_turn".into()),
            usage: ApiUsage {
                input_tokens: 5,
                output_tokens: 3,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        };
        let inference = to_inference_response(&resp);
        assert_eq!(inference.text, "Hello world!");
    }

    #[test]
    fn mixed_text_and_tool_blocks_both_captured() {
        let resp = MessageResponse {
            id: "msg_mixed".into(),
            content: vec![
                ApiContentBlock::Text {
                    text: "I will call a tool.".into(),
                },
                ApiContentBlock::ToolUse {
                    id: "tu_1".into(),
                    name: "my_tool".into(),
                    input: serde_json::json!({"key": "value"}),
                },
            ],
            model: "test".into(),
            stop_reason: Some("tool_use".into()),
            usage: ApiUsage {
                input_tokens: 10,
                output_tokens: 8,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        };
        let inference = to_inference_response(&resp);
        assert_eq!(inference.text, "I will call a tool.");
        assert_eq!(inference.tool_calls.len(), 1);
        assert_eq!(inference.tool_calls[0].tool_name, "my_tool");
    }

    // -----------------------------------------------------------------------
    // Retry logic tests (unit-level, no HTTP)
    // -----------------------------------------------------------------------

    #[test]
    fn retryable_errors_are_identified_correctly() {
        // Rate limit is retryable.
        assert!(ExecutorError::RateLimit {
            retry_after_secs: None,
        }
        .is_retryable());

        // Timeout is retryable.
        assert!(ExecutorError::Timeout { elapsed_ms: 5000 }.is_retryable());

        // Transport with retryable=true is retryable.
        assert!(ExecutorError::Transport {
            message: "server error".into(),
            retryable: true,
        }
        .is_retryable());

        // Transport with retryable=false is not retryable.
        assert!(!ExecutorError::Transport {
            message: "bad request".into(),
            retryable: false,
        }
        .is_retryable());

        // Authentication is not retryable.
        assert!(!ExecutorError::Authentication {
            message: "invalid key".into(),
        }
        .is_retryable());

        // Cancelled is not retryable.
        assert!(!ExecutorError::Cancelled.is_retryable());
    }

    #[test]
    fn map_api_error_produces_retryable_for_server_errors() {
        for status in [500, 502, 503, 504] {
            let err = map_api_error(status, "error");
            assert!(
                err.is_retryable(),
                "status {status} should produce retryable error"
            );
        }
    }

    #[test]
    fn map_api_error_produces_non_retryable_for_client_errors() {
        for status in [400, 401, 403] {
            let err = map_api_error(status, r#"{"error":{"type":"e","message":"m"}}"#);
            assert!(
                !err.is_retryable(),
                "status {status} should produce non-retryable error"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Full request round-trip serialization test
    // -----------------------------------------------------------------------

    #[test]
    fn full_request_round_trip_matches_anthropic_format() {
        let req = request_with_tools();
        let body = build_request_body(&req, false);
        let json_str = serde_json::to_string_pretty(&body).expect("serialize");

        // Verify the overall structure matches what the Anthropic API expects.
        let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("re-parse");

        assert!(parsed.get("model").is_some());
        assert!(parsed.get("messages").is_some());
        assert!(parsed.get("max_tokens").is_some());
        assert!(parsed.get("tools").is_some());

        // Messages should be an array of objects with role + content.
        let msgs = parsed["messages"].as_array().expect("messages array");
        for msg in msgs {
            assert!(msg.get("role").is_some());
            assert!(msg.get("content").is_some());
        }

        // Tools should have name, description, input_schema.
        let tools = parsed["tools"].as_array().expect("tools array");
        for tool in tools {
            assert!(tool.get("name").is_some());
            assert!(tool.get("description").is_some());
            assert!(tool.get("input_schema").is_some());
        }
    }

    // -----------------------------------------------------------------------
    // Full response round-trip deserialization test
    // -----------------------------------------------------------------------

    #[test]
    fn full_response_round_trip() {
        // A response with multiple content blocks, tool use, and cache tokens.
        let json = serde_json::json!({
            "id": "msg_roundtrip",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-20250514",
            "content": [
                {"type": "text", "text": "Processing your request."},
                {
                    "type": "tool_use",
                    "id": "toolu_rt01",
                    "name": "calculator",
                    "input": {"expression": "2 + 2"}
                },
                {"type": "text", "text": " Here is the result."}
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_read_input_tokens": 30,
                "cache_creation_input_tokens": 15
            }
        });

        let parsed: MessageResponse =
            serde_json::from_value(json).expect("parse response");
        let response = to_inference_response(&parsed);

        assert_eq!(
            response.text,
            "Processing your request. Here is the result."
        );
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].tool_call_id, "toolu_rt01");
        assert_eq!(response.tool_calls[0].tool_name, "calculator");
        assert_eq!(response.stop_reason, "tool_use");
        assert_eq!(response.usage.input_tokens, 100);
        assert_eq!(response.usage.output_tokens, 50);
        assert_eq!(response.usage.cache_read_tokens, Some(30));
        assert_eq!(response.usage.cache_write_tokens, Some(15));
        assert_eq!(
            response.provider_request_id.as_deref(),
            Some("msg_roundtrip")
        );
    }
}

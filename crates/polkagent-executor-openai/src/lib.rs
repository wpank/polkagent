//! OpenAI-compatible Chat Completions API adapter for the [`ModelExecutor`] trait.
//!
//! This crate provides [`OpenAiExecutor`] -- a production adapter that
//! translates the provider-agnostic [`InferenceRequest`] into the OpenAI
//! Chat Completions API wire format, handles authentication, rate-limit
//! retries with exponential backoff, and maps responses back to
//! [`InferenceResponse`].
//!
//! Compatible with OpenAI, Azure OpenAI, vLLM, Ollama, and any other
//! provider that exposes the OpenAI Chat Completions API.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use polkagent_executor_openai::OpenAiExecutor;
//!
//! // OpenAI
//! let executor = OpenAiExecutor::new(
//!     "sk-...".to_string(),
//!     "gpt-4o".to_string(),
//! );
//!
//! // Ollama (local)
//! let executor = OpenAiExecutor::new_builder(
//!     String::new(),
//!     "llama3".to_string(),
//! )
//! .with_base_url("http://localhost:11434/v1".to_string())
//! .build();
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

/// Default OpenAI API base URL.
const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// Default request timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Default maximum number of retries for retryable errors.
const DEFAULT_MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff in milliseconds.
const BACKOFF_BASE_MS: u64 = 500;

// ---------------------------------------------------------------------------
// OpenAI API types (private)
// ---------------------------------------------------------------------------

/// Request body for the OpenAI Chat Completions API.
#[derive(Debug, Clone, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

/// A message in the OpenAI conversation format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ApiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

/// A tool definition in the OpenAI wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: ApiFunction,
}

/// A function definition within an OpenAI tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// A tool call in the OpenAI wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ApiFunctionCall,
}

/// A function call within an OpenAI tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiFunctionCall {
    name: String,
    arguments: String,
}

/// Top-level response from the OpenAI Chat Completions API.
#[derive(Debug, Clone, Deserialize)]
struct ChatCompletionResponse {
    id: String,
    choices: Vec<Choice>,
    usage: Option<ApiUsage>,
    #[allow(dead_code)]
    model: Option<String>,
}

/// A single choice in the OpenAI response.
#[derive(Debug, Clone, Deserialize)]
struct Choice {
    message: ChoiceMessage,
    finish_reason: Option<String>,
}

/// The message within a choice.
#[derive(Debug, Clone, Deserialize)]
struct ChoiceMessage {
    #[allow(dead_code)]
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<ApiToolCall>>,
}

/// Token usage from the OpenAI API response.
#[derive(Debug, Clone, Deserialize)]
struct ApiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    #[allow(dead_code)]
    total_tokens: u32,
}

/// Error response body from the OpenAI API.
#[derive(Debug, Clone, Deserialize)]
struct ApiErrorResponse {
    error: ApiErrorDetail,
}

/// Inner error detail from the OpenAI API.
#[derive(Debug, Clone, Deserialize)]
struct ApiErrorDetail {
    message: String,
    #[allow(dead_code)]
    #[serde(rename = "type")]
    error_type: Option<String>,
    #[allow(dead_code)]
    code: Option<String>,
}

// ---------------------------------------------------------------------------
// SSE / streaming types
// ---------------------------------------------------------------------------

/// A single chunk from the OpenAI streaming API.
#[derive(Debug, Clone, Deserialize)]
struct StreamChunk {
    #[allow(dead_code)]
    id: Option<String>,
    choices: Vec<StreamChoice>,
    usage: Option<ApiUsage>,
}

/// A choice within a streaming chunk.
#[derive(Debug, Clone, Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
    finish_reason: Option<String>,
}

/// The delta content within a streaming choice.
#[derive(Debug, Clone, Deserialize)]
struct StreamDelta {
    #[allow(dead_code)]
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<StreamToolCall>>,
}

/// A tool call delta within a streaming response.
#[derive(Debug, Clone, Deserialize)]
struct StreamToolCall {
    index: usize,
    id: Option<String>,
    function: Option<StreamFunctionCall>,
}

/// A function call delta within a streaming tool call.
#[derive(Debug, Clone, Deserialize)]
struct StreamFunctionCall {
    name: Option<String>,
    arguments: Option<String>,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a provider-agnostic `InferenceMessage` to the OpenAI wire format.
///
/// OpenAI uses a different message structure than Anthropic:
/// - User messages have `role: "user"` and `content` as a string.
/// - Assistant messages have `role: "assistant"` with optional `tool_calls`.
/// - Tool results are sent as `role: "tool"` messages with `tool_call_id`.
///
/// A single `InferenceMessage` may expand into multiple OpenAI messages
/// when it contains mixed content blocks (e.g., text + tool results).
fn to_api_messages(msg: &InferenceMessage) -> Vec<ApiMessage> {
    let role_str = match msg.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    };

    // Separate content blocks into text parts, tool_use parts, and tool_result parts.
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ApiToolCall> = Vec::new();
    let mut tool_results: Vec<(String, String, bool)> = Vec::new();

    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => {
                text_parts.push(text.clone());
            }
            ContentBlock::ToolUse {
                tool_call_id,
                tool_name,
                arguments_json,
            } => {
                tool_calls.push(ApiToolCall {
                    id: tool_call_id.clone(),
                    call_type: "function".to_string(),
                    function: ApiFunctionCall {
                        name: tool_name.clone(),
                        arguments: arguments_json.clone(),
                    },
                });
            }
            ContentBlock::ToolResult {
                tool_call_id,
                content,
                is_error,
            } => {
                tool_results.push((tool_call_id.clone(), content.clone(), *is_error));
            }
        }
    }

    let mut messages = Vec::new();

    // If this is an assistant message with tool calls, emit one message.
    if role_str == "assistant" && !tool_calls.is_empty() {
        let content = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        };
        messages.push(ApiMessage {
            role: "assistant".to_string(),
            content,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        });
    } else if !text_parts.is_empty() {
        messages.push(ApiMessage {
            role: role_str.to_string(),
            content: Some(text_parts.join("")),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    // Tool results become separate "tool" role messages.
    for (call_id, content, _is_error) in tool_results {
        messages.push(ApiMessage {
            role: "tool".to_string(),
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(call_id),
        });
    }

    messages
}

/// Convert a provider-agnostic `ToolDefinition` to the OpenAI wire format.
fn to_api_tool(tool: &ToolDefinition) -> ApiTool {
    let parameters: serde_json::Value = serde_json::from_str(&tool.input_schema_json)
        .unwrap_or_else(|_| {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
        });

    ApiTool {
        tool_type: "function".to_string(),
        function: ApiFunction {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters,
        },
    }
}

/// Build the API request body from an `InferenceRequest`.
fn build_request_body(request: &InferenceRequest, stream: bool) -> ChatCompletionRequest {
    let mut messages: Vec<ApiMessage> = Vec::new();

    // OpenAI uses a system message as the first message with role "system".
    if let Some(system) = &request.system {
        messages.push(ApiMessage {
            role: "system".to_string(),
            content: Some(system.clone()),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    // Convert each inference message.
    for msg in &request.messages {
        messages.extend(to_api_messages(msg));
    }

    let tools: Vec<ApiTool> = request.tools.iter().map(to_api_tool).collect();

    let tool_choice = if !tools.is_empty() {
        Some("auto".to_string())
    } else {
        None
    };

    ChatCompletionRequest {
        model: request.model_id.clone(),
        messages,
        max_tokens: Some(request.max_tokens),
        temperature: request.temperature,
        tools,
        tool_choice,
        stream: if stream { Some(true) } else { None },
    }
}

/// Convert an `ApiUsage` to the trait-level `TokenUsage`.
fn to_token_usage(api_usage: &ApiUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: api_usage.prompt_tokens,
        output_tokens: api_usage.completion_tokens,
        cache_read_tokens: None,
        cache_write_tokens: None,
    }
}

/// Convert a `ChatCompletionResponse` to the trait-level `InferenceResponse`.
fn to_inference_response(resp: &ChatCompletionResponse) -> InferenceResponse {
    let choice = resp.choices.first();

    let text = choice
        .and_then(|c| c.message.content.clone())
        .unwrap_or_default();

    let tool_calls: Vec<ToolCall> = choice
        .and_then(|c| c.message.tool_calls.as_ref())
        .map(|tcs| {
            tcs.iter()
                .map(|tc| ToolCall {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.function.name.clone(),
                    arguments_json: tc.function.arguments.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    let finish_reason = choice
        .and_then(|c| c.finish_reason.clone())
        .unwrap_or_else(|| "stop".to_string());

    // Map OpenAI finish_reason to our normalized stop_reason values.
    let stop_reason = map_finish_reason(&finish_reason);

    let usage = resp
        .usage
        .as_ref()
        .map(to_token_usage)
        .unwrap_or_default();

    InferenceResponse {
        text,
        tool_calls,
        stop_reason,
        usage,
        provider_request_id: Some(resp.id.clone()),
    }
}

/// Map OpenAI `finish_reason` to the normalized stop reason values
/// used by the executor trait.
fn map_finish_reason(finish_reason: &str) -> String {
    match finish_reason {
        "stop" => "end_turn".to_string(),
        "length" => "max_tokens".to_string(),
        "tool_calls" => "tool_use".to_string(),
        "content_filter" => "end_turn".to_string(),
        other => other.to_string(),
    }
}

/// Map an HTTP status code and optional error body to an `ExecutorError`.
fn map_api_error(status: u16, body: &str) -> ExecutorError {
    let detail = serde_json::from_str::<ApiErrorResponse>(body)
        .map(|e| e.error.message)
        .unwrap_or_else(|_| body.to_string());

    match status {
        401 => ExecutorError::Authentication { message: detail },
        429 => ExecutorError::RateLimit {
            retry_after_secs: None,
        },
        400 => {
            // Check for context window errors.
            if body.contains("context_length_exceeded")
                || body.contains("maximum context length")
                || body.contains("too many tokens")
            {
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
/// Returns a list of parsed `StreamChunk` values. Lines that are not valid
/// `data:` lines, are `[DONE]` markers, or cannot be parsed are silently
/// skipped.
fn parse_sse_chunks(chunk: &str) -> Vec<StreamChunk> {
    let mut chunks = Vec::new();
    for line in chunk.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                continue;
            }
            match serde_json::from_str::<StreamChunk>(data) {
                Ok(parsed) => chunks.push(parsed),
                Err(e) => {
                    debug!(data = data, error = %e, "skipping unparseable SSE data line");
                }
            }
        }
    }
    chunks
}

/// State tracker for assembling tool calls from streaming deltas.
#[derive(Debug, Default)]
struct ToolCallAssembler {
    id: String,
    name: String,
    arguments: String,
}

/// Process raw SSE chunks into a list of `StreamEvent` values.
///
/// This function handles text deltas, tool call assembly from partial
/// function call deltas, usage tracking, and produces the terminal
/// `Completed` event.
fn process_sse_chunks(chunks: Vec<StreamChunk>) -> Vec<Result<StreamEvent, ExecutorError>> {
    let mut stream_events: Vec<Result<StreamEvent, ExecutorError>> = Vec::new();
    let mut accumulated_text = String::new();
    let mut tool_assemblers: Vec<ToolCallAssembler> = Vec::new();
    let mut final_tool_calls: Vec<ToolCall> = Vec::new();
    let mut usage = TokenUsage::default();
    let mut stop_reason = String::from("end_turn");

    for chunk in &chunks {
        // Track usage if present.
        if let Some(api_usage) = &chunk.usage {
            usage = to_token_usage(api_usage);
        }

        for choice in &chunk.choices {
            // Track finish_reason.
            if let Some(reason) = &choice.finish_reason {
                stop_reason = map_finish_reason(reason);
            }

            let delta = &choice.delta;

            // Text delta.
            if let Some(content) = &delta.content {
                if !content.is_empty() {
                    accumulated_text.push_str(content);
                    stream_events.push(Ok(StreamEvent::TextDelta {
                        delta: content.clone(),
                    }));
                }
            }

            // Tool call deltas.
            if let Some(tool_calls) = &delta.tool_calls {
                for tc in tool_calls {
                    let idx = tc.index;

                    // Ensure we have an assembler for this index.
                    while tool_assemblers.len() <= idx {
                        tool_assemblers.push(ToolCallAssembler::default());
                    }
                    let assembler = &mut tool_assemblers[idx];

                    // Capture id if present (first delta for this call).
                    if let Some(id) = &tc.id {
                        assembler.id = id.clone();
                    }

                    if let Some(func) = &tc.function {
                        // Capture name if present.
                        let name_delta = func.name.as_ref();
                        if let Some(name) = name_delta {
                            assembler.name.push_str(name);
                        }

                        // Append argument fragment.
                        if let Some(args) = &func.arguments {
                            assembler.arguments.push_str(args);

                            stream_events.push(Ok(StreamEvent::ToolCallDelta {
                                tool_call_id: assembler.id.clone(),
                                name: name_delta.cloned(),
                                input_delta: args.clone(),
                            }));
                        }
                    }
                }
            }
        }
    }

    // Finalize assembled tool calls.
    for assembler in &tool_assemblers {
        if !assembler.id.is_empty() {
            let call = ToolCall {
                tool_call_id: assembler.id.clone(),
                tool_name: assembler.name.clone(),
                arguments_json: assembler.arguments.clone(),
            };
            stream_events.push(Ok(StreamEvent::ToolCallComplete { call: call.clone() }));
            final_tool_calls.push(call);
        }
    }

    // Emit usage update.
    stream_events.push(Ok(StreamEvent::UsageUpdate {
        usage: usage.clone(),
    }));

    // Emit the terminal Completed event.
    let response = InferenceResponse {
        text: accumulated_text,
        tool_calls: final_tool_calls,
        stop_reason,
        usage,
        provider_request_id: None,
    };
    stream_events.push(Ok(StreamEvent::Completed { result: response }));

    stream_events
}

// ---------------------------------------------------------------------------
// OpenAiExecutor
// ---------------------------------------------------------------------------

/// A [`ModelExecutor`] adapter for the OpenAI Chat Completions API.
///
/// Compatible with OpenAI, Azure OpenAI, vLLM, Ollama, and any other
/// service that exposes the OpenAI Chat Completions API.
///
/// Handles authentication, request/response mapping, rate-limit retries with
/// exponential backoff, and streaming via SSE.
pub struct OpenAiExecutor {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
    max_retries: u32,
}

impl OpenAiExecutor {
    /// Create a new `OpenAiExecutor` with the given API key and model.
    ///
    /// Uses the default OpenAI API base URL (`https://api.openai.com`),
    /// a 120-second timeout, and up to 3 retries for retryable errors.
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
        })
    }

    /// Override the base URL (for Azure OpenAI, Ollama, vLLM, etc.).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use polkagent_executor_openai::OpenAiExecutor;
    ///
    /// let executor = OpenAiExecutor::new_builder(String::new(), "llama3".into())
    ///     .with_base_url("http://localhost:11434/v1".into())
    ///     .build();
    /// ```
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
        }
    }

    /// Execute the HTTP request with retry logic for retryable errors.
    async fn execute_with_retries(
        &self,
        body: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ExecutorError> {
        let url = format!("{}/v1/chat/completions", self.base_url);
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
                .header("Authorization", format!("Bearer {}", self.api_key))
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

                let parsed: ChatCompletionResponse =
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

    /// Execute a streaming request and collect SSE chunks into `StreamEvent`s.
    async fn execute_streaming(
        &self,
        body: &ChatCompletionRequest,
    ) -> Result<Vec<Result<StreamEvent, ExecutorError>>, ExecutorError> {
        let url = format!("{}/v1/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
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

        let sse_chunks = parse_sse_chunks(&full_body);
        Ok(process_sse_chunks(sse_chunks))
    }
}

// ---------------------------------------------------------------------------
// ModelExecutor implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ModelExecutor for OpenAiExecutor {
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
            "executing openai inference"
        );

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
            "starting openai streaming inference"
        );

        let body = build_request_body(&request, true);
        let events = self.execute_streaming(&body).await?;

        Ok(Box::new(stream::iter(events)))
    }

    async fn health(&self) -> Result<(), ExecutorError> {
        // GET /v1/models to verify connectivity and authentication.
        let url = format!("{}/v1/models", self.base_url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
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
            model_id: "gpt-4o".into(),
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
            model_id: "gpt-4o".into(),
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
                        tool_call_id: "call_abc123".into(),
                        tool_name: "file_read".into(),
                        arguments_json: r#"{"path":"/tmp/test"}"#.into(),
                    }],
                },
                InferenceMessage {
                    role: MessageRole::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_call_id: "call_abc123".into(),
                        content: "file contents here".into(),
                        is_error: false,
                    }],
                },
            ],
            system: None,
            tools: vec![],
            model_id: "gpt-4o".into(),
            max_tokens: 1024,
            temperature: None,
        }
    }

    fn sample_text_response_json() -> String {
        serde_json::json!({
            "id": "chatcmpl-abc123",
            "object": "chat.completion",
            "created": 1677858242,
            "model": "gpt-4o",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hello! How can I help you today?"
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 25,
                "completion_tokens": 12,
                "total_tokens": 37
            }
        })
        .to_string()
    }

    fn sample_tool_use_response_json() -> String {
        serde_json::json!({
            "id": "chatcmpl-def456",
            "object": "chat.completion",
            "created": 1677858242,
            "model": "gpt-4o",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_xyz789",
                                "type": "function",
                                "function": {
                                    "name": "file_read",
                                    "arguments": "{\"path\":\"/tmp/test.txt\"}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 30,
                "total_tokens": 80
            }
        })
        .to_string()
    }

    fn sample_tool_use_with_text_response_json() -> String {
        serde_json::json!({
            "id": "chatcmpl-ghi012",
            "object": "chat.completion",
            "created": 1677858242,
            "model": "gpt-4o",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "I'll read that file for you.",
                        "tool_calls": [
                            {
                                "id": "call_xyz789",
                                "type": "function",
                                "function": {
                                    "name": "file_read",
                                    "arguments": "{\"path\":\"/tmp/test.txt\"}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 30,
                "total_tokens": 80
            }
        })
        .to_string()
    }

    fn sample_multiple_choices_response_json() -> String {
        serde_json::json!({
            "id": "chatcmpl-multi",
            "object": "chat.completion",
            "created": 1677858242,
            "model": "gpt-4o",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "First choice."
                    },
                    "finish_reason": "stop"
                },
                {
                    "index": 1,
                    "message": {
                        "role": "assistant",
                        "content": "Second choice."
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 8,
                "total_tokens": 18
            }
        })
        .to_string()
    }

    fn sample_error_response_json(error_type: &str, message: &str) -> String {
        serde_json::json!({
            "error": {
                "message": message,
                "type": error_type,
                "code": null
            }
        })
        .to_string()
    }

    fn sample_sse_text_stream() -> String {
        [
            r#"data: {"id":"chatcmpl-stream1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
            r#"data: {"id":"chatcmpl-stream1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#,
            r#"data: {"id":"chatcmpl-stream1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":null}]}"#,
            r#"data: {"id":"chatcmpl-stream1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            r#"data: {"id":"chatcmpl-stream1","object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":20,"completion_tokens":5,"total_tokens":25}}"#,
            "data: [DONE]",
        ]
        .join("\n")
    }

    fn sample_sse_tool_stream() -> String {
        [
            r#"data: {"id":"chatcmpl-stream2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_abc123","type":"function","function":{"name":"file_read","arguments":""}}]},"finish_reason":null}]}"#,
            r#"data: {"id":"chatcmpl-stream2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"pa"}}]},"finish_reason":null}]}"#,
            r#"data: {"id":"chatcmpl-stream2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"/tmp/test\"}"}}]},"finish_reason":null}]}"#,
            r#"data: {"id":"chatcmpl-stream2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"data: {"id":"chatcmpl-stream2","object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":30,"completion_tokens":15,"total_tokens":45}}"#,
            "data: [DONE]",
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

        assert_eq!(json["model"], "gpt-4o");
        assert_eq!(json["max_tokens"], 1024);
        assert!(json.get("stream").is_none());
    }

    #[test]
    fn request_body_includes_system_as_first_message() {
        let req = minimal_request();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages array");
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are a helpful assistant.");
    }

    #[test]
    fn request_body_omits_system_message_when_none() {
        let mut req = minimal_request();
        req.system = None;
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages");
        // No system message; first message should be user.
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn request_body_includes_temperature_when_set() {
        let req = minimal_request();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let temp = json["temperature"]
            .as_f64()
            .expect("temperature is a number");
        assert!(
            (temp - 0.7).abs() < 0.001,
            "temperature should be ~0.7, got {temp}"
        );
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
        // Index 1 because index 0 is the system message.
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "hello");
    }

    #[test]
    fn request_body_maps_tools_correctly() {
        let req = request_with_tools();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let tools = json["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "file_read");
        assert_eq!(tools[0]["function"]["description"], "Read a file from disk");
        assert_eq!(tools[0]["function"]["parameters"]["type"], "object");
        assert!(tools[0]["function"]["parameters"]["properties"]["path"].is_object());
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
    fn request_body_sets_tool_choice_auto_when_tools_present() {
        let req = request_with_tools();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        assert_eq!(json["tool_choice"], "auto");
    }

    #[test]
    fn request_body_omits_tool_choice_when_no_tools() {
        let req = minimal_request();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        assert!(json.get("tool_choice").is_none());
    }

    #[test]
    fn request_body_maps_tool_use_as_assistant_tool_calls() {
        let req = request_with_tool_result();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages");
        // Messages: [0] user "read the file", [1] assistant with tool_calls, [2] tool result
        let assistant_msg = &messages[1];
        assert_eq!(assistant_msg["role"], "assistant");
        let tool_calls = assistant_msg["tool_calls"]
            .as_array()
            .expect("tool_calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "call_abc123");
        assert_eq!(tool_calls[0]["type"], "function");
        assert_eq!(tool_calls[0]["function"]["name"], "file_read");
        assert_eq!(
            tool_calls[0]["function"]["arguments"],
            r#"{"path":"/tmp/test"}"#
        );
    }

    #[test]
    fn request_body_maps_tool_result_as_tool_role_message() {
        let req = request_with_tool_result();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages");
        // The tool result is the third message (index 2).
        let tool_msg = &messages[2];
        assert_eq!(tool_msg["role"], "tool");
        assert_eq!(tool_msg["tool_call_id"], "call_abc123");
        assert_eq!(tool_msg["content"], "file contents here");
    }

    // -----------------------------------------------------------------------
    // Response deserialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn deserialize_text_response() {
        let json = sample_text_response_json();
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&json).expect("parse response");

        assert_eq!(parsed.id, "chatcmpl-abc123");
        assert_eq!(parsed.choices.len(), 1);
        assert_eq!(
            parsed.choices[0].message.content.as_deref(),
            Some("Hello! How can I help you today?")
        );
        assert_eq!(
            parsed.choices[0].finish_reason.as_deref(),
            Some("stop")
        );
        let usage = parsed.usage.as_ref().expect("usage");
        assert_eq!(usage.prompt_tokens, 25);
        assert_eq!(usage.completion_tokens, 12);
        assert_eq!(usage.total_tokens, 37);
    }

    #[test]
    fn deserialize_tool_use_response() {
        let json = sample_tool_use_response_json();
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&json).expect("parse response");

        assert_eq!(parsed.choices.len(), 1);
        assert!(parsed.choices[0].message.content.is_none());
        let tool_calls = parsed.choices[0]
            .message
            .tool_calls
            .as_ref()
            .expect("tool_calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_xyz789");
        assert_eq!(tool_calls[0].call_type, "function");
        assert_eq!(tool_calls[0].function.name, "file_read");
        assert_eq!(
            tool_calls[0].function.arguments,
            r#"{"path":"/tmp/test.txt"}"#
        );
        assert_eq!(
            parsed.choices[0].finish_reason.as_deref(),
            Some("tool_calls")
        );
    }

    #[test]
    fn deserialize_multiple_choices() {
        let json = sample_multiple_choices_response_json();
        let parsed: ChatCompletionResponse = serde_json::from_str(&json).expect("parse");

        assert_eq!(parsed.choices.len(), 2);
        assert_eq!(
            parsed.choices[0].message.content.as_deref(),
            Some("First choice.")
        );
        assert_eq!(
            parsed.choices[1].message.content.as_deref(),
            Some("Second choice.")
        );
    }

    #[test]
    fn to_inference_response_maps_text_correctly() {
        let json = sample_text_response_json();
        let parsed: ChatCompletionResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.text, "Hello! How can I help you today?");
        assert!(response.tool_calls.is_empty());
        assert_eq!(response.stop_reason, "end_turn");
        assert_eq!(
            response.provider_request_id.as_deref(),
            Some("chatcmpl-abc123")
        );
    }

    #[test]
    fn to_inference_response_maps_tool_calls() {
        let json = sample_tool_use_response_json();
        let parsed: ChatCompletionResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert!(response.text.is_empty());
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].tool_call_id, "call_xyz789");
        assert_eq!(response.tool_calls[0].tool_name, "file_read");
        assert_eq!(response.stop_reason, "tool_use");
    }

    #[test]
    fn to_inference_response_maps_tool_calls_with_text() {
        let json = sample_tool_use_with_text_response_json();
        let parsed: ChatCompletionResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.text, "I'll read that file for you.");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].tool_name, "file_read");
        assert_eq!(response.stop_reason, "tool_use");
    }

    #[test]
    fn to_inference_response_uses_first_choice() {
        let json = sample_multiple_choices_response_json();
        let parsed: ChatCompletionResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        // Should use the first choice.
        assert_eq!(response.text, "First choice.");
    }

    #[test]
    fn token_usage_extraction() {
        let json = sample_text_response_json();
        let parsed: ChatCompletionResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.usage.input_tokens, 25);
        assert_eq!(response.usage.output_tokens, 12);
        assert!(response.usage.cache_read_tokens.is_none());
        assert!(response.usage.cache_write_tokens.is_none());
    }

    #[test]
    fn tool_call_arguments_json_is_valid() {
        let json = sample_tool_use_response_json();
        let parsed: ChatCompletionResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        let args: serde_json::Value =
            serde_json::from_str(&response.tool_calls[0].arguments_json)
                .expect("valid JSON");
        assert_eq!(args["path"], "/tmp/test.txt");
    }

    // -----------------------------------------------------------------------
    // Finish reason mapping tests
    // -----------------------------------------------------------------------

    #[test]
    fn finish_reason_stop_maps_to_end_turn() {
        assert_eq!(map_finish_reason("stop"), "end_turn");
    }

    #[test]
    fn finish_reason_length_maps_to_max_tokens() {
        assert_eq!(map_finish_reason("length"), "max_tokens");
    }

    #[test]
    fn finish_reason_tool_calls_maps_to_tool_use() {
        assert_eq!(map_finish_reason("tool_calls"), "tool_use");
    }

    #[test]
    fn finish_reason_content_filter_maps_to_end_turn() {
        assert_eq!(map_finish_reason("content_filter"), "end_turn");
    }

    #[test]
    fn finish_reason_unknown_passes_through() {
        assert_eq!(map_finish_reason("custom_reason"), "custom_reason");
    }

    // -----------------------------------------------------------------------
    // Error mapping tests
    // -----------------------------------------------------------------------

    #[test]
    fn error_mapping_401_authentication() {
        let body = sample_error_response_json(
            "invalid_api_key",
            "Incorrect API key provided.",
        );
        let err = map_api_error(401, &body);
        assert!(matches!(err, ExecutorError::Authentication { .. }));
        assert!(!err.is_retryable());
    }

    #[test]
    fn error_mapping_429_rate_limit() {
        let body = sample_error_response_json(
            "rate_limit_exceeded",
            "Rate limit reached for model",
        );
        let err = map_api_error(429, &body);
        assert!(matches!(err, ExecutorError::RateLimit { .. }));
        assert!(err.is_retryable());
    }

    #[test]
    fn error_mapping_400_bad_request() {
        let body = sample_error_response_json(
            "invalid_request_error",
            "messages is a required field",
        );
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
        let body = r#"{"error":{"message":"This model's maximum context length is 8192 tokens","type":"invalid_request_error","code":"context_length_exceeded"}}"#;
        let err = map_api_error(400, body);
        assert!(matches!(err, ExecutorError::ContextWindowExceeded { .. }));
    }

    #[test]
    fn error_mapping_400_context_window_alternative_message() {
        let body = r#"{"error":{"message":"maximum context length exceeded","type":"invalid_request_error","code":null}}"#;
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
        let body =
            sample_error_response_json("server_error", "internal server error");
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
        let body =
            sample_error_response_json("server_error", "service unavailable");
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
    fn parse_sse_text_stream() {
        let raw = sample_sse_text_stream();
        let chunks = parse_sse_chunks(&raw);

        // 6 data lines, one is [DONE] which is skipped = 5 chunks.
        assert_eq!(chunks.len(), 5);
    }

    #[test]
    fn parse_sse_tool_stream() {
        let raw = sample_sse_tool_stream();
        let chunks = parse_sse_chunks(&raw);

        // 6 data lines, one is [DONE] which is skipped = 5 chunks.
        assert_eq!(chunks.len(), 5);
    }

    #[test]
    fn parse_sse_done_marker_skipped() {
        let raw = "data: [DONE]\n";
        let chunks = parse_sse_chunks(raw);
        assert!(chunks.is_empty());
    }

    #[test]
    fn parse_sse_ignores_non_data_lines() {
        let raw = "event: something\n: comment\ndata: [DONE]\n";
        let chunks = parse_sse_chunks(raw);
        assert!(chunks.is_empty());
    }

    #[test]
    fn parse_sse_ignores_invalid_json() {
        let raw = "data: {invalid json}\n";
        let chunks = parse_sse_chunks(raw);
        assert!(chunks.is_empty());
    }

    // -----------------------------------------------------------------------
    // SSE processing tests
    // -----------------------------------------------------------------------

    #[test]
    fn process_sse_text_stream_produces_correct_events() {
        let raw = sample_sse_text_stream();
        let chunks = parse_sse_chunks(&raw);
        let events = process_sse_chunks(chunks);

        // Should have: TextDelta("Hello"), TextDelta(" world"), UsageUpdate, Completed
        // (The empty content delta at the start is skipped.)
        let mut text_deltas = Vec::new();
        let mut has_usage_update = false;
        let mut has_completed = false;

        for event in &events {
            match event.as_ref().expect("no error") {
                StreamEvent::TextDelta { delta } => text_deltas.push(delta.clone()),
                StreamEvent::UsageUpdate { .. } => has_usage_update = true,
                StreamEvent::Completed { result } => {
                    has_completed = true;
                    assert_eq!(result.text, "Hello world");
                    assert_eq!(result.stop_reason, "end_turn");
                    assert_eq!(result.usage.input_tokens, 20);
                    assert_eq!(result.usage.output_tokens, 5);
                }
                _ => {}
            }
        }

        assert_eq!(text_deltas, vec!["Hello", " world"]);
        assert!(has_usage_update);
        assert!(has_completed);
    }

    #[test]
    fn process_sse_tool_stream_produces_correct_events() {
        let raw = sample_sse_tool_stream();
        let chunks = parse_sse_chunks(&raw);
        let events = process_sse_chunks(chunks);

        let mut tool_deltas = Vec::new();
        let mut has_tool_complete = false;
        let mut has_completed = false;

        for event in &events {
            match event.as_ref().expect("no error") {
                StreamEvent::ToolCallDelta {
                    tool_call_id,
                    name,
                    input_delta,
                } => {
                    tool_deltas.push((
                        tool_call_id.clone(),
                        name.clone(),
                        input_delta.clone(),
                    ));
                }
                StreamEvent::ToolCallComplete { call } => {
                    has_tool_complete = true;
                    assert_eq!(call.tool_call_id, "call_abc123");
                    assert_eq!(call.tool_name, "file_read");
                    assert_eq!(
                        call.arguments_json,
                        r#"{"path":"/tmp/test"}"#
                    );
                }
                StreamEvent::Completed { result } => {
                    has_completed = true;
                    assert_eq!(result.stop_reason, "tool_use");
                    assert_eq!(result.tool_calls.len(), 1);
                    assert_eq!(result.usage.input_tokens, 30);
                    assert_eq!(result.usage.output_tokens, 15);
                }
                _ => {}
            }
        }

        // First delta should have name "file_read".
        assert!(!tool_deltas.is_empty());
        assert_eq!(tool_deltas[0].1, Some("file_read".to_string()));

        assert!(has_tool_complete);
        assert!(has_completed);
    }

    // -----------------------------------------------------------------------
    // Builder / constructor tests
    // -----------------------------------------------------------------------

    #[test]
    fn new_creates_executor_with_defaults() {
        let exec = OpenAiExecutor::new("sk-test".into(), "gpt-4o".into());
        assert_eq!(exec.api_key, "sk-test");
        assert_eq!(exec.model, "gpt-4o");
        assert_eq!(exec.base_url, "https://api.openai.com");
        assert_eq!(exec.max_retries, 3);
    }

    #[test]
    fn with_base_url_overrides_default() {
        let exec = OpenAiExecutor::new_builder("sk-test".into(), "gpt-4o".into())
            .with_base_url("http://localhost:11434/v1".into());
        assert_eq!(exec.base_url, "http://localhost:11434/v1");
    }

    #[test]
    fn with_max_retries_overrides_default() {
        let exec = OpenAiExecutor::new_builder("sk-test".into(), "gpt-4o".into())
            .with_max_retries(5);
        assert_eq!(exec.max_retries, 5);
    }

    #[test]
    fn with_timeout_creates_new_client() {
        let exec = OpenAiExecutor::new_builder("sk-test".into(), "gpt-4o".into())
            .with_timeout(Duration::from_secs(30));
        // Just verify it doesn't panic and produces a valid executor.
        assert_eq!(exec.model, "gpt-4o");
    }

    #[test]
    fn builder_chain_works() {
        let exec = OpenAiExecutor::new_builder("sk-test".into(), "gpt-4o".into())
            .with_base_url("http://localhost:8080".into())
            .with_max_retries(1)
            .with_timeout(Duration::from_secs(60))
            .build();

        assert_eq!(exec.base_url, "http://localhost:8080");
        assert_eq!(exec.max_retries, 1);
    }

    #[test]
    fn new_builder_returns_non_arc() {
        let exec = OpenAiExecutor::new_builder("key".into(), "model".into());
        assert_eq!(exec.api_key, "key");
        assert_eq!(exec.model, "model");
    }

    #[test]
    fn build_returns_arc() {
        let exec =
            OpenAiExecutor::new_builder("key".into(), "model".into()).build();
        // Verify it's an Arc by cloning (Arc implements Clone).
        let _clone = Arc::clone(&exec);
        assert_eq!(exec.model, "model");
    }

    /// Compile-time check: `OpenAiExecutor` implements `ModelExecutor`.
    #[allow(dead_code)]
    fn _openai_executor_implements_model_executor(e: &OpenAiExecutor) {
        let _: &dyn ModelExecutor = e;
    }

    // -----------------------------------------------------------------------
    // Message conversion edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn assistant_message_with_text_and_tool_use() {
        let msg = InferenceMessage {
            role: MessageRole::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "Let me check.".into(),
                },
                ContentBlock::ToolUse {
                    tool_call_id: "call_001".into(),
                    tool_name: "lookup".into(),
                    arguments_json: r#"{"q":"test"}"#.into(),
                },
            ],
        };

        let api_msgs = to_api_messages(&msg);
        assert_eq!(api_msgs.len(), 1);
        assert_eq!(api_msgs[0].role, "assistant");
        assert_eq!(api_msgs[0].content.as_deref(), Some("Let me check."));
        let tool_calls = api_msgs[0].tool_calls.as_ref().expect("tool_calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "lookup");
    }

    #[test]
    fn user_message_with_tool_result_becomes_tool_role() {
        let msg = InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_call_id: "call_001".into(),
                content: "result data".into(),
                is_error: false,
            }],
        };

        let api_msgs = to_api_messages(&msg);
        assert_eq!(api_msgs.len(), 1);
        assert_eq!(api_msgs[0].role, "tool");
        assert_eq!(api_msgs[0].content.as_deref(), Some("result data"));
        assert_eq!(api_msgs[0].tool_call_id.as_deref(), Some("call_001"));
    }

    #[test]
    fn assistant_tool_use_without_text_has_null_content() {
        let msg = InferenceMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                tool_call_id: "call_002".into(),
                tool_name: "search".into(),
                arguments_json: "{}".into(),
            }],
        };

        let api_msgs = to_api_messages(&msg);
        assert_eq!(api_msgs.len(), 1);
        assert!(api_msgs[0].content.is_none());
    }
}

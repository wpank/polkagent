//! Local model executor adapter for Ollama and OpenAI-compatible local APIs.
//!
//! This crate provides [`LocalExecutor`] -- a thin adapter that translates the
//! provider-agnostic [`InferenceRequest`] into the OpenAI Chat Completions wire
//! format, targeting Ollama's OpenAI-compatible endpoint or any other local
//! server that speaks the same protocol.
//!
//! # Key differences from cloud executors
//!
//! - **No API key required.** Local models do not need authentication.
//! - **Longer default timeout.** Local models can be slow, so the default
//!   timeout is 120 seconds (configurable via [`LocalExecutor::with_timeout`]).
//! - **Graceful handling of missing token usage.** Many local models do not
//!   report token consumption; this adapter zero-fills usage fields when absent.
//! - **No retry logic.** Local models are not subject to rate limits.
//! - **Connection-refused is normal.** The health check returns a clear error
//!   when the local server is not running rather than treating it as a
//!   transport failure.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use polkagent_executor_local::LocalExecutor;
//!
//! // Connect to Ollama running locally with the llama3.2 model.
//! let executor = LocalExecutor::ollama("llama3.2".to_string());
//!
//! // Or connect to a custom OpenAI-compatible server.
//! let executor = LocalExecutor::custom(
//!     "http://my-server:8080/v1".to_string(),
//!     "my-model".to_string(),
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

/// Default Ollama OpenAI-compatible API base URL.
const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";

/// Default request timeout for local models (120 seconds, since local
/// inference can be slow depending on hardware).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------
// OpenAI Chat Completions API types (private)
// ---------------------------------------------------------------------------

/// Request body for the OpenAI Chat Completions API.
#[derive(Debug, Clone, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ApiMessage>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiTool>,
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

/// A tool call in the OpenAI wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ApiFunction,
}

/// The function description within a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiFunction {
    name: String,
    arguments: String,
}

/// A tool definition in the OpenAI wire format.
#[derive(Debug, Clone, Serialize)]
struct ApiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: ApiFunctionDefinition,
}

/// A function definition within a tool specification.
#[derive(Debug, Clone, Serialize)]
struct ApiFunctionDefinition {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// Top-level response from the OpenAI Chat Completions API.
#[derive(Debug, Clone, Deserialize)]
struct ChatCompletionResponse {
    id: String,
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<ApiUsage>,
}

/// A single choice in the Chat Completions response.
#[derive(Debug, Clone, Deserialize)]
struct Choice {
    message: ChoiceMessage,
    finish_reason: Option<String>,
}

/// The message content within a choice.
#[derive(Debug, Clone, Deserialize)]
struct ChoiceMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ApiToolCall>>,
}

/// Token usage from the OpenAI API response.
#[derive(Debug, Clone, Deserialize)]
struct ApiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

/// Error response body from OpenAI-compatible APIs.
#[derive(Debug, Clone, Deserialize)]
struct ApiErrorResponse {
    error: ApiErrorDetail,
}

/// Inner error detail from OpenAI-compatible APIs.
#[derive(Debug, Clone, Deserialize)]
struct ApiErrorDetail {
    message: String,
    #[serde(default)]
    #[allow(dead_code)]
    code: Option<String>,
}

// ---------------------------------------------------------------------------
// SSE streaming types for OpenAI Chat Completions
// ---------------------------------------------------------------------------

/// A single SSE chunk from the streaming Chat Completions API.
#[derive(Debug, Clone, Deserialize)]
struct StreamChunk {
    #[allow(dead_code)]
    id: Option<String>,
    choices: Vec<StreamChoice>,
    #[serde(default)]
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
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<StreamToolCall>>,
}

/// A tool call delta within a streaming chunk.
#[derive(Debug, Clone, Deserialize)]
struct StreamToolCall {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamFunction>,
}

/// A function delta within a streaming tool call.
#[derive(Debug, Clone, Deserialize)]
struct StreamFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a provider-agnostic `InferenceMessage` to the OpenAI wire format.
fn to_api_message(msg: &InferenceMessage, system: Option<&str>) -> Vec<ApiMessage> {
    let mut result = Vec::new();

    // If there is a system prompt, inject it as the first message.
    // (Only used for the very first message in the conversation.)
    if let Some(sys) = system {
        result.push(ApiMessage {
            role: "system".to_string(),
            content: Some(sys.to_string()),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    let role = match msg.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    };

    // Collect text content and tool calls / tool results from blocks.
    let mut text_parts: Vec<String> = Vec::new();
    let mut api_tool_calls: Vec<ApiToolCall> = Vec::new();
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
                api_tool_calls.push(ApiToolCall {
                    id: tool_call_id.clone(),
                    call_type: "function".to_string(),
                    function: ApiFunction {
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

    // For assistant messages with tool calls, emit a single message.
    if role == "assistant" && !api_tool_calls.is_empty() {
        result.push(ApiMessage {
            role: "assistant".to_string(),
            content: if text_parts.is_empty() {
                None
            } else {
                Some(text_parts.join(""))
            },
            tool_calls: Some(api_tool_calls),
            tool_call_id: None,
        });
        return result;
    }

    // For user messages with tool results, emit one "tool" message per result.
    if !tool_results.is_empty() {
        for (tool_call_id, content, is_error) in tool_results {
            let output = if is_error {
                format!("[ERROR] {content}")
            } else {
                content
            };
            result.push(ApiMessage {
                role: "tool".to_string(),
                content: Some(output),
                tool_calls: None,
                tool_call_id: Some(tool_call_id),
            });
        }
        return result;
    }

    // Default: a plain text message.
    result.push(ApiMessage {
        role: role.to_string(),
        content: if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        },
        tool_calls: None,
        tool_call_id: None,
    });

    result
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
        function: ApiFunctionDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters,
        },
    }
}

/// Build the API request body from an `InferenceRequest`.
fn build_request_body(request: &InferenceRequest, stream: bool) -> ChatCompletionRequest {
    let mut messages: Vec<ApiMessage> = Vec::new();

    for (i, msg) in request.messages.iter().enumerate() {
        let system = if i == 0 {
            request.system.as_deref()
        } else {
            None
        };
        messages.extend(to_api_message(msg, system));
    }

    // If there are no messages but there is a system prompt, add it standalone.
    if messages.is_empty() {
        if let Some(ref sys) = request.system {
            messages.push(ApiMessage {
                role: "system".to_string(),
                content: Some(sys.clone()),
                tool_calls: None,
                tool_call_id: None,
            });
        }
    }

    ChatCompletionRequest {
        model: request.model_id.clone(),
        messages,
        max_tokens: request.max_tokens,
        temperature: request.temperature,
        tools: request.tools.iter().map(to_api_tool).collect(),
        stream: if stream { Some(true) } else { None },
    }
}

/// Convert an optional `ApiUsage` to the trait-level `TokenUsage`.
///
/// Local models may not report usage at all, so this gracefully defaults to
/// zero when usage data is unavailable.
fn to_token_usage(api_usage: Option<&ApiUsage>) -> TokenUsage {
    match api_usage {
        Some(u) => TokenUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        None => TokenUsage::default(),
    }
}

/// Map an OpenAI finish_reason to a normalized stop reason.
fn normalize_stop_reason(finish_reason: Option<&str>) -> String {
    match finish_reason {
        Some("stop") => "end_turn".to_string(),
        Some("length") => "max_tokens".to_string(),
        Some("tool_calls") => "tool_use".to_string(),
        Some(other) => other.to_string(),
        None => "end_turn".to_string(),
    }
}

/// Convert a `ChatCompletionResponse` to the trait-level `InferenceResponse`.
fn to_inference_response(resp: &ChatCompletionResponse) -> InferenceResponse {
    let choice = resp.choices.first();

    let text = choice
        .and_then(|c| c.message.content.as_deref())
        .unwrap_or("")
        .to_string();

    let tool_calls: Vec<ToolCall> = choice
        .and_then(|c| c.message.tool_calls.as_ref())
        .map(|calls| {
            calls
                .iter()
                .map(|tc| ToolCall {
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.function.name.clone(),
                    arguments_json: tc.function.arguments.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    let stop_reason =
        normalize_stop_reason(choice.and_then(|c| c.finish_reason.as_deref()));

    InferenceResponse {
        text,
        tool_calls,
        stop_reason,
        usage: to_token_usage(resp.usage.as_ref()),
        provider_request_id: Some(resp.id.clone()),
    }
}

/// Map an HTTP status code and optional error body to an `ExecutorError`.
fn map_api_error(status: u16, body: &str) -> ExecutorError {
    let detail = serde_json::from_str::<ApiErrorResponse>(body)
        .map(|e| e.error.message)
        .unwrap_or_else(|_| body.to_string());

    match status {
        400 => {
            if body.contains("context_length_exceeded")
                || body.contains("too many tokens")
                || body.contains("maximum context length")
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
        404 => ExecutorError::Transport {
            message: format!("model or endpoint not found: {detail}"),
            retryable: false,
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

/// Parse SSE data lines from a streaming response body.
///
/// Returns a list of parsed `StreamChunk` values. Lines that are not valid
/// `data:` lines, `[DONE]` markers, or unparseable JSON are silently skipped.
fn parse_sse_chunks(body: &str) -> Vec<StreamChunk> {
    let mut chunks = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                continue;
            }
            match serde_json::from_str::<StreamChunk>(data) {
                Ok(chunk) => chunks.push(chunk),
                Err(e) => {
                    debug!(data = data, error = %e, "skipping unparseable SSE data line");
                }
            }
        }
    }
    chunks
}

/// Process raw SSE chunks into a list of `StreamEvent` values.
///
/// This function handles text deltas, tool call assembly from partial deltas,
/// usage tracking, and produces the terminal `Completed` event.
fn process_sse_chunks(chunks: Vec<StreamChunk>) -> Vec<Result<StreamEvent, ExecutorError>> {
    let mut stream_events: Vec<Result<StreamEvent, ExecutorError>> = Vec::new();
    let mut accumulated_text = String::new();

    // Track tool calls being assembled. Indexed by tool call index (from the
    // streaming API), each entry stores (id, name, accumulated_arguments).
    let mut tool_call_builders: Vec<(String, String, String)> = Vec::new();
    let mut usage = TokenUsage::default();
    let mut stop_reason = String::from("end_turn");

    for chunk in &chunks {
        // Capture usage if present.
        if let Some(ref u) = chunk.usage {
            usage = to_token_usage(Some(u));
        }

        for choice in &chunk.choices {
            // Track finish_reason.
            if let Some(ref reason) = choice.finish_reason {
                stop_reason = normalize_stop_reason(Some(reason));
            }

            // Process text deltas.
            if let Some(ref content) = choice.delta.content {
                if !content.is_empty() {
                    accumulated_text.push_str(content);
                    stream_events.push(Ok(StreamEvent::TextDelta {
                        delta: content.clone(),
                    }));
                }
            }

            // Process tool call deltas.
            if let Some(ref tool_calls) = choice.delta.tool_calls {
                for tc in tool_calls {
                    let idx = tc.index;

                    // Extend builder vector if needed.
                    while tool_call_builders.len() <= idx {
                        tool_call_builders.push((String::new(), String::new(), String::new()));
                    }

                    // If this chunk provides an id, store it.
                    if let Some(ref id) = tc.id {
                        tool_call_builders[idx].0 = id.clone();
                    }

                    // If this chunk provides a function name, store it and
                    // emit the initial ToolCallDelta with the name.
                    let mut emitted_name: Option<String> = None;
                    if let Some(ref func) = tc.function {
                        if let Some(ref name) = func.name {
                            tool_call_builders[idx].1 = name.clone();
                            emitted_name = Some(name.clone());
                        }
                        if let Some(ref args) = func.arguments {
                            tool_call_builders[idx].2.push_str(args);
                        }
                    }

                    let args_delta = tc
                        .function
                        .as_ref()
                        .and_then(|f| f.arguments.as_deref())
                        .unwrap_or("")
                        .to_string();

                    stream_events.push(Ok(StreamEvent::ToolCallDelta {
                        tool_call_id: tool_call_builders[idx].0.clone(),
                        name: emitted_name,
                        input_delta: args_delta,
                    }));
                }
            }
        }
    }

    // Finalize all assembled tool calls.
    let mut completed_tool_calls: Vec<ToolCall> = Vec::new();
    for (id, name, arguments) in &tool_call_builders {
        if !id.is_empty() || !name.is_empty() {
            let call = ToolCall {
                tool_call_id: id.clone(),
                tool_name: name.clone(),
                arguments_json: arguments.clone(),
            };
            stream_events.push(Ok(StreamEvent::ToolCallComplete { call: call.clone() }));
            completed_tool_calls.push(call);
        }
    }

    // Emit usage update.
    stream_events.push(Ok(StreamEvent::UsageUpdate {
        usage: usage.clone(),
    }));

    // Emit the terminal Completed event.
    let response = InferenceResponse {
        text: accumulated_text,
        tool_calls: completed_tool_calls,
        stop_reason,
        usage,
        provider_request_id: None,
    };
    stream_events.push(Ok(StreamEvent::Completed { result: response }));

    stream_events
}

// ---------------------------------------------------------------------------
// LocalExecutor
// ---------------------------------------------------------------------------

/// A [`ModelExecutor`] adapter for local models via Ollama or any
/// OpenAI-compatible local API.
///
/// This executor requires no API key and uses longer timeouts to accommodate
/// the typically slower inference speed of local models.
pub struct LocalExecutor {
    client: Client,
    model: String,
    base_url: String,
    timeout: Duration,
}

impl LocalExecutor {
    /// Create a `LocalExecutor` targeting Ollama at the default local endpoint
    /// (`http://localhost:11434/v1`).
    ///
    /// # Arguments
    ///
    /// * `model` - The model name as known to Ollama (e.g., `"llama3.2"`,
    ///   `"mistral"`, `"codellama"`).
    pub fn ollama(model: String) -> Arc<Self> {
        Self::custom(DEFAULT_OLLAMA_BASE_URL.to_string(), model)
    }

    /// Create a `LocalExecutor` targeting any OpenAI-compatible local API.
    ///
    /// # Arguments
    ///
    /// * `base_url` - The API base URL (e.g., `"http://localhost:8080/v1"`).
    ///   Should include the `/v1` path prefix if the server expects it.
    /// * `model` - The model identifier expected by the server.
    pub fn custom(base_url: String, model: String) -> Arc<Self> {
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());

        Arc::new(Self {
            client,
            model,
            base_url,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Construct a raw (non-Arc) executor for builder chaining.
    ///
    /// Call [`build`](Self::build) after applying builder methods to obtain
    /// an `Arc<Self>`.
    pub fn new_builder(base_url: String, model: String) -> Self {
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            model,
            base_url,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Construct a raw (non-Arc) executor for builder chaining targeting
    /// Ollama at the default endpoint.
    pub fn ollama_builder(model: String) -> Self {
        Self::new_builder(DEFAULT_OLLAMA_BASE_URL.to_string(), model)
    }

    /// Override the HTTP request timeout.
    ///
    /// The default is 120 seconds, which accommodates slow local models.
    /// Increase this for very large models or reduce it for faster models.
    #[must_use]
    pub fn with_timeout(mut self, duration: Duration) -> Self {
        self.timeout = duration;
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

    /// Execute a non-streaming HTTP request.
    async fn execute_request(
        &self,
        body: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ExecutorError> {
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ExecutorError::Timeout {
                        elapsed_ms: self.timeout.as_millis() as u64,
                    }
                } else if e.is_connect() {
                    ExecutorError::Transport {
                        message: format!(
                            "connection refused: is the local model server running at {}? ({})",
                            self.base_url, e
                        ),
                        retryable: false,
                    }
                } else {
                    ExecutorError::Transport {
                        message: format!("HTTP transport error: {e}"),
                        retryable: false,
                    }
                }
            })?;

        let status = response.status().as_u16();
        if status == 200 {
            let response_body =
                response.text().await.map_err(|e| ExecutorError::InvalidResponse {
                    message: format!("failed to read response body: {e}"),
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
        Err(map_api_error(status, &error_body))
    }

    /// Execute a streaming request and collect SSE chunks into `StreamEvent`s.
    async fn execute_streaming(
        &self,
        body: &ChatCompletionRequest,
    ) -> Result<Vec<Result<StreamEvent, ExecutorError>>, ExecutorError> {
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ExecutorError::Timeout {
                        elapsed_ms: self.timeout.as_millis() as u64,
                    }
                } else if e.is_connect() {
                    ExecutorError::Transport {
                        message: format!(
                            "connection refused: is the local model server running at {}? ({})",
                            self.base_url, e
                        ),
                        retryable: false,
                    }
                } else {
                    ExecutorError::Transport {
                        message: format!("HTTP transport error: {e}"),
                        retryable: false,
                    }
                }
            })?;

        let status = response.status().as_u16();
        if status != 200 {
            let error_body = response.text().await.unwrap_or_default();
            return Err(map_api_error(status, &error_body));
        }

        let full_body = response.text().await.map_err(|e| ExecutorError::InvalidResponse {
            message: format!("failed to read SSE stream body: {e}"),
        })?;

        let chunks = parse_sse_chunks(&full_body);
        Ok(process_sse_chunks(chunks))
    }
}

// ---------------------------------------------------------------------------
// ModelExecutor implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ModelExecutor for LocalExecutor {
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
            "executing local inference"
        );

        let body = build_request_body(&request, false);
        let api_response = self.execute_request(&body).await?;
        let response = to_inference_response(&api_response);

        debug!(
            input_tokens = response.usage.input_tokens,
            output_tokens = response.usage.output_tokens,
            stop_reason = %response.stop_reason,
            "local inference complete"
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
            "starting local streaming inference"
        );

        let body = build_request_body(&request, true);
        let events = self.execute_streaming(&body).await?;

        Ok(Box::new(stream::iter(events)))
    }

    async fn health(&self) -> Result<(), ExecutorError> {
        // Try Ollama-specific endpoint first, then fall back to generic
        // OpenAI-compatible models endpoint.
        let ollama_url = self.base_url.replace("/v1", "/api/tags");
        let generic_url = format!("{}/models", self.base_url);

        // First try the Ollama-specific endpoint.
        let result = self.client.get(&ollama_url).send().await;

        match result {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(_) => {
                // Ollama endpoint returned non-success; try generic.
            }
            Err(e) if e.is_connect() => {
                return Err(ExecutorError::Transport {
                    message: format!(
                        "local model server not reachable at {}: connection refused",
                        self.base_url
                    ),
                    retryable: false,
                });
            }
            Err(e) if e.is_timeout() => {
                return Err(ExecutorError::Timeout {
                    elapsed_ms: self.timeout.as_millis() as u64,
                });
            }
            Err(_) => {
                // Try generic endpoint as fallback.
            }
        }

        // Try generic OpenAI-compatible models endpoint.
        let result = self.client.get(&generic_url).send().await;

        match result {
            Ok(resp) if resp.status().is_success() => Ok(()),
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                warn!(status, "health check failed on generic endpoint");
                Err(map_api_error(status, &body))
            }
            Err(e) if e.is_connect() => Err(ExecutorError::Transport {
                message: format!(
                    "local model server not reachable at {}: connection refused",
                    self.base_url
                ),
                retryable: false,
            }),
            Err(e) if e.is_timeout() => Err(ExecutorError::Timeout {
                elapsed_ms: self.timeout.as_millis() as u64,
            }),
            Err(e) => Err(ExecutorError::Transport {
                message: format!("health check failed: {e}"),
                retryable: false,
            }),
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
            model_id: "llama3.2".into(),
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
            model_id: "llama3.2".into(),
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
                        tool_call_id: "call_001".into(),
                        tool_name: "file_read".into(),
                        arguments_json: r#"{"path":"/tmp/test"}"#.into(),
                    }],
                },
                InferenceMessage {
                    role: MessageRole::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_call_id: "call_001".into(),
                        content: "file contents here".into(),
                        is_error: false,
                    }],
                },
            ],
            system: None,
            tools: vec![],
            model_id: "llama3.2".into(),
            max_tokens: 1024,
            temperature: None,
        }
    }

    fn sample_text_response_json() -> String {
        serde_json::json!({
            "id": "chatcmpl-local-001",
            "object": "chat.completion",
            "model": "llama3.2",
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
            "id": "chatcmpl-local-002",
            "object": "chat.completion",
            "model": "llama3.2",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_abc123",
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
                "completion_tokens": 20,
                "total_tokens": 70
            }
        })
        .to_string()
    }

    fn sample_response_no_usage_json() -> String {
        serde_json::json!({
            "id": "chatcmpl-local-003",
            "object": "chat.completion",
            "model": "llama3.2",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hello!"
                    },
                    "finish_reason": "stop"
                }
            ]
        })
        .to_string()
    }

    fn sample_sse_text_stream() -> String {
        [
            r#"data: {"id":"chatcmpl-stream-01","object":"chat.completion.chunk","model":"llama3.2","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
            r#"data: {"id":"chatcmpl-stream-01","object":"chat.completion.chunk","model":"llama3.2","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#,
            r#"data: {"id":"chatcmpl-stream-01","object":"chat.completion.chunk","model":"llama3.2","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":null}]}"#,
            r#"data: {"id":"chatcmpl-stream-01","object":"chat.completion.chunk","model":"llama3.2","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#,
            "data: [DONE]",
        ]
        .join("\n")
    }

    fn sample_sse_tool_stream() -> String {
        [
            r#"data: {"id":"chatcmpl-stream-02","object":"chat.completion.chunk","model":"llama3.2","choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_stream_001","type":"function","function":{"name":"file_read","arguments":""}}]},"finish_reason":null}]}"#,
            r#"data: {"id":"chatcmpl-stream-02","object":"chat.completion.chunk","model":"llama3.2","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"pa"}}]},"finish_reason":null}]}"#,
            r#"data: {"id":"chatcmpl-stream-02","object":"chat.completion.chunk","model":"llama3.2","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"/tmp/test\"}"}}]},"finish_reason":null}]}"#,
            r#"data: {"id":"chatcmpl-stream-02","object":"chat.completion.chunk","model":"llama3.2","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
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

        assert_eq!(json["model"], "llama3.2");
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
    fn request_body_omits_system_when_none() {
        let mut req = minimal_request();
        req.system = None;
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages array");
        // First message should be user, not system.
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
        // First message is system, second is user.
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

        assert!(json.get("tools").is_none());
    }

    #[test]
    fn request_body_maps_tool_use_in_assistant_message() {
        let req = request_with_tool_result();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages");
        // Find the assistant message (skip system if present).
        let assistant_msg = messages
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("assistant message");
        assert_eq!(assistant_msg["tool_calls"][0]["id"], "call_001");
        assert_eq!(assistant_msg["tool_calls"][0]["type"], "function");
        assert_eq!(
            assistant_msg["tool_calls"][0]["function"]["name"],
            "file_read"
        );
        assert_eq!(
            assistant_msg["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"/tmp/test"}"#
        );
    }

    #[test]
    fn request_body_maps_tool_result_as_tool_message() {
        let req = request_with_tool_result();
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages");
        // Find the tool message.
        let tool_msg = messages
            .iter()
            .find(|m| m["role"] == "tool")
            .expect("tool message");
        assert_eq!(tool_msg["tool_call_id"], "call_001");
        assert_eq!(tool_msg["content"], "file contents here");
    }

    #[test]
    fn request_body_maps_tool_error_result() {
        let mut req = request_with_tool_result();
        if let ContentBlock::ToolResult { is_error, .. } = &mut req.messages[2].content[0] {
            *is_error = true;
        }
        let body = build_request_body(&req, false);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages");
        let tool_msg = messages
            .iter()
            .find(|m| m["role"] == "tool")
            .expect("tool message");
        let content = tool_msg["content"].as_str().expect("content string");
        assert!(content.starts_with("[ERROR]"));
    }

    // -----------------------------------------------------------------------
    // Response deserialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn deserialize_text_response() {
        let json = sample_text_response_json();
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&json).expect("parse response");

        assert_eq!(parsed.id, "chatcmpl-local-001");
        assert_eq!(parsed.choices.len(), 1);
        assert_eq!(
            parsed.choices[0].message.content.as_deref(),
            Some("Hello! How can I help you today?")
        );
        assert_eq!(
            parsed.choices[0].finish_reason.as_deref(),
            Some("stop")
        );
    }

    #[test]
    fn deserialize_tool_use_response() {
        let json = sample_tool_use_response_json();
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&json).expect("parse response");

        assert_eq!(parsed.choices.len(), 1);
        let tool_calls = parsed.choices[0]
            .message
            .tool_calls
            .as_ref()
            .expect("tool_calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_abc123");
        assert_eq!(tool_calls[0].function.name, "file_read");
    }

    #[test]
    fn deserialize_response_with_usage() {
        let json = sample_text_response_json();
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&json).expect("parse");

        let usage = parsed.usage.expect("usage present");
        assert_eq!(usage.prompt_tokens, 25);
        assert_eq!(usage.completion_tokens, 12);
    }

    #[test]
    fn deserialize_response_without_usage() {
        let json = sample_response_no_usage_json();
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&json).expect("parse");

        assert!(parsed.usage.is_none());
    }

    #[test]
    fn to_inference_response_maps_text_correctly() {
        let json = sample_text_response_json();
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.text, "Hello! How can I help you today?");
        assert!(response.tool_calls.is_empty());
        assert_eq!(response.stop_reason, "end_turn");
        assert_eq!(
            response.provider_request_id.as_deref(),
            Some("chatcmpl-local-001")
        );
    }

    #[test]
    fn to_inference_response_maps_tool_calls() {
        let json = sample_tool_use_response_json();
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].tool_call_id, "call_abc123");
        assert_eq!(response.tool_calls[0].tool_name, "file_read");
        assert_eq!(response.stop_reason, "tool_use");
    }

    #[test]
    fn to_inference_response_handles_missing_usage() {
        let json = sample_response_no_usage_json();
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.usage.input_tokens, 0);
        assert_eq!(response.usage.output_tokens, 0);
        assert!(response.usage.cache_read_tokens.is_none());
        assert!(response.usage.cache_write_tokens.is_none());
    }

    #[test]
    fn to_inference_response_maps_usage_when_present() {
        let json = sample_text_response_json();
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.usage.input_tokens, 25);
        assert_eq!(response.usage.output_tokens, 12);
    }

    #[test]
    fn tool_call_arguments_json_is_valid() {
        let json = sample_tool_use_response_json();
        let parsed: ChatCompletionResponse =
            serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        let args: serde_json::Value =
            serde_json::from_str(&response.tool_calls[0].arguments_json)
                .expect("valid JSON");
        assert_eq!(args["path"], "/tmp/test.txt");
    }

    // -----------------------------------------------------------------------
    // Stop reason normalization tests
    // -----------------------------------------------------------------------

    #[test]
    fn normalize_stop_maps_correctly() {
        assert_eq!(normalize_stop_reason(Some("stop")), "end_turn");
        assert_eq!(normalize_stop_reason(Some("length")), "max_tokens");
        assert_eq!(normalize_stop_reason(Some("tool_calls")), "tool_use");
        assert_eq!(normalize_stop_reason(Some("custom")), "custom");
        assert_eq!(normalize_stop_reason(None), "end_turn");
    }

    // -----------------------------------------------------------------------
    // Error mapping tests
    // -----------------------------------------------------------------------

    #[test]
    fn map_api_error_400_bad_request() {
        let error = map_api_error(400, r#"{"error":{"message":"invalid model"}}"#);
        assert!(
            matches!(error, ExecutorError::Transport { retryable: false, .. }),
            "400 should be non-retryable transport error"
        );
    }

    #[test]
    fn map_api_error_400_context_exceeded() {
        let error = map_api_error(
            400,
            r#"{"error":{"message":"maximum context length exceeded"}}"#,
        );
        assert!(
            matches!(error, ExecutorError::ContextWindowExceeded { .. }),
            "context length error should be ContextWindowExceeded"
        );
    }

    #[test]
    fn map_api_error_404_not_found() {
        let error = map_api_error(404, r#"{"error":{"message":"model not found"}}"#);
        assert!(
            matches!(error, ExecutorError::Transport { retryable: false, .. }),
            "404 should be non-retryable"
        );
    }

    #[test]
    fn map_api_error_500_server_error() {
        let error = map_api_error(500, r#"{"error":{"message":"internal error"}}"#);
        assert!(
            matches!(error, ExecutorError::Transport { retryable: true, .. }),
            "500 should be retryable"
        );
    }

    #[test]
    fn map_api_error_plain_text_body() {
        let error = map_api_error(503, "Service Unavailable");
        assert!(matches!(
            error,
            ExecutorError::Transport { retryable: true, .. }
        ));
    }

    // -----------------------------------------------------------------------
    // SSE stream parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_sse_text_stream() {
        let body = sample_sse_text_stream();
        let chunks = parse_sse_chunks(&body);

        // Should have 4 chunks (the [DONE] marker is skipped).
        assert_eq!(chunks.len(), 4);
    }

    #[test]
    fn parse_sse_done_marker_is_skipped() {
        let body = "data: [DONE]\n";
        let chunks = parse_sse_chunks(body);
        assert!(chunks.is_empty());
    }

    #[test]
    fn parse_sse_ignores_non_data_lines() {
        let body = "event: ping\n: comment\ndata: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n";
        let chunks = parse_sse_chunks(body);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn process_sse_text_stream_produces_text_deltas() {
        let body = sample_sse_text_stream();
        let chunks = parse_sse_chunks(&body);
        let events = process_sse_chunks(chunks);

        // Count text deltas.
        let text_deltas: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Ok(StreamEvent::TextDelta { .. })))
            .collect();

        // "Hello" and " world" are non-empty text deltas.
        assert_eq!(text_deltas.len(), 2);

        // Verify the final Completed event has the accumulated text.
        let completed = events.iter().find_map(|e| match e {
            Ok(StreamEvent::Completed { result }) => Some(result),
            _ => None,
        });
        assert!(completed.is_some());
        assert_eq!(completed.expect("completed").text, "Hello world");
    }

    #[test]
    fn process_sse_text_stream_has_correct_stop_reason() {
        let body = sample_sse_text_stream();
        let chunks = parse_sse_chunks(&body);
        let events = process_sse_chunks(chunks);

        let completed = events.iter().find_map(|e| match e {
            Ok(StreamEvent::Completed { result }) => Some(result),
            _ => None,
        });
        assert_eq!(completed.expect("completed").stop_reason, "end_turn");
    }

    #[test]
    fn process_sse_tool_stream_produces_tool_events() {
        let body = sample_sse_tool_stream();
        let chunks = parse_sse_chunks(&body);
        let events = process_sse_chunks(chunks);

        // Should have ToolCallDelta events.
        let tool_deltas: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Ok(StreamEvent::ToolCallDelta { .. })))
            .collect();
        assert!(!tool_deltas.is_empty(), "should have tool call deltas");

        // Should have a ToolCallComplete event.
        let tool_complete: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Ok(StreamEvent::ToolCallComplete { .. })))
            .collect();
        assert_eq!(tool_complete.len(), 1);

        // Verify the completed tool call.
        if let Ok(StreamEvent::ToolCallComplete { call }) = &tool_complete[0] {
            assert_eq!(call.tool_call_id, "call_stream_001");
            assert_eq!(call.tool_name, "file_read");
            assert!(call.arguments_json.contains("/tmp/test"));
        }
    }

    #[test]
    fn process_sse_tool_stream_has_tool_use_stop_reason() {
        let body = sample_sse_tool_stream();
        let chunks = parse_sse_chunks(&body);
        let events = process_sse_chunks(chunks);

        let completed = events.iter().find_map(|e| match e {
            Ok(StreamEvent::Completed { result }) => Some(result),
            _ => None,
        });
        assert_eq!(completed.expect("completed").stop_reason, "tool_use");
    }

    #[test]
    fn process_sse_stream_has_usage_update() {
        let body = sample_sse_text_stream();
        let chunks = parse_sse_chunks(&body);
        let events = process_sse_chunks(chunks);

        let usage_update = events
            .iter()
            .find(|e| matches!(e, Ok(StreamEvent::UsageUpdate { .. })));
        assert!(usage_update.is_some(), "should have a UsageUpdate event");
    }

    #[test]
    fn process_sse_stream_always_ends_with_completed() {
        let body = sample_sse_text_stream();
        let chunks = parse_sse_chunks(&body);
        let events = process_sse_chunks(chunks);

        let last = events.last().expect("should have events");
        assert!(
            matches!(last, Ok(StreamEvent::Completed { .. })),
            "last event should be Completed"
        );
    }

    // -----------------------------------------------------------------------
    // Builder and configuration tests
    // -----------------------------------------------------------------------

    #[test]
    fn ollama_sets_default_base_url() {
        let executor = LocalExecutor::ollama("llama3.2".to_string());
        assert_eq!(executor.base_url, "http://localhost:11434/v1");
        assert_eq!(executor.model, "llama3.2");
    }

    #[test]
    fn custom_sets_provided_base_url() {
        let executor = LocalExecutor::custom(
            "http://my-server:8080/v1".to_string(),
            "my-model".to_string(),
        );
        assert_eq!(executor.base_url, "http://my-server:8080/v1");
        assert_eq!(executor.model, "my-model");
    }

    #[test]
    fn with_timeout_updates_duration() {
        let executor = LocalExecutor::ollama_builder("llama3.2".to_string())
            .with_timeout(Duration::from_secs(300));
        assert_eq!(executor.timeout, Duration::from_secs(300));
    }

    #[test]
    fn default_timeout_is_120_seconds() {
        let executor = LocalExecutor::ollama("llama3.2".to_string());
        assert_eq!(executor.timeout, Duration::from_secs(120));
    }

    #[test]
    fn builder_pattern_produces_arc() {
        let executor: Arc<LocalExecutor> = LocalExecutor::ollama_builder("llama3.2".to_string())
            .with_timeout(Duration::from_secs(60))
            .build();
        assert_eq!(executor.timeout, Duration::from_secs(60));
        assert_eq!(executor.model, "llama3.2");
    }

    // -----------------------------------------------------------------------
    // Compile-time checks
    // -----------------------------------------------------------------------

    /// Compile-time check: `LocalExecutor` implements `ModelExecutor`.
    #[allow(dead_code)]
    fn _local_executor_is_model_executor(_e: &dyn ModelExecutor) {}

    /// Compile-time check: `LocalExecutor` is Send + Sync.
    #[allow(dead_code)]
    fn _local_executor_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LocalExecutor>();
    }
}

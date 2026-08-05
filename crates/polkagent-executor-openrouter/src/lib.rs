//! `OpenRouter` adapter for the [`ModelExecutor`] trait.
//!
//! This crate provides [`OpenRouterExecutor`] -- a production adapter that
//! translates the provider-agnostic [`InferenceRequest`] into the `OpenAI`-compatible
//! Chat Completions API wire format used by `OpenRouter`, adds `OpenRouter`-specific
//! routing headers (`X-Title`, `HTTP-Referer`) and provider preferences, and maps
//! responses back to [`InferenceResponse`].
//!
//! # Quick start
//!
//! ```rust,no_run
//! use polkagent_executor_openrouter::OpenRouterExecutor;
//!
//! let executor = OpenRouterExecutor::new(
//!     "sk-or-...".to_string(),
//!     "anthropic/claude-opus-4-6".to_string(),
//! );
//!
//! // With provider preferences
//! use polkagent_executor_openrouter::ProviderPreferences;
//!
//! let executor = OpenRouterExecutor::builder(
//!     "sk-or-...".to_string(),
//!     "anthropic/claude-opus-4-6".to_string(),
//! )
//! .with_app_title("My Agent".to_string())
//! .with_site_url("https://myapp.example.com".to_string())
//! .with_provider_preferences(ProviderPreferences {
//!     order: Some(vec!["Anthropic".to_string()]),
//!     allow_fallbacks: Some(false),
//!     ..Default::default()
//! })
//! .build();
//! ```

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, Stream};
use polkagent_executor_trait::{
    ContentBlock, ExecutorError, InferenceMessage, InferenceRequest, InferenceResponse,
    MessageRole, ModelExecutor, ProviderError, StreamEvent, TokenUsage, ToolCall, ToolDefinition,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default `OpenRouter` API base URL.
const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Default request timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Default maximum number of retries for retryable errors.
const DEFAULT_MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff in milliseconds.
const BACKOFF_BASE_MS: u64 = 500;

/// Default concurrency limit for `OpenRouter` requests.
const DEFAULT_MAX_CONCURRENT: u32 = 10;

// ---------------------------------------------------------------------------
// OpenRouter-specific types
// ---------------------------------------------------------------------------

/// Provider preferences for `OpenRouter` routing.
///
/// Controls which upstream providers `OpenRouter` routes to, fallback behavior,
/// and data handling policies.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderPreferences {
    /// Ordered list of provider names to prefer (e.g. `["Anthropic", "OpenAI"]`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,

    /// Whether to allow fallback to other providers if preferred ones are unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_fallbacks: Option<bool>,

    /// Require the provider to comply with specific data policies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_parameters: Option<bool>,

    /// Data collection preference: `"deny"` or `"allow"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_collection: Option<String>,

    /// Provider-specific quantization preference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantizations: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// OpenAI-compatible API types (private)
// ---------------------------------------------------------------------------

/// Request body for the `OpenAI` Chat Completions API (`OpenRouter`-extended).
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
    /// `OpenRouter`-specific: provider routing preferences.
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<ProviderPreferences>,
}

/// A message in the `OpenAI` conversation format.
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

/// A tool definition in the `OpenAI` wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: ApiFunction,
}

/// A function definition within an `OpenAI` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// A tool call in the `OpenAI` wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ApiFunctionCall,
}

/// A function call within an `OpenAI` tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiFunctionCall {
    name: String,
    arguments: String,
}

/// Top-level response from the OpenRouter/OpenAI Chat Completions API.
#[derive(Debug, Clone, Deserialize)]
struct ChatCompletionResponse {
    id: String,
    choices: Vec<Choice>,
    usage: Option<ApiUsage>,
    #[allow(dead_code)]
    model: Option<String>,
    /// `OpenRouter`-specific: upstream provider that served the request.
    #[allow(dead_code)]
    #[serde(default)]
    provider: Option<String>,
}

/// A single choice in the response.
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

/// Token usage from the API response.
#[derive(Debug, Clone, Deserialize)]
struct ApiUsage {
    #[serde(rename = "prompt_tokens")]
    prompt: u32,
    #[serde(rename = "completion_tokens")]
    completion: u32,
    #[allow(dead_code)]
    #[serde(rename = "total_tokens")]
    total: u32,
}

/// Error response body from the API.
#[derive(Debug, Clone, Deserialize)]
struct ApiErrorResponse {
    error: ApiErrorDetail,
}

/// Inner error detail from the API.
#[derive(Debug, Clone, Deserialize)]
struct ApiErrorDetail {
    message: String,
    #[allow(dead_code)]
    #[serde(rename = "type")]
    error_type: Option<String>,
    #[allow(dead_code)]
    code: Option<serde_json::Value>,
    /// `OpenRouter`-specific: metadata about the upstream provider error.
    #[allow(dead_code)]
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// SSE / streaming types
// ---------------------------------------------------------------------------

/// A single chunk from the streaming API.
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
// Provider error classification
// ---------------------------------------------------------------------------

/// Classification of an `OpenRouter` provider error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    /// The upstream provider is temporarily unavailable.
    ProviderUnavailable,
    /// The upstream provider rate-limited the request.
    ProviderRateLimit,
    /// The upstream provider rejected the request content.
    ContentModeration,
    /// Authentication failed at the `OpenRouter` layer.
    Authentication,
    /// The requested model or provider was not found.
    NotFound,
    /// The context window was exceeded.
    ContextWindowExceeded,
    /// A generic bad request.
    BadRequest,
    /// Unknown/other error.
    Unknown,
}

/// Classify an `OpenRouter` error body into a provider error kind.
#[cfg(test)]
fn classify_provider_error(status: u16, body: &str) -> ProviderErrorKind {
    match status {
        401 | 403 => ProviderErrorKind::Authentication,
        404 => ProviderErrorKind::NotFound,
        429 => ProviderErrorKind::ProviderRateLimit,
        502 | 503 => ProviderErrorKind::ProviderUnavailable,
        400 => {
            if body.contains("context_length_exceeded")
                || body.contains("maximum context length")
                || body.contains("too many tokens")
            {
                ProviderErrorKind::ContextWindowExceeded
            } else if body.contains("content_filter")
                || body.contains("content_policy")
                || body.contains("moderation")
            {
                ProviderErrorKind::ContentModeration
            } else {
                ProviderErrorKind::BadRequest
            }
        }
        _ => ProviderErrorKind::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a provider-agnostic `InferenceMessage` to the `OpenAI` wire format.
fn to_api_messages(msg: &InferenceMessage) -> Vec<ApiMessage> {
    let role_str = match msg.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    };

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

/// Convert a provider-agnostic `ToolDefinition` to the `OpenAI` wire format.
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
fn build_request_body(
    request: &InferenceRequest,
    stream: bool,
    provider_prefs: Option<&ProviderPreferences>,
) -> ChatCompletionRequest {
    let mut messages: Vec<ApiMessage> = Vec::new();

    if let Some(system) = &request.system {
        messages.push(ApiMessage {
            role: "system".to_string(),
            content: Some(system.clone()),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    for msg in &request.messages {
        messages.extend(to_api_messages(msg));
    }

    let tools: Vec<ApiTool> = request.tools.iter().map(to_api_tool).collect();

    let tool_choice = if tools.is_empty() {
        None
    } else {
        Some("auto".to_string())
    };

    ChatCompletionRequest {
        model: request.model_id.clone(),
        messages,
        max_tokens: Some(request.max_tokens),
        temperature: request.temperature,
        tools,
        tool_choice,
        stream: if stream { Some(true) } else { None },
        provider: provider_prefs.cloned(),
    }
}

/// Convert an `ApiUsage` to the trait-level `TokenUsage`.
fn to_token_usage(api_usage: &ApiUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: api_usage.prompt,
        output_tokens: api_usage.completion,
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

    let stop_reason = map_finish_reason(&finish_reason);

    let usage = resp.usage.as_ref().map(to_token_usage).unwrap_or_default();

    InferenceResponse {
        text,
        tool_calls,
        stop_reason,
        usage,
        provider_request_id: Some(resp.id.clone()),
    }
}

/// Map `OpenAI` `finish_reason` to the normalized stop reason values.
fn map_finish_reason(finish_reason: &str) -> String {
    match finish_reason {
        "stop" | "content_filter" => "end_turn".to_string(),
        "length" => "max_tokens".to_string(),
        "tool_calls" => "tool_use".to_string(),
        other => other.to_string(),
    }
}

/// Parse the `Retry-After` header from an HTTP response as whole seconds.
fn parse_retry_after(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

/// Map an HTTP status code, error body, and optional `Retry-After` header
/// to an [`ExecutorError`] via [`ProviderError`] classification.
fn map_api_error(
    status: u16,
    body: &str,
    retry_after_secs: Option<u64>,
    model_id: &str,
) -> ExecutorError {
    let detail = serde_json::from_str::<ApiErrorResponse>(body)
        .map_or_else(|_| body.to_string(), |e| e.error.message);

    ProviderError::classify(status, &detail, retry_after_secs, model_id).into()
}

/// Parse SSE lines from a response body chunk.
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
fn process_sse_chunks(chunks: &[StreamChunk]) -> Vec<Result<StreamEvent, ExecutorError>> {
    let mut stream_events: Vec<Result<StreamEvent, ExecutorError>> = Vec::new();
    let mut accumulated_text = String::new();
    let mut tool_assemblers: Vec<ToolCallAssembler> = Vec::new();
    let mut final_tool_calls: Vec<ToolCall> = Vec::new();
    let mut usage = TokenUsage::default();
    let mut stop_reason = String::from("end_turn");

    for chunk in chunks {
        if let Some(api_usage) = &chunk.usage {
            usage = to_token_usage(api_usage);
        }

        for choice in &chunk.choices {
            if let Some(reason) = &choice.finish_reason {
                stop_reason = map_finish_reason(reason);
            }

            let delta = &choice.delta;

            if let Some(content) = &delta.content {
                if !content.is_empty() {
                    accumulated_text.push_str(content);
                    stream_events.push(Ok(StreamEvent::TextDelta {
                        delta: content.clone(),
                    }));
                }
            }

            if let Some(tool_calls) = &delta.tool_calls {
                for tc in tool_calls {
                    let idx = tc.index;

                    while tool_assemblers.len() <= idx {
                        tool_assemblers.push(ToolCallAssembler::default());
                    }
                    let assembler = &mut tool_assemblers[idx];

                    if let Some(id) = &tc.id {
                        assembler.id.clone_from(id);
                    }

                    if let Some(func) = &tc.function {
                        let name_delta = func.name.as_ref();
                        if let Some(name) = name_delta {
                            assembler.name.push_str(name);
                        }

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

    stream_events.push(Ok(StreamEvent::UsageUpdate {
        usage: usage.clone(),
    }));

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
// OpenRouterExecutor
// ---------------------------------------------------------------------------

/// A [`ModelExecutor`] adapter for the `OpenRouter` API.
///
/// Wraps the `OpenAI`-compatible Chat Completions API with `OpenRouter`-specific
/// routing headers (`X-Title`, `HTTP-Referer`) and provider preferences.
///
/// Handles authentication via `OPENROUTER_API_KEY`, request/response mapping,
/// rate-limit retries with exponential backoff, and streaming via SSE.
pub struct OpenRouterExecutor {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
    max_retries: u32,
    concurrency_semaphore: Arc<Semaphore>,
    /// `OpenRouter` `X-Title` header value (app name).
    app_title: Option<String>,
    /// `OpenRouter` `HTTP-Referer` header value (site URL).
    site_url: Option<String>,
    /// Provider routing preferences sent in the request body.
    provider_preferences: Option<ProviderPreferences>,
}

impl OpenRouterExecutor {
    /// Create a new `OpenRouterExecutor` with the given API key and model.
    ///
    /// Uses the default `OpenRouter` API base URL, a 120-second timeout,
    /// up to 3 retries, and a concurrency limit of 10.
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
            concurrency_semaphore: Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENT as usize)),
            app_title: None,
            site_url: None,
            provider_preferences: None,
        })
    }

    /// Construct a builder for fine-grained configuration.
    pub fn builder(api_key: String, model: String) -> OpenRouterBuilder {
        OpenRouterBuilder {
            api_key,
            model,
            base_url: DEFAULT_BASE_URL.to_string(),
            timeout: DEFAULT_TIMEOUT,
            max_retries: DEFAULT_MAX_RETRIES,
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            app_title: None,
            site_url: None,
            provider_preferences: None,
        }
    }

    /// Execute the HTTP request with retry logic for retryable errors.
    async fn execute_with_retries(
        &self,
        body: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ExecutorError> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut last_error: Option<ExecutorError> = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay_ms = BACKOFF_BASE_MS * 2u64.pow(attempt - 1);
                debug!(attempt, delay_ms, "retrying after backoff");
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            let mut req = self
                .client
                .post(&url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("content-type", "application/json");

            // Add OpenRouter-specific routing headers.
            if let Some(title) = &self.app_title {
                req = req.header("X-Title", title.as_str());
            }
            if let Some(site) = &self.site_url {
                req = req.header("HTTP-Referer", site.as_str());
            }

            let result = req.json(body).send().await;

            let response = match result {
                Ok(resp) => resp,
                Err(e) => {
                    if e.is_timeout() {
                        let err = ExecutorError::Timeout {
                            elapsed_ms: u64::try_from(DEFAULT_TIMEOUT.as_millis())
                                .unwrap_or(u64::MAX),
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
                let response_body =
                    response
                        .text()
                        .await
                        .map_err(|e| ExecutorError::InvalidResponse {
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

            let retry_after = parse_retry_after(&response);
            let error_body = response.text().await.unwrap_or_default();
            let error = map_api_error(status, &error_body, retry_after, &self.model);

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
        let url = format!("{}/chat/completions", self.base_url);

        let mut req = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json");

        if let Some(title) = &self.app_title {
            req = req.header("X-Title", title.as_str());
        }
        if let Some(site) = &self.site_url {
            req = req.header("HTTP-Referer", site.as_str());
        }

        let response = req.json(body).send().await.map_err(|e| {
            if e.is_timeout() {
                ExecutorError::Timeout {
                    elapsed_ms: u64::try_from(DEFAULT_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
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
            let retry_after = parse_retry_after(&response);
            let error_body = response.text().await.unwrap_or_default();
            return Err(map_api_error(status, &error_body, retry_after, &self.model));
        }

        let full_body = response
            .text()
            .await
            .map_err(|e| ExecutorError::InvalidResponse {
                message: format!("failed to read SSE stream body: {e}"),
            })?;

        let sse_chunks = parse_sse_chunks(&full_body);
        Ok(process_sse_chunks(&sse_chunks))
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for configuring an [`OpenRouterExecutor`].
pub struct OpenRouterBuilder {
    api_key: String,
    model: String,
    base_url: String,
    timeout: Duration,
    max_retries: u32,
    max_concurrent: u32,
    app_title: Option<String>,
    site_url: Option<String>,
    provider_preferences: Option<ProviderPreferences>,
}

impl OpenRouterBuilder {
    /// Override the base URL.
    #[must_use]
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Override the HTTP request timeout.
    #[must_use]
    pub fn with_timeout(mut self, duration: Duration) -> Self {
        self.timeout = duration;
        self
    }

    /// Override the maximum number of retries for retryable errors.
    #[must_use]
    pub fn with_max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    /// Set the maximum number of concurrent HTTP requests.
    #[must_use]
    pub fn with_max_concurrent(mut self, n: u32) -> Self {
        self.max_concurrent = n;
        self
    }

    /// Set the `X-Title` header for `OpenRouter` app identification.
    #[must_use]
    pub fn with_app_title(mut self, title: String) -> Self {
        self.app_title = Some(title);
        self
    }

    /// Set the `HTTP-Referer` header for `OpenRouter` site identification.
    #[must_use]
    pub fn with_site_url(mut self, url: String) -> Self {
        self.site_url = Some(url);
        self
    }

    /// Set provider routing preferences.
    #[must_use]
    pub fn with_provider_preferences(mut self, prefs: ProviderPreferences) -> Self {
        self.provider_preferences = Some(prefs);
        self
    }

    /// Build the executor as an `Arc<OpenRouterExecutor>`.
    pub fn build(self) -> Arc<OpenRouterExecutor> {
        let client = Client::builder()
            .timeout(self.timeout)
            .build()
            .unwrap_or_else(|_| Client::new());

        Arc::new(OpenRouterExecutor {
            client,
            api_key: self.api_key,
            model: self.model,
            base_url: self.base_url,
            max_retries: self.max_retries,
            concurrency_semaphore: Arc::new(Semaphore::new(self.max_concurrent as usize)),
            app_title: self.app_title,
            site_url: self.site_url,
            provider_preferences: self.provider_preferences,
        })
    }
}

// ---------------------------------------------------------------------------
// ModelExecutor implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ModelExecutor for OpenRouterExecutor {
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
            "executing openrouter inference"
        );

        let body = build_request_body(&request, false, self.provider_preferences.as_ref());

        // Acquire concurrency permit, held for the request duration.
        let _permit = self
            .concurrency_semaphore
            .acquire()
            .await
            .map_err(|_| ExecutorError::Cancelled)?;

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
            "starting openrouter streaming inference"
        );

        let body = build_request_body(&request, true, self.provider_preferences.as_ref());

        let _permit = self
            .concurrency_semaphore
            .acquire()
            .await
            .map_err(|_| ExecutorError::Cancelled)?;

        let events = self.execute_streaming(&body).await?;

        Ok(Box::new(stream::iter(events)))
    }

    async fn health(&self) -> Result<(), ExecutorError> {
        // OpenRouter supports GET /api/v1/models for health/auth check.
        let url = format!("{}/models", self.base_url);

        let mut req = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key));

        if let Some(title) = &self.app_title {
            req = req.header("X-Title", title.as_str());
        }
        if let Some(site) = &self.site_url {
            req = req.header("HTTP-Referer", site.as_str());
        }

        let response = req.send().await.map_err(|e| ExecutorError::Transport {
            message: format!("health check failed: {e}"),
            retryable: true,
        })?;

        let status = response.status().as_u16();
        if status == 200 {
            Ok(())
        } else {
            let retry_after = parse_retry_after(&response);
            let error_body = response.text().await.unwrap_or_default();
            Err(map_api_error(status, &error_body, retry_after, &self.model))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Adapter unit tests intentionally panic at the exact wire-contract boundary
// that failed so malformed fixtures remain easy to diagnose.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "provider adapter test assertions intentionally panic with focused diagnostics"
)]
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
            model_id: "anthropic/claude-opus-4-6".into(),
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
            model_id: "openai/gpt-4o".into(),
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
            model_id: "openai/gpt-4o".into(),
            max_tokens: 1024,
            temperature: None,
        }
    }

    fn sample_text_response_json() -> String {
        serde_json::json!({
            "id": "gen-abc123",
            "object": "chat.completion",
            "created": 1_677_858_242,
            "model": "anthropic/claude-opus-4-6",
            "provider": "Anthropic",
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
            "id": "gen-def456",
            "object": "chat.completion",
            "created": 1_677_858_242,
            "model": "openai/gpt-4o",
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
            "id": "gen-ghi012",
            "object": "chat.completion",
            "created": 1_677_858_242,
            "model": "openai/gpt-4o",
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
            "id": "gen-multi",
            "object": "chat.completion",
            "created": 1_677_858_242,
            "model": "openai/gpt-4o",
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
            r#"data: {"id":"gen-stream1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
            r#"data: {"id":"gen-stream1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#,
            r#"data: {"id":"gen-stream1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":null}]}"#,
            r#"data: {"id":"gen-stream1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            r#"data: {"id":"gen-stream1","object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":20,"completion_tokens":5,"total_tokens":25}}"#,
            "data: [DONE]",
        ]
        .join("\n")
    }

    fn sample_sse_tool_stream() -> String {
        [
            r#"data: {"id":"gen-stream2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_abc123","type":"function","function":{"name":"file_read","arguments":""}}]},"finish_reason":null}]}"#,
            r#"data: {"id":"gen-stream2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"pa"}}]},"finish_reason":null}]}"#,
            r#"data: {"id":"gen-stream2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"/tmp/test\"}"}}]},"finish_reason":null}]}"#,
            r#"data: {"id":"gen-stream2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"data: {"id":"gen-stream2","object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":30,"completion_tokens":15,"total_tokens":45}}"#,
            "data: [DONE]",
        ]
        .join("\n")
    }

    // -----------------------------------------------------------------------
    // Constructor / builder tests
    // -----------------------------------------------------------------------

    #[test]
    fn new_creates_executor_with_defaults() {
        let exec = OpenRouterExecutor::new("sk-or-test".into(), "openai/gpt-4o".into());
        assert_eq!(exec.api_key, "sk-or-test");
        assert_eq!(exec.model, "openai/gpt-4o");
        assert_eq!(exec.base_url, DEFAULT_BASE_URL);
        assert_eq!(exec.max_retries, DEFAULT_MAX_RETRIES);
        assert!(exec.app_title.is_none());
        assert!(exec.site_url.is_none());
        assert!(exec.provider_preferences.is_none());
    }

    #[test]
    fn builder_sets_app_title() {
        let exec = OpenRouterExecutor::builder("key".into(), "model".into())
            .with_app_title("My App".into())
            .build();
        assert_eq!(exec.app_title.as_deref(), Some("My App"));
    }

    #[test]
    fn builder_sets_site_url() {
        let exec = OpenRouterExecutor::builder("key".into(), "model".into())
            .with_site_url("https://example.com".into())
            .build();
        assert_eq!(exec.site_url.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn builder_sets_provider_preferences() {
        let prefs = ProviderPreferences {
            order: Some(vec!["Anthropic".into(), "OpenAI".into()]),
            allow_fallbacks: Some(false),
            ..Default::default()
        };
        let exec = OpenRouterExecutor::builder("key".into(), "model".into())
            .with_provider_preferences(prefs)
            .build();
        let pp = exec.provider_preferences.as_ref().expect("prefs set");
        assert_eq!(pp.order.as_ref().expect("order"), &["Anthropic", "OpenAI"]);
        assert_eq!(pp.allow_fallbacks, Some(false));
    }

    #[test]
    fn builder_sets_max_retries() {
        let exec = OpenRouterExecutor::builder("key".into(), "model".into())
            .with_max_retries(5)
            .build();
        assert_eq!(exec.max_retries, 5);
    }

    #[test]
    fn builder_sets_base_url() {
        let exec = OpenRouterExecutor::builder("key".into(), "model".into())
            .with_base_url("https://custom.openrouter.ai/api/v1".into())
            .build();
        assert_eq!(exec.base_url, "https://custom.openrouter.ai/api/v1");
    }

    // -----------------------------------------------------------------------
    // Request serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn request_body_includes_model_and_max_tokens() {
        let req = minimal_request();
        let body = build_request_body(&req, false, None);
        let json = serde_json::to_value(&body).expect("serialize request body");

        assert_eq!(json["model"], "anthropic/claude-opus-4-6");
        assert_eq!(json["max_tokens"], 1024);
        assert!(json.get("stream").is_none());
    }

    #[test]
    fn request_body_includes_system_as_first_message() {
        let req = minimal_request();
        let body = build_request_body(&req, false, None);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages array");
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are a helpful assistant.");
    }

    #[test]
    fn request_body_omits_system_message_when_none() {
        let mut req = minimal_request();
        req.system = None;
        let body = build_request_body(&req, false, None);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages");
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn request_body_includes_temperature_when_set() {
        let req = minimal_request();
        let body = build_request_body(&req, false, None);
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
        let body = build_request_body(&req, false, None);
        let json = serde_json::to_value(&body).expect("serialize");

        assert!(json.get("temperature").is_none());
    }

    #[test]
    fn request_body_sets_stream_flag() {
        let req = minimal_request();
        let body = build_request_body(&req, true, None);
        let json = serde_json::to_value(&body).expect("serialize");

        assert_eq!(json["stream"], true);
    }

    #[test]
    fn request_body_maps_user_message_correctly() {
        let req = minimal_request();
        let body = build_request_body(&req, false, None);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages array");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "hello");
    }

    #[test]
    fn request_body_maps_tools_correctly() {
        let req = request_with_tools();
        let body = build_request_body(&req, false, None);
        let json = serde_json::to_value(&body).expect("serialize");

        let tools = json["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "file_read");
        assert_eq!(tools[0]["function"]["description"], "Read a file from disk");
        assert_eq!(tools[0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn request_body_omits_tools_when_empty() {
        let req = minimal_request();
        let body = build_request_body(&req, false, None);
        let json = serde_json::to_value(&body).expect("serialize");

        assert!(json.get("tools").is_none());
    }

    #[test]
    fn request_body_sets_tool_choice_auto_when_tools_present() {
        let req = request_with_tools();
        let body = build_request_body(&req, false, None);
        let json = serde_json::to_value(&body).expect("serialize");

        assert_eq!(json["tool_choice"], "auto");
    }

    #[test]
    fn request_body_omits_tool_choice_when_no_tools() {
        let req = minimal_request();
        let body = build_request_body(&req, false, None);
        let json = serde_json::to_value(&body).expect("serialize");

        assert!(json.get("tool_choice").is_none());
    }

    #[test]
    fn request_body_includes_provider_preferences() {
        let prefs = ProviderPreferences {
            order: Some(vec!["Anthropic".into()]),
            allow_fallbacks: Some(false),
            ..Default::default()
        };
        let req = minimal_request();
        let body = build_request_body(&req, false, Some(&prefs));
        let json = serde_json::to_value(&body).expect("serialize");

        let provider = &json["provider"];
        assert_eq!(provider["order"][0], "Anthropic");
        assert_eq!(provider["allow_fallbacks"], false);
    }

    #[test]
    fn request_body_omits_provider_when_none() {
        let req = minimal_request();
        let body = build_request_body(&req, false, None);
        let json = serde_json::to_value(&body).expect("serialize");

        assert!(json.get("provider").is_none());
    }

    #[test]
    fn request_body_maps_tool_use_as_assistant_tool_calls() {
        let req = request_with_tool_result();
        let body = build_request_body(&req, false, None);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages");
        let assistant_msg = &messages[1];
        assert_eq!(assistant_msg["role"], "assistant");
        let tool_calls = assistant_msg["tool_calls"].as_array().expect("tool_calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "call_abc123");
        assert_eq!(tool_calls[0]["type"], "function");
        assert_eq!(tool_calls[0]["function"]["name"], "file_read");
    }

    #[test]
    fn request_body_maps_tool_result_as_tool_role_message() {
        let req = request_with_tool_result();
        let body = build_request_body(&req, false, None);
        let json = serde_json::to_value(&body).expect("serialize");

        let messages = json["messages"].as_array().expect("messages");
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
        let parsed: ChatCompletionResponse = serde_json::from_str(&json).expect("parse response");

        assert_eq!(parsed.id, "gen-abc123");
        assert_eq!(parsed.choices.len(), 1);
        assert_eq!(
            parsed.choices[0].message.content.as_deref(),
            Some("Hello! How can I help you today?")
        );
        assert_eq!(parsed.choices[0].finish_reason.as_deref(), Some("stop"));
        let usage = parsed.usage.as_ref().expect("usage");
        assert_eq!(usage.prompt, 25);
        assert_eq!(usage.completion, 12);
    }

    #[test]
    fn deserialize_response_with_provider_field() {
        let json = sample_text_response_json();
        let parsed: ChatCompletionResponse = serde_json::from_str(&json).expect("parse response");
        assert_eq!(parsed.provider.as_deref(), Some("Anthropic"));
    }

    #[test]
    fn deserialize_tool_use_response() {
        let json = sample_tool_use_response_json();
        let parsed: ChatCompletionResponse = serde_json::from_str(&json).expect("parse response");

        assert_eq!(parsed.choices.len(), 1);
        assert!(parsed.choices[0].message.content.is_none());
        let tool_calls = parsed.choices[0]
            .message
            .tool_calls
            .as_ref()
            .expect("tool_calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_xyz789");
        assert_eq!(tool_calls[0].function.name, "file_read");
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
        assert_eq!(response.provider_request_id.as_deref(), Some("gen-abc123"));
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
        assert_eq!(response.stop_reason, "tool_use");
    }

    #[test]
    fn to_inference_response_uses_first_choice() {
        let json = sample_multiple_choices_response_json();
        let parsed: ChatCompletionResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

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
    }

    #[test]
    fn tool_call_arguments_json_is_valid() {
        let json = sample_tool_use_response_json();
        let parsed: ChatCompletionResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        let args: serde_json::Value =
            serde_json::from_str(&response.tool_calls[0].arguments_json).expect("valid JSON");
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
        let body = sample_error_response_json("invalid_api_key", "Incorrect API key provided.");
        let err = map_api_error(401, &body, None, "test-model");
        assert!(matches!(err, ExecutorError::Authentication { .. }));
        assert!(!err.is_retryable());
    }

    #[test]
    fn error_mapping_429_rate_limit() {
        let body =
            sample_error_response_json("rate_limit_exceeded", "Rate limit reached for model");
        let err = map_api_error(429, &body, None, "test-model");
        assert!(matches!(err, ExecutorError::RateLimit { .. }));
        assert!(err.is_retryable());
    }

    #[test]
    fn error_mapping_429_with_retry_after() {
        let body = sample_error_response_json("rate_limit_exceeded", "slow down");
        let err = map_api_error(429, &body, Some(30), "test-model");
        match &err {
            ExecutorError::RateLimit { retry_after_secs } => {
                assert_eq!(*retry_after_secs, Some(30));
            }
            _ => panic!("expected RateLimit error"),
        }
    }

    #[test]
    fn error_mapping_400_bad_request() {
        let body =
            sample_error_response_json("invalid_request_error", "messages is a required field");
        let err = map_api_error(400, &body, None, "test-model");
        // 400 without context/content keywords
        assert!(!matches!(err, ExecutorError::ContextWindowExceeded { .. }));
    }

    #[test]
    fn error_mapping_400_context_window() {
        let body = r#"{"error":{"message":"This model's maximum context length is 8192 tokens","type":"invalid_request_error","code":"context_length_exceeded"}}"#;
        let err = map_api_error(400, body, None, "test-model");
        assert!(matches!(err, ExecutorError::ContextWindowExceeded { .. }));
    }

    #[test]
    fn error_mapping_400_content_moderation() {
        let body = r#"{"error":{"message":"content_filter triggered","type":"invalid_request_error","code":null}}"#;
        let err = map_api_error(400, body, None, "test-model");
        assert!(matches!(err, ExecutorError::ContentPolicy { .. }));
    }

    #[test]
    fn error_mapping_403_forbidden() {
        let body = sample_error_response_json("permission_error", "not allowed");
        let err = map_api_error(403, &body, None, "test-model");
        assert!(matches!(err, ExecutorError::Authentication { .. }));
    }

    #[test]
    fn error_mapping_404_not_found() {
        let body = sample_error_response_json("not_found", "model not found");
        let err = map_api_error(404, &body, None, "test-model");
        assert!(matches!(err, ExecutorError::ModelNotFound { .. }));
    }

    #[test]
    fn error_mapping_500_server_error() {
        let body = sample_error_response_json("server_error", "internal server error");
        let err = map_api_error(500, &body, None, "test-model");
        assert!(matches!(
            err,
            ExecutorError::Transport {
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn error_mapping_502_provider_unavailable() {
        let err = map_api_error(502, "Bad Gateway", None, "test-model");
        assert!(matches!(
            err,
            ExecutorError::Transport {
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn error_mapping_503_provider_unavailable() {
        let body = sample_error_response_json("server_error", "service unavailable");
        let err = map_api_error(503, &body, None, "test-model");
        assert!(matches!(
            err,
            ExecutorError::Transport {
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn error_mapping_408_timeout() {
        let err = map_api_error(408, "Request Timeout", None, "test-model");
        assert!(matches!(err, ExecutorError::Timeout { .. }));
    }

    #[test]
    fn error_mapping_unknown_status() {
        let err = map_api_error(418, "I'm a teapot", None, "test-model");
        // Unknown status falls to ProviderError's body heuristics -> ServerError
        assert!(err.is_retryable());
    }

    // -----------------------------------------------------------------------
    // Provider error classification tests
    // -----------------------------------------------------------------------

    #[test]
    fn classify_401_as_authentication() {
        assert_eq!(
            classify_provider_error(401, ""),
            ProviderErrorKind::Authentication
        );
    }

    #[test]
    fn classify_403_as_authentication() {
        assert_eq!(
            classify_provider_error(403, ""),
            ProviderErrorKind::Authentication
        );
    }

    #[test]
    fn classify_404_as_not_found() {
        assert_eq!(
            classify_provider_error(404, ""),
            ProviderErrorKind::NotFound
        );
    }

    #[test]
    fn classify_429_as_rate_limit() {
        assert_eq!(
            classify_provider_error(429, ""),
            ProviderErrorKind::ProviderRateLimit
        );
    }

    #[test]
    fn classify_502_as_provider_unavailable() {
        assert_eq!(
            classify_provider_error(502, ""),
            ProviderErrorKind::ProviderUnavailable
        );
    }

    #[test]
    fn classify_503_as_provider_unavailable() {
        assert_eq!(
            classify_provider_error(503, ""),
            ProviderErrorKind::ProviderUnavailable
        );
    }

    #[test]
    fn classify_400_context_exceeded() {
        assert_eq!(
            classify_provider_error(400, "context_length_exceeded"),
            ProviderErrorKind::ContextWindowExceeded
        );
    }

    #[test]
    fn classify_400_content_moderation() {
        assert_eq!(
            classify_provider_error(400, "content_filter violation"),
            ProviderErrorKind::ContentModeration
        );
    }

    #[test]
    fn classify_400_generic() {
        assert_eq!(
            classify_provider_error(400, "something else"),
            ProviderErrorKind::BadRequest
        );
    }

    #[test]
    fn classify_unknown_status() {
        assert_eq!(classify_provider_error(418, ""), ProviderErrorKind::Unknown);
    }

    // -----------------------------------------------------------------------
    // SSE parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_sse_text_stream() {
        let sse = sample_sse_text_stream();
        let chunks = parse_sse_chunks(&sse);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn parse_sse_tool_stream() {
        let sse = sample_sse_tool_stream();
        let chunks = parse_sse_chunks(&sse);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn parse_sse_skips_done_marker() {
        let sse = "data: [DONE]\n";
        let chunks = parse_sse_chunks(sse);
        assert!(chunks.is_empty());
    }

    #[test]
    fn parse_sse_skips_invalid_json() {
        let sse = "data: {invalid json}\n";
        let chunks = parse_sse_chunks(sse);
        assert!(chunks.is_empty());
    }

    #[test]
    fn parse_sse_skips_non_data_lines() {
        let sse = "event: ping\nretry: 5000\n";
        let chunks = parse_sse_chunks(sse);
        assert!(chunks.is_empty());
    }

    #[test]
    fn process_sse_text_stream_produces_correct_events() {
        let sse = sample_sse_text_stream();
        let chunks = parse_sse_chunks(&sse);
        let events = process_sse_chunks(&chunks);

        // Should have: text deltas + usage + completed
        let text_deltas: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Ok(StreamEvent::TextDelta { .. })))
            .collect();
        assert!(!text_deltas.is_empty());

        // Last event should be Completed
        let last = events.last().expect("has events");
        assert!(matches!(last, Ok(StreamEvent::Completed { .. })));
    }

    #[test]
    fn process_sse_text_stream_accumulates_text() {
        let sse = sample_sse_text_stream();
        let chunks = parse_sse_chunks(&sse);
        let events = process_sse_chunks(&chunks);

        let completed = events.iter().find_map(|e| match e {
            Ok(StreamEvent::Completed { result }) => Some(result),
            _ => None,
        });
        let result = completed.expect("has completed event");
        assert_eq!(result.text, "Hello world");
        assert_eq!(result.stop_reason, "end_turn");
    }

    #[test]
    fn process_sse_tool_stream_assembles_tool_call() {
        let sse = sample_sse_tool_stream();
        let chunks = parse_sse_chunks(&sse);
        let events = process_sse_chunks(&chunks);

        let tool_complete: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Ok(StreamEvent::ToolCallComplete { .. })))
            .collect();
        assert_eq!(tool_complete.len(), 1);

        let completed = events.iter().find_map(|e| match e {
            Ok(StreamEvent::Completed { result }) => Some(result),
            _ => None,
        });
        let result = completed.expect("has completed event");
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].tool_name, "file_read");
        assert_eq!(result.tool_calls[0].tool_call_id, "call_abc123");
        assert_eq!(result.stop_reason, "tool_use");
    }

    #[test]
    fn process_sse_stream_extracts_usage() {
        let sse = sample_sse_text_stream();
        let chunks = parse_sse_chunks(&sse);
        let events = process_sse_chunks(&chunks);

        let usage_event = events.iter().find_map(|e| match e {
            Ok(StreamEvent::UsageUpdate { usage }) => Some(usage),
            _ => None,
        });
        let usage = usage_event.expect("has usage event");
        assert_eq!(usage.input_tokens, 20);
        assert_eq!(usage.output_tokens, 5);
    }

    // -----------------------------------------------------------------------
    // ProviderPreferences serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn provider_preferences_serializes_order() {
        let prefs = ProviderPreferences {
            order: Some(vec!["Anthropic".into(), "OpenAI".into()]),
            ..Default::default()
        };
        let json = serde_json::to_value(&prefs).expect("serialize");
        assert_eq!(json["order"][0], "Anthropic");
        assert_eq!(json["order"][1], "OpenAI");
    }

    #[test]
    fn provider_preferences_skips_none_fields() {
        let prefs = ProviderPreferences::default();
        let json = serde_json::to_value(&prefs).expect("serialize");
        assert!(json.get("order").is_none());
        assert!(json.get("allow_fallbacks").is_none());
        assert!(json.get("require_parameters").is_none());
    }

    #[test]
    fn provider_preferences_serializes_data_collection() {
        let prefs = ProviderPreferences {
            data_collection: Some("deny".into()),
            ..Default::default()
        };
        let json = serde_json::to_value(&prefs).expect("serialize");
        assert_eq!(json["data_collection"], "deny");
    }

    #[test]
    fn provider_preferences_serializes_quantizations() {
        let prefs = ProviderPreferences {
            quantizations: Some(vec!["bf16".into(), "fp8".into()]),
            ..Default::default()
        };
        let json = serde_json::to_value(&prefs).expect("serialize");
        assert_eq!(json["quantizations"][0], "bf16");
        assert_eq!(json["quantizations"][1], "fp8");
    }

    #[test]
    fn provider_preferences_deserializes() {
        let json = r#"{"order":["Anthropic"],"allow_fallbacks":true}"#;
        let prefs: ProviderPreferences = serde_json::from_str(json).expect("deserialize");
        assert_eq!(prefs.order.as_ref().expect("order"), &["Anthropic"]);
        assert_eq!(prefs.allow_fallbacks, Some(true));
    }

    // -----------------------------------------------------------------------
    // OpenRouter-specific response parsing
    // -----------------------------------------------------------------------

    #[test]
    fn deserialize_response_with_metadata_error() {
        let json = r#"{
            "error": {
                "message": "Provider returned error",
                "type": "provider_error",
                "code": 502,
                "metadata": {"provider_name": "Anthropic", "raw": "upstream error"}
            }
        }"#;
        let parsed: ApiErrorResponse = serde_json::from_str(json).expect("parse");
        assert_eq!(parsed.error.message, "Provider returned error");
        assert!(parsed.error.metadata.is_some());
    }

    #[test]
    fn deserialize_error_with_numeric_code() {
        let json = r#"{"error":{"message":"error","type":"test","code":429}}"#;
        let parsed: ApiErrorResponse = serde_json::from_str(json).expect("parse");
        assert_eq!(parsed.error.message, "error");
    }

    #[test]
    fn deserialize_error_with_string_code() {
        let json = r#"{"error":{"message":"error","type":"test","code":"rate_limit"}}"#;
        let parsed: ApiErrorResponse = serde_json::from_str(json).expect("parse");
        assert_eq!(parsed.error.message, "error");
    }
}

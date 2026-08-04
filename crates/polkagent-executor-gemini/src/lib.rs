//! Google Gemini native API adapter for the [`ModelExecutor`] trait.
//!
//! This crate provides [`GeminiExecutor`] -- a production adapter that
//! translates the provider-agnostic [`InferenceRequest`] into the Gemini
//! `generateContent` / `streamGenerateContent` API wire format, handles
//! authentication via API key, rate-limit retries with exponential backoff,
//! and maps responses back to [`InferenceResponse`].
//!
//! # Quick start
//!
//! ```rust,no_run
//! use polkagent_executor_gemini::GeminiExecutor;
//!
//! let executor = GeminiExecutor::new(
//!     "AIza...".to_string(),
//!     "gemini-2.5-flash".to_string(),
//! );
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

/// Default Gemini API base URL.
const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Default request timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Default maximum number of retries for retryable errors.
const DEFAULT_MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff in milliseconds.
const BACKOFF_BASE_MS: u64 = 500;

// ---------------------------------------------------------------------------
// Gemini API types (private)
// ---------------------------------------------------------------------------

/// Request body for the Gemini `generateContent` API.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<GeminiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

/// Generation configuration for Gemini.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

/// A content message in the Gemini conversation format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

/// A part within a Gemini content message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_response: Option<GeminiFunctionResponse>,
}

/// A function call emitted by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

/// A function response returned to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GeminiFunctionResponse {
    name: String,
    response: serde_json::Value,
}

/// A tool definition in the Gemini wire format.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiTool {
    function_declarations: Vec<GeminiFunctionDeclaration>,
}

/// A function declaration within a Gemini tool.
#[derive(Debug, Clone, Serialize)]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// Top-level response from the Gemini `generateContent` API.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    usage_metadata: Option<UsageMetadata>,
    #[allow(dead_code)]
    model_version: Option<String>,
}

/// A single candidate in the Gemini response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    finish_reason: Option<String>,
    #[allow(dead_code)]
    safety_ratings: Option<Vec<serde_json::Value>>,
}

/// Token usage metadata from the Gemini API.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    #[allow(dead_code)]
    total_token_count: Option<u32>,
    cached_content_token_count: Option<u32>,
}

/// Error response body from the Gemini API.
#[derive(Debug, Clone, Deserialize)]
struct GeminiErrorResponse {
    error: GeminiErrorDetail,
}

/// Inner error detail from the Gemini API.
#[derive(Debug, Clone, Deserialize)]
struct GeminiErrorDetail {
    message: String,
    #[allow(dead_code)]
    status: Option<String>,
    #[allow(dead_code)]
    code: Option<u16>,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a provider-agnostic `InferenceMessage` to the Gemini wire format.
///
/// Gemini uses a different message structure than OpenAI/Anthropic:
/// - User messages have `role: "user"` with `parts` containing text.
/// - Model messages have `role: "model"` with `parts` containing text or `functionCall`.
/// - Tool results are sent as `role: "user"` messages with `functionResponse` parts.
///
/// A single `InferenceMessage` may expand into multiple Gemini content entries
/// when it contains mixed content blocks (e.g., text + tool results need separate
/// content entries since tool results must be in a "user" role message).
fn to_gemini_contents(msg: &InferenceMessage) -> Vec<GeminiContent> {
    let role = match msg.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "model",
    };

    let mut text_parts: Vec<GeminiPart> = Vec::new();
    let mut function_call_parts: Vec<GeminiPart> = Vec::new();
    let mut function_response_parts: Vec<GeminiPart> = Vec::new();

    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => {
                text_parts.push(GeminiPart {
                    text: Some(text.clone()),
                    function_call: None,
                    function_response: None,
                });
            }
            ContentBlock::ToolUse {
                tool_name,
                arguments_json,
                ..
            } => {
                let args: serde_json::Value =
                    serde_json::from_str(arguments_json).unwrap_or_else(|_| serde_json::json!({}));
                function_call_parts.push(GeminiPart {
                    text: None,
                    function_call: Some(GeminiFunctionCall {
                        name: tool_name.clone(),
                        args,
                    }),
                    function_response: None,
                });
            }
            ContentBlock::ToolResult {
                tool_call_id,
                content,
                is_error,
            } => {
                let response = if *is_error {
                    serde_json::json!({ "error": content })
                } else {
                    serde_json::json!({ "result": content })
                };
                // Use tool_call_id as the function name for correlation.
                // In practice, the caller should track which tool_call_id maps
                // to which function name.
                function_response_parts.push(GeminiPart {
                    text: None,
                    function_call: None,
                    function_response: Some(GeminiFunctionResponse {
                        name: tool_call_id.clone(),
                        response,
                    }),
                });
            }
        }
    }

    let mut contents = Vec::new();

    // Model messages: combine text + function calls in one content entry.
    if role == "model" {
        let mut parts = text_parts;
        parts.extend(function_call_parts);
        if !parts.is_empty() {
            contents.push(GeminiContent {
                role: Some("model".to_string()),
                parts,
            });
        }
    } else {
        // User messages: text parts go in one content entry.
        if !text_parts.is_empty() {
            contents.push(GeminiContent {
                role: Some("user".to_string()),
                parts: text_parts,
            });
        }
    }

    // Function responses always go in a "user" role content entry.
    if !function_response_parts.is_empty() {
        contents.push(GeminiContent {
            role: Some("user".to_string()),
            parts: function_response_parts,
        });
    }

    contents
}

/// Convert a provider-agnostic `ToolDefinition` to the Gemini wire format.
fn to_gemini_function_declaration(tool: &ToolDefinition) -> GeminiFunctionDeclaration {
    let parameters: serde_json::Value = serde_json::from_str(&tool.input_schema_json)
        .unwrap_or_else(|_| {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
        });

    GeminiFunctionDeclaration {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters,
    }
}

/// Build the API request body from an `InferenceRequest`.
fn build_request_body(request: &InferenceRequest) -> GenerateContentRequest {
    let mut contents: Vec<GeminiContent> = Vec::new();

    for msg in &request.messages {
        contents.extend(to_gemini_contents(msg));
    }

    let system_instruction = request.system.as_ref().map(|system| GeminiContent {
        role: None,
        parts: vec![GeminiPart {
            text: Some(system.clone()),
            function_call: None,
            function_response: None,
        }],
    });

    let tools: Vec<GeminiTool> = if request.tools.is_empty() {
        Vec::new()
    } else {
        vec![GeminiTool {
            function_declarations: request
                .tools
                .iter()
                .map(to_gemini_function_declaration)
                .collect(),
        }]
    };

    let generation_config = Some(GenerationConfig {
        max_output_tokens: Some(request.max_tokens),
        temperature: request.temperature,
    });

    GenerateContentRequest {
        contents,
        system_instruction,
        tools,
        generation_config,
    }
}

/// Convert `UsageMetadata` to the trait-level `TokenUsage`.
fn to_token_usage(usage: &UsageMetadata) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.prompt_token_count.unwrap_or(0),
        output_tokens: usage.candidates_token_count.unwrap_or(0),
        cache_read_tokens: usage.cached_content_token_count,
        cache_write_tokens: None,
    }
}

/// Convert a `GenerateContentResponse` to the trait-level `InferenceResponse`.
fn to_inference_response(resp: &GenerateContentResponse) -> InferenceResponse {
    let candidate = resp.candidates.as_ref().and_then(|c| c.first());

    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    if let Some(candidate) = candidate {
        if let Some(content) = &candidate.content {
            for (idx, part) in content.parts.iter().enumerate() {
                if let Some(t) = &part.text {
                    text.push_str(t);
                }
                if let Some(fc) = &part.function_call {
                    tool_calls.push(ToolCall {
                        tool_call_id: format!("gemini_call_{idx}"),
                        tool_name: fc.name.clone(),
                        arguments_json: serde_json::to_string(&fc.args)
                            .unwrap_or_else(|_| "{}".to_string()),
                    });
                }
            }
        }
    }

    let finish_reason = candidate
        .and_then(|c| c.finish_reason.clone())
        .unwrap_or_else(|| "STOP".to_string());

    let stop_reason = map_finish_reason(&finish_reason);

    let usage = resp
        .usage_metadata
        .as_ref()
        .map(to_token_usage)
        .unwrap_or_default();

    InferenceResponse {
        text,
        tool_calls,
        stop_reason,
        usage,
        provider_request_id: None,
    }
}

/// Map Gemini `finishReason` to the normalized stop reason values
/// used by the executor trait.
fn map_finish_reason(finish_reason: &str) -> String {
    match finish_reason {
        "STOP" => "end_turn".to_string(),
        "MAX_TOKENS" => "max_tokens".to_string(),
        "SAFETY" => "end_turn".to_string(),
        "RECITATION" => "end_turn".to_string(),
        // Gemini does not use a separate finish reason for tool use.
        // When function calls are present the finish_reason is typically "STOP".
        // We handle this at the response level by checking for tool calls.
        other => other.to_string(),
    }
}

/// Map an HTTP status code, error body, and optional `Retry-After` header
/// to an [`ExecutorError`] via [`ProviderError`] classification.
fn map_api_error(
    status: u16,
    body: &str,
    retry_after_secs: Option<u64>,
    model_id: &str,
) -> ExecutorError {
    let detail = serde_json::from_str::<GeminiErrorResponse>(body)
        .map(|e| e.error.message)
        .unwrap_or_else(|_| body.to_string());

    ProviderError::classify(status, &detail, retry_after_secs, model_id).into()
}

/// Parse the `Retry-After` header from an HTTP response as whole seconds.
fn parse_retry_after(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

// ---------------------------------------------------------------------------
// SSE / streaming types
// ---------------------------------------------------------------------------

/// Parse streaming response chunks from a Gemini SSE response body.
///
/// Gemini streaming returns a JSON array of `GenerateContentResponse` objects,
/// or line-delimited JSON objects prefixed with `data: `.
fn parse_streaming_response(body: &str) -> Vec<GenerateContentResponse> {
    // Gemini streaming can return either:
    // 1. A JSON array of response objects
    // 2. SSE-style "data:" lines
    let trimmed = body.trim();

    // Try JSON array first.
    if trimmed.starts_with('[') {
        if let Ok(responses) = serde_json::from_str::<Vec<GenerateContentResponse>>(trimmed) {
            return responses;
        }
    }

    // Try SSE-style parsing.
    let mut responses = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                continue;
            }
            if let Ok(parsed) = serde_json::from_str::<GenerateContentResponse>(data) {
                responses.push(parsed);
            }
        }
    }

    // As a fallback, try to parse the whole body as a single response.
    if responses.is_empty() {
        if let Ok(single) = serde_json::from_str::<GenerateContentResponse>(trimmed) {
            responses.push(single);
        }
    }

    responses
}

/// Process streaming response chunks into a list of `StreamEvent` values.
fn process_streaming_chunks(
    chunks: Vec<GenerateContentResponse>,
) -> Vec<Result<StreamEvent, ExecutorError>> {
    let mut stream_events: Vec<Result<StreamEvent, ExecutorError>> = Vec::new();
    let mut accumulated_text = String::new();
    let mut final_tool_calls: Vec<ToolCall> = Vec::new();
    let mut usage = TokenUsage::default();
    let mut stop_reason = String::from("end_turn");
    let mut tool_call_counter: usize = 0;

    for chunk in &chunks {
        // Track usage if present.
        if let Some(usage_metadata) = &chunk.usage_metadata {
            usage = to_token_usage(usage_metadata);
        }

        if let Some(candidates) = &chunk.candidates {
            for candidate in candidates {
                // Track finish_reason.
                if let Some(reason) = &candidate.finish_reason {
                    stop_reason = map_finish_reason(reason);
                }

                if let Some(content) = &candidate.content {
                    for part in &content.parts {
                        // Text delta.
                        if let Some(text) = &part.text {
                            if !text.is_empty() {
                                accumulated_text.push_str(text);
                                stream_events.push(Ok(StreamEvent::TextDelta {
                                    delta: text.clone(),
                                }));
                            }
                        }

                        // Function call.
                        if let Some(fc) = &part.function_call {
                            let call_id = format!("gemini_call_{tool_call_counter}");
                            tool_call_counter += 1;
                            let args_json = serde_json::to_string(&fc.args)
                                .unwrap_or_else(|_| "{}".to_string());

                            let call = ToolCall {
                                tool_call_id: call_id.clone(),
                                tool_name: fc.name.clone(),
                                arguments_json: args_json.clone(),
                            };

                            stream_events.push(Ok(StreamEvent::ToolCallDelta {
                                tool_call_id: call_id.clone(),
                                name: Some(fc.name.clone()),
                                input_delta: args_json,
                            }));

                            stream_events
                                .push(Ok(StreamEvent::ToolCallComplete { call: call.clone() }));
                            final_tool_calls.push(call);
                        }
                    }
                }
            }
        }
    }

    // If we have tool calls and stop_reason is "end_turn", override to "tool_use".
    if !final_tool_calls.is_empty() && stop_reason == "end_turn" {
        stop_reason = "tool_use".to_string();
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
// GeminiExecutor
// ---------------------------------------------------------------------------

/// A [`ModelExecutor`] adapter for the Google Gemini native API.
///
/// Handles authentication via API key query parameter, request/response mapping,
/// rate-limit retries with exponential backoff, and streaming.
pub struct GeminiExecutor {
    client: Client,
    api_key: String,
    model: String,
    base_url: String,
    max_retries: u32,
    concurrency_semaphore: Option<Arc<Semaphore>>,
}

impl GeminiExecutor {
    /// Create a new `GeminiExecutor` with the given API key and model.
    ///
    /// Uses the default Gemini API base URL, a 120-second timeout,
    /// and up to 3 retries for retryable errors.
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

    /// Override the base URL (for proxies, test servers, etc.).
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

    /// Limit the number of concurrent HTTP requests to the provider.
    #[must_use]
    pub fn with_max_concurrent(mut self, n: u32) -> Self {
        self.concurrency_semaphore = Some(Arc::new(Semaphore::new(n as usize)));
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

    /// Build the URL for the `generateContent` endpoint.
    fn generate_content_url(&self, model_id: &str) -> String {
        format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, model_id, self.api_key
        )
    }

    /// Build the URL for the `streamGenerateContent` endpoint.
    fn stream_generate_content_url(&self, model_id: &str) -> String {
        format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url, model_id, self.api_key
        )
    }

    /// Execute the HTTP request with retry logic for retryable errors.
    async fn execute_with_retries(
        &self,
        body: &GenerateContentRequest,
        model_id: &str,
    ) -> Result<GenerateContentResponse, ExecutorError> {
        let url = self.generate_content_url(model_id);
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
                let response_body =
                    response
                        .text()
                        .await
                        .map_err(|e| ExecutorError::InvalidResponse {
                            message: format!("failed to read response body: {e}"),
                        })?;

                let parsed: GenerateContentResponse = serde_json::from_str(&response_body)
                    .map_err(|e| ExecutorError::InvalidResponse {
                        message: format!("failed to parse response JSON: {e}"),
                    })?;

                return Ok(parsed);
            }

            let retry_after = parse_retry_after(&response);
            let error_body = response.text().await.unwrap_or_default();
            let error = map_api_error(status, &error_body, retry_after, model_id);

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

    /// Execute a streaming request and collect response chunks into `StreamEvent`s.
    async fn execute_streaming(
        &self,
        body: &GenerateContentRequest,
        model_id: &str,
    ) -> Result<Vec<Result<StreamEvent, ExecutorError>>, ExecutorError> {
        let url = self.stream_generate_content_url(model_id);

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
            let retry_after = parse_retry_after(&response);
            let error_body = response.text().await.unwrap_or_default();
            return Err(map_api_error(status, &error_body, retry_after, model_id));
        }

        let full_body = response
            .text()
            .await
            .map_err(|e| ExecutorError::InvalidResponse {
                message: format!("failed to read streaming response body: {e}"),
            })?;

        let chunks = parse_streaming_response(&full_body);
        Ok(process_streaming_chunks(chunks))
    }
}

// ---------------------------------------------------------------------------
// ModelExecutor implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ModelExecutor for GeminiExecutor {
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
            "executing gemini inference"
        );

        let body = build_request_body(&request);

        // Acquire concurrency permit if configured, held for the request duration.
        let _permit = match &self.concurrency_semaphore {
            Some(sem) => Some(sem.acquire().await.map_err(|_| ExecutorError::Cancelled)?),
            None => None,
        };

        let api_response = self.execute_with_retries(&body, &request.model_id).await?;
        let mut response = to_inference_response(&api_response);

        // If we have tool calls and stop_reason is "end_turn", override to "tool_use".
        if !response.tool_calls.is_empty() && response.stop_reason == "end_turn" {
            response.stop_reason = "tool_use".to_string();
        }

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
            "starting gemini streaming inference"
        );

        let body = build_request_body(&request);

        // Acquire concurrency permit if configured, held for the request duration.
        let _permit = match &self.concurrency_semaphore {
            Some(sem) => Some(sem.acquire().await.map_err(|_| ExecutorError::Cancelled)?),
            None => None,
        };

        let events = self.execute_streaming(&body, &request.model_id).await?;

        Ok(Box::new(stream::iter(events)))
    }

    async fn health(&self) -> Result<(), ExecutorError> {
        // List models endpoint to verify connectivity and authentication.
        let url = format!("{}/models?key={}", self.base_url, self.api_key);

        let response =
            self.client
                .get(&url)
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
            model_id: "gemini-2.5-flash".into(),
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
            model_id: "gemini-2.5-flash".into(),
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
                        tool_call_id: "file_read".into(),
                        content: "file contents here".into(),
                        is_error: false,
                    }],
                },
            ],
            system: None,
            tools: vec![],
            model_id: "gemini-2.5-flash".into(),
            max_tokens: 1024,
            temperature: None,
        }
    }

    fn sample_text_response_json() -> String {
        serde_json::json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "text": "Hello! How can I help you today?"
                            }
                        ],
                        "role": "model"
                    },
                    "finishReason": "STOP",
                    "safetyRatings": []
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 25,
                "candidatesTokenCount": 12,
                "totalTokenCount": 37
            }
        })
        .to_string()
    }

    fn sample_tool_use_response_json() -> String {
        serde_json::json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "functionCall": {
                                    "name": "file_read",
                                    "args": {
                                        "path": "/tmp/test.txt"
                                    }
                                }
                            }
                        ],
                        "role": "model"
                    },
                    "finishReason": "STOP",
                    "safetyRatings": []
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 50,
                "candidatesTokenCount": 30,
                "totalTokenCount": 80
            }
        })
        .to_string()
    }

    fn sample_tool_use_with_text_response_json() -> String {
        serde_json::json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "text": "I'll read that file for you."
                            },
                            {
                                "functionCall": {
                                    "name": "file_read",
                                    "args": {
                                        "path": "/tmp/test.txt"
                                    }
                                }
                            }
                        ],
                        "role": "model"
                    },
                    "finishReason": "STOP",
                    "safetyRatings": []
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 50,
                "candidatesTokenCount": 30,
                "totalTokenCount": 80
            }
        })
        .to_string()
    }

    fn sample_max_tokens_response_json() -> String {
        serde_json::json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "text": "This is a truncated..."
                            }
                        ],
                        "role": "model"
                    },
                    "finishReason": "MAX_TOKENS",
                    "safetyRatings": []
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 1024,
                "totalTokenCount": 1034
            }
        })
        .to_string()
    }

    fn sample_error_response_json(status: &str, message: &str) -> String {
        serde_json::json!({
            "error": {
                "message": message,
                "status": status,
                "code": 400
            }
        })
        .to_string()
    }

    fn sample_sse_text_stream() -> String {
        [
            r#"data: {"candidates":[{"content":{"parts":[{"text":"Hello"}],"role":"model"},"finishReason":null}]}"#,
            r#"data: {"candidates":[{"content":{"parts":[{"text":" world"}],"role":"model"},"finishReason":null}]}"#,
            r#"data: {"candidates":[{"content":{"parts":[{"text":""}],"role":"model"},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":20,"candidatesTokenCount":5,"totalTokenCount":25}}"#,
            "data: [DONE]",
        ]
        .join("\n")
    }

    fn sample_sse_tool_stream() -> String {
        [
            r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"file_read","args":{"path":"/tmp/test"}}}],"role":"model"},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":30,"candidatesTokenCount":15,"totalTokenCount":45}}"#,
            "data: [DONE]",
        ]
        .join("\n")
    }

    fn sample_json_array_stream() -> String {
        serde_json::json!([
            {
                "candidates": [
                    {
                        "content": {
                            "parts": [{"text": "Hello"}],
                            "role": "model"
                        }
                    }
                ]
            },
            {
                "candidates": [
                    {
                        "content": {
                            "parts": [{"text": " world"}],
                            "role": "model"
                        },
                        "finishReason": "STOP"
                    }
                ],
                "usageMetadata": {
                    "promptTokenCount": 10,
                    "candidatesTokenCount": 4,
                    "totalTokenCount": 14
                }
            }
        ])
        .to_string()
    }

    fn sample_cached_response_json() -> String {
        serde_json::json!({
            "candidates": [
                {
                    "content": {
                        "parts": [{"text": "Cached result"}],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 100,
                "candidatesTokenCount": 20,
                "totalTokenCount": 120,
                "cachedContentTokenCount": 80
            }
        })
        .to_string()
    }

    // -----------------------------------------------------------------------
    // Request serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn request_body_includes_model_contents() {
        let req = minimal_request();
        let body = build_request_body(&req);
        let json = serde_json::to_value(&body).expect("serialize request body");

        let contents = json["contents"].as_array().expect("contents array");
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "hello");
    }

    #[test]
    fn request_body_includes_system_instruction() {
        let req = minimal_request();
        let body = build_request_body(&req);
        let json = serde_json::to_value(&body).expect("serialize");

        let system = &json["systemInstruction"];
        assert_eq!(system["parts"][0]["text"], "You are a helpful assistant.");
        // System instruction should not have a role field set.
        assert!(system.get("role").is_none() || system["role"].is_null());
    }

    #[test]
    fn request_body_omits_system_instruction_when_none() {
        let mut req = minimal_request();
        req.system = None;
        let body = build_request_body(&req);
        let json = serde_json::to_value(&body).expect("serialize");

        assert!(json.get("systemInstruction").is_none());
    }

    #[test]
    fn request_body_includes_generation_config() {
        let req = minimal_request();
        let body = build_request_body(&req);
        let json = serde_json::to_value(&body).expect("serialize");

        let config = &json["generationConfig"];
        assert_eq!(config["maxOutputTokens"], 1024);
        let temp = config["temperature"]
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
        let body = build_request_body(&req);
        let json = serde_json::to_value(&body).expect("serialize");

        assert!(json["generationConfig"].get("temperature").is_none());
    }

    #[test]
    fn request_body_maps_tools_as_function_declarations() {
        let req = request_with_tools();
        let body = build_request_body(&req);
        let json = serde_json::to_value(&body).expect("serialize");

        let tools = json["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        let decls = tools[0]["functionDeclarations"]
            .as_array()
            .expect("function declarations");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0]["name"], "file_read");
        assert_eq!(decls[0]["description"], "Read a file from disk");
        assert_eq!(decls[0]["parameters"]["type"], "object");
        assert!(decls[0]["parameters"]["properties"]["path"].is_object());
    }

    #[test]
    fn request_body_omits_tools_when_empty() {
        let req = minimal_request();
        let body = build_request_body(&req);
        let json = serde_json::to_value(&body).expect("serialize");

        assert!(json.get("tools").is_none());
    }

    #[test]
    fn request_body_maps_user_message_correctly() {
        let req = minimal_request();
        let body = build_request_body(&req);
        let json = serde_json::to_value(&body).expect("serialize");

        let contents = json["contents"].as_array().expect("contents array");
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "hello");
    }

    #[test]
    fn request_body_maps_tool_use_as_function_call() {
        let req = request_with_tool_result();
        let body = build_request_body(&req);
        let json = serde_json::to_value(&body).expect("serialize");

        let contents = json["contents"].as_array().expect("contents");
        // Contents: [0] user "read the file", [1] model with functionCall, [2] user with functionResponse
        let model_msg = &contents[1];
        assert_eq!(model_msg["role"], "model");
        let fc = &model_msg["parts"][0]["functionCall"];
        assert_eq!(fc["name"], "file_read");
        assert_eq!(fc["args"]["path"], "/tmp/test");
    }

    #[test]
    fn request_body_maps_tool_result_as_function_response() {
        let req = request_with_tool_result();
        let body = build_request_body(&req);
        let json = serde_json::to_value(&body).expect("serialize");

        let contents = json["contents"].as_array().expect("contents");
        // The function response is the third content entry.
        let tool_msg = &contents[2];
        assert_eq!(tool_msg["role"], "user");
        let fr = &tool_msg["parts"][0]["functionResponse"];
        assert_eq!(fr["name"], "file_read");
        assert_eq!(fr["response"]["result"], "file contents here");
    }

    #[test]
    fn request_body_maps_error_tool_result() {
        let req = InferenceRequest {
            run_id: RunId::new(),
            step_id: StepId::new(),
            messages: vec![InferenceMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::ToolResult {
                    tool_call_id: "test_tool".into(),
                    content: "something went wrong".into(),
                    is_error: true,
                }],
            }],
            system: None,
            tools: vec![],
            model_id: "gemini-2.5-flash".into(),
            max_tokens: 1024,
            temperature: None,
        };
        let body = build_request_body(&req);
        let json = serde_json::to_value(&body).expect("serialize");

        let contents = json["contents"].as_array().expect("contents");
        let fr = &contents[0]["parts"][0]["functionResponse"];
        assert_eq!(fr["response"]["error"], "something went wrong");
    }

    // -----------------------------------------------------------------------
    // Response deserialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn deserialize_text_response() {
        let json = sample_text_response_json();
        let parsed: GenerateContentResponse = serde_json::from_str(&json).expect("parse response");

        let candidates = parsed.candidates.as_ref().expect("candidates");
        assert_eq!(candidates.len(), 1);
        let content = candidates[0].content.as_ref().expect("content");
        assert_eq!(
            content.parts[0].text.as_deref(),
            Some("Hello! How can I help you today?")
        );
        assert_eq!(candidates[0].finish_reason.as_deref(), Some("STOP"));
        let usage = parsed.usage_metadata.as_ref().expect("usage");
        assert_eq!(usage.prompt_token_count, Some(25));
        assert_eq!(usage.candidates_token_count, Some(12));
    }

    #[test]
    fn deserialize_tool_use_response() {
        let json = sample_tool_use_response_json();
        let parsed: GenerateContentResponse = serde_json::from_str(&json).expect("parse response");

        let candidates = parsed.candidates.as_ref().expect("candidates");
        assert_eq!(candidates.len(), 1);
        let content = candidates[0].content.as_ref().expect("content");
        let fc = content.parts[0]
            .function_call
            .as_ref()
            .expect("function_call");
        assert_eq!(fc.name, "file_read");
        assert_eq!(fc.args["path"], "/tmp/test.txt");
    }

    #[test]
    fn to_inference_response_maps_text_correctly() {
        let json = sample_text_response_json();
        let parsed: GenerateContentResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.text, "Hello! How can I help you today?");
        assert!(response.tool_calls.is_empty());
        assert_eq!(response.stop_reason, "end_turn");
    }

    #[test]
    fn to_inference_response_maps_tool_calls() {
        let json = sample_tool_use_response_json();
        let parsed: GenerateContentResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert!(response.text.is_empty());
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].tool_name, "file_read");
        assert_eq!(response.tool_calls[0].tool_call_id, "gemini_call_0");
    }

    #[test]
    fn to_inference_response_maps_tool_calls_with_text() {
        let json = sample_tool_use_with_text_response_json();
        let parsed: GenerateContentResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.text, "I'll read that file for you.");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].tool_name, "file_read");
    }

    #[test]
    fn to_inference_response_maps_max_tokens_stop() {
        let json = sample_max_tokens_response_json();
        let parsed: GenerateContentResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.stop_reason, "max_tokens");
        assert_eq!(response.text, "This is a truncated...");
    }

    #[test]
    fn token_usage_extraction() {
        let json = sample_text_response_json();
        let parsed: GenerateContentResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.usage.input_tokens, 25);
        assert_eq!(response.usage.output_tokens, 12);
        assert!(response.usage.cache_read_tokens.is_none());
    }

    #[test]
    fn token_usage_with_cached_content() {
        let json = sample_cached_response_json();
        let parsed: GenerateContentResponse = serde_json::from_str(&json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.usage.input_tokens, 100);
        assert_eq!(response.usage.output_tokens, 20);
        assert_eq!(response.usage.cache_read_tokens, Some(80));
    }

    #[test]
    fn tool_call_arguments_json_is_valid() {
        let json = sample_tool_use_response_json();
        let parsed: GenerateContentResponse = serde_json::from_str(&json).expect("parse");
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
        assert_eq!(map_finish_reason("STOP"), "end_turn");
    }

    #[test]
    fn finish_reason_max_tokens_maps_correctly() {
        assert_eq!(map_finish_reason("MAX_TOKENS"), "max_tokens");
    }

    #[test]
    fn finish_reason_safety_maps_to_end_turn() {
        assert_eq!(map_finish_reason("SAFETY"), "end_turn");
    }

    #[test]
    fn finish_reason_recitation_maps_to_end_turn() {
        assert_eq!(map_finish_reason("RECITATION"), "end_turn");
    }

    #[test]
    fn finish_reason_unknown_passes_through() {
        assert_eq!(map_finish_reason("CUSTOM_REASON"), "CUSTOM_REASON");
    }

    // -----------------------------------------------------------------------
    // Error mapping tests
    // -----------------------------------------------------------------------

    #[test]
    fn error_mapping_401_authentication() {
        let body = sample_error_response_json("UNAUTHENTICATED", "API key not valid");
        let err = map_api_error(401, &body, None, "test-model");
        assert!(matches!(err, ExecutorError::Authentication { .. }));
        assert!(!err.is_retryable());
    }

    #[test]
    fn error_mapping_403_permission() {
        let body = sample_error_response_json("PERMISSION_DENIED", "Permission denied");
        let err = map_api_error(403, &body, None, "test-model");
        assert!(matches!(err, ExecutorError::Authentication { .. }));
        assert!(!err.is_retryable());
    }

    #[test]
    fn error_mapping_429_rate_limit() {
        let body = sample_error_response_json("RESOURCE_EXHAUSTED", "Rate limit exceeded");
        let err = map_api_error(429, &body, None, "test-model");
        assert!(matches!(err, ExecutorError::RateLimit { .. }));
        assert!(err.is_retryable());
    }

    #[test]
    fn error_mapping_429_with_retry_after() {
        let err = map_api_error(429, "{}", Some(30), "test-model");
        match err {
            ExecutorError::RateLimit { retry_after_secs } => {
                assert_eq!(retry_after_secs, Some(30));
            }
            _ => panic!("expected RateLimit"),
        }
    }

    #[test]
    fn error_mapping_400_bad_request() {
        let body = sample_error_response_json("INVALID_ARGUMENT", "messages is a required field");
        let err = map_api_error(400, &body, None, "test-model");
        // 400 without context/content keywords
        assert!(!matches!(err, ExecutorError::ContextWindowExceeded { .. }));
    }

    #[test]
    fn error_mapping_400_context_window() {
        let body = r#"{"error":{"message":"context window exceeded: too many tokens","status":"INVALID_ARGUMENT","code":400}}"#;
        let err = map_api_error(400, body, None, "test-model");
        assert!(matches!(err, ExecutorError::ContextWindowExceeded { .. }));
    }

    #[test]
    fn error_mapping_404_model_not_found() {
        let body = sample_error_response_json("NOT_FOUND", "Model not found");
        let err = map_api_error(404, &body, None, "gemini-nonexistent");
        assert!(matches!(err, ExecutorError::ModelNotFound { .. }));
    }

    #[test]
    fn error_mapping_500_server_error() {
        let body = sample_error_response_json("INTERNAL", "internal server error");
        let err = map_api_error(500, &body, None, "test-model");
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
        let body = sample_error_response_json("UNAVAILABLE", "service unavailable");
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
    fn error_mapping_handles_invalid_json_body() {
        let err = map_api_error(500, "not json at all", None, "test-model");
        assert!(matches!(
            err,
            ExecutorError::Transport {
                retryable: true,
                ..
            }
        ));
    }

    // -----------------------------------------------------------------------
    // SSE / streaming parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_sse_text_stream() {
        let raw = sample_sse_text_stream();
        let chunks = parse_streaming_response(&raw);
        // 4 lines, one is [DONE] which is skipped = 3 chunks.
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn parse_sse_tool_stream() {
        let raw = sample_sse_tool_stream();
        let chunks = parse_streaming_response(&raw);
        // 2 lines, one is [DONE] which is skipped = 1 chunk.
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn parse_json_array_stream() {
        let raw = sample_json_array_stream();
        let chunks = parse_streaming_response(&raw);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn parse_sse_done_marker_skipped() {
        let raw = "data: [DONE]\n";
        let chunks = parse_streaming_response(raw);
        assert!(chunks.is_empty());
    }

    #[test]
    fn parse_sse_ignores_invalid_json() {
        let raw = "data: {invalid json}\n";
        let chunks = parse_streaming_response(raw);
        assert!(chunks.is_empty());
    }

    // -----------------------------------------------------------------------
    // SSE processing tests
    // -----------------------------------------------------------------------

    #[test]
    fn process_sse_text_stream_produces_correct_events() {
        let raw = sample_sse_text_stream();
        let chunks = parse_streaming_response(&raw);
        let events = process_streaming_chunks(chunks);

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
        let chunks = parse_streaming_response(&raw);
        let events = process_streaming_chunks(chunks);

        let mut has_tool_delta = false;
        let mut has_tool_complete = false;
        let mut has_completed = false;

        for event in &events {
            match event.as_ref().expect("no error") {
                StreamEvent::ToolCallDelta {
                    tool_call_id, name, ..
                } => {
                    has_tool_delta = true;
                    assert_eq!(tool_call_id, "gemini_call_0");
                    assert_eq!(name.as_deref(), Some("file_read"));
                }
                StreamEvent::ToolCallComplete { call } => {
                    has_tool_complete = true;
                    assert_eq!(call.tool_call_id, "gemini_call_0");
                    assert_eq!(call.tool_name, "file_read");
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

        assert!(has_tool_delta);
        assert!(has_tool_complete);
        assert!(has_completed);
    }

    #[test]
    fn process_json_array_stream_produces_correct_events() {
        let raw = sample_json_array_stream();
        let chunks = parse_streaming_response(&raw);
        let events = process_streaming_chunks(chunks);

        let mut text_deltas = Vec::new();
        let mut has_completed = false;

        for event in &events {
            match event.as_ref().expect("no error") {
                StreamEvent::TextDelta { delta } => text_deltas.push(delta.clone()),
                StreamEvent::Completed { result } => {
                    has_completed = true;
                    assert_eq!(result.text, "Hello world");
                    assert_eq!(result.stop_reason, "end_turn");
                }
                _ => {}
            }
        }

        assert_eq!(text_deltas, vec!["Hello", " world"]);
        assert!(has_completed);
    }

    // -----------------------------------------------------------------------
    // Builder / constructor tests
    // -----------------------------------------------------------------------

    #[test]
    fn new_creates_executor_with_defaults() {
        let exec = GeminiExecutor::new("AIza-test".into(), "gemini-2.5-flash".into());
        assert_eq!(exec.api_key, "AIza-test");
        assert_eq!(exec.model, "gemini-2.5-flash");
        assert_eq!(
            exec.base_url,
            "https://generativelanguage.googleapis.com/v1beta"
        );
        assert_eq!(exec.max_retries, 3);
    }

    #[test]
    fn with_base_url_overrides_default() {
        let exec = GeminiExecutor::new_builder("key".into(), "model".into())
            .with_base_url("http://localhost:8080".into());
        assert_eq!(exec.base_url, "http://localhost:8080");
    }

    #[test]
    fn with_max_retries_overrides_default() {
        let exec = GeminiExecutor::new_builder("key".into(), "model".into()).with_max_retries(5);
        assert_eq!(exec.max_retries, 5);
    }

    #[test]
    fn with_timeout_creates_new_client() {
        let exec = GeminiExecutor::new_builder("key".into(), "model".into())
            .with_timeout(Duration::from_secs(30));
        // Just verify it doesn't panic.
        assert_eq!(exec.model, "model");
    }

    #[test]
    fn builder_chain_works() {
        let exec = GeminiExecutor::new_builder("key".into(), "model".into())
            .with_base_url("http://localhost:8080".into())
            .with_max_retries(1)
            .with_timeout(Duration::from_secs(60))
            .build();

        assert_eq!(exec.base_url, "http://localhost:8080");
        assert_eq!(exec.max_retries, 1);
    }

    #[test]
    fn new_builder_returns_non_arc() {
        let exec = GeminiExecutor::new_builder("key".into(), "model".into());
        assert_eq!(exec.api_key, "key");
        assert_eq!(exec.model, "model");
    }

    #[test]
    fn build_returns_arc() {
        let exec = GeminiExecutor::new_builder("key".into(), "model".into()).build();
        let _clone = Arc::clone(&exec);
        assert_eq!(exec.model, "model");
    }

    #[test]
    fn with_max_concurrent_sets_semaphore() {
        let exec = GeminiExecutor::new_builder("key".into(), "model".into()).with_max_concurrent(5);
        assert!(exec.concurrency_semaphore.is_some());
    }

    /// Compile-time check: `GeminiExecutor` implements `ModelExecutor`.
    #[allow(dead_code)]
    fn _gemini_executor_implements_model_executor(e: &GeminiExecutor) {
        let _: &dyn ModelExecutor = e;
    }

    // -----------------------------------------------------------------------
    // URL construction tests
    // -----------------------------------------------------------------------

    #[test]
    fn generate_content_url_is_correct() {
        let exec = GeminiExecutor::new_builder("test-key".into(), "model".into());
        let url = exec.generate_content_url("gemini-2.5-flash");
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent?key=test-key"
        );
    }

    #[test]
    fn stream_generate_content_url_is_correct() {
        let exec = GeminiExecutor::new_builder("test-key".into(), "model".into());
        let url = exec.stream_generate_content_url("gemini-2.5-pro");
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse&key=test-key"
        );
    }

    #[test]
    fn custom_base_url_reflected_in_generate_content_url() {
        let exec = GeminiExecutor::new_builder("key".into(), "model".into())
            .with_base_url("http://localhost:9000".into());
        let url = exec.generate_content_url("gemini-2.5-flash");
        assert!(url.starts_with("http://localhost:9000/"));
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

        let contents = to_gemini_contents(&msg);
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role.as_deref(), Some("model"));
        assert_eq!(contents[0].parts.len(), 2);
        assert_eq!(contents[0].parts[0].text.as_deref(), Some("Let me check."));
        let fc = contents[0].parts[1]
            .function_call
            .as_ref()
            .expect("function_call");
        assert_eq!(fc.name, "lookup");
    }

    #[test]
    fn user_message_with_tool_result() {
        let msg = InferenceMessage {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_call_id: "lookup".into(),
                content: "result data".into(),
                is_error: false,
            }],
        };

        let contents = to_gemini_contents(&msg);
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role.as_deref(), Some("user"));
        let fr = contents[0].parts[0]
            .function_response
            .as_ref()
            .expect("function_response");
        assert_eq!(fr.name, "lookup");
        assert_eq!(fr.response["result"], "result data");
    }

    #[test]
    fn assistant_tool_use_without_text() {
        let msg = InferenceMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                tool_call_id: "call_002".into(),
                tool_name: "search".into(),
                arguments_json: "{}".into(),
            }],
        };

        let contents = to_gemini_contents(&msg);
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role.as_deref(), Some("model"));
        assert!(contents[0].parts[0].text.is_none());
        assert!(contents[0].parts[0].function_call.is_some());
    }

    #[test]
    fn invalid_tool_input_schema_uses_fallback() {
        let tool = ToolDefinition {
            name: "test".into(),
            description: "test tool".into(),
            input_schema_json: "not valid json".into(),
        };
        let decl = to_gemini_function_declaration(&tool);
        assert_eq!(decl.parameters["type"], "object");
    }

    #[test]
    fn invalid_tool_arguments_json_uses_fallback() {
        let msg = InferenceMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                tool_call_id: "call_x".into(),
                tool_name: "test".into(),
                arguments_json: "not valid json".into(),
            }],
        };
        let contents = to_gemini_contents(&msg);
        let fc = contents[0].parts[0].function_call.as_ref().expect("fc");
        assert_eq!(fc.args, serde_json::json!({}));
    }

    // -----------------------------------------------------------------------
    // Response handling edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn empty_candidates_produces_empty_response() {
        let resp = GenerateContentResponse {
            candidates: Some(vec![]),
            usage_metadata: None,
            model_version: None,
        };
        let response = to_inference_response(&resp);
        assert!(response.text.is_empty());
        assert!(response.tool_calls.is_empty());
    }

    #[test]
    fn no_candidates_produces_empty_response() {
        let resp = GenerateContentResponse {
            candidates: None,
            usage_metadata: None,
            model_version: None,
        };
        let response = to_inference_response(&resp);
        assert!(response.text.is_empty());
        assert!(response.tool_calls.is_empty());
        // "STOP" is mapped to "end_turn" by map_finish_reason.
        assert_eq!(response.stop_reason, "end_turn");
    }

    #[test]
    fn missing_usage_metadata_defaults_to_zero() {
        let resp = GenerateContentResponse {
            candidates: Some(vec![GeminiCandidate {
                content: Some(GeminiContent {
                    role: Some("model".into()),
                    parts: vec![GeminiPart {
                        text: Some("hi".into()),
                        function_call: None,
                        function_response: None,
                    }],
                }),
                finish_reason: Some("STOP".into()),
                safety_ratings: None,
            }]),
            usage_metadata: None,
            model_version: None,
        };
        let response = to_inference_response(&resp);
        assert_eq!(response.usage.input_tokens, 0);
        assert_eq!(response.usage.output_tokens, 0);
    }

    #[test]
    fn multiple_tool_calls_have_unique_ids() {
        let resp_json = serde_json::json!({
            "candidates": [{
                "content": {
                    "parts": [
                        {"functionCall": {"name": "tool_a", "args": {"x": 1}}},
                        {"functionCall": {"name": "tool_b", "args": {"y": 2}}}
                    ],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 5, "totalTokenCount": 15}
        });
        let parsed: GenerateContentResponse = serde_json::from_value(resp_json).expect("parse");
        let response = to_inference_response(&parsed);

        assert_eq!(response.tool_calls.len(), 2);
        assert_eq!(response.tool_calls[0].tool_call_id, "gemini_call_0");
        assert_eq!(response.tool_calls[1].tool_call_id, "gemini_call_1");
        assert_ne!(
            response.tool_calls[0].tool_call_id,
            response.tool_calls[1].tool_call_id
        );
    }
}

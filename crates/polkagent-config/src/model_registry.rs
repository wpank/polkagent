//! Model registry: built-in catalog of known models and their capabilities.
//!
//! The registry provides [`ModelDescriptor`] entries for every model the
//! platform knows about out-of-the-box (see [`BuiltInModelCatalog`]), plus a
//! [`ModelCatalog`] trait so users can extend the catalog with custom entries.
//!
//! # Standard provider synthesis
//!
//! [`synthesize_providers_from_env`] inspects well-known environment variables
//! (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc.) and returns zero-config
//! [`ProviderConfig`] entries for every provider whose key is present.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::schema::ProviderConfig;

// ---------------------------------------------------------------------------
// ProviderKind
// ---------------------------------------------------------------------------

/// Enumeration of known provider backends.
///
/// Used by [`ModelDescriptor`] and by the provider synthesis logic to determine
/// which executor implementation to instantiate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Anthropic Messages API.
    AnthropicApi,
    /// Any provider that exposes an OpenAI-compatible `/v1/chat/completions` endpoint.
    OpenaiCompat,
    /// A locally-hosted model (e.g. llama.cpp, Ollama).
    Local,
    /// Google Gemini native API.
    GeminiApi,
    /// OpenRouter aggregation gateway.
    Openrouter,
    /// AWS Bedrock.
    Bedrock,
    /// Azure OpenAI Service.
    AzureOpenai,
    /// Perplexity Sonar API.
    PerplexityApi,
    /// Cerebras inference API.
    CerebrasApi,
}

// ---------------------------------------------------------------------------
// ToolFormat
// ---------------------------------------------------------------------------

/// Wire format used to represent tool definitions and tool-use messages for a
/// given model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFormat {
    /// Anthropic-style tool-use blocks (`tool_use` / `tool_result` content blocks).
    AnthropicBlocks,
    /// OpenAI-style JSON function calling (`tools` array, `tool_calls` in assistant turn).
    OpenAiJson,
    /// Google Gemini native function-declaration format.
    GeminiNative,
    /// ReAct-style plain-text tool invocation (Thought / Action / Observation).
    ReActText,
}

// ---------------------------------------------------------------------------
// ModelDescriptor
// ---------------------------------------------------------------------------

/// Canonical description of a model's identity, capabilities, and pricing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDescriptor {
    /// Unique slug used to reference this model (e.g. `"claude-opus-4-6"`).
    pub slug: String,
    /// Identifier of the provider that serves this model.
    pub provider: String,
    /// Maximum token context window.
    pub context_window: u64,
    /// Maximum output tokens the model can produce. `None` if unbounded.
    pub max_output: Option<u64>,
    /// Whether the model supports structured tool calling.
    pub supports_tools: bool,
    /// Whether the model supports an extended thinking / chain-of-thought mode.
    pub supports_thinking: bool,
    /// Whether the model accepts image (vision) inputs.
    pub supports_vision: bool,
    /// Whether the model supports streaming responses.
    #[serde(default = "default_true")]
    pub supports_streaming: bool,
    /// Whether the model supports prompt caching.
    #[serde(default)]
    pub supports_caching: bool,
    /// Whether the model supports structured output (JSON schema constrained).
    #[serde(default)]
    pub supports_structured_output: bool,
    /// Whether the model has built-in web search capability.
    #[serde(default)]
    pub supports_web_search: bool,
    /// Wire format for tool definitions and tool-use messages.
    pub tool_format: ToolFormat,
    /// If `true`, use `max_completion_tokens` instead of `max_tokens` in the
    /// request (OpenAI o-series behaviour).
    #[serde(default)]
    pub use_max_completion_tokens: bool,
    /// Cost per million input tokens (USD). `None` if unknown or free-tier.
    pub cost_input_per_m: Option<f64>,
    /// Cost per million output tokens (USD).
    pub cost_output_per_m: Option<f64>,
    /// Cost per million cache-read tokens (USD).
    pub cost_cache_read_per_m: Option<f64>,
    /// Cost per million cache-write tokens (USD).
    pub cost_cache_write_per_m: Option<f64>,
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// ModelCatalog trait
// ---------------------------------------------------------------------------

/// Read-only catalog of known models.
///
/// The platform ships with [`BuiltInModelCatalog`]; users may layer custom
/// entries on top.
pub trait ModelCatalog {
    /// Return every model in the catalog.
    fn list(&self) -> Vec<&ModelDescriptor>;

    /// Look up a model by its slug.
    fn get(&self, slug: &str) -> Option<&ModelDescriptor>;

    /// Return models that satisfy **all** of the given capability predicates.
    ///
    /// Each predicate receives a `&ModelDescriptor` and returns `true` if the
    /// model meets that criterion.
    fn find_capable(&self, predicates: &[fn(&ModelDescriptor) -> bool]) -> Vec<&ModelDescriptor> {
        self.list()
            .into_iter()
            .filter(|m| predicates.iter().all(|p| p(m)))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// BuiltInModelCatalog
// ---------------------------------------------------------------------------

/// The default model catalog shipped with the platform.
///
/// Contains the 13 models specified in PRD-04a section 3.1.
pub struct BuiltInModelCatalog {
    models: HashMap<String, ModelDescriptor>,
}

impl BuiltInModelCatalog {
    /// Build the catalog with all built-in models.
    #[must_use]
    pub fn new() -> Self {
        let descriptors = vec![
            // ── Anthropic ──────────────────────────────────────────────
            ModelDescriptor {
                slug: "claude-opus-4-6".into(),
                provider: "anthropic".into(),
                context_window: 200_000,
                max_output: Some(32_000),
                supports_tools: true,
                supports_thinking: true,
                supports_vision: true,
                supports_streaming: true,
                supports_caching: true,
                supports_structured_output: false,
                supports_web_search: false,
                tool_format: ToolFormat::AnthropicBlocks,
                use_max_completion_tokens: false,
                cost_input_per_m: Some(15.0),
                cost_output_per_m: Some(75.0),
                cost_cache_read_per_m: Some(1.5),
                cost_cache_write_per_m: Some(18.75),
            },
            ModelDescriptor {
                slug: "claude-sonnet-4-6".into(),
                provider: "anthropic".into(),
                context_window: 200_000,
                max_output: Some(16_000),
                supports_tools: true,
                supports_thinking: true,
                supports_vision: true,
                supports_streaming: true,
                supports_caching: true,
                supports_structured_output: false,
                supports_web_search: false,
                tool_format: ToolFormat::AnthropicBlocks,
                use_max_completion_tokens: false,
                cost_input_per_m: Some(3.0),
                cost_output_per_m: Some(15.0),
                cost_cache_read_per_m: Some(0.3),
                cost_cache_write_per_m: Some(3.75),
            },
            ModelDescriptor {
                slug: "claude-haiku-4-5".into(),
                provider: "anthropic".into(),
                context_window: 200_000,
                max_output: Some(8_192),
                supports_tools: true,
                supports_thinking: false,
                supports_vision: true,
                supports_streaming: true,
                supports_caching: true,
                supports_structured_output: false,
                supports_web_search: false,
                tool_format: ToolFormat::AnthropicBlocks,
                use_max_completion_tokens: false,
                cost_input_per_m: Some(0.8),
                cost_output_per_m: Some(4.0),
                cost_cache_read_per_m: Some(0.08),
                cost_cache_write_per_m: Some(1.0),
            },
            // ── OpenAI ─────────────────────────────────────────────────
            ModelDescriptor {
                slug: "gpt-5.5".into(),
                provider: "openai".into(),
                context_window: 200_000,
                max_output: Some(32_768),
                supports_tools: true,
                supports_thinking: false,
                supports_vision: true,
                supports_streaming: true,
                supports_caching: false,
                supports_structured_output: true,
                supports_web_search: false,
                tool_format: ToolFormat::OpenAiJson,
                use_max_completion_tokens: false,
                cost_input_per_m: Some(2.0),
                cost_output_per_m: Some(10.0),
                cost_cache_read_per_m: None,
                cost_cache_write_per_m: None,
            },
            ModelDescriptor {
                slug: "gpt-5.4-mini".into(),
                provider: "openai".into(),
                context_window: 128_000,
                max_output: Some(16_384),
                supports_tools: true,
                supports_thinking: false,
                supports_vision: true,
                supports_streaming: true,
                supports_caching: false,
                supports_structured_output: true,
                supports_web_search: false,
                tool_format: ToolFormat::OpenAiJson,
                use_max_completion_tokens: false,
                cost_input_per_m: Some(0.15),
                cost_output_per_m: Some(0.60),
                cost_cache_read_per_m: None,
                cost_cache_write_per_m: None,
            },
            ModelDescriptor {
                slug: "o3".into(),
                provider: "openai".into(),
                context_window: 200_000,
                max_output: Some(100_000),
                supports_tools: true,
                supports_thinking: true,
                supports_vision: true,
                supports_streaming: true,
                supports_caching: false,
                supports_structured_output: true,
                supports_web_search: false,
                tool_format: ToolFormat::OpenAiJson,
                use_max_completion_tokens: true,
                cost_input_per_m: Some(10.0),
                cost_output_per_m: Some(40.0),
                cost_cache_read_per_m: None,
                cost_cache_write_per_m: None,
            },
            ModelDescriptor {
                slug: "o4-mini".into(),
                provider: "openai".into(),
                context_window: 200_000,
                max_output: Some(100_000),
                supports_tools: true,
                supports_thinking: true,
                supports_vision: true,
                supports_streaming: true,
                supports_caching: false,
                supports_structured_output: true,
                supports_web_search: false,
                tool_format: ToolFormat::OpenAiJson,
                use_max_completion_tokens: true,
                cost_input_per_m: Some(1.10),
                cost_output_per_m: Some(4.40),
                cost_cache_read_per_m: None,
                cost_cache_write_per_m: None,
            },
            ModelDescriptor {
                slug: "gpt-4o".into(),
                provider: "openai".into(),
                context_window: 128_000,
                max_output: Some(16_384),
                supports_tools: true,
                supports_thinking: false,
                supports_vision: true,
                supports_streaming: true,
                supports_caching: false,
                supports_structured_output: true,
                supports_web_search: false,
                tool_format: ToolFormat::OpenAiJson,
                use_max_completion_tokens: false,
                cost_input_per_m: Some(2.50),
                cost_output_per_m: Some(10.0),
                cost_cache_read_per_m: None,
                cost_cache_write_per_m: None,
            },
            ModelDescriptor {
                slug: "codex-mini".into(),
                provider: "openai".into(),
                context_window: 200_000,
                max_output: Some(100_000),
                supports_tools: true,
                supports_thinking: true,
                supports_vision: false,
                supports_streaming: true,
                supports_caching: false,
                supports_structured_output: true,
                supports_web_search: false,
                tool_format: ToolFormat::OpenAiJson,
                use_max_completion_tokens: true,
                cost_input_per_m: Some(1.50),
                cost_output_per_m: Some(6.0),
                cost_cache_read_per_m: None,
                cost_cache_write_per_m: None,
            },
            // ── Gemini ─────────────────────────────────────────────────
            ModelDescriptor {
                slug: "gemini-2.5-pro".into(),
                provider: "gemini".into(),
                context_window: 1_048_576,
                max_output: Some(65_536),
                supports_tools: true,
                supports_thinking: true,
                supports_vision: true,
                supports_streaming: true,
                supports_caching: true,
                supports_structured_output: true,
                supports_web_search: true,
                tool_format: ToolFormat::GeminiNative,
                use_max_completion_tokens: false,
                cost_input_per_m: Some(1.25),
                cost_output_per_m: Some(10.0),
                cost_cache_read_per_m: Some(0.315),
                cost_cache_write_per_m: None,
            },
            ModelDescriptor {
                slug: "gemini-2.5-flash".into(),
                provider: "gemini".into(),
                context_window: 1_048_576,
                max_output: Some(65_536),
                supports_tools: true,
                supports_thinking: true,
                supports_vision: true,
                supports_streaming: true,
                supports_caching: true,
                supports_structured_output: true,
                supports_web_search: true,
                tool_format: ToolFormat::GeminiNative,
                use_max_completion_tokens: false,
                cost_input_per_m: Some(0.15),
                cost_output_per_m: Some(0.60),
                cost_cache_read_per_m: Some(0.0375),
                cost_cache_write_per_m: None,
            },
            // ── OpenRouter ────────────────────────────────────────────
            ModelDescriptor {
                slug: "openrouter/auto".into(),
                provider: "openrouter".into(),
                context_window: 200_000,
                max_output: Some(32_000),
                supports_tools: true,
                supports_thinking: false,
                supports_vision: true,
                supports_streaming: true,
                supports_caching: false,
                supports_structured_output: true,
                supports_web_search: false,
                tool_format: ToolFormat::OpenAiJson,
                use_max_completion_tokens: false,
                cost_input_per_m: None,
                cost_output_per_m: None,
                cost_cache_read_per_m: None,
                cost_cache_write_per_m: None,
            },
            // ── Perplexity ─────────────────────────────────────────────
            ModelDescriptor {
                slug: "sonar-pro".into(),
                provider: "perplexity".into(),
                context_window: 200_000,
                max_output: Some(8_000),
                supports_tools: false,
                supports_thinking: false,
                supports_vision: false,
                supports_streaming: true,
                supports_caching: false,
                supports_structured_output: false,
                supports_web_search: true,
                tool_format: ToolFormat::OpenAiJson,
                use_max_completion_tokens: false,
                cost_input_per_m: Some(3.0),
                cost_output_per_m: Some(15.0),
                cost_cache_read_per_m: None,
                cost_cache_write_per_m: None,
            },
            ModelDescriptor {
                slug: "sonar".into(),
                provider: "perplexity".into(),
                context_window: 128_000,
                max_output: Some(8_000),
                supports_tools: false,
                supports_thinking: false,
                supports_vision: false,
                supports_streaming: true,
                supports_caching: false,
                supports_structured_output: false,
                supports_web_search: true,
                tool_format: ToolFormat::OpenAiJson,
                use_max_completion_tokens: false,
                cost_input_per_m: Some(1.0),
                cost_output_per_m: Some(1.0),
                cost_cache_read_per_m: None,
                cost_cache_write_per_m: None,
            },
        ];

        let mut models = HashMap::with_capacity(descriptors.len());
        for desc in descriptors {
            models.insert(desc.slug.clone(), desc);
        }

        Self { models }
    }
}

impl Default for BuiltInModelCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelCatalog for BuiltInModelCatalog {
    fn list(&self) -> Vec<&ModelDescriptor> {
        self.models.values().collect()
    }

    fn get(&self, slug: &str) -> Option<&ModelDescriptor> {
        self.models.get(slug)
    }
}

// ---------------------------------------------------------------------------
// Standard provider synthesis from environment variables (§ 2.2)
// ---------------------------------------------------------------------------

/// Well-known environment variables and the provider configs they synthesize.
struct EnvProviderSpec {
    env_var: &'static str,
    id: &'static str,
    provider_type: &'static str,
    kind: ProviderKind,
    base_url: &'static str,
    default_model: &'static str,
}

const ENV_PROVIDER_SPECS: &[EnvProviderSpec] = &[
    EnvProviderSpec {
        env_var: "ANTHROPIC_API_KEY",
        id: "anthropic",
        provider_type: "anthropic",
        kind: ProviderKind::AnthropicApi,
        base_url: "https://api.anthropic.com",
        default_model: "claude-sonnet-4-6",
    },
    EnvProviderSpec {
        env_var: "OPENAI_API_KEY",
        id: "openai",
        provider_type: "openai_compatible",
        kind: ProviderKind::OpenaiCompat,
        base_url: "https://api.openai.com/v1",
        default_model: "gpt-4o",
    },
    EnvProviderSpec {
        env_var: "GEMINI_API_KEY",
        id: "gemini",
        provider_type: "gemini",
        kind: ProviderKind::GeminiApi,
        base_url: "https://generativelanguage.googleapis.com",
        default_model: "gemini-2.5-flash",
    },
    EnvProviderSpec {
        env_var: "OPENROUTER_API_KEY",
        id: "openrouter",
        provider_type: "openrouter",
        kind: ProviderKind::Openrouter,
        base_url: "https://openrouter.ai/api/v1",
        default_model: "anthropic/claude-sonnet-4-6",
    },
    EnvProviderSpec {
        env_var: "PERPLEXITY_API_KEY",
        id: "perplexity",
        provider_type: "openai_compatible",
        kind: ProviderKind::PerplexityApi,
        base_url: "https://api.perplexity.ai",
        default_model: "sonar",
    },
    EnvProviderSpec {
        env_var: "CEREBRAS_API_KEY",
        id: "cerebras",
        provider_type: "openai_compatible",
        kind: ProviderKind::CerebrasApi,
        base_url: "https://api.cerebras.ai/v1",
        default_model: "llama-4-scout-17b-16e",
    },
];

/// Inspect well-known environment variables and return a [`ProviderConfig`]
/// for each provider whose API key is set.
///
/// This allows zero-config usage: if the user has `ANTHROPIC_API_KEY` set,
/// an `anthropic` provider is available without any TOML configuration.
///
/// The `env_lookup` parameter is a closure so callers can inject a custom
/// environment for testing. In production, pass `std::env::var`.
pub fn synthesize_providers_from_env<F>(env_lookup: F) -> Vec<(ProviderKind, ProviderConfig)>
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    ENV_PROVIDER_SPECS
        .iter()
        .filter(|spec| env_lookup(spec.env_var).is_ok_and(|v| !v.is_empty()))
        .map(|spec| {
            let config = ProviderConfig {
                id: spec.id.to_owned(),
                provider_type: spec.provider_type.to_owned(),
                api_key_env: spec.env_var.to_owned(),
                base_url: spec.base_url.to_owned(),
                default_model: spec.default_model.to_owned(),
                kind: Some(spec.kind),
                ..Default::default()
            };
            (spec.kind, config)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Catalog completeness ───────────────────────────────────────────

    #[test]
    fn builtin_catalog_has_14_models() {
        let catalog = BuiltInModelCatalog::new();
        assert_eq!(catalog.list().len(), 14);
    }

    #[test]
    fn all_expected_slugs_present() {
        let catalog = BuiltInModelCatalog::new();
        let expected = [
            "claude-opus-4-6",
            "claude-sonnet-4-6",
            "claude-haiku-4-5",
            "gpt-5.5",
            "gpt-5.4-mini",
            "o3",
            "o4-mini",
            "gpt-4o",
            "codex-mini",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            "openrouter/auto",
            "sonar-pro",
            "sonar",
        ];
        for slug in &expected {
            assert!(
                catalog.get(slug).is_some(),
                "missing model: {slug}"
            );
        }
    }

    // ── get() ──────────────────────────────────────────────────────────

    #[test]
    fn get_returns_correct_model() {
        let catalog = BuiltInModelCatalog::new();
        let model = catalog.get("claude-opus-4-6").expect("should exist");
        assert_eq!(model.slug, "claude-opus-4-6");
        assert_eq!(model.provider, "anthropic");
        assert_eq!(model.context_window, 200_000);
        assert!(model.supports_tools);
        assert!(model.supports_thinking);
        assert!(model.supports_vision);
        assert!(model.supports_caching);
        assert_eq!(model.tool_format, ToolFormat::AnthropicBlocks);
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let catalog = BuiltInModelCatalog::new();
        assert!(catalog.get("nonexistent-model").is_none());
    }

    // ── find_capable() ─────────────────────────────────────────────────

    #[test]
    fn find_capable_filters_by_tools() {
        let catalog = BuiltInModelCatalog::new();
        let with_tools = catalog.find_capable(&[|m| m.supports_tools]);
        // Perplexity models (sonar, sonar-pro) do not support tools.
        assert!(with_tools.len() >= 11);
        for m in &with_tools {
            assert!(m.supports_tools, "{} should support tools", m.slug);
        }
    }

    #[test]
    fn find_capable_filters_by_vision_and_thinking() {
        let catalog = BuiltInModelCatalog::new();
        let results = catalog.find_capable(&[|m| m.supports_vision, |m| m.supports_thinking]);
        // All results must have both.
        for m in &results {
            assert!(m.supports_vision, "{} should support vision", m.slug);
            assert!(m.supports_thinking, "{} should support thinking", m.slug);
        }
        // At minimum: claude-opus-4-6, claude-sonnet-4-6, o3, o4-mini, gemini-2.5-pro, gemini-2.5-flash
        assert!(results.len() >= 6);
    }

    #[test]
    fn find_capable_empty_predicates_returns_all() {
        let catalog = BuiltInModelCatalog::new();
        let all = catalog.find_capable(&[]);
        assert_eq!(all.len(), 14);
    }

    #[test]
    fn find_capable_web_search() {
        let catalog = BuiltInModelCatalog::new();
        let results = catalog.find_capable(&[|m| m.supports_web_search]);
        let slugs: Vec<&str> = results.iter().map(|m| m.slug.as_str()).collect();
        assert!(slugs.contains(&"sonar"));
        assert!(slugs.contains(&"sonar-pro"));
        assert!(slugs.contains(&"gemini-2.5-pro"));
        assert!(slugs.contains(&"gemini-2.5-flash"));
    }

    #[test]
    fn find_capable_max_completion_tokens() {
        let catalog = BuiltInModelCatalog::new();
        let results = catalog.find_capable(&[|m| m.use_max_completion_tokens]);
        let slugs: Vec<&str> = results.iter().map(|m| m.slug.as_str()).collect();
        assert!(slugs.contains(&"o3"));
        assert!(slugs.contains(&"o4-mini"));
        assert!(slugs.contains(&"codex-mini"));
    }

    // ── ProviderKind serde round-trip ──────────────────────────────────

    #[test]
    fn provider_kind_serde_round_trip() {
        let variants = [
            ProviderKind::AnthropicApi,
            ProviderKind::OpenaiCompat,
            ProviderKind::Local,
            ProviderKind::GeminiApi,
            ProviderKind::Openrouter,
            ProviderKind::Bedrock,
            ProviderKind::AzureOpenai,
            ProviderKind::PerplexityApi,
            ProviderKind::CerebrasApi,
        ];
        for variant in &variants {
            let json = serde_json::to_string(variant).expect("serialize");
            let back: ProviderKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*variant, back);
        }
    }

    #[test]
    fn provider_kind_serializes_snake_case() {
        let json = serde_json::to_string(&ProviderKind::AnthropicApi).expect("serialize");
        assert_eq!(json, "\"anthropic_api\"");
        let json = serde_json::to_string(&ProviderKind::OpenaiCompat).expect("serialize");
        assert_eq!(json, "\"openai_compat\"");
        let json = serde_json::to_string(&ProviderKind::AzureOpenai).expect("serialize");
        assert_eq!(json, "\"azure_openai\"");
    }

    // ── ToolFormat serde round-trip ────────────────────────────────────

    #[test]
    fn tool_format_serde_round_trip() {
        let variants = [
            ToolFormat::AnthropicBlocks,
            ToolFormat::OpenAiJson,
            ToolFormat::GeminiNative,
            ToolFormat::ReActText,
        ];
        for variant in &variants {
            let json = serde_json::to_string(variant).expect("serialize");
            let back: ToolFormat = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*variant, back);
        }
    }

    #[test]
    fn tool_format_serializes_snake_case() {
        let json = serde_json::to_string(&ToolFormat::AnthropicBlocks).expect("serialize");
        assert_eq!(json, "\"anthropic_blocks\"");
        let json = serde_json::to_string(&ToolFormat::OpenAiJson).expect("serialize");
        assert_eq!(json, "\"open_ai_json\"");
    }

    // ── ModelDescriptor serde round-trip ───────────────────────────────

    #[test]
    fn model_descriptor_serde_round_trip() {
        let catalog = BuiltInModelCatalog::new();
        let model = catalog.get("claude-opus-4-6").expect("should exist");
        let json = serde_json::to_string(model).expect("serialize");
        let back: ModelDescriptor = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.slug, model.slug);
        assert_eq!(back.context_window, model.context_window);
        assert_eq!(back.tool_format, model.tool_format);
        assert_eq!(back.supports_tools, model.supports_tools);
    }

    // ── Env var synthesis ──────────────────────────────────────────────

    #[test]
    fn synthesize_with_no_env_vars_returns_empty() {
        let result = synthesize_providers_from_env(|_| Err(std::env::VarError::NotPresent));
        assert!(result.is_empty());
    }

    #[test]
    fn synthesize_with_anthropic_key() {
        let result = synthesize_providers_from_env(|var| {
            if var == "ANTHROPIC_API_KEY" {
                Ok("sk-ant-test".to_owned())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        });
        assert_eq!(result.len(), 1);
        let (kind, config) = &result[0];
        assert_eq!(*kind, ProviderKind::AnthropicApi);
        assert_eq!(config.id, "anthropic");
        assert_eq!(config.provider_type, "anthropic");
        assert_eq!(config.api_key_env, "ANTHROPIC_API_KEY");
        assert_eq!(config.base_url, "https://api.anthropic.com");
        assert_eq!(config.default_model, "claude-sonnet-4-6");
        assert_eq!(config.kind, Some(ProviderKind::AnthropicApi));
    }

    #[test]
    fn synthesize_with_multiple_keys() {
        let result = synthesize_providers_from_env(|var| match var {
            "ANTHROPIC_API_KEY" => Ok("sk-ant-test".to_owned()),
            "OPENAI_API_KEY" => Ok("sk-test".to_owned()),
            "PERPLEXITY_API_KEY" => Ok("pplx-test".to_owned()),
            _ => Err(std::env::VarError::NotPresent),
        });
        assert_eq!(result.len(), 3);
        let kinds: Vec<ProviderKind> = result.iter().map(|(k, _)| *k).collect();
        assert!(kinds.contains(&ProviderKind::AnthropicApi));
        assert!(kinds.contains(&ProviderKind::OpenaiCompat));
        assert!(kinds.contains(&ProviderKind::PerplexityApi));
    }

    #[test]
    fn synthesize_openai_has_correct_base_url() {
        let result = synthesize_providers_from_env(|var| {
            if var == "OPENAI_API_KEY" {
                Ok("sk-test".to_owned())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        });
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].1.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn synthesize_gemini_has_correct_config() {
        let result = synthesize_providers_from_env(|var| {
            if var == "GEMINI_API_KEY" {
                Ok("key".to_owned())
            } else {
                Err(std::env::VarError::NotPresent)
            }
        });
        assert_eq!(result.len(), 1);
        let (kind, config) = &result[0];
        assert_eq!(*kind, ProviderKind::GeminiApi);
        assert_eq!(config.id, "gemini");
        assert_eq!(config.default_model, "gemini-2.5-flash");
    }
}

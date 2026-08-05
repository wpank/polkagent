//! Model-provider adapter composition.

use std::collections::HashSet;
use std::sync::Arc;

use polkagent_config::model_registry::synthesize_providers_from_env;
use polkagent_config::{Config, ProviderConfig};
use polkagent_executor_anthropic::AnthropicExecutor;
use polkagent_executor_fake::FakeExecutor;
use polkagent_executor_gemini::GeminiExecutor;
use polkagent_executor_local::LocalExecutor;
use polkagent_executor_openai::OpenAiExecutor;
use polkagent_executor_openrouter::OpenRouterExecutor;
use polkagent_executor_trait::ModelExecutor;
use polkagent_service::ProviderRegistry;

use crate::{
    AdapterPolicy, ComponentReadiness, ReadinessWarning, RuntimeError, RuntimeOptions, WarningCode,
};

/// Provider components ready to pass into `AppServiceBuilder`.
pub(crate) struct ProviderBuild {
    pub(crate) registry: ProviderRegistry,
    pub(crate) executor: Option<Arc<dyn ModelExecutor>>,
    pub(crate) readiness: ComponentReadiness,
    pub(crate) warnings: Vec<ReadinessWarning>,
}

/// Compose configured and environment-synthesized providers deterministically.
pub(crate) fn build(
    config: &Config,
    options: &RuntimeOptions,
) -> Result<ProviderBuild, RuntimeError> {
    let mut configs = Vec::new();
    if options.discover_environment_providers {
        configs.extend(
            synthesize_providers_from_env(|key| std::env::var(key))
                .into_iter()
                .map(|(_, config)| config),
        );
    }
    // Config entries come last so the registry's replace behavior gives them
    // precedence over synthesized entries with the same identifier.
    configs.extend(config.providers.iter().cloned());

    let mut registry = ProviderRegistry::new();
    let mut registered_order = Vec::new();
    let mut registered_ids = HashSet::new();
    let mut registered_configs = Vec::new();
    for provider in configs {
        if let Some(executor) = executor_from_config(&provider, None) {
            registry
                .register(provider.clone(), executor)
                .map_err(|error| RuntimeError::Provider {
                    message: error.to_string(),
                })?;
            if registered_ids.insert(provider.id.clone()) {
                registered_order.push(provider.id.clone());
            }
            if let Some(index) = registered_configs
                .iter()
                .position(|candidate: &ProviderConfig| candidate.id == provider.id)
            {
                registered_configs[index] = provider;
            } else {
                registered_configs.push(provider);
            }
        }
    }

    let selected_id = options
        .provider_override
        .as_deref()
        .or(config.execution.default_provider.as_deref())
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(|| registered_order.first().cloned());

    let selected = selected_id
        .as_deref()
        .and_then(|id| registered_configs.iter().find(|provider| provider.id == id));

    if let Some(requested) = options.provider_override.as_deref() {
        if selected.is_none() {
            return Err(RuntimeError::ProviderUnavailable {
                provider: requested.to_owned(),
                reason: "not configured, unsupported, or its required API key is unset".to_owned(),
            });
        }
    }

    let mut warnings = Vec::new();
    let (executor, readiness) = if let Some(provider) = selected {
        let executor = executor_from_config(provider, options.model_override.as_deref())
            .ok_or_else(|| RuntimeError::ProviderUnavailable {
                provider: provider.id.clone(),
                reason: "adapter could not be constructed".to_owned(),
            })?;
        (
            Some(executor),
            ComponentReadiness::ready(format!(
                "provider '{}' registered (health not probed)",
                provider.id
            )),
        )
    } else if options.adapter_policy == AdapterPolicy::AllowSimulated {
        warnings.push(ReadinessWarning::new(
            WarningCode::SimulatedExecutor,
            "no concrete provider was available; using FakeExecutor by explicit policy",
        ));
        (
            Some(FakeExecutor::new() as Arc<dyn ModelExecutor>),
            ComponentReadiness::degraded("simulated FakeExecutor"),
        )
    } else {
        (
            None,
            ComponentReadiness::unavailable("no usable provider configured"),
        )
    };

    Ok(ProviderBuild {
        registry,
        executor,
        readiness,
        warnings,
    })
}

fn executor_from_config(
    provider: &ProviderConfig,
    model_override: Option<&str>,
) -> Option<Arc<dyn ModelExecutor>> {
    let model = model_override
        .filter(|model| !model.is_empty())
        .unwrap_or(&provider.default_model)
        .to_owned();
    let api_key = if provider.api_key_env.is_empty() {
        String::new()
    } else {
        std::env::var(&provider.api_key_env)
            .ok()
            .filter(|key| !key.is_empty())?
    };

    match provider.provider_type.as_str() {
        "anthropic" => Some(AnthropicExecutor::new(api_key, model)),
        "openai" | "openai_compatible" => Some(OpenAiExecutor::new(api_key, model)),
        "local" | "ollama" => {
            let url = if provider.base_url.is_empty() {
                "http://localhost:11434/v1".to_owned()
            } else {
                provider.base_url.clone()
            };
            Some(LocalExecutor::custom(url, model))
        }
        "gemini" => Some(GeminiExecutor::new(api_key, model)),
        "openrouter" => Some(OpenRouterExecutor::new(api_key, model)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulated_executor_is_never_silent() {
        let config = Config::default();
        let mut options = RuntimeOptions::new(".");
        options.adapter_policy = AdapterPolicy::AllowSimulated;
        options.discover_environment_providers = false;

        let built = build(&config, &options).expect("simulated build");
        assert_eq!(built.readiness.state, crate::ComponentState::Degraded);
        assert_eq!(built.warnings[0].code, WarningCode::SimulatedExecutor);
    }

    #[test]
    fn strict_missing_provider_returns_no_executor() {
        let config = Config::default();
        let mut options = RuntimeOptions::new(".");
        options.discover_environment_providers = false;

        let built = build(&config, &options).expect("provider scan");
        assert!(built.executor.is_none());
        assert_eq!(built.readiness.state, crate::ComponentState::Unavailable);
    }
}

//! External coding-harness composition.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use polkagent_config::Config;
use polkagent_harness_trait::{Harness, HarnessConfig};
use polkagent_service::HarnessRegistry;

use crate::{ComponentReadiness, ReadinessWarning, RuntimeError, RuntimeOptions, WarningCode};

/// Harness ready to pass into `AppServiceBuilder`.
pub(crate) struct HarnessBuild {
    pub(crate) harness: Option<Arc<dyn Harness>>,
    pub(crate) readiness: ComponentReadiness,
    pub(crate) warnings: Vec<ReadinessWarning>,
}

/// Probe and compose one supported harness without writing protocol output.
pub(crate) fn build(
    config: &Config,
    options: &RuntimeOptions,
    workdir: &std::path::Path,
) -> Result<HarnessBuild, RuntimeError> {
    if options.disable_harness
        || options
            .harness_override
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("none"))
    {
        return Ok(HarnessBuild {
            harness: None,
            readiness: ComponentReadiness::disabled("disabled by runtime options"),
            warnings: Vec::new(),
        });
    }

    let mut registry = HarnessRegistry::with_known_harnesses();
    for (id, entry) in &config.harness.harnesses {
        if let Some(path) = &entry.binary_path {
            registry.register_with_path(id, id, path.clone());
        } else {
            registry.register(id, id);
        }
    }
    if let Some(path) = config.harness.binary_path.as_ref() {
        let id = normalized_harness_name(&config.harness.harness_type);
        registry.register_with_path(id, id, path.to_string_lossy().into_owned());
    }

    let requested = options
        .harness_override
        .as_deref()
        .map(normalized_harness_name);
    let configured_default = config
        .harness
        .default
        .as_deref()
        .map(normalized_harness_name);
    let resolution = registry.resolve(requested, configured_default);

    let Some(name) = resolution.harness_name.as_deref() else {
        if let Some(explicit) = options.harness_override.as_deref() {
            return Err(RuntimeError::HarnessUnavailable {
                harness: explicit.to_owned(),
                reason: "binary was not found on PATH".to_owned(),
            });
        }
        let mut warnings = Vec::new();
        if let Some(note) = resolution.note {
            if note.starts_with("Warning:") {
                warnings.push(ReadinessWarning::new(WarningCode::HarnessUnavailable, note));
            }
        }
        return Ok(HarnessBuild {
            harness: None,
            readiness: ComponentReadiness::disabled("executor-only mode"),
            warnings,
        });
    };

    let canonical_name = normalized_harness_name(name);
    let mut harness_config = HarnessConfig::new(canonical_name);
    harness_config.executable_path = registry.binary_path(name).map(PathBuf::from);
    harness_config.workspace_path = Some(workdir.to_path_buf());
    harness_config.timeout = Duration::from_secs(config.harness.timeout_secs);

    let harness: Arc<dyn Harness> = match canonical_name {
        "codex" => Arc::new(
            polkagent_harness_codex::CodexHarness::new(
                harness_config,
                polkagent_harness_codex::CodexHarnessConfig::default(),
            )
            .map_err(|error| RuntimeError::HarnessUnavailable {
                harness: canonical_name.to_owned(),
                reason: error.to_string(),
            })?,
        ),
        "claude-code" => Arc::new(
            polkagent_harness_claude::ClaudeHarness::new(
                harness_config,
                polkagent_harness_claude::ClaudeHarnessConfig::default(),
            )
            .map_err(|error| RuntimeError::HarnessUnavailable {
                harness: canonical_name.to_owned(),
                reason: error.to_string(),
            })?,
        ),
        "cursor" => Arc::new(polkagent_harness_acp::AcpHarness::new(
            polkagent_harness_cursor::CursorConfigurator::default(),
            harness_config,
        )),
        unsupported => {
            if options.harness_override.is_some() {
                return Err(RuntimeError::HarnessUnavailable {
                    harness: unsupported.to_owned(),
                    reason: "runtime has no concrete adapter for this harness".to_owned(),
                });
            }
            return Ok(HarnessBuild {
                harness: None,
                readiness: ComponentReadiness::unavailable(format!(
                    "detected unsupported harness '{unsupported}'"
                )),
                warnings: vec![ReadinessWarning::new(
                    WarningCode::HarnessUnavailable,
                    format!("runtime has no concrete adapter for harness '{unsupported}'"),
                )],
            });
        }
    };

    Ok(HarnessBuild {
        harness: Some(harness),
        readiness: ComponentReadiness::ready(format!("harness '{canonical_name}' selected")),
        warnings: Vec::new(),
    })
}

fn normalized_harness_name(name: &str) -> &str {
    match name {
        "claude" => "claude-code",
        "cursor-acp" => "cursor",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_disable_never_probes() {
        let config = Config::default();
        let mut options = RuntimeOptions::new(".");
        options.disable_harness = true;

        let built =
            build(&config, &options, std::path::Path::new(".")).expect("disabled harness build");
        assert!(built.harness.is_none());
        assert_eq!(built.readiness.state, crate::ComponentState::Disabled);
    }

    #[test]
    fn aliases_match_supported_adapter_ids() {
        assert_eq!(normalized_harness_name("claude"), "claude-code");
        assert_eq!(normalized_harness_name("cursor-acp"), "cursor");
    }
}

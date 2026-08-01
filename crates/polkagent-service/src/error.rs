//! Error types for the `polkagent-service` crate.

use polkagent_core::{AgentId, EffectId, RunId};
use thiserror::Error;

/// Unified error type for all service-layer operations.
#[derive(Debug, Error)]
pub enum ServiceError {
    /// Configuration is invalid or missing required fields.
    #[error("configuration error: {message}")]
    Config {
        /// Human-readable description of the configuration problem.
        message: String,
    },

    /// The requested provider is not registered.
    #[error("provider not found: {provider_id}")]
    ProviderNotFound {
        /// The provider identifier that was requested.
        provider_id: String,
    },

    /// The requested agent does not exist.
    #[error("agent not found: {agent_id}")]
    AgentNotFound {
        /// The agent identifier that was requested.
        agent_id: AgentId,
    },

    /// The requested run does not exist.
    #[error("run not found: {run_id}")]
    RunNotFound {
        /// The run identifier that was requested.
        run_id: RunId,
    },

    /// The requested effect does not exist.
    #[error("effect not found: {effect_id}")]
    EffectNotFound {
        /// The effect identifier that was requested.
        effect_id: EffectId,
    },

    /// A run lifecycle state transition was invalid.
    #[error("invalid state transition: {message}")]
    InvalidTransition {
        /// Human-readable description of the failed transition.
        message: String,
    },

    /// The service has not been fully initialized (missing a required component).
    #[error("service not initialized: {component}")]
    NotInitialized {
        /// The name of the component that is missing.
        component: String,
    },

    /// The underlying store returned an error.
    #[error("store error: {message}")]
    Store {
        /// Human-readable description of the store error.
        message: String,
    },

    /// The model executor returned an error.
    #[error("executor error: {message}")]
    Executor {
        /// Human-readable description of the executor error.
        message: String,
    },

    /// The event subsystem returned an error.
    #[error("event error: {message}")]
    Event {
        /// Human-readable description of the event error.
        message: String,
    },

    /// An operation was attempted on a service that is shutting down.
    #[error("service is shutting down")]
    ShuttingDown,

    /// The explain-before-sign pipeline returned an error.
    #[error("explain pipeline error: {message}")]
    ExplainPipeline {
        /// Human-readable description of the pipeline error.
        message: String,
    },

    /// A hot-reload of the configuration failed.
    #[error("config reload failed: {message}")]
    ConfigReload {
        /// Human-readable description of the reload failure.
        message: String,
    },

    /// An unexpected internal error.
    #[error("internal service error: {message}")]
    Internal {
        /// Human-readable description of the internal error.
        message: String,
    },
}

impl From<polkagent_run::RunError> for ServiceError {
    fn from(err: polkagent_run::RunError) -> Self {
        match err {
            polkagent_run::RunError::NotFound(run_id) => {
                Self::RunNotFound { run_id }
            }
            polkagent_run::RunError::Transition(msg) => {
                Self::InvalidTransition {
                    message: msg.to_string(),
                }
            }
            polkagent_run::RunError::Store(msg) => Self::Store { message: msg },
            polkagent_run::RunError::Event(msg) => Self::Event { message: msg },
            polkagent_run::RunError::TurnNotFound => Self::Store {
                message: "turn not found".into(),
            },
            polkagent_run::RunError::AlreadyTerminal(run_id, state) => {
                Self::InvalidTransition {
                    message: format!(
                        "run {run_id} is already in terminal state ({state})"
                    ),
                }
            }
            polkagent_run::RunError::DeadlineExceeded(run_id) => {
                Self::InvalidTransition {
                    message: format!("run {run_id} exceeded its deadline"),
                }
            }
            polkagent_run::RunError::Serialization(err) => Self::Internal {
                message: format!("serialization error: {err}"),
            },
        }
    }
}

impl From<polkagent_store_trait::StoreError> for ServiceError {
    fn from(err: polkagent_store_trait::StoreError) -> Self {
        Self::Store {
            message: err.to_string(),
        }
    }
}

impl From<polkagent_event::EventError> for ServiceError {
    fn from(err: polkagent_event::EventError) -> Self {
        Self::Event {
            message: err.to_string(),
        }
    }
}

impl From<polkagent_config::ConfigError> for ServiceError {
    fn from(err: polkagent_config::ConfigError) -> Self {
        Self::Config {
            message: err.to_string(),
        }
    }
}

impl From<crate::explain::ExplainError> for ServiceError {
    fn from(err: crate::explain::ExplainError) -> Self {
        Self::ExplainPipeline {
            message: err.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_error_config_display() {
        let err = ServiceError::Config {
            message: "missing provider".into(),
        };
        assert!(format!("{err}").contains("missing provider"));
    }

    #[test]
    fn service_error_provider_not_found_display() {
        let err = ServiceError::ProviderNotFound {
            provider_id: "anthropic-default".into(),
        };
        assert!(format!("{err}").contains("anthropic-default"));
    }

    #[test]
    fn service_error_shutting_down_display() {
        let err = ServiceError::ShuttingDown;
        assert!(format!("{err}").contains("shutting down"));
    }

    #[test]
    fn service_error_from_store_error() {
        let store_err = polkagent_store_trait::StoreError::NotFound {
            resource_type: "Run",
            id: "abc".into(),
        };
        let service_err: ServiceError = store_err.into();
        assert!(matches!(service_err, ServiceError::Store { .. }));
    }
}

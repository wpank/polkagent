//! Structured runtime readiness reporting.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Readiness state for one composed subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentState {
    /// A concrete production implementation was initialized.
    Ready,
    /// The component can operate with an explicitly accepted limitation.
    Degraded,
    /// The component was intentionally disabled by configuration or options.
    Disabled,
    /// No usable implementation is currently composed.
    Unavailable,
}

impl ComponentState {
    /// Whether this state can currently serve requests.
    pub const fn is_operational(self) -> bool {
        matches!(self, Self::Ready | Self::Degraded)
    }
}

/// State and human-readable context for one runtime component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentReadiness {
    /// Machine-readable readiness state.
    pub state: ComponentState,
    /// Safe diagnostic detail. This field must never contain secrets.
    pub detail: String,
}

impl ComponentReadiness {
    /// Construct a component readiness entry.
    pub fn new(state: ComponentState, detail: impl Into<String>) -> Self {
        Self {
            state,
            detail: detail.into(),
        }
    }

    /// Construct a ready component.
    pub fn ready(detail: impl Into<String>) -> Self {
        Self::new(ComponentState::Ready, detail)
    }

    /// Construct a degraded but operational component.
    pub fn degraded(detail: impl Into<String>) -> Self {
        Self::new(ComponentState::Degraded, detail)
    }

    /// Construct an intentionally disabled component.
    pub fn disabled(detail: impl Into<String>) -> Self {
        Self::new(ComponentState::Disabled, detail)
    }

    /// Construct an unavailable component.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self::new(ComponentState::Unavailable, detail)
    }
}

/// File layers used to resolve the active configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfigSource {
    /// A caller-selected file replaced normal file discovery.
    Explicit {
        /// Resolved file path.
        path: PathBuf,
    },
    /// Global and/or project files discovered for the runtime workdir.
    Discovered {
        /// Applied file paths in merge order.
        files: Vec<PathBuf>,
    },
    /// No config file existed; built-in defaults plus environment were used.
    Defaults,
}

/// Stable warning identifiers suitable for machine-readable diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    /// A fake model executor is active by explicit policy.
    SimulatedExecutor,
    /// A fake chain client is active by explicit policy.
    SimulatedChain,
    /// Chain tools are unavailable because no RPC endpoint was configured.
    ChainUnavailable,
    /// A configured or discovered harness could not be composed.
    HarnessUnavailable,
    /// A legacy persisted agent representation was normalized at startup.
    LegacyAgentNormalized,
    /// Runtime read-only state is only enforced by API surface middleware.
    ReadOnlySurfaceOnly,
    /// A configured `SQLite` option is not honored by the current pool adapter.
    SqliteOptionIgnored,
    /// A configured memory backend cannot be composed by this runtime.
    MemoryUnavailable,
    /// Policy and grant composition is incomplete in a legacy runtime path.
    /// The shared runtime factory now fails startup instead of emitting this
    /// warning when explicitly enabled policy loading cannot be composed.
    PolicyCompositionIncomplete,
    /// Graceful shutdown of service background tasks is incomplete upstream.
    ShutdownIncomplete,
}

/// One safe, structured readiness warning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadinessWarning {
    /// Stable warning identifier.
    pub code: WarningCode,
    /// Human-readable remediation context with no secrets.
    pub message: String,
}

impl ReadinessWarning {
    /// Construct a new warning.
    pub fn new(code: WarningCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Snapshot of what a constructed runtime can truthfully provide.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeReadiness {
    /// Whether the durable core and an execution backend can accept work.
    pub operational: bool,
    /// Config file layers used before environment and caller overrides.
    pub config_source: ConfigSource,
    /// Resolved durable database file.
    pub database_path: PathBuf,
    /// Durable `SQLite` storage readiness.
    pub database: ComponentReadiness,
    /// Default model-executor readiness.
    pub executor: ComponentReadiness,
    /// External coding harness readiness.
    pub harness: ComponentReadiness,
    /// Chain client readiness.
    pub chain: ComponentReadiness,
    /// Governance and treasury tool registration readiness.
    pub tools: ComponentReadiness,
    /// Durable effect pipeline readiness.
    pub effects: ComponentReadiness,
    /// Conversation persistence readiness.
    pub conversations: ComponentReadiness,
    /// Payment persistence readiness.
    pub payments: ComponentReadiness,
    /// Agent memory persistence readiness.
    pub memory: ComponentReadiness,
    /// Signing adapter readiness.
    pub signer: ComponentReadiness,
    /// Skill loading and execution readiness.
    pub skills: ComponentReadiness,
    /// Policy and grant resolver readiness.
    pub policy_and_grants: ComponentReadiness,
    /// Whether read-only behavior was requested for compatible surfaces.
    pub read_only_requested: bool,
    /// Number of abandoned non-terminal runs failed during startup recovery.
    pub recovered_runs: usize,
    /// Number of active persisted agents registered in the service facade.
    pub rehydrated_agents: usize,
    /// Safe diagnostics for degraded or unsupported components.
    pub warnings: Vec<ReadinessWarning>,
}

impl RuntimeReadiness {
    /// Whether any operational component is running in degraded mode.
    pub fn is_degraded(&self) -> bool {
        [
            &self.database,
            &self.executor,
            &self.harness,
            &self.chain,
            &self.tools,
            &self.effects,
            &self.conversations,
            &self.payments,
            &self.memory,
            &self.signer,
            &self.skills,
            &self.policy_and_grants,
        ]
        .iter()
        .any(|component| component.state == ComponentState::Degraded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_operational_states_are_explicit() {
        assert!(ComponentState::Ready.is_operational());
        assert!(ComponentState::Degraded.is_operational());
        assert!(!ComponentState::Disabled.is_operational());
        assert!(!ComponentState::Unavailable.is_operational());
    }
}

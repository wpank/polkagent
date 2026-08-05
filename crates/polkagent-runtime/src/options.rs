//! Runtime construction options.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Controls whether missing concrete adapters are fatal or may be simulated.
///
/// Production and editor surfaces should normally use [`Self::Strict`].
/// Tests and explicitly requested offline demonstrations may use
/// [`Self::AllowSimulated`], which is always reflected as degraded readiness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterPolicy {
    /// Never silently replace a missing execution adapter with a fake.
    #[default]
    Strict,
    /// Permit deterministic fake adapters and report them as degraded.
    AllowSimulated,
}

/// Inputs used to build one process-wide [`crate::PolkagentRuntime`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOptions {
    /// Explicit configuration file. Relative paths are resolved under
    /// [`Self::workdir`]. When absent, global and workdir-local discovery is
    /// performed.
    pub config_path: Option<PathBuf>,
    /// Explicit `SQLite` file override. Relative paths are resolved under
    /// [`Self::workdir`]. This is primarily useful to embedders and tests.
    pub database_path: Option<PathBuf>,
    /// Workspace used for project config discovery, relative paths, and
    /// external harness sessions.
    pub workdir: PathBuf,
    /// Optional provider identifier selected ahead of configuration defaults.
    pub provider_override: Option<String>,
    /// Optional model identifier for the selected executor and rehydrated
    /// agent specifications.
    pub model_override: Option<String>,
    /// Optional harness identifier selected ahead of configuration defaults.
    pub harness_override: Option<String>,
    /// Disable external harness discovery and run through a model executor.
    pub disable_harness: bool,
    /// Request read-only API behavior. The current service facade does not
    /// enforce this on direct method calls; readiness reports that limitation.
    pub read_only: bool,
    /// Policy for missing concrete execution and chain adapters.
    pub adapter_policy: AdapterPolicy,
    /// Whether standard provider environment variables may synthesize
    /// providers. Disable this for deterministic embedded/test construction.
    pub discover_environment_providers: bool,
}

impl RuntimeOptions {
    /// Construct strict runtime options rooted at `workdir`.
    pub fn new(workdir: impl Into<PathBuf>) -> Self {
        Self {
            workdir: workdir.into(),
            ..Self::default()
        }
    }
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            config_path: None,
            database_path: None,
            workdir: PathBuf::from("."),
            provider_override: None,
            model_override: None,
            harness_override: None,
            disable_harness: false,
            read_only: false,
            adapter_policy: AdapterPolicy::Strict,
            discover_environment_providers: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fail_closed() {
        let options = RuntimeOptions::default();
        assert_eq!(options.adapter_policy, AdapterPolicy::Strict);
        assert!(options.discover_environment_providers);
        assert!(!options.read_only);
    }

    #[test]
    fn constructor_sets_workdir_without_weakening_policy() {
        let options = RuntimeOptions::new("/tmp/project");
        assert_eq!(options.workdir, PathBuf::from("/tmp/project"));
        assert_eq!(options.adapter_policy, AdapterPolicy::Strict);
    }
}

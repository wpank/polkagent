//! Runtime-only approval execution configuration.
//!
//! Surface authentication remains outside `polkagent-run`. The orchestrator
//! receives one already-authenticated, scope-bound authority and uses it only
//! to persist/read approval state; surfaces never receive direct store access.

use std::path::PathBuf;
use std::time::Duration;

use polkagent_config::SecurityConfig;
use polkagent_core::PrincipalId;

/// Exact authorization and security context attached to approval-capable
/// executor runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRuntimeConfig {
    /// Tenant authorization scope.
    pub tenant_id: String,
    /// Workspace authorization scope.
    pub workspace_id: String,
    /// One principal allowed to resolve human decisions in the APR-03 slice.
    pub authorized_principal_id: PrincipalId,
    /// Stable service identity used only for expiry/cancellation.
    pub service_principal_id: PrincipalId,
    /// Lexically stable workspace origin included in the authorization subject.
    pub working_directory: PathBuf,
    /// Exact security configuration revalidated immediately before tool I/O.
    pub security_config: SecurityConfig,
    /// Maximum time an approval may remain pending when the run has no earlier
    /// deadline.
    pub approval_timeout: Duration,
    /// Lease shared by the checkpoint and exact approved effect.
    pub recovery_lease: Duration,
    /// Store polling cadence. Polling is only a wake-up mechanism; every
    /// decision is re-read from the durable coordinator.
    pub poll_interval: Duration,
}

impl ApprovalRuntimeConfig {
    /// Validate the fail-closed fields required before grant-bearing tools can
    /// be advertised.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.tenant_id.trim().is_empty() {
            return Err("approval tenant scope is empty");
        }
        if self.workspace_id.trim().is_empty() {
            return Err("approval workspace scope is empty");
        }
        if !self.working_directory.is_absolute() {
            return Err("approval working directory is not absolute");
        }
        if self.approval_timeout.is_zero() {
            return Err("approval timeout is zero");
        }
        if self.recovery_lease.is_zero() {
            return Err("approval recovery lease is zero");
        }
        if self.poll_interval.is_zero() {
            return Err("approval poll interval is zero");
        }
        Ok(())
    }
}

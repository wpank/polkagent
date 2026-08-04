//! Pure-proxy signer adapter with budget enforcement.
//!
//! This crate implements the [`Signer`] trait for funded pure-proxy accounts
//! on Polkadot/Substrate networks. It wraps a delegate signer (the real key
//! holder) and interposes budget enforcement and dry-run validation before
//! forwarding signing requests.
//!
//! # Architecture
//!
//! ```text
//! CanonicalSignRequest
//!        │
//!        ▼
//! ┌──────────────────┐
//! │  ProxySigner      │
//! │  1. Budget check  │  ← BudgetTracker (ceiling enforcement)
//! │  2. Dry-run       │  ← ChainClient::dry_run_call (preflight)
//! │  3. Delegate sign │  ← inner Signer (real key holder)
//! └──────────────────┘
//! ```
//!
//! # Budget enforcement
//!
//! Before every signing request, the proxy checks the agent's remaining
//! budget via [`BudgetTracker`]. If the transfer amount would exceed the
//! configured ceiling, the request is refused with
//! [`ProxySignerError::BudgetExceeded`] before reaching the delegate signer.
//!
//! # DryRunApi pre-flight
//!
//! When a [`ChainClient`] is provided, the proxy performs a `dry_run_call`
//! against the extrinsic payload before delegating to the inner signer. This
//! catches dispatch errors (e.g. insufficient balance, bad destination)
//! before submitting on-chain.
//!
//! # Supported transfers
//!
//! - DOT transfers on relay chains
//! - Stablecoin (USDT/USDC) transfers on Asset Hub via `Assets` pallet

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tracing::{debug, warn};

use polkagent_chain_trait::ChainClient;
use polkagent_core::ids::{AgentId, RunId};
use polkagent_core::now;
use polkagent_grant::budget::{BudgetError, BudgetTracker};
use polkagent_signer_trait::{
    CanonicalSignRequest, ChainProfileId, SignedPayload, Signer, SignerCapabilities, SignerError,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors specific to the proxy signer layer.
#[derive(Debug, Error)]
pub enum ProxySignerError {
    #[error("budget exceeded: requested {requested}, remaining {remaining}")]
    BudgetExceeded { requested: u64, remaining: u64 },

    #[error("budget check failed: {0}")]
    BudgetCheck(#[from] BudgetError),

    #[error("dry-run failed: {message}")]
    DryRunFailed { message: String },

    #[error("delegate signer error: {0}")]
    Delegate(#[from] SignerError),
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a [`ProxySigner`].
pub struct ProxySignerConfig {
    /// The agent whose budget is being enforced.
    pub agent_id: AgentId,

    /// The current run for spend recording.
    pub run_id: RunId,

    /// Transfer amount to check against the budget ceiling (in the asset's
    /// smallest unit, e.g. planck for DOT).
    ///
    /// Set to `None` for non-transfer operations that don't consume budget.
    pub transfer_amount: Option<u64>,

    /// Whether to perform DryRunApi pre-flight validation.
    pub dry_run_enabled: bool,

    /// Chain profiles this proxy signer supports (e.g. Asset Hub, relay chain).
    pub chain_profiles: Vec<ChainProfileId>,
}

// ---------------------------------------------------------------------------
// ProxySigner
// ---------------------------------------------------------------------------

/// A signer that interposes budget enforcement and dry-run validation before
/// delegating to an inner signer.
///
/// Use [`ProxySigner::new`] for the simplest construction. Attach a
/// [`ChainClient`] via [`with_chain_client`] for DryRunApi pre-flight.
///
/// [`with_chain_client`]: ProxySigner::with_chain_client
pub struct ProxySigner {
    inner: Box<dyn Signer>,
    budget_tracker: Arc<BudgetTracker>,
    chain_client: Option<Arc<dyn ChainClient>>,
    config: ProxySignerConfig,
}

impl ProxySigner {
    /// Create a new proxy signer wrapping `inner` with budget enforcement.
    ///
    /// The `budget_tracker` must have been pre-configured with a ceiling for
    /// `config.agent_id` before the first signing request.
    #[must_use]
    pub fn new(
        inner: Box<dyn Signer>,
        budget_tracker: Arc<BudgetTracker>,
        config: ProxySignerConfig,
    ) -> Self {
        Self {
            inner,
            budget_tracker,
            chain_client: None,
            config,
        }
    }

    /// Attach a chain client for DryRunApi pre-flight validation.
    #[must_use]
    pub fn with_chain_client(mut self, client: Arc<dyn ChainClient>) -> Self {
        self.chain_client = Some(client);
        self
    }

    /// Update the transfer amount for the next signing request.
    pub fn set_transfer_amount(&mut self, amount: Option<u64>) {
        self.config.transfer_amount = amount;
    }

    /// Check the budget ceiling before signing.
    ///
    /// Returns `Ok(())` if the transfer amount is within the remaining budget,
    /// or if no transfer amount is set.
    async fn check_budget(&self) -> Result<(), ProxySignerError> {
        let amount = match self.config.transfer_amount {
            Some(a) => a,
            None => return Ok(()),
        };

        let within_budget = self
            .budget_tracker
            .check_budget(self.config.agent_id, amount)
            .await?;

        if !within_budget {
            let status = self
                .budget_tracker
                .get_remaining(self.config.agent_id)
                .await?;

            warn!(
                agent_id = %self.config.agent_id,
                requested = amount,
                remaining = status.remaining,
                "proxy signer: budget ceiling exceeded"
            );

            return Err(ProxySignerError::BudgetExceeded {
                requested: amount,
                remaining: status.remaining,
            });
        }

        Ok(())
    }

    /// Record the spend after a successful sign+submit.
    async fn record_spend(&self) -> Result<(), ProxySignerError> {
        if let Some(amount) = self.config.transfer_amount {
            self.budget_tracker
                .record_spend(self.config.agent_id, self.config.run_id, amount)
                .await?;
            debug!(
                agent_id = %self.config.agent_id,
                run_id = %self.config.run_id,
                amount,
                "proxy signer: spend recorded"
            );
        }
        Ok(())
    }

    /// Run DryRunApi pre-flight validation if a chain client is attached.
    async fn preflight_dry_run(&self, payload: &[u8]) -> Result<(), ProxySignerError> {
        if !self.config.dry_run_enabled {
            return Ok(());
        }

        let client = match &self.chain_client {
            Some(c) => c,
            None => return Ok(()),
        };

        let result = client.dry_run_call(payload).await.map_err(|e| {
            ProxySignerError::DryRunFailed {
                message: format!("chain client dry-run error: {e}"),
            }
        })?;

        if !result.execution_ok {
            return Err(ProxySignerError::DryRunFailed {
                message: "dry-run execution failed: extrinsic would not succeed on-chain"
                    .to_string(),
            });
        }

        debug!("proxy signer: dry-run passed");
        Ok(())
    }
}

#[async_trait]
impl Signer for ProxySigner {
    async fn describe(&self) -> Result<SignerCapabilities, SignerError> {
        let mut caps = self.inner.describe().await?;
        caps.display_name = format!("ProxySigner({})", caps.display_name);
        if !self.config.chain_profiles.is_empty() {
            caps.chain_profiles = self.config.chain_profiles.clone();
        }
        Ok(caps)
    }

    async fn sign(&self, request: CanonicalSignRequest) -> Result<SignedPayload, SignerError> {
        // 1. Check expiry before doing anything else.
        if request.expires_at <= now() {
            return Err(SignerError::Expired {
                expired_at: request.expires_at,
            });
        }

        // 2. Budget ceiling check.
        self.check_budget().await.map_err(|e| SignerError::Internal {
            message: format!("budget gate: {e}"),
        })?;

        // 3. DryRunApi pre-flight validation.
        self.preflight_dry_run(&request.payload)
            .await
            .map_err(|e| SignerError::Internal {
                message: format!("dry-run preflight: {e}"),
            })?;

        // 4. Delegate to the inner signer.
        let signed = self.inner.sign(request).await?;

        // 5. Record spend on successful sign.
        self.record_spend()
            .await
            .map_err(|e| SignerError::Internal {
                message: format!("spend recording: {e}"),
            })?;

        Ok(signed)
    }

    async fn health(&self) -> Result<(), SignerError> {
        // Check delegate signer health.
        self.inner.health().await?;

        // Check chain client health if attached.
        if let Some(client) = &self.chain_client {
            client.health().await.map_err(|e| SignerError::Internal {
                message: format!("chain client unhealthy: {e}"),
            })?;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::ids::{AgentId, RunId};
    use polkagent_signer_trait::{
        AccountRef, ApprovalId, CanonicalSignRequest, ChainProfileId, GrantDigest, MetadataDigest,
    };

    fn test_account() -> AccountRef {
        AccountRef::from_bytes([0u8; 32])
    }

    fn valid_request(account: AccountRef) -> CanonicalSignRequest {
        CanonicalSignRequest {
            request_id: "proxy-test-req-001".into(),
            payload: vec![0xCA, 0xFE, 0xBA, 0xBE, 0x01, 0x02, 0x03, 0x04],
            account,
            chain_profile: ChainProfileId::new("polkadot"),
            metadata_hash: MetadataDigest(vec![0xAB; 32]),
            grant_digest: GrantDigest(vec![0xCD; 32]),
            approval_id: ApprovalId::new("proxy-test-approval"),
            expires_at: now() + chrono::Duration::hours(1),
        }
    }

    fn make_proxy(
        agent_id: AgentId,
        run_id: RunId,
        transfer_amount: Option<u64>,
        tracker: Arc<BudgetTracker>,
    ) -> ProxySigner {
        use polkagent_signer_fake::FakeSigner;
        let config = ProxySignerConfig {
            agent_id,
            run_id,
            transfer_amount,
            dry_run_enabled: false,
            chain_profiles: vec![ChainProfileId::new("polkadot")],
        };
        ProxySigner::new(Box::new(FakeSigner::new()), tracker, config)
    }

    // ---- Signer trait basic tests -------------------------------------------

    #[tokio::test]
    async fn describe_reports_proxy_display_name() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 1_000_000).await;

        let proxy = make_proxy(agent, run, None, tracker);
        let caps = proxy.describe().await.expect("describe ok");

        assert!(
            caps.display_name.contains("ProxySigner"),
            "display_name should contain ProxySigner, got: {}",
            caps.display_name
        );
        assert!(!caps.accounts.is_empty());
        assert!(caps.can_sign);
    }

    #[tokio::test]
    async fn sign_succeeds_within_budget() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 10_000).await;

        let proxy = make_proxy(agent, run, Some(5_000), tracker.clone());
        let account = test_account();
        let req = valid_request(account);

        let signed = proxy.sign(req).await.expect("sign should succeed");
        assert!(!signed.signature.is_empty());
        assert!(!signed.signed_extrinsic.is_empty());

        // Verify spend was recorded.
        let status = tracker.get_remaining(agent).await.expect("status ok");
        assert_eq!(status.spent, 5_000);
        assert_eq!(status.remaining, 5_000);
    }

    #[tokio::test]
    async fn sign_without_transfer_amount_skips_budget() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 100).await;

        let proxy = make_proxy(agent, run, None, tracker.clone());
        let account = test_account();
        let req = valid_request(account);

        let signed = proxy.sign(req).await.expect("sign should succeed");
        assert!(!signed.signature.is_empty());

        // No spend should be recorded.
        let status = tracker.get_remaining(agent).await.expect("status ok");
        assert_eq!(status.spent, 0);
    }

    // ---- Budget enforcement tests -------------------------------------------

    #[tokio::test]
    async fn sign_refused_when_budget_exceeded() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 100).await;

        // Request 200 against a 100 ceiling.
        let proxy = make_proxy(agent, run, Some(200), tracker);
        let account = test_account();
        let req = valid_request(account);

        let err = proxy.sign(req).await.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("budget"),
            "error should mention budget, got: {msg}"
        );
    }

    #[tokio::test]
    async fn cumulative_spend_hits_ceiling() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 100).await;

        // First request: spend 60 of 100.
        let proxy = make_proxy(agent, run, Some(60), tracker.clone());
        let req = valid_request(test_account());
        proxy.sign(req).await.expect("first sign ok");

        // Second request: spend 50 more → exceeds the 100 ceiling.
        let proxy2 = make_proxy(agent, run, Some(50), tracker.clone());
        let req2 = valid_request(test_account());
        let err = proxy2.sign(req2).await.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("budget"),
            "error should mention budget, got: {msg}"
        );

        // Total spend should be 60 (the failed request was NOT recorded).
        let status = tracker.get_remaining(agent).await.expect("status ok");
        assert_eq!(status.spent, 60);
        assert_eq!(status.remaining, 40);
    }

    #[tokio::test]
    async fn exact_ceiling_is_allowed() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 500).await;

        let proxy = make_proxy(agent, run, Some(500), tracker.clone());
        let req = valid_request(test_account());
        proxy.sign(req).await.expect("exact ceiling should succeed");

        let status = tracker.get_remaining(agent).await.expect("status ok");
        assert_eq!(status.spent, 500);
        assert_eq!(status.remaining, 0);
    }

    #[tokio::test]
    async fn one_over_ceiling_is_denied() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 500).await;

        let proxy = make_proxy(agent, run, Some(501), tracker);
        let req = valid_request(test_account());
        let err = proxy.sign(req).await.unwrap_err();
        assert!(format!("{err}").contains("budget"));
    }

    // ---- Expiry tests -------------------------------------------------------

    #[tokio::test]
    async fn expired_request_is_rejected() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 100_000).await;

        let proxy = make_proxy(agent, run, None, tracker);
        let account = test_account();
        let mut req = valid_request(account);
        req.expires_at = now() - chrono::Duration::seconds(1);

        let err = proxy.sign(req).await.unwrap_err();
        assert!(matches!(err, SignerError::Expired { .. }));
    }

    // ---- Health check -------------------------------------------------------

    #[tokio::test]
    async fn health_delegates_to_inner() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 100).await;

        let proxy = make_proxy(agent, run, None, tracker);
        proxy.health().await.expect("health should be ok");
    }

    // ---- Config tests -------------------------------------------------------

    #[tokio::test]
    async fn set_transfer_amount_updates_budget_check() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 100).await;

        let mut proxy = make_proxy(agent, run, None, tracker.clone());

        // First sign with no amount — succeeds.
        let req = valid_request(test_account());
        proxy.sign(req).await.expect("no-amount sign ok");

        // Set amount above ceiling.
        proxy.set_transfer_amount(Some(200));
        let req2 = valid_request(test_account());
        let err = proxy.sign(req2).await.unwrap_err();
        assert!(format!("{err}").contains("budget"));
    }

    #[tokio::test]
    async fn chain_profiles_override_inner() {
        let tracker = BudgetTracker::new();
        let agent = AgentId::new();
        let run = RunId::new();
        tracker.configure(agent, 100).await;

        let config = ProxySignerConfig {
            agent_id: agent,
            run_id: run,
            transfer_amount: None,
            dry_run_enabled: false,
            chain_profiles: vec![
                ChainProfileId::new("asset-hub-polkadot"),
                ChainProfileId::new("polkadot"),
            ],
        };

        use polkagent_signer_fake::FakeSigner;
        let proxy = ProxySigner::new(Box::new(FakeSigner::new()), tracker, config);
        let caps = proxy.describe().await.expect("describe ok");

        assert_eq!(caps.chain_profiles.len(), 2);
        assert_eq!(caps.chain_profiles[0].0, "asset-hub-polkadot");
        assert_eq!(caps.chain_profiles[1].0, "polkadot");
    }
}

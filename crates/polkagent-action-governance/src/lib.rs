#![forbid(unsafe_code)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use std::collections::BTreeSet;

use async_trait::async_trait;
use polkagent_chain_trait::{ChainClient, ChainError, ChainProfileId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum GovernanceError {
    #[error("chain error: {0}")]
    Chain(#[from] ChainError),

    #[error("multisig not found for account {account}")]
    MultisigNotFound { account: String },

    #[error("duplicate approval from {signer}")]
    DuplicateApproval { signer: String },

    #[error("unknown signer {signer} — not a member of the multisig")]
    UnknownSigner { signer: String },
}

// ---------------------------------------------------------------------------
// MultisigState
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultisigState {
    pub multisig_account: String,
    pub signatories: Vec<String>,
    pub threshold: u16,
    pub approvals: BTreeSet<String>,
}

impl MultisigState {
    pub fn new(
        multisig_account: impl Into<String>,
        signatories: Vec<String>,
        threshold: u16,
    ) -> Self {
        Self {
            multisig_account: multisig_account.into(),
            signatories,
            threshold,
            approvals: BTreeSet::new(),
        }
    }

    pub fn record_approval(&mut self, signer: &str) -> Result<(), GovernanceError> {
        if !self.signatories.iter().any(|s| s == signer) {
            return Err(GovernanceError::UnknownSigner {
                signer: signer.to_string(),
            });
        }
        if self.approvals.contains(signer) {
            return Err(GovernanceError::DuplicateApproval {
                signer: signer.to_string(),
            });
        }
        self.approvals.insert(signer.to_string());
        Ok(())
    }

    pub fn is_threshold_met(&self) -> bool {
        self.approvals.len() >= usize::from(self.threshold)
    }

    pub fn missing_signers(&self) -> Vec<&str> {
        self.signatories
            .iter()
            .filter(|s| !self.approvals.contains(s.as_str()))
            .map(String::as_str)
            .collect()
    }

    pub fn summary(&self) -> MultisigSummary {
        MultisigSummary {
            multisig_account: self.multisig_account.clone(),
            threshold: self.threshold,
            total_signatories: self.signatories.len() as u16,
            current_approvals: self.approvals.len() as u16,
            approved_by: self.approvals.iter().cloned().collect(),
            missing: self.missing_signers().into_iter().map(String::from).collect(),
            threshold_met: self.is_threshold_met(),
        }
    }
}

// ---------------------------------------------------------------------------
// MultisigSummary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultisigSummary {
    pub multisig_account: String,
    pub threshold: u16,
    pub total_signatories: u16,
    pub current_approvals: u16,
    pub approved_by: Vec<String>,
    pub missing: Vec<String>,
    pub threshold_met: bool,
}

// ---------------------------------------------------------------------------
// ProxyConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub pure_proxy: String,
    pub controllers: Vec<String>,
    pub agent_proxy: Option<String>,
    pub proxy_filter: Option<String>,
    pub announcement_delay: u32,
}

impl ProxyConfig {
    pub fn new(pure_proxy: impl Into<String>, controllers: Vec<String>) -> Self {
        Self {
            pure_proxy: pure_proxy.into(),
            controllers,
            agent_proxy: None,
            proxy_filter: None,
            announcement_delay: 0,
        }
    }

    pub fn with_agent_proxy(mut self, proxy: impl Into<String>, filter: impl Into<String>) -> Self {
        self.agent_proxy = Some(proxy.into());
        self.proxy_filter = Some(filter.into());
        self
    }

    pub fn with_announcement_delay(mut self, blocks: u32) -> Self {
        self.announcement_delay = blocks;
        self
    }
}

// ---------------------------------------------------------------------------
// GovernanceReader trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait GovernanceReader: Send + Sync {
    async fn fetch_multisig_state(
        &self,
        chain_profile: ChainProfileId,
        multisig_account: &str,
    ) -> Result<MultisigState, GovernanceError>;

    async fn fetch_proxy_config(
        &self,
        chain_profile: ChainProfileId,
        pure_proxy: &str,
    ) -> Result<ProxyConfig, GovernanceError>;
}

// ---------------------------------------------------------------------------
// StorageKeyGovernanceReader
// ---------------------------------------------------------------------------

pub struct StorageKeyGovernanceReader<C> {
    chain_client: C,
}

impl<C: ChainClient> StorageKeyGovernanceReader<C> {
    pub fn new(chain_client: C) -> Self {
        Self { chain_client }
    }
}

#[async_trait]
impl<C: ChainClient> GovernanceReader for StorageKeyGovernanceReader<C> {
    async fn fetch_multisig_state(
        &self,
        chain_profile: ChainProfileId,
        multisig_account: &str,
    ) -> Result<MultisigState, GovernanceError> {
        let storage_key = multisig_storage_key(multisig_account);
        let raw = self
            .chain_client
            .query_storage(&storage_key, None, chain_profile)
            .await?;

        match raw {
            Some(bytes) => decode_multisig_state(multisig_account, &bytes),
            None => Err(GovernanceError::MultisigNotFound {
                account: multisig_account.to_string(),
            }),
        }
    }

    async fn fetch_proxy_config(
        &self,
        chain_profile: ChainProfileId,
        pure_proxy: &str,
    ) -> Result<ProxyConfig, GovernanceError> {
        let storage_key = proxy_storage_key(pure_proxy);
        let raw = self
            .chain_client
            .query_storage(&storage_key, None, chain_profile)
            .await?;

        match raw {
            Some(bytes) => decode_proxy_config(pure_proxy, &bytes),
            None => Ok(ProxyConfig::new(pure_proxy, vec![])),
        }
    }
}

// ---------------------------------------------------------------------------
// Storage key helpers
// ---------------------------------------------------------------------------

fn multisig_storage_key(account: &str) -> Vec<u8> {
    let mut key = b"Multisig:Multisigs:".to_vec();
    key.extend_from_slice(account.as_bytes());
    key
}

fn proxy_storage_key(account: &str) -> Vec<u8> {
    let mut key = b"Proxy:Proxies:".to_vec();
    key.extend_from_slice(account.as_bytes());
    key
}

// ---------------------------------------------------------------------------
// Decoders (JSON-encoded storage for testability; real chain would use SCALE)
// ---------------------------------------------------------------------------

fn decode_multisig_state(account: &str, bytes: &[u8]) -> Result<MultisigState, GovernanceError> {
    #[derive(Deserialize)]
    struct RawMultisig {
        signatories: Vec<String>,
        threshold: u16,
    }

    let raw: RawMultisig = serde_json::from_slice(bytes).map_err(|e| ChainError::DecodeFailed {
        message: format!("multisig decode: {e}"),
    })?;

    Ok(MultisigState::new(account, raw.signatories, raw.threshold))
}

fn decode_proxy_config(account: &str, bytes: &[u8]) -> Result<ProxyConfig, GovernanceError> {
    #[derive(Deserialize)]
    struct RawProxy {
        controllers: Vec<String>,
        #[serde(default)]
        agent_proxy: Option<String>,
        #[serde(default)]
        proxy_filter: Option<String>,
        #[serde(default)]
        announcement_delay: u32,
    }

    let raw: RawProxy = serde_json::from_slice(bytes).map_err(|e| ChainError::DecodeFailed {
        message: format!("proxy decode: {e}"),
    })?;

    let mut config = ProxyConfig::new(account, raw.controllers);
    config.announcement_delay = raw.announcement_delay;
    if let (Some(ap), Some(pf)) = (raw.agent_proxy, raw.proxy_filter) {
        config = config.with_agent_proxy(ap, pf);
    }
    Ok(config)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_chain_fake::FakeChainClientBuilder;

    // -----------------------------------------------------------------------
    // MultisigState unit tests
    // -----------------------------------------------------------------------

    fn sample_multisig() -> MultisigState {
        MultisigState::new(
            "5Multi",
            vec!["Alice".into(), "Bob".into(), "Charlie".into()],
            2,
        )
    }

    #[test]
    fn new_multisig_has_no_approvals() {
        let ms = sample_multisig();
        assert_eq!(ms.approvals.len(), 0);
        assert!(!ms.is_threshold_met());
    }

    #[test]
    fn record_approval_adds_signer() {
        let mut ms = sample_multisig();
        ms.record_approval("Alice").expect("ok");
        assert_eq!(ms.approvals.len(), 1);
        assert!(ms.approvals.contains("Alice"));
    }

    #[test]
    fn threshold_met_after_enough_approvals() {
        let mut ms = sample_multisig();
        ms.record_approval("Alice").expect("ok");
        assert!(!ms.is_threshold_met());
        ms.record_approval("Bob").expect("ok");
        assert!(ms.is_threshold_met());
    }

    #[test]
    fn duplicate_approval_rejected() {
        let mut ms = sample_multisig();
        ms.record_approval("Alice").expect("ok");
        let err = ms.record_approval("Alice").unwrap_err();
        assert!(matches!(err, GovernanceError::DuplicateApproval { .. }));
    }

    #[test]
    fn unknown_signer_rejected() {
        let mut ms = sample_multisig();
        let err = ms.record_approval("Eve").unwrap_err();
        assert!(matches!(err, GovernanceError::UnknownSigner { .. }));
    }

    #[test]
    fn missing_signers_tracks_who_has_not_approved() {
        let mut ms = sample_multisig();
        ms.record_approval("Bob").expect("ok");
        let missing = ms.missing_signers();
        assert_eq!(missing, vec!["Alice", "Charlie"]);
    }

    #[test]
    fn summary_reflects_state() {
        let mut ms = sample_multisig();
        ms.record_approval("Alice").expect("ok");
        let s = ms.summary();
        assert_eq!(s.threshold, 2);
        assert_eq!(s.total_signatories, 3);
        assert_eq!(s.current_approvals, 1);
        assert_eq!(s.approved_by, vec!["Alice"]);
        assert_eq!(s.missing, vec!["Bob", "Charlie"]);
        assert!(!s.threshold_met);
    }

    #[test]
    fn summary_serde_roundtrip() {
        let mut ms = sample_multisig();
        ms.record_approval("Alice").expect("ok");
        let s = ms.summary();
        let json = serde_json::to_string(&s).expect("serialize");
        let back: MultisigSummary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
    }

    // -----------------------------------------------------------------------
    // ProxyConfig unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn proxy_config_builder() {
        let pc = ProxyConfig::new("5Pure", vec!["Controller1".into()])
            .with_agent_proxy("5Agent", "NonTransfer")
            .with_announcement_delay(100);
        assert_eq!(pc.pure_proxy, "5Pure");
        assert_eq!(pc.agent_proxy.as_deref(), Some("5Agent"));
        assert_eq!(pc.proxy_filter.as_deref(), Some("NonTransfer"));
        assert_eq!(pc.announcement_delay, 100);
    }

    #[test]
    fn proxy_config_serde_roundtrip() {
        let pc = ProxyConfig::new("5Pure", vec!["C1".into(), "C2".into()])
            .with_agent_proxy("5Ag", "Staking")
            .with_announcement_delay(50);
        let json = serde_json::to_string(&pc).expect("serialize");
        let back: ProxyConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(pc, back);
    }

    // -----------------------------------------------------------------------
    // StorageKeyGovernanceReader with FakeChainClient
    // -----------------------------------------------------------------------

    fn seed_multisig_storage(
        builder: FakeChainClientBuilder,
        account: &str,
        signatories: &[&str],
        threshold: u16,
    ) -> FakeChainClientBuilder {
        let key = multisig_storage_key(account);
        let value = serde_json::json!({
            "signatories": signatories,
            "threshold": threshold,
        });
        builder.with_storage_value(key, serde_json::to_vec(&value).expect("encode"))
    }

    fn seed_proxy_storage(
        builder: FakeChainClientBuilder,
        account: &str,
        controllers: &[&str],
        agent_proxy: Option<&str>,
        proxy_filter: Option<&str>,
        delay: u32,
    ) -> FakeChainClientBuilder {
        let key = proxy_storage_key(account);
        let value = serde_json::json!({
            "controllers": controllers,
            "agent_proxy": agent_proxy,
            "proxy_filter": proxy_filter,
            "announcement_delay": delay,
        });
        builder.with_storage_value(key, serde_json::to_vec(&value).expect("encode"))
    }

    #[tokio::test]
    async fn fetch_multisig_state_from_fake_chain() {
        let builder = FakeChainClientBuilder::new("polkadot");
        let builder = seed_multisig_storage(builder, "5Multi", &["Alice", "Bob", "Charlie"], 2);
        let client = builder.build();

        let reader = StorageKeyGovernanceReader::new(client);
        let ms = reader
            .fetch_multisig_state(ChainProfileId::new("polkadot"), "5Multi")
            .await
            .expect("ok");

        assert_eq!(ms.threshold, 2);
        assert_eq!(ms.signatories.len(), 3);
        assert!(ms.approvals.is_empty());
    }

    #[tokio::test]
    async fn fetch_multisig_not_found() {
        let client = FakeChainClientBuilder::new("polkadot").build();
        let reader = StorageKeyGovernanceReader::new(client);
        let err = reader
            .fetch_multisig_state(ChainProfileId::new("polkadot"), "5Missing")
            .await
            .unwrap_err();

        assert!(matches!(err, GovernanceError::MultisigNotFound { .. }));
    }

    #[tokio::test]
    async fn fetch_proxy_config_from_fake_chain() {
        let builder = FakeChainClientBuilder::new("polkadot");
        let builder = seed_proxy_storage(
            builder,
            "5Pure",
            &["Controller1"],
            Some("5Agent"),
            Some("NonTransfer"),
            100,
        );
        let client = builder.build();

        let reader = StorageKeyGovernanceReader::new(client);
        let pc = reader
            .fetch_proxy_config(ChainProfileId::new("polkadot"), "5Pure")
            .await
            .expect("ok");

        assert_eq!(pc.pure_proxy, "5Pure");
        assert_eq!(pc.controllers, vec!["Controller1"]);
        assert_eq!(pc.agent_proxy.as_deref(), Some("5Agent"));
        assert_eq!(pc.proxy_filter.as_deref(), Some("NonTransfer"));
        assert_eq!(pc.announcement_delay, 100);
    }

    #[tokio::test]
    async fn fetch_proxy_config_missing_returns_empty() {
        let client = FakeChainClientBuilder::new("polkadot").build();
        let reader = StorageKeyGovernanceReader::new(client);
        let pc = reader
            .fetch_proxy_config(ChainProfileId::new("polkadot"), "5None")
            .await
            .expect("ok");

        assert_eq!(pc.pure_proxy, "5None");
        assert!(pc.controllers.is_empty());
        assert!(pc.agent_proxy.is_none());
    }

    // -----------------------------------------------------------------------
    // End-to-end: fetch → approve → summarise
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn end_to_end_fetch_approve_summarise() {
        let builder = FakeChainClientBuilder::new("polkadot");
        let builder = seed_multisig_storage(builder, "5Multi", &["Alice", "Bob", "Charlie"], 2);
        let client = builder.build();

        let reader = StorageKeyGovernanceReader::new(client);
        let mut ms = reader
            .fetch_multisig_state(ChainProfileId::new("polkadot"), "5Multi")
            .await
            .expect("ok");

        ms.record_approval("Alice").expect("ok");
        assert!(!ms.is_threshold_met());

        ms.record_approval("Charlie").expect("ok");
        assert!(ms.is_threshold_met());

        let summary = ms.summary();
        assert!(summary.threshold_met);
        assert_eq!(summary.current_approvals, 2);
        assert_eq!(summary.missing, vec!["Bob"]);
    }
}

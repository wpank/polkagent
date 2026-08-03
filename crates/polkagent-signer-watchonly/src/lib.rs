//! Watch-only [`Signer`] adapter for the Polkagent platform.
//!
//! This crate provides [`WatchOnlySigner`] -- a signer that can enumerate
//! accounts it watches but **always refuses to produce signatures**.  It
//! is useful as a safe default signer in read-only or monitoring
//! configurations where transaction signing must never occur.
//!
//! # Behaviour
//!
//! | Method | Behaviour |
//! |---|---|
//! | [`describe`] | Reports configured accounts with `can_sign: false`. |
//! | [`sign`] | Always returns [`SignerError::WatchOnly`]. |
//! | [`health`] | Always returns `Ok(())`. |
//!
//! [`describe`]: Signer::describe
//! [`sign`]: Signer::sign
//! [`health`]: Signer::health

use async_trait::async_trait;
use polkagent_signer_trait::{
    AccountRef, CanonicalSignRequest, ChainProfileId, SignedPayload, Signer, SignerCapabilities,
    SignerError,
};

// ---------------------------------------------------------------------------
// WatchOnlySigner
// ---------------------------------------------------------------------------

/// A signer that watches accounts but never signs.
///
/// `WatchOnlySigner` reports a configured set of accounts and chain profiles
/// via [`describe`], but every call to [`sign`] returns
/// [`SignerError::WatchOnly`].  The [`health`] check always succeeds.
///
/// This is the recommended default signer for deployments that should never
/// produce on-chain transactions (e.g., monitoring dashboards, dry-run
/// environments, read-only API nodes).
///
/// [`describe`]: Signer::describe
/// [`sign`]: Signer::sign
/// [`health`]: Signer::health
pub struct WatchOnlySigner {
    accounts: Vec<AccountRef>,
    chain_profiles: Vec<ChainProfileId>,
}

impl WatchOnlySigner {
    /// Create a `WatchOnlySigner` that watches the given accounts.
    #[must_use]
    pub fn new(accounts: Vec<AccountRef>) -> Self {
        Self {
            accounts,
            chain_profiles: vec![],
        }
    }

    /// Create a `WatchOnlySigner` with both accounts and chain profiles.
    #[must_use]
    pub fn with_chain_profiles(
        accounts: Vec<AccountRef>,
        chain_profiles: Vec<ChainProfileId>,
    ) -> Self {
        Self {
            accounts,
            chain_profiles,
        }
    }
}

#[async_trait]
impl Signer for WatchOnlySigner {
    async fn describe(&self) -> Result<SignerCapabilities, SignerError> {
        Ok(SignerCapabilities {
            accounts: self.accounts.clone(),
            chain_profiles: self.chain_profiles.clone(),
            hardware_backed: false,
            display_name: "WatchOnlySigner".into(),
            can_sign: false,
        })
    }

    async fn sign(&self, _request: CanonicalSignRequest) -> Result<SignedPayload, SignerError> {
        Err(SignerError::WatchOnly)
    }

    async fn health(&self) -> Result<(), SignerError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::now;
    use polkagent_signer_trait::{ApprovalId, GrantDigest, MetadataDigest};

    fn valid_request(account: AccountRef) -> CanonicalSignRequest {
        CanonicalSignRequest {
            request_id: "req-watchonly-001".into(),
            payload: vec![1, 2, 3, 4, 5],
            account,
            chain_profile: ChainProfileId::new("polkadot"),
            metadata_hash: MetadataDigest(vec![0xab; 32]),
            grant_digest: GrantDigest(vec![0xcd; 32]),
            approval_id: ApprovalId::new("approval-watchonly-001"),
            expires_at: now() + chrono::Duration::hours(1),
        }
    }

    fn test_accounts() -> Vec<AccountRef> {
        vec![
            AccountRef::from_bytes([0xAA; 32]),
            AccountRef::from_bytes([0xBB; 32]),
        ]
    }

    // -----------------------------------------------------------------------
    // Constructor tests
    // -----------------------------------------------------------------------

    #[test]
    fn new_stores_accounts() {
        let signer = WatchOnlySigner::new(test_accounts());
        assert_eq!(signer.accounts.len(), 2);
        assert!(signer.chain_profiles.is_empty());
    }

    #[test]
    fn with_chain_profiles_stores_both() {
        let profiles = vec![
            ChainProfileId::new("polkadot"),
            ChainProfileId::new("kusama"),
        ];
        let signer = WatchOnlySigner::with_chain_profiles(test_accounts(), profiles);
        assert_eq!(signer.accounts.len(), 2);
        assert_eq!(signer.chain_profiles.len(), 2);
    }

    #[test]
    fn new_with_empty_accounts() {
        let signer = WatchOnlySigner::new(vec![]);
        assert!(signer.accounts.is_empty());
    }

    // -----------------------------------------------------------------------
    // describe() tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn describe_returns_configured_accounts() {
        let signer = WatchOnlySigner::new(test_accounts());
        let caps = signer.describe().await.expect("describe should succeed");
        assert_eq!(caps.accounts.len(), 2);
        assert_eq!(caps.accounts[0].account_id, [0xAA; 32]);
        assert_eq!(caps.accounts[1].account_id, [0xBB; 32]);
    }

    #[tokio::test]
    async fn describe_reports_can_sign_false() {
        let signer = WatchOnlySigner::new(test_accounts());
        let caps = signer.describe().await.expect("describe should succeed");
        assert!(!caps.can_sign);
    }

    #[tokio::test]
    async fn describe_reports_not_hardware_backed() {
        let signer = WatchOnlySigner::new(test_accounts());
        let caps = signer.describe().await.expect("describe should succeed");
        assert!(!caps.hardware_backed);
    }

    #[tokio::test]
    async fn describe_returns_display_name() {
        let signer = WatchOnlySigner::new(test_accounts());
        let caps = signer.describe().await.expect("describe should succeed");
        assert_eq!(caps.display_name, "WatchOnlySigner");
    }

    #[tokio::test]
    async fn describe_returns_chain_profiles() {
        let profiles = vec![ChainProfileId::new("polkadot")];
        let signer = WatchOnlySigner::with_chain_profiles(test_accounts(), profiles);
        let caps = signer.describe().await.expect("describe should succeed");
        assert_eq!(caps.chain_profiles.len(), 1);
        assert_eq!(caps.chain_profiles[0].0, "polkadot");
    }

    #[tokio::test]
    async fn describe_with_empty_accounts_returns_empty() {
        let signer = WatchOnlySigner::new(vec![]);
        let caps = signer.describe().await.expect("describe should succeed");
        assert!(caps.accounts.is_empty());
        assert!(!caps.can_sign);
    }

    // -----------------------------------------------------------------------
    // sign() tests -- always refuses
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn sign_returns_watch_only_error() {
        let signer = WatchOnlySigner::new(test_accounts());
        let account = AccountRef::from_bytes([0xAA; 32]);
        let result = signer.sign(valid_request(account)).await;
        assert!(matches!(result, Err(SignerError::WatchOnly)));
    }

    #[tokio::test]
    async fn sign_refuses_even_for_known_account() {
        let accounts = vec![AccountRef::from_bytes([0xCC; 32])];
        let signer = WatchOnlySigner::new(accounts);
        let account = AccountRef::from_bytes([0xCC; 32]);
        let result = signer.sign(valid_request(account)).await;
        assert!(matches!(result, Err(SignerError::WatchOnly)));
    }

    #[tokio::test]
    async fn sign_refuses_for_unknown_account() {
        let signer = WatchOnlySigner::new(test_accounts());
        let unknown = AccountRef::from_bytes([0xFF; 32]);
        let result = signer.sign(valid_request(unknown)).await;
        assert!(matches!(result, Err(SignerError::WatchOnly)));
    }

    #[tokio::test]
    async fn sign_refuses_with_expired_request() {
        let signer = WatchOnlySigner::new(test_accounts());
        let account = AccountRef::from_bytes([0xAA; 32]);
        let mut req = valid_request(account);
        req.expires_at = now() - chrono::Duration::seconds(10);
        let result = signer.sign(req).await;
        assert!(matches!(result, Err(SignerError::WatchOnly)));
    }

    #[tokio::test]
    async fn sign_error_message_contains_watch_only() {
        let signer = WatchOnlySigner::new(test_accounts());
        let account = AccountRef::from_bytes([0xAA; 32]);
        let result = signer.sign(valid_request(account)).await;
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("watch-only"));
    }

    // -----------------------------------------------------------------------
    // health() tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn health_always_returns_ok() {
        let signer = WatchOnlySigner::new(test_accounts());
        assert!(signer.health().await.is_ok());
    }

    #[tokio::test]
    async fn health_ok_even_with_empty_accounts() {
        let signer = WatchOnlySigner::new(vec![]);
        assert!(signer.health().await.is_ok());
    }

    // -----------------------------------------------------------------------
    // Trait object safety
    // -----------------------------------------------------------------------

    #[allow(dead_code)]
    fn _watch_only_is_object_safe(_s: &dyn Signer) {}

    #[test]
    fn can_construct_as_dyn_signer() {
        let signer = WatchOnlySigner::new(test_accounts());
        let _dyn_ref: &dyn Signer = &signer;
    }

    #[test]
    fn can_box_as_dyn_signer() {
        let signer = WatchOnlySigner::new(test_accounts());
        let _boxed: Box<dyn Signer> = Box::new(signer);
    }
}

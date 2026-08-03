//! Identity resolver types and People Chain integration.
//!
//! This module provides types and traits for resolving on-chain identity
//! information from Substrate-based chains (via the People Chain or the
//! legacy Identity pallet):
//!
//! - [`IdentityField`] — The standard identity fields (display, legal, web, etc.)
//! - [`JudgmentLevel`] — Registrar judgment levels per the Identity pallet.
//! - [`RegistrarJudgment`] — A judgment issued by a specific registrar.
//! - [`IdentityResolution`] — The full resolved identity for an account.
//! - [`SubIdentity`] — A sub-account linked to a parent identity.
//! - [`IdentityResolver`] — Async trait for resolving identities.
//! - [`CachedIdentityResolver`] — A TTL-based caching wrapper around any resolver.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::IdentityError;
use crate::types::AccountId32;

// ---------------------------------------------------------------------------
// IdentityField
// ---------------------------------------------------------------------------

/// Standard identity fields from the Substrate Identity pallet.
///
/// These correspond to the fields that can be set via `set_identity` on-chain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IdentityField {
    /// The display name (the primary human-readable name).
    Display,
    /// Legal name.
    Legal,
    /// Website URL.
    Web,
    /// Riot/Matrix handle.
    Riot,
    /// Email address.
    Email,
    /// Twitter handle.
    Twitter,
    /// A custom field not covered by the standard set.
    Custom(String),
}

impl fmt::Display for IdentityField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Display => write!(f, "display"),
            Self::Legal => write!(f, "legal"),
            Self::Web => write!(f, "web"),
            Self::Riot => write!(f, "riot"),
            Self::Email => write!(f, "email"),
            Self::Twitter => write!(f, "twitter"),
            Self::Custom(name) => write!(f, "custom:{name}"),
        }
    }
}

impl IdentityField {
    /// Parse a field name string into an `IdentityField`.
    ///
    /// Recognized names (case-insensitive): "display", "legal", "web",
    /// "riot", "email", "twitter". Anything else becomes `Custom(name)`.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "display" => Self::Display,
            "legal" => Self::Legal,
            "web" => Self::Web,
            "riot" => Self::Riot,
            "email" => Self::Email,
            "twitter" => Self::Twitter,
            _ => Self::Custom(name.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// JudgmentLevel
// ---------------------------------------------------------------------------

/// Registrar judgment levels from the Substrate Identity pallet.
///
/// These correspond to `Judgement` in `pallet_identity::types`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum JudgmentLevel {
    /// Identity status is unknown (not yet judged, or judgment was cleared).
    Unknown,
    /// The registrar considers the data reasonable but has not performed
    /// extensive verification.
    Reasonable,
    /// The registrar has fully verified the identity data.
    KnownGood,
    /// A previously valid judgment is now considered out of date.
    OutOfDate,
    /// The registrar considers the data to be of low quality.
    LowQuality,
    /// The registrar has determined the identity data to be erroneous.
    Erroneous,
}

impl JudgmentLevel {
    /// Returns `true` if this judgment level is considered a positive
    /// verification (either `Reasonable` or `KnownGood`).
    #[must_use]
    pub fn is_verified(&self) -> bool {
        matches!(self, Self::Reasonable | Self::KnownGood)
    }
}

impl fmt::Display for JudgmentLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => write!(f, "Unknown"),
            Self::Reasonable => write!(f, "Reasonable"),
            Self::KnownGood => write!(f, "KnownGood"),
            Self::OutOfDate => write!(f, "OutOfDate"),
            Self::LowQuality => write!(f, "LowQuality"),
            Self::Erroneous => write!(f, "Erroneous"),
        }
    }
}

// ---------------------------------------------------------------------------
// RegistrarJudgment
// ---------------------------------------------------------------------------

/// A judgment issued by a specific registrar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrarJudgment {
    /// The index of the registrar in the registrar list.
    pub registrar_index: u32,
    /// The judgment level assigned by this registrar.
    pub judgment: JudgmentLevel,
}

impl RegistrarJudgment {
    /// Create a new `RegistrarJudgment`.
    #[must_use]
    pub fn new(registrar_index: u32, judgment: JudgmentLevel) -> Self {
        Self {
            registrar_index,
            judgment,
        }
    }
}

// ---------------------------------------------------------------------------
// SubIdentity
// ---------------------------------------------------------------------------

/// A sub-account linked to a parent identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubIdentity {
    /// The parent account that owns this sub-identity.
    pub parent: AccountId32,
    /// The display name given to this sub-account by the parent.
    pub name: String,
    /// The account ID of the sub-account itself.
    pub account: AccountId32,
}

impl SubIdentity {
    /// Create a new `SubIdentity`.
    #[must_use]
    pub fn new(parent: AccountId32, name: impl Into<String>, account: AccountId32) -> Self {
        Self {
            parent,
            name: name.into(),
            account,
        }
    }
}

// ---------------------------------------------------------------------------
// IdentityResolution
// ---------------------------------------------------------------------------

/// The fully resolved on-chain identity for an account.
///
/// Contains the identity fields, any registrar judgments, sub-accounts,
/// and freshness metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityResolution {
    /// The account this identity belongs to.
    pub account: AccountId32,
    /// Identity field values (display name, email, web, etc.).
    pub fields: HashMap<IdentityField, String>,
    /// Registrar judgments that have been issued for this identity.
    pub judgments: Vec<RegistrarJudgment>,
    /// Sub-accounts associated with this identity.
    pub sub_accounts: Vec<SubIdentity>,
    /// When this resolution was fetched.
    pub fetched_at: DateTime<Utc>,
    /// The block number at which the identity data was observed.
    pub block_number: u64,
    /// Whether this resolution is considered stale (e.g., past its TTL).
    pub is_stale: bool,
}

impl IdentityResolution {
    /// Create a new `IdentityResolution` with the given account and block number.
    ///
    /// The `fetched_at` field is set to the current UTC time. All collection
    /// fields start empty and `is_stale` starts as `false`.
    #[must_use]
    pub fn new(account: AccountId32, block_number: u64) -> Self {
        Self {
            account,
            fields: HashMap::new(),
            judgments: Vec::new(),
            sub_accounts: Vec::new(),
            fetched_at: Utc::now(),
            block_number,
            is_stale: false,
        }
    }

    /// Add an identity field value.
    #[must_use]
    pub fn with_field(mut self, field: IdentityField, value: impl Into<String>) -> Self {
        self.fields.insert(field, value.into());
        self
    }

    /// Add a registrar judgment.
    #[must_use]
    pub fn with_judgment(mut self, judgment: RegistrarJudgment) -> Self {
        self.judgments.push(judgment);
        self
    }

    /// Add a sub-account.
    #[must_use]
    pub fn with_sub_account(mut self, sub: SubIdentity) -> Self {
        self.sub_accounts.push(sub);
        self
    }

    /// Returns the display name if the `Display` field is set.
    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        self.fields.get(&IdentityField::Display).map(String::as_str)
    }

    /// Returns `true` if any judgment is a positive verification
    /// (`Reasonable` or `KnownGood`).
    #[must_use]
    pub fn has_verified_judgment(&self) -> bool {
        self.judgments.iter().any(|j| j.judgment.is_verified())
    }

    /// Returns the highest judgment level, or `None` if there are no judgments.
    #[must_use]
    pub fn best_judgment(&self) -> Option<JudgmentLevel> {
        self.judgments.iter().map(|j| j.judgment).max()
    }
}

// ---------------------------------------------------------------------------
// IdentityResolver trait
// ---------------------------------------------------------------------------

/// Async trait for resolving on-chain identities.
///
/// Implementations may query the People Chain, the legacy Identity pallet,
/// or any other source of on-chain identity data.
#[async_trait::async_trait]
pub trait IdentityResolver: Send + Sync {
    /// Resolve the on-chain identity for the given account.
    ///
    /// Returns `Ok(resolution)` with the identity data, or an error if the
    /// account has no identity set or the query fails.
    async fn resolve(&self, account: &AccountId32) -> Result<IdentityResolution, IdentityError>;

    /// Resolve all sub-identities for the given parent account.
    ///
    /// Returns an empty `Vec` if the account has no sub-identities.
    async fn resolve_sub(
        &self,
        account: &AccountId32,
    ) -> Result<Vec<SubIdentity>, IdentityError>;
}

// ---------------------------------------------------------------------------
// CachedIdentityResolver
// ---------------------------------------------------------------------------

/// A TTL-based caching wrapper around any [`IdentityResolver`].
///
/// Caches resolved identities and sub-identities for a configurable duration.
/// Expired entries are marked as stale and re-fetched on the next access.
///
/// The cache uses `parking_lot::RwLock` for interior mutability, so callers
/// do not need external synchronization.
pub struct CachedIdentityResolver<R> {
    inner: R,
    ttl: Duration,
    identity_cache: parking_lot::RwLock<HashMap<[u8; 32], CachedEntry<IdentityResolution>>>,
    sub_cache: parking_lot::RwLock<HashMap<[u8; 32], CachedEntry<Vec<SubIdentity>>>>,
}

/// A cache entry with an insertion timestamp.
struct CachedEntry<T> {
    value: T,
    inserted_at: std::time::Instant,
}

impl<T> CachedEntry<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            inserted_at: std::time::Instant::now(),
        }
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        self.inserted_at.elapsed() > ttl
    }
}

impl<R> CachedIdentityResolver<R> {
    /// Create a new `CachedIdentityResolver` wrapping the given resolver
    /// with the specified TTL for cache entries.
    pub fn new(inner: R, ttl: Duration) -> Self {
        Self {
            inner,
            ttl,
            identity_cache: parking_lot::RwLock::new(HashMap::new()),
            sub_cache: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// Returns the configured TTL.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Returns the number of entries in the identity cache.
    #[must_use]
    pub fn identity_cache_len(&self) -> usize {
        self.identity_cache.read().len()
    }

    /// Returns the number of entries in the sub-identity cache.
    #[must_use]
    pub fn sub_cache_len(&self) -> usize {
        self.sub_cache.read().len()
    }

    /// Clear all cached entries.
    pub fn clear(&self) {
        self.identity_cache.write().clear();
        self.sub_cache.write().clear();
    }

    /// Remove a specific account from both caches.
    pub fn invalidate(&self, account: &AccountId32) {
        let key = account.to_bytes();
        self.identity_cache.write().remove(&key);
        self.sub_cache.write().remove(&key);
    }

    /// Returns a reference to the inner resolver.
    pub fn inner(&self) -> &R {
        &self.inner
    }
}

impl<R> fmt::Debug for CachedIdentityResolver<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachedIdentityResolver")
            .field("ttl", &self.ttl)
            .field("identity_cache_len", &self.identity_cache_len())
            .field("sub_cache_len", &self.sub_cache_len())
            .finish()
    }
}

#[async_trait::async_trait]
impl<R: IdentityResolver> IdentityResolver for CachedIdentityResolver<R> {
    async fn resolve(&self, account: &AccountId32) -> Result<IdentityResolution, IdentityError> {
        let key = account.to_bytes();

        // Check cache with a read lock first.
        {
            let cache = self.identity_cache.read();
            if let Some(entry) = cache.get(&key) {
                if !entry.is_expired(self.ttl) {
                    return Ok(entry.value.clone());
                }
            }
        }

        // Cache miss or expired — resolve from the inner resolver.
        let resolution = self.inner.resolve(account).await?;

        // Insert into cache.
        {
            let mut cache = self.identity_cache.write();
            cache.insert(key, CachedEntry::new(resolution.clone()));
        }

        Ok(resolution)
    }

    async fn resolve_sub(
        &self,
        account: &AccountId32,
    ) -> Result<Vec<SubIdentity>, IdentityError> {
        let key = account.to_bytes();

        // Check cache with a read lock first.
        {
            let cache = self.sub_cache.read();
            if let Some(entry) = cache.get(&key) {
                if !entry.is_expired(self.ttl) {
                    return Ok(entry.value.clone());
                }
            }
        }

        // Cache miss or expired — resolve from the inner resolver.
        let subs = self.inner.resolve_sub(account).await?;

        // Insert into cache.
        {
            let mut cache = self.sub_cache.write();
            cache.insert(key, CachedEntry::new(subs.clone()));
        }

        Ok(subs)
    }
}

/// Allow `Arc<T>` to be used as an `IdentityResolver` when `T` implements it.
#[async_trait::async_trait]
impl<T: IdentityResolver> IdentityResolver for Arc<T> {
    async fn resolve(&self, account: &AccountId32) -> Result<IdentityResolution, IdentityError> {
        (**self).resolve(account).await
    }

    async fn resolve_sub(
        &self,
        account: &AccountId32,
    ) -> Result<Vec<SubIdentity>, IdentityError> {
        (**self).resolve_sub(account).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    // -- IdentityField tests --

    #[test]
    fn identity_field_display() {
        assert_eq!(IdentityField::Display.to_string(), "display");
        assert_eq!(IdentityField::Legal.to_string(), "legal");
        assert_eq!(IdentityField::Web.to_string(), "web");
        assert_eq!(IdentityField::Riot.to_string(), "riot");
        assert_eq!(IdentityField::Email.to_string(), "email");
        assert_eq!(IdentityField::Twitter.to_string(), "twitter");
        assert_eq!(
            IdentityField::Custom("github".into()).to_string(),
            "custom:github"
        );
    }

    #[test]
    fn identity_field_from_name_known() {
        assert_eq!(IdentityField::from_name("display"), IdentityField::Display);
        assert_eq!(IdentityField::from_name("Display"), IdentityField::Display);
        assert_eq!(IdentityField::from_name("DISPLAY"), IdentityField::Display);
        assert_eq!(IdentityField::from_name("legal"), IdentityField::Legal);
        assert_eq!(IdentityField::from_name("web"), IdentityField::Web);
        assert_eq!(IdentityField::from_name("riot"), IdentityField::Riot);
        assert_eq!(IdentityField::from_name("email"), IdentityField::Email);
        assert_eq!(IdentityField::from_name("twitter"), IdentityField::Twitter);
    }

    #[test]
    fn identity_field_from_name_custom() {
        let field = IdentityField::from_name("github");
        assert_eq!(field, IdentityField::Custom("github".into()));
    }

    #[test]
    fn identity_field_hash_eq() {
        let mut map = HashMap::new();
        map.insert(IdentityField::Display, "Alice");
        map.insert(IdentityField::Email, "alice@example.com");
        map.insert(IdentityField::Custom("pgp".into()), "0xABCD");

        assert_eq!(map.get(&IdentityField::Display), Some(&"Alice"));
        assert_eq!(
            map.get(&IdentityField::Custom("pgp".into())),
            Some(&"0xABCD")
        );
        assert_eq!(map.get(&IdentityField::Web), None);
    }

    #[test]
    fn identity_field_serde_round_trip() {
        let fields = vec![
            IdentityField::Display,
            IdentityField::Custom("github".into()),
        ];
        let json = serde_json::to_string(&fields).expect("serialize");
        let back: Vec<IdentityField> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(fields, back);
    }

    // -- JudgmentLevel tests --

    #[test]
    fn judgment_level_is_verified() {
        assert!(!JudgmentLevel::Unknown.is_verified());
        assert!(JudgmentLevel::Reasonable.is_verified());
        assert!(JudgmentLevel::KnownGood.is_verified());
        assert!(!JudgmentLevel::OutOfDate.is_verified());
        assert!(!JudgmentLevel::LowQuality.is_verified());
        assert!(!JudgmentLevel::Erroneous.is_verified());
    }

    #[test]
    fn judgment_level_ordering() {
        assert!(JudgmentLevel::Unknown < JudgmentLevel::Reasonable);
        assert!(JudgmentLevel::Reasonable < JudgmentLevel::KnownGood);
        assert!(JudgmentLevel::KnownGood < JudgmentLevel::OutOfDate);
        assert!(JudgmentLevel::OutOfDate < JudgmentLevel::LowQuality);
        assert!(JudgmentLevel::LowQuality < JudgmentLevel::Erroneous);
    }

    #[test]
    fn judgment_level_display() {
        assert_eq!(JudgmentLevel::Unknown.to_string(), "Unknown");
        assert_eq!(JudgmentLevel::Reasonable.to_string(), "Reasonable");
        assert_eq!(JudgmentLevel::KnownGood.to_string(), "KnownGood");
        assert_eq!(JudgmentLevel::OutOfDate.to_string(), "OutOfDate");
        assert_eq!(JudgmentLevel::LowQuality.to_string(), "LowQuality");
        assert_eq!(JudgmentLevel::Erroneous.to_string(), "Erroneous");
    }

    #[test]
    fn judgment_level_serde_round_trip() {
        for level in [
            JudgmentLevel::Unknown,
            JudgmentLevel::Reasonable,
            JudgmentLevel::KnownGood,
            JudgmentLevel::OutOfDate,
            JudgmentLevel::LowQuality,
            JudgmentLevel::Erroneous,
        ] {
            let json = serde_json::to_string(&level).expect("serialize");
            let back: JudgmentLevel = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(level, back);
        }
    }

    // -- RegistrarJudgment tests --

    #[test]
    fn registrar_judgment_construction() {
        let j = RegistrarJudgment::new(1, JudgmentLevel::KnownGood);
        assert_eq!(j.registrar_index, 1);
        assert_eq!(j.judgment, JudgmentLevel::KnownGood);
    }

    #[test]
    fn registrar_judgment_serde_round_trip() {
        let j = RegistrarJudgment::new(3, JudgmentLevel::Reasonable);
        let json = serde_json::to_string(&j).expect("serialize");
        let back: RegistrarJudgment = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(j, back);
    }

    // -- SubIdentity tests --

    #[test]
    fn sub_identity_construction() {
        let parent = AccountId32::from_bytes([1; 32]);
        let child = AccountId32::from_bytes([2; 32]);
        let sub = SubIdentity::new(parent, "staking", child);
        assert_eq!(sub.parent, parent);
        assert_eq!(sub.name, "staking");
        assert_eq!(sub.account, child);
    }

    #[test]
    fn sub_identity_serde_round_trip() {
        let sub = SubIdentity::new(
            AccountId32::from_bytes([0xAA; 32]),
            "validator",
            AccountId32::from_bytes([0xBB; 32]),
        );
        let json = serde_json::to_string(&sub).expect("serialize");
        let back: SubIdentity = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(sub, back);
    }

    // -- IdentityResolution tests --

    #[test]
    fn identity_resolution_builder() {
        let account = AccountId32::from_bytes([42; 32]);
        let resolution = IdentityResolution::new(account, 100)
            .with_field(IdentityField::Display, "Alice")
            .with_field(IdentityField::Email, "alice@example.com")
            .with_judgment(RegistrarJudgment::new(0, JudgmentLevel::KnownGood))
            .with_sub_account(SubIdentity::new(
                account,
                "staking",
                AccountId32::from_bytes([43; 32]),
            ));

        assert_eq!(resolution.account, account);
        assert_eq!(resolution.block_number, 100);
        assert_eq!(resolution.fields.len(), 2);
        assert_eq!(resolution.display_name(), Some("Alice"));
        assert_eq!(resolution.judgments.len(), 1);
        assert_eq!(resolution.sub_accounts.len(), 1);
        assert!(!resolution.is_stale);
    }

    #[test]
    fn identity_resolution_display_name_none_when_missing() {
        let resolution = IdentityResolution::new(AccountId32::from_bytes([1; 32]), 50);
        assert_eq!(resolution.display_name(), None);
    }

    #[test]
    fn identity_resolution_has_verified_judgment() {
        let resolution = IdentityResolution::new(AccountId32::from_bytes([1; 32]), 50)
            .with_judgment(RegistrarJudgment::new(0, JudgmentLevel::Unknown))
            .with_judgment(RegistrarJudgment::new(1, JudgmentLevel::Reasonable));

        assert!(resolution.has_verified_judgment());
    }

    #[test]
    fn identity_resolution_no_verified_judgment() {
        let resolution = IdentityResolution::new(AccountId32::from_bytes([1; 32]), 50)
            .with_judgment(RegistrarJudgment::new(0, JudgmentLevel::LowQuality));

        assert!(!resolution.has_verified_judgment());
    }

    #[test]
    fn identity_resolution_best_judgment() {
        let resolution = IdentityResolution::new(AccountId32::from_bytes([1; 32]), 50)
            .with_judgment(RegistrarJudgment::new(0, JudgmentLevel::Unknown))
            .with_judgment(RegistrarJudgment::new(1, JudgmentLevel::KnownGood))
            .with_judgment(RegistrarJudgment::new(2, JudgmentLevel::Reasonable));

        assert_eq!(resolution.best_judgment(), Some(JudgmentLevel::KnownGood));
    }

    #[test]
    fn identity_resolution_best_judgment_empty() {
        let resolution = IdentityResolution::new(AccountId32::from_bytes([1; 32]), 50);
        assert_eq!(resolution.best_judgment(), None);
    }

    #[test]
    fn identity_resolution_serde_round_trip() {
        let resolution = IdentityResolution::new(AccountId32::from_bytes([0xCC; 32]), 999)
            .with_field(IdentityField::Display, "Bob")
            .with_field(IdentityField::Web, "https://bob.dev")
            .with_judgment(RegistrarJudgment::new(0, JudgmentLevel::Reasonable));

        let json = serde_json::to_string(&resolution).expect("serialize");
        let back: IdentityResolution = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.account, resolution.account);
        assert_eq!(back.block_number, 999);
        assert_eq!(back.fields.len(), 2);
        assert_eq!(back.display_name(), Some("Bob"));
        assert_eq!(back.judgments.len(), 1);
    }

    // -- Fake resolver for testing the trait and cache --

    /// A fake resolver that returns deterministic data based on account bytes.
    struct FakeResolver {
        call_count: AtomicU32,
    }

    impl FakeResolver {
        fn new() -> Self {
            Self {
                call_count: AtomicU32::new(0),
            }
        }

        fn calls(&self) -> u32 {
            self.call_count.load(Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl IdentityResolver for FakeResolver {
        async fn resolve(
            &self,
            account: &AccountId32,
        ) -> Result<IdentityResolution, IdentityError> {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            let name = format!("User-{}", account.to_bytes()[0]);
            Ok(IdentityResolution::new(*account, 1000)
                .with_field(IdentityField::Display, &name))
        }

        async fn resolve_sub(
            &self,
            account: &AccountId32,
        ) -> Result<Vec<SubIdentity>, IdentityError> {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            Ok(vec![SubIdentity::new(
                *account,
                "sub-0",
                AccountId32::from_bytes([0xFF; 32]),
            )])
        }
    }

    #[tokio::test]
    async fn resolver_trait_resolve() {
        let resolver = FakeResolver::new();
        let account = AccountId32::from_bytes([7; 32]);
        let resolution = resolver.resolve(&account).await.expect("resolve");

        assert_eq!(resolution.display_name(), Some("User-7"));
        assert_eq!(resolution.block_number, 1000);
    }

    #[tokio::test]
    async fn resolver_trait_resolve_sub() {
        let resolver = FakeResolver::new();
        let account = AccountId32::from_bytes([7; 32]);
        let subs = resolver.resolve_sub(&account).await.expect("resolve_sub");

        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].name, "sub-0");
        assert_eq!(subs[0].parent, account);
    }

    #[tokio::test]
    async fn cached_resolver_returns_cached_identity() {
        let inner = FakeResolver::new();
        let cached = CachedIdentityResolver::new(inner, Duration::from_secs(60));
        let account = AccountId32::from_bytes([1; 32]);

        // First call populates the cache.
        let r1 = cached.resolve(&account).await.expect("resolve");
        assert_eq!(r1.display_name(), Some("User-1"));
        assert_eq!(cached.inner().calls(), 1);

        // Second call should come from cache.
        let r2 = cached.resolve(&account).await.expect("resolve");
        assert_eq!(r2.display_name(), Some("User-1"));
        assert_eq!(cached.inner().calls(), 1); // no additional call
    }

    #[tokio::test]
    async fn cached_resolver_returns_cached_subs() {
        let inner = FakeResolver::new();
        let cached = CachedIdentityResolver::new(inner, Duration::from_secs(60));
        let account = AccountId32::from_bytes([2; 32]);

        let s1 = cached.resolve_sub(&account).await.expect("resolve_sub");
        assert_eq!(s1.len(), 1);
        assert_eq!(cached.inner().calls(), 1);

        let s2 = cached.resolve_sub(&account).await.expect("resolve_sub");
        assert_eq!(s2.len(), 1);
        assert_eq!(cached.inner().calls(), 1); // cached
    }

    #[tokio::test]
    async fn cached_resolver_different_accounts_are_separate() {
        let inner = FakeResolver::new();
        let cached = CachedIdentityResolver::new(inner, Duration::from_secs(60));
        let a1 = AccountId32::from_bytes([1; 32]);
        let a2 = AccountId32::from_bytes([2; 32]);

        let r1 = cached.resolve(&a1).await.expect("resolve");
        let r2 = cached.resolve(&a2).await.expect("resolve");

        assert_eq!(r1.display_name(), Some("User-1"));
        assert_eq!(r2.display_name(), Some("User-2"));
        assert_eq!(cached.inner().calls(), 2);
    }

    #[tokio::test]
    async fn cached_resolver_invalidate() {
        let inner = FakeResolver::new();
        let cached = CachedIdentityResolver::new(inner, Duration::from_secs(60));
        let account = AccountId32::from_bytes([5; 32]);

        cached.resolve(&account).await.expect("resolve");
        assert_eq!(cached.identity_cache_len(), 1);

        cached.invalidate(&account);
        assert_eq!(cached.identity_cache_len(), 0);

        // Next call should go to inner again.
        cached.resolve(&account).await.expect("resolve");
        assert_eq!(cached.inner().calls(), 2);
    }

    #[tokio::test]
    async fn cached_resolver_clear() {
        let inner = FakeResolver::new();
        let cached = CachedIdentityResolver::new(inner, Duration::from_secs(60));

        cached
            .resolve(&AccountId32::from_bytes([1; 32]))
            .await
            .expect("resolve");
        cached
            .resolve(&AccountId32::from_bytes([2; 32]))
            .await
            .expect("resolve");
        cached
            .resolve_sub(&AccountId32::from_bytes([3; 32]))
            .await
            .expect("resolve_sub");

        assert_eq!(cached.identity_cache_len(), 2);
        assert_eq!(cached.sub_cache_len(), 1);

        cached.clear();
        assert_eq!(cached.identity_cache_len(), 0);
        assert_eq!(cached.sub_cache_len(), 0);
    }

    #[tokio::test]
    async fn cached_resolver_expired_entry_refetches() {
        let inner = FakeResolver::new();
        // Use a zero TTL so entries expire immediately.
        let cached = CachedIdentityResolver::new(inner, Duration::from_millis(0));
        let account = AccountId32::from_bytes([9; 32]);

        cached.resolve(&account).await.expect("resolve");
        assert_eq!(cached.inner().calls(), 1);

        // Entry is already expired because TTL is 0.
        cached.resolve(&account).await.expect("resolve");
        assert_eq!(cached.inner().calls(), 2);
    }

    #[tokio::test]
    async fn cached_resolver_ttl_accessor() {
        let inner = FakeResolver::new();
        let ttl = Duration::from_secs(300);
        let cached = CachedIdentityResolver::new(inner, ttl);
        assert_eq!(cached.ttl(), ttl);
    }

    #[tokio::test]
    async fn cached_resolver_debug_impl() {
        let inner = FakeResolver::new();
        let cached = CachedIdentityResolver::new(inner, Duration::from_secs(60));
        let debug = format!("{cached:?}");
        assert!(debug.contains("CachedIdentityResolver"));
        assert!(debug.contains("ttl"));
    }

    #[tokio::test]
    async fn resolver_via_arc() {
        let resolver = Arc::new(FakeResolver::new());
        let account = AccountId32::from_bytes([3; 32]);

        let r = resolver.resolve(&account).await.expect("resolve");
        assert_eq!(r.display_name(), Some("User-3"));
    }
}

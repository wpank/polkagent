//! Audit store trait.
//!
//! [`AuditStore`] defines the async interface for persisting and querying
//! audit entries. Implementations range from the in-memory store (for
//! testing) to durable backends.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::action::AuditAction;
use crate::entry::{AuditEntry, AuditId};
use crate::error::AuditResult;
use crate::query::AuditQuery;

/// Async trait for audit log persistence.
///
/// All operations are append-only: there is no `delete` or `update` method.
/// This is deliberate — audit logs must not support retroactive modification.
#[async_trait]
pub trait AuditStore: Send + Sync {
    /// Append a new entry to the audit log.
    ///
    /// The implementation must ensure the entry's integrity hash is set
    /// correctly (chained to the previous entry) before storage.
    async fn append(&self, entry: AuditEntry) -> AuditResult<AuditEntry>;

    /// Retrieve a single entry by its unique [`AuditId`].
    ///
    /// Returns `Ok(Some(entry))` if found, `Ok(None)` if not.
    async fn get_by_id(&self, id: AuditId) -> AuditResult<Option<AuditEntry>>;

    /// Query entries by actor ID, ordered by timestamp ascending.
    async fn query_by_actor(&self, actor_id: &str) -> AuditResult<Vec<AuditEntry>>;

    /// Query entries by action type, ordered by timestamp ascending.
    async fn query_by_action(&self, action: &AuditAction) -> AuditResult<Vec<AuditEntry>>;

    /// Query entries within a time range (inclusive), ordered by timestamp
    /// ascending.
    async fn query_by_time_range(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> AuditResult<Vec<AuditEntry>>;

    /// Execute an arbitrary query built with [`AuditQuery`].
    async fn query(&self, query: &AuditQuery) -> AuditResult<Vec<AuditEntry>>;

    /// Return the total number of entries in the store.
    async fn count(&self) -> AuditResult<usize>;

    /// Verify the integrity of the hash chain over all entries.
    ///
    /// Returns `Ok(())` if the chain is valid, or an
    /// [`crate::AuditError::IntegrityViolation`] on the first mismatch.
    async fn verify_integrity(&self) -> AuditResult<()>;
}

//! `polkagent-audit` -- Comprehensive audit logging for the Polkagent platform.
//!
//! This crate provides a structured, tamper-evident audit trail for all agent
//! actions, policy decisions, and system events. Every auditable event is
//! recorded as an [`AuditEntry`] with a BLAKE3 integrity hash that chains it
//! to the previous entry, making silent deletion or modification detectable.
//!
//! # Architecture
//!
//! ```text
//!  caller
//!    |
//!    v
//! AuditLogger  (facade -- the main entry point)
//!    |
//!    v
//! AuditStore   (trait -- pluggable storage backend)
//!    |
//!    v
//! InMemoryAuditStore | (future) SqliteAuditStore | ...
//! ```
//!
//! # Module overview
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`entry`] | [`AuditEntry`], [`AuditId`], [`ActionOutcome`], [`ResourceInfo`] |
//! | [`actor`] | [`ActorInfo`], [`ActorType`] |
//! | [`action`] | [`AuditAction`] enum |
//! | [`store`] | [`AuditStore`] trait |
//! | [`memory_store`] | [`InMemoryAuditStore`] |
//! | [`query`] | [`AuditQuery`] builder |
//! | [`integrity`] | BLAKE3 hash chain functions |
//! | [`formatter`] | JSON Lines, text, and CSV formatters |
//! | [`error`] | [`AuditError`] |
//!
//! # Quick start
//!
//! ```rust
//! use polkagent_audit::{AuditLogger, InMemoryAuditStore, AuditAction, ActorInfo, ResourceInfo, ActionOutcome};
//! use std::sync::Arc;
//!
//! # #[tokio::main]
//! # async fn main() {
//! let store = Arc::new(InMemoryAuditStore::new());
//! let logger = AuditLogger::new(store);
//!
//! logger.log(
//!     ActorInfo::agent("agent-1"),
//!     AuditAction::RunStarted,
//!     ResourceInfo::new("run", "run-42"),
//!     ActionOutcome::Success,
//!     serde_json::json!({"trigger": "api"}),
//! ).await.expect("audit log");
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod action;
pub mod actor;
pub mod entry;
pub mod error;
pub mod formatter;
pub mod integrity;
pub mod memory_store;
pub mod query;
pub mod store;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use action::AuditAction;
pub use actor::{ActorInfo, ActorType};
pub use entry::{ActionOutcome, AuditEntry, AuditId, ResourceInfo};
pub use error::{AuditError, AuditResult};
pub use formatter::OutputFormat;
pub use integrity::{compute_hash, verify_chain, GENESIS_HASH};
pub use memory_store::InMemoryAuditStore;
pub use query::AuditQuery;
pub use store::AuditStore;

// ---------------------------------------------------------------------------
// AuditLogger
// ---------------------------------------------------------------------------

use std::sync::Arc;
use tracing::instrument;

/// The main facade for audit logging.
///
/// `AuditLogger` wraps an [`AuditStore`] implementation and provides a
/// high-level `log` method that constructs an [`AuditEntry`], computes its
/// integrity hash, and appends it to the store.
///
/// The logger is cheaply cloneable (it holds an `Arc` to the store).
#[derive(Clone)]
pub struct AuditLogger {
    store: Arc<dyn AuditStore>,
}

impl std::fmt::Debug for AuditLogger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLogger")
            .field("store", &"<dyn AuditStore>")
            .finish()
    }
}

impl AuditLogger {
    /// Create a new `AuditLogger` backed by the given store.
    pub fn new(store: Arc<dyn AuditStore>) -> Self {
        Self { store }
    }

    /// Record an audit event.
    ///
    /// This constructs an [`AuditEntry`], delegates integrity-hash computation
    /// to the store's `append` method, and returns the stored entry.
    #[instrument(skip(self, context), fields(action = %action, actor = %actor))]
    pub async fn log(
        &self,
        actor: ActorInfo,
        action: AuditAction,
        resource: ResourceInfo,
        outcome: ActionOutcome,
        context: serde_json::Value,
    ) -> AuditResult<AuditEntry> {
        let entry = AuditEntry::new(actor, action, resource, outcome, context);
        self.store.append(entry).await
    }

    /// Return a reference to the underlying store for direct queries.
    pub fn store(&self) -> &dyn AuditStore {
        self.store.as_ref()
    }

    /// Return the total number of entries in the store.
    pub async fn count(&self) -> AuditResult<usize> {
        self.store.count().await
    }

    /// Query entries using an [`AuditQuery`].
    pub async fn query(&self, query: &AuditQuery) -> AuditResult<Vec<AuditEntry>> {
        self.store.query(query).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_logger() -> AuditLogger {
        AuditLogger::new(Arc::new(InMemoryAuditStore::new()))
    }

    #[tokio::test]
    async fn logger_log_creates_entry() {
        let logger = make_logger();
        let entry = logger
            .log(
                ActorInfo::agent("agent-1"),
                AuditAction::RunStarted,
                ResourceInfo::new("run", "run-42"),
                ActionOutcome::Success,
                serde_json::json!({"trigger": "api"}),
            )
            .await
            .expect("log");

        assert_eq!(entry.actor.id, "agent-1");
        assert_eq!(entry.action, AuditAction::RunStarted);
        assert!(!entry.integrity_hash.is_empty());
    }

    #[tokio::test]
    async fn logger_count() {
        let logger = make_logger();
        assert_eq!(logger.count().await.expect("count"), 0);

        logger
            .log(
                ActorInfo::agent("a-1"),
                AuditAction::RunStarted,
                ResourceInfo::new("run", "r-1"),
                ActionOutcome::Success,
                serde_json::Value::Null,
            )
            .await
            .expect("log");

        assert_eq!(logger.count().await.expect("count"), 1);
    }

    #[tokio::test]
    async fn logger_query() {
        let logger = make_logger();
        logger
            .log(
                ActorInfo::agent("a-1"),
                AuditAction::RunStarted,
                ResourceInfo::new("run", "r-1"),
                ActionOutcome::Success,
                serde_json::Value::Null,
            )
            .await
            .expect("log");
        logger
            .log(
                ActorInfo::agent("a-2"),
                AuditAction::ToolInvoked,
                ResourceInfo::new("tool", "t-1"),
                ActionOutcome::Success,
                serde_json::Value::Null,
            )
            .await
            .expect("log");

        let q = AuditQuery::new().actor("a-1").build();
        let results = logger.query(&q).await.expect("query");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].actor.id, "a-1");
    }

    #[tokio::test]
    async fn logger_cloneable() {
        let logger = make_logger();
        let logger2 = logger.clone();

        logger
            .log(
                ActorInfo::agent("a-1"),
                AuditAction::RunStarted,
                ResourceInfo::new("run", "r-1"),
                ActionOutcome::Success,
                serde_json::Value::Null,
            )
            .await
            .expect("log");

        // The clone shares the same store.
        assert_eq!(logger2.count().await.expect("count"), 1);
    }

    #[tokio::test]
    async fn logger_multiple_entries_form_valid_chain() {
        let store = Arc::new(InMemoryAuditStore::new());
        let logger = AuditLogger::new(store.clone());

        for i in 0..5 {
            logger
                .log(
                    ActorInfo::agent(&format!("a-{i}")),
                    AuditAction::RunStarted,
                    ResourceInfo::new("run", &format!("r-{i}")),
                    ActionOutcome::Success,
                    serde_json::Value::Null,
                )
                .await
                .expect("log");
        }

        store.verify_integrity().expect("chain should be valid");
    }

    #[tokio::test]
    async fn concurrent_logging() {
        let store = Arc::new(InMemoryAuditStore::new());
        let logger = AuditLogger::new(store.clone());

        let mut handles = Vec::new();
        for i in 0..20 {
            let logger = logger.clone();
            handles.push(tokio::spawn(async move {
                logger
                    .log(
                        ActorInfo::agent(&format!("a-{i}")),
                        AuditAction::ToolInvoked,
                        ResourceInfo::new("tool", &format!("t-{i}")),
                        ActionOutcome::Success,
                        serde_json::Value::Null,
                    )
                    .await
                    .expect("log");
            }));
        }

        for handle in handles {
            handle.await.expect("join");
        }

        assert_eq!(store.snapshot().len(), 20);
        // The chain should still be valid even under concurrent appends.
        store.verify_integrity().expect("chain should be valid");
    }

    #[tokio::test]
    async fn security_sensitive_audit_trail() {
        let logger = make_logger();

        // Log a security-sensitive action.
        let entry = logger
            .log(
                ActorInfo::agent("agent-1"),
                AuditAction::SecretAccessed,
                ResourceInfo::new("secret", "api-key").with_description("Production API key"),
                ActionOutcome::Success,
                serde_json::json!({"purpose": "chain_query"}),
            )
            .await
            .expect("log");

        assert!(entry.action.is_security_sensitive());
        assert_eq!(
            entry.resource.description.as_deref(),
            Some("Production API key")
        );
    }

    #[tokio::test]
    async fn denied_action_audit() {
        let logger = make_logger();

        let entry = logger
            .log(
                ActorInfo::agent("agent-untrusted"),
                AuditAction::PolicyDecision,
                ResourceInfo::new("effect", "chain_submit"),
                ActionOutcome::Denied,
                serde_json::json!({"policy": "no-chain-submit", "reason": "agent not authorized"}),
            )
            .await
            .expect("log");

        assert_eq!(entry.outcome, ActionOutcome::Denied);
        assert!(entry.action.is_security_sensitive());
    }

    #[tokio::test]
    async fn full_lifecycle_audit() {
        let store = Arc::new(InMemoryAuditStore::new());
        let logger = AuditLogger::new(store.clone());

        // Simulate a full run lifecycle.
        logger
            .log(
                ActorInfo::agent("agent-1"),
                AuditAction::RunStarted,
                ResourceInfo::new("run", "run-1"),
                ActionOutcome::Success,
                serde_json::Value::Null,
            )
            .await
            .expect("log");

        logger
            .log(
                ActorInfo::agent("agent-1"),
                AuditAction::EffectRequested,
                ResourceInfo::new("effect", "eff-1"),
                ActionOutcome::Success,
                serde_json::json!({"kind": "chain_submit"}),
            )
            .await
            .expect("log");

        logger
            .log(
                ActorInfo::agent("agent-1"),
                AuditAction::GrantEvaluated,
                ResourceInfo::new("grant", "grant-1"),
                ActionOutcome::Success,
                serde_json::json!({"policy": "allow-chain-submit"}),
            )
            .await
            .expect("log");

        logger
            .log(
                ActorInfo::agent("agent-1"),
                AuditAction::EffectExecuted,
                ResourceInfo::new("effect", "eff-1"),
                ActionOutcome::Success,
                serde_json::json!({"tx_hash": "0xdeadbeef"}),
            )
            .await
            .expect("log");

        logger
            .log(
                ActorInfo::agent("agent-1"),
                AuditAction::RunCompleted,
                ResourceInfo::new("run", "run-1"),
                ActionOutcome::Success,
                serde_json::Value::Null,
            )
            .await
            .expect("log");

        assert_eq!(store.snapshot().len(), 5);
        store.verify_integrity().expect("chain valid");

        // Query only effect actions.
        let q = AuditQuery::new()
            .actor("agent-1")
            .action(AuditAction::EffectExecuted)
            .build();
        let results = logger.query(&q).await.expect("query");
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn format_audit_trail() {
        let store = Arc::new(InMemoryAuditStore::new());
        let logger = AuditLogger::new(store.clone());

        logger
            .log(
                ActorInfo::agent("agent-1"),
                AuditAction::RunStarted,
                ResourceInfo::new("run", "run-1"),
                ActionOutcome::Success,
                serde_json::Value::Null,
            )
            .await
            .expect("log");

        let entries = store.snapshot();
        let json = formatter::format_entries(&entries, OutputFormat::JsonLines).expect("format");
        assert!(!json.is_empty());

        let text = formatter::format_entries(&entries, OutputFormat::Text).expect("format");
        assert!(text.contains("run_started"));

        let csv = formatter::format_entries(&entries, OutputFormat::Csv).expect("format");
        assert!(csv.starts_with(formatter::csv_header()));
    }
}

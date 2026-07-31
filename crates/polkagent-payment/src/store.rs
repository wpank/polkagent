//! Async trait for persisting payment intents, receipts, and cost records.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::PaymentError;
use crate::types::{CostRecord, PaymentIntent, PaymentReceipt, PaymentStatus, UsageSummary};

/// Persistence layer for payment-related data.
///
/// Implementations may be backed by SQLite, PostgreSQL, or an in-memory store
/// for testing. All methods are async to accommodate network-backed stores.
#[async_trait::async_trait]
pub trait PaymentStore: Send + Sync {
    /// Record an LLM cost entry.
    async fn record_cost(&self, cost_record: CostRecord) -> Result<(), PaymentError>;

    /// Retrieve all cost records for a given run.
    async fn get_costs(&self, run_id: &str) -> Result<Vec<CostRecord>, PaymentError>;

    /// Compute aggregated usage statistics for an agent over a time range.
    async fn get_usage(
        &self,
        agent_id: &str,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<UsageSummary, PaymentError>;

    /// Persist a new payment intent.
    async fn create_intent(&self, intent: PaymentIntent) -> Result<(), PaymentError>;

    /// Look up a payment intent by ID.
    async fn get_intent(&self, id: Uuid) -> Result<PaymentIntent, PaymentError>;

    /// Transition the status of an existing payment intent.
    async fn update_intent_status(
        &self,
        id: Uuid,
        status: PaymentStatus,
    ) -> Result<(), PaymentError>;

    /// Store a payment receipt (on-chain confirmation proof).
    async fn create_receipt(&self, receipt: PaymentReceipt) -> Result<(), PaymentError>;
}

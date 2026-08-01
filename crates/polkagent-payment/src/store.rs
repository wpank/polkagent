//! Async trait for persisting payment intents, receipts, and cost records.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::PaymentError;
use crate::types::{CostRecord, PaymentIntent, PaymentReceipt, PaymentStatus, UsageSummary};

/// Summary of the agent's current balance and budget state.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BalanceSummary {
    /// Available balance in the smallest denomination, if tracked.
    pub available: Option<u128>,
    /// Currency / asset identifier (e.g. `"NATIVE"`, `"USDT"`).
    pub currency: String,
    /// Total amount spent to date (in the smallest denomination).
    pub total_spent: u128,
    /// Whether a budget has been configured.
    pub budget_configured: bool,
    /// The budget limit (in the smallest denomination), if configured.
    pub budget_limit: Option<u128>,
}

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

    /// List all payment receipts, ordered by confirmation time descending.
    ///
    /// The default implementation returns an empty list so that existing
    /// implementations continue to compile without changes.
    async fn list_receipts(&self) -> Result<Vec<PaymentReceipt>, PaymentError> {
        Ok(vec![])
    }

    /// Look up a payment receipt by its associated intent ID.
    ///
    /// The default implementation returns `IntentNotFound` so that existing
    /// implementations continue to compile without changes.
    async fn get_receipt(&self, intent_id: Uuid) -> Result<PaymentReceipt, PaymentError> {
        Err(PaymentError::IntentNotFound { id: intent_id })
    }

    /// Get a balance summary for the agent.
    ///
    /// The default implementation returns a summary indicating that balance
    /// tracking is not yet implemented by this store.
    async fn get_balance(&self) -> Result<BalanceSummary, PaymentError> {
        Ok(BalanceSummary {
            available: None,
            currency: "NATIVE".to_owned(),
            total_spent: 0,
            budget_configured: false,
            budget_limit: None,
        })
    }
}

//! Async trait for persisting payment intents, receipts, and cost records.

use std::collections::HashMap;
use std::sync::Mutex;

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

// ---------------------------------------------------------------------------
// InMemoryPaymentStore
// ---------------------------------------------------------------------------

/// Internal state for [`InMemoryPaymentStore`].
#[derive(Debug, Default)]
struct InMemoryState {
    /// Cost records keyed by `run_id`.
    costs: HashMap<String, Vec<CostRecord>>,
    /// Payment intents keyed by intent UUID.
    intents: HashMap<Uuid, PaymentIntent>,
    /// Payment receipts keyed by intent UUID.
    receipts: HashMap<Uuid, PaymentReceipt>,
    /// Maps `run_id` -> `agent_id` (populated from intents) so that
    /// `get_usage` can correlate cost records to agents.
    run_to_agent: HashMap<String, String>,
    /// Optional budget configuration: `(limit, currency)`.
    budget: Option<(u128, String)>,
}

/// A fully in-memory [`PaymentStore`] suitable for tests and lightweight
/// single-process deployments.
///
/// All data lives in a `Mutex`-protected hash map and is lost when the
/// store is dropped.
#[derive(Debug, Default)]
pub struct InMemoryPaymentStore {
    state: Mutex<InMemoryState>,
}

impl InMemoryPaymentStore {
    /// Create a new empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a store with a pre-configured budget.
    #[must_use]
    pub fn with_budget(limit: u128, currency: impl Into<String>) -> Self {
        Self {
            state: Mutex::new(InMemoryState {
                budget: Some((limit, currency.into())),
                ..Default::default()
            }),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, InMemoryState>, PaymentError> {
        self.state
            .lock()
            .map_err(|_| PaymentError::store("in-memory store lock poisoned"))
    }
}

#[async_trait::async_trait]
impl PaymentStore for InMemoryPaymentStore {
    async fn record_cost(&self, cost_record: CostRecord) -> Result<(), PaymentError> {
        let mut state = self.lock()?;
        state
            .costs
            .entry(cost_record.run_id.clone())
            .or_default()
            .push(cost_record);
        Ok(())
    }

    async fn get_costs(&self, run_id: &str) -> Result<Vec<CostRecord>, PaymentError> {
        let state = self.lock()?;
        Ok(state.costs.get(run_id).cloned().unwrap_or_default())
    }

    async fn get_usage(
        &self,
        agent_id: &str,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<UsageSummary, PaymentError> {
        let state = self.lock()?;

        // Collect all run_ids that belong to this agent.
        let run_ids: Vec<&String> = state
            .run_to_agent
            .iter()
            .filter(|(_, aid)| aid.as_str() == agent_id)
            .map(|(rid, _)| rid)
            .collect();

        let mut total_tokens: u64 = 0;
        let mut estimated_usd: f64 = 0.0;
        let mut seen_runs = std::collections::HashSet::new();

        for rid in &run_ids {
            if let Some(records) = state.costs.get(rid.as_str()) {
                for r in records {
                    if r.recorded_at >= since && r.recorded_at < until {
                        total_tokens += r.input_tokens + r.output_tokens;
                        estimated_usd += r.estimated_usd;
                        seen_runs.insert(rid);
                    }
                }
            }
        }

        Ok(UsageSummary {
            total_runs: seen_runs.len() as u64,
            total_tokens,
            estimated_usd,
            period_start: since,
            period_end: until,
        })
    }

    async fn create_intent(&self, intent: PaymentIntent) -> Result<(), PaymentError> {
        let mut state = self.lock()?;

        // Idempotency check.
        let duplicate = state
            .intents
            .values()
            .any(|i| i.idempotency_key == intent.idempotency_key);
        if duplicate {
            return Err(PaymentError::IdempotencyConflict {
                key: intent.idempotency_key.clone(),
            });
        }

        // Track the run_id -> agent_id mapping for get_usage.
        state
            .run_to_agent
            .insert(intent.run_id.clone(), intent.agent_id.clone());
        state.intents.insert(intent.id, intent);
        Ok(())
    }

    async fn get_intent(&self, id: Uuid) -> Result<PaymentIntent, PaymentError> {
        let state = self.lock()?;
        state
            .intents
            .get(&id)
            .cloned()
            .ok_or(PaymentError::IntentNotFound { id })
    }

    async fn update_intent_status(
        &self,
        id: Uuid,
        status: PaymentStatus,
    ) -> Result<(), PaymentError> {
        let mut state = self.lock()?;
        let intent = state
            .intents
            .get_mut(&id)
            .ok_or(PaymentError::IntentNotFound { id })?;
        intent.status = status;
        Ok(())
    }

    async fn create_receipt(&self, receipt: PaymentReceipt) -> Result<(), PaymentError> {
        let mut state = self.lock()?;
        state.receipts.insert(receipt.intent_id, receipt);
        Ok(())
    }

    async fn list_receipts(&self) -> Result<Vec<PaymentReceipt>, PaymentError> {
        let state = self.lock()?;
        let mut receipts: Vec<PaymentReceipt> = state.receipts.values().cloned().collect();
        receipts.sort_by(|a, b| b.confirmed_at.cmp(&a.confirmed_at));
        Ok(receipts)
    }

    async fn get_receipt(&self, intent_id: Uuid) -> Result<PaymentReceipt, PaymentError> {
        let state = self.lock()?;
        state
            .receipts
            .get(&intent_id)
            .cloned()
            .ok_or(PaymentError::IntentNotFound { id: intent_id })
    }

    async fn get_balance(&self) -> Result<BalanceSummary, PaymentError> {
        let state = self.lock()?;

        // Sum all receipt fee_paid amounts as total_spent.
        let total_spent: u128 = state.receipts.values().map(|r| r.fee_paid.value).sum();

        let (budget_configured, budget_limit, currency) = match &state.budget {
            Some((limit, curr)) => (true, Some(*limit), curr.clone()),
            None => (false, None, "NATIVE".to_owned()),
        };

        let available = budget_limit.map(|limit| limit.saturating_sub(total_spent));

        Ok(BalanceSummary {
            available,
            currency,
            total_spent,
            budget_configured,
            budget_limit,
        })
    }
}

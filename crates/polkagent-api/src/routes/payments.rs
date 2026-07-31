//! Payment tracking and budget endpoints.
//!
//! | Method | Path | Handler |
//! |---|---|---|
//! | `GET` | `/payments/balance` | [`get_balance`] |
//! | `GET` | `/payments/usage` | [`get_usage`] |
//! | `GET` | `/payments/receipts` | [`list_receipts`] |
//! | `GET` | `/payments/receipts/:receipt_id` | [`get_receipt`] |
//!
//! All endpoints return 501 Not Implemented when no [`PaymentStore`] has been
//! configured in [`AppState`].

use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use tracing::instrument;

use crate::{
    dto::{
        ListPaymentReceiptsResponse, PaymentBalanceResponse, PaymentReceiptResponse,
        PaymentUsageResponse, API_VERSION,
    },
    error::ApiError,
    state::AppState,
};

// ---------------------------------------------------------------------------
// GET /payments/balance
// ---------------------------------------------------------------------------

/// Get the current agent budget and balance summary.
///
/// Returns 501 Not Implemented when no payment store is configured.
pub async fn get_balance(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let _store = state
        .payment_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("payment store not configured".to_owned()))?;

    // Placeholder: when a real payment store is wired in, query it for
    // the agent's current balance and budget configuration.
    Ok(Json(PaymentBalanceResponse {
        version: API_VERSION.to_owned(),
        balance: serde_json::json!({
            "available": null,
            "currency": "NATIVE",
            "note": "balance tracking not yet implemented"
        }),
        budget: serde_json::json!({
            "configured": false
        }),
    }))
}

// ---------------------------------------------------------------------------
// GET /payments/usage
// ---------------------------------------------------------------------------

/// Get aggregated usage statistics (token counts, estimated cost).
///
/// Returns 501 Not Implemented when no payment store is configured.
pub async fn get_usage(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state
        .payment_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("payment store not configured".to_owned()))?;

    // Query usage over the last 30 days as a default window.
    let until = Utc::now();
    let since = until - chrono::Duration::days(30);

    let summary = store
        .get_usage("", since, until)
        .await
        .map_err(|e| ApiError::InternalError(e.to_string()))?;

    Ok(Json(PaymentUsageResponse {
        version: API_VERSION.to_owned(),
        total_runs: summary.total_runs,
        total_tokens: summary.total_tokens,
        estimated_usd: summary.estimated_usd,
        period_start: summary.period_start.to_rfc3339(),
        period_end: summary.period_end.to_rfc3339(),
    }))
}

// ---------------------------------------------------------------------------
// GET /payments/receipts
// ---------------------------------------------------------------------------

/// List all payment receipts.
///
/// Returns 501 Not Implemented when no payment store is configured.
pub async fn list_receipts(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let _store = state
        .payment_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("payment store not configured".to_owned()))?;

    // Placeholder: the PaymentStore trait does not yet expose a list-receipts
    // method. When that is added, delegate here.
    Ok(Json(ListPaymentReceiptsResponse {
        version: API_VERSION.to_owned(),
        data: vec![],
    }))
}

// ---------------------------------------------------------------------------
// GET /payments/receipts/:receipt_id
// ---------------------------------------------------------------------------

/// Get a single payment receipt by its intent ID.
///
/// Returns 501 Not Implemented when no payment store is configured.
#[instrument(skip(state), fields(receipt_id = %receipt_id))]
pub async fn get_receipt(
    State(state): State<AppState>,
    Path(receipt_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let _store = state
        .payment_store
        .as_ref()
        .ok_or_else(|| ApiError::NotImplemented("payment store not configured".to_owned()))?;

    // Placeholder: the PaymentStore trait exposes get_intent but not
    // get_receipt by ID yet. Return 404 until receipt lookup is implemented.
    Err::<Json<PaymentReceiptResponse>, _>(ApiError::NotFound(format!(
        "receipt '{receipt_id}'"
    )))
}

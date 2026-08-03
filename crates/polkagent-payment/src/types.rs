//! Payment domain types: assets, amounts, intents, receipts, and cost records.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::PaymentError;

// ---------------------------------------------------------------------------
// AssetId
// ---------------------------------------------------------------------------

/// Identifies a blockchain asset — either the native token or a registered
/// on-chain token.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssetId {
    /// The chain's native gas/staking token (e.g. DOT, KSM).
    Native,
    /// A registered on-chain token (e.g. USDT on Asset Hub).
    Token {
        /// The chain identifier (e.g. "polkadot", "kusama").
        chain: String,
        /// Human-readable ticker symbol (e.g. "USDT").
        symbol: String,
        /// The number of decimal places used by this token.
        decimals: u8,
    },
}

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Native => write!(f, "NATIVE"),
            Self::Token { symbol, .. } => write!(f, "{symbol}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Amount
// ---------------------------------------------------------------------------

/// A precise on-chain amount, stored in the smallest indivisible unit
/// (planck / smallest denomination).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Amount {
    /// The raw integer value in the smallest denomination (e.g. planck).
    pub value: u128,
    /// Which asset this amount represents.
    pub asset: AssetId,
    /// Number of decimal places for human-readable display.
    pub decimals: u8,
}

impl Amount {
    /// Create a new amount.
    #[must_use]
    pub fn new(value: u128, asset: AssetId, decimals: u8) -> Self {
        Self {
            value,
            asset,
            decimals,
        }
    }

    /// Create a zero amount for the given asset.
    #[must_use]
    pub fn zero(asset: AssetId, decimals: u8) -> Self {
        Self {
            value: 0,
            asset,
            decimals,
        }
    }

    /// Format the amount with proper decimal places for human display.
    ///
    /// Examples: `"1.500000000000 NATIVE"`, `"0.001000 USDT"`.
    #[must_use]
    pub fn display_human(&self) -> String {
        let divisor = 10u128.pow(u32::from(self.decimals));
        let whole = self.value / divisor;
        let frac = self.value % divisor;

        let frac_str = format!("{frac:0>width$}", width = self.decimals as usize);
        format!("{whole}.{frac_str} {}", self.asset)
    }

    /// Return the raw planck value (smallest indivisible unit).
    #[must_use]
    pub fn to_planck(&self) -> u128 {
        self.value
    }

    /// Checked addition. Returns `None` if the assets do not match or
    /// arithmetic overflows.
    pub fn checked_add(&self, other: &Self) -> Result<Self, PaymentError> {
        if self.asset != other.asset {
            return Err(PaymentError::AssetMismatch {
                left: self.asset.to_string(),
                right: other.asset.to_string(),
            });
        }
        self.value
            .checked_add(other.value)
            .map(|v| Self {
                value: v,
                asset: self.asset.clone(),
                decimals: self.decimals,
            })
            .ok_or(PaymentError::ArithmeticOverflow {
                context: "checked_add overflow".into(),
            })
    }

    /// Checked subtraction. Returns `None` if the assets do not match or
    /// the result would underflow.
    pub fn checked_sub(&self, other: &Self) -> Result<Self, PaymentError> {
        if self.asset != other.asset {
            return Err(PaymentError::AssetMismatch {
                left: self.asset.to_string(),
                right: other.asset.to_string(),
            });
        }
        self.value
            .checked_sub(other.value)
            .map(|v| Self {
                value: v,
                asset: self.asset.clone(),
                decimals: self.decimals,
            })
            .ok_or(PaymentError::ArithmeticOverflow {
                context: "checked_sub underflow".into(),
            })
    }

    /// Checked multiplication by a scalar. Returns an error on overflow.
    pub fn checked_mul(&self, scalar: u128) -> Result<Self, PaymentError> {
        self.value
            .checked_mul(scalar)
            .map(|v| Self {
                value: v,
                asset: self.asset.clone(),
                decimals: self.decimals,
            })
            .ok_or(PaymentError::ArithmeticOverflow {
                context: "checked_mul overflow".into(),
            })
    }
}

impl std::fmt::Display for Amount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_human())
    }
}

// ---------------------------------------------------------------------------
// PaymentStatus
// ---------------------------------------------------------------------------

/// Lifecycle states for a [`PaymentIntent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    /// Intent created, awaiting approval.
    Pending,
    /// Approved by the budget / policy layer.
    Approved,
    /// Transaction submitted to the chain.
    Submitted,
    /// Transaction confirmed on-chain.
    Confirmed,
    /// The payment failed (chain rejection, timeout, etc.).
    Failed,
    /// Cancelled before submission.
    Cancelled,
}

impl std::fmt::Display for PaymentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Submitted => "submitted",
            Self::Confirmed => "confirmed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// PaymentIntent
// ---------------------------------------------------------------------------

/// A request to make a payment, subject to budget checks and approval
/// before chain submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentIntent {
    /// Unique identifier for this intent.
    pub id: Uuid,
    /// The agent requesting the payment.
    pub agent_id: String,
    /// The run that originated this payment request.
    pub run_id: String,
    /// The amount to transfer.
    pub amount: Amount,
    /// The recipient address (chain-specific encoding).
    pub recipient: String,
    /// Client-provided key for at-most-once delivery.
    pub idempotency_key: String,
    /// When this intent was created.
    pub created_at: DateTime<Utc>,
    /// Current lifecycle status.
    pub status: PaymentStatus,
    /// Optional structured metadata (e.g. batch call details for risk
    /// analysis).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// PaymentReceipt
// ---------------------------------------------------------------------------

/// Proof that a [`PaymentIntent`] was settled on-chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentReceipt {
    /// The intent that was fulfilled.
    pub intent_id: Uuid,
    /// The on-chain transaction hash.
    pub tx_hash: String,
    /// The block number in which the transaction was included.
    pub block_number: u64,
    /// The transaction fee paid (in chain-native units).
    pub fee_paid: Amount,
    /// When the confirmation was observed.
    pub confirmed_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// CostRecord
// ---------------------------------------------------------------------------

/// A record of LLM usage cost incurred during a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostRecord {
    /// The run that incurred this cost.
    pub run_id: String,
    /// LLM provider name (e.g. "anthropic", "openai", "local").
    pub provider: String,
    /// Model identifier (e.g. "claude-sonnet-4").
    pub model: String,
    /// Number of input (prompt) tokens consumed.
    pub input_tokens: u64,
    /// Number of output (completion) tokens consumed.
    pub output_tokens: u64,
    /// Estimated cost in US dollars.
    pub estimated_usd: f64,
    /// When this cost was recorded.
    pub recorded_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// UsageSummary
// ---------------------------------------------------------------------------

/// Aggregated usage statistics over a time period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    /// Total number of runs in this period.
    pub total_runs: u64,
    /// Total tokens (input + output) consumed.
    pub total_tokens: u64,
    /// Total estimated USD cost.
    pub estimated_usd: f64,
    /// Start of the reporting period (inclusive).
    pub period_start: DateTime<Utc>,
    /// End of the reporting period (exclusive).
    pub period_end: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn dot_asset() -> AssetId {
        AssetId::Native
    }

    fn usdt_asset() -> AssetId {
        AssetId::Token {
            chain: "polkadot-asset-hub".into(),
            symbol: "USDT".into(),
            decimals: 6,
        }
    }

    // --- Amount arithmetic ---

    #[test]
    fn amount_display_human_native() {
        // 1.5 DOT = 15_000_000_000 planck (10 decimals)
        let amt = Amount::new(15_000_000_000, dot_asset(), 10);
        assert_eq!(amt.display_human(), "1.5000000000 NATIVE");
    }

    #[test]
    fn amount_display_human_token() {
        // 1.5 USDT = 1_500_000 (6 decimals)
        let amt = Amount::new(1_500_000, usdt_asset(), 6);
        assert_eq!(amt.display_human(), "1.500000 USDT");
    }

    #[test]
    fn amount_display_human_zero() {
        let amt = Amount::zero(dot_asset(), 10);
        assert_eq!(amt.display_human(), "0.0000000000 NATIVE");
    }

    #[test]
    fn amount_to_planck() {
        let amt = Amount::new(42, dot_asset(), 10);
        assert_eq!(amt.to_planck(), 42);
    }

    #[test]
    fn checked_add_same_asset() {
        let a = Amount::new(100, dot_asset(), 10);
        let b = Amount::new(200, dot_asset(), 10);
        let result = a.checked_add(&b).expect("should succeed");
        assert_eq!(result.value, 300);
        assert_eq!(result.asset, dot_asset());
    }

    #[test]
    fn checked_add_different_asset_fails() {
        let a = Amount::new(100, dot_asset(), 10);
        let b = Amount::new(200, usdt_asset(), 6);
        let err = a.checked_add(&b).unwrap_err();
        assert!(matches!(err, PaymentError::AssetMismatch { .. }));
    }

    #[test]
    fn checked_add_overflow_fails() {
        let a = Amount::new(u128::MAX, dot_asset(), 10);
        let b = Amount::new(1, dot_asset(), 10);
        let err = a.checked_add(&b).unwrap_err();
        assert!(matches!(err, PaymentError::ArithmeticOverflow { .. }));
    }

    #[test]
    fn checked_sub_same_asset() {
        let a = Amount::new(300, dot_asset(), 10);
        let b = Amount::new(100, dot_asset(), 10);
        let result = a.checked_sub(&b).expect("should succeed");
        assert_eq!(result.value, 200);
    }

    #[test]
    fn checked_sub_underflow_fails() {
        let a = Amount::new(100, dot_asset(), 10);
        let b = Amount::new(200, dot_asset(), 10);
        let err = a.checked_sub(&b).unwrap_err();
        assert!(matches!(err, PaymentError::ArithmeticOverflow { .. }));
    }

    #[test]
    fn checked_sub_different_asset_fails() {
        let a = Amount::new(100, dot_asset(), 10);
        let b = Amount::new(50, usdt_asset(), 6);
        let err = a.checked_sub(&b).unwrap_err();
        assert!(matches!(err, PaymentError::AssetMismatch { .. }));
    }

    #[test]
    fn checked_mul_scalar() {
        let a = Amount::new(100, dot_asset(), 10);
        let result = a.checked_mul(3).expect("should succeed");
        assert_eq!(result.value, 300);
    }

    #[test]
    fn checked_mul_overflow_fails() {
        let a = Amount::new(u128::MAX, dot_asset(), 10);
        let err = a.checked_mul(2).unwrap_err();
        assert!(matches!(err, PaymentError::ArithmeticOverflow { .. }));
    }

    #[test]
    fn checked_mul_by_zero() {
        let a = Amount::new(1_000_000, dot_asset(), 10);
        let result = a.checked_mul(0).expect("should succeed");
        assert_eq!(result.value, 0);
    }

    // --- PaymentStatus ---

    #[test]
    fn payment_status_display() {
        assert_eq!(PaymentStatus::Pending.to_string(), "pending");
        assert_eq!(PaymentStatus::Confirmed.to_string(), "confirmed");
        assert_eq!(PaymentStatus::Failed.to_string(), "failed");
    }

    // --- Serde round-trip ---

    #[test]
    fn asset_id_native_serde() {
        let asset = AssetId::Native;
        let json = serde_json::to_string(&asset).expect("serialize");
        let back: AssetId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(asset, back);
    }

    #[test]
    fn asset_id_token_serde() {
        let asset = usdt_asset();
        let json = serde_json::to_string(&asset).expect("serialize");
        let back: AssetId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(asset, back);
    }

    #[test]
    fn amount_serde_round_trip() {
        let amt = Amount::new(42_000_000_000, dot_asset(), 10);
        let json = serde_json::to_string(&amt).expect("serialize");
        let back: Amount = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(amt, back);
    }

    #[test]
    fn payment_status_serde_round_trip() {
        let status = PaymentStatus::Submitted;
        let json = serde_json::to_string(&status).expect("serialize");
        let back: PaymentStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(status, back);
    }

    #[test]
    fn cost_record_serde_round_trip() {
        let record = CostRecord {
            run_id: "run-1".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4".into(),
            input_tokens: 1000,
            output_tokens: 500,
            estimated_usd: 0.0105,
            recorded_at: Utc::now(),
        };
        let json = serde_json::to_string(&record).expect("serialize");
        let back: CostRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(record.run_id, back.run_id);
        assert_eq!(record.provider, back.provider);
    }

    // --- Amount::zero ---

    #[test]
    fn amount_zero_has_correct_asset() {
        let z = Amount::zero(usdt_asset(), 6);
        assert_eq!(z.value, 0);
        assert_eq!(z.asset, usdt_asset());
        assert_eq!(z.decimals, 6);
    }
}

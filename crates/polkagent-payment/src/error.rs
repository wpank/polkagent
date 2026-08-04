//! Error types for the payment and budget system.

use thiserror::Error;
use uuid::Uuid;

/// Errors that can occur during payment, budget, and cost-estimation
/// operations.
#[derive(Debug, Error)]
pub enum PaymentError {
    // --- Store errors ---
    /// A payment store operation failed.
    #[error("store error: {0}")]
    Store(String),

    // --- Budget errors ---
    /// A proposed spend was denied by the budget checker.
    #[error("budget denied for agent '{agent_id}': {reason}")]
    BudgetDenied {
        /// The agent whose budget was exceeded.
        agent_id: String,
        /// Human-readable reason for the denial.
        reason: String,
    },

    /// The budget configuration is invalid.
    #[error("invalid budget configuration: {0}")]
    InvalidBudgetConfig(String),

    // --- Payment intent errors ---
    /// A payment intent was not found.
    #[error("payment intent not found: {id}")]
    IntentNotFound {
        /// The UUID of the missing intent.
        id: Uuid,
    },

    /// A payment intent status transition is not permitted.
    #[error("invalid status transition from {from} to {to}")]
    InvalidStatusTransition {
        /// The current status.
        from: String,
        /// The requested status.
        to: String,
    },

    /// An idempotency conflict: a payment with the same key already exists.
    #[error("idempotency conflict for key '{key}'")]
    IdempotencyConflict {
        /// The conflicting idempotency key.
        key: String,
    },

    // --- Estimation errors ---
    /// No pricing data is available for the requested model.
    #[error("no pricing data for provider '{provider}', model '{model}'")]
    NoPricingData {
        /// The LLM provider name.
        provider: String,
        /// The model identifier.
        model: String,
    },

    // --- Ledger errors (EXPERIMENTAL — PRD-08 §5.2 prototype) ---
    /// The agent's ledger balance is too low for the requested spend.
    #[error("insufficient balance: have {available_planck} planck, need {required_planck}")]
    InsufficientBalance {
        /// The current balance in planck.
        available_planck: u128,
        /// The amount the operation requires.
        required_planck: u128,
    },

    // --- Arithmetic errors ---
    /// An arithmetic overflow occurred during amount calculation.
    #[error("arithmetic overflow: {context}")]
    ArithmeticOverflow {
        /// What operation overflowed.
        context: String,
    },

    /// The assets in an arithmetic operation do not match.
    #[error("asset mismatch: cannot combine {left} with {right}")]
    AssetMismatch {
        /// Display name of the left-hand operand's asset.
        left: String,
        /// Display name of the right-hand operand's asset.
        right: String,
    },

    // --- Validation errors ---
    /// A domain value failed validation.
    #[error("validation failed for field '{field}': {message}")]
    Validation {
        /// Which field or constraint failed.
        field: String,
        /// Human-readable description of the failure.
        message: String,
    },

    // --- Serialization errors ---
    /// A serialization or deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl PaymentError {
    /// Construct a `Store` error from any displayable value.
    pub fn store(msg: impl std::fmt::Display) -> Self {
        Self::Store(msg.to_string())
    }

    /// Construct a `Validation` error.
    pub fn validation(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation {
            field: field.into(),
            message: message.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_error_display() {
        let err = PaymentError::store("connection refused");
        assert!(err.to_string().contains("connection refused"));
    }

    #[test]
    fn budget_denied_display() {
        let err = PaymentError::BudgetDenied {
            agent_id: "agent-1".into(),
            reason: "daily limit exceeded".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("agent-1"));
        assert!(msg.contains("daily limit exceeded"));
    }

    #[test]
    fn intent_not_found_display() {
        let id = Uuid::now_v7();
        let err = PaymentError::IntentNotFound { id };
        assert!(err.to_string().contains(&id.to_string()));
    }

    #[test]
    fn validation_error_constructor() {
        let err = PaymentError::validation("amount", "must be positive");
        let msg = err.to_string();
        assert!(msg.contains("amount"));
        assert!(msg.contains("must be positive"));
    }

    #[test]
    fn asset_mismatch_display() {
        let err = PaymentError::AssetMismatch {
            left: "DOT".into(),
            right: "KSM".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("DOT"));
        assert!(msg.contains("KSM"));
    }

    #[test]
    fn arithmetic_overflow_display() {
        let err = PaymentError::ArithmeticOverflow {
            context: "checked_add".into(),
        };
        assert!(err.to_string().contains("checked_add"));
    }
}

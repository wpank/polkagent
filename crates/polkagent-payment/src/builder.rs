//! Fluent builder for [`PaymentIntent`](crate::types::PaymentIntent).

use chrono::Utc;
use uuid::Uuid;

use crate::action::PaymentAction;
use crate::error::PaymentError;
use crate::intent::IntentStateMachine;
use crate::types::{Amount, PaymentIntent, PaymentStatus};

/// A built intent combining the original [`PaymentIntent`] with its
/// [`PaymentAction`] and [`IntentStateMachine`].
#[derive(Debug, Clone)]
pub struct BuiltIntent {
    /// The payment intent record.
    pub intent: PaymentIntent,
    /// The kind of on-chain operation.
    pub action: PaymentAction,
    /// The lifecycle state machine.
    pub state_machine: IntentStateMachine,
}

/// Fluent builder for constructing a [`PaymentIntent`] together with its
/// action type and lifecycle state machine.
///
/// # Example
///
/// ```
/// use polkagent_payment::builder::PaymentIntentBuilder;
/// use polkagent_payment::action::PaymentAction;
/// use polkagent_payment::types::{Amount, AssetId};
///
/// let built = PaymentIntentBuilder::new()
///     .agent_id("agent-1")
///     .run_id("run-42")
///     .action(PaymentAction::NativeTransfer)
///     .amount(Amount::new(1_000_000, AssetId::Native, 10))
///     .recipient("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY")
///     .idempotency_key("transfer-001")
///     .build()
///     .expect("should build successfully");
///
/// assert_eq!(built.intent.agent_id, "agent-1");
/// ```
#[derive(Debug, Default)]
pub struct PaymentIntentBuilder {
    agent_id: Option<String>,
    run_id: Option<String>,
    action: Option<PaymentAction>,
    amount: Option<Amount>,
    recipient: Option<String>,
    idempotency_key: Option<String>,
}

impl PaymentIntentBuilder {
    /// Create a new builder with all fields unset.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the agent identifier.
    #[must_use]
    pub fn agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    /// Set the run identifier.
    #[must_use]
    pub fn run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    /// Set the payment action type.
    #[must_use]
    pub fn action(mut self, action: PaymentAction) -> Self {
        self.action = Some(action);
        self
    }

    /// Set the transfer amount.
    #[must_use]
    pub fn amount(mut self, amount: Amount) -> Self {
        self.amount = Some(amount);
        self
    }

    /// Set the recipient address.
    #[must_use]
    pub fn recipient(mut self, recipient: impl Into<String>) -> Self {
        self.recipient = Some(recipient.into());
        self
    }

    /// Set the idempotency key for at-most-once delivery.
    #[must_use]
    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    /// Consume the builder and produce a [`BuiltIntent`].
    ///
    /// Returns a validation error if any required field is missing.
    pub fn build(self) -> Result<BuiltIntent, PaymentError> {
        let agent_id = self
            .agent_id
            .ok_or_else(|| PaymentError::validation("agent_id", "agent_id is required"))?;
        let run_id = self
            .run_id
            .ok_or_else(|| PaymentError::validation("run_id", "run_id is required"))?;
        let action = self
            .action
            .ok_or_else(|| PaymentError::validation("action", "action is required"))?;
        let amount = self
            .amount
            .ok_or_else(|| PaymentError::validation("amount", "amount is required"))?;
        let recipient = self
            .recipient
            .ok_or_else(|| PaymentError::validation("recipient", "recipient is required"))?;
        let idempotency_key = self.idempotency_key.ok_or_else(|| {
            PaymentError::validation("idempotency_key", "idempotency_key is required")
        })?;

        if recipient.is_empty() {
            return Err(PaymentError::validation(
                "recipient",
                "recipient must not be empty",
            ));
        }

        if amount.value == 0 {
            return Err(PaymentError::validation(
                "amount",
                "amount must be greater than zero",
            ));
        }

        let intent = PaymentIntent {
            id: Uuid::now_v7(),
            agent_id,
            run_id,
            amount,
            recipient,
            idempotency_key,
            created_at: Utc::now(),
            status: PaymentStatus::Pending,
            metadata: None,
        };

        Ok(BuiltIntent {
            intent,
            action,
            state_machine: IntentStateMachine::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::IntentStatus;
    use crate::types::AssetId;

    fn dot(planck: u128) -> Amount {
        Amount::new(planck, AssetId::Native, 10)
    }

    fn valid_builder() -> PaymentIntentBuilder {
        PaymentIntentBuilder::new()
            .agent_id("agent-1")
            .run_id("run-42")
            .action(PaymentAction::NativeTransfer)
            .amount(dot(1_000_000_000_000))
            .recipient("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY")
            .idempotency_key("key-001")
    }

    #[test]
    fn build_success() {
        let built = valid_builder().build().expect("should build");
        assert_eq!(built.intent.agent_id, "agent-1");
        assert_eq!(built.intent.run_id, "run-42");
        assert_eq!(built.action, PaymentAction::NativeTransfer);
        assert_eq!(built.intent.status, PaymentStatus::Pending);
        assert_eq!(built.state_machine.current_status(), IntentStatus::Drafting);
    }

    #[test]
    fn build_missing_agent_id() {
        let result = PaymentIntentBuilder::new()
            .run_id("run-42")
            .action(PaymentAction::NativeTransfer)
            .amount(dot(1_000))
            .recipient("addr")
            .idempotency_key("key")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_missing_run_id() {
        let result = PaymentIntentBuilder::new()
            .agent_id("agent")
            .action(PaymentAction::NativeTransfer)
            .amount(dot(1_000))
            .recipient("addr")
            .idempotency_key("key")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_missing_action() {
        let result = PaymentIntentBuilder::new()
            .agent_id("agent")
            .run_id("run")
            .amount(dot(1_000))
            .recipient("addr")
            .idempotency_key("key")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_missing_amount() {
        let result = PaymentIntentBuilder::new()
            .agent_id("agent")
            .run_id("run")
            .action(PaymentAction::NativeTransfer)
            .recipient("addr")
            .idempotency_key("key")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_missing_recipient() {
        let result = PaymentIntentBuilder::new()
            .agent_id("agent")
            .run_id("run")
            .action(PaymentAction::NativeTransfer)
            .amount(dot(1_000))
            .idempotency_key("key")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_missing_idempotency_key() {
        let result = PaymentIntentBuilder::new()
            .agent_id("agent")
            .run_id("run")
            .action(PaymentAction::NativeTransfer)
            .amount(dot(1_000))
            .recipient("addr")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_empty_recipient_rejected() {
        let result = PaymentIntentBuilder::new()
            .agent_id("agent")
            .run_id("run")
            .action(PaymentAction::NativeTransfer)
            .amount(dot(1_000))
            .recipient("")
            .idempotency_key("key")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn build_zero_amount_rejected() {
        let result = PaymentIntentBuilder::new()
            .agent_id("agent")
            .run_id("run")
            .action(PaymentAction::NativeTransfer)
            .amount(dot(0))
            .recipient("addr")
            .idempotency_key("key")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn builder_is_fluent() {
        // Verify that chaining works and each method returns Self.
        let _builder = PaymentIntentBuilder::new()
            .agent_id("a")
            .run_id("r")
            .action(PaymentAction::Batch)
            .amount(dot(1))
            .recipient("addr")
            .idempotency_key("k");
    }

    #[test]
    fn built_intent_has_unique_id() {
        let a = valid_builder().build().expect("build");
        let b = valid_builder().build().expect("build");
        assert_ne!(a.intent.id, b.intent.id);
    }

    #[test]
    fn state_machine_starts_drafting() {
        let built = valid_builder().build().expect("build");
        assert_eq!(built.state_machine.current_status(), IntentStatus::Drafting);
        assert!(!built.state_machine.is_terminal());
    }

    #[test]
    fn cross_chain_action() {
        let built = PaymentIntentBuilder::new()
            .agent_id("agent")
            .run_id("run")
            .action(PaymentAction::CrossChain)
            .amount(dot(5_000))
            .recipient("addr")
            .idempotency_key("xcm-001")
            .build()
            .expect("build");
        assert_eq!(built.action, PaymentAction::CrossChain);
    }
}

//! Pre-flight validation checks for payment intents.
//!
//! Before a payment intent is submitted on-chain, a series of checks verify
//! that the transaction is likely to succeed: balance sufficiency, fee
//! coverage, existential deposit rules, address validity, nonce freshness,
//! and metadata staleness.

use serde::{Deserialize, Serialize};

use crate::error::PaymentError;
use crate::types::PaymentIntent;

// ---------------------------------------------------------------------------
// PreFlightWarning
// ---------------------------------------------------------------------------

/// A non-blocking warning discovered during pre-flight checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreFlightWarning {
    /// Machine-readable warning code (e.g. `"low_balance"`, `"stale_metadata"`).
    pub code: String,
    /// Human-readable description.
    pub message: String,
    /// Severity level.
    pub severity: WarningSeverity,
}

/// Severity of a pre-flight warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningSeverity {
    /// Informational — not expected to cause problems.
    Info,
    /// The transaction may succeed but the situation is unusual.
    Low,
    /// The transaction might fail under some conditions.
    Medium,
    /// The transaction is very likely to fail unless the user intervenes.
    High,
}

impl std::fmt::Display for WarningSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// PreFlightBlocker
// ---------------------------------------------------------------------------

/// A blocking issue that prevents the payment from being submitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreFlightBlocker {
    /// Machine-readable blocker code (e.g. `"insufficient_balance"`,
    /// `"invalid_address"`).
    pub code: String,
    /// Human-readable description.
    pub message: String,
}

impl std::fmt::Display for PreFlightBlocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

// ---------------------------------------------------------------------------
// PreFlightResult
// ---------------------------------------------------------------------------

/// The aggregated result of running one or more pre-flight checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreFlightResult {
    /// `true` if all checks passed without blockers.
    pub passed: bool,
    /// Non-blocking warnings discovered during checks.
    pub warnings: Vec<PreFlightWarning>,
    /// Blocking issues that must be resolved before submission.
    pub blockers: Vec<PreFlightBlocker>,
}

impl PreFlightResult {
    /// Create a passing result with no warnings or blockers.
    #[must_use]
    pub fn pass() -> Self {
        Self {
            passed: true,
            warnings: Vec::new(),
            blockers: Vec::new(),
        }
    }

    /// Create a failing result with a single blocker.
    #[must_use]
    pub fn fail(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            passed: false,
            warnings: Vec::new(),
            blockers: vec![PreFlightBlocker {
                code: code.into(),
                message: message.into(),
            }],
        }
    }

    /// Merge another result into this one. Blockers from `other` are
    /// appended and `passed` is set to `false` if either result has
    /// blockers.
    pub fn merge(&mut self, other: PreFlightResult) {
        self.warnings.extend(other.warnings);
        self.blockers.extend(other.blockers);
        if !self.blockers.is_empty() {
            self.passed = false;
        }
    }
}

// ---------------------------------------------------------------------------
// PreFlightCheck trait
// ---------------------------------------------------------------------------

/// A single pre-flight validation check.
///
/// Implementations inspect a [`PaymentIntent`] and return a
/// [`PreFlightResult`] indicating whether the payment can proceed.
#[async_trait::async_trait]
pub trait PreFlightCheck: Send + Sync {
    /// Run this check against the given intent.
    async fn check(&self, intent: &PaymentIntent) -> Result<PreFlightResult, PaymentError>;
}

// ---------------------------------------------------------------------------
// BalanceCheck
// ---------------------------------------------------------------------------

/// Verifies that the sender has sufficient free balance for the transfer.
pub struct BalanceCheck {
    /// The sender's available balance in planck.
    available_balance: u128,
}

impl BalanceCheck {
    /// Create a new balance check with the given available balance.
    #[must_use]
    pub fn new(available_balance: u128) -> Self {
        Self { available_balance }
    }
}

#[async_trait::async_trait]
impl PreFlightCheck for BalanceCheck {
    async fn check(&self, intent: &PaymentIntent) -> Result<PreFlightResult, PaymentError> {
        if self.available_balance < intent.amount.value {
            Ok(PreFlightResult::fail(
                "insufficient_balance",
                format!(
                    "available balance {} is less than transfer amount {}",
                    self.available_balance, intent.amount.value
                ),
            ))
        } else {
            Ok(PreFlightResult::pass())
        }
    }
}

// ---------------------------------------------------------------------------
// FeeCheck
// ---------------------------------------------------------------------------

/// Verifies that the sender can cover the estimated transaction fee in
/// addition to the transfer amount.
pub struct FeeCheck {
    /// The sender's available balance in planck.
    available_balance: u128,
    /// The estimated fee in planck.
    estimated_fee: u128,
}

impl FeeCheck {
    /// Create a new fee check.
    #[must_use]
    pub fn new(available_balance: u128, estimated_fee: u128) -> Self {
        Self {
            available_balance,
            estimated_fee,
        }
    }
}

#[async_trait::async_trait]
impl PreFlightCheck for FeeCheck {
    async fn check(&self, intent: &PaymentIntent) -> Result<PreFlightResult, PaymentError> {
        let total_needed = intent.amount.value.checked_add(self.estimated_fee).ok_or(
            PaymentError::ArithmeticOverflow {
                context: "fee check: amount + fee overflow".into(),
            },
        )?;

        if self.available_balance < total_needed {
            Ok(PreFlightResult::fail(
                "insufficient_for_fee",
                format!(
                    "available balance {} cannot cover amount {} + fee {}",
                    self.available_balance, intent.amount.value, self.estimated_fee
                ),
            ))
        } else {
            Ok(PreFlightResult::pass())
        }
    }
}

// ---------------------------------------------------------------------------
// ExistentialDepositCheck
// ---------------------------------------------------------------------------

/// Verifies that the transfer will not kill the sender's or recipient's
/// account by dropping below the existential deposit.
pub struct ExistentialDepositCheck {
    /// The chain's existential deposit in planck.
    existential_deposit: u128,
    /// The sender's balance after the transfer (in planck). This should
    /// account for the transfer amount and estimated fee.
    sender_remaining: u128,
    /// The recipient's current balance in planck.
    recipient_balance: u128,
}

impl ExistentialDepositCheck {
    /// Create a new existential deposit check.
    #[must_use]
    pub fn new(existential_deposit: u128, sender_remaining: u128, recipient_balance: u128) -> Self {
        Self {
            existential_deposit,
            sender_remaining,
            recipient_balance,
        }
    }
}

#[async_trait::async_trait]
impl PreFlightCheck for ExistentialDepositCheck {
    async fn check(&self, intent: &PaymentIntent) -> Result<PreFlightResult, PaymentError> {
        let mut result = PreFlightResult::pass();

        // Check sender won't be reaped.
        if self.sender_remaining > 0 && self.sender_remaining < self.existential_deposit {
            result.blockers.push(PreFlightBlocker {
                code: "sender_below_ed".into(),
                message: format!(
                    "sender remaining balance {} would fall below existential deposit {}",
                    self.sender_remaining, self.existential_deposit
                ),
            });
        }

        // Check recipient won't receive dust (below ED with no prior balance).
        let recipient_after = self
            .recipient_balance
            .checked_add(intent.amount.value)
            .ok_or(PaymentError::ArithmeticOverflow {
                context: "existential deposit check: recipient balance overflow".into(),
            })?;
        if recipient_after < self.existential_deposit {
            result.blockers.push(PreFlightBlocker {
                code: "recipient_below_ed".into(),
                message: format!(
                    "recipient balance after transfer {} would be below existential deposit {}",
                    recipient_after, self.existential_deposit
                ),
            });
        }

        if !result.blockers.is_empty() {
            result.passed = false;
        }
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// AddressCheck
// ---------------------------------------------------------------------------

/// Verifies that the recipient address is a valid SS58 address for the
/// target network.
///
/// This performs a basic structural check: valid Base58 characters and
/// minimum length. Full SS58 decoding with checksum verification would
/// require a dedicated SS58 codec.
pub struct AddressCheck {
    /// Expected SS58 address prefix for the target network (e.g. 0 for
    /// Polkadot, 2 for Kusama).
    expected_prefix: Option<u16>,
}

impl AddressCheck {
    /// Create an address check without prefix validation.
    #[must_use]
    pub fn new() -> Self {
        Self {
            expected_prefix: None,
        }
    }

    /// Create an address check that also validates the SS58 network prefix.
    #[must_use]
    pub fn with_prefix(prefix: u16) -> Self {
        Self {
            expected_prefix: Some(prefix),
        }
    }
}

impl Default for AddressCheck {
    fn default() -> Self {
        Self::new()
    }
}

/// Characters valid in Base58 encoding (no 0, O, I, l).
const BASE58_CHARS: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

#[async_trait::async_trait]
impl PreFlightCheck for AddressCheck {
    async fn check(&self, intent: &PaymentIntent) -> Result<PreFlightResult, PaymentError> {
        let addr = &intent.recipient;

        if addr.is_empty() {
            return Ok(PreFlightResult::fail(
                "invalid_address",
                "recipient address is empty",
            ));
        }

        // SS58 addresses are at least 3 characters (prefix + payload + checksum).
        if addr.len() < 3 {
            return Ok(PreFlightResult::fail(
                "invalid_address",
                format!("address too short: {} characters", addr.len()),
            ));
        }

        // Check all characters are valid Base58.
        if let Some(pos) = addr.find(|c: char| !BASE58_CHARS.contains(c)) {
            return Ok(PreFlightResult::fail(
                "invalid_address",
                format!(
                    "invalid character at position {pos}: '{}'",
                    addr.chars().nth(pos).unwrap_or('?')
                ),
            ));
        }

        // If prefix validation is requested, return a warning (full SS58
        // decoding is beyond scope here).
        let mut result = PreFlightResult::pass();
        if let Some(_prefix) = self.expected_prefix {
            result.warnings.push(PreFlightWarning {
                code: "prefix_unchecked".into(),
                message: "SS58 prefix validation requires full codec; skipping".into(),
                severity: WarningSeverity::Info,
            });
        }

        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// NonceCheck
// ---------------------------------------------------------------------------

/// Verifies that the account nonce is fresh (matches expected value).
pub struct NonceCheck {
    /// The expected nonce (from the chain's account info).
    expected_nonce: u32,
    /// The nonce the transaction will use.
    tx_nonce: u32,
}

impl NonceCheck {
    /// Create a nonce check.
    #[must_use]
    pub fn new(expected_nonce: u32, tx_nonce: u32) -> Self {
        Self {
            expected_nonce,
            tx_nonce,
        }
    }
}

#[async_trait::async_trait]
impl PreFlightCheck for NonceCheck {
    async fn check(&self, _intent: &PaymentIntent) -> Result<PreFlightResult, PaymentError> {
        match self.tx_nonce.cmp(&self.expected_nonce) {
            std::cmp::Ordering::Less => Ok(PreFlightResult::fail(
                "stale_nonce",
                format!(
                    "transaction nonce {} is behind expected {}",
                    self.tx_nonce, self.expected_nonce
                ),
            )),
            std::cmp::Ordering::Equal => Ok(PreFlightResult::pass()),
            std::cmp::Ordering::Greater => {
                let mut result = PreFlightResult::pass();
                result.warnings.push(PreFlightWarning {
                    code: "future_nonce".into(),
                    message: format!(
                        "transaction nonce {} is ahead of expected {}; may wait in pool",
                        self.tx_nonce, self.expected_nonce
                    ),
                    severity: WarningSeverity::Medium,
                });
                Ok(result)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// MetadataFreshnessCheck
// ---------------------------------------------------------------------------

/// Verifies that the chain metadata used to construct the extrinsic is not
/// stale.
pub struct MetadataFreshnessCheck {
    /// The spec version of the metadata used to build the transaction.
    tx_spec_version: u32,
    /// The current spec version on-chain.
    chain_spec_version: u32,
}

impl MetadataFreshnessCheck {
    /// Create a metadata freshness check.
    #[must_use]
    pub fn new(tx_spec_version: u32, chain_spec_version: u32) -> Self {
        Self {
            tx_spec_version,
            chain_spec_version,
        }
    }
}

#[async_trait::async_trait]
impl PreFlightCheck for MetadataFreshnessCheck {
    async fn check(&self, _intent: &PaymentIntent) -> Result<PreFlightResult, PaymentError> {
        if self.tx_spec_version == self.chain_spec_version {
            Ok(PreFlightResult::pass())
        } else {
            Ok(PreFlightResult::fail(
                "stale_metadata",
                format!(
                    "transaction spec version {} does not match chain spec version {}",
                    self.tx_spec_version, self.chain_spec_version
                ),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// CompositePreFlight
// ---------------------------------------------------------------------------

/// Runs multiple [`PreFlightCheck`] implementations and merges the results.
pub struct CompositePreFlight {
    checks: Vec<Box<dyn PreFlightCheck>>,
}

impl CompositePreFlight {
    /// Create a composite with no checks.
    #[must_use]
    pub fn new() -> Self {
        Self { checks: Vec::new() }
    }

    /// Add a check to the composite.
    pub fn add_check(&mut self, check: impl PreFlightCheck + 'static) {
        self.checks.push(Box::new(check));
    }

    /// Consume and add a check (builder-style).
    #[must_use]
    pub fn with_check(mut self, check: impl PreFlightCheck + 'static) -> Self {
        self.checks.push(Box::new(check));
        self
    }

    /// Run all checks and merge results.
    pub async fn run(&self, intent: &PaymentIntent) -> Result<PreFlightResult, PaymentError> {
        let mut combined = PreFlightResult::pass();
        for check in &self.checks {
            let result = check.check(intent).await?;
            combined.merge(result);
        }
        Ok(combined)
    }

    /// Returns the number of registered checks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.checks.len()
    }

    /// Returns `true` if no checks are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.checks.is_empty()
    }
}

impl Default for CompositePreFlight {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::types::{Amount, AssetId, PaymentStatus};

    fn test_intent(planck: u128) -> PaymentIntent {
        PaymentIntent {
            id: Uuid::now_v7(),
            agent_id: "agent-1".into(),
            run_id: "run-1".into(),
            amount: Amount::new(planck, AssetId::Native, 10),
            recipient: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".into(),
            idempotency_key: "key-1".into(),
            created_at: Utc::now(),
            status: PaymentStatus::Pending,
            metadata: None,
        }
    }

    // --- PreFlightResult ---

    #[test]
    fn result_pass() {
        let r = PreFlightResult::pass();
        assert!(r.passed);
        assert!(r.warnings.is_empty());
        assert!(r.blockers.is_empty());
    }

    #[test]
    fn result_fail() {
        let r = PreFlightResult::fail("code", "msg");
        assert!(!r.passed);
        assert_eq!(r.blockers.len(), 1);
        assert_eq!(r.blockers[0].code, "code");
    }

    #[test]
    fn result_merge_two_passes() {
        let mut a = PreFlightResult::pass();
        let b = PreFlightResult::pass();
        a.merge(b);
        assert!(a.passed);
    }

    #[test]
    fn result_merge_pass_and_fail() {
        let mut a = PreFlightResult::pass();
        let b = PreFlightResult::fail("x", "y");
        a.merge(b);
        assert!(!a.passed);
        assert_eq!(a.blockers.len(), 1);
    }

    #[test]
    fn result_merge_accumulates() {
        let mut a = PreFlightResult::fail("a", "a_msg");
        let b = PreFlightResult::fail("b", "b_msg");
        a.merge(b);
        assert!(!a.passed);
        assert_eq!(a.blockers.len(), 2);
    }

    #[test]
    fn result_serde_round_trip() {
        let r = PreFlightResult::fail("test", "test msg");
        let json = serde_json::to_string(&r).expect("serialize");
        let back: PreFlightResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(r, back);
    }

    // --- WarningSeverity ---

    #[test]
    fn severity_display() {
        assert_eq!(WarningSeverity::Info.to_string(), "info");
        assert_eq!(WarningSeverity::Low.to_string(), "low");
        assert_eq!(WarningSeverity::Medium.to_string(), "medium");
        assert_eq!(WarningSeverity::High.to_string(), "high");
    }

    #[test]
    fn severity_serde_round_trip() {
        for sev in [
            WarningSeverity::Info,
            WarningSeverity::Low,
            WarningSeverity::Medium,
            WarningSeverity::High,
        ] {
            let json = serde_json::to_string(&sev).expect("serialize");
            let back: WarningSeverity = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(sev, back);
        }
    }

    // --- PreFlightBlocker ---

    #[test]
    fn blocker_display() {
        let b = PreFlightBlocker {
            code: "test".into(),
            message: "something".into(),
        };
        assert_eq!(b.to_string(), "[test] something");
    }

    // --- BalanceCheck ---

    #[tokio::test]
    async fn balance_check_sufficient() {
        let check = BalanceCheck::new(10_000);
        let intent = test_intent(5_000);
        let result = check.check(&intent).await.expect("check");
        assert!(result.passed);
    }

    #[tokio::test]
    async fn balance_check_exact() {
        let check = BalanceCheck::new(5_000);
        let intent = test_intent(5_000);
        let result = check.check(&intent).await.expect("check");
        assert!(result.passed);
    }

    #[tokio::test]
    async fn balance_check_insufficient() {
        let check = BalanceCheck::new(1_000);
        let intent = test_intent(5_000);
        let result = check.check(&intent).await.expect("check");
        assert!(!result.passed);
        assert_eq!(result.blockers[0].code, "insufficient_balance");
    }

    // --- FeeCheck ---

    #[tokio::test]
    async fn fee_check_sufficient() {
        let check = FeeCheck::new(10_000, 500);
        let intent = test_intent(5_000);
        let result = check.check(&intent).await.expect("check");
        assert!(result.passed);
    }

    #[tokio::test]
    async fn fee_check_insufficient() {
        let check = FeeCheck::new(5_000, 500);
        let intent = test_intent(5_000);
        let result = check.check(&intent).await.expect("check");
        assert!(!result.passed);
        assert_eq!(result.blockers[0].code, "insufficient_for_fee");
    }

    // --- ExistentialDepositCheck ---

    #[tokio::test]
    async fn ed_check_both_ok() {
        // sender keeps 2000, recipient gets 1000+5000=6000, ED=1000 -> OK
        let check = ExistentialDepositCheck::new(1_000, 2_000, 1_000);
        let intent = test_intent(5_000);
        let result = check.check(&intent).await.expect("check");
        assert!(result.passed);
    }

    #[tokio::test]
    async fn ed_check_sender_reaped() {
        // sender remaining 500 < ED 1000
        let check = ExistentialDepositCheck::new(1_000, 500, 5_000);
        let intent = test_intent(5_000);
        let result = check.check(&intent).await.expect("check");
        assert!(!result.passed);
        assert!(result.blockers.iter().any(|b| b.code == "sender_below_ed"));
    }

    #[tokio::test]
    async fn ed_check_recipient_dust() {
        // recipient has 0, receives 500, but ED is 1000 -> dust
        let check = ExistentialDepositCheck::new(1_000, 5_000, 0);
        let intent = test_intent(500);
        let result = check.check(&intent).await.expect("check");
        assert!(!result.passed);
        assert!(result
            .blockers
            .iter()
            .any(|b| b.code == "recipient_below_ed"));
    }

    #[tokio::test]
    async fn ed_check_sender_zero_remaining_ok() {
        // sender remaining is 0 (exact transfer) -- account is killed
        // intentionally, not a dust scenario
        let check = ExistentialDepositCheck::new(1_000, 0, 5_000);
        let intent = test_intent(5_000);
        let result = check.check(&intent).await.expect("check");
        assert!(result.passed);
    }

    // --- AddressCheck ---

    #[tokio::test]
    async fn address_check_valid() {
        let check = AddressCheck::new();
        let intent = test_intent(1_000);
        let result = check.check(&intent).await.expect("check");
        assert!(result.passed);
    }

    #[tokio::test]
    async fn address_check_empty() {
        let check = AddressCheck::new();
        let mut intent = test_intent(1_000);
        intent.recipient = String::new();
        let result = check.check(&intent).await.expect("check");
        assert!(!result.passed);
        assert_eq!(result.blockers[0].code, "invalid_address");
    }

    #[tokio::test]
    async fn address_check_too_short() {
        let check = AddressCheck::new();
        let mut intent = test_intent(1_000);
        intent.recipient = "Ab".into();
        let result = check.check(&intent).await.expect("check");
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn address_check_invalid_char() {
        let check = AddressCheck::new();
        let mut intent = test_intent(1_000);
        intent.recipient = "5Grw-vaEF".into(); // hyphen is invalid Base58
        let result = check.check(&intent).await.expect("check");
        assert!(!result.passed);
        assert_eq!(result.blockers[0].code, "invalid_address");
    }

    #[tokio::test]
    async fn address_check_with_prefix_warning() {
        let check = AddressCheck::with_prefix(0);
        let intent = test_intent(1_000);
        let result = check.check(&intent).await.expect("check");
        assert!(result.passed);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].code, "prefix_unchecked");
    }

    // --- NonceCheck ---

    #[tokio::test]
    async fn nonce_check_match() {
        let check = NonceCheck::new(5, 5);
        let intent = test_intent(1_000);
        let result = check.check(&intent).await.expect("check");
        assert!(result.passed);
    }

    #[tokio::test]
    async fn nonce_check_stale() {
        let check = NonceCheck::new(10, 5);
        let intent = test_intent(1_000);
        let result = check.check(&intent).await.expect("check");
        assert!(!result.passed);
        assert_eq!(result.blockers[0].code, "stale_nonce");
    }

    #[tokio::test]
    async fn nonce_check_future() {
        let check = NonceCheck::new(5, 10);
        let intent = test_intent(1_000);
        let result = check.check(&intent).await.expect("check");
        assert!(result.passed);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].code, "future_nonce");
    }

    // --- MetadataFreshnessCheck ---

    #[tokio::test]
    async fn metadata_check_fresh() {
        let check = MetadataFreshnessCheck::new(100, 100);
        let intent = test_intent(1_000);
        let result = check.check(&intent).await.expect("check");
        assert!(result.passed);
    }

    #[tokio::test]
    async fn metadata_check_stale() {
        let check = MetadataFreshnessCheck::new(99, 100);
        let intent = test_intent(1_000);
        let result = check.check(&intent).await.expect("check");
        assert!(!result.passed);
        assert_eq!(result.blockers[0].code, "stale_metadata");
    }

    // --- CompositePreFlight ---

    #[tokio::test]
    async fn composite_empty() {
        let composite = CompositePreFlight::new();
        assert!(composite.is_empty());
        let intent = test_intent(1_000);
        let result = composite.run(&intent).await.expect("run");
        assert!(result.passed);
    }

    #[tokio::test]
    async fn composite_all_pass() {
        let composite = CompositePreFlight::new()
            .with_check(BalanceCheck::new(10_000))
            .with_check(NonceCheck::new(5, 5))
            .with_check(MetadataFreshnessCheck::new(100, 100));
        assert_eq!(composite.len(), 3);
        let intent = test_intent(1_000);
        let result = composite.run(&intent).await.expect("run");
        assert!(result.passed);
    }

    #[tokio::test]
    async fn composite_one_fails() {
        let composite = CompositePreFlight::new()
            .with_check(BalanceCheck::new(500)) // insufficient
            .with_check(NonceCheck::new(5, 5));
        let intent = test_intent(1_000);
        let result = composite.run(&intent).await.expect("run");
        assert!(!result.passed);
        assert_eq!(result.blockers.len(), 1);
    }

    #[tokio::test]
    async fn composite_multiple_fail() {
        let composite = CompositePreFlight::new()
            .with_check(BalanceCheck::new(500))
            .with_check(MetadataFreshnessCheck::new(99, 100));
        let intent = test_intent(1_000);
        let result = composite.run(&intent).await.expect("run");
        assert!(!result.passed);
        assert_eq!(result.blockers.len(), 2);
    }

    #[tokio::test]
    async fn composite_add_check() {
        let mut composite = CompositePreFlight::new();
        composite.add_check(BalanceCheck::new(10_000));
        assert_eq!(composite.len(), 1);
    }
}

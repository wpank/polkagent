//! Risk detection engine for payment safety.
//!
//! Provides a pluggable gate that inspects [`PaymentIntent`]s before they are
//! submitted, producing [`RiskFinding`]s that callers can act on (block, warn,
//! or log).
//!
//! # Built-in detectors
//!
//! | Detector | Risk code | What it catches |
//! |----------|-----------|-----------------|
//! | [`BatchHidingDetector`] | `BatchHiding` | Batch calls that hide transfer sub-calls |
//! | [`HomoglyphDetector`] | `HomoglyphAddress` | Addresses with confusable Unicode characters |
//! | [`HighValueDetector`] | `HighValue` | Transfers above a configurable threshold |
//! | [`CompositeRiskGate`] | *(all)* | Runs every registered detector |

use serde::{Deserialize, Serialize};

use crate::types::PaymentIntent;

// ---------------------------------------------------------------------------
// RiskSeverity
// ---------------------------------------------------------------------------

/// How severe a risk finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskSeverity {
    /// Informational — no action required but worth logging.
    Info,
    /// The user should review the intent before proceeding.
    Warning,
    /// The intent should be blocked unless explicitly overridden.
    Critical,
}

impl std::fmt::Display for RiskSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// RiskCode
// ---------------------------------------------------------------------------

/// Identifies the class of risk detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskCode {
    /// A batch call contains hidden sub-calls (e.g. a transfer buried inside
    /// a `utility.batch` extrinsic).
    BatchHiding,
    /// The recipient address contains Unicode homoglyphs that could be
    /// confused with ASCII characters.
    HomoglyphAddress,
    /// Two or more intents target nearly identical recipients.
    NearDuplicate,
    /// The metadata referenced by the intent is stale.
    StaleMetadata,
    /// The transfer would leave the sender near the existential deposit.
    NearExistentialDeposit,
    /// Fee estimation is unavailable or uncertain.
    UnknownFee,
    /// The transfer value exceeds a configured threshold.
    HighValue,
    /// The recipient has never received funds from this agent before.
    FirstTimeRecipient,
    /// The extrinsic wraps a proxy call.
    ProxyCall,
    /// The intent involves a cross-chain (XCM) transfer.
    CrossChain,
}

impl std::fmt::Display for RiskCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::BatchHiding => "batch_hiding",
            Self::HomoglyphAddress => "homoglyph_address",
            Self::NearDuplicate => "near_duplicate",
            Self::StaleMetadata => "stale_metadata",
            Self::NearExistentialDeposit => "near_existential_deposit",
            Self::UnknownFee => "unknown_fee",
            Self::HighValue => "high_value",
            Self::FirstTimeRecipient => "first_time_recipient",
            Self::ProxyCall => "proxy_call",
            Self::CrossChain => "cross_chain",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// RiskFinding
// ---------------------------------------------------------------------------

/// A single risk finding produced by a detector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskFinding {
    /// Which risk was detected.
    pub code: RiskCode,
    /// Severity level.
    pub severity: RiskSeverity,
    /// Human-readable description of the finding.
    pub message: String,
    /// Structured evidence (detector-specific payload).
    pub evidence: serde_json::Value,
}

impl std::fmt::Display for RiskFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}: {}", self.severity, self.code, self.message)
    }
}

// ---------------------------------------------------------------------------
// RiskGate trait
// ---------------------------------------------------------------------------

/// A detector that inspects a [`PaymentIntent`] and returns zero or more
/// risk findings.
pub trait RiskGate: Send + Sync {
    /// Assess the given intent and return any findings.
    fn assess(&self, intent: &PaymentIntent) -> Vec<RiskFinding>;
}

// ---------------------------------------------------------------------------
// BatchHidingDetector
// ---------------------------------------------------------------------------

/// Detects batch calls that contain hidden sub-calls.
///
/// Looks at the `metadata` field of a [`PaymentIntent`] for a JSON object
/// with a `"calls"` array. If the array contains more entries than expected
/// for a simple transfer, a finding is raised.
pub struct BatchHidingDetector;

impl RiskGate for BatchHidingDetector {
    fn assess(&self, intent: &PaymentIntent) -> Vec<RiskFinding> {
        let metadata = match &intent.metadata {
            Some(m) => m,
            None => return vec![],
        };

        let calls = match metadata.get("calls").and_then(|c| c.as_array()) {
            Some(c) => c,
            None => return vec![],
        };

        // A simple transfer should have at most 1 call. If there are more,
        // someone may be hiding additional operations in a batch.
        if calls.len() <= 1 {
            return vec![];
        }

        // Check if any sub-call is not a balance transfer.
        let hidden: Vec<&serde_json::Value> = calls
            .iter()
            .filter(|c| {
                let pallet = c.get("pallet").and_then(|p| p.as_str()).unwrap_or("");
                let call = c.get("call").and_then(|c| c.as_str()).unwrap_or("");
                // Non-transfer calls inside a batch are suspicious.
                !(pallet == "Balances"
                    && (call == "transfer_keep_alive"
                        || call == "transfer_allow_death"
                        || call == "transfer_all"))
            })
            .collect();

        if hidden.is_empty() {
            return vec![];
        }

        vec![RiskFinding {
            code: RiskCode::BatchHiding,
            severity: RiskSeverity::Critical,
            message: format!(
                "batch contains {} non-transfer call(s) among {} total calls",
                hidden.len(),
                calls.len()
            ),
            evidence: serde_json::json!({
                "total_calls": calls.len(),
                "hidden_calls": hidden,
            }),
        }]
    }
}

// ---------------------------------------------------------------------------
// HomoglyphDetector
// ---------------------------------------------------------------------------

/// Flags addresses that contain Unicode characters visually similar to ASCII
/// characters (homoglyphs).
///
/// Substrate addresses should be pure ASCII (Base58 encoding). Any non-ASCII
/// character is suspicious and may indicate a spoofed address.
pub struct HomoglyphDetector;

impl HomoglyphDetector {
    /// Characters that are known homoglyphs for common Base58 characters.
    /// This is a representative subset; in production a full Unicode
    /// confusables table (UTS #39) would be used.
    const CONFUSABLE_PAIRS: &'static [(char, char)] = &[
        ('\u{0410}', 'A'), // Cyrillic А → Latin A
        ('\u{0412}', 'B'), // Cyrillic В → Latin B
        ('\u{0421}', 'C'), // Cyrillic С → Latin C
        ('\u{0415}', 'E'), // Cyrillic Е → Latin E
        ('\u{041D}', 'H'), // Cyrillic Н → Latin H
        ('\u{041A}', 'K'), // Cyrillic К → Latin K
        ('\u{041C}', 'M'), // Cyrillic М → Latin M
        ('\u{041E}', 'O'), // Cyrillic О → Latin O
        ('\u{0420}', 'P'), // Cyrillic Р → Latin P
        ('\u{0422}', 'T'), // Cyrillic Т → Latin T
        ('\u{0425}', 'X'), // Cyrillic Х → Latin X
        ('\u{0430}', 'a'), // Cyrillic а → Latin a
        ('\u{0435}', 'e'), // Cyrillic е → Latin e
        ('\u{043E}', 'o'), // Cyrillic о → Latin o
        ('\u{0440}', 'p'), // Cyrillic р → Latin p
        ('\u{0441}', 'c'), // Cyrillic с → Latin c
        ('\u{0443}', 'y'), // Cyrillic у → Latin y
        ('\u{0445}', 'x'), // Cyrillic х → Latin x
        ('\u{0456}', 'i'), // Cyrillic і → Latin i
        ('\u{0458}', 'j'), // Cyrillic ј → Latin j
        ('\u{04BB}', 'h'), // Cyrillic һ → Latin h
        ('\u{0501}', 'd'), // Cyrillic ԁ → Latin d
        ('\u{051B}', 'q'), // Cyrillic ԛ → Latin q
        ('\u{051D}', 'w'), // Cyrillic ԝ → Latin w
    ];

    /// Returns confusable characters found in the address.
    fn find_confusables(address: &str) -> Vec<(usize, char, char)> {
        let mut found = Vec::new();
        for (pos, ch) in address.char_indices() {
            // Any non-ASCII character in a Base58 address is suspicious.
            if !ch.is_ascii() {
                // Check if it's a known confusable.
                let look_alike = Self::CONFUSABLE_PAIRS
                    .iter()
                    .find(|(confusable, _)| *confusable == ch)
                    .map(|(_, ascii)| *ascii)
                    .unwrap_or('?');
                found.push((pos, ch, look_alike));
            }
        }
        found
    }
}

impl RiskGate for HomoglyphDetector {
    fn assess(&self, intent: &PaymentIntent) -> Vec<RiskFinding> {
        let confusables = Self::find_confusables(&intent.recipient);
        if confusables.is_empty() {
            return vec![];
        }

        let details: Vec<serde_json::Value> = confusables
            .iter()
            .map(|(pos, ch, look_alike)| {
                serde_json::json!({
                    "position": pos,
                    "character": ch.to_string(),
                    "codepoint": format!("U+{:04X}", *ch as u32),
                    "looks_like": look_alike.to_string(),
                })
            })
            .collect();

        vec![RiskFinding {
            code: RiskCode::HomoglyphAddress,
            severity: RiskSeverity::Critical,
            message: format!(
                "recipient address contains {} confusable character(s)",
                confusables.len()
            ),
            evidence: serde_json::json!({
                "address": intent.recipient,
                "confusables": details,
            }),
        }]
    }
}

// ---------------------------------------------------------------------------
// HighValueDetector
// ---------------------------------------------------------------------------

/// Flags transfers whose value exceeds a configurable threshold (in planck).
pub struct HighValueDetector {
    /// The threshold in planck above which a finding is raised.
    threshold_planck: u128,
}

impl HighValueDetector {
    /// Create a new high-value detector with the given threshold in planck.
    #[must_use]
    pub fn new(threshold_planck: u128) -> Self {
        Self { threshold_planck }
    }
}

impl RiskGate for HighValueDetector {
    fn assess(&self, intent: &PaymentIntent) -> Vec<RiskFinding> {
        if intent.amount.value <= self.threshold_planck {
            return vec![];
        }

        vec![RiskFinding {
            code: RiskCode::HighValue,
            severity: RiskSeverity::Warning,
            message: format!(
                "transfer value {} exceeds threshold of {} planck",
                intent.amount.display_human(),
                self.threshold_planck,
            ),
            evidence: serde_json::json!({
                "value_planck": intent.amount.value,
                "threshold_planck": self.threshold_planck,
                "asset": intent.amount.asset.to_string(),
            }),
        }]
    }
}

// ---------------------------------------------------------------------------
// CompositeRiskGate
// ---------------------------------------------------------------------------

/// Runs all registered detectors and aggregates their findings.
pub struct CompositeRiskGate {
    gates: Vec<Box<dyn RiskGate>>,
}

impl CompositeRiskGate {
    /// Create a new composite gate with no detectors.
    #[must_use]
    pub fn new() -> Self {
        Self { gates: Vec::new() }
    }

    /// Create a composite gate pre-loaded with all built-in detectors.
    ///
    /// `high_value_threshold` is the planck threshold for the
    /// [`HighValueDetector`].
    #[must_use]
    pub fn with_defaults(high_value_threshold: u128) -> Self {
        let mut gate = Self::new();
        gate.add(Box::new(BatchHidingDetector));
        gate.add(Box::new(HomoglyphDetector));
        gate.add(Box::new(HighValueDetector::new(high_value_threshold)));
        gate
    }

    /// Add a detector to the composite gate.
    pub fn add(&mut self, gate: Box<dyn RiskGate>) {
        self.gates.push(gate);
    }

    /// Return the number of registered detectors.
    #[must_use]
    pub fn detector_count(&self) -> usize {
        self.gates.len()
    }
}

impl Default for CompositeRiskGate {
    fn default() -> Self {
        Self::new()
    }
}

impl RiskGate for CompositeRiskGate {
    fn assess(&self, intent: &PaymentIntent) -> Vec<RiskFinding> {
        self.gates.iter().flat_map(|g| g.assess(intent)).collect()
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

    // ======================================================================
    // Helpers
    // ======================================================================

    fn dot_asset() -> AssetId {
        AssetId::Native
    }

    fn make_intent(recipient: &str, planck: u128) -> PaymentIntent {
        PaymentIntent {
            id: Uuid::now_v7(),
            agent_id: "agent-test".into(),
            run_id: "run-1".into(),
            amount: Amount::new(planck, dot_asset(), 10),
            recipient: recipient.into(),
            idempotency_key: Uuid::now_v7().to_string(),
            created_at: Utc::now(),
            status: PaymentStatus::Pending,
            metadata: None,
        }
    }

    fn make_intent_with_metadata(
        recipient: &str,
        planck: u128,
        metadata: serde_json::Value,
    ) -> PaymentIntent {
        let mut intent = make_intent(recipient, planck);
        intent.metadata = Some(metadata);
        intent
    }

    // ======================================================================
    // RiskSeverity tests
    // ======================================================================

    #[test]
    fn severity_display() {
        assert_eq!(RiskSeverity::Info.to_string(), "info");
        assert_eq!(RiskSeverity::Warning.to_string(), "warning");
        assert_eq!(RiskSeverity::Critical.to_string(), "critical");
    }

    #[test]
    fn severity_serde_round_trip() {
        for severity in [
            RiskSeverity::Info,
            RiskSeverity::Warning,
            RiskSeverity::Critical,
        ] {
            let json = serde_json::to_string(&severity).expect("serialize");
            let back: RiskSeverity = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(severity, back);
        }
    }

    // ======================================================================
    // RiskCode tests
    // ======================================================================

    #[test]
    fn risk_code_display() {
        assert_eq!(RiskCode::BatchHiding.to_string(), "batch_hiding");
        assert_eq!(RiskCode::HomoglyphAddress.to_string(), "homoglyph_address");
        assert_eq!(RiskCode::HighValue.to_string(), "high_value");
        assert_eq!(RiskCode::NearDuplicate.to_string(), "near_duplicate");
        assert_eq!(RiskCode::StaleMetadata.to_string(), "stale_metadata");
        assert_eq!(
            RiskCode::NearExistentialDeposit.to_string(),
            "near_existential_deposit"
        );
        assert_eq!(RiskCode::UnknownFee.to_string(), "unknown_fee");
        assert_eq!(
            RiskCode::FirstTimeRecipient.to_string(),
            "first_time_recipient"
        );
        assert_eq!(RiskCode::ProxyCall.to_string(), "proxy_call");
        assert_eq!(RiskCode::CrossChain.to_string(), "cross_chain");
    }

    #[test]
    fn risk_code_serde_round_trip() {
        for code in [
            RiskCode::BatchHiding,
            RiskCode::HomoglyphAddress,
            RiskCode::NearDuplicate,
            RiskCode::StaleMetadata,
            RiskCode::NearExistentialDeposit,
            RiskCode::UnknownFee,
            RiskCode::HighValue,
            RiskCode::FirstTimeRecipient,
            RiskCode::ProxyCall,
            RiskCode::CrossChain,
        ] {
            let json = serde_json::to_string(&code).expect("serialize");
            let back: RiskCode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(code, back);
        }
    }

    // ======================================================================
    // RiskFinding tests
    // ======================================================================

    #[test]
    fn finding_display() {
        let finding = RiskFinding {
            code: RiskCode::HighValue,
            severity: RiskSeverity::Warning,
            message: "too much".into(),
            evidence: serde_json::json!({}),
        };
        assert_eq!(finding.to_string(), "[warning] high_value: too much");
    }

    #[test]
    fn finding_serde_round_trip() {
        let finding = RiskFinding {
            code: RiskCode::HomoglyphAddress,
            severity: RiskSeverity::Critical,
            message: "suspicious address".into(),
            evidence: serde_json::json!({"position": 3}),
        };
        let json = serde_json::to_string(&finding).expect("serialize");
        let back: RiskFinding = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(finding, back);
    }

    // ======================================================================
    // BatchHidingDetector tests
    // ======================================================================

    #[test]
    fn batch_hiding_no_metadata() {
        let detector = BatchHidingDetector;
        let intent = make_intent("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY", 1000);
        let findings = detector.assess(&intent);
        assert!(findings.is_empty());
    }

    #[test]
    fn batch_hiding_no_calls_key() {
        let detector = BatchHidingDetector;
        let intent = make_intent_with_metadata(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            1000,
            serde_json::json!({"other": "data"}),
        );
        let findings = detector.assess(&intent);
        assert!(findings.is_empty());
    }

    #[test]
    fn batch_hiding_single_transfer_call() {
        let detector = BatchHidingDetector;
        let intent = make_intent_with_metadata(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            1000,
            serde_json::json!({
                "calls": [
                    {"pallet": "Balances", "call": "transfer_keep_alive", "args": {}}
                ]
            }),
        );
        let findings = detector.assess(&intent);
        assert!(findings.is_empty());
    }

    #[test]
    fn batch_hiding_multiple_transfers_only() {
        let detector = BatchHidingDetector;
        let intent = make_intent_with_metadata(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            1000,
            serde_json::json!({
                "calls": [
                    {"pallet": "Balances", "call": "transfer_keep_alive", "args": {}},
                    {"pallet": "Balances", "call": "transfer_allow_death", "args": {}}
                ]
            }),
        );
        let findings = detector.assess(&intent);
        assert!(findings.is_empty());
    }

    #[test]
    fn batch_hiding_hidden_system_call() {
        let detector = BatchHidingDetector;
        let intent = make_intent_with_metadata(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            1000,
            serde_json::json!({
                "calls": [
                    {"pallet": "Balances", "call": "transfer_keep_alive", "args": {}},
                    {"pallet": "System", "call": "set_code", "args": {"code": "0x1234"}}
                ]
            }),
        );
        let findings = detector.assess(&intent);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, RiskCode::BatchHiding);
        assert_eq!(findings[0].severity, RiskSeverity::Critical);
        assert!(findings[0].message.contains("1 non-transfer call(s)"));
    }

    #[test]
    fn batch_hiding_multiple_hidden_calls() {
        let detector = BatchHidingDetector;
        let intent = make_intent_with_metadata(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            1000,
            serde_json::json!({
                "calls": [
                    {"pallet": "Balances", "call": "transfer_keep_alive", "args": {}},
                    {"pallet": "Staking", "call": "bond", "args": {}},
                    {"pallet": "Proxy", "call": "add_proxy", "args": {}}
                ]
            }),
        );
        let findings = detector.assess(&intent);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, RiskCode::BatchHiding);
        assert!(findings[0].message.contains("2 non-transfer call(s)"));
        assert!(findings[0].message.contains("3 total calls"));
    }

    #[test]
    fn batch_hiding_transfer_all_is_safe() {
        let detector = BatchHidingDetector;
        let intent = make_intent_with_metadata(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            1000,
            serde_json::json!({
                "calls": [
                    {"pallet": "Balances", "call": "transfer_all", "args": {}},
                    {"pallet": "Balances", "call": "transfer_keep_alive", "args": {}}
                ]
            }),
        );
        let findings = detector.assess(&intent);
        assert!(findings.is_empty());
    }

    #[test]
    fn batch_hiding_empty_calls_array() {
        let detector = BatchHidingDetector;
        let intent = make_intent_with_metadata(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            1000,
            serde_json::json!({"calls": []}),
        );
        let findings = detector.assess(&intent);
        assert!(findings.is_empty());
    }

    // ======================================================================
    // HomoglyphDetector tests
    // ======================================================================

    #[test]
    fn homoglyph_clean_ascii_address() {
        let detector = HomoglyphDetector;
        let intent = make_intent("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY", 1000);
        let findings = detector.assess(&intent);
        assert!(findings.is_empty());
    }

    #[test]
    fn homoglyph_cyrillic_a() {
        let detector = HomoglyphDetector;
        // Replace the 'A' at index 10 (the A in the address) with Cyrillic А (U+0410)
        let spoofed = "5GrwvaEF5z\u{0410}b26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
        let intent = make_intent(spoofed, 1000);
        let findings = detector.assess(&intent);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, RiskCode::HomoglyphAddress);
        assert_eq!(findings[0].severity, RiskSeverity::Critical);

        let confusables = findings[0]
            .evidence
            .get("confusables")
            .expect("has confusables");
        let arr = confusables.as_array().expect("is array");
        assert_eq!(arr.len(), 1);
        assert_eq!(
            arr[0].get("codepoint").and_then(|v| v.as_str()),
            Some("U+0410")
        );
        assert_eq!(arr[0].get("looks_like").and_then(|v| v.as_str()), Some("A"));
    }

    #[test]
    fn homoglyph_multiple_confusables() {
        let detector = HomoglyphDetector;
        // Replace both 'o' chars with Cyrillic о (U+043E)
        let spoofed = "5Grwva\u{0415}F5zXb26Fz9rcQpDWS57Ct\u{0415}RHpNehXCPcN\u{043E}HGKutQY";
        let intent = make_intent(spoofed, 1000);
        let findings = detector.assess(&intent);
        assert_eq!(findings.len(), 1);
        let confusables = findings[0]
            .evidence
            .get("confusables")
            .and_then(|v| v.as_array())
            .expect("confusables array");
        assert_eq!(confusables.len(), 3);
    }

    #[test]
    fn homoglyph_cyrillic_o_lowercase() {
        let detector = HomoglyphDetector;
        let spoofed = "5Grwva\u{043E}F5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
        let intent = make_intent(spoofed, 1000);
        let findings = detector.assess(&intent);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, RiskCode::HomoglyphAddress);
    }

    #[test]
    fn homoglyph_cyrillic_p_lowercase() {
        let detector = HomoglyphDetector;
        let spoofed = "5GrwvaEF5zXb26Fz9rcQ\u{0440}DWS57CtERHpNehXCPcNoHGKutQY";
        let intent = make_intent(spoofed, 1000);
        let findings = detector.assess(&intent);
        assert_eq!(findings.len(), 1);
        let confusables = findings[0]
            .evidence
            .get("confusables")
            .and_then(|v| v.as_array())
            .expect("confusables array");
        assert_eq!(
            confusables[0].get("looks_like").and_then(|v| v.as_str()),
            Some("p")
        );
    }

    #[test]
    fn homoglyph_unknown_non_ascii() {
        let detector = HomoglyphDetector;
        // Use a non-ASCII char that is NOT in the known confusables table.
        let spoofed = "5GrwvaEF5zXb26F\u{00E9}9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
        let intent = make_intent(spoofed, 1000);
        let findings = detector.assess(&intent);
        assert_eq!(findings.len(), 1);
        let confusables = findings[0]
            .evidence
            .get("confusables")
            .and_then(|v| v.as_array())
            .expect("confusables array");
        assert_eq!(
            confusables[0].get("looks_like").and_then(|v| v.as_str()),
            Some("?")
        );
    }

    #[test]
    fn homoglyph_empty_address() {
        let detector = HomoglyphDetector;
        let intent = make_intent("", 1000);
        let findings = detector.assess(&intent);
        assert!(findings.is_empty());
    }

    // ======================================================================
    // HighValueDetector tests
    // ======================================================================

    #[test]
    fn high_value_below_threshold() {
        let detector = HighValueDetector::new(100_000_000_000); // 10 DOT
        let intent = make_intent(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            50_000_000_000,
        );
        let findings = detector.assess(&intent);
        assert!(findings.is_empty());
    }

    #[test]
    fn high_value_at_threshold() {
        let detector = HighValueDetector::new(100_000_000_000);
        let intent = make_intent(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            100_000_000_000,
        );
        let findings = detector.assess(&intent);
        assert!(findings.is_empty(), "at threshold should not trigger");
    }

    #[test]
    fn high_value_above_threshold() {
        let detector = HighValueDetector::new(100_000_000_000); // 10 DOT
        let intent = make_intent(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            200_000_000_000, // 20 DOT
        );
        let findings = detector.assess(&intent);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, RiskCode::HighValue);
        assert_eq!(findings[0].severity, RiskSeverity::Warning);
    }

    #[test]
    fn high_value_just_above_threshold() {
        let detector = HighValueDetector::new(100_000_000_000);
        let intent = make_intent(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            100_000_000_001,
        );
        let findings = detector.assess(&intent);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, RiskCode::HighValue);
    }

    #[test]
    fn high_value_zero_threshold() {
        let detector = HighValueDetector::new(0);
        let intent = make_intent("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY", 1);
        let findings = detector.assess(&intent);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, RiskCode::HighValue);
    }

    #[test]
    fn high_value_zero_amount() {
        let detector = HighValueDetector::new(0);
        let intent = make_intent("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY", 0);
        let findings = detector.assess(&intent);
        assert!(findings.is_empty());
    }

    #[test]
    fn high_value_evidence_fields() {
        let detector = HighValueDetector::new(1000);
        let intent = make_intent("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY", 5000);
        let findings = detector.assess(&intent);
        assert_eq!(findings.len(), 1);

        let evidence = &findings[0].evidence;
        assert_eq!(
            evidence.get("value_planck").and_then(|v| v.as_u64()),
            Some(5000)
        );
        assert_eq!(
            evidence.get("threshold_planck").and_then(|v| v.as_u64()),
            Some(1000)
        );
        assert_eq!(
            evidence.get("asset").and_then(|v| v.as_str()),
            Some("NATIVE")
        );
    }

    // ======================================================================
    // CompositeRiskGate tests
    // ======================================================================

    #[test]
    fn composite_empty_no_findings() {
        let gate = CompositeRiskGate::new();
        let intent = make_intent("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY", 1000);
        let findings = gate.assess(&intent);
        assert!(findings.is_empty());
    }

    #[test]
    fn composite_with_defaults_detector_count() {
        let gate = CompositeRiskGate::with_defaults(100_000_000_000);
        assert_eq!(gate.detector_count(), 3);
    }

    #[test]
    fn composite_clean_intent_no_findings() {
        let gate = CompositeRiskGate::with_defaults(100_000_000_000);
        let intent = make_intent(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            50_000_000_000,
        );
        let findings = gate.assess(&intent);
        assert!(findings.is_empty());
    }

    #[test]
    fn composite_high_value_only() {
        let gate = CompositeRiskGate::with_defaults(100_000_000_000);
        let intent = make_intent(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            500_000_000_000,
        );
        let findings = gate.assess(&intent);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, RiskCode::HighValue);
    }

    #[test]
    fn composite_homoglyph_only() {
        let gate = CompositeRiskGate::with_defaults(100_000_000_000);
        let spoofed = "5Grwva\u{0415}F5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
        let intent = make_intent(spoofed, 1000);
        let findings = gate.assess(&intent);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, RiskCode::HomoglyphAddress);
    }

    #[test]
    fn composite_multiple_findings() {
        let gate = CompositeRiskGate::with_defaults(1000);
        // Both homoglyph AND high value
        let spoofed = "5Grwva\u{0415}F5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
        let intent = make_intent(spoofed, 500_000_000_000);
        let findings = gate.assess(&intent);
        assert_eq!(findings.len(), 2);

        let codes: Vec<RiskCode> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&RiskCode::HomoglyphAddress));
        assert!(codes.contains(&RiskCode::HighValue));
    }

    #[test]
    fn composite_batch_hiding_with_high_value() {
        let gate = CompositeRiskGate::with_defaults(1000);
        let intent = make_intent_with_metadata(
            "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
            500_000_000_000,
            serde_json::json!({
                "calls": [
                    {"pallet": "Balances", "call": "transfer_keep_alive", "args": {}},
                    {"pallet": "Staking", "call": "bond", "args": {}}
                ]
            }),
        );
        let findings = gate.assess(&intent);
        assert_eq!(findings.len(), 2);
        let codes: Vec<RiskCode> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&RiskCode::BatchHiding));
        assert!(codes.contains(&RiskCode::HighValue));
    }

    #[test]
    fn composite_all_three_detectors_fire() {
        let gate = CompositeRiskGate::with_defaults(1000);
        let spoofed = "5Grwva\u{0415}F5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
        let intent = make_intent_with_metadata(
            spoofed,
            500_000_000_000,
            serde_json::json!({
                "calls": [
                    {"pallet": "Balances", "call": "transfer_keep_alive", "args": {}},
                    {"pallet": "System", "call": "remark", "args": {}}
                ]
            }),
        );
        let findings = gate.assess(&intent);
        assert_eq!(findings.len(), 3);
        let codes: Vec<RiskCode> = findings.iter().map(|f| f.code).collect();
        assert!(codes.contains(&RiskCode::BatchHiding));
        assert!(codes.contains(&RiskCode::HomoglyphAddress));
        assert!(codes.contains(&RiskCode::HighValue));
    }

    #[test]
    fn composite_add_custom_detector() {
        struct AlwaysWarnGate;
        impl RiskGate for AlwaysWarnGate {
            fn assess(&self, _intent: &PaymentIntent) -> Vec<RiskFinding> {
                vec![RiskFinding {
                    code: RiskCode::CrossChain,
                    severity: RiskSeverity::Info,
                    message: "always warns".into(),
                    evidence: serde_json::json!(null),
                }]
            }
        }

        let mut gate = CompositeRiskGate::new();
        gate.add(Box::new(AlwaysWarnGate));
        assert_eq!(gate.detector_count(), 1);

        let intent = make_intent("5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY", 1000);
        let findings = gate.assess(&intent);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, RiskCode::CrossChain);
    }

    #[test]
    fn composite_default_is_empty() {
        let gate = CompositeRiskGate::default();
        assert_eq!(gate.detector_count(), 0);
    }
}

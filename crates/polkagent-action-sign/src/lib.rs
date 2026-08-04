#![forbid(unsafe_code)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use polkagent_codec::{
    call::{call_index, pallet_index},
    decode_batch_call, decode_proxy_call, is_batch_call, is_proxy_call, is_transfer_call,
    DecodedExtrinsic,
};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Well-known pallet/call indices not yet in polkagent-codec
// ---------------------------------------------------------------------------

mod known {
    pub const MULTISIG_PALLET: u8 = 30;
    pub const MULTISIG_AS_MULTI: u8 = 1;
    pub const MULTISIG_APPROVE_AS_MULTI: u8 = 2;

    pub const PROXY_ADD_PROXY: u8 = 1;
    pub const PROXY_REMOVE_PROXY: u8 = 2;
    pub const PROXY_REMOVE_PROXIES: u8 = 3;
    pub const PROXY_ANONYMOUS: u8 = 4;
}

// ---------------------------------------------------------------------------
// RiskLevel
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "LOW"),
            Self::Medium => write!(f, "MEDIUM"),
            Self::High => write!(f, "HIGH"),
            Self::Critical => write!(f, "CRITICAL"),
        }
    }
}

// ---------------------------------------------------------------------------
// CallPattern
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CallPattern {
    SimpleTransfer,
    Batch,
    BatchAll,
    ForceBatch,
    Proxy,
    ProxyAddition,
    ProxyRemoval,
    Multisig,
    MultisigApproval,
    BatchWithProxy,
    BatchAllWithProxy,
    BatchWithProxyMutation,
    NestedProxy,
    UnknownCall,
}

impl std::fmt::Display for CallPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SimpleTransfer => write!(f, "simple_transfer"),
            Self::Batch => write!(f, "batch"),
            Self::BatchAll => write!(f, "batch_all"),
            Self::ForceBatch => write!(f, "force_batch"),
            Self::Proxy => write!(f, "proxy"),
            Self::ProxyAddition => write!(f, "proxy_addition"),
            Self::ProxyRemoval => write!(f, "proxy_removal"),
            Self::Multisig => write!(f, "multisig"),
            Self::MultisigApproval => write!(f, "multisig_approval"),
            Self::BatchWithProxy => write!(f, "batch_with_proxy"),
            Self::BatchAllWithProxy => write!(f, "batch_all_with_proxy"),
            Self::BatchWithProxyMutation => write!(f, "batch_with_proxy_mutation"),
            Self::NestedProxy => write!(f, "nested_proxy"),
            Self::UnknownCall => write!(f, "unknown_call"),
        }
    }
}

// ---------------------------------------------------------------------------
// RiskFinding
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskFinding {
    pub level: RiskLevel,
    pub pattern: CallPattern,
    pub description: String,
}

// ---------------------------------------------------------------------------
// RiskAssessment
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub overall_level: RiskLevel,
    pub findings: Vec<RiskFinding>,
}

impl RiskAssessment {
    pub fn should_block(&self) -> bool {
        self.overall_level == RiskLevel::Critical
    }

    pub fn should_warn(&self) -> bool {
        self.overall_level >= RiskLevel::High
    }
}

// ---------------------------------------------------------------------------
// classify_risk — main entry point
// ---------------------------------------------------------------------------

pub fn classify_risk(ext: &DecodedExtrinsic) -> RiskAssessment {
    let mut findings = Vec::new();

    classify_top_level(ext, &mut findings);

    if is_batch_call(ext) {
        classify_batch(ext, &mut findings);
    }

    if is_proxy_call(ext) {
        classify_proxy(ext, &mut findings);
    }

    let overall_level = findings
        .iter()
        .map(|f| f.level)
        .max()
        .unwrap_or(RiskLevel::Low);

    RiskAssessment {
        overall_level,
        findings,
    }
}

// ---------------------------------------------------------------------------
// Internal classifiers
// ---------------------------------------------------------------------------

fn classify_top_level(ext: &DecodedExtrinsic, findings: &mut Vec<RiskFinding>) {
    if is_transfer_call(ext) {
        findings.push(RiskFinding {
            level: RiskLevel::Low,
            pattern: CallPattern::SimpleTransfer,
            description: "Simple balance transfer".into(),
        });
        return;
    }

    if is_batch_call(ext) {
        let pattern = match ext.call_index {
            call_index::UTILITY_BATCH => CallPattern::Batch,
            call_index::UTILITY_BATCH_ALL => CallPattern::BatchAll,
            call_index::UTILITY_FORCE_BATCH => CallPattern::ForceBatch,
            _ => CallPattern::Batch,
        };
        let level = match pattern {
            CallPattern::BatchAll | CallPattern::ForceBatch => RiskLevel::Medium,
            _ => RiskLevel::Low,
        };
        findings.push(RiskFinding {
            level,
            pattern,
            description: format!("Batch call ({pattern})"),
        });
        return;
    }

    if is_proxy_call(ext) {
        findings.push(RiskFinding {
            level: RiskLevel::Medium,
            pattern: CallPattern::Proxy,
            description: "Proxy dispatch — executing call on behalf of another account".into(),
        });
        return;
    }

    if is_proxy_mutation(ext) {
        let pattern = if ext.call_index == known::PROXY_ADD_PROXY {
            CallPattern::ProxyAddition
        } else {
            CallPattern::ProxyRemoval
        };
        findings.push(RiskFinding {
            level: RiskLevel::High,
            pattern,
            description: "Proxy permission change — modifies account delegation".into(),
        });
        return;
    }

    if is_multisig_call(ext) {
        let pattern = if ext.call_index == known::MULTISIG_APPROVE_AS_MULTI {
            CallPattern::MultisigApproval
        } else {
            CallPattern::Multisig
        };
        findings.push(RiskFinding {
            level: RiskLevel::Medium,
            pattern,
            description: "Multisig call — requires multiple signatories".into(),
        });
        return;
    }

    if !is_known_call(ext) {
        findings.push(RiskFinding {
            level: RiskLevel::High,
            pattern: CallPattern::UnknownCall,
            description: format!(
                "Unknown call (pallet={}, call={})",
                ext.pallet_index, ext.call_index
            ),
        });
    }
}

fn classify_batch(ext: &DecodedExtrinsic, findings: &mut Vec<RiskFinding>) {
    let inner_calls = match decode_batch_call(ext) {
        Ok(calls) => calls,
        Err(_) => return,
    };

    let has_proxy_dispatch = inner_calls.iter().any(is_proxy_call);
    let has_proxy_mutation = inner_calls.iter().any(is_proxy_mutation);
    let is_batch_all = ext.call_index == call_index::UTILITY_BATCH_ALL;

    if has_proxy_mutation {
        let level = if is_batch_all {
            RiskLevel::Critical
        } else {
            RiskLevel::High
        };
        findings.push(RiskFinding {
            level,
            pattern: CallPattern::BatchWithProxyMutation,
            description:
                "Batch contains proxy permission change — may hide addProxy/removeProxy alongside innocuous calls"
                    .into(),
        });
    }

    if has_proxy_dispatch {
        let level = if is_batch_all {
            RiskLevel::High
        } else {
            RiskLevel::Medium
        };
        findings.push(RiskFinding {
            level,
            pattern: if is_batch_all {
                CallPattern::BatchAllWithProxy
            } else {
                CallPattern::BatchWithProxy
            },
            description: "Batch contains proxy dispatch call".into(),
        });
    }

    for inner in &inner_calls {
        if !is_known_call(inner) {
            findings.push(RiskFinding {
                level: RiskLevel::High,
                pattern: CallPattern::UnknownCall,
                description: format!(
                    "Batch contains unknown inner call (pallet={}, call={})",
                    inner.pallet_index, inner.call_index
                ),
            });
        }
    }
}

fn classify_proxy(ext: &DecodedExtrinsic, findings: &mut Vec<RiskFinding>) {
    let inner = match decode_proxy_call(ext) {
        Ok(call) => call,
        Err(_) => return,
    };

    if is_proxy_call(&inner) {
        findings.push(RiskFinding {
            level: RiskLevel::Critical,
            pattern: CallPattern::NestedProxy,
            description: "Nested proxy dispatch — deep call chain obscures true intent".into(),
        });
    }

    if is_proxy_mutation(&inner) {
        findings.push(RiskFinding {
            level: RiskLevel::Critical,
            pattern: CallPattern::ProxyAddition,
            description: "Proxy dispatch contains proxy permission change".into(),
        });
    }

    if !is_known_call(&inner) {
        findings.push(RiskFinding {
            level: RiskLevel::High,
            pattern: CallPattern::UnknownCall,
            description: format!(
                "Proxy wraps unknown inner call (pallet={}, call={})",
                inner.pallet_index, inner.call_index
            ),
        });
    }
}

// ---------------------------------------------------------------------------
// Predicate helpers
// ---------------------------------------------------------------------------

fn is_proxy_mutation(ext: &DecodedExtrinsic) -> bool {
    ext.pallet_index == pallet_index::PROXY
        && matches!(
            ext.call_index,
            known::PROXY_ADD_PROXY
                | known::PROXY_REMOVE_PROXY
                | known::PROXY_REMOVE_PROXIES
                | known::PROXY_ANONYMOUS
        )
}

fn is_multisig_call(ext: &DecodedExtrinsic) -> bool {
    ext.pallet_index == known::MULTISIG_PALLET
        && matches!(
            ext.call_index,
            known::MULTISIG_AS_MULTI | known::MULTISIG_APPROVE_AS_MULTI
        )
}

fn is_known_call(ext: &DecodedExtrinsic) -> bool {
    is_transfer_call(ext)
        || is_batch_call(ext)
        || is_proxy_call(ext)
        || is_proxy_mutation(ext)
        || is_multisig_call(ext)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_codec::call::{call_index, pallet_index};
    use polkagent_codec::scale::ScaleEncoder;
    use polkagent_codec::{DecodedExtrinsic, DecodedField, FieldValue};

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn make_ext(pallet: u8, call: u8) -> DecodedExtrinsic {
        DecodedExtrinsic {
            pallet_index: pallet,
            call_index: call,
            pallet_name: None,
            call_name: None,
            args: vec![],
        }
    }

    fn make_ext_with_raw(pallet: u8, call: u8, raw_args: Vec<u8>) -> DecodedExtrinsic {
        DecodedExtrinsic {
            pallet_index: pallet,
            call_index: call,
            pallet_name: None,
            call_name: None,
            args: vec![DecodedField::unnamed(FieldValue::Bytes(raw_args))],
        }
    }

    /// Encode inner calls as a batch argument (compact count + compact-length-prefixed calls).
    fn encode_batch_args(inner_calls: &[(u8, u8)]) -> Vec<u8> {
        let mut enc = ScaleEncoder::new();
        enc.encode_compact_u32(inner_calls.len() as u32);
        for &(pallet, call) in inner_calls {
            // Each inner call is length-prefixed: just 2 bytes (pallet + call index).
            let call_bytes = vec![pallet, call];
            enc.encode_bytes(&call_bytes);
        }
        enc.finish()
    }

    /// Encode a proxy.proxy arg wrapping an inner call.
    /// Simplified: AccountId variant 0x00 + 32 zero bytes + None force_proxy_type + inner call.
    fn encode_proxy_args(inner_pallet: u8, inner_call: u8) -> Vec<u8> {
        let mut buf = Vec::new();
        // MultiAddress::Id variant
        buf.push(0x00);
        // 32-byte account id (zeros)
        buf.extend_from_slice(&[0u8; 32]);
        // Option<u8>::None for force_proxy_type
        buf.push(0x00);
        // Inner call as length-prefixed bytes
        let inner_bytes = vec![inner_pallet, inner_call];
        let mut enc = ScaleEncoder::new();
        enc.encode_bytes(&inner_bytes);
        buf.extend_from_slice(&enc.finish());
        buf
    }

    // -----------------------------------------------------------------------
    // 1. Simple transfer → LOW
    // -----------------------------------------------------------------------
    #[test]
    fn test_simple_transfer_is_low_risk() {
        let ext = make_ext(
            pallet_index::BALANCES,
            call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        );
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::Low);
        assert_eq!(assessment.findings.len(), 1);
        assert_eq!(assessment.findings[0].pattern, CallPattern::SimpleTransfer);
        assert!(!assessment.should_block());
        assert!(!assessment.should_warn());
    }

    // -----------------------------------------------------------------------
    // 2. transfer_all → LOW
    // -----------------------------------------------------------------------
    #[test]
    fn test_transfer_all_is_low_risk() {
        let ext = make_ext(pallet_index::BALANCES, call_index::BALANCES_TRANSFER_ALL);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::Low);
        assert_eq!(assessment.findings[0].pattern, CallPattern::SimpleTransfer);
    }

    // -----------------------------------------------------------------------
    // 3. Utility.batch with only transfers → LOW
    // -----------------------------------------------------------------------
    #[test]
    fn test_batch_of_transfers_is_low() {
        let args = encode_batch_args(&[
            (
                pallet_index::BALANCES,
                call_index::BALANCES_TRANSFER_KEEP_ALIVE,
            ),
            (
                pallet_index::BALANCES,
                call_index::BALANCES_TRANSFER_KEEP_ALIVE,
            ),
        ]);
        let ext = make_ext_with_raw(pallet_index::UTILITY, call_index::UTILITY_BATCH, args);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::Low);
    }

    // -----------------------------------------------------------------------
    // 4. Utility.batchAll → MEDIUM (atomic batch)
    // -----------------------------------------------------------------------
    #[test]
    fn test_batch_all_is_medium() {
        let args = encode_batch_args(&[(
            pallet_index::BALANCES,
            call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        )]);
        let ext = make_ext_with_raw(pallet_index::UTILITY, call_index::UTILITY_BATCH_ALL, args);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::Medium);
        assert!(assessment
            .findings
            .iter()
            .any(|f| f.pattern == CallPattern::BatchAll));
    }

    // -----------------------------------------------------------------------
    // 5. Proxy.proxy dispatch → MEDIUM
    // -----------------------------------------------------------------------
    #[test]
    fn test_proxy_dispatch_is_medium() {
        let args = encode_proxy_args(
            pallet_index::BALANCES,
            call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        );
        let ext = make_ext_with_raw(pallet_index::PROXY, call_index::PROXY_PROXY, args);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::Medium);
        assert!(assessment
            .findings
            .iter()
            .any(|f| f.pattern == CallPattern::Proxy));
    }

    // -----------------------------------------------------------------------
    // 6. Proxy.addProxy (direct) → HIGH
    // -----------------------------------------------------------------------
    #[test]
    fn test_proxy_add_is_high() {
        let ext = make_ext(pallet_index::PROXY, known::PROXY_ADD_PROXY);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::High);
        assert!(assessment
            .findings
            .iter()
            .any(|f| f.pattern == CallPattern::ProxyAddition));
    }

    // -----------------------------------------------------------------------
    // 7. Multisig.asMulti → MEDIUM
    // -----------------------------------------------------------------------
    #[test]
    fn test_multisig_is_medium() {
        let ext = make_ext(known::MULTISIG_PALLET, known::MULTISIG_AS_MULTI);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::Medium);
        assert!(assessment
            .findings
            .iter()
            .any(|f| f.pattern == CallPattern::Multisig));
    }

    // -----------------------------------------------------------------------
    // 8. batch_all containing proxy.addProxy → CRITICAL (batch hiding)
    // -----------------------------------------------------------------------
    #[test]
    fn test_batch_all_with_proxy_add_is_critical() {
        let args = encode_batch_args(&[
            (
                pallet_index::BALANCES,
                call_index::BALANCES_TRANSFER_KEEP_ALIVE,
            ),
            (pallet_index::PROXY, known::PROXY_ADD_PROXY),
        ]);
        let ext = make_ext_with_raw(pallet_index::UTILITY, call_index::UTILITY_BATCH_ALL, args);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::Critical);
        assert!(assessment.should_block());
        assert!(assessment
            .findings
            .iter()
            .any(|f| f.pattern == CallPattern::BatchWithProxyMutation));
    }

    // -----------------------------------------------------------------------
    // 9. Proxy wrapping another proxy → CRITICAL (nested proxy)
    // -----------------------------------------------------------------------
    #[test]
    fn test_nested_proxy_is_critical() {
        // Outer proxy wraps inner proxy.proxy call
        let inner_proxy_args = encode_proxy_args(
            pallet_index::BALANCES,
            call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        );
        // Build the inner proxy call as raw bytes: [pallet=29, call=0, args...]
        let mut inner_call_bytes = vec![pallet_index::PROXY, call_index::PROXY_PROXY];
        inner_call_bytes.extend_from_slice(&inner_proxy_args);

        // Outer proxy args: AccountId + None + length-prefixed inner call
        let mut buf = Vec::new();
        buf.push(0x00);
        buf.extend_from_slice(&[0u8; 32]);
        buf.push(0x00);
        let mut enc = ScaleEncoder::new();
        enc.encode_bytes(&inner_call_bytes);
        buf.extend_from_slice(&enc.finish());

        let ext = make_ext_with_raw(pallet_index::PROXY, call_index::PROXY_PROXY, buf);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::Critical);
        assert!(assessment
            .findings
            .iter()
            .any(|f| f.pattern == CallPattern::NestedProxy));
    }

    // -----------------------------------------------------------------------
    // 10. Unknown pallet call → HIGH
    // -----------------------------------------------------------------------
    #[test]
    fn test_unknown_call_is_high() {
        let ext = make_ext(200, 99);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::High);
        assert!(assessment
            .findings
            .iter()
            .any(|f| f.pattern == CallPattern::UnknownCall));
    }

    // -----------------------------------------------------------------------
    // 11. batch with unknown inner call → HIGH
    // -----------------------------------------------------------------------
    #[test]
    fn test_batch_with_unknown_inner_is_high() {
        let args = encode_batch_args(&[
            (
                pallet_index::BALANCES,
                call_index::BALANCES_TRANSFER_KEEP_ALIVE,
            ),
            (200, 99),
        ]);
        let ext = make_ext_with_raw(pallet_index::UTILITY, call_index::UTILITY_BATCH, args);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::High);
        assert!(assessment
            .findings
            .iter()
            .any(|f| f.pattern == CallPattern::UnknownCall));
    }

    // -----------------------------------------------------------------------
    // 12. Proxy.removeProxy → HIGH
    // -----------------------------------------------------------------------
    #[test]
    fn test_proxy_remove_is_high() {
        let ext = make_ext(pallet_index::PROXY, known::PROXY_REMOVE_PROXY);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::High);
        assert!(assessment
            .findings
            .iter()
            .any(|f| f.pattern == CallPattern::ProxyRemoval));
    }

    // -----------------------------------------------------------------------
    // 13. Multisig.approveAsMulti → MEDIUM
    // -----------------------------------------------------------------------
    #[test]
    fn test_multisig_approval_is_medium() {
        let ext = make_ext(known::MULTISIG_PALLET, known::MULTISIG_APPROVE_AS_MULTI);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::Medium);
        assert!(assessment
            .findings
            .iter()
            .any(|f| f.pattern == CallPattern::MultisigApproval));
    }

    // -----------------------------------------------------------------------
    // 14. Proxy wrapping proxy.addProxy → CRITICAL
    // -----------------------------------------------------------------------
    #[test]
    fn test_proxy_wrapping_add_proxy_is_critical() {
        let args = encode_proxy_args(pallet_index::PROXY, known::PROXY_ADD_PROXY);
        let ext = make_ext_with_raw(pallet_index::PROXY, call_index::PROXY_PROXY, args);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::Critical);
        assert!(assessment.should_block());
    }

    // -----------------------------------------------------------------------
    // 15. RiskLevel ordering
    // -----------------------------------------------------------------------
    #[test]
    fn test_risk_level_ordering() {
        assert!(RiskLevel::Low < RiskLevel::Medium);
        assert!(RiskLevel::Medium < RiskLevel::High);
        assert!(RiskLevel::High < RiskLevel::Critical);
    }

    // -----------------------------------------------------------------------
    // 16. RiskLevel display
    // -----------------------------------------------------------------------
    #[test]
    fn test_risk_level_display() {
        assert_eq!(format!("{}", RiskLevel::Low), "LOW");
        assert_eq!(format!("{}", RiskLevel::Critical), "CRITICAL");
    }

    // -----------------------------------------------------------------------
    // 17. RiskAssessment serialization round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn test_assessment_serde_roundtrip() {
        let ext = make_ext(
            pallet_index::BALANCES,
            call_index::BALANCES_TRANSFER_KEEP_ALIVE,
        );
        let assessment = classify_risk(&ext);
        let json = serde_json::to_string(&assessment).expect("serialize");
        let deser: RiskAssessment = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(assessment, deser);
    }

    // -----------------------------------------------------------------------
    // 18. force_batch → MEDIUM
    // -----------------------------------------------------------------------
    #[test]
    fn test_force_batch_is_medium() {
        let args = encode_batch_args(&[(pallet_index::BALANCES, call_index::BALANCES_TRANSFER)]);
        let ext = make_ext_with_raw(pallet_index::UTILITY, call_index::UTILITY_FORCE_BATCH, args);
        let assessment = classify_risk(&ext);
        assert_eq!(assessment.overall_level, RiskLevel::Medium);
        assert!(assessment
            .findings
            .iter()
            .any(|f| f.pattern == CallPattern::ForceBatch));
    }
}

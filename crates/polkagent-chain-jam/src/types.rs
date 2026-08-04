//! JAM-specific block and header types.
//!
//! These types model the JAM block structure as described in the Graypaper.
//! They are intentionally separate from the Substrate/Polkadot `BlockRef` and
//! `ChainProfile` types — JAM uses a different block model.

use serde::{Deserialize, Serialize};

/// A JAM service identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JamServiceId(pub u32);

/// A JAM block header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JamBlockHeader {
    /// Slot number (JAM uses slot-based consensus).
    pub slot: u64,
    /// Parent block hash (hex-encoded).
    pub parent_hash: String,
    /// State root after block execution (hex-encoded).
    pub state_root: String,
    /// Extrinsic root (hex-encoded).
    pub extrinsic_root: String,
}

/// A JAM block containing header and work report summaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JamBlock {
    /// Block hash (hex-encoded).
    pub hash: String,
    /// Block header.
    pub header: JamBlockHeader,
    /// Service IDs that had work reports included in this block.
    pub work_report_service_ids: Vec<JamServiceId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jam_block_header_roundtrips_json() {
        let header = JamBlockHeader {
            slot: 42,
            parent_hash: "0xabc123".into(),
            state_root: "0xdef456".into(),
            extrinsic_root: "0x789012".into(),
        };
        let json = serde_json::to_string(&header).expect("serialize");
        let parsed: JamBlockHeader = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(header, parsed);
    }

    #[test]
    fn jam_block_roundtrips_json() {
        let block = JamBlock {
            hash: "0xblockhash".into(),
            header: JamBlockHeader {
                slot: 100,
                parent_hash: "0xparent".into(),
                state_root: "0xstate".into(),
                extrinsic_root: "0xext".into(),
            },
            work_report_service_ids: vec![JamServiceId(1), JamServiceId(42)],
        };
        let json = serde_json::to_string(&block).expect("serialize");
        let parsed: JamBlock = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(block, parsed);
    }

    #[test]
    fn jam_block_empty_work_reports() {
        let block = JamBlock {
            hash: "0xempty".into(),
            header: JamBlockHeader {
                slot: 0,
                parent_hash: "0x00".into(),
                state_root: "0x00".into(),
                extrinsic_root: "0x00".into(),
            },
            work_report_service_ids: vec![],
        };
        let json = serde_json::to_string(&block).expect("serialize");
        assert!(json.contains("\"work_report_service_ids\":[]"));
        let parsed: JamBlock = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(block, parsed);
    }

    #[test]
    fn jam_service_id_equality() {
        let a = JamServiceId(10);
        let b = JamServiceId(10);
        let c = JamServiceId(20);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}

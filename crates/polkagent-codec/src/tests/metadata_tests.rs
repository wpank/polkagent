//! Tests for metadata parsing.

use crate::error::CodecError;
use crate::metadata::{build_minimal_metadata_v14, parse_metadata_v14};

#[test]
fn parse_minimal_v14_no_pallets() {
    let bytes = build_minimal_metadata_v14(&[]);
    let meta = parse_metadata_v14(&bytes).expect("parse v14 empty");
    assert_eq!(meta.version, 14);
    assert!(meta.pallets.is_empty());
    assert!(meta.types.is_empty());
}

#[test]
fn parse_minimal_v14_with_pallets() {
    let pallets = [("System", 0u8), ("Balances", 5), ("Utility", 24)];
    let bytes = build_minimal_metadata_v14(&pallets);
    let meta = parse_metadata_v14(&bytes).expect("parse v14 with pallets");
    assert_eq!(meta.version, 14);
    assert_eq!(meta.pallets.len(), 3);

    let sys = meta.pallet_by_name("System").expect("System pallet");
    assert_eq!(sys.index, 0);

    let bal = meta.pallet_by_index(5).expect("Balances pallet");
    assert_eq!(bal.name, "Balances");

    let util = meta.pallet_by_name("Utility").expect("Utility pallet");
    assert_eq!(util.index, 24);
}

#[test]
fn parse_metadata_wrong_magic() {
    let bad = b"NOPE\x38\x00".to_vec();
    let err = parse_metadata_v14(&bad).expect_err("should fail on bad magic");
    assert!(matches!(err, CodecError::DecodeError { .. }));
}

#[test]
fn parse_metadata_wrong_version() {
    // Build v14 metadata then ask for v15.
    let bytes = build_minimal_metadata_v14(&[]);
    let err =
        crate::metadata::parse_metadata_v15(&bytes).expect_err("should fail on wrong version");
    assert!(matches!(err, CodecError::UnsupportedVersion { .. }));
}

#[test]
fn parse_metadata_truncated() {
    // Only the magic bytes, nothing more.
    let bytes = b"meta".to_vec();
    let err = parse_metadata_v14(&bytes).expect_err("truncated should fail");
    assert!(
        err.to_string().contains("unexpected")
            || err.to_string().contains("decode")
            || err.to_string().contains("unsupported")
    );
}

#[test]
fn pallet_by_index_not_found() {
    let bytes = build_minimal_metadata_v14(&[("System", 0)]);
    let meta = parse_metadata_v14(&bytes).expect("parse");
    assert!(meta.pallet_by_index(99).is_none());
}

#[test]
fn pallet_by_name_not_found() {
    let bytes = build_minimal_metadata_v14(&[("System", 0)]);
    let meta = parse_metadata_v14(&bytes).expect("parse");
    assert!(meta.pallet_by_name("DoesNotExist").is_none());
}

#[test]
fn call_by_indices_no_calls() {
    let bytes = build_minimal_metadata_v14(&[("System", 0)]);
    let meta = parse_metadata_v14(&bytes).expect("parse");
    // No calls registered for the pallet (None).
    assert!(meta.call_by_indices(0, 0).is_none());
}

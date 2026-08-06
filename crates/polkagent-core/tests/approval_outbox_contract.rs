//! Executable APR-08 snapshot for approval coordinator outbox payloads.

#![allow(
    clippy::expect_used,
    reason = "the static outbox fixture stops at the exact serialization drift"
)]

use polkagent_core::event::EventKind;
use serde::{Deserialize, Serialize};

const FIXTURE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalOutboxSnapshot {
    fixture_schema_version: u32,
    events: Vec<EventKind>,
}

fn expected_snapshot() -> ApprovalOutboxSnapshot {
    ApprovalOutboxSnapshot {
        fixture_schema_version: FIXTURE_SCHEMA_VERSION,
        events: vec![
            EventKind::ApprovalRequested {
                request_id: "approval-0001".to_owned(),
            },
            EventKind::ApprovalGranted {
                approval_id: "approval-0001".to_owned(),
            },
            EventKind::ApprovalDenied {
                reason: "approval rejected".to_owned(),
            },
            EventKind::RunTimedOut,
            EventKind::RunCancelled {
                reason: "approval cancelled".to_owned(),
            },
            EventKind::EffectsResolved,
        ],
    }
}

fn decode_snapshot(input: &str) -> Result<ApprovalOutboxSnapshot, String> {
    let value: serde_json::Value =
        serde_json::from_str(input).map_err(|error| error.to_string())?;
    let version = value
        .get("fixture_schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "fixture schema version is missing".to_owned())?;
    if version != u64::from(FIXTURE_SCHEMA_VERSION) {
        return Err(format!("unsupported fixture schema version {version}"));
    }
    serde_json::from_value(value).map_err(|error| error.to_string())
}

#[test]
fn approval_outbox_snapshot_has_exact_payloads_and_round_trips() {
    let fixture = include_str!("fixtures/approval_outbox_events_v1.json");
    let decoded = decode_snapshot(fixture).expect("decode outbox fixture");
    assert_eq!(decoded, expected_snapshot());
    assert_eq!(
        serde_json::to_value(&decoded).expect("serialize outbox fixture"),
        serde_json::from_str::<serde_json::Value>(fixture).expect("parse outbox fixture")
    );
}

#[test]
fn approval_outbox_snapshot_rejects_unknown_event_and_fixture_version() {
    assert!(serde_json::from_str::<EventKind>(r#""approval_reconciled""#).is_err());
    assert!(serde_json::from_str::<EventKind>(
        r#"{"approval_reconciled":{"approval_id":"approval-0001"}}"#
    )
    .is_err());

    let future = include_str!("fixtures/approval_outbox_events_v1.json").replacen(
        "\"fixture_schema_version\": 1",
        "\"fixture_schema_version\": 2",
        1,
    );
    assert!(decode_snapshot(&future).is_err());
}

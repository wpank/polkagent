//! Executable APR-08 snapshots for approval state and store-error contracts.

#![allow(
    clippy::expect_used,
    reason = "static contract fixtures stop at the exact serialization drift"
)]

use polkagent_store_trait::approval::{
    ApprovalDecision, ApprovalPrincipalType, ApprovalStatus, ApprovalStoreError,
    CheckpointCommitStatus, CheckpointEffectStatus, CheckpointStatus, ResolveDisposition,
};
use polkagent_store_trait::StoreRetryClass;
use serde::{Deserialize, Serialize};

const FIXTURE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalStateSnapshot {
    fixture_schema_version: u32,
    approval_statuses: Vec<ApprovalStatus>,
    approval_decisions: Vec<ApprovalDecision>,
    principal_types: Vec<ApprovalPrincipalType>,
    checkpoint_effect_statuses: Vec<CheckpointEffectStatus>,
    checkpoint_statuses: Vec<CheckpointStatus>,
    checkpoint_commit_statuses: Vec<CheckpointCommitStatus>,
    resolve_dispositions: Vec<ResolveDisposition>,
    retry_classes: Vec<StoreRetryClass>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovalErrorSnapshot {
    fixture_schema_version: u32,
    errors: Vec<ApprovalStoreError>,
}

fn decode_versioned<T>(input: &str) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
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

fn expected_states() -> ApprovalStateSnapshot {
    ApprovalStateSnapshot {
        fixture_schema_version: FIXTURE_SCHEMA_VERSION,
        approval_statuses: vec![
            ApprovalStatus::Pending,
            ApprovalStatus::Approved,
            ApprovalStatus::Denied,
            ApprovalStatus::Expired,
            ApprovalStatus::Cancelled,
        ],
        approval_decisions: vec![
            ApprovalDecision::AllowOnce,
            ApprovalDecision::RejectOnce,
            ApprovalDecision::Expire,
            ApprovalDecision::Cancel,
        ],
        principal_types: vec![
            ApprovalPrincipalType::Human,
            ApprovalPrincipalType::Service,
            ApprovalPrincipalType::Quorum,
        ],
        checkpoint_effect_statuses: vec![
            CheckpointEffectStatus::AwaitingApproval,
            CheckpointEffectStatus::Approved,
            CheckpointEffectStatus::Denied,
            CheckpointEffectStatus::Expired,
            CheckpointEffectStatus::Cancelled,
        ],
        checkpoint_statuses: vec![
            CheckpointStatus::PausedForApproval,
            CheckpointStatus::Resumable,
            CheckpointStatus::Leased,
            CheckpointStatus::Terminal,
        ],
        checkpoint_commit_statuses: vec![
            CheckpointCommitStatus::Resumable,
            CheckpointCommitStatus::Terminal,
        ],
        resolve_dispositions: vec![
            ResolveDisposition::Applied,
            ResolveDisposition::AlreadyApplied,
        ],
        retry_classes: vec![
            StoreRetryClass::Idempotent,
            StoreRetryClass::CheckBeforeRetry,
            StoreRetryClass::NoAutoRetry,
        ],
    }
}

fn expected_errors() -> ApprovalErrorSnapshot {
    ApprovalErrorSnapshot {
        fixture_schema_version: FIXTURE_SCHEMA_VERSION,
        errors: vec![
            ApprovalStoreError::NotFound {
                resource_type: "Approval".to_owned(),
                id: "approval-0001".to_owned(),
            },
            ApprovalStoreError::Conflict {
                resource_type: "Approval".to_owned(),
                id: "approval-0001".to_owned(),
                message: "a different decision already won".to_owned(),
            },
            ApprovalStoreError::InvalidTransition {
                resource_type: "Effect".to_owned(),
                id: "effect-0001".to_owned(),
                expected: "approved".to_owned(),
                actual: "executing".to_owned(),
            },
            ApprovalStoreError::ScopeMismatch,
            ApprovalStoreError::DigestMismatch,
            ApprovalStoreError::DeadlineElapsed,
            ApprovalStoreError::UnsupportedCheckpointVersion { version: 2 },
            ApprovalStoreError::Integrity {
                message: "checkpoint digest mismatch".to_owned(),
            },
            ApprovalStoreError::Backend {
                message: "backend unavailable".to_owned(),
            },
        ],
    }
}

fn assert_known_state_variants(snapshot: &ApprovalStateSnapshot) {
    for status in &snapshot.approval_statuses {
        match status {
            ApprovalStatus::Pending
            | ApprovalStatus::Approved
            | ApprovalStatus::Denied
            | ApprovalStatus::Expired
            | ApprovalStatus::Cancelled => {}
        }
    }
    for decision in &snapshot.approval_decisions {
        match decision {
            ApprovalDecision::AllowOnce
            | ApprovalDecision::RejectOnce
            | ApprovalDecision::Expire
            | ApprovalDecision::Cancel => {}
        }
    }
    for principal_type in &snapshot.principal_types {
        match principal_type {
            ApprovalPrincipalType::Human
            | ApprovalPrincipalType::Service
            | ApprovalPrincipalType::Quorum => {}
        }
    }
    for status in &snapshot.checkpoint_effect_statuses {
        match status {
            CheckpointEffectStatus::AwaitingApproval
            | CheckpointEffectStatus::Approved
            | CheckpointEffectStatus::Denied
            | CheckpointEffectStatus::Expired
            | CheckpointEffectStatus::Cancelled => {}
        }
    }
    for status in &snapshot.checkpoint_statuses {
        match status {
            CheckpointStatus::PausedForApproval
            | CheckpointStatus::Resumable
            | CheckpointStatus::Leased
            | CheckpointStatus::Terminal => {}
        }
    }
    for status in &snapshot.checkpoint_commit_statuses {
        match status {
            CheckpointCommitStatus::Resumable | CheckpointCommitStatus::Terminal => {}
        }
    }
    for disposition in &snapshot.resolve_dispositions {
        match disposition {
            ResolveDisposition::Applied | ResolveDisposition::AlreadyApplied => {}
        }
    }
    for retry_class in &snapshot.retry_classes {
        match retry_class {
            StoreRetryClass::Idempotent
            | StoreRetryClass::CheckBeforeRetry
            | StoreRetryClass::NoAutoRetry => {}
        }
    }
}

fn assert_known_error_variants(snapshot: &ApprovalErrorSnapshot) {
    for error in &snapshot.errors {
        match error {
            ApprovalStoreError::NotFound { .. }
            | ApprovalStoreError::Conflict { .. }
            | ApprovalStoreError::InvalidTransition { .. }
            | ApprovalStoreError::ScopeMismatch
            | ApprovalStoreError::DigestMismatch
            | ApprovalStoreError::DeadlineElapsed
            | ApprovalStoreError::UnsupportedCheckpointVersion { .. }
            | ApprovalStoreError::Integrity { .. }
            | ApprovalStoreError::Backend { .. } => {}
        }
    }
}

#[test]
fn approval_state_snapshot_is_exhaustive_and_round_trips() {
    let fixture = include_str!("fixtures/approval_state_v1.json");
    let decoded: ApprovalStateSnapshot = decode_versioned(fixture).expect("decode state fixture");
    assert_known_state_variants(&decoded);
    let expected = expected_states();
    assert_eq!(decoded, expected);
    assert_eq!(
        serde_json::to_value(&decoded).expect("serialize state fixture"),
        serde_json::from_str::<serde_json::Value>(fixture).expect("parse state fixture")
    );
}

#[test]
fn approval_state_snapshot_rejects_unknown_state_and_fixture_version() {
    assert!(serde_json::from_str::<ApprovalStatus>(r#""future_status""#).is_err());
    assert!(serde_json::from_str::<ApprovalDecision>(r#""future_decision""#).is_err());
    assert!(serde_json::from_str::<CheckpointEffectStatus>(r#""future_effect""#).is_err());
    assert!(serde_json::from_str::<CheckpointStatus>(r#""future_checkpoint""#).is_err());
    assert!(serde_json::from_str::<StoreRetryClass>(r#""future_retry""#).is_err());

    let future = include_str!("fixtures/approval_state_v1.json").replacen(
        "\"fixture_schema_version\": 1",
        "\"fixture_schema_version\": 2",
        1,
    );
    assert!(decode_versioned::<ApprovalStateSnapshot>(&future).is_err());
}

#[test]
fn approval_error_snapshot_is_exhaustive_and_round_trips() {
    let fixture = include_str!("fixtures/approval_error_v1.json");
    let decoded: ApprovalErrorSnapshot = decode_versioned(fixture).expect("decode error fixture");
    assert_known_error_variants(&decoded);
    let expected = expected_errors();
    assert_eq!(decoded, expected);
    assert_eq!(
        serde_json::to_value(&decoded).expect("serialize error fixture"),
        serde_json::from_str::<serde_json::Value>(fixture).expect("parse error fixture")
    );
}

#[test]
fn approval_error_snapshot_rejects_unknown_error_and_fixture_version() {
    assert!(serde_json::from_str::<ApprovalStoreError>(r#"{"error":"future_error"}"#).is_err());
    let future = include_str!("fixtures/approval_error_v1.json").replacen(
        "\"fixture_schema_version\": 1",
        "\"fixture_schema_version\": 2",
        1,
    );
    assert!(decode_versioned::<ApprovalErrorSnapshot>(&future).is_err());
}

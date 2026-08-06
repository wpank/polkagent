//! Evidence for the pre-schema execution-plan serialization freeze gate.
//!
//! The current serde fixture is diagnostic and must not be used as a durable
//! storage contract. The proposed v1 fixture records the normalization shape
//! that ADR-003 requires before an execution ledger can be migrated.

#![allow(clippy::expect_used)]

use polkagent_core::ids::AgentId;
use polkagent_group::{ExecutionMode, ExecutionPlan, GrantSpec, GroupTask, TaskId};
use serde_json::{json, Value};
use uuid::Uuid;

fn agent_id(value: &str) -> AgentId {
    AgentId::from_uuid(Uuid::parse_str(value).expect("valid fixture UUID"))
}

fn audited_plan() -> ExecutionPlan {
    let researcher = GroupTask::new(
        TaskId::new(10),
        agent_id("018f1000-0000-7000-8000-000000000001"),
    )
    .with_input(json!({"query": "referendum 42", "limit": 2}))
    .with_grant(GrantSpec {
        capabilities: vec!["chain.read".to_string()],
        max_budget: Some(10.5),
        allowed_pallets: vec!["System".to_string()],
    });
    let writer = GroupTask::new(
        TaskId::new(20),
        agent_id("018f1000-0000-7000-8000-000000000002"),
    )
    .with_input(json!({"format": "brief"}));
    let mut plan = ExecutionPlan::new(ExecutionMode::Pipeline)
        .with_task(researcher)
        .with_task(writer);
    plan.add_dependency(TaskId::new(20), TaskId::new(10));
    plan
}

#[test]
fn current_serde_shape_matches_diagnostic_fixture_but_has_no_version_envelope() {
    let actual = serde_json::to_value(audited_plan()).expect("serialize current plan");
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/execution_plan_current_serde.json"))
            .expect("parse current-shape fixture");

    assert_eq!(actual, fixture);
    assert!(actual.get("schema_version").is_none());
    assert!(actual["dependencies"].is_object());
    assert_eq!(actual["dependencies"]["10"], json!([20]));
}

#[test]
fn current_serde_shape_accepts_duplicate_dangling_dependency_edges() {
    let mut plan = audited_plan();
    plan.add_dependency(TaskId::new(999), TaskId::new(10));
    plan.add_dependency(TaskId::new(999), TaskId::new(10));

    let serialized = serde_json::to_value(plan).expect("serde does not validate the graph");
    assert_eq!(serialized["dependencies"]["10"], json!([20, 999, 999]));
}

#[test]
fn proposed_v1_fixture_has_explicit_version_and_normalized_edge_list() {
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/group_execution_plan_v1_proposed.json"
    ))
    .expect("parse proposed v1 fixture");

    assert_eq!(fixture["contract"], "polkagent.group-execution-plan");
    assert_eq!(fixture["schema_version"], 1);
    assert_eq!(fixture["tasks"][0]["ordinal"], 0);
    assert_eq!(fixture["tasks"][1]["ordinal"], 1);
    assert!(fixture["dependency_edges"].is_array());
    assert_eq!(
        fixture["dependency_edges"],
        json!([{"blocked_by": 10, "task_id": 20}])
    );
}

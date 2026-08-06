//! Contract and adversarial tests for the durable execution-plan v1 codec.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "contract tests intentionally fail immediately when static fixtures are invalid"
)]

use std::str;

use polkagent_core::ids::AgentId;
use polkagent_group::{
    compute_execution_plan_digest_v1, decode_execution_plan_v1, encode_execution_plan_v1,
    ExecutionMode, ExecutionPlan, ExecutionPlanCodecError, GrantSpec, GroupTask, TaskId,
    EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES, EXECUTION_PLAN_V1_MAX_DEPENDENCY_EDGES,
    EXECUTION_PLAN_V1_MAX_TASKS, EXECUTION_PLAN_V1_MAX_TASK_INPUT_BYTES,
    EXECUTION_PLAN_V1_MAX_TASK_INPUT_DEPTH,
};
use serde_json::{json, Value};
use uuid::Uuid;

const AGENT_ONE: &str = "018f1000-0000-7000-8000-000000000001";
const AGENT_TWO: &str = "018f1000-0000-7000-8000-000000000002";

fn agent(value: &str) -> AgentId {
    AgentId::from_uuid(Uuid::parse_str(value).expect("valid static agent UUID"))
}

fn task(id: u64, agent_id: &str) -> GroupTask {
    GroupTask::new(TaskId::new(id), agent(agent_id))
}

fn golden_plan() -> ExecutionPlan {
    let first = task(10, AGENT_ONE)
        .with_input(json!({
            "é": "raw UTF-8",
            "a": {"z": 1, "a": i64::MIN},
            "😀": u64::MAX,
            "escape": "\0\u{0001}\u{0008}\u{000c}\n\r\t\"\\"
        }))
        .with_grant(GrantSpec {
            capabilities: vec![
                "z.read".to_string(),
                "a.read".to_string(),
                "z.read".to_string(),
            ],
            max_budget: Some(-0.0),
            allowed_pallets: vec!["é".to_string(), "A".to_string(), "é".to_string()],
        });
    let second = task(u64::MAX, AGENT_TWO).with_input(json!({"stage": "final"}));
    let mut plan = ExecutionPlan::new(ExecutionMode::Pipeline)
        .with_task(first)
        .with_task(second);
    plan.add_dependency(TaskId::new(u64::MAX), TaskId::new(10));
    plan
}

fn decode_modified(original: &str, modified: &str) -> Result<(), ExecutionPlanCodecError> {
    assert_ne!(original, modified);
    let digest = compute_execution_plan_digest_v1(modified.as_bytes());
    decode_execution_plan_v1(modified.as_bytes(), &digest).map(|_| ())
}

fn canonical_text(plan: &ExecutionPlan) -> String {
    String::from_utf8(
        encode_execution_plan_v1(plan)
            .expect("valid plan")
            .canonical_bytes()
            .to_vec(),
    )
    .expect("canonical plan is UTF-8")
}

#[test]
fn exact_canonical_bytes_and_domain_separated_digest_match_golden_fixtures() {
    let frozen = encode_execution_plan_v1(&golden_plan()).expect("golden plan is valid");
    let expected_bytes = include_bytes!("fixtures/group_execution_plan_v1.canonical.json")
        .strip_suffix(b"\n")
        .expect("fixture has exactly one repository newline");
    let expected_digest = include_str!("fixtures/group_execution_plan_v1.blake3")
        .strip_suffix('\n')
        .expect("digest fixture has exactly one repository newline");

    assert_eq!(frozen.canonical_bytes(), expected_bytes);
    assert_eq!(frozen.digest(), expected_digest);
    assert_eq!(
        compute_execution_plan_digest_v1(expected_bytes),
        expected_digest
    );
    let decoded = decode_execution_plan_v1(expected_bytes, expected_digest)
        .expect("golden bytes decode under their pinned digest");
    assert_eq!(decoded.canonical_bytes(), expected_bytes);
}

#[test]
fn full_u64_ids_integer_extremes_and_normalized_lossless_upcast_round_trip() {
    let frozen = encode_execution_plan_v1(&golden_plan()).expect("valid plan");
    assert_eq!(frozen.plan().tasks()[1].task_id().value(), u64::MAX);
    assert_eq!(
        frozen.plan().tasks()[1].task_id_decimal(),
        "18446744073709551615"
    );
    assert!(str::from_utf8(frozen.canonical_bytes())
        .expect("UTF-8")
        .contains("18446744073709551615"));

    let decoded = decode_execution_plan_v1(frozen.canonical_bytes(), frozen.digest())
        .expect("decode exact frozen bytes");
    let upcast = decoded.to_execution_plan().expect("lossless upcast");
    assert_eq!(upcast.mode, ExecutionMode::Pipeline);
    assert_eq!(upcast.tasks.len(), 2);
    assert_eq!(upcast.tasks[0].id, TaskId::new(10));
    assert_eq!(upcast.tasks[1].id, TaskId::new(u64::MAX));
    assert_eq!(upcast.tasks[0].input, golden_plan().tasks[0].input);
    let grant = upcast.tasks[0].grant_spec.as_ref().expect("grant retained");
    assert_eq!(grant.capabilities, vec!["a.read", "z.read"]);
    assert_eq!(grant.allowed_pallets, vec!["A", "é"]);
    assert_eq!(grant.max_budget.map(f64::to_bits), Some(0.0_f64.to_bits()));
    assert_eq!(
        upcast.dependencies[&TaskId::new(10)],
        vec![TaskId::new(u64::MAX)]
    );
}

#[test]
fn canonical_writer_sorts_utf8_keys_escapes_minimally_and_normalizes_grants() {
    let text = canonical_text(&golden_plan());
    assert!(text.contains(
        r#""input":{"a":{"a":-9223372036854775808,"z":1},"escape":"\u0000\u0001\b\f\n\r\t\"\\","é":"raw UTF-8","😀":18446744073709551615}"#
    ));
    assert!(text.contains(
        r#""grant_override":{"allowed_pallets":["A","é"],"capabilities":["a.read","z.read"],"max_budget":"0"}"#
    ));
    assert!(!text.ends_with('\n'));
    assert!(!text.starts_with('\u{feff}'));

    let separators = ExecutionPlan::new(ExecutionMode::Sequential)
        .with_task(task(1, AGENT_ONE).with_input(json!({"separator": "before\u{2028}after"})));
    let separators = canonical_text(&separators);
    assert!(separators.contains("before\u{2028}after"));
    assert!(!separators.contains(r"\u2028"));
}

#[test]
fn input_numbers_must_be_json_integers() {
    let accepted = ExecutionPlan::new(ExecutionMode::Sequential)
        .with_task(task(1, AGENT_ONE).with_input(json!({"min": i64::MIN, "max": u64::MAX})));
    encode_execution_plan_v1(&accepted).expect("full integer domain accepted");

    for value in [
        json!(1.5),
        serde_json::from_str::<Value>("1.0").expect("JSON"),
    ] {
        let rejected = ExecutionPlan::new(ExecutionMode::Sequential)
            .with_task(task(1, AGENT_ONE).with_input(value));
        assert!(matches!(
            encode_execution_plan_v1(&rejected),
            Err(ExecutionPlanCodecError::NonIntegerTaskInput { ordinal: 0 })
        ));
    }

    let original = canonical_text(&golden_plan());
    let fractional = original.replace("\"z\":1", "\"z\":1.5");
    assert!(matches!(
        decode_modified(&original, &fractional),
        Err(ExecutionPlanCodecError::NonIntegerTaskInput { ordinal: 0 })
    ));
    let exponent = original.replace("\"z\":1", "\"z\":1e0");
    assert!(matches!(
        decode_modified(&original, &exponent),
        Err(ExecutionPlanCodecError::NonIntegerTaskInput { ordinal: 0 })
    ));
}

#[test]
fn grant_budgets_reject_negative_and_non_finite_values_and_pin_signed_zero() {
    for budget in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let plan = ExecutionPlan::new(ExecutionMode::Sequential).with_task(
            task(1, AGENT_ONE).with_grant(GrantSpec {
                capabilities: Vec::new(),
                max_budget: Some(budget),
                allowed_pallets: Vec::new(),
            }),
        );
        assert_eq!(
            encode_execution_plan_v1(&plan),
            Err(ExecutionPlanCodecError::InvalidGrantBudget)
        );
    }

    let text = canonical_text(&golden_plan());
    assert!(text.contains("\"max_budget\":\"0\""));
    let negative_zero = text.replace("\"max_budget\":\"0\"", "\"max_budget\":\"-0\"");
    assert_eq!(
        decode_modified(&text, &negative_zero),
        Err(ExecutionPlanCodecError::InvalidGrantBudget)
    );
    let decimal_zero = text.replace("\"max_budget\":\"0\"", "\"max_budget\":\"0.0\"");
    assert_eq!(
        decode_modified(&text, &decimal_zero),
        Err(ExecutionPlanCodecError::InvalidGrantBudget)
    );

    let positive = ExecutionPlan::new(ExecutionMode::Sequential).with_task(
        task(1, AGENT_ONE).with_grant(GrantSpec {
            capabilities: Vec::new(),
            max_budget: Some(10.5),
            allowed_pallets: Vec::new(),
        }),
    );
    assert!(canonical_text(&positive).contains("\"max_budget\":\"10.5\""));
}

#[test]
fn grant_sets_reject_empty_values_and_noncanonical_decoded_order() {
    let plan = ExecutionPlan::new(ExecutionMode::Sequential).with_task(
        task(1, AGENT_ONE).with_grant(GrantSpec {
            capabilities: vec![String::new()],
            max_budget: None,
            allowed_pallets: Vec::new(),
        }),
    );
    assert_eq!(
        encode_execution_plan_v1(&plan),
        Err(ExecutionPlanCodecError::EmptyGrantEntry {
            field: "capabilities"
        })
    );

    let text = canonical_text(&golden_plan());
    let unsorted = text.replace(
        "\"capabilities\":[\"a.read\",\"z.read\"]",
        "\"capabilities\":[\"z.read\",\"a.read\"]",
    );
    assert_eq!(
        decode_modified(&text, &unsorted),
        Err(ExecutionPlanCodecError::NonCanonicalGrantSet {
            field: "capabilities"
        })
    );
}

#[test]
fn rejects_duplicate_task_ids_and_invalid_ordinals() {
    let plan = ExecutionPlan::new(ExecutionMode::Parallel)
        .with_task(task(7, AGENT_ONE))
        .with_task(task(7, AGENT_TWO));
    assert_eq!(
        encode_execution_plan_v1(&plan),
        Err(ExecutionPlanCodecError::DuplicateTaskId(7))
    );

    let text = canonical_text(&golden_plan());
    let duplicate_id = text.replace(
        "\"ordinal\":1,\"task_id\":\"18446744073709551615\"",
        "\"ordinal\":1,\"task_id\":\"10\"",
    );
    assert_eq!(
        decode_modified(&text, &duplicate_id),
        Err(ExecutionPlanCodecError::DuplicateTaskId(10))
    );
    let invalid = text.replace("\"ordinal\":1", "\"ordinal\":0");
    assert_eq!(
        decode_modified(&text, &invalid),
        Err(ExecutionPlanCodecError::InvalidOrdinal {
            position: 1,
            expected: 1,
            actual: 0
        })
    );
}

#[test]
fn rejects_duplicate_dangling_self_and_cyclic_dependencies() {
    let mut duplicate = ExecutionPlan::new(ExecutionMode::Parallel)
        .with_task(task(1, AGENT_ONE))
        .with_task(task(2, AGENT_TWO));
    duplicate.add_dependency(TaskId::new(2), TaskId::new(1));
    duplicate.add_dependency(TaskId::new(2), TaskId::new(1));
    assert_eq!(
        encode_execution_plan_v1(&duplicate),
        Err(ExecutionPlanCodecError::DuplicateDependency {
            blocked_by: 1,
            task_id: 2
        })
    );

    let mut dangling = ExecutionPlan::new(ExecutionMode::Parallel)
        .with_task(task(1, AGENT_ONE))
        .with_task(task(2, AGENT_TWO));
    dangling.add_dependency(TaskId::new(3), TaskId::new(1));
    assert_eq!(
        encode_execution_plan_v1(&dangling),
        Err(ExecutionPlanCodecError::DanglingDependency {
            blocked_by: 1,
            task_id: 3
        })
    );

    let mut self_edge = ExecutionPlan::new(ExecutionMode::Sequential).with_task(task(1, AGENT_ONE));
    self_edge.add_dependency(TaskId::new(1), TaskId::new(1));
    assert_eq!(
        encode_execution_plan_v1(&self_edge),
        Err(ExecutionPlanCodecError::SelfDependency(1))
    );

    let mut cycle = ExecutionPlan::new(ExecutionMode::Parallel)
        .with_task(task(1, AGENT_ONE))
        .with_task(task(2, AGENT_TWO));
    cycle.add_dependency(TaskId::new(2), TaskId::new(1));
    cycle.add_dependency(TaskId::new(1), TaskId::new(2));
    assert_eq!(
        encode_execution_plan_v1(&cycle),
        Err(ExecutionPlanCodecError::DependencyCycle)
    );

    let text = canonical_text(&golden_plan());
    let edge = "{\"blocked_by\":\"10\",\"task_id\":\"18446744073709551615\"}";
    let duplicate_edge = text.replace(edge, &format!("{edge},{edge}"));
    assert_eq!(
        decode_modified(&text, &duplicate_edge),
        Err(ExecutionPlanCodecError::DuplicateDependency {
            blocked_by: 10,
            task_id: u64::MAX
        })
    );
}

#[test]
fn stable_task_edge_input_plan_and_depth_limits_fail_closed() {
    let mut too_many_tasks = ExecutionPlan::new(ExecutionMode::Parallel);
    for id in 0..=EXECUTION_PLAN_V1_MAX_TASKS {
        too_many_tasks = too_many_tasks.with_task(task(id as u64, AGENT_ONE));
    }
    assert!(matches!(
        encode_execution_plan_v1(&too_many_tasks),
        Err(ExecutionPlanCodecError::TooManyTasks { actual, maximum })
            if actual == EXECUTION_PLAN_V1_MAX_TASKS + 1 && maximum == EXECUTION_PLAN_V1_MAX_TASKS
    ));

    let mut too_many_edges = ExecutionPlan::new(ExecutionMode::Parallel);
    for id in 0..200_u64 {
        too_many_edges = too_many_edges.with_task(task(id, AGENT_ONE));
    }
    let mut count = 0_usize;
    'outer: for blocked_by in 0..200_u64 {
        for task_id in (blocked_by + 1)..200_u64 {
            too_many_edges.add_dependency(TaskId::new(task_id), TaskId::new(blocked_by));
            count += 1;
            if count > EXECUTION_PLAN_V1_MAX_DEPENDENCY_EDGES {
                break 'outer;
            }
        }
    }
    assert_eq!(count, EXECUTION_PLAN_V1_MAX_DEPENDENCY_EDGES + 1);
    assert!(matches!(
        encode_execution_plan_v1(&too_many_edges),
        Err(ExecutionPlanCodecError::TooManyDependencyEdges { actual, maximum })
            if actual == count && maximum == EXECUTION_PLAN_V1_MAX_DEPENDENCY_EDGES
    ));

    let exact_input = "x".repeat(EXECUTION_PLAN_V1_MAX_TASK_INPUT_BYTES - 2);
    let exact_input_plan = ExecutionPlan::new(ExecutionMode::Sequential)
        .with_task(task(1, AGENT_ONE).with_input(Value::String(exact_input)));
    encode_execution_plan_v1(&exact_input_plan).expect("exact input byte limit is accepted");

    let oversized_input = "x".repeat(EXECUTION_PLAN_V1_MAX_TASK_INPUT_BYTES - 1);
    let input_plan = ExecutionPlan::new(ExecutionMode::Sequential)
        .with_task(task(1, AGENT_ONE).with_input(Value::String(oversized_input)));
    assert!(matches!(
        encode_execution_plan_v1(&input_plan),
        Err(ExecutionPlanCodecError::TaskInputTooLarge { ordinal: 0, .. })
    ));

    let mut exact_depth = Value::Null;
    for _ in 0..EXECUTION_PLAN_V1_MAX_TASK_INPUT_DEPTH {
        exact_depth = Value::Array(vec![exact_depth]);
    }
    let exact_depth_plan = ExecutionPlan::new(ExecutionMode::Sequential)
        .with_task(task(1, AGENT_ONE).with_input(exact_depth));
    encode_execution_plan_v1(&exact_depth_plan).expect("exact JSON depth limit is accepted");

    let mut too_deep = Value::Null;
    for _ in 0..=EXECUTION_PLAN_V1_MAX_TASK_INPUT_DEPTH {
        too_deep = Value::Array(vec![too_deep]);
    }
    let deep_plan = ExecutionPlan::new(ExecutionMode::Sequential)
        .with_task(task(1, AGENT_ONE).with_input(too_deep));
    assert_eq!(
        encode_execution_plan_v1(&deep_plan),
        Err(ExecutionPlanCodecError::TaskInputTooDeep {
            ordinal: 0,
            maximum: EXECUTION_PLAN_V1_MAX_TASK_INPUT_DEPTH
        })
    );

    let full_input = "x".repeat(EXECUTION_PLAN_V1_MAX_TASK_INPUT_BYTES - 2);
    let mut exact_plan = ExecutionPlan::new(ExecutionMode::Parallel);
    for id in 0..3_u64 {
        exact_plan =
            exact_plan.with_task(task(id, AGENT_ONE).with_input(Value::String(full_input.clone())));
    }
    exact_plan = exact_plan.with_task(task(3, AGENT_ONE).with_input(Value::String(String::new())));
    let baseline_length = encode_execution_plan_v1(&exact_plan)
        .expect("baseline remains below plan limit")
        .canonical_bytes()
        .len();
    let remaining = EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES - baseline_length;
    assert!(remaining < EXECUTION_PLAN_V1_MAX_TASK_INPUT_BYTES - 2);
    exact_plan.tasks[3].input = Value::String("x".repeat(remaining));
    let exactly_frozen =
        encode_execution_plan_v1(&exact_plan).expect("exact complete-plan byte limit is accepted");
    assert_eq!(
        exactly_frozen.canonical_bytes().len(),
        EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES
    );

    exact_plan.tasks[3].input = Value::String("x".repeat(remaining + 1));
    assert!(matches!(
        encode_execution_plan_v1(&exact_plan),
        Err(ExecutionPlanCodecError::PlanTooLarge { actual, maximum })
            if actual == EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES + 1
                && maximum == EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES
    ));
}

#[test]
fn consensus_requires_two_tasks_while_other_modes_allow_one() {
    let consensus = ExecutionPlan::new(ExecutionMode::Consensus).with_task(task(1, AGENT_ONE));
    assert_eq!(
        encode_execution_plan_v1(&consensus),
        Err(ExecutionPlanCodecError::ConsensusRequiresTwoTasks)
    );
    for mode in [
        ExecutionMode::Sequential,
        ExecutionMode::Parallel,
        ExecutionMode::Pipeline,
    ] {
        encode_execution_plan_v1(&ExecutionPlan::new(mode).with_task(task(1, AGENT_ONE)))
            .expect("one task is valid outside consensus mode");
    }
}

#[test]
fn decoder_detects_duplicate_json_keys_before_value_construction() {
    let text = canonical_text(&golden_plan());
    let duplicate = text.replacen(
        "\"contract\":\"polkagent.group-execution-plan\"",
        "\"contract\":\"polkagent.group-execution-plan\",\"contract\":\"polkagent.group-execution-plan\"",
        1,
    );
    let error = decode_modified(&text, &duplicate).expect_err("duplicate key must fail");
    assert_eq!(error, ExecutionPlanCodecError::DuplicateJsonKey);

    let nested = text.replacen(
        "\"stage\":\"final\"",
        "\"sensitive-user-key\":\"first\",\"sensitive-user-key\":\"second\",\"stage\":\"final\"",
        1,
    );
    let error = decode_modified(&text, &nested).expect_err("nested duplicate key must fail");
    assert_eq!(error, ExecutionPlanCodecError::DuplicateJsonKey);
    assert!(!error.to_string().contains("sensitive-user-key"));
}

#[test]
fn decoder_rejects_digest_tampering_unknown_versions_and_noncanonical_bytes() {
    let frozen = encode_execution_plan_v1(&golden_plan()).expect("valid plan");
    let mut tampered_digest = frozen.digest().to_string();
    tampered_digest.replace_range(10..11, "0");
    if tampered_digest == frozen.digest() {
        tampered_digest.replace_range(10..11, "1");
    }
    assert_eq!(
        decode_execution_plan_v1(frozen.canonical_bytes(), &tampered_digest),
        Err(ExecutionPlanCodecError::DigestMismatch)
    );
    assert_eq!(
        decode_execution_plan_v1(b"{", frozen.digest()),
        Err(ExecutionPlanCodecError::DigestMismatch),
        "digest verification must happen before JSON decoding"
    );
    assert_eq!(
        decode_execution_plan_v1(frozen.canonical_bytes(), "blake3-v1:ABC"),
        Err(ExecutionPlanCodecError::InvalidDigestFormat)
    );

    let text = str::from_utf8(frozen.canonical_bytes()).expect("UTF-8");
    let future = text.replace("\"schema_version\":1", "\"schema_version\":2");
    assert_eq!(
        decode_modified(text, &future),
        Err(ExecutionPlanCodecError::UnsupportedVersion(2))
    );
    let unknown_contract = text.replace(
        "polkagent.group-execution-plan",
        "sensitive-user-contract-value",
    );
    let contract_error =
        decode_modified(text, &unknown_contract).expect_err("unknown contract must fail");
    assert_eq!(contract_error, ExecutionPlanCodecError::UnsupportedContract);
    assert!(!contract_error
        .to_string()
        .contains("sensitive-user-contract-value"));

    let with_newline = format!("{text}\n");
    assert_eq!(
        decode_modified(text, &with_newline),
        Err(ExecutionPlanCodecError::NonCanonicalBytes)
    );
    let with_space = text.replacen(",\"dependency_edges\"", ", \"dependency_edges\"", 1);
    assert_eq!(
        decode_modified(text, &with_space),
        Err(ExecutionPlanCodecError::NonCanonicalBytes)
    );
    let reordered_root = text.replacen(
        "{\"contract\":\"polkagent.group-execution-plan\",\"dependency_edges\":",
        "{\"dependency_edges\":",
        1,
    );
    let reordered_root = reordered_root.replacen(
        "],\"mode\":\"pipeline\"",
        "],\"contract\":\"polkagent.group-execution-plan\",\"mode\":\"pipeline\"",
        1,
    );
    assert_eq!(
        decode_modified(text, &reordered_root),
        Err(ExecutionPlanCodecError::NonCanonicalBytes)
    );
    let mut with_bom = Vec::from([0xef, 0xbb, 0xbf]);
    with_bom.extend_from_slice(frozen.canonical_bytes());
    let bom_digest = compute_execution_plan_digest_v1(&with_bom);
    assert!(matches!(
        decode_execution_plan_v1(&with_bom, &bom_digest),
        Err(ExecutionPlanCodecError::InvalidJson { .. }
            | ExecutionPlanCodecError::NonCanonicalBytes)
    ));
    let unversioned = br#"{"dependencies":{},"mode":"sequential","tasks":[]}"#;
    let digest = compute_execution_plan_digest_v1(unversioned);
    assert_eq!(
        decode_execution_plan_v1(unversioned, &digest),
        Err(ExecutionPlanCodecError::InvalidEnvelopeField("contract"))
    );
}

#[test]
fn decoder_rejects_noncanonical_ids_edge_order_and_raw_size_before_parsing() {
    let frozen = encode_execution_plan_v1(&golden_plan()).expect("valid plan");
    let text = str::from_utf8(frozen.canonical_bytes()).expect("UTF-8");
    let leading_zero = text.replacen("\"task_id\":\"10\"", "\"task_id\":\"010\"", 1);
    assert_eq!(
        decode_modified(text, &leading_zero),
        Err(ExecutionPlanCodecError::InvalidTaskId)
    );
    for invalid_id in ["+10", "00", "18446744073709551616"] {
        let invalid = text.replacen(
            "\"task_id\":\"10\"",
            &format!("\"task_id\":\"{invalid_id}\""),
            1,
        );
        assert_eq!(
            decode_modified(text, &invalid),
            Err(ExecutionPlanCodecError::InvalidTaskId)
        );
    }
    let sensitive_id = text.replacen(
        "\"task_id\":\"10\"",
        "\"task_id\":\"sensitive-user-task-id\"",
        1,
    );
    let id_error = decode_modified(text, &sensitive_id).expect_err("invalid ID must fail");
    assert_eq!(id_error, ExecutionPlanCodecError::InvalidTaskId);
    assert!(!id_error.to_string().contains("sensitive-user-task-id"));

    let bytes = vec![b' '; EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES + 1];
    assert!(matches!(
        decode_execution_plan_v1(&bytes, &compute_execution_plan_digest_v1(&bytes)),
        Err(ExecutionPlanCodecError::PlanTooLarge { actual, maximum })
            if actual == bytes.len() && maximum == EXECUTION_PLAN_V1_MAX_CANONICAL_BYTES
    ));
}

#[test]
fn decoder_rejects_unknown_and_missing_fields_including_absent_option_fields() {
    let text = canonical_text(&golden_plan());
    let unknown_root = text.replacen(
        "\"dependency_edges\":",
        "\"extra\":null,\"dependency_edges\":",
        1,
    );
    assert_eq!(
        decode_modified(&text, &unknown_root),
        Err(ExecutionPlanCodecError::InvalidStructure)
    );

    let unknown_task = text.replacen(
        "\"grant_override\":",
        "\"extra\":null,\"grant_override\":",
        1,
    );
    assert_eq!(
        decode_modified(&text, &unknown_task),
        Err(ExecutionPlanCodecError::InvalidStructure)
    );

    let unknown_grant = text.replacen(
        "\"max_budget\":\"0\"",
        "\"extra\":null,\"max_budget\":\"0\"",
        1,
    );
    assert_eq!(
        decode_modified(&text, &unknown_grant),
        Err(ExecutionPlanCodecError::InvalidStructure)
    );

    let unknown_edge = text.replacen(
        "\"blocked_by\":\"10\"",
        "\"blocked_by\":\"10\",\"extra\":null",
        1,
    );
    assert_eq!(
        decode_modified(&text, &unknown_edge),
        Err(ExecutionPlanCodecError::InvalidStructure)
    );

    let missing_mode = text.replace("\"mode\":\"pipeline\",", "");
    assert_eq!(
        decode_modified(&text, &missing_mode),
        Err(ExecutionPlanCodecError::InvalidStructure)
    );

    let missing_task_id = text.replacen("\"task_id\":\"10\"", "", 1);
    let missing_task_id = missing_task_id.replace(",}", "}");
    assert_eq!(
        decode_modified(&text, &missing_task_id),
        Err(ExecutionPlanCodecError::InvalidStructure)
    );

    let missing_grant_override = text.replacen("\"grant_override\":null,", "", 1);
    assert_eq!(
        decode_modified(&text, &missing_grant_override),
        Err(ExecutionPlanCodecError::NonCanonicalBytes),
        "serde Option defaults to None, but re-freeze restores the required null field"
    );

    let missing_max_budget = text.replacen("\"max_budget\":\"0\"", "", 1);
    let missing_max_budget = missing_max_budget.replace(",}", "}");
    assert_eq!(
        decode_modified(&text, &missing_max_budget),
        Err(ExecutionPlanCodecError::NonCanonicalBytes),
        "serde Option defaults to None, but re-freeze restores the required null field"
    );
}

#[test]
fn decoder_rejects_noncanonical_dependency_order() {
    let mut plan = ExecutionPlan::new(ExecutionMode::Parallel)
        .with_task(task(1, AGENT_ONE))
        .with_task(task(2, AGENT_ONE))
        .with_task(task(3, AGENT_TWO));
    plan.add_dependency(TaskId::new(3), TaskId::new(2));
    plan.add_dependency(TaskId::new(3), TaskId::new(1));
    let text = canonical_text(&plan);
    let canonical_edges = concat!(
        "[{\"blocked_by\":\"1\",\"task_id\":\"3\"},",
        "{\"blocked_by\":\"2\",\"task_id\":\"3\"}]"
    );
    let reversed_edges = concat!(
        "[{\"blocked_by\":\"2\",\"task_id\":\"3\"},",
        "{\"blocked_by\":\"1\",\"task_id\":\"3\"}]"
    );
    assert!(text.contains(canonical_edges));
    assert_eq!(
        decode_modified(&text, &text.replace(canonical_edges, reversed_edges)),
        Err(ExecutionPlanCodecError::NonCanonicalBytes)
    );
}

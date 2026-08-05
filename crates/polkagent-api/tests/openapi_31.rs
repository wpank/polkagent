//! `OpenAPI` 3.1 dialect regression checks.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "contract fixtures fail at the exact schema pointer that regresses"
)]

use std::collections::BTreeSet;

use serde_json::Value;

const OPENAPI_SOURCE: &str = include_str!("../../../openapi.yaml");

fn assert_no_legacy_nullable(value: &Value, pointer: &str) {
    match value {
        Value::Object(object) => {
            assert!(
                !object.contains_key("nullable"),
                "legacy OpenAPI nullable keyword at {pointer}"
            );
            for (key, nested) in object {
                assert_no_legacy_nullable(nested, &format!("{pointer}/{key}"));
            }
        }
        Value::Array(values) => {
            for (index, nested) in values.iter().enumerate() {
                assert_no_legacy_nullable(nested, &format!("{pointer}/{index}"));
            }
        }
        _ => {}
    }
}

fn schema<'a>(root: &'a Value, pointer: &str) -> &'a Value {
    root.pointer(pointer)
        .unwrap_or_else(|| panic!("missing representative schema at {pointer}"))
}

fn assert_primitive_null_union(schema: &Value, primitive: &str) {
    let types = schema["type"]
        .as_array()
        .expect("nullable primitive uses an OpenAPI 3.1 type array")
        .iter()
        .map(|value| value.as_str().expect("type array contains strings"))
        .collect::<BTreeSet<_>>();
    assert_eq!(types, BTreeSet::from([primitive, "null"]));
}

fn assert_reference_null_union(schema: &Value, reference: &str) {
    let variants = schema["anyOf"]
        .as_array()
        .expect("nullable reference uses explicit OpenAPI 3.1 union composition");
    assert_eq!(variants.len(), 2);
    assert!(variants.iter().any(|variant| variant["$ref"] == reference));
    assert!(variants.iter().any(|variant| variant["type"] == "null"));
}

#[test]
fn openapi_31_forbids_legacy_nullable_everywhere() {
    assert!(
        !OPENAPI_SOURCE.contains("nullable:"),
        "legacy nullable syntax must not reappear in source"
    );
    let source: Value = serde_yaml::from_str(OPENAPI_SOURCE).expect("parse OpenAPI YAML");
    assert_eq!(source["openapi"], "3.1.0");
    assert_no_legacy_nullable(&source, "#");
}

#[test]
fn representative_nullability_uses_semantically_equivalent_31_unions() {
    let source: Value = serde_yaml::from_str(OPENAPI_SOURCE).expect("parse OpenAPI YAML");

    let cursor = schema(&source, "/components/schemas/CursorInfo/properties/next");
    assert_primitive_null_union(cursor, "string");

    let started_at = schema(
        &source,
        "/components/schemas/RunResponse/properties/started_at",
    );
    assert_primitive_null_union(started_at, "string");
    assert_eq!(started_at["format"], "date-time");

    let payload = schema(&source, "/components/schemas/RunEvent/properties/payload");
    assert_primitive_null_union(payload, "object");

    let budget_cost = schema(
        &source,
        "/components/schemas/InteractionBudgetLimits/properties/max_cost_usd",
    );
    assert_primitive_null_union(budget_cost, "number");
    assert_eq!(budget_cost["minimum"], 0);

    assert_reference_null_union(
        schema(
            &source,
            "/components/schemas/InteractionBudget/properties/per_run",
        ),
        "#/components/schemas/InteractionBudgetLimits",
    );
    assert_reference_null_union(
        schema(
            &source,
            "/components/schemas/InteractionConfig/properties/budget",
        ),
        "#/components/schemas/InteractionBudget",
    );

    let model = schema(
        &source,
        "/components/schemas/InteractionConfig/properties/model",
    );
    assert_primitive_null_union(model, "string");
    assert!(
        source["components"]["schemas"]["InteractionConfig"]["required"]
            .as_array()
            .expect("interaction config required fields")
            .iter()
            .any(|field| field == "model")
    );
}

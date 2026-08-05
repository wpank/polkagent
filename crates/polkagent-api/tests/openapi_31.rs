//! `OpenAPI` 3.1 dialect regression checks.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "contract fixtures fail at the exact schema pointer that regresses"
)]

use std::collections::BTreeSet;

use serde_json::Value;

const OPENAPI_SOURCE: &str = include_str!("../../../openapi.yaml");
const REDOCLY_IGNORE_SOURCE: &str = include_str!("../../../.redocly.lint-ignore.yaml");

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

fn string_set(value: &Value) -> BTreeSet<&str> {
    value
        .as_array()
        .expect("string array")
        .iter()
        .map(|entry| entry.as_str().expect("string array entry"))
        .collect()
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
fn operational_access_contract_and_lint_exceptions_are_exact() {
    let source: Value = serde_yaml::from_str(OPENAPI_SOURCE).expect("parse OpenAPI YAML");

    assert_eq!(source["info"]["license"]["name"], "Apache License 2.0");
    assert_eq!(source["info"]["license"]["identifier"], "Apache-2.0");
    assert_eq!(source["servers"][0]["url"], "/");
    assert!(!OPENAPI_SOURCE.contains("localhost"));

    let security = source["security"]
        .as_array()
        .expect("global authentication alternatives");
    assert_eq!(security.len(), 2);
    assert!(security
        .iter()
        .any(|entry| entry.get("ApiKeyAuth").is_some()));
    assert!(security
        .iter()
        .any(|entry| entry.get("BearerAuth").is_some()));

    let public_paths = BTreeSet::from([
        "/openapi.json",
        "/health/live",
        "/health/ready",
        "/health/startup",
        "/v1/compat/pca/health",
    ]);
    for path in &public_paths {
        assert_eq!(
            source["paths"][*path]["get"]["security"],
            serde_json::json!([]),
            "{path} must explicitly override global authentication"
        );
    }

    let http_methods = BTreeSet::from([
        "get", "post", "put", "patch", "delete", "options", "head", "trace",
    ]);
    let mut discovered_public_paths = BTreeSet::new();
    let mut protected_operation_count = 0_usize;
    for (path, item) in source["paths"].as_object().expect("OpenAPI paths") {
        for (method, operation) in item.as_object().expect("path item") {
            if !http_methods.contains(method.as_str()) {
                continue;
            }
            if operation["security"] == serde_json::json!([]) {
                discovered_public_paths.insert(path.as_str());
                continue;
            }

            protected_operation_count += 1;
            let operation_name = format!("{} {path}", method.to_uppercase());
            assert_eq!(
                operation["responses"]["401"]["$ref"], "#/components/responses/Unauthorized",
                "{operation_name}"
            );
            assert_eq!(
                operation["responses"]["429"]["$ref"], "#/components/responses/RateLimited",
                "{operation_name}"
            );
        }
    }
    assert_eq!(discovered_public_paths, public_paths);
    assert_eq!(protected_operation_count, 75);

    let ignore: Value =
        serde_yaml::from_str(REDOCLY_IGNORE_SOURCE).expect("parse Redocly ignore YAML");
    let ignored_rules = ignore["openapi.yaml"]
        .as_object()
        .expect("ignore rules scoped to openapi.yaml");
    assert_eq!(
        ignored_rules.keys().collect::<Vec<_>>(),
        vec!["operation-4xx-response"]
    );
    let ignored_pointers = ignored_rules["operation-4xx-response"]
        .as_array()
        .expect("operation-specific ignore pointers")
        .iter()
        .map(|value| value.as_str().expect("ignore pointer"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ignored_pointers,
        BTreeSet::from([
            "#/paths/~1health~1live/get/responses",
            "#/paths/~1health~1ready/get/responses",
            "#/paths/~1health~1startup/get/responses",
            "#/paths/~1openapi.json/get/responses",
            "#/paths/~1v1~1compat~1pca~1health/get/responses",
        ])
    );
}

#[test]
fn middleware_and_health_schemas_match_runtime_shapes() {
    let source: Value = serde_yaml::from_str(OPENAPI_SOURCE).expect("parse OpenAPI YAML");

    let auth = schema(
        &source,
        "/components/schemas/AuthMiddlewareErrorResponse/properties/error",
    );
    assert_eq!(auth["additionalProperties"], false);
    assert_eq!(auth["properties"]["code"]["const"], "UNAUTHORIZED");
    assert_eq!(
        string_set(&auth["properties"]["message"]["enum"]),
        BTreeSet::from(["invalid API key", "missing credentials"])
    );

    let rate = schema(
        &source,
        "/components/schemas/RateLimitMiddlewareErrorResponse/properties/error",
    );
    assert_eq!(rate["properties"]["code"]["const"], "RATE_LIMIT_EXCEEDED");
    assert_eq!(rate["properties"]["message"]["const"], "too many requests");
    assert_eq!(rate["properties"]["retry_after"]["minimum"], 1);

    assert_eq!(
        source["paths"]["/health/startup"]["get"]["responses"]["503"]["content"]
            ["application/json"]["schema"]["$ref"],
        "#/components/schemas/HealthStartupResponse"
    );
    assert_eq!(
        source["components"]["schemas"]["HealthReadinessResponse"]["properties"]["checks"]["type"],
        "array"
    );
}

#[test]
fn skill_reads_and_mutation_boundary_are_explicit() {
    let source: Value = serde_yaml::from_str(OPENAPI_SOURCE).expect("parse OpenAPI YAML");

    let list = &source["paths"]["/api/v1alpha1/skills"]["get"];
    assert!(list["description"]
        .as_str()
        .expect("skill list description")
        .contains("authoritative, deterministically ordered startup snapshot"));
    assert!(list["responses"].get("200").is_some());

    let get = &source["paths"]["/api/v1alpha1/skills/{skill_id}"]["get"];
    assert!(get["responses"].get("200").is_some());
    assert_eq!(
        get["responses"]["404"]["$ref"],
        "#/components/responses/NotFound"
    );

    for (path, method, success) in [
        ("/api/v1alpha1/skills/install", "post", "200"),
        ("/api/v1alpha1/skills/{skill_id}/uninstall", "post", "204"),
        ("/api/v1alpha1/skills/{skill_id}/config", "put", "200"),
    ] {
        assert!(
            source["paths"][path][method]["responses"]
                .get(success)
                .is_some(),
            "{method} {path} must document its actual success status"
        );
        assert_eq!(
            source["paths"][path][method]["responses"]["501"]["$ref"],
            "#/components/responses/NotImplemented",
            "{method} {path}"
        );
    }
}

#[test]
fn memory_reads_and_remaining_runtime_boundary_are_explicit() {
    let source: Value = serde_yaml::from_str(OPENAPI_SOURCE).expect("parse OpenAPI YAML");

    let query = &source["paths"]["/api/v1alpha1/memory/query"]["post"];
    assert!(query["description"]
        .as_str()
        .expect("memory query description")
        .contains("exact durable memory store owned by the production runtime"));
    assert!(query["responses"].get("200").is_some());
    assert_eq!(
        query["responses"]["422"]["$ref"],
        "#/components/responses/ValidationError"
    );

    let get = &source["paths"]["/api/v1alpha1/memory/entries/{entry_id}"]["get"];
    assert!(get["description"]
        .as_str()
        .expect("memory get description")
        .contains("canonical non-mutating lookup port"));
    for status in ["200", "404", "422", "501", "500"] {
        assert!(
            get["responses"].get(status).is_some(),
            "memory get must document {status}"
        );
    }

    for (path, method) in [
        ("/api/v1alpha1/memory/stats", "get"),
        ("/api/v1alpha1/memory/forget", "post"),
    ] {
        assert_eq!(
            source["paths"][path][method]["responses"]["501"]["$ref"],
            "#/components/responses/NotImplemented",
            "{method} {path}"
        );
    }
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

//! Contract smoke tests for the durable interaction `OpenAPI` surface.

#![allow(
    clippy::expect_used,
    reason = "contract fixtures fail at the exact missing path or schema boundary"
)]

use std::sync::Arc;

use axum::http::StatusCode;
use axum_test::TestServer;
use polkagent_api::{ApiServer, InMemoryAgentStore, InMemoryRunManager};
use polkagent_config::Config;
use polkagent_event::EventBus;
use polkagent_store_sqlite::SqlitePool;
use serde_json::Value;

const OPENAPI_SOURCE: &str = include_str!("../../../openapi.yaml");

fn test_server() -> TestServer {
    let effects = Arc::new(SqlitePool::open_in_memory().expect("open SQLite fixture"));
    TestServer::new(
        ApiServer::new(
            Config::default(),
            Arc::new(InMemoryAgentStore::new()),
            Arc::new(InMemoryRunManager::new()),
            effects,
            EventBus::new(8),
        )
        .into_router(),
    )
}

fn local_pointer(reference: &str) -> Option<&str> {
    reference.strip_prefix('#')
}

fn assert_local_refs_resolve(root: &Value, value: &Value) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                let pointer = local_pointer(reference)
                    .unwrap_or_else(|| panic!("OpenAPI reference must be local: {reference}"));
                assert!(
                    root.pointer(pointer).is_some(),
                    "unresolved OpenAPI reference: {reference}"
                );
            }
            for nested in object.values() {
                assert_local_refs_resolve(root, nested);
            }
        }
        Value::Array(values) => {
            for nested in values {
                assert_local_refs_resolve(root, nested);
            }
        }
        _ => {}
    }
}

fn string_set(value: &Value) -> std::collections::BTreeSet<&str> {
    value
        .as_array()
        .expect("string array")
        .iter()
        .map(|entry| entry.as_str().expect("string array entry"))
        .collect()
}

#[tokio::test]
async fn embedded_openapi_matches_source_and_interaction_contract() {
    let source: Value = serde_yaml::from_str(OPENAPI_SOURCE).expect("parse OpenAPI YAML");
    assert_eq!(source["openapi"], "3.1.0");
    assert_local_refs_resolve(&source, &source);

    let response = test_server().get("/openapi.json").await;
    response.assert_status_ok();
    let served = response.json::<Value>();
    assert_eq!(served, source, "embedded JSON must match root openapi.yaml");

    let operations = [
        ("/api/v1alpha1/interactions", "post", "createInteraction"),
        ("/api/v1alpha1/interactions", "get", "listInteractions"),
        ("/api/v1alpha1/interactions/{id}", "get", "getInteraction"),
        (
            "/api/v1alpha1/interactions/{id}",
            "delete",
            "archiveInteraction",
        ),
        (
            "/api/v1alpha1/interactions/{id}/turns",
            "get",
            "listInteractionTurns",
        ),
        (
            "/api/v1alpha1/interactions/{id}/prompt",
            "post",
            "promptInteraction",
        ),
        (
            "/api/v1alpha1/interactions/{id}/turns/{turn_id}/cancel",
            "post",
            "cancelInteractionTurn",
        ),
        (
            "/api/v1alpha1/interactions/{id}/target",
            "put",
            "updateInteractionTarget",
        ),
        (
            "/api/v1alpha1/interactions/{id}/events",
            "get",
            "replayInteractionEvents",
        ),
    ];
    for (path, method, operation_id) in operations {
        assert_eq!(
            source["paths"][path][method]["operationId"], operation_id,
            "missing or mismatched {method} {path}"
        );
        let responses = &source["paths"][path][method]["responses"];
        for error_status in ["403", "500", "501", "503"] {
            assert!(
                responses.get(error_status).is_some(),
                "{method} {path} must document {error_status}"
            );
        }
    }
    let interaction_operation_count = source["paths"]
        .as_object()
        .expect("paths object")
        .iter()
        .filter(|(path, _)| path.starts_with("/api/v1alpha1/interactions"))
        .flat_map(|(_, item)| item.as_object().expect("path item").keys())
        .filter(|key| matches!(key.as_str(), "get" | "post" | "put" | "delete"))
        .count();
    assert_eq!(interaction_operation_count, operations.len());

    let schemas = &source["components"]["schemas"];
    let create = &schemas["CreateHttpInteractionRequest"];
    assert_eq!(create["additionalProperties"], false);
    assert_eq!(
        string_set(&create["required"]),
        ["target", "working_directory"].into_iter().collect()
    );
    assert!(create["properties"].get("model").is_none());
    assert!(create["properties"].get("provider").is_none());

    let prompt = &schemas["PromptHttpInteractionRequest"];
    assert_eq!(prompt["additionalProperties"], false);
    assert_eq!(prompt["properties"]["turn_id"]["format"], "uuid");
    assert_eq!(
        string_set(&prompt["required"]),
        ["prompt", "working_directory"].into_iter().collect()
    );
    assert!(prompt["properties"].get("model").is_none());
    assert!(prompt["properties"].get("provider").is_none());

    let target = &schemas["UpdateHttpInteractionTargetRequest"];
    assert_eq!(target["additionalProperties"], false);
    assert_eq!(
        target["properties"]
            .as_object()
            .expect("target properties")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["target"]
    );

    let replay = &source["paths"]["/api/v1alpha1/interactions/{id}/events"]["get"];
    let replay_parameters: std::collections::BTreeSet<_> = replay["parameters"]
        .as_array()
        .expect("replay parameters")
        .iter()
        .map(|parameter| parameter["name"].as_str().expect("parameter name"))
        .collect();
    assert_eq!(
        replay_parameters,
        ["after_sequence", "limit", "turn_id"].into_iter().collect()
    );
    assert_eq!(
        replay["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/ReplayInteractionEventsResponse"
    );
    assert_eq!(
        string_set(&schemas["InteractionReplayCheckpoint"]["required"]),
        ["has_more", "next_after_sequence"].into_iter().collect()
    );
    assert!(
        source["paths"]["/api/v1alpha1/interactions/{id}"]["delete"]["responses"]["204"]
            .get("content")
            .is_none()
    );
    assert_eq!(
        source["paths"]["/api/v1alpha1/interactions/{id}/turns/{turn_id}/cancel"]["post"]
            ["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/CancelHttpInteractionTurnResponse"
    );

    for unsupported in [
        "/api/v1alpha1/interactions/{id}/approve",
        "/api/v1alpha1/interactions/{id}/deny",
        "/api/v1alpha1/interactions/{id}/events/stream",
    ] {
        assert!(source["paths"].get(unsupported).is_none());
    }
}

#[tokio::test]
async fn documented_interaction_methods_resolve_but_unsupported_methods_do_not() {
    let server = test_server();
    let id = uuid::Uuid::now_v7();
    let turn_id = uuid::Uuid::now_v7();
    let responses = [
        server.post("/api/v1alpha1/interactions").await,
        server.get("/api/v1alpha1/interactions").await,
        server
            .get(&format!("/api/v1alpha1/interactions/{id}"))
            .await,
        server
            .delete(&format!("/api/v1alpha1/interactions/{id}"))
            .await,
        server
            .get(&format!("/api/v1alpha1/interactions/{id}/turns"))
            .await,
        server
            .post(&format!("/api/v1alpha1/interactions/{id}/prompt"))
            .await,
        server
            .post(&format!(
                "/api/v1alpha1/interactions/{id}/turns/{turn_id}/cancel"
            ))
            .await,
        server
            .put(&format!("/api/v1alpha1/interactions/{id}/target"))
            .await,
        server
            .get(&format!("/api/v1alpha1/interactions/{id}/events"))
            .await,
    ];
    for response in responses {
        assert_ne!(response.status_code(), StatusCode::NOT_FOUND);
        assert_ne!(response.status_code(), StatusCode::METHOD_NOT_ALLOWED);
    }

    for unsupported in ["approve", "deny", "events/stream"] {
        server
            .post(&format!("/api/v1alpha1/interactions/{id}/{unsupported}"))
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }
}

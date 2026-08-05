//! Router-derived parity checks for the non-interaction API surface.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "contract tests must fail at the exact malformed source or OpenAPI boundary"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum_test::TestServer;
use polkagent_api::{ApiServer, InMemoryAgentStore, InMemoryRunManager};
use polkagent_config::Config;
use polkagent_event::EventBus;
use polkagent_store_sqlite::SqlitePool;
use serde_json::Value;
use syn::{Expr, ExprCall, ExprMethodCall, Item, Pat, Stmt};

const ROUTE_SOURCE: &str = include_str!("../src/routes/mod.rs");
const OPENAPI_SOURCE: &str = include_str!("../../../openapi.yaml");

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Operation {
    method: String,
    path: String,
}

impl Operation {
    fn new(method: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            path: normalize_path(&path.into()),
        }
    }
}

fn normalize_path(path: &str) -> String {
    assert!(path.starts_with('/'), "route path must be absolute: {path}");
    let segments = path
        .split('/')
        .map(|segment| {
            segment
                .strip_prefix(':')
                .map_or_else(|| segment.to_owned(), |name| format!("{{{name}}}"))
        })
        .collect::<Vec<_>>();
    segments.join("/")
}

fn method_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
    .and_then(|name| match name.as_str() {
        "get" => Some("get"),
        "post" => Some("post"),
        "put" => Some("put"),
        "delete" => Some("delete"),
        "patch" => Some("patch"),
        "head" => Some("head"),
        "options" => Some("options"),
        "trace" => Some("trace"),
        _ => None,
    })
}

fn method_router_methods(expr: &Expr) -> BTreeSet<String> {
    match expr {
        Expr::Call(call) => method_name(call.func.as_ref()).map_or_else(
            || panic!("unsupported method-router call: {expr:?}"),
            |method| BTreeSet::from([method.to_owned()]),
        ),
        Expr::MethodCall(call) => {
            let mut methods = method_router_methods(call.receiver.as_ref());
            let method = call.method.to_string();
            assert!(
                matches!(
                    method.as_str(),
                    "get" | "post" | "put" | "delete" | "patch" | "head" | "options" | "trace"
                ),
                "unsupported chained route method: {method}"
            );
            methods.insert(method);
            methods
        }
        _ => panic!("unsupported method-router expression: {expr:?}"),
    }
}

fn string_literal(expr: &Expr) -> String {
    let Expr::Lit(literal) = expr else {
        panic!("router path or prefix is not a literal: {expr:?}");
    };
    let syn::Lit::Str(value) = &literal.lit else {
        panic!("router path or prefix is not a string: {expr:?}");
    };
    value.value()
}

fn one_argument<'a>(call: &'a ExprMethodCall, name: &str) -> &'a Expr {
    assert_eq!(call.args.len(), 1, "{name} must have one argument");
    call.args.first().expect("one router argument")
}

fn join_prefix(prefix: &str, path: &str) -> String {
    format!("{}{}", prefix.trim_end_matches('/'), path)
}

fn evaluate_router(
    expr: &Expr,
    bindings: &BTreeMap<String, BTreeSet<Operation>>,
) -> BTreeSet<Operation> {
    match expr {
        Expr::Call(ExprCall { func, .. }) => {
            let Expr::Path(path) = func.as_ref() else {
                panic!("unsupported router constructor: {expr:?}");
            };
            assert_eq!(
                path.path
                    .segments
                    .last()
                    .expect("constructor segment")
                    .ident,
                "new",
                "unsupported router constructor: {expr:?}"
            );
            BTreeSet::new()
        }
        Expr::Path(path) => {
            let name = path
                .path
                .get_ident()
                .unwrap_or_else(|| panic!("router binding is not a simple identifier: {expr:?}"))
                .to_string();
            bindings
                .get(&name)
                .unwrap_or_else(|| panic!("unknown router binding: {name}"))
                .clone()
        }
        Expr::MethodCall(call) => {
            let method = call.method.to_string();
            let mut routes = evaluate_router(call.receiver.as_ref(), bindings);
            match method.as_str() {
                "route" => {
                    assert_eq!(call.args.len(), 2, "route must have path and methods");
                    let mut arguments = call.args.iter();
                    let path = normalize_path(&string_literal(
                        arguments.next().expect("route path argument"),
                    ));
                    for method in method_router_methods(
                        arguments.next().expect("route method-router argument"),
                    ) {
                        assert!(
                            routes.insert(Operation::new(method, path.clone())),
                            "duplicate router operation for {path}"
                        );
                    }
                }
                "merge" => {
                    routes.extend(evaluate_router(one_argument(call, "merge"), bindings));
                }
                "nest" => {
                    assert_eq!(call.args.len(), 2, "nest must have prefix and router");
                    let mut arguments = call.args.iter();
                    let prefix = string_literal(arguments.next().expect("nest prefix"));
                    let nested =
                        evaluate_router(arguments.next().expect("nested router"), bindings);
                    routes.extend(nested.into_iter().map(|operation| Operation {
                        method: operation.method,
                        path: normalize_path(&join_prefix(&prefix, &operation.path)),
                    }));
                }
                "layer" | "with_state" => {}
                _ => panic!("unsupported router builder method: {method}"),
            }
            routes
        }
        Expr::Group(group) => evaluate_router(&group.expr, bindings),
        Expr::Paren(paren) => evaluate_router(&paren.expr, bindings),
        _ => panic!("unsupported router expression: {expr:?}"),
    }
}

fn registered_operations() -> BTreeSet<Operation> {
    let file = syn::parse_file(ROUTE_SOURCE).expect("parse routes/mod.rs");
    let function = file
        .items
        .into_iter()
        .find_map(|item| match item {
            Item::Fn(function) if function.sig.ident == "register" => Some(function),
            _ => None,
        })
        .expect("routes::register function");
    let mut bindings = BTreeMap::new();
    let mut result = None;
    for statement in function.block.stmts {
        match statement {
            Stmt::Local(local) => {
                let Pat::Ident(binding) = local.pat else {
                    continue;
                };
                let Some(initializer) = local.init else {
                    continue;
                };
                bindings.insert(
                    binding.ident.to_string(),
                    evaluate_router(&initializer.expr, &bindings),
                );
            }
            Stmt::Expr(expression, None) => {
                result = Some(evaluate_router(&expression, &bindings));
            }
            _ => {}
        }
    }
    result.expect("routes::register tail router expression")
}

fn openapi_operations(document: &Value) -> BTreeSet<Operation> {
    const HTTP_METHODS: &[&str] = &[
        "get", "post", "put", "delete", "patch", "head", "options", "trace",
    ];
    let mut operation_ids = BTreeSet::new();
    let mut operations = BTreeSet::new();
    for (path, item) in document["paths"].as_object().expect("OpenAPI paths object") {
        for method in HTTP_METHODS {
            let Some(operation) = item.get(*method) else {
                continue;
            };
            let operation_id = operation["operationId"]
                .as_str()
                .unwrap_or_else(|| panic!("missing operationId for {method} {path}"));
            assert!(
                operation_ids.insert(operation_id.to_owned()),
                "duplicate OpenAPI operationId: {operation_id}"
            );
            assert!(
                operations.insert(Operation::new(*method, path)),
                "duplicate OpenAPI operation: {method} {path}"
            );
        }
    }
    operations
}

fn assert_local_refs_resolve(root: &Value, value: &Value) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                let pointer = reference
                    .strip_prefix('#')
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

fn transport_allowlist() -> BTreeMap<Operation, &'static str> {
    BTreeMap::from([
        (
            Operation::new("get", "/api/v1alpha1/events/stream"),
            "OpenAPI can describe the HTTP upgrade handshake but not the bidirectional WebSocket frame protocol; omitting a misleading handshake-only operation keeps the contract truthful",
        ),
        (
            Operation::new("get", "/ws/v1alpha1"),
            "OpenAPI can describe the HTTP upgrade handshake but not the legacy bidirectional WebSocket message protocol; omitting a misleading handshake-only operation keeps the contract truthful",
        ),
    ])
}

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

#[test]
fn axum_parameter_syntax_is_normalized_explicitly() {
    assert_eq!(normalize_path("/runs/:id/events"), "/runs/{id}/events");
    assert_eq!(normalize_path("/runs/{id}/events"), "/runs/{id}/events");
}

#[tokio::test]
async fn router_openapi_and_served_document_have_exact_operation_parity() {
    let source: Value = serde_yaml::from_str(OPENAPI_SOURCE).expect("parse root OpenAPI YAML");
    assert_eq!(source["openapi"], "3.1.0");
    assert_local_refs_resolve(&source, &source);

    let served = test_server().get("/openapi.json").await;
    served.assert_status_ok();
    assert_eq!(
        served.json::<Value>(),
        source,
        "served OpenAPI must equal root YAML"
    );

    let registered = registered_operations();
    let documented = openapi_operations(&source);
    let missing = registered
        .difference(&documented)
        .cloned()
        .collect::<BTreeSet<_>>();
    let stale = documented
        .difference(&registered)
        .cloned()
        .collect::<BTreeSet<_>>();
    let allowed = transport_allowlist().into_keys().collect::<BTreeSet<_>>();
    assert_eq!(
        missing, allowed,
        "missing OpenAPI operations differ from exact transport allowlist"
    );
    assert!(stale.is_empty(), "stale OpenAPI operations: {stale:#?}");
}

#[test]
fn transport_allowlist_is_exact_minimal_and_explained() {
    let registered = registered_operations();
    let source: Value = serde_yaml::from_str(OPENAPI_SOURCE).expect("parse root OpenAPI YAML");
    let documented = openapi_operations(&source);
    let allowlist = transport_allowlist();
    let actual_undocumented = registered
        .difference(&documented)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_undocumented,
        allowlist.keys().cloned().collect(),
        "every allowlisted operation must be necessary and every undocumented operation explained"
    );
    for (operation, rationale) in allowlist {
        assert!(registered.contains(&operation));
        assert!(!documented.contains(&operation));
        assert!(rationale.contains("WebSocket") && rationale.contains("OpenAPI"));
    }
}

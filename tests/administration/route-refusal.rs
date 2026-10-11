//! Real isolated gateway and credential authority; catalogue reads may start discovery.
use super::{request, required, Evidence};
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use std::path::PathBuf;

fn row<'a>(document: &'a Value, field: &str, key: &str, name: &str) -> &'a Value {
    document[field]
        .as_array()
        .expect("actual response rows")
        .iter()
        .find(|entry| entry[key].as_str() == Some(name))
        .expect("fixture route must remain visible")
}

fn refusal(value: &Value, operation: &str, cause: &str) {
    let reason = value.as_str().expect("actual refusal reason");
    assert!(reason.contains(operation), "missing operation: {reason}");
    assert!(reason.contains(cause), "missing cause: {reason}");
}

#[tokio::test]
#[ignore = "Requires an isolated real gateway with a declared alias and a failing real credential-route authority"]
async fn route_authority_failure_remains_visible_and_preserves_registry() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-tests/route-authority")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&directory).expect("create evidence directory");
    let mut evidence = Evidence {
        path: directory.join("report.json"),
        report: json!({"test_build": brama::build_info::current(), "requests": [],
            "command": std::env::args().collect::<Vec<_>>()}),
    };
    assert_eq!(
        evidence.report["test_build"]["source_revision"],
        required("TEST_SOURCE_REVISION")
    );
    let origin = reqwest::Url::parse(&required("BRAMA_ADMIN_URL")).expect("actual gateway URL");
    assert!(origin.username().is_empty() && origin.password().is_none());
    let bearer = required("BRAMA_ADMIN_BEARER");
    let alias = required("EXPECTED_ALIAS");
    let route = required("EXPECTED_ROUTE");
    let operation = required("EXPECTED_ROUTE_OPERATION");
    let cause = required("EXPECTED_ROUTE_CAUSE");
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("HTTP client");
    let snapshot = origin.join("/v1/admin/snapshot").unwrap();
    let (status, before) = request(
        &client,
        &mut evidence,
        Method::GET,
        snapshot.as_str(),
        &bearer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        before["build"]["source_revision"],
        required("GATEWAY_SOURCE_REVISION")
    );
    assert_eq!(
        before["routes"]["routes"][&alias], route,
        "fixture alias must already point to the independently declared route"
    );
    let aliases = origin.join("/v1/aliases").unwrap();
    let (status, diagnoses) = request(
        &client,
        &mut evidence,
        Method::GET,
        aliases.as_str(),
        &bearer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let diagnosis = row(&diagnoses, "aliases", "alias", &alias);
    assert_eq!(diagnosis["state"], "capability_check_failed");
    assert_eq!(diagnosis["route"], route);
    refusal(&diagnosis["reason"], &operation, &cause);
    let models = origin.join("/v1/models").unwrap();
    let (status, catalogue) = request(
        &client,
        &mut evidence,
        Method::GET,
        models.as_str(),
        &bearer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let model = row(&catalogue, "data", "id", &alias);
    assert_eq!(model["available"], false);
    refusal(&model["unavailable_reason"], &operation, &cause);
    let routes = origin.join("/v1/admin/routes").unwrap();
    let (status, rejected) = request(
        &client,
        &mut evidence,
        Method::PUT,
        routes.as_str(),
        &bearer,
        Some(json!({"alias": alias, "primary": route})),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    refusal(&rejected["error"]["message"], &operation, &cause);
    let (status, after) = request(
        &client,
        &mut evidence,
        Method::GET,
        snapshot.as_str(),
        &bearer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        after["routes"], before["routes"],
        "rejected mutation changed persisted routing"
    );
    let (status, repeated) = request(
        &client,
        &mut evidence,
        Method::GET,
        aliases.as_str(),
        &bearer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let diagnosis = row(&repeated, "aliases", "alias", &alias);
    assert_eq!(
        diagnosis["state"], "capability_check_failed",
        "failed inspection must not become cached absence"
    );
    refusal(&diagnosis["reason"], &operation, &cause);
}

#[tokio::test]
#[ignore = "Requires an isolated real gateway with an existing invalid registry readable by the qualification process"]
async fn invalid_registry_preserves_read_and_mutation_causes() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-tests/registry-refusal")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&directory).expect("create evidence directory");
    let mut evidence = Evidence {
        path: directory.join("report.json"),
        report: json!({"test_build": brama::build_info::current(), "requests": [],
            "command": std::env::args().collect::<Vec<_>>()}),
    };
    assert_eq!(
        evidence.report["test_build"]["source_revision"],
        required("TEST_SOURCE_REVISION")
    );
    let origin = reqwest::Url::parse(&required("BRAMA_ADMIN_URL")).expect("actual gateway URL");
    assert!(origin.username().is_empty() && origin.password().is_none());
    let bearer = required("BRAMA_ADMIN_BEARER");
    let path = required("EXPECTED_REGISTRY_PATH");
    let cause = required("EXPECTED_REGISTRY_CAUSE");
    let original = std::fs::read(&path).expect("read isolated fixture before requests");
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("HTTP client");
    let (status, health) = request(
        &client,
        &mut evidence,
        Method::GET,
        origin.join("/health").unwrap().as_str(),
        &bearer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        health["build"]["source_revision"],
        required("GATEWAY_SOURCE_REVISION")
    );
    let snapshot = origin.join("/v1/admin/snapshot").unwrap();
    let (status, unreadable) = request(
        &client,
        &mut evidence,
        Method::GET,
        snapshot.as_str(),
        &bearer,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "refuse a healthy fixture"
    );
    refusal(
        &unreadable["error"]["message"],
        &format!("read route registry {path}"),
        &cause,
    );
    let alias = uuid::Uuid::new_v4().to_string();
    evidence.report["owned_alias"] = json!(alias);
    let routes = origin.join("/v1/admin/routes").unwrap();
    let (put_status, updated) = request(
        &client,
        &mut evidence,
        Method::PUT,
        routes.as_str(),
        &bearer,
        Some(json!({"alias": alias, "primary": "best"})),
    )
    .await;
    // Delete only this run's fresh name even if the update unexpectedly worked.
    let (delete_status, deleted) = request(
        &client,
        &mut evidence,
        Method::DELETE,
        routes.as_str(),
        &bearer,
        Some(json!({"alias": alias})),
    )
    .await;
    assert_eq!(put_status, StatusCode::CONFLICT);
    assert_eq!(delete_status, StatusCode::CONFLICT);
    refusal(
        &updated["error"]["message"],
        &format!("update alias `{alias}` in route registry {path}"),
        &cause,
    );
    refusal(
        &deleted["error"]["message"],
        &format!("delete alias `{alias}` from route registry {path}"),
        &cause,
    );
    assert_eq!(
        std::fs::read(&path).expect("read fixture after refusals"),
        original,
        "refused registry operations changed persisted bytes"
    );
}

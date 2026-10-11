//! Real isolated gateway and real credential authority; no provider request.
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

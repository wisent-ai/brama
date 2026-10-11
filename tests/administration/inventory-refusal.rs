//! Read-only qualification against an isolated real gateway and its failing inventory.
use super::{request, required, Evidence};
use reqwest::{Client, Method, StatusCode};
use serde_json::json;
use std::path::PathBuf;

#[tokio::test]
#[ignore = "Requires an isolated real gateway with an actual provider-inventory failure and administrative bearer"]
async fn provider_inventory_failure_is_not_reported_as_an_empty_pool() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-tests/provider-inventory")
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
    let operation = required("EXPECTED_PROVIDER_INVENTORY_OPERATION");
    let resource = required("EXPECTED_PROVIDER_INVENTORY_RESOURCE");
    let cause = required("EXPECTED_PROVIDER_INVENTORY_CAUSE");
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
    for path in ["/v1/admin/snapshot", "/stats", "/v1/models"] {
        let endpoint = origin.join(path).unwrap();
        let (status, response) = request(
            &client,
            &mut evidence,
            Method::GET,
            endpoint.as_str(),
            &bearer,
            None,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "unreadable provider inventory must not become a successful empty census at {path}"
        );
        let reason = response["error"]["message"]
            .as_str()
            .expect("actual refusal cause");
        for expected in [&operation, &resource, &cause] {
            assert!(
                reason.contains(expected.as_str()),
                "{path} lost {expected}: {reason}"
            );
        }
    }
}

#[tokio::test]
#[ignore = "Requires the actual vault environment with an invalid executable declaration"]
async fn invalid_vault_declaration_is_not_an_empty_subscription_pool() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-tests/vault-declaration")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&directory).expect("create evidence directory");
    let mut evidence = Evidence {
        path: directory.join("report.json"),
        report: json!({"test_build": brama::build_info::current(),
            "command": std::env::args().collect::<Vec<_>>()}),
    };
    assert_eq!(
        evidence.report["test_build"]["source_revision"],
        required("TEST_SOURCE_REVISION")
    );
    let declaration = required("EXPECTED_VAULT_DECLARATION");
    let cause = required("EXPECTED_VAULT_DECLARATION_CAUSE");
    let result = brama::gateway::broker::list_all_subscriptions().await;
    let error = result.expect_err("invalid executable declaration must refuse discovery");
    evidence.report["refusal"] = json!(error);
    assert!(
        error.contains(&declaration),
        "refusal lost declaration: {error}"
    );
    assert!(error.contains(&cause), "refusal lost actual cause: {error}");
}

//! Actual isolated gateway only; no credential is installed or replaced.
use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Value};
use std::path::PathBuf;

struct Evidence {
    path: PathBuf,
    report: Value,
}

impl Drop for Evidence {
    fn drop(&mut self) {
        self.report["passed"] = json!(!std::thread::panicking());
        std::fs::write(&self.path, self.report.to_string())
            .expect("retain administration evidence");
    }
}

fn required(name: &str) -> String {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(error) => panic!("read qualification input {name} from environment: {error}"),
    };
    assert!(
        !value.trim().is_empty(),
        "{name} must name the actual fixture input"
    );
    value
}

async fn request(
    client: &Client,
    evidence: &mut Evidence,
    method: Method,
    endpoint: &str,
    bearer: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = client.request(method.clone(), endpoint).bearer_auth(bearer);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let result = request.send().await;
    if let Err(error) = &result {
        evidence.report["transport_error"] = json!(error.to_string());
    }
    let response = result.expect("actual gateway request");
    let status = response.status();
    let text = response.text().await;
    if let Err(error) = &text {
        evidence.report["body_read_error"] = json!(error.to_string());
    }
    let text = text.expect("actual response body");
    assert!(
        !text.contains(bearer),
        "response exposed the administrative bearer"
    );
    evidence.report["requests"].as_array_mut().unwrap().push(json!({
        "method": method.as_str(), "endpoint": endpoint, "http_status": status.as_u16(), "body": text,
    }));
    (
        status,
        serde_json::from_str(&text).expect("actual gateway JSON"),
    )
}

#[tokio::test]
#[ignore = "Requires an isolated real gateway with an unavailable standalone credential store and administrative identity"]
async fn unavailable_credential_store_preserves_operation_and_cause() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-tests/credential-administration")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&directory).expect("create evidence directory");
    let mut evidence = Evidence {
        path: directory.join("report.json"),
        report: json!({"test_build": brama::build_info::current(), "requests": [],
            "command": std::env::args().collect::<Vec<_>>()}),
    };
    let revision = required("TEST_SOURCE_REVISION");
    assert_eq!(evidence.report["test_build"]["source_revision"], revision);
    let origin = reqwest::Url::parse(&required("BRAMA_ADMIN_URL")).expect("actual gateway URL");
    assert!(origin.username().is_empty() && origin.password().is_none());
    let bearer = required("BRAMA_ADMIN_BEARER");
    let cause = required("EXPECTED_STORE_REFUSAL");
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("HTTP client");
    let health = origin.join("/health").unwrap();
    let (status, build) = request(
        &client,
        &mut evidence,
        Method::GET,
        health.as_str(),
        &bearer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        build["build"]["source_revision"],
        required("GATEWAY_SOURCE_REVISION")
    );
    let endpoint = origin.join("/v1/admin/credentials").unwrap();
    let (status, before) = request(
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
        StatusCode::CONFLICT,
        "refuse to continue against a readable store"
    );
    assert_eq!(
        before["error"]["message"],
        format!("list standalone provider credentials: {cause}")
    );
    let provider = uuid::Uuid::new_v4().to_string();
    let (status, deleted) = request(
        &client,
        &mut evidence,
        Method::DELETE,
        endpoint.as_str(),
        &bearer,
        Some(json!({"provider": provider})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        deleted["error"]["message"],
        format!("delete standalone provider credential: {cause}")
    );
    let (status, after) = request(
        &client,
        &mut evidence,
        Method::GET,
        endpoint.as_str(),
        &bearer,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        after["error"], before["error"],
        "refusal state changed after rejected deletion"
    );
}

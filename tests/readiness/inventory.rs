//! Read-only qualification of the actual deployed readiness endpoint.
use serde_json::{json, Value};
use std::path::PathBuf;

struct Evidence {
    path: PathBuf,
    report: Value,
}

impl Drop for Evidence {
    fn drop(&mut self) {
        self.report["passed"] = json!(!std::thread::panicking());
        std::fs::write(&self.path, self.report.to_string()).expect("retain readiness evidence");
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

#[tokio::test]
#[ignore = "Requires a real completed readiness observation on an isolated gateway, no simulated dependencies"]
async fn inventory_read_failures_remain_visible_in_readiness() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-tests/readiness-inventory")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&directory).expect("create evidence directory");
    let mut evidence = Evidence {
        path: directory.join("report.json"),
        report: json!({"test_build": brama::build_info::current(),
            "command": std::env::args().collect::<Vec<_>>()}),
    };
    let test_revision = required("TEST_SOURCE_REVISION");
    evidence.report["expected_test_source_revision"] = json!(test_revision);
    assert_eq!(
        evidence.report["test_build"]["source_revision"], test_revision,
        "qualification binary must match its declared source revision"
    );
    let endpoint = required("BRAMA_READINESS_URL");
    let revision = required("GATEWAY_SOURCE_REVISION");
    let expected_ready: bool = required("EXPECTED_READY")
        .parse()
        .expect("EXPECTED_READY is a boolean");
    let status: reqwest::StatusCode = required("EXPECTED_READINESS_HTTP_STATUS")
        .parse()
        .expect("actual expected HTTP status");
    // Empty means a fully healthy completed census, not permission to ignore an error.
    let cause = std::env::var("EXPECTED_INVENTORY_ERROR")
        .expect("EXPECTED_INVENTORY_ERROR must be supplied, empty only for healthy census");
    let operation = if cause.is_empty() {
        None
    } else {
        Some(required("EXPECTED_INVENTORY_OPERATION"))
    };
    evidence.report["request"] = json!({"method": "GET", "endpoint": endpoint});
    evidence.report["expected"] = json!({"source_revision": revision, "ready": expected_ready,
        "http_status": status.as_u16(), "operation": operation, "cause": cause});
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("HTTP client")
        .get(&endpoint)
        .send()
        .await;
    if let Err(error) = &response {
        evidence.report["transport_error"] = json!(error.to_string());
    }
    let response = response.expect("real readiness request refused");
    evidence.report["http_status"] = json!(response.status().as_u16());
    let actual_status = response.status();
    let bytes = response.bytes().await;
    if let Err(error) = &bytes {
        evidence.report["body_read_error"] = json!(error.to_string());
    }
    let bytes = bytes.expect("read actual readiness body");
    evidence.report["body"] = json!(String::from_utf8_lossy(&bytes));
    let body: Value = serde_json::from_slice(&bytes).expect("readiness JSON");
    assert_eq!(actual_status, status);
    assert_eq!(body["build"]["source_revision"], revision);
    assert_eq!(body["ready"], expected_ready);
    let errors = body["inventory_errors"]
        .as_array()
        .expect("typed inventory diagnostic array");
    if let Some(operation) = operation {
        assert_eq!(body["degraded"], true);
        assert_eq!(body["operator_action_required"], true);
        assert!(
            errors.iter().any(|entry| entry["operation"] == operation
                && entry["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains(&cause))),
            "actual failed operation/cause absent: {errors:?}"
        );
        let reason = body["reason"].as_str().expect("readiness reason");
        assert!(reason.contains(&operation) && reason.contains(&cause));
    } else {
        assert!(
            errors.is_empty(),
            "healthy census retains inventory failures"
        );
        assert_eq!(
            body["degraded"], false,
            "interim or degraded response is not a healthy census"
        );
    }
}

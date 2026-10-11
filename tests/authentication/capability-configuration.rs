//! Actual key-file reads and optional redemption against the real local authority.
use super::support::required;
use brama::capability::{CapabilityClient, CapabilityRef};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

struct Evidence {
    path: PathBuf,
    report: Value,
}
impl Drop for Evidence {
    fn drop(&mut self) {
        self.report["passed"] = json!(!std::thread::panicking());
        std::fs::write(&self.path, self.report.to_string()).expect("retain capability evidence");
    }
}

#[test]
#[ignore = "Requires an isolated real key-file fixture; redeemed outcome also consumes a real capability"]
fn capability_configuration_preserves_actual_refusal_or_redeems_expected_secret() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-tests/capability-configuration")
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
    let outcome = required("EXPECTED_CAPABILITY_OUTCOME");
    evidence.report["expected_outcome"] = json!(outcome);
    match outcome.as_str() {
        "configuration-refusal" => {
            let expected = required("EXPECTED_CAPABILITY_CAUSE");
            let resource = required("EXPECTED_CAPABILITY_RESOURCE");
            let error =
                CapabilityClient::from_env().expect_err("actual invalid configuration must refuse");
            let detail = error.to_string();
            evidence.report["refusal"] = json!(detail);
            assert!(detail.contains(&expected), "actual cause lost: {detail}");
            assert!(
                detail.contains(&resource),
                "failed field or resource lost: {detail}"
            );
        }
        "redeemed" => {
            let expected = required("EXPECTED_SECRET_SHA256");
            let id = required("CAPABILITY_ID");
            let resource = required("CAPABILITY_RESOURCE");
            let client = CapabilityClient::from_env().expect("read actual protected workload key");
            let binding = CapabilityRef::provider(&id, &resource)
                .expect("actual provider capability binding");
            let result = client.redeem(&binding);
            if let Err(error) = &result {
                evidence.report["refusal"] = json!(error.to_string());
            }
            let secret = result.expect("real authority redemption");
            let observed = hex::encode(Sha256::digest(secret.expose()));
            evidence.report["resource"] = json!(resource);
            evidence.report["secret_sha256"] = json!(observed);
            assert_eq!(
                observed, expected,
                "authority returned a different fixture credential"
            );
            let expected_utf8 = required("EXPECTED_SECRET_UTF8")
                .parse::<bool>()
                .expect("EXPECTED_SECRET_UTF8 must be a boolean");
            match secret.expose_utf8() {
                Ok(_) => assert!(
                    expected_utf8,
                    "fixture was expected to contain invalid UTF-8"
                ),
                Err(error) => {
                    let detail = error.to_string();
                    evidence.report["decoding_refusal"] = json!(detail);
                    assert!(!expected_utf8, "expected valid UTF-8: {detail}");
                    assert!(
                        matches!(error, brama::capability::CapabilityError::SecretEncoding(_)),
                        "successful redemption was misclassified as authorization denial: {detail}"
                    );
                    assert!(
                        detail.contains(&required("EXPECTED_DECODE_CAUSE")),
                        "decoder lost actual cause: {detail}"
                    );
                }
            }
        }
        _ => panic!("EXPECTED_CAPABILITY_OUTCOME must be configuration-refusal or redeemed"),
    }
}

#[tokio::test]
#[ignore = "Requires an isolated real request-sign identity or capability authority; no simulated secret"]
async fn request_sign_lookup_preserves_actual_refusal_or_expected_identity() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-tests/request-sign-lookup")
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
    let agent = required("REQUEST_SIGN_AGENT");
    let expected_success: bool = required("EXPECTED_SIGNING_SUCCESS")
        .parse()
        .expect("EXPECTED_SIGNING_SUCCESS is a boolean");
    evidence.report["agent"] = json!(agent);
    evidence.report["expected_success"] = json!(expected_success);
    let result = brama::gateway::broker::get_agent_auth_secret(&agent).await;
    if let Err(error) = &result {
        evidence.report["refusal"] = json!(error);
    }
    if expected_success {
        let secret = result.expect("resolve actual request-sign identity");
        let observed = hex::encode(Sha256::digest(secret.expose()));
        evidence.report["secret_sha256"] = json!(observed);
        assert_eq!(
            observed,
            required("EXPECTED_SECRET_SHA256"),
            "lookup resolved a different identity"
        );
    } else {
        let cause = required("EXPECTED_SIGNING_CAUSE");
        let resource = required("EXPECTED_SIGNING_RESOURCE");
        let error = result.expect_err("real failing identity lookup must refuse");
        assert!(error.contains(&cause), "lookup lost actual cause: {error}");
        assert!(
            error.contains(&resource),
            "lookup lost failed resource: {error}"
        );
    }
}

#[test]
#[ignore = "Requires an isolated real capability authority with an observed transport or response-frame failure"]
fn capability_redemption_retains_actual_wire_failure() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-tests/capability-wire")
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
    let id = required("CAPABILITY_ID");
    let resource = required("CAPABILITY_RESOURCE");
    let operation = required("EXPECTED_CAPABILITY_OPERATION");
    let cause = required("EXPECTED_CAPABILITY_CAUSE");
    let socket = required("SKARBIEC_CAP_SOCKET");
    let client = CapabilityClient::from_env().expect("actual valid client configuration");
    let binding = CapabilityRef::provider(&id, &resource).expect("actual capability binding");
    let error = client
        .redeem(&binding)
        .expect_err("actual failed wire operation must refuse");
    let detail = error.to_string();
    evidence.report["refusal"] = json!(detail);
    evidence.report["resource"] = json!(resource);
    assert!(
        matches!(
            error,
            brama::capability::CapabilityError::AuthorityUnreachable(_)
                | brama::capability::CapabilityError::AuthorityAnswer(_)
        ),
        "wire failure was misclassified as authorization denial: {detail}"
    );
    for expected in [&operation, &cause, &socket] {
        assert!(
            detail.contains(expected.as_str()),
            "wire refusal lost {expected}: {detail}"
        );
    }
}

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
        }
        _ => panic!("EXPECTED_CAPABILITY_OUTCOME must be configuration-refusal or redeemed"),
    }
}

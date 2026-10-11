//! Read-only qualification through the real CLI, gateway and workload authority.
//! Run on an isolated fixture host with its actual gateway log readable locally.
#[path = "../subscriptions/support.rs"]
mod support;

use axum::http::StatusCode;
use serde_json::{json, Value};
use support::{required, Run};

#[test]
#[ignore = "Requires a real isolated gateway, workload bearer role and gateway authority log"]
fn workload_authority_admission_preserves_real_refusal_cause() {
    let expected = required("ADMISSION_STATUS")
        .parse::<StatusCode>()
        .expect("expected gateway HTTP status");
    assert!(expected == StatusCode::OK || expected == StatusCode::SERVICE_UNAVAILABLE);
    let workload_role = required("WORKLOAD_BEARER_ROLE");
    let consumer = required("CONSUMER");
    let log_path = required("GATEWAY_LOG");
    let refusal = if expected == StatusCode::SERVICE_UNAVAILABLE {
        let status = required("AUTHORITY_HTTP_STATUS")
            .parse::<StatusCode>()
            .expect("actual authority HTTP status");
        assert!(status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS);
        Some((
            status,
            required("AUTHORITY_OPERATION"),
            required("AUTHORITY_CAUSE"),
        ))
    } else {
        None
    };
    let mut run = Run::new();
    run.report["authority"] = json!({
        "expected_status": expected.as_u16(), "workload_role": workload_role,
        "gateway_log": log_path,
        "expected_refusal": refusal.as_ref().map(|(status, operation, cause)| {
            json!({"status": status.as_u16(), "operation": operation, "cause": cause})
        }),
    });
    run.save();
    let control = run.command(&["subscription", "lease", "list"]);
    assert!(
        control.status.success(),
        "control identity cannot read real lease state"
    );
    let control: Value = serde_json::from_slice(&control.stdout).expect("control lease state");
    let before_log = std::fs::read(&log_path).expect("read actual gateway log before admission");
    let output = run.standalone(&[
        "subscription",
        "lease",
        "list",
        "--gateway-consumer",
        &consumer,
        "--bearer-role",
        &workload_role,
        "--json",
    ]);
    let after_log = std::fs::read(&log_path).expect("read actual gateway log after admission");
    assert!(
        after_log.starts_with(&before_log),
        "gateway log rotated during qualification"
    );
    let observed =
        String::from_utf8(after_log[before_log.len()..].to_vec()).expect("gateway diagnostic text");
    run.report["authority"]["observed_log"] = json!(observed);
    run.save();
    if let Some((status, operation, cause)) = refusal {
        assert!(
            !output.status.success(),
            "unavailable authority admitted a workload"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains(&format!("HTTP {}:", expected.as_u16())),
            "CLI did not preserve the gateway service-unavailable refusal"
        );
        let diagnostic = observed.lines().find(|line| {
            line.contains("workload_authority_unavailable")
                && line.contains(&operation)
                && line.contains(&status.to_string())
                && line.contains(&cause)
        });
        assert!(
            diagnostic.is_some(),
            "actual authority operation/status/body cause missing from new gateway diagnostics"
        );
    } else {
        assert!(
            output.status.success(),
            "actual workload authority refused admission"
        );
        let admitted: Value =
            serde_json::from_slice(&output.stdout).expect("authorized lease state");
        assert_eq!(
            admitted, control,
            "authorized workload did not read the actual gateway lease state"
        );
        assert!(
            !observed.contains("workload_authority_unavailable"),
            "authorized fixture still has an authority failure"
        );
    }
    let after = run.command(&["subscription", "lease", "list"]);
    assert!(after.status.success(), "final control lease read refused");
    let after: Value = serde_json::from_slice(&after.stdout).expect("final persisted lease state");
    assert_eq!(
        after, control,
        "read-only authority qualification changed lease state"
    );
    run.report["result"] = json!("passed");
    run.save();
}

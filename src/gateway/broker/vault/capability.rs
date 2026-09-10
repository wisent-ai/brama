//! Obtaining one capability from the authority for one resource.
//!
//! This is the stronger of the two credential paths and the only one that
//! spends an issuance: the agent it is issued to, the purposes it may be
//! issued for, the request's own shape, and the two ways of waiting for the
//! answer -- the request path awaits it, the startup check blocks on it. It is
//! separate because the limits below belong to the broker's contract rather
//! than to any caller that redeems what comes back.

use serde_json::Value;

use super::super::credential_failure;
use super::router::router_output;
use crate::core::failure;
use wisent_errors::{Code, Failure};

pub(in crate::gateway::broker) const PROVIDER_PURPOSE: &str = "brama.provider.authenticate";
pub(in crate::gateway::broker) const REQUEST_SIGN_PURPOSE: &str = "brama.request.sign";
/// The agent a capability is issued to, and the identity whose registered key
/// the authority verifies a redemption against.
///
/// It is deliberately not `brama-service`. That consumer exists, but it holds
/// one `read` capability for this gateway's GPG key, and the authority allows
/// a workload key only on a consumer that carries `acquire` -- so naming it
/// here can never redeem, whatever else is fixed. The runtime agent needs its
/// own acquisition consumer in the vault, bound to the proof key this
/// installation provisions.
const RUNTIME_AGENT: &str = "brama-runtime";
const CAPABILITY_TARGET: &str = "brama";

/// The `capability-issue` request, taking the broker's own limits.
///
/// No lifetime or use count is passed, and that is the point. Skarbiec refuses
/// a ttl over an hour and a use count over sixteen, while the launcher asked
/// for thirty days and a million uses: every request was refused and the
/// gateway came up answering `/health` and serving nothing. The broker's
/// defaults are a short life and a single use, which is the right shape once
/// the capability is obtained where it is spent. Nothing is cached: a
/// single-use capability has nothing worth keeping, and an id the launcher put
/// in the environment of a running process cannot be refreshed at all.
fn issue_arguments(purpose: &str, resource: &str) -> Vec<String> {
    vec![
        "capability-issue".to_owned(),
        "--agent".to_owned(),
        RUNTIME_AGENT.to_owned(),
        "--purpose".to_owned(),
        purpose.to_owned(),
        "--resource".to_owned(),
        resource.to_owned(),
        "--target".to_owned(),
        CAPABILITY_TARGET.to_owned(),
    ]
}
fn capability_refusal_detail(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_owned();
    }
    let Ok(document) = serde_json::from_slice::<Value>(stdout) else {
        return "authority refused the capability without a reason".to_owned();
    };
    let reason = document
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("authority refused the capability");
    match document.get("remedy").and_then(Value::as_str) {
        Some(remedy) if !remedy.is_empty() => format!("{reason}; {remedy}"),
        _ => reason.to_owned(),
    }
}

fn issued_capability_id(output: &std::process::Output, resource: &str) -> Result<String, Failure> {
    if !output.status.success() {
        let detail = capability_refusal_detail(&output.stdout, &output.stderr);
        return Err(credential_failure(
            format!(
                "capability issuance for `{resource}` failed with status {}: {detail}",
                output.status
            ),
            resource,
            failure::code_for("credential_unauthorized"),
        ));
    }
    let document: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        credential_failure(
            format!("capability issuance returned malformed JSON: {error}"),
            resource,
            Code::Unknown,
        )
    })?;
    document
        .get("capability_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            credential_failure(
                "capability issuance response is missing non-empty field `capability_id`",
                resource,
                Code::Unknown,
            )
        })
}

/// Obtain one capability on the request path.
pub(in crate::gateway::broker) async fn issue_capability(
    purpose: &str,
    resource: &str,
) -> Result<String, Failure> {
    let arguments = issue_arguments(purpose, resource);
    let output = router_output("issue capability", |command| {
        command.args(arguments);
    })
    .await
    .map_err(|error| credential_failure(error, resource, Code::Unknown))?;
    issued_capability_id(&output, resource)
}

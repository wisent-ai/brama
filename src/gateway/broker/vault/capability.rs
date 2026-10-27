//! Obtaining one capability from the authority for one resource.
//!
//! This is the stronger of the two credential paths and the only one that
//! spends an issuance: the agent it is issued to, the purposes it may be
//! issued for, the request's own shape, and the two ways of waiting for the
//! answer -- the request path awaits it, the startup check blocks on it. It is
//! separate because the limits below belong to the broker's contract rather
//! than to any caller that redeems what comes back.

use std::num::NonZeroU64;

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

/// How long, in whole seconds, a capability obtained on the request path may
/// wait for its redemption. Skarbiec holds no default and no ceiling for a
/// capability's lifetime, so the issuer states it on every issue; the window
/// that fits a host is the operator's to declare, and an issue without it is
/// refused naming this variable.
const CAPABILITY_LIFETIME_ENV: &str = "BRAMA_CAPABILITY_LIFETIME_SECONDS";

/// How many times a request-path capability may be redeemed: once, by the
/// request that obtained it. Nothing is cached, so a capability is spent by
/// its one redemption and the vault drops it on its next load instead of
/// keeping it until its lifetime ends.
const REQUEST_USES: NonZeroU64 = NonZeroU64::MIN;

/// The `grant capability` request: the agent, purpose, resource and target,
/// the declared lifetime and the single use. Skarbiec requires both
/// `--ttl` and `--max-uses`, and refuses an issue without them before any
/// model call reaches a provider.
fn issue_arguments(purpose: &str, resource: &str) -> Result<Vec<String>, String> {
    let lifetime = crate::types::declared_age(CAPABILITY_LIFETIME_ENV)?.ok_or_else(|| {
        format!(
            "{CAPABILITY_LIFETIME_ENV} is not declared: state how many whole seconds a \
             capability obtained for one request may wait for its redemption"
        )
    })?;
    if lifetime.is_zero() {
        return Err(format!(
            "{CAPABILITY_LIFETIME_ENV} must be at least one second"
        ));
    }
    Ok(vec![
        "grant".to_owned(),
        "capability".to_owned(),
        "--agent".to_owned(),
        RUNTIME_AGENT.to_owned(),
        "--purpose".to_owned(),
        purpose.to_owned(),
        "--resource".to_owned(),
        resource.to_owned(),
        "--target".to_owned(),
        CAPABILITY_TARGET.to_owned(),
        "--ttl".to_owned(),
        lifetime.as_secs().to_string(),
        "--max-uses".to_owned(),
        REQUEST_USES.to_string(),
    ])
}

/// Why the authority refused: its stderr sentence, else the reason and
/// remedy of the refusal document it printed, else what it printed instead.
fn capability_refusal_detail(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_owned();
    }
    let document = match serde_json::from_slice::<Value>(stdout) {
        Ok(document) => document,
        Err(error) => {
            return format!(
                "the authority said nothing on stderr and its stdout is not a refusal document \
                 ({error}): {}",
                String::from_utf8_lossy(stdout).trim()
            )
        }
    };
    let text = |key: &str| {
        document
            .get(key)
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
    };
    match (text("reason"), text("remedy")) {
        (Some(reason), Some(remedy)) => format!("{reason}; {remedy}"),
        (Some(reason), None) => reason.to_owned(),
        (None, _) => format!("the authority's refusal document names no reason: {document}"),
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
    let arguments = issue_arguments(purpose, resource)
        .map_err(|refusal| credential_failure(refusal, resource, Code::Config))?;
    let output = router_output("issue capability", |command| {
        command.args(arguments);
    })
    .await
    .map_err(|error| credential_failure(error, resource, Code::Unknown))?;
    issued_capability_id(&output, resource)
}

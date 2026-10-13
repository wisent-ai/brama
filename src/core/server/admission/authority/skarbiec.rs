//! What Skarbiec says about a workload bearer: whether it is active, whose
//! consumer it is, which audience it signs for, and which Brama capabilities it
//! was issued. A token with no `call brama` capability is not a router client.

use std::collections::HashSet;

use axum::http::StatusCode;
use serde_json::{json, Value};

use super::super::identity::{valid_agent_id, ModelClientIdentity, ModelClientKind};
use super::WorkloadAuthorityAnswer;

pub(super) async fn ask_authority(bearer: &str) -> WorkloadAuthorityAnswer {
    let (Ok(base), Ok(consumer), Ok(token_file)) = (
        std::env::var("WC_SKARBIEC_URL"),
        std::env::var("BRAMA_SKARBIEC_CONSUMER"),
        std::env::var("BRAMA_SKARBIEC_TOKEN_FILE"),
    ) else {
        return WorkloadAuthorityAnswer::NotConfigured;
    };
    let own = match std::fs::read_to_string(&token_file) {
        Ok(own) => own,
        Err(error) => {
            return WorkloadAuthorityAnswer::Unavailable(format!(
                "cannot read BRAMA_SKARBIEC_TOKEN_FILE {token_file}: {error}"
            ))
        }
    };
    let client = match crate::providers::adapter::control_client() {
        Ok(client) => client,
        Err(error) => {
            return WorkloadAuthorityAnswer::Unavailable(format!(
                "cannot build the Skarbiec client: {error}"
            ))
        }
    };
    let introspect = format!("{}/v1/tokens/introspect", base.trim_end_matches('/'));
    // A bind the resolver already holds unanswered is not sent one more
    // request to stand behind the held ones: the caller is told what stands.
    match super::held::held_at(&base) {
        Ok(None) => {}
        Ok(Some(held)) => return WorkloadAuthorityAnswer::Unavailable(held),
        Err(unreadable) => return WorkloadAuthorityAnswer::Unavailable(unreadable),
    }
    // The call stands in the readiness register until it is answered; a
    // readiness reading that finds it standing past the declared refresh
    // interval ends it with the sentence `/readyz` gives.
    let mut standing = crate::core::server::readiness::standing::Standing::begin(&introspect);
    let sent = client
        .post(&introspect)
        .header("X-Consumer", consumer)
        .bearer_auth(own.trim())
        .json(&json!({"token": bearer}))
        .send();
    let response = tokio::select! {
        answered = sent => match answered {
            Ok(response) => response,
            Err(error) => {
                return WorkloadAuthorityAnswer::Unavailable(format!("POST {introspect}: {error}"))
            }
        },
        refusal = standing.ended() => {
            return WorkloadAuthorityAnswer::Unavailable(format!("POST {introspect}: {refusal}"))
        }
    };
    let status = response.status();
    if !status.is_success() && !status.is_server_error() && status != StatusCode::TOO_MANY_REQUESTS
    {
        return WorkloadAuthorityAnswer::Rejected;
    }
    let read = super::response::read_json(response, "POST", &introspect, &[bearer, own.trim()]);
    let answer: Value = tokio::select! {
        read = read => match read {
            Ok(answer) => answer,
            Err(error) => return WorkloadAuthorityAnswer::Unavailable(error),
        },
        refusal = standing.ended() => {
            return WorkloadAuthorityAnswer::Unavailable(format!("POST {introspect}: {refusal}"))
        }
    };
    drop(standing);
    if answer.get("active").and_then(Value::as_bool) != Some(true) {
        return WorkloadAuthorityAnswer::Rejected;
    }
    let Some(client_id) = answer.get("consumer").and_then(Value::as_str) else {
        return WorkloadAuthorityAnswer::Unavailable(format!(
            "POST {introspect} answered an active token with no consumer"
        ));
    };
    let agent_id = answer
        .get("audience")
        .and_then(Value::as_str)
        .filter(|audience| valid_agent_id(audience))
        .map(str::to_string);
    let Some(capabilities) = answer.get("capabilities").and_then(Value::as_array) else {
        return WorkloadAuthorityAnswer::Unavailable(format!(
            "POST {introspect} answered an active token with no capabilities array"
        ));
    };
    let routes: HashSet<String> = capabilities
        .iter()
        .filter(|capability| {
            capability.get("action").and_then(Value::as_str) == Some("call")
                && capability.get("item").and_then(Value::as_str) == Some("brama")
        })
        .filter_map(|capability| capability.get("field").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if routes.is_empty() {
        return WorkloadAuthorityAnswer::Rejected;
    }
    WorkloadAuthorityAnswer::Resolved(ModelClientIdentity {
        client_id: client_id.to_string(),
        kind: ModelClientKind::Workload { agent_id },
        allowed_models: Some(routes),
    })
}

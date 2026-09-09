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
    let Ok(own) = std::fs::read_to_string(&token_file) else {
        return WorkloadAuthorityAnswer::Unavailable;
    };
    let Ok(client) = crate::providers::adapter::control_client() else {
        return WorkloadAuthorityAnswer::Unavailable;
    };
    let response = match client
        .post(format!(
            "{}/v1/tokens/introspect",
            base.trim_end_matches('/')
        ))
        .header("X-Consumer", consumer)
        .bearer_auth(own.trim())
        .json(&json!({"token": bearer}))
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return WorkloadAuthorityAnswer::Unavailable,
    };
    if response.status().is_server_error() || response.status() == StatusCode::TOO_MANY_REQUESTS {
        return WorkloadAuthorityAnswer::Unavailable;
    }
    if !response.status().is_success() {
        return WorkloadAuthorityAnswer::Rejected;
    }
    let answer: Value = match response.json().await {
        Ok(answer) => answer,
        Err(_) => return WorkloadAuthorityAnswer::Unavailable,
    };
    if answer.get("active").and_then(Value::as_bool) != Some(true) {
        return WorkloadAuthorityAnswer::Rejected;
    }
    let Some(client_id) = answer.get("consumer").and_then(Value::as_str) else {
        return WorkloadAuthorityAnswer::Unavailable;
    };
    let agent_id = answer
        .get("audience")
        .and_then(Value::as_str)
        .filter(|audience| valid_agent_id(audience))
        .map(str::to_string);
    let Some(capabilities) = answer.get("capabilities").and_then(Value::as_array) else {
        return WorkloadAuthorityAnswer::Unavailable;
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

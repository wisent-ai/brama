//! The provider capabilities that are not chat: OpenAI's embeddings and
//! moderations, and the typed-decision wire, each with its own route check.

use serde_json::{Map, Value};

use super::super::registry::{endpoint, supports_embedding_route, supports_moderation_route};
use super::credential::authorize_provider;
use super::outcome::refusal::{provider_refused, transport_refusal};
use super::outcome::response_body::response_text;
use super::outcome::typed::{typed_object, typed_route, typed_transport};
use crate::types::{GatewayRefusal, Refusal};

pub async fn dispatch_openai_typed(
    route_id: &str,
    path: &str,
    mut payload: Map<String, Value>,
    item: &str,
    secret: &str,
) -> Result<Value, Refusal> {
    let supported = match path {
        "/v1/embeddings" => supports_embedding_route(route_id),
        "/v1/moderations" => supports_moderation_route(route_id),
        _ => false,
    };
    if !supported {
        // The route comes from this deployment's alias, not from the caller.
        return Err(Refusal::gateway(
            GatewayRefusal::ProviderFailure,
            "model route does not support the requested capability",
        ));
    }
    let (descriptor, model_id) = typed_route(route_id)?;
    let (key, base_url, client) = typed_transport(descriptor, item, secret)?;
    payload.insert("model".to_string(), Value::String(model_id));
    let response = authorize_provider(
        client.post(endpoint(&base_url, path)),
        descriptor,
        &key,
        secret,
    )
    .json(&Value::Object(payload))
    .send()
    .await
    .map_err(|error| transport_refusal(&error))?;
    let (status, _plan, text) = response_text(response).await?;
    if !status.is_success() {
        return Err(provider_refused(route_id, status, &text));
    }
    let mut body = typed_object(&text)?;
    if let Some(object) = body.as_object_mut() {
        object.insert("model".to_string(), Value::String(route_id.to_string()));
    }
    Ok(body)
}

/// The typed-decision wire: a state and a set of questions out, one typed
/// answer per question back.
///
/// Only a provider that declares a `decision_path` speaks it, and nothing is
/// translated here: the caller's `state` and `questions` travel as they were
/// written and the provider's answers come back whole. Rendering a decision
/// onto a chat model is a different thing and lives in
/// `crate::core::decisions`.
pub async fn dispatch_decision(
    route_id: &str,
    mut payload: Map<String, Value>,
    item: &str,
    secret: &str,
) -> Result<Value, Refusal> {
    let (descriptor, model_id) = typed_route(route_id)?;
    if descriptor.decision_path.is_empty() {
        return Err(Refusal::gateway(
            GatewayRefusal::ProviderFailure,
            format!("provider `{}` serves no typed decisions", descriptor.id),
        ));
    }
    let (key, base_url, client) = typed_transport(descriptor, item, secret)?;
    payload.insert("model".to_string(), Value::String(model_id));
    let response = authorize_provider(
        client.post(endpoint(&base_url, descriptor.decision_path)),
        descriptor,
        &key,
        secret,
    )
    .json(&Value::Object(payload))
    .send()
    .await
    .map_err(|error| transport_refusal(&error))?;
    let (status, _plan, text) = response_text(response).await?;
    if !status.is_success() {
        return Err(provider_refused(route_id, status, &text));
    }
    typed_object(&text)
}

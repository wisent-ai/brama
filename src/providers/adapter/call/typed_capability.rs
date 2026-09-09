//! The two OpenAI capabilities that are not chat, with their own route check.

use serde_json::{Map, Value};

use super::super::registry::{
    endpoint, provider_base_url, route, supports_embedding_route, supports_moderation_route,
};
use super::credential::{authorize_provider, provider_credential_key};
use super::dispatch_client;
use super::refusal::{provider_error, transport_error_message};
use super::response_body::bounded_response_text;

pub async fn dispatch_openai_typed(
    route_id: &str,
    path: &str,
    mut payload: Map<String, Value>,
    item: &str,
    secret: &str,
) -> Result<Value, String> {
    let supported = match path {
        "/v1/embeddings" => supports_embedding_route(route_id),
        "/v1/moderations" => supports_moderation_route(route_id),
        _ => false,
    };
    if !supported {
        return Err("model route does not support the requested capability".to_string());
    }
    let (descriptor, model_id) =
        route(route_id).ok_or_else(|| "invalid provider/model route".to_string())?;
    let key = provider_credential_key(descriptor, item, secret)?;
    let base_url = provider_base_url(descriptor)?;
    let client = dispatch_client()
        .map_err(|_| "dependency_unavailable: provider client could not be built".to_string())?;
    payload.insert("model".to_string(), Value::String(model_id.to_string()));
    let response = authorize_provider(
        client.post(endpoint(&base_url, path)),
        descriptor,
        &key,
        secret,
    )
    .json(&Value::Object(payload))
    .send()
    .await
    .map_err(|error| transport_error_message(&error))?;
    let (status, _plan, text) = bounded_response_text(response).await?;
    if !status.is_success() {
        let failure = provider_error(route_id, status, &text);
        return Err(failure
            .error
            .unwrap_or_else(|| format!("provider returned HTTP {}", status.as_u16())));
    }
    let mut body: Value = serde_json::from_str(&text)
        .map_err(|_| "provider_failure: provider returned malformed JSON".to_string())?;
    let object = body
        .as_object_mut()
        .ok_or_else(|| "provider_failure: provider returned a non-object response".to_string())?;
    object.insert("model".to_string(), Value::String(route_id.to_string()));
    Ok(body)
}

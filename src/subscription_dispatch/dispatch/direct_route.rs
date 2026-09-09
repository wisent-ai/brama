//! Caller-independent routes, paid for by this gateway's own provider
//! capability. One route, one attempt, and its refusal is the answer.

use serde_json::Value;

use crate::gateway::broker;
use crate::providers::adapter as provider_registry;
use crate::types::{ModelRequest, ModelResponse};

use super::catalogue::route::{provider_for, provider_requires_caller_identity};
use super::routed_stream::RoutedStream;

/// Execute a caller-independent canonical route with Brama's dedicated direct
/// provider capability. Subscription provider credentials are never eligible.
pub async fn dispatch_direct(request: &ModelRequest) -> ModelResponse {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return ModelResponse::failure(&request.model, "unknown provider/model route".into())
        }
    };
    if provider_requires_caller_identity(&request.model) {
        return ModelResponse::failure(
            &request.model,
            "auth: caller identity is required for subscription providers".into(),
        );
    }
    let credential = match broker::provider_credential(provider).await {
        Some(credential) => credential,
        None => {
            return ModelResponse::failure(
                &request.model,
                format!("direct '{provider}' credential is unavailable"),
            )
        }
    };
    let credential = match credential.expose_utf8() {
        Ok(credential) => credential,
        Err(_) => {
            return ModelResponse::failure(
                &request.model,
                format!("direct '{provider}' credential is not valid UTF-8"),
            )
        }
    };
    provider_registry::dispatch(request, &broker::provider_resource(provider), credential).await
}

pub async fn dispatch_direct_openai_typed(
    route_id: &str,
    path: &str,
    payload: serde_json::Map<String, Value>,
) -> Result<Value, String> {
    let provider =
        provider_for(route_id).ok_or_else(|| "unknown provider/model route".to_string())?;
    if provider_requires_caller_identity(route_id) {
        return Err("auth: caller identity is required for subscription providers".to_string());
    }
    let credential = broker::provider_credential(provider)
        .await
        .ok_or_else(|| format!("direct '{provider}' credential is unavailable"))?;
    let credential = credential
        .expose_utf8()
        .map_err(|_| format!("direct '{provider}' credential is not valid UTF-8"))?;
    provider_registry::dispatch_openai_typed(
        route_id,
        path,
        payload,
        &broker::provider_resource(provider),
        credential,
    )
    .await
}

/// Open one streaming generation on a direct route: one provider attempt, no
/// rotation, no subscription credential ever eligible.
pub async fn dispatch_direct_stream(request: &ModelRequest) -> Result<RoutedStream, ModelResponse> {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return Err(ModelResponse::failure(
                &request.model,
                "unknown provider/model route".into(),
            ))
        }
    };
    if provider_requires_caller_identity(&request.model) {
        return Err(ModelResponse::failure(
            &request.model,
            "auth: caller identity is required for subscription providers".into(),
        ));
    }
    let credential = match broker::provider_credential(provider).await {
        Some(credential) => credential,
        None => {
            return Err(ModelResponse::failure(
                &request.model,
                format!("direct '{provider}' credential is unavailable"),
            ))
        }
    };
    let credential = match credential.expose_utf8() {
        Ok(credential) => credential,
        Err(_) => {
            return Err(ModelResponse::failure(
                &request.model,
                format!("direct '{provider}' credential is not valid UTF-8"),
            ))
        }
    };
    let stream = provider_registry::dispatch_stream(
        request,
        &broker::provider_resource(provider),
        credential,
    )
    .await?;
    Ok(RoutedStream {
        model: request.model.clone(),
        attempts: u32::from(true),
        events: stream.events,
    })
}

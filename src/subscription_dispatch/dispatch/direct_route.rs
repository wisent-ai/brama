//! Caller-independent routes, paid for by this gateway's own provider
//! capability. One route, one attempt, and its refusal is the answer.

use serde_json::Value;

use crate::gateway::broker;
use crate::providers::adapter as provider_registry;
use crate::types::{GatewayRefusal, ModelRequest, ModelResponse, Refusal};

use super::catalogue::route::{provider_for, provider_requires_caller_identity};
use super::routed_stream::RoutedStream;

/// Execute a caller-independent canonical route with Brama's dedicated direct
/// provider capability. Subscription provider credentials are never eligible.
pub async fn dispatch_direct(request: &ModelRequest) -> ModelResponse {
    match direct_credential(&request.model).await {
        Ok((provider, credential)) => {
            provider_registry::dispatch(request, &broker::provider_resource(&provider), &credential)
                .await
        }
        Err(refused) => ModelResponse::from_refusal(&request.model, refused),
    }
}

pub async fn dispatch_direct_openai_typed(
    route_id: &str,
    path: &str,
    payload: serde_json::Map<String, Value>,
) -> Result<Value, Refusal> {
    let (provider, credential) = direct_credential(route_id).await?;
    provider_registry::dispatch_openai_typed(
        route_id,
        path,
        payload,
        &broker::provider_resource(&provider),
        &credential,
    )
    .await
}

/// One image generation on a route this deployment pays for.
///
/// Media is never funded by a subscription: a Claude Code or ChatGPT plan
/// carries no image or video quota, and charging a caller's plan for a render
/// it cannot see on its own bill is the one thing the entitlement contract
/// exists to prevent.
pub async fn dispatch_direct_image(
    route_id: &str,
    payload: serde_json::Map<String, Value>,
) -> Result<Value, Refusal> {
    let (provider, credential) = direct_credential(route_id).await?;
    provider_registry::dispatch_image(
        route_id,
        payload,
        &broker::provider_resource(&provider),
        &credential,
    )
    .await
}

/// Start one video job on a route this deployment pays for.
pub async fn dispatch_direct_video(
    route_id: &str,
    payload: serde_json::Map<String, Value>,
) -> Result<Value, Refusal> {
    let (provider, credential) = direct_credential(route_id).await?;
    provider_registry::dispatch_video(
        route_id,
        payload,
        &broker::provider_resource(&provider),
        &credential,
    )
    .await
}

/// Read one started video job back from the provider that started it.
pub async fn dispatch_direct_video_status(route_id: &str, job_id: &str) -> Result<Value, Refusal> {
    let (provider, credential) = direct_credential(route_id).await?;
    provider_registry::dispatch_video_status(
        route_id,
        job_id,
        &broker::provider_resource(&provider),
        &credential,
    )
    .await
}

/// Speak one text on a route this deployment pays for. The answer is audio
/// as the provider encoded it, not JSON.
pub async fn dispatch_direct_speech(
    route_id: &str,
    payload: serde_json::Map<String, Value>,
) -> Result<provider_registry::SpokenAudio, Refusal> {
    let (provider, credential) = direct_credential(route_id).await?;
    provider_registry::dispatch_speech(
        route_id,
        payload,
        &broker::provider_resource(&provider),
        &credential,
    )
    .await
}

/// Compose one song on a route this deployment pays for. The answer is audio.
pub async fn dispatch_direct_music(
    route_id: &str,
    payload: serde_json::Map<String, Value>,
) -> Result<provider_registry::SpokenAudio, Refusal> {
    let (provider, credential) = direct_credential(route_id).await?;
    provider_registry::dispatch_music(
        route_id,
        payload,
        &broker::provider_resource(&provider),
        &credential,
    )
    .await
}

/// The voices the deployment's account on this route can speak with.
pub async fn dispatch_direct_voices(route_id: &str) -> Result<Value, Refusal> {
    let (provider, credential) = direct_credential(route_id).await?;
    provider_registry::dispatch_voices(route_id, &broker::provider_resource(&provider), &credential)
        .await
}

/// Clone one voice on the deployment's account on this route.
pub async fn dispatch_direct_voice_clone(
    route_id: &str,
    name: &str,
    description: Option<&str>,
    samples: Vec<provider_registry::VoiceSample>,
) -> Result<Value, Refusal> {
    let (provider, credential) = direct_credential(route_id).await?;
    provider_registry::dispatch_voice_clone(
        route_id,
        name,
        description,
        samples,
        &broker::provider_resource(&provider),
        &credential,
    )
    .await
}

/// Delete one voice from the deployment's account on this route.
pub async fn dispatch_direct_voice_delete(route_id: &str, voice_id: &str) -> Result<Value, Refusal> {
    let (provider, credential) = direct_credential(route_id).await?;
    provider_registry::dispatch_voice_delete(
        route_id,
        voice_id,
        &broker::provider_resource(&provider),
        &credential,
    )
    .await
}

/// The deployment's own credential for one canonical route, with the
/// refusals every direct path shares, each with its class: a route no
/// provider here serves, a route whose provider is only reachable with a
/// caller's own subscription, and a credential this deployment cannot read.
async fn direct_credential(route_id: &str) -> Result<(String, String), Refusal> {
    let provider = provider_for(route_id)
        .ok_or_else(|| {
            Refusal::gateway(
                GatewayRefusal::InvalidRequest,
                "unknown provider/model route",
            )
        })?
        .to_owned();
    if provider_requires_caller_identity(route_id) {
        return Err(Refusal::gateway(
            GatewayRefusal::Unauthenticated,
            "auth: caller identity is required for subscription providers",
        ));
    }
    let credential = broker::provider_credential(&provider)
        .await
        .ok_or_else(|| {
            Refusal::gateway(
                GatewayRefusal::DependencyUnavailable,
                format!("direct '{provider}' credential is unavailable"),
            )
        })?;
    let credential = credential
        .expose_utf8()
        .map_err(|_| {
            Refusal::gateway(
                GatewayRefusal::ProviderFailure,
                format!("direct '{provider}' credential is not valid UTF-8"),
            )
        })?
        .to_string();
    Ok((provider, credential))
}

/// One typed decision on a provider that speaks the decision wire itself,
/// paid by this gateway's own capability. A subscription credential is never
/// eligible here for the same reason it is not on the other typed paths: the
/// decision is the deployment's call, not a caller's plan quota.
pub async fn dispatch_direct_decision(
    route_id: &str,
    payload: serde_json::Map<String, Value>,
) -> Result<Value, Refusal> {
    let (provider, credential) = direct_credential(route_id).await?;
    provider_registry::dispatch_decision(
        route_id,
        payload,
        &broker::provider_resource(&provider),
        &credential,
    )
    .await
}

/// Open one streaming generation on a direct route: one provider attempt, no
/// rotation, no subscription credential ever eligible.
pub async fn dispatch_direct_stream(request: &ModelRequest) -> Result<RoutedStream, ModelResponse> {
    let (provider, credential) = direct_credential(&request.model)
        .await
        .map_err(|refused| ModelResponse::from_refusal(&request.model, refused))?;
    let stream = provider_registry::dispatch_stream(
        request,
        &broker::provider_resource(&provider),
        &credential,
    )
    .await?;
    Ok(RoutedStream {
        model: request.model.clone(),
        attempts: u32::from(true),
        events: stream.events,
    })
}

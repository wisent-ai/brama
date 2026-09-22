//! Native Wisent provider registry for credentials redeemed from Skarbiec.
//!
//! This module is intentionally independent from external agent harnesses. It owns
//! provider discovery, request shaping and response normalization for API-backed
//! subscriptions. Secrets are passed in-memory by the Skarbiec capability broker
//! and are never persisted here.

mod call;
mod catalog;
mod dialect;
mod plan;
mod registry;

use std::time::Instant;

use serde_json::{json, Value};

use crate::subscription_dispatch::model_catalog;
use crate::types::{ModelRequest, ModelResponse};

use call::credential::{
    authorize_catalog, authorize_provider, provider_body, provider_credential_key,
};
use call::outcome::refusal::{attempted_failure, provider_error, transport_failure};
use call::outcome::response_body::bounded_response_text;
use call::outcome::retry::send_once_more_if_unsent;
use call::{dispatch_client, stream_client};
use catalog::dispatch_catalog;
use catalog::endpoint::{catalog_endpoint, catalog_provider_base_url};
use catalog::model_row::catalog_model_from_value;
use dialect::anthropic_messages::model_response_from_anthropic;
use dialect::openai_chat::model_response_from_openai;
use dialect::openai_responses::event_stream::model_response_from_responses_stream;
use dialect::{chat_payload, refused_settings, streaming_chat_payload};
use plan::headers::{limit_readings, plan_headers, with_limits};
use registry::{
    apply_omp_model_metadata, endpoint, model_from_value, provider_base_url, provider_base_url_for,
};

pub use call::control_client;
pub use call::media::{
    dispatch_image, dispatch_speech, dispatch_video, dispatch_video_status, SpokenAudio,
};
pub use call::typed_capability::{dispatch_decision, dispatch_openai_typed};
pub use plan::probe::plan_probe_route;
pub use plan::{publishes_plan_usage, read_plan_usage, PlanUsage};
pub use registry::{
    kind_from_output, native_decision_route, provider, provider_endpoint, provider_id_from_route,
    providers, route, supports_chat_route, supports_decision_route, supports_embedding_route,
    supports_image_route, supports_moderation_route, supports_speech_route, supports_video_route,
    valid_provider_id, AuthKind, ModelKind, ProviderDescriptor, RegistryModel, WireProtocol,
};

pub(crate) use call::credential::credential_key;
pub(crate) use registry::provider_requires_credential;



pub use catalog::discovery::discover_models;

pub async fn dispatch(request: &ModelRequest, item: &str, secret: &str) -> ModelResponse {
    let Some((descriptor, model_id)) = route(&request.model) else {
        return dispatch_catalog(request, item, secret).await;
    };
    // A provider that generates no text has no chat endpoint to send this to.
    // Naming one of its routes on a chat call is the caller's mistake, and it
    // is answered here rather than as a 404 from a URL this build invented.
    if descriptor.chat_path.is_empty() {
        return ModelResponse::failure(
            &request.model,
            format!(
                "invalid_request: route `{}` serves typed decisions only; call POST /v1/decisions",
                request.model
            ),
        );
    }
    let key = match provider_credential_key(descriptor, item, secret) {
        Ok(key) => key,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    let base_url = match provider_base_url_for(descriptor, &model_id) {
        Ok(base_url) => base_url,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    let client = match dispatch_client() {
        Ok(client) => client,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    let payload = chat_payload(descriptor, model_id.as_ref(), request);
    let started = Instant::now();
    let response = match send_once_more_if_unsent(provider_body(
        authorize_provider(
            client.post(endpoint(&base_url, descriptor.chat_path)),
            descriptor,
            &key,
            secret,
        ),
        descriptor,
        payload,
        request,
    ))
    .await
    {
        Ok(response) => response,
        Err(error) => return transport_failure(&request.model, &error),
    };
    let (status, plan, text) = match bounded_response_text(response).await {
        Ok(result) => result,
        Err(message) => return attempted_failure(&request.model, message),
    };
    let limits = limit_readings(descriptor.id, &plan);
    if !status.is_success() {
        // The provider just said this model takes no temperature: the same
        // request goes once more without it. `learn` is true only the first
        // time per model, so this recursion ends.
        if refused_settings::learn_refused_temperature(model_id.as_ref(), status, &text) {
            return Box::pin(dispatch(request, item, secret)).await;
        }
        return with_limits(provider_error(&request.model, status, &text), limits);
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    if descriptor.wire == WireProtocol::OpenAiResponses {
        return with_limits(
            model_response_from_responses_stream(&request.model, &text, elapsed_ms),
            limits,
        );
    }
    let body = match serde_json::from_str::<Value>(&text) {
        Ok(body) => body,
        Err(error) => {
            return attempted_failure(
                &request.model,
                format!("invalid provider response: {error}"),
            )
        }
    };
    with_limits(
        match descriptor.wire {
            WireProtocol::OpenAiChat => {
                model_response_from_openai(&request.model, body, elapsed_ms)
            }
            WireProtocol::AnthropicMessages => {
                model_response_from_anthropic(&request.model, body, elapsed_ms)
            }
            // Both are refused before a request is built: the Responses wire
            // returns above, and a decision-only provider has no chat path.
            WireProtocol::OpenAiResponses | WireProtocol::TypeSafeSystemOne => unreachable!(),
        },
        limits,
    )
}

/// Open one streaming provider generation.
///
/// `Ok` is the commit point: the provider answered with a success status, the
/// plan windows its headers carried are in `limits`, and generation events
/// arrive on `events`. `Err` is the same failure the buffered path would have
/// returned, made a different type because nothing was sent to any caller yet
/// and rotation is still possible. After `Ok` nothing in this process retries:
/// bytes may already be with the caller, and a second attempt would double
/// both the bill and the text.
pub async fn dispatch_stream(
    request: &ModelRequest,
    item: &str,
    secret: &str,
) -> Result<crate::providers::stream::ProviderStream, ModelResponse> {
    let Some((descriptor, model_id)) = route(&request.model) else {
        return Err(ModelResponse::failure(
            &request.model,
            "streaming is supported for provider routes only".to_string(),
        ));
    };
    if descriptor.chat_path.is_empty() {
        return Err(ModelResponse::failure(
            &request.model,
            format!(
                "invalid_request: route `{}` serves typed decisions only; call POST /v1/decisions",
                request.model
            ),
        ));
    }
    let key = match provider_credential_key(descriptor, item, secret) {
        Ok(key) => key,
        Err(error) => return Err(ModelResponse::failure(&request.model, error)),
    };
    let base_url = match provider_base_url_for(descriptor, &model_id) {
        Ok(base_url) => base_url,
        Err(error) => return Err(ModelResponse::failure(&request.model, error)),
    };
    let client = match stream_client() {
        Ok(client) => client,
        Err(error) => return Err(ModelResponse::failure(&request.model, error)),
    };
    let payload = streaming_chat_payload(descriptor, model_id.as_ref(), request);
    let response = match send_once_more_if_unsent(provider_body(
        authorize_provider(
            client.post(endpoint(&base_url, descriptor.chat_path)),
            descriptor,
            &key,
            secret,
        ),
        descriptor,
        payload,
        request,
    ))
    .await
    {
        Ok(response) => response,
        Err(error) => return Err(transport_failure(&request.model, &error)),
    };
    if !response.status().is_success() {
        let (status, plan, text) = match bounded_response_text(response).await {
            Ok(result) => result,
            Err(message) => return Err(attempted_failure(&request.model, message)),
        };
        // Nothing has reached the caller yet, so the one more send without
        // the refused setting is still possible here.
        if refused_settings::learn_refused_temperature(model_id.as_ref(), status, &text) {
            return Box::pin(dispatch_stream(request, item, secret)).await;
        }
        let limits = limit_readings(descriptor.id, &plan);
        return Err(with_limits(
            provider_error(&request.model, status, &text),
            limits,
        ));
    }
    let limits = limit_readings(descriptor.id, &plan_headers(response.headers()));
    Ok(crate::providers::stream::ProviderStream {
        limits,
        events: crate::providers::stream::spawn(descriptor.wire, response),
    })
}

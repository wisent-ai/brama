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

use call::credential::{authorize_catalog, authorize_provider, provider_credential_key};
use call::refusal::{attempted_failure, provider_error, transport_failure};
use call::response_body::bounded_response_text;
use call::retry::send_once_more_if_unsent;
use call::{dispatch_client, stream_client};
use catalog::dispatch_catalog;
use catalog::endpoint::{catalog_endpoint, catalog_provider_base_url};
use catalog::model_row::catalog_model_from_value;
use dialect::anthropic_messages::model_response_from_anthropic;
use dialect::openai_chat::model_response_from_openai;
use dialect::openai_responses::event_stream::model_response_from_responses_stream;
use dialect::{chat_payload, streaming_chat_payload};
use plan::headers::{limit_readings, plan_headers, with_limits};
use registry::{
    apply_omp_model_metadata, endpoint, model_from_value, provider_base_url, provider_base_url_for,
};

pub use call::control_client;
pub use call::typed_capability::dispatch_openai_typed;
pub use plan::probe::plan_probe_route;
pub use plan::{publishes_plan_usage, read_plan_usage, PlanUsage};
pub use registry::{
    provider, provider_id_from_route, providers, route, supports_chat_route,
    supports_embedding_route, supports_moderation_route, AuthKind, ProviderDescriptor,
    RegistryModel, WireProtocol,
};

pub(crate) use call::credential::credential_key;
pub(crate) use registry::provider_requires_credential;

pub async fn discover_models(
    provider_id: &str,
    item: &str,
    secret: &str,
) -> Result<Vec<RegistryModel>, String> {
    let catalog = model_catalog::snapshot().await?;
    if let Some(descriptor) = catalog.providers.get(provider_id) {
        if !descriptor.executable() {
            return Err(format!(
                "provider `{provider_id}` uses a protocol not implemented by Brama"
            ));
        }
        let mut models = catalog
            .models
            .iter()
            .filter(|model| model.provider_id == provider_id)
            .cloned()
            .collect::<Vec<_>>();
        let key = credential_key(item, secret)?;
        let base_url = catalog_provider_base_url(descriptor)?;
        let client = control_client()?;
        let request = authorize_catalog(
            client.get(catalog_endpoint(&base_url, "/models")),
            descriptor,
            &key,
        );
        if let Ok(response) = request.send().await {
            if response.status().is_success() {
                if let Ok(body) = response.json::<Value>().await {
                    let dynamic = body
                        .get("data")
                        .or_else(|| body.get("models"))
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    models.extend(
                        dynamic
                            .iter()
                            .filter_map(|row| catalog_model_from_value(provider_id, row)),
                    );
                }
            }
        }
        models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
        models.dedup_by(|left, right| left.route_id == right.route_id);
        if models.is_empty() {
            return Err(format!("provider `{provider_id}` has no catalog models"));
        }
        return Ok(models);
    }

    let descriptor = provider(provider_id)
        .ok_or_else(|| format!("provider `{provider_id}` is not in the Wisent registry"))?;
    let key = provider_credential_key(descriptor, item, secret)?;
    let base_url = provider_base_url(descriptor)?;
    let client = control_client()?;
    let request = authorize_provider(
        client.get(endpoint(&base_url, descriptor.models_path)),
        descriptor,
        &key,
        secret,
    );
    let dynamic = match request.send().await {
        Ok(response) if response.status().is_success() => response
            .json::<Value>()
            .await
            .ok()
            .and_then(|body| {
                body.get("data")
                    .or_else(|| body.get("models"))
                    .and_then(Value::as_array)
                    .cloned()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut models = dynamic
        .iter()
        .filter_map(|row| model_from_value(descriptor, row))
        .collect::<Vec<_>>();
    models.extend(
        descriptor
            .static_models
            .iter()
            .filter_map(|id| model_from_value(descriptor, &json!({"id": id}))),
    );
    models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
    models.dedup_by(|left, right| left.route_id == right.route_id);
    apply_omp_model_metadata(provider_id, &mut models);
    if models.is_empty() {
        return Err(format!(
            "provider `{provider_id}` returned no models and this build carries no substitute \
             model list for it"
        ));
    }
    Ok(models)
}

pub async fn dispatch(request: &ModelRequest, item: &str, secret: &str) -> ModelResponse {
    let Some((descriptor, model_id)) = route(&request.model) else {
        return dispatch_catalog(request, item, secret).await;
    };
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
    let response = match send_once_more_if_unsent(
        authorize_provider(
            client.post(endpoint(&base_url, descriptor.chat_path)),
            descriptor,
            &key,
            secret,
        )
        .json(&payload),
    )
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
            WireProtocol::OpenAiResponses => unreachable!(),
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
    let response = match send_once_more_if_unsent(
        authorize_provider(
            client.post(endpoint(&base_url, descriptor.chat_path)),
            descriptor,
            &key,
            secret,
        )
        .json(&payload),
    )
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

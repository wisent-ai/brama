//! Sending a request to a provider this build knows only from the model
//! catalog.

pub(in crate::providers::adapter) mod endpoint;
pub(in crate::providers::adapter) mod google_generate;
pub(in crate::providers::adapter) mod model_row;

use std::time::Instant;

use serde_json::{json, Map, Value};

use super::call::credential::{authorize_catalog, credential_key};
use super::call::dispatch_client;
use super::call::refusal::{attempted_failure, provider_error, transport_failure};
use super::call::response_body::bounded_response_text;
use super::call::retry::send_once_more_if_unsent;
use super::dialect::anthropic_messages::{
    anthropic_messages, anthropic_tool_choice, anthropic_tools, model_response_from_anthropic,
};
use super::dialect::openai_chat::{model_response_from_openai, openai_messages};
use super::dialect::tool_schema::normalized_tools_value;
use super::plan::headers::{limit_readings, with_limits};
use super::registry::{valid_model_id, valid_provider_id};
use crate::subscription_dispatch::model_catalog::{self, CatalogProtocol};
use crate::types::{ModelRequest, ModelResponse};
use endpoint::{catalog_endpoint, catalog_provider_base_url};
use google_generate::{google_payload, model_response_from_google};

pub(in crate::providers::adapter) async fn dispatch_catalog(
    request: &ModelRequest,
    item: &str,
    secret: &str,
) -> ModelResponse {
    let Some((provider_id, model_id)) = request.model.split_once('/') else {
        return ModelResponse::failure(&request.model, "invalid provider/model route".into());
    };
    if !valid_provider_id(provider_id) || !valid_model_id(model_id) {
        return ModelResponse::failure(&request.model, "invalid provider/model route".into());
    }
    let catalog = match model_catalog::snapshot().await {
        Ok(catalog) => catalog,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    let Some(descriptor) = catalog.providers.get(provider_id) else {
        return ModelResponse::failure(
            &request.model,
            format!("provider `{provider_id}` is not in the Wisent catalog"),
        );
    };
    if !descriptor.executable() {
        return ModelResponse::failure(
            &request.model,
            format!("provider `{provider_id}` uses an unsupported protocol"),
        );
    }
    if !catalog
        .models
        .iter()
        .any(|model| model.route_id == request.model)
    {
        return ModelResponse::failure(
            &request.model,
            format!(
                "model `{}` is not advertised by provider `{provider_id}`",
                model_id
            ),
        );
    }
    let key = match credential_key(item, secret) {
        Ok(key) => key,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    let base_url = match catalog_provider_base_url(descriptor) {
        Ok(base_url) => base_url,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    let client = match dispatch_client() {
        Ok(client) => client,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    let (url, payload) = match descriptor.protocol {
        CatalogProtocol::OpenAiChat => {
            let mut body = Map::new();
            body.insert("model".into(), json!(model_id));
            body.insert("messages".into(), Value::Array(openai_messages(request)));
            body.insert("max_tokens".into(), json!(request.max_tokens));
            body.insert("temperature".into(), json!(request.temperature));
            if let Some(tools) = &request.tools {
                body.insert("tools".into(), normalized_tools_value(tools));
            }
            if let Some(choice) = &request.tool_choice {
                body.insert("tool_choice".into(), choice.clone());
            }
            (
                catalog_endpoint(&base_url, "/chat/completions"),
                Value::Object(body),
            )
        }
        CatalogProtocol::AnthropicMessages => {
            let mut body = json!({
                "model": model_id,
                "messages": anthropic_messages(request),
                "max_tokens": request.max_tokens,
                "temperature": request.temperature,
            });
            if let Some(system) = request.system.as_deref().filter(|value| !value.is_empty()) {
                body["system"] = json!(system);
            }
            if let Some(tools) = anthropic_tools(request) {
                body["tools"] = json!(tools);
            }
            if let Some(choice) = request.tool_choice.as_ref().and_then(anthropic_tool_choice) {
                body["tool_choice"] = choice;
            }
            (catalog_endpoint(&base_url, "/messages"), body)
        }
        CatalogProtocol::GoogleGenerateContent => {
            let mut url = match reqwest::Url::parse(&base_url) {
                Ok(url) => url,
                Err(error) => return ModelResponse::failure(&request.model, error.to_string()),
            };
            match url.path_segments_mut() {
                Ok(mut segments) => {
                    segments.pop_if_empty();
                    segments.push("models");
                    segments.push(&format!("{model_id}:generateContent"));
                }
                Err(()) => {
                    return ModelResponse::failure(
                        &request.model,
                        "provider API endpoint cannot be a base URL".into(),
                    )
                }
            }
            (url.to_string(), google_payload(request))
        }
        CatalogProtocol::Unsupported => unreachable!(),
    };
    let started = Instant::now();
    let response = match send_once_more_if_unsent(
        authorize_catalog(client.post(url), descriptor, &key).json(&payload),
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
    let limits = limit_readings(&descriptor.id, &plan);
    if !status.is_success() {
        return with_limits(provider_error(&request.model, status, &text), limits);
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
    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    with_limits(
        match descriptor.protocol {
            CatalogProtocol::OpenAiChat => {
                model_response_from_openai(&request.model, body, elapsed_ms)
            }
            CatalogProtocol::AnthropicMessages => {
                model_response_from_anthropic(&request.model, body, elapsed_ms)
            }
            CatalogProtocol::GoogleGenerateContent => {
                model_response_from_google(&request.model, body, elapsed_ms)
            }
            CatalogProtocol::Unsupported => unreachable!(),
        },
        limits,
    )
}

//! The same entrypoints for one streaming generation: every selector decision
//! is made before the first caller byte, because after it none can be.

use axum::http::HeaderMap;

use crate::types::{ModelRequest, ModelResponse};

use super::super::caller_identity::authenticate_agent;
use super::super::catalogue::route::provider_for;
use super::super::ranking::candidates::{
    active_supported_models_for_agent, active_vision_capable_models_for_agent,
    best_subscription_models,
};
use super::super::ranking::task_quality::task_quality_models;
use super::super::rotation::streaming::attempt_subscription_stream;
use super::super::routed_stream::RoutedStream;
use super::ranked_walk::{
    dispatch_ranked_models_stream, ANY_SUBSCRIPTION_CONTEXT, ANY_VISION_CONTEXT,
};

/// Authenticate the Jeden caller and open one streaming subscription
/// generation, rotating across bounded credentials exactly as the buffered
/// path does -- except every rotation happens before the first caller byte,
/// because after it none is possible.
pub async fn dispatch_subscription_stream(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> Result<RoutedStream, ModelResponse> {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return Err(ModelResponse::failure(
                &request.model,
                "unknown provider/model route".into(),
            ))
        }
    };
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(error) => return Err(ModelResponse::failure(&request.model, error)),
    };
    attempt_subscription_stream(provider, &agent_id, request)
        .await
        .opened
}

pub async fn dispatch_subscription_stream_for_agent(
    agent_id: &str,
    request: &ModelRequest,
) -> Result<RoutedStream, ModelResponse> {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return Err(ModelResponse::failure(
                &request.model,
                "unknown provider/model route".into(),
            ))
        }
    };
    attempt_subscription_stream(provider, agent_id, request)
        .await
        .opened
}

pub async fn dispatch_any_subscription_stream(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> Result<RoutedStream, ModelResponse> {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    let models = match active_supported_models_for_agent(&agent_id).await {
        Ok(models) => models,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    dispatch_ranked_models_stream(&agent_id, request, models, ANY_SUBSCRIPTION_CONTEXT).await
}

/// The streaming counterpart of
/// [`dispatch_best_subscription`](super::buffered::dispatch_best_subscription):
/// the configured route leads, and a provider that cannot open a stream is
/// walked past exactly as the buffered path walks past one that cannot answer.
pub async fn dispatch_best_subscription_stream(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
    preferred: Option<&str>,
) -> Result<RoutedStream, ModelResponse> {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    dispatch_best_subscription_stream_for_agent(&agent_id, request, preferred).await
}

pub async fn dispatch_best_subscription_stream_for_agent(
    agent_id: &str,
    request: &ModelRequest,
    preferred: Option<&str>,
) -> Result<RoutedStream, ModelResponse> {
    let models = match best_subscription_models(agent_id, preferred).await {
        Ok(models) => models,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    dispatch_ranked_models_stream(agent_id, request, models, ANY_SUBSCRIPTION_CONTEXT).await
}

pub async fn dispatch_any_vision_capable_subscription_stream(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> Result<RoutedStream, ModelResponse> {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    let models = match active_vision_capable_models_for_agent(&agent_id).await {
        Ok(models) => models,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    dispatch_ranked_models_stream(&agent_id, request, models, ANY_VISION_CONTEXT).await
}

pub async fn dispatch_task_subscription_stream(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
    task: &str,
) -> Result<RoutedStream, ModelResponse> {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(id) => id,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    let models = match task_quality_models(&agent_id, task).await {
        Ok(models) => models,
        Err(e) => return Err(ModelResponse::failure(&request.model, e)),
    };
    dispatch_ranked_models_stream(
        &agent_id,
        request,
        models,
        &format!("no working quality-ranked model for task '{task}'"),
    )
    .await
}

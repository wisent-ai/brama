//! What the HTTP edge calls for one buffered generation: a named route, or one
//! of the selectors that decide the route for the caller.

use axum::http::HeaderMap;

use crate::types::{ModelRequest, ModelResponse};

use super::super::caller_identity::authenticate_agent;
use super::super::catalogue::route::provider_for;
use super::super::ranking::candidates::{
    active_supported_models_for_agent, active_vision_capable_models_for_agent,
    best_subscription_models,
};
use super::super::ranking::task_quality::task_quality_models;
use super::super::rotation::buffered::dispatch_subscription_attempt;
use super::ranked_walk::{
    dispatch_ranked_models, ANY_SUBSCRIPTION_CONTEXT, ANY_VISION_CONTEXT,
};

/// `model: "any"` selects among active stateless provider routes for the
/// signed agent and rotates across credentials on provider exhaustion.
pub async fn dispatch_any_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    let models = match active_supported_models_for_agent(&agent_id).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_ranked_models(&agent_id, request, models, ANY_SUBSCRIPTION_CONTEXT).await
}

/// `best` is a selector, not a route: it means the best subscription model this
/// signed caller can actually be served from right now.
///
/// The alias resolves to one configured provider route and that route leads the
/// list, but it has never been the only thing `best` may answer with. Dispatching
/// it alone is what turned one unredeemable codex credential into a `503` for a
/// fleet holding three live subscription providers.
pub async fn dispatch_best_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
    preferred: Option<&str>,
) -> ModelResponse {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_best_subscription_for_agent(&agent_id, request, preferred).await
}

pub async fn dispatch_best_subscription_for_agent(
    agent_id: &str,
    request: &ModelRequest,
    preferred: Option<&str>,
) -> ModelResponse {
    let models = match best_subscription_models(agent_id, preferred).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_ranked_models(agent_id, request, models, ANY_SUBSCRIPTION_CONTEXT).await
}

/// `model: "any-vision-capable"` selects an active stateless provider route
/// whose catalog metadata advertises image input.
pub async fn dispatch_any_vision_capable_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    let models = match active_vision_capable_models_for_agent(&agent_id).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_ranked_models(&agent_id, request, models, ANY_VISION_CONTEXT).await
}

/// `model: "task:<name>"` means: use measured quality evidence for `<name>`.
/// The router does not infer tasks from prompt text. It only uses persisted
/// rows written by the task-quality collector.
pub async fn dispatch_task_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
    task: &str,
) -> ModelResponse {
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(id) => id,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    let models = match task_quality_models(&agent_id, task).await {
        Ok(models) => models,
        Err(e) => return ModelResponse::failure(&request.model, e),
    };
    dispatch_ranked_models(
        &agent_id,
        request,
        models,
        &format!("no working quality-ranked model for task '{task}'"),
    )
    .await
}

/// Authenticate the Jeden caller, redeem the selected provider credential at
/// the final-use boundary, and execute one stateless provider API request.
pub async fn dispatch_subscription(
    headers: &HeaderMap,
    request: &ModelRequest,
    raw_body: &[u8],
) -> ModelResponse {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return ModelResponse::failure(&request.model, "unknown provider/model route".into())
        }
    };
    let agent_id = match authenticate_agent(headers, raw_body).await {
        Ok(agent_id) => agent_id,
        Err(error) => return ModelResponse::failure(&request.model, error),
    };
    dispatch_subscription_attempt(provider, &agent_id, request).await
}

pub async fn dispatch_subscription_for_agent(
    agent_id: &str,
    request: &ModelRequest,
) -> ModelResponse {
    let provider = match provider_for(&request.model) {
        Some(provider) => provider,
        None => {
            return ModelResponse::failure(&request.model, "unknown provider/model route".into())
        }
    };
    dispatch_subscription_attempt(provider, agent_id, request).await
}

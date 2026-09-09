//! Signing one pooled account back in, from the three places the question is
//! asked: an account holder for its own subscription, the console for a named
//! agent's, and the console for a pooled account with no agent in the path.
//!
//! All three narrow to the same execution, which re-reads the account rather
//! than trusting the path: a retired or inactive subscription is refused, and a
//! Weles login item that disagrees with the stored one is a `409` rather than a
//! sign-in against the wrong identity.

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use crate::core::server::administration::require_brama_desktop;
use crate::core::server::admission::identity::{valid_agent_id, ModelClientIdentity};
use crate::core::server::refusal::{api_error, ApiError};

use super::account::account_agent_id;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct SignInSubscriptionRequest {
    subscription_id: Option<String>,
    reason: Option<String>,
    login_item: Option<String>,
    login_timeout_ms: Option<u64>,
}

pub(in crate::core::server) async fn sign_in_account_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Path(subscription_id): Path<String>,
    Json(request): Json<SignInSubscriptionRequest>,
) -> Result<Json<Value>, ApiError> {
    let agent_id = account_agent_id(&client_identity)?;
    sign_in_selected_subscription(Some(&agent_id), &subscription_id, request).await
}

pub(in crate::core::server) async fn sign_in_admin_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Path((agent_id, subscription_id)): Path<(String, String)>,
    Json(request): Json<SignInSubscriptionRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    if !valid_agent_id(&agent_id) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid agent id"));
    }
    sign_in_selected_subscription(Some(&agent_id), &subscription_id, request).await
}

pub(in crate::core::server) async fn sign_in_admin_pool_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(mut request): Json<SignInSubscriptionRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let subscription_id = request
        .subscription_id
        .take()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "a subscription id is required"))?;
    sign_in_selected_subscription(None, &subscription_id, request).await
}

async fn sign_in_selected_subscription(
    agent_id: Option<&str>,
    subscription_id: &str,
    request: SignInSubscriptionRequest,
) -> Result<Json<Value>, ApiError> {
    let reason = request
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "--reason must say why this sign-in is being run",
            )
        })?;
    if request
        .subscription_id
        .as_deref()
        .is_some_and(|id| id != subscription_id)
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "subscription id does not match the requested account",
        ));
    }
    let entries = match agent_id {
        Some(agent_id) => crate::gateway::broker::discover_subscriptions(agent_id).await,
        None => crate::gateway::broker::list_all_subscriptions().await,
    }
    .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.id == subscription_id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "subscription not found"))?;
    if entry.status != "active" || crate::journal::is_retired(&entry.id) {
        return Err(api_error(
            StatusCode::CONFLICT,
            "retired subscriptions cannot be signed in",
        ));
    }
    if let (Some(held), Some(asked)) = (entry.login_item.as_deref(), request.login_item.as_deref())
    {
        if held != asked.trim() {
            return Err(api_error(
                StatusCode::CONFLICT,
                &format!("subscription `{subscription_id}` uses Weles login item `{held}`, not `{asked}`"),
            ));
        }
    }
    crate::subscription_dispatch::sign_in::sign_in_provider(
        crate::subscription_dispatch::sign_in::SignInOptions {
            provider: entry.provider,
            subscription_id: Some(entry.id),
            login_item: request.login_item.or(entry.login_item),
            reason: reason.to_string(),
            login_timeout_ms: request.login_timeout_ms.unwrap_or(900_000),
        },
    )
    .await
    .map(Json)
    // A missing declaration is this deployment's own configuration, not a bad
    // gateway upstream: Desktop reads the status apart from the sentence, so
    // the two must not both say `502`.
    .map_err(|error| {
        let status = if error.blocked().is_some() {
            StatusCode::CONFLICT
        } else {
            StatusCode::BAD_GATEWAY
        };
        api_error(status, &error.to_string())
    })
}
